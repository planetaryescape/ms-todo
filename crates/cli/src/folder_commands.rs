//! `lists move|order` and `folders list|rename|delete|order`
//! (docs/blueprint/07-cli.md). The daemon resolves the lists and folders
//! and plans the change; `--dry-run` asks it for the plan only. `folders
//! delete` without `--yes` asks in a terminal and exits 2 anywhere else.

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{Anchor, Folder, ListChange, Request, ResponseData, SyncInfo};
use serde_json::{Map, Value, json};

use crate::args::{
    DeleteFolderArgs, MoveListsArgs, OrderFolderArgs, OrderListArgs, RenameFolderArgs,
};
use crate::confirm::{can_prompt, confirm};
use crate::daemon_client;
use crate::error::CliError;
use crate::output::{OutputFormat, Table, csv_cell};
use crate::task_commands::{expand_stdin, op_id_unless, send};
use crate::task_output::describe_plan;

pub const FOLDERS_TABLE: Table = Table {
    headings: &["FOLDER", "LISTS", "OPEN"],
    row: |folder| cells(folder, &["name", "list_count", "open_count"]),
    csv_headings: &["name", "list_count", "open_count", "lists"],
    csv_row: |folder| cells(folder, &["name", "list_count", "open_count", "lists"]),
    bold_matches: None,
};

fn cells(folder: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .map(|key| csv_cell(folder.get(*key).unwrap_or(&Value::Null)))
        .collect()
}

/// `folders list`: each folder as `{ name, lists, list_count, open_count }`.
pub async fn list(paths: &Paths) -> Result<(Vec<Map<String, Value>>, SyncInfo), CliError> {
    match daemon_client::ask(paths, Request::ListFolders).await? {
        ResponseData::Folders { items, sync } => {
            Ok((items.iter().map(folder_item).collect(), sync))
        }
        _ => Err(crate::unexpected_response()),
    }
}

fn folder_item(folder: &Folder) -> Map<String, Value> {
    [
        ("name", json!(folder.name)),
        ("lists", json!(folder.lists)),
        ("list_count", json!(folder.lists.len())),
        ("open_count", json!(folder.open_count)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

pub async fn move_lists(
    paths: &Paths,
    args: MoveListsArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let (lists, _) = expand_stdin(args.lists)?;
    let change = ListChange::MoveList {
        lists,
        folder: args.folder.filter(|_| !args.no_folder),
    };
    let request = change_request(change, args.dry_run, args.idempotency.idempotency_key);
    send(paths, request, format).await
}

pub async fn order_list(
    paths: &Paths,
    args: OrderListArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = ListChange::OrderList {
        list: args.list,
        anchor: anchor(args.before, args.after)?,
    };
    let request = change_request(change, args.dry_run, args.idempotency.idempotency_key);
    send(paths, request, format).await
}

pub async fn rename(
    paths: &Paths,
    args: RenameFolderArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = ListChange::RenameFolder {
        folder: args.folder,
        name: args.name,
    };
    let request = change_request(change, args.dry_run, args.idempotency.idempotency_key);
    send(paths, request, format).await
}

pub async fn delete(
    paths: &Paths,
    args: DeleteFolderArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = ListChange::DeleteFolder {
        folder: args.folder,
    };
    let (dry_run, key) = (args.dry_run, args.idempotency.idempotency_key);
    if args.yes || dry_run {
        return send(paths, change_request(change, dry_run, key), format).await;
    }
    if !can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "there's no terminal to ask in: pass --yes to delete the folder (its lists stay, and \
             `ms-todo undo` puts them back), or --dry-run to see what would change"
                .into(),
        ));
    }
    let plan = match daemon_client::ask(paths, change_request(change.clone(), true, None)).await? {
        ResponseData::Plan(plan) => plan,
        _ => return Err(crate::unexpected_response()),
    };
    eprintln!("{}", describe_plan(&plan).join("\n"));
    if !confirm("Delete the folder? Its lists stay.")? {
        eprintln!("Nothing was changed.");
        return Ok(());
    }
    send(paths, change_request(change, false, key), format).await
}

pub async fn order_folder(
    paths: &Paths,
    args: OrderFolderArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let change = ListChange::OrderFolder {
        folder: args.folder,
        anchor: anchor(args.before, args.after)?,
    };
    let request = change_request(change, args.dry_run, args.idempotency.idempotency_key);
    send(paths, request, format).await
}

fn change_request(change: ListChange, dry_run: bool, idempotency_key: Option<String>) -> Request {
    Request::ChangeLists {
        change,
        dry_run,
        op_id: op_id_unless(dry_run),
        idempotency_key,
    }
}

// clap requires exactly one of the two.
fn anchor(before: Option<String>, after: Option<String>) -> Result<Anchor, CliError> {
    match (before, after) {
        (Some(before), None) => Ok(Anchor::Before(before)),
        (None, Some(after)) => Ok(Anchor::After(after)),
        _ => Err(CliError::message(
            ErrorKind::InvalidInput,
            "give one of --before or --after".into(),
        )),
    }
}
