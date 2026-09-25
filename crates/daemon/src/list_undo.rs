//! Undoing a change to lists: a folder change, or a list's create, rename
//! or delete (rung 8e). Each is undone only while what it set still holds:
//!
//! - a folder write, while each extension field it wrote holds its value;
//! - a create, by deleting the list, while it has its name and no task;
//! - a rename, by the old name, while the list has the new one;
//! - a delete, by making the list again (a new Graph ID, the same local
//!   ID, name and folder), only when it held no task: Microsoft To Do
//!   deleted any it held, and they can't be brought back.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, ResponseData, TaskAction};
use ms_todo_store::{
    FOLDER_FIELD, FOLDER_ORDER_FIELD, ListExtensionOp, ListOp, ListRow, ListWrite, ORDER_FIELD,
    OpKind, OutboxRow,
};
use serde_json::{Map, Value};

use crate::handlers::{State, error_payload, store_error};
use crate::list_lifecycle::lifecycle;
use crate::list_writes::queue_lists;
use crate::outbox::op_id_for;
use crate::task_writes::action_name;
use crate::undo::{conflict, inverse_fields, nothing_to_undo, rejected};

pub(crate) async fn undo_lists(
    state: &State,
    target: &str,
    ops: &[OutboxRow],
    op_id: &str,
) -> Result<ResponseData, ErrorPayload> {
    let mut inverse: Vec<ListOp> = Vec::new();
    // A list the command made is deleted whole: its folder write needs no
    // undoing of its own.
    let created: Vec<&str> = ops
        .iter()
        .filter(|op| op.op == OpKind::ListCreate)
        .map(|op| op.entity_local_id.as_str())
        .collect();
    for op in ops {
        if rejected(op)?
            || (op.op == OpKind::Extension && created.contains(&op.entity_local_id.as_str()))
        {
            continue;
        }
        let id = op_id_for(op_id, inverse.len());
        let found = state
            .store
            .list_any(&op.entity_local_id)
            .await
            .map_err(store_error)?;
        let Some((list, deleted)) = found else {
            return Err(gone(op, target));
        };
        match op.op {
            OpKind::Extension => {
                if deleted {
                    return Err(gone(op, target));
                }
                check_list_unchanged(op, &list)?;
                inverse.push(ListOp::Extension(ListExtensionOp {
                    op_id: id,
                    list_local_id: op.entity_local_id.clone(),
                    action: op.action.clone(),
                    fields: inverse_fields(op)?,
                }));
            }
            OpKind::ListCreate => {
                if deleted {
                    return Err(gone(op, target));
                }
                check_name(op, &list)?;
                let tasks = state
                    .store
                    .list_task_count(&list.local_id)
                    .await
                    .map_err(store_error)?;
                if tasks > 0 {
                    return Err(conflict(format!(
                        "{:?} has {tasks} task(s) now, so undoing its create would delete them; \
                         `ms-todo lists delete` does that on purpose",
                        list.display_name
                    )));
                }
                inverse.push(lifecycle(
                    id,
                    &list,
                    TaskAction::DeleteList,
                    ListWrite::Delete { tasks: 0 },
                ));
            }
            OpKind::ListUpdate => {
                if deleted {
                    return Err(gone(op, target));
                }
                check_name(op, &list)?;
                let name = op
                    .rollback
                    .as_ref()
                    .and_then(|before| before["raw"]["displayName"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                let write = ListWrite::Rename { name };
                inverse.push(lifecycle(id, &list, TaskAction::RenameList, write));
            }
            OpKind::ListDelete => {
                if !deleted {
                    return Err(error_payload(
                        ErrorKind::InvalidInput,
                        format!(
                            "{:?} is back already, so there's nothing to undo",
                            list.display_name
                        ),
                    ));
                }
                // The tasks cached then, or those Graph was found to hold.
                let cached = op
                    .rollback
                    .as_ref()
                    .and_then(|before| before["tasks"].as_array())
                    .map_or(0, Vec::len);
                let confirmed = op.payload["tasks"]
                    .as_u64()
                    .and_then(|tasks| usize::try_from(tasks).ok())
                    .unwrap_or(0);
                let tasks = cached.max(confirmed);
                if tasks > 0 {
                    return Err(error_payload(
                        ErrorKind::Unsupported,
                        format!(
                            "deleting {:?} can't be undone: Microsoft To Do deleted the {tasks} \
                             task(s) in it too. Only an empty list's delete can be undone",
                            list.display_name
                        ),
                    ));
                }
                inverse.push(lifecycle(
                    id,
                    &list,
                    TaskAction::CreateList,
                    ListWrite::Recreate,
                ));
                // Its folder and place, which lived in its extension.
                let fields = ours(&list);
                if !fields.is_empty() {
                    inverse.push(ListOp::Extension(ListExtensionOp {
                        op_id: op_id_for(op_id, inverse.len()),
                        list_local_id: list.local_id.clone(),
                        action: action_name(TaskAction::MoveList).to_owned(),
                        fields,
                    }));
                }
            }
            _ => {
                return Err(error_payload(
                    ErrorKind::Internal,
                    format!("{target} changes both tasks and lists, which no command does"),
                ));
            }
        }
    }
    if inverse.is_empty() {
        return Err(nothing_to_undo(target));
    }
    queue_lists(state, op_id, Some(target), inverse, TaskAction::Undo).await
}

/// The folder fields of `list`'s extension, to write on it again.
fn ours(list: &ListRow) -> Map<String, Value> {
    list.extension
        .as_ref()
        .and_then(Value::as_object)
        .map(|extension| {
            extension
                .iter()
                .filter(|(key, _)| {
                    [FOLDER_FIELD, ORDER_FIELD, FOLDER_ORDER_FIELD].contains(&key.as_str())
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Refuse to undo `op` if the list's name isn't the one `op` gave it.
fn check_name(op: &OutboxRow, list: &ListRow) -> Result<(), ErrorPayload> {
    let set = op.body()["displayName"].as_str().unwrap_or_default();
    if list.display_name == set {
        return Ok(());
    }
    Err(conflict(format!(
        "the list {set:?} has been renamed {:?} since; undo would overwrite that",
        list.display_name
    )))
}

fn gone(op: &OutboxRow, target: &str) -> ErrorPayload {
    error_payload(
        ErrorKind::NotFound,
        format!(
            "list {} was deleted since {target}, so it can't be undone",
            op.entity_local_id
        ),
    )
}

/// Refuse to undo `op` if a field it set on `list`'s extension doesn't
/// hold the value it set any more (a null having removed it).
fn check_list_unchanged(op: &OutboxRow, list: &ListRow) -> Result<(), ErrorPayload> {
    let Some(fields) = op.body().as_object() else {
        return Ok(());
    };
    let current = |key: &str| {
        list.extension
            .as_ref()
            .and_then(|extension| extension.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let moved: Vec<&String> = fields
        .iter()
        .filter(|(key, set)| current(key) != **set)
        .map(|(key, _)| key)
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    let since = if moved.iter().any(|key| key.as_str() == FOLDER_FIELD) {
        match list.folder() {
            Some(folder) => format!("has moved to {folder:?}"),
            None => "has moved out of its folder".to_owned(),
        }
    } else {
        "was reordered".to_owned()
    };
    Err(conflict(format!(
        "{:?} {since} since; undo would overwrite that",
        list.display_name
    )))
}
