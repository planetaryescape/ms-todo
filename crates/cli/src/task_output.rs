//! What mutations print: the plan from a dry run, or what was done, with
//! `schema_version` and the `op_id` (docs/blueprint/07-cli.md#output-contract).

use std::io::Write;

use ms_todo_protocol::{Applied, Plan, TaskAction};
use serde::Serialize;
use serde_json::Value;

use crate::csv_columns::{TASK_COLUMNS, task_row, text};
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
        OutputFormat::Ids => print_ids(plan.targets.iter().map(|target| target.id.as_str())),
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
    match (plan.action, &plan.list) {
        (TaskAction::Add, Some(list)) => lines.push(format!(
            "Would add {:?} to {:?} ({})",
            planned_title(plan),
            list.name,
            list.id
        )),
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
            for task in &applied.items {
                let id = text(task, "id");
                writeln!(stdout, "{done} {:?}  {id}", text(task, "title"))?;
                if let Some(rolled) = applied.rolled.iter().find(|rolled| rolled.id == id) {
                    writeln!(
                        stdout,
                        "  It repeats: the task stays open, now due {}. Microsoft To Do also \
                         added the occurrence you completed as a new, completed task.",
                        rolled.next_due
                    )?;
                }
            }
            Ok(())
        }
    }
}

fn verb(action: TaskAction) -> &'static str {
    match action {
        TaskAction::Add => "add",
        TaskAction::Complete => "complete",
        TaskAction::Reopen => "reopen",
        TaskAction::Edit => "edit",
        TaskAction::Delete => "delete",
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
        TaskAction::Unknown => "Changed",
    }
}

// The title of a task to add: it's in the POST body the plan carries.
fn planned_title(plan: &Plan) -> &str {
    plan.changes
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
}
