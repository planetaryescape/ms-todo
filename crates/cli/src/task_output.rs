//! What mutations print: the plan from a dry run, or what was done, with
//! `schema_version` and the `op_id` (docs/blueprint/07-cli.md#output-contract).
//! A change to lists (folders) plans and answers with lists, not tasks.

use std::io::Write;

use ms_todo_protocol::{Applied, Plan, TaskAction};
use serde::Serialize;
use serde_json::Value;

use crate::csv_columns::{LIST_COLUMNS, TASK_COLUMNS, list_row, task_row, text};
use crate::error::CliError;
use crate::output::{OutputFormat, SCHEMA_VERSION, Versioned, print_ids, print_json, write_csv};

#[derive(Serialize)]
struct DryRun<'a> {
    dry_run: bool,
    #[serde(flatten)]
    plan: &'a Plan,
}

pub fn print_plan(format: OutputFormat, plan: &Plan) -> Result<(), CliError> {
    match format {
        OutputFormat::Json | OutputFormat::Jsonl => print_json(
            format,
            &Versioned {
                schema_version: SCHEMA_VERSION,
                inner: &DryRun {
                    dry_run: true,
                    plan,
                },
            },
        ),
        OutputFormat::Ids => print_ids(
            plan.targets
                .iter()
                .map(|target| target.id.as_str())
                .chain(plan.lists.iter().map(|list| list.id.as_str())),
        ),
        OutputFormat::Csv if changes_lists(plan.action) => {
            let rows: Vec<Vec<String>> = plan
                .lists
                .iter()
                .map(|list| {
                    vec![
                        verb(plan.action).to_owned(),
                        list.id.clone(),
                        list.name.clone(),
                        list.changes.to_string(),
                    ]
                })
                .collect();
            write_csv(&["action", "id", "name", "changes"], &rows)
        }
        OutputFormat::Csv => {
            let rows: Vec<Vec<String>> = if plan.action == TaskAction::Add {
                let list = plan.list.as_ref().map(|list| list.id.clone());
                vec![vec![
                    verb(plan.action).to_owned(),
                    String::new(),
                    planned_title(plan).to_owned(),
                    list.unwrap_or_default(),
                ]]
            } else {
                plan.targets
                    .iter()
                    .map(|target| {
                        vec![
                            verb(plan.action).to_owned(),
                            target.id.clone(),
                            target.title.clone(),
                            target.list_id.clone(),
                        ]
                    })
                    .collect()
            };
            write_csv(&["action", "id", "title", "list"], &rows)
        }
        OutputFormat::Table => {
            let mut stdout = std::io::stdout().lock();
            for line in describe_plan(plan) {
                writeln!(stdout, "{line}")?;
            }
            Ok(())
        }
    }
}

/// A dry run for people: what would happen, to what.
pub fn describe_plan(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    if changes_lists(plan.action) {
        let count = plan.lists.len();
        let noun = if count == 1 { "list" } else { "lists" };
        lines.push(format!("Would {} {count} {noun}:", verb(plan.action)));
        for list in &plan.lists {
            lines.push(format!("  {:?}  {}  {}", list.name, list.id, list.changes));
        }
        return lines;
    }
    match (plan.action, &plan.list) {
        (TaskAction::Add, Some(list)) => lines.push(format!(
            "Would add {:?} to {:?} ({})",
            planned_title(plan),
            list.name,
            list.id
        )),
        (TaskAction::Move, Some(list)) => {
            let count = plan.targets.len();
            let noun = if count == 1 { "task" } else { "tasks" };
            lines.push(format!(
                "Would move {count} {noun} to {:?} ({}):",
                list.name, list.id
            ));
            for target in &plan.targets {
                lines.push(format!("  {:?}  {}", target.title, target.id));
            }
            return lines;
        }
        _ => {
            let count = plan.targets.len();
            let noun = if count == 1 { "task" } else { "tasks" };
            lines.push(format!("Would {} {count} {noun}:", verb(plan.action)));
            for target in &plan.targets {
                lines.push(format!("  {:?}  {}", target.title, target.id));
            }
        }
    }
    if let Value::Object(changes) = &plan.changes
        && !changes.is_empty()
    {
        lines.push(format!("Sending: {}", plan.changes));
    }
    lines
}

pub fn print_applied(format: OutputFormat, applied: &Applied) -> Result<(), CliError> {
    match format {
        OutputFormat::Json | OutputFormat::Jsonl => print_json(
            format,
            &Versioned {
                schema_version: SCHEMA_VERSION,
                inner: applied,
            },
        ),
        OutputFormat::Ids => print_ids(applied.items.iter().map(|task| text(task, "id"))),
        OutputFormat::Csv if applied.items.iter().any(is_list) => {
            let rows: Vec<Vec<String>> = applied.items.iter().map(list_row).collect();
            write_csv(LIST_COLUMNS, &rows)
        }
        OutputFormat::Csv => {
            // A change can span lists, so its rows always say which list.
            let headers: Vec<&str> = TASK_COLUMNS.iter().copied().chain(["list"]).collect();
            let rows: Vec<Vec<String>> = applied
                .items
                .iter()
                .zip(&applied.list_ids)
                .map(|(task, list)| {
                    let mut row = task_row(task);
                    row.push(list.clone());
                    row
                })
                .collect();
            write_csv(&headers, &rows)
        }
        OutputFormat::Table => {
            let mut stdout = std::io::stdout().lock();
            let done = past_tense(applied.action);
            if let Some(undone) = &applied.undoes {
                writeln!(stdout, "Undoing {undone}:")?;
            }
            if applied.items.is_empty() {
                writeln!(stdout, "Nothing needed changing.")?;
            }
            for task in &applied.items {
                let id = text(task, "id");
                let state = match text(task, "sync_state") {
                    "pending" => "  (queued; it syncs in the background)",
                    _ => "",
                };
                if is_list(task) {
                    let folder = match text(task, "folder") {
                        "" => "no folder".to_owned(),
                        folder => format!("folder {folder:?}"),
                    };
                    let name = text(task, "displayName");
                    writeln!(stdout, "{done} {name:?} ({folder})  {id}{state}")?;
                    continue;
                }
                writeln!(stdout, "{done} {:?}  {id}{state}", text(task, "title"))?;
                write_children(&mut stdout, applied.action, task)?;
                if let Some(rolled) = applied.rolled.iter().find(|rolled| rolled.id == id) {
                    writeln!(
                        stdout,
                        "  It repeats: the task stays open, now due {}. Microsoft To Do also \
                         added the occurrence you completed as a new, completed task.",
                        rolled.next_due
                    )?;
                }
            }
            for task in &applied.refused {
                writeln!(
                    stdout,
                    "Left alone {:?}  {}: {}",
                    task.title, task.id, task.reason
                )?;
            }
            Ok(())
        }
    }
}

/// After a step, link or attachment write, the task's steps, link or
/// attachments as they are now.
fn write_children(
    out: &mut impl Write,
    action: TaskAction,
    task: &ms_todo_protocol::Entity,
) -> std::io::Result<()> {
    let (collection, none) = match action {
        TaskAction::StepAdd
        | TaskAction::StepEdit
        | TaskAction::StepCheck
        | TaskAction::StepUncheck
        | TaskAction::StepDelete => ("checklistItems", "steps"),
        TaskAction::LinkAdd | TaskAction::LinkEdit | TaskAction::LinkDelete => {
            ("linkedResources", "link")
        }
        TaskAction::AttachmentAdd | TaskAction::AttachmentDelete => ("attachments", "attachments"),
        _ => return Ok(()),
    };
    let children = task
        .get(collection)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if children.is_empty() {
        writeln!(out, "  (no {none})")?;
    }
    for (at, child) in children.iter().enumerate() {
        let field = |key: &str| {
            ms_todo_core::display_safe(child.get(key).and_then(Value::as_str).unwrap_or_default())
        };
        if collection == "checklistItems" {
            let mark = if child["isChecked"] == true { "x" } else { " " };
            writeln!(out, "  {} [{mark}] {}", at + 1, field("displayName"))?;
        } else if collection == "attachments" {
            let pending = child["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(ms_todo_core::LOCAL_CHILD_PREFIX));
            let state = if pending { "  (uploading)" } else { "" };
            writeln!(out, "  {} {}{state}", at + 1, field("name"))?;
        } else {
            let name = field("displayName");
            let url = field("webUrl");
            match name.as_ref() {
                "" => writeln!(out, "  {} {url}", at + 1)?,
                name => writeln!(out, "  {} {name}  {url}", at + 1)?,
            }
        }
    }
    Ok(())
}

fn verb(action: TaskAction) -> &'static str {
    match action {
        TaskAction::Add => "add",
        TaskAction::Complete => "complete",
        TaskAction::Reopen => "reopen",
        TaskAction::Edit => "edit",
        TaskAction::Delete => "delete",
        TaskAction::Undo => "undo",
        TaskAction::Move => "move",
        TaskAction::MoveList => "move",
        TaskAction::OrderList => "reorder",
        TaskAction::RenameFolder => "rename the folder of",
        TaskAction::DeleteFolder => "take out of the folder",
        TaskAction::OrderFolder => "reorder the folder of",
        TaskAction::MyDayAdd => "put in My Day",
        TaskAction::MyDayRemove => "take out of My Day",
        TaskAction::MyDayRollover => "roll out of an earlier My Day",
        TaskAction::StepAdd => "add steps to",
        TaskAction::StepEdit => "rename a step of",
        TaskAction::StepCheck => "check steps of",
        TaskAction::StepUncheck => "uncheck steps of",
        TaskAction::StepDelete => "delete steps of",
        TaskAction::LinkAdd => "add a link to",
        TaskAction::LinkEdit => "change the link of",
        TaskAction::LinkDelete => "delete the link of",
        TaskAction::AttachmentAdd => "attach files to",
        TaskAction::AttachmentDelete => "delete attachments of",
        TaskAction::Unknown => "change",
    }
}

fn past_tense(action: TaskAction) -> &'static str {
    match action {
        TaskAction::Add => "Added",
        TaskAction::Complete => "Completed",
        TaskAction::Reopen => "Reopened",
        TaskAction::Edit => "Updated",
        TaskAction::Delete => "Deleted",
        TaskAction::Undo => "  Reverting",
        TaskAction::Move => "Moving",
        TaskAction::MoveList => "Moved",
        TaskAction::OrderList => "Reordered",
        TaskAction::RenameFolder => "Renamed the folder of",
        TaskAction::DeleteFolder => "Took out of the folder",
        TaskAction::OrderFolder => "Reordered the folder of",
        TaskAction::MyDayAdd => "In My Day:",
        TaskAction::MyDayRemove => "Out of My Day:",
        TaskAction::MyDayRollover => "Rolled out of My Day:",
        TaskAction::StepAdd => "Added steps to",
        TaskAction::StepEdit => "Renamed a step of",
        TaskAction::StepCheck => "Checked steps of",
        TaskAction::StepUncheck => "Unchecked steps of",
        TaskAction::StepDelete => "Deleted steps of",
        TaskAction::LinkAdd => "Added a link to",
        TaskAction::LinkEdit => "Changed the link of",
        TaskAction::LinkDelete => "Deleted the link of",
        TaskAction::AttachmentAdd => "Attaching to",
        TaskAction::AttachmentDelete => "Deleted attachments of",
        TaskAction::Unknown => "Changed",
    }
}

/// Whether `action` changes lists (folders) rather than tasks.
fn changes_lists(action: TaskAction) -> bool {
    matches!(
        action,
        TaskAction::MoveList
            | TaskAction::OrderList
            | TaskAction::RenameFolder
            | TaskAction::DeleteFolder
            | TaskAction::OrderFolder
    )
}

/// A list, not a task: every task has a title, and a list a display name.
/// An undo's answer holds either, and its action doesn't say which.
fn is_list(item: &ms_todo_protocol::Entity) -> bool {
    item.contains_key("displayName") && !item.contains_key("title")
}

// The title of a task to add: it's in the POST body the plan carries.
fn planned_title(plan: &Plan) -> &str {
    plan.changes
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
}
