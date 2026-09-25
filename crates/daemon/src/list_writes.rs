//! `lists move|order` and `folders list|rename|delete|order`
//! (docs/blueprint/05-custom-features.md#folders-list-groups). Each change
//! is planned by `folders` as field changes to lists' extensions; a dry run
//! returns the plan, and a real run applies it to the cache and queues one
//! extension write per list in the outbox, in one transaction, like a task
//! write: offline-safe, and undone by `undo`.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{
    Applied, ErrorPayload, Folder, ListChange, Plan, PlannedList, ResponseData, TaskAction,
};
use ms_todo_store::{LISTS_SCOPE, ListExtensionOp, ListRow};
use serde_json::Value;

use crate::entities::list_entity;
use crate::folders::{self, Planned, anchor};
use crate::freshness::{ensure_ready, read_state};
use crate::handlers::{State, error_payload, store_error};
use crate::list_resolution::resolve_list;
use crate::outbox::op_id_for;
use crate::task_writes::action_name;

pub(crate) async fn list_folders(state: &State) -> Result<ResponseData, ErrorPayload> {
    let sync = read_state(state, LISTS_SCOPE).await?;
    let (lists, counts) = tokio::try_join!(
        async { state.store.lists().await.map_err(store_error) },
        async { state.store.task_counts().await.map_err(store_error) },
    )?;
    let items = folders::groups(&lists)
        .into_iter()
        .map(|group| Folder {
            name: group.name.to_owned(),
            open_count: group
                .lists
                .iter()
                .map(|list| {
                    counts
                        .open_by_list
                        .get(&list.local_id)
                        .copied()
                        .unwrap_or(0)
                })
                .sum(),
            lists: group
                .lists
                .iter()
                .map(|list| list.local_id.clone())
                .collect(),
        })
        .collect();
    Ok(ResponseData::Folders { items, sync })
}

pub(crate) async fn change_lists(
    state: &State,
    change: ListChange,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    ensure_ready(state, LISTS_SCOPE).await?;
    let lists = state.store.lists().await.map_err(store_error)?;
    let (action, planned) = plan(&lists, &change)?;
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action,
            list: None,
            targets: Vec::new(),
            lists: planned
                .iter()
                .map(|(list, fields)| PlannedList {
                    id: list.local_id.clone(),
                    name: list.display_name.clone(),
                    changes: Value::Object(fields.clone()),
                })
                .collect(),
            changes: Value::Null,
        }));
    }
    let ops = planned
        .into_iter()
        .enumerate()
        .map(|(index, (list, fields))| ListExtensionOp {
            op_id: op_id_for(&op_id, index),
            list_local_id: list.local_id.clone(),
            action: action_name(action).to_owned(),
            fields,
        })
        .collect();
    queue_lists(state, &op_id, None, ops, action).await
}

/// Queue `ops`, one command's, wake the worker, and answer with the lists
/// as they are now. A command that changes nothing queues nothing.
pub(crate) async fn queue_lists(
    state: &State,
    command_id: &str,
    undoes: Option<&str>,
    ops: Vec<ListExtensionOp>,
    action: TaskAction,
) -> Result<ResponseData, ErrorPayload> {
    let rows = if ops.is_empty() {
        Vec::new()
    } else {
        let rows = state
            .store
            .enqueue_list_extension(command_id, undoes, ops)
            .await
            .map_err(store_error)?;
        state.outbox.wake();
        rows
    };
    let ids: Vec<String> = rows.iter().map(|row| row.local_id.clone()).collect();
    state.events.changed(ids.clone(), Vec::new());
    Ok(ResponseData::Applied(Applied {
        op_id: command_id.to_owned(),
        action,
        items: rows.iter().map(list_entity).collect(),
        list_ids: ids,
        rolled: Vec::new(),
        undoes: undoes.map(str::to_owned),
    }))
}

fn plan<'a>(
    lists: &'a [ListRow],
    change: &ListChange,
) -> Result<(TaskAction, Planned<'a>), ErrorPayload> {
    Ok(match change {
        ListChange::MoveList {
            lists: names,
            folder,
        } => {
            if names.is_empty() {
                return Err(error_payload(
                    ErrorKind::InvalidInput,
                    "name at least one list to move".into(),
                ));
            }
            let mut targets: Vec<&ListRow> = Vec::with_capacity(names.len());
            for name in names {
                let found = find(lists, name)?;
                if !targets.iter().any(|seen| seen.local_id == found.local_id) {
                    targets.push(found);
                }
            }
            (
                TaskAction::MoveList,
                folders::move_lists(lists, &targets, folder.as_deref())?,
            )
        }
        ListChange::OrderList { list, anchor: to } => {
            let (next_to, before) = anchor(to);
            let planned =
                folders::order_list(lists, find(lists, list)?, find(lists, next_to)?, before)?;
            (TaskAction::OrderList, planned)
        }
        ListChange::RenameFolder { folder, name } => (
            TaskAction::RenameFolder,
            folders::rename(lists, folder, name)?,
        ),
        ListChange::DeleteFolder { folder } => {
            (TaskAction::DeleteFolder, folders::delete(lists, folder)?)
        }
        ListChange::OrderFolder { folder, anchor: to } => {
            let (next_to, before) = anchor(to);
            (
                TaskAction::OrderFolder,
                folders::order_folder(lists, folder, next_to, before)?,
            )
        }
        ListChange::Unknown => {
            return Err(error_payload(
                ErrorKind::Unsupported,
                "this daemon doesn't know that change; restart it with `ms-todo daemon stop`"
                    .into(),
            ));
        }
    })
}

/// The list `wanted` names or identifies, as `--list` resolves it.
fn find<'a>(lists: &'a [ListRow], wanted: &str) -> Result<&'a ListRow, ErrorPayload> {
    let found = resolve_list(lists, Some(wanted))?;
    lists
        .iter()
        .find(|list| list.local_id == found.local_id)
        .ok_or_else(|| error_payload(ErrorKind::Internal, "a resolved list vanished".into()))
}
