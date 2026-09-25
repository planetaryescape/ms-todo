//! `steps list|add|edit|check|uncheck|delete` and `links
//! list|add|edit|delete` (docs/blueprint/07-cli.md, D-055): a task's steps
//! and its link. Reads come from the task the daemon has cached; writes
//! are `ChangeTasks` requests on the one task, so they go through the
//! outbox and `undo` reverses them. Deleting asks first in a terminal and
//! needs `--yes` anywhere else, as `tasks delete` does.

use ms_todo_core::links::{openable, storable};
use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{Entity, LinkEdit, NewLink, ResponseData, TaskChange};
use serde_json::{Value, json};

use crate::args::{LinkArgs, LinksCommand, StepTargetArgs, StepsCommand, WriteArgs};
use crate::confirm::{can_prompt, confirm};
use crate::csv_columns::text;
use crate::daemon_client;
use crate::error::CliError;
use crate::link_commands;
use crate::output::{OutputFormat, Table, print_collection};
use crate::task_commands::{change_request, send};

pub const STEP_COLUMNS: &[&str] = &["index", "id", "text", "checked", "checked_at", "created"];

pub const STEPS_TABLE: Table = Table {
    headings: &["#", "DONE", "STEP", "ID"],
    row: |step| {
        vec![
            number(step),
            checkbox(step).to_owned(),
            text(step, "displayName").to_owned(),
            text(step, "id").to_owned(),
        ]
    },
    csv_headings: STEP_COLUMNS,
    csv_row: |step| {
        vec![
            number(step),
            text(step, "id").to_owned(),
            text(step, "displayName").to_owned(),
            (step.get("isChecked") == Some(&Value::Bool(true))).to_string(),
            text(step, "checkedDateTime").to_owned(),
            text(step, "createdDateTime").to_owned(),
        ]
    },
    bold_matches: None,
};

pub const LINK_COLUMNS: &[&str] = &["index", "id", "url", "name", "app", "external_id"];

pub const LINKS_TABLE: Table = Table {
    headings: &["#", "NAME", "URL", "APP", "ID"],
    row: |link| {
        vec![
            number(link),
            text(link, "displayName").to_owned(),
            text(link, "webUrl").to_owned(),
            text(link, "applicationName").to_owned(),
            text(link, "id").to_owned(),
        ]
    },
    csv_headings: LINK_COLUMNS,
    csv_row: |link| {
        vec![
            number(link),
            text(link, "id").to_owned(),
            text(link, "webUrl").to_owned(),
            text(link, "displayName").to_owned(),
            text(link, "applicationName").to_owned(),
            text(link, "externalId").to_owned(),
        ]
    },
    bold_matches: None,
};

pub async fn steps(
    paths: &Paths,
    command: StepsCommand,
    format: OutputFormat,
) -> Result<(), CliError> {
    match command {
        StepsCommand::List(task) => list(paths, task, "checklistItems", &STEPS_TABLE, format).await,
        StepsCommand::Add { task, steps, write } => {
            write_child(paths, task, TaskChange::AddSteps { steps }, write, format).await
        }
        StepsCommand::Edit {
            task,
            step,
            text,
            write,
        } => {
            write_child(
                paths,
                task,
                TaskChange::EditStep { step, text },
                write,
                format,
            )
            .await
        }
        StepsCommand::Check(args) => check(paths, args, true, format).await,
        StepsCommand::Uncheck(args) => check(paths, args, false, format).await,
        StepsCommand::Delete { steps, yes } => {
            let change = TaskChange::DeleteSteps { steps: steps.steps };
            delete(paths, steps.task, change, steps.write, yes, format).await
        }
    }
}

pub async fn links(
    paths: &Paths,
    command: LinksCommand,
    format: OutputFormat,
) -> Result<(), CliError> {
    match command {
        LinksCommand::List(task) => {
            list(paths, task, "linkedResources", &LINKS_TABLE, format).await
        }
        LinksCommand::Add {
            task,
            url,
            fields,
            write,
        } => {
            warn_unopenable(&url, format);
            let link = NewLink {
                url,
                name: fields.name,
                app: fields.app,
                external_id: fields.external_id,
            };
            write_child(paths, task, TaskChange::AddLink(link), write, format).await
        }
        LinksCommand::Edit {
            task,
            link,
            url,
            fields,
            write,
        } => {
            if let Some(url) = &url {
                warn_unopenable(url, format);
            }
            let edit = LinkEdit {
                link,
                url,
                name: fields.name,
                app: fields.app,
                external_id: fields.external_id,
            };
            write_child(paths, task, TaskChange::EditLink(edit), write, format).await
        }
        LinksCommand::Delete {
            task,
            link,
            yes,
            write,
        } => {
            delete(
                paths,
                task,
                TaskChange::DeleteLink { link },
                write,
                yes,
                format,
            )
            .await
        }
    }
}

/// The task's steps or links, each with `index` (its number from 1) and
/// Graph's fields.
async fn list(
    paths: &Paths,
    task: LinkArgs,
    collection: &str,
    table: &Table,
    format: OutputFormat,
) -> Result<(), CliError> {
    let (task, sync) = link_commands::task(paths, task).await?;
    let items = numbered(&task, collection);
    print_collection(format, &items, sync, table)
}

pub(crate) fn numbered(task: &Entity, collection: &str) -> Vec<Entity> {
    task.get(collection)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(at, child)| {
            let mut child = child.as_object()?.clone();
            child.insert("index".into(), json!(at + 1));
            Some(child)
        })
        .collect()
}

async fn check(
    paths: &Paths,
    args: StepTargetArgs,
    checked: bool,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = TaskChange::CheckSteps {
        steps: args.steps,
        checked,
    };
    write_child(paths, args.task, change, args.write, format).await
}

pub(crate) async fn write_child(
    paths: &Paths,
    task: LinkArgs,
    change: TaskChange,
    write: WriteArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let request = change_request(
        vec![task.task],
        task.list,
        None,
        change,
        write.dry_run,
        write.idempotency.idempotency_key,
    );
    send(paths, request, format).await
}

/// Delete steps, the link or attachments: at once with `--yes` or `--dry-run`; in a
/// terminal, after showing what goes and asking; anywhere else, not at
/// all (exit 2).
pub(crate) async fn delete(
    paths: &Paths,
    task: LinkArgs,
    change: TaskChange,
    write: WriteArgs,
    yes: bool,
    format: OutputFormat,
) -> Result<(), CliError> {
    if yes || write.dry_run {
        return write_child(paths, task, change, write, format).await;
    }
    if !can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "there's no terminal to ask in: pass --yes to delete (`ms-todo undo` brings it \
             back), or --dry-run to see what would be deleted"
                .into(),
        ));
    }
    let key = write.idempotency.idempotency_key;
    let preview = change_request(
        vec![task.task.clone()],
        task.list.clone(),
        None,
        change.clone(),
        true,
        None,
    );
    let plan = match daemon_client::ask(paths, preview).await? {
        ResponseData::Plan(plan) => plan,
        _ => return Err(crate::unexpected_response()),
    };
    let Some(target) = plan.targets.first() else {
        return Err(crate::unexpected_response());
    };
    // Named by ID from here, so what's deleted is what was shown.
    let ids: Vec<String> = plan
        .changes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|write| write["id"].as_str().map(str::to_owned))
        .collect();
    let (cached, _) = link_commands::task(
        paths,
        LinkArgs {
            task: target.id.clone(),
            list: None,
        },
    )
    .await?;
    let (collection, noun, label) = match change {
        TaskChange::DeleteLink { .. } => ("linkedResources", "link", "webUrl"),
        TaskChange::DeleteAttachments { .. } => ("attachments", "attachment", "name"),
        _ => ("checklistItems", "step", "displayName"),
    };
    for child in numbered(&cached, collection) {
        if ids.iter().any(|id| child.get("id") == Some(&json!(id))) {
            eprintln!("  {}  {:?}", number(&child), text(&child, label));
        }
    }
    let count = ids.len();
    let what = if count == 1 {
        format!("this {noun}")
    } else {
        format!("these {count} {noun}s")
    };
    if !confirm(&format!("Delete {what} of {:?}?", target.title))? {
        eprintln!("Nothing was deleted.");
        return Ok(());
    }
    let change = match change {
        TaskChange::DeleteLink { .. } => TaskChange::DeleteLink {
            link: ids.first().cloned(),
        },
        TaskChange::DeleteAttachments { .. } => TaskChange::DeleteAttachments { attachments: ids },
        _ => TaskChange::DeleteSteps { steps: ids },
    };
    let request = change_request(vec![target.id.clone()], None, None, change, false, key);
    send(paths, request, format).await
}

/// A link is kept whatever its scheme, but only some open: say so, for
/// people (JSON's stderr is for errors). One that isn't a URL at all is
/// the daemon's to refuse.
fn warn_unopenable(url: &str, format: OutputFormat) {
    if matches!(format, OutputFormat::Json | OutputFormat::Jsonl) {
        return;
    }
    if storable(url.trim()).is_ok()
        && let Err(why) = openable(url.trim())
    {
        eprintln!("note: kept, but ms-todo won't open it: {why}");
    }
}

pub(crate) fn number(child: &Entity) -> String {
    child.get("index").map(Value::to_string).unwrap_or_default()
}

fn checkbox(step: &Entity) -> &'static str {
    if step.get("isChecked") == Some(&Value::Bool(true)) {
        "[x]"
    } else {
        "[ ]"
    }
}
