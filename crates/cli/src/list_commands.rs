//! `lists create|rename|delete` (rung 8e). Each is a `ChangeLists` the
//! daemon plans and queues in the outbox, as the folder changes are;
//! `--dry-run` asks for the plan only. `lists delete` asks in a terminal
//! and exits 2 anywhere else without `--yes`.

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{ListChange, ResponseData};

use crate::confirm::{can_prompt, confirm};
use crate::daemon_client;
use crate::error::CliError;
use crate::folder_commands::change_request;
use crate::list_args::{CreateListArgs, DeleteListArgs, RenameListArgs};
use crate::output::OutputFormat;
use crate::task_commands::send;
use crate::task_output::describe_plan;

pub async fn create(
    paths: &Paths,
    args: CreateListArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = ListChange::CreateList {
        name: args.name,
        folder: args.folder,
    };
    let request = change_request(change, args.dry_run, args.idempotency.idempotency_key);
    send(paths, request, format).await
}

pub async fn rename(
    paths: &Paths,
    args: RenameListArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = ListChange::RenameList {
        list: args.list,
        name: args.name,
    };
    let request = change_request(change, args.dry_run, args.idempotency.idempotency_key);
    send(paths, request, format).await
}

pub async fn delete(
    paths: &Paths,
    args: DeleteListArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = ListChange::DeleteList { list: args.list };
    let (dry_run, key) = (args.dry_run, args.idempotency.idempotency_key);
    if args.yes || dry_run {
        return send(paths, change_request(change, dry_run, key), format).await;
    }
    if !can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "there's no terminal to ask in: pass --yes to delete the list and every task in it \
             (only an empty list's delete can be undone), or --dry-run to see what would go"
                .into(),
        ));
    }
    let plan = match daemon_client::ask(paths, change_request(change.clone(), true, None)).await? {
        ResponseData::Plan(plan) => plan,
        _ => return Err(crate::unexpected_response()),
    };
    eprintln!("{}", describe_plan(&plan).join("\n"));
    let tasks = plan
        .lists
        .first()
        .and_then(|list| list.changes["tasks"].as_u64())
        .unwrap_or(0);
    let question = match tasks {
        0 => "Delete the list? `ms-todo undo` brings it back.".to_owned(),
        1 => "Delete the list and the task in it? This can't be undone.".to_owned(),
        many => format!("Delete the list and the {many} tasks in it? This can't be undone."),
    };
    if !confirm(&question)? {
        eprintln!("Nothing was deleted.");
        return Ok(());
    }
    // The list the preview resolved, so what runs is what was shown.
    let change = match plan.lists.first() {
        Some(list) => ListChange::DeleteList {
            list: list.id.clone(),
        },
        None => change,
    };
    send(paths, change_request(change, false, key), format).await
}
