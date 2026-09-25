//! `ms-todo undo [OP_ID] [--copy ID]` (docs/blueprint/07-cli.md#output-contract):
//! queue the inverse of a command's operations, as a command of its own,
//! so an undo goes through the outbox like any write and can be undone
//! too. With no `OP_ID`, it's the latest command that isn't an undo and
//! hasn't been undone.
//!
//! - An add is undone by deleting the task.
//! - An edit, complete or reopen by setting the fields it sent back to
//!   what they were.
//! - A delete by creating the task again from what it was, with a new
//!   Graph ID; it keeps its local ID.
//! - A recurring completion by deleting the completed copy Graph made and
//!   putting the due date back (S12). Which task is the copy is never
//!   guessed: `--copy` names one of the candidates, and without it the
//!   candidates come back in an `invalid_input` error (exit 2).
//!
//! - A folder change by setting the extension fields it wrote on each list
//!   back to what they were.
//!
//! An operation Graph rejected changed nothing, so it's skipped; one whose
//! outcome is `unknown` must be resolved first.
//!
//! An edit, complete, reopen or folder change is undone only while every
//! field it set still holds the value it set, as the cache has it now. If a
//! later change or a sync from another device moved one, putting the old
//! value back would overwrite that, so the undo is refused (`conflict`,
//! exit 5) and nothing is queued. For a change to several tasks (a bulk
//! reschedule, rung 5d) the rule is per task: the tasks that moved are
//! left alone and listed in the answer's `refused`, and the rest are
//! undone; only when every one moved is the undo refused.

use chrono::{DateTime, Duration};
use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Candidate, ErrorPayload, Refused, ResponseData, TaskAction};
use ms_todo_store::{
    Entity, FOLDER_FIELD, ListExtensionOp, ListRow, LocalChange, NewOp, OpKind, OpState, OutboxRow,
    TaskRow,
};
use serde_json::{Map, Value, json};

use crate::handlers::{State, error_payload, store_error};
use crate::list_writes::queue_lists;
use crate::outbox::move_job;
use crate::outbox::{fields_not_holding, op_id_for};
use crate::task_fields::{as_written, creatable_fields};
use crate::task_writes::{delete_op, move_op, new_task_raw, our_extension, queue, update_op};

/// A completed copy is created at real time (S12), so a copy of this
/// completion was created no earlier than this before it was sent.
const COPY_SLACK_MINUTES: i64 = 5;

pub(crate) async fn undo(
    state: &State,
    target: Option<String>,
    copy: Option<&str>,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let target = match target {
        Some(target) => target,
        None => state
            .store
            .last_undoable_command()
            .await
            .map_err(store_error)?
            .ok_or_else(|| error_payload(ErrorKind::NotFound, "there's nothing to undo".into()))?,
    };
    let ops = state
        .store
        .command_ops(&target)
        .await
        .map_err(store_error)?;
    if ops.is_empty() {
        return Err(error_payload(
            ErrorKind::NotFound,
            format!("no change has the op_id {target:?}; see `ms-todo outbox list --state done`"),
        ));
    }
    if let Some(by) = state.store.undone_by(&target).await.map_err(store_error)? {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            format!("{target} was undone already, by {by}; `ms-todo undo {by}` redoes it"),
        ));
    }
    if ops.iter().all(|op| op.op == OpKind::Extension) {
        return undo_lists(state, &target, &ops, &op_id).await;
    }
    let mut inverse: Vec<NewOp> = Vec::new();
    let mut refused: Vec<Refused> = Vec::new();
    let id = |queued: usize| op_id_for(&op_id, queued);
    for op in &ops {
        if rejected(op)? {
            continue;
        }
        let (row, deleted) = state
            .store
            .task_any(&op.entity_local_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                error_payload(
                    ErrorKind::NotFound,
                    format!("task {} isn't cached any more", op.entity_local_id),
                )
            })?;
        match op.op {
            OpKind::Delete if !deleted => return Err(changed_since(op)),
            OpKind::Delete => inverse.push(recreate(id(inverse.len()), &row, op)?),
            _ if deleted => return Err(changed_since(op)),
            OpKind::Create => inverse.push(delete_op(id(inverse.len()), &row, TaskAction::Delete)),
            OpKind::Update if op.is_recurring_completion() => {
                let copy = pick_copy(state, op, &row, copy).await?;
                inverse.push(delete_op(id(inverse.len()), &copy, TaskAction::Delete));
                let due = before(op)?
                    .get("dueDateTime")
                    .cloned()
                    .unwrap_or(Value::Null);
                let body = json!({ "dueDateTime": as_written("dueDateTime", due) });
                inverse.push(update_op(id(inverse.len()), &row, &body, TaskAction::Edit));
            }
            OpKind::Update => {
                let moved = fields_not_holding(op.body(), &row.raw);
                if !moved.is_empty() {
                    let reason = format!(
                        "has changed since ({}); undo would overwrite that",
                        moved.join(", ")
                    );
                    refused.push(Refused {
                        id: row.local_id.clone(),
                        title: row.title.clone(),
                        reason,
                    });
                    continue;
                }
                let (body, action) = inverse_update(op)?;
                inverse.push(update_op(id(inverse.len()), &row, &body, action));
            }
            OpKind::Move => {
                if op.state != OpState::Done {
                    return Err(error_payload(
                        ErrorKind::InvalidInput,
                        format!(
                            "moving {:?} hasn't finished yet, so it can't be undone; wait for it \
                             (`ms-todo outbox list`)",
                            row.title
                        ),
                    ));
                }
                if let Some(reason) = moved_since(op, &row) {
                    refused.push(Refused {
                        id: row.local_id.clone(),
                        title: row.title.clone(),
                        reason,
                    });
                    continue;
                }
                let from_list = move_job::from_list(op);
                inverse.push(move_op(id(inverse.len()), &row, &from_list));
            }
            OpKind::Extension => {
                return Err(error_payload(
                    ErrorKind::Internal,
                    format!("{target} changes both tasks and lists, which no command does"),
                ));
            }
        }
    }
    if inverse.is_empty() {
        return Err(match refused.as_slice() {
            [] => nothing_to_undo(&target),
            [task] => conflict(format!("{:?} {}", task.title, task.reason)),
            many => {
                let titles: Vec<String> = many
                    .iter()
                    .map(|task| format!("{:?}", task.title))
                    .collect();
                conflict(format!(
                    "every task {target} changed has changed since ({}); undo would overwrite that",
                    titles.join(", ")
                ))
            }
        });
    }
    let mut answer = queue(state, &op_id, Some(&target), inverse, TaskAction::Undo).await?;
    if let ResponseData::Applied(applied) = &mut answer {
        applied.refused = refused;
    }
    Ok(answer)
}

/// Undo a folder change: each list's extension fields it wrote go back to
/// what they were before it.
async fn undo_lists(
    state: &State,
    target: &str,
    ops: &[OutboxRow],
    op_id: &str,
) -> Result<ResponseData, ErrorPayload> {
    let mut inverse: Vec<ListExtensionOp> = Vec::new();
    for op in ops {
        if rejected(op)? {
            continue;
        }
        let Some(list) = state
            .store
            .list(&op.entity_local_id)
            .await
            .map_err(store_error)?
        else {
            return Err(error_payload(
                ErrorKind::NotFound,
                format!(
                    "list {} was deleted since {target}, so it can't be undone",
                    op.entity_local_id
                ),
            ));
        };
        check_list_unchanged(op, &list)?;
        inverse.push(ListExtensionOp {
            op_id: op_id_for(op_id, inverse.len()),
            list_local_id: op.entity_local_id.clone(),
            action: op.action.clone(),
            fields: inverse_fields(op)?,
        });
    }
    if inverse.is_empty() {
        return Err(nothing_to_undo(target));
    }
    queue_lists(state, op_id, Some(target), inverse, TaskAction::Undo).await
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

/// Why undoing the move `op` would overwrite a later change to its task,
/// if it would: the task has moved on to another list, or the copy the
/// move made has changed since (D-047).
fn moved_since(op: &OutboxRow, row: &TaskRow) -> Option<String> {
    if row.list_local_id != op.list_local_id {
        return Some("has moved to another list since; undo would overwrite that".into());
    }
    let progress = move_job::Progress::of(op);
    let copy = progress.copy.as_ref()?;
    let changed = move_job::differences(
        &move_job::comparable(copy, progress.copy_extension.as_ref()),
        &move_job::comparable(&row.raw, row.extension.as_ref()),
    );
    (!changed.is_empty()).then(|| {
        format!(
            "has changed since ({}); undo would overwrite that",
            changed.join(", ")
        )
    })
}

fn conflict(message: String) -> ErrorPayload {
    error_payload(ErrorKind::Conflict, message)
}

/// Whether `op` is skipped because Graph rejected it, so it changed
/// nothing. One whose outcome is `unknown` can't be undone yet.
fn rejected(op: &OutboxRow) -> Result<bool, ErrorPayload> {
    match op.state {
        OpState::Failed => Ok(true),
        OpState::Unknown => Err(error_payload(
            ErrorKind::InvalidInput,
            format!(
                "operation {} may or may not have reached Microsoft To Do, so it can't be \
                 undone yet; see `ms-todo outbox list --state unknown`",
                op.op_id
            ),
        )),
        OpState::Pending | OpState::Inflight | OpState::Done => Ok(false),
    }
}

fn nothing_to_undo(target: &str) -> ErrorPayload {
    error_payload(
        ErrorKind::InvalidInput,
        format!("nothing to undo: Microsoft To Do rejected {target}, so it changed nothing"),
    )
}

/// The fields `op` sent, set back to what they were, and the action that
/// is.
fn inverse_update(op: &OutboxRow) -> Result<(Value, TaskAction), ErrorPayload> {
    let body = inverse_fields(op)?;
    let action = match op.action.as_str() {
        "complete" => TaskAction::Reopen,
        "reopen" => TaskAction::Complete,
        _ => TaskAction::Edit,
    };
    Ok((Value::Object(body), action))
}

/// Each field `op` sent, with its value before `op` (null where it had
/// none, which removes it), a date written as its local day.
fn inverse_fields(op: &OutboxRow) -> Result<Map<String, Value>, ErrorPayload> {
    let before = before(op)?;
    Ok(op
        .body()
        .as_object()
        .map(|fields| {
            fields
                .keys()
                .map(|key| {
                    let value = before.get(key).cloned().unwrap_or(Value::Null);
                    (key.clone(), as_written(key, value))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// A create of the task `op` deleted, from what it was then. The task
/// keeps its local ID and gets a new Graph ID when it's sent.
fn recreate(op_id: String, row: &TaskRow, op: &OutboxRow) -> Result<NewOp, ErrorPayload> {
    let before = before(op)?;
    let mut body = creatable_fields(before);
    body.insert(
        "extensions".into(),
        json!([our_extension(&op_id, row.extension.as_ref())]),
    );
    let body = Value::Object(body);
    let mut raw = before.clone();
    // What it will be, less what only Graph knows.
    raw.extend(
        new_task_raw(&body)
            .into_iter()
            .filter(|(key, _)| !before.contains_key(key)),
    );
    Ok(NewOp {
        op_id,
        entity_local_id: row.local_id.clone(),
        list_local_id: row.list_local_id.clone(),
        op: OpKind::Create,
        action: "add".into(),
        payload: json!({ "body": body }),
        change: LocalChange::Replace(raw),
    })
}

/// The completed copy a recurring completion made, as `--copy` names it.
async fn pick_copy(
    state: &State,
    op: &OutboxRow,
    row: &TaskRow,
    copy: Option<&str>,
) -> Result<TaskRow, ErrorPayload> {
    if op.state != OpState::Done {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            format!(
                "completing this recurring task hasn't reached Microsoft To Do yet; \
                 `ms-todo outbox discard {}` cancels it",
                op.op_id
            ),
        ));
    }
    let candidates = copies(state, op, row).await?;
    let chosen = copy.and_then(|copy| {
        candidates
            .iter()
            .find(|task| task.local_id == copy || task.graph_id.as_deref() == Some(copy))
    });
    if let Some(chosen) = chosen {
        return Ok(chosen.clone());
    }
    let message = match (copy, candidates.is_empty()) {
        (_, true) => format!(
            "can't undo yet: the completed copy of {:?} that Microsoft To Do makes hasn't synced; \
             run `ms-todo sync --wait` and try again",
            row.title
        ),
        (None, false) => format!(
            "completing a recurring task made a completed copy; name the one to delete with \
             `ms-todo undo {} --copy <ID>`",
            op.command_id
        ),
        (Some(copy), false) => {
            format!("{copy:?} isn't one of the completed copies; pick one of these")
        }
    };
    Err(ErrorPayload {
        undo_target: Some(op.command_id.clone()),
        candidates: candidates
            .iter()
            .map(|task| Candidate {
                id: task.local_id.clone(),
                name: task.title.clone(),
                created_at: text(&task.raw, "createdDateTime"),
                list_id: Some(task.list_local_id.clone()),
            })
            .collect(),
        ..error_payload(ErrorKind::InvalidInput, message)
    })
}

/// The candidates for the completed copy (04): new tasks in the same list
/// with the original's title, `completed`, created at or after the
/// completion was sent, less 5 minutes.
async fn copies(
    state: &State,
    op: &OutboxRow,
    row: &TaskRow,
) -> Result<Vec<TaskRow>, ErrorPayload> {
    let sent = DateTime::from_timestamp(op.sent_at.unwrap_or(op.created_at), 0).unwrap_or_default()
        - Duration::minutes(COPY_SLACK_MINUTES);
    let tasks = state
        .store
        .tasks_in_list(&row.list_local_id)
        .await
        .map_err(store_error)?;
    Ok(tasks
        .into_iter()
        .filter(|task| {
            task.local_id != row.local_id
                && task.title == row.title
                && text(&task.raw, "status").as_deref() == Some("completed")
                && text(&task.raw, "createdDateTime")
                    .and_then(|created| DateTime::parse_from_rfc3339(&created).ok())
                    .is_some_and(|created| created >= sent)
        })
        .collect())
}

fn before(op: &OutboxRow) -> Result<&Entity, ErrorPayload> {
    op.rollback.as_ref().ok_or_else(|| {
        error_payload(
            ErrorKind::Internal,
            format!("operation {} has no record of the task before it", op.op_id),
        )
    })
}

fn changed_since(op: &OutboxRow) -> ErrorPayload {
    error_payload(
        ErrorKind::InvalidInput,
        format!(
            "task {} changed since {} (it was deleted or re-created), so it can't be undone",
            op.entity_local_id, op.command_id
        ),
    )
}

fn text(task: &Entity, key: &str) -> Option<String> {
    task.get(key).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ms_todo_store::OpKind;

    fn op(action: &str, body: Value, before: Value) -> OutboxRow {
        OutboxRow {
            op_id: "op-1".into(),
            seq: 1,
            command_id: "op-1".into(),
            created_at: 0,
            entity_local_id: "t1".into(),
            list_local_id: "l1".into(),
            op: OpKind::Update,
            action: action.into(),
            payload: json!({ "body": body }),
            depends_on: None,
            undoes: None,
            attempts: 1,
            next_attempt_at: 0,
            state: OpState::Done,
            last_error: None,
            rollback: before.as_object().cloned(),
            sent_at: None,
            unknown_since: None,
            note: None,
            finished_at: None,
            progress: None,
            title: None,
        }
    }

    #[test]
    fn undoing_a_completion_reopens_and_an_edit_puts_back_only_its_fields() {
        let before = json!({ "status": "notStarted", "title": "Milk", "importance": "low" });
        let (body, action) = inverse_update(&op(
            "complete",
            json!({ "status": "completed" }),
            before.clone(),
        ))
        .expect("inverse");
        assert_eq!(action, TaskAction::Reopen);
        assert_eq!(body, json!({ "status": "notStarted" }));

        let edit = op(
            "edit",
            json!({ "title": "Oat milk", "dueDateTime": { "dateTime": "x" } }),
            before,
        );
        let (body, action) = inverse_update(&edit).expect("inverse");
        assert_eq!(action, TaskAction::Edit);
        assert_eq!(body, json!({ "title": "Milk", "dueDateTime": null }));
    }
}
