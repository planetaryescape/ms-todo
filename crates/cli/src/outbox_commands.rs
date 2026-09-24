//! `ms-todo outbox list|retry|discard` (docs/blueprint/07-cli.md): the
//! queue of writes on their way to Microsoft To Do, and the user's way to
//! resolve those the daemon won't decide alone. `discard` is destructive,
//! so it asks in a terminal and needs `--yes` anywhere else.

use std::io::Write;

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{OutboxOp, OutboxState, Request, ResponseData};
use serde::Serialize;

use crate::args::OutboxStateArg;
use crate::confirm::{can_prompt, confirm};
use crate::daemon_client;
use crate::error::CliError;
use crate::output::{OutputFormat, SCHEMA_VERSION, Versioned, print_ids, print_json, write_csv};

const CSV_COLUMNS: &[&str] = &[
    "op_id", "state", "action", "task_id", "title", "attempts", "error", "note",
];

#[derive(Serialize)]
struct Items<'a> {
    items: &'a [OutboxOp],
}

pub async fn list(
    paths: &Paths,
    state: Option<OutboxStateArg>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let request = Request::OutboxList {
        state: state.map(outbox_state),
    };
    let items = match daemon_client::ask(paths, request).await? {
        ResponseData::Outbox { items } => items,
        _ => return Err(crate::unexpected_response()),
    };
    print_ops(format, &items)
}

pub async fn retry(paths: &Paths, op: String, format: OutputFormat) -> Result<(), CliError> {
    let op = one(daemon_client::ask(paths, Request::OutboxRetry { op_id: op }).await?)?;
    print_ops(format, std::slice::from_ref(&op))
}

pub async fn discard(
    paths: &Paths,
    op: String,
    yes: bool,
    format: OutputFormat,
) -> Result<(), CliError> {
    if !yes {
        if !can_prompt() {
            return Err(CliError::message(
                ErrorKind::InvalidInput,
                "discarding a write drops it for good, and there's no terminal to ask in; pass \
                 --yes to discard it"
                    .into(),
            ));
        }
        if !confirm(&format!("Discard {op}? It won't be sent."))? {
            eprintln!("Nothing was discarded.");
            return Ok(());
        }
    }
    let op = one(daemon_client::ask(paths, Request::OutboxDiscard { op_id: op }).await?)?;
    print_ops(format, std::slice::from_ref(&op))
}

fn one(data: ResponseData) -> Result<OutboxOp, CliError> {
    match data {
        ResponseData::OutboxOp(op) => Ok(op),
        _ => Err(crate::unexpected_response()),
    }
}

fn print_ops(format: OutputFormat, items: &[OutboxOp]) -> Result<(), CliError> {
    match format {
        OutputFormat::Json => print_json(
            format,
            &Versioned {
                schema_version: SCHEMA_VERSION,
                inner: &Items { items },
            },
        ),
        OutputFormat::Jsonl => {
            for item in items {
                print_json(
                    format,
                    &Versioned {
                        schema_version: SCHEMA_VERSION,
                        inner: item,
                    },
                )?;
            }
            Ok(())
        }
        OutputFormat::Ids => print_ids(items.iter().map(|op| op.op_id.as_str())),
        OutputFormat::Csv => {
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|op| {
                    vec![
                        op.op_id.clone(),
                        state_name(op.state).to_owned(),
                        op.action.clone(),
                        op.task_id.clone(),
                        op.title.clone().unwrap_or_default(),
                        op.attempts.to_string(),
                        op.last_error
                            .as_ref()
                            .map(|error| format!("{}: {}", error.kind, error.message))
                            .unwrap_or_default(),
                        op.note.clone().unwrap_or_default(),
                    ]
                })
                .collect();
            write_csv(CSV_COLUMNS, &rows)
        }
        OutputFormat::Table => {
            let mut stdout = std::io::stdout().lock();
            if items.is_empty() {
                writeln!(stdout, "Nothing is queued.")?;
            }
            for op in items {
                let mut state = state_name(op.state).to_owned();
                if op.flagged {
                    state.push_str(" (needs you)");
                }
                writeln!(
                    stdout,
                    "{}  {state}  {} {:?}",
                    op.op_id,
                    op.action,
                    op.title.as_deref().unwrap_or("")
                )?;
                if let Some(error) = &op.last_error
                    && op.state != OutboxState::Done
                {
                    writeln!(stdout, "  {}: {}", error.kind, error.message)?;
                }
                if let Some(note) = &op.note {
                    writeln!(stdout, "  {note}")?;
                }
            }
            Ok(())
        }
    }
}

fn state_name(state: OutboxState) -> &'static str {
    match state {
        OutboxState::Pending => "pending",
        OutboxState::Inflight => "inflight",
        OutboxState::Unknown => "unknown",
        OutboxState::Failed => "failed",
        OutboxState::Done => "done",
        OutboxState::Other => "other",
    }
}

fn outbox_state(state: OutboxStateArg) -> OutboxState {
    match state {
        OutboxStateArg::Pending => OutboxState::Pending,
        OutboxStateArg::Inflight => OutboxState::Inflight,
        OutboxStateArg::Unknown => OutboxState::Unknown,
        OutboxStateArg::Failed => OutboxState::Failed,
        OutboxStateArg::Done => OutboxState::Done,
    }
}
