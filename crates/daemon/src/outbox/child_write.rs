//! Sending a `child` operation: a POST, PATCH or DELETE of one of a
//! task's steps or its link (D-055).
//!
//! - **Create:** a POST, never resent after it may have reached Graph: a
//!   step or link carries no marker to attribute it by, so a lost answer
//!   is `unknown` and flagged for the user at once (04).
//! - **Update and delete** send `If-Match` with the task's etag, which
//!   guards its children (S6, S15). On a 412 the task is read again: a
//!   child already as asked (or, for a delete, gone) is done; one whose
//!   fields we change were changed elsewhere is a conflict, overwriting
//!   nothing; otherwise it's resent with the new etag. A step's
//!   `isChecked` is only carried in an edit (S1), so it takes the value
//!   Graph has now rather than conflict.
//!
//! After each write the task is read back, since the write moved its etag
//! (S1); if that read fails, the next sync brings it.

use ms_todo_core::ErrorKind;
use ms_todo_store::{ChildVerb, Entity, OutboxRow, find_child};
use serde_json::{Map, Value};

use super::send::{Attempt, Failure, classify, etag};
use crate::entities::split_extension;
use crate::handlers::{State, error_payload};

pub(super) async fn send(
    state: &State,
    list: &str,
    task: &str,
    op: &OutboxRow,
    cached: &Entity,
) -> Result<Attempt, Failure> {
    let collection = op.payload["collection"].as_str().unwrap_or_default();
    let id = op.payload["id"].as_str().unwrap_or_default();
    let body = op.body();
    match ChildVerb::of(&op.payload) {
        Some(ChildVerb::Create) => {
            let created = state
                .graph
                .create_child(list, task, collection, body)
                .await
                .map_err(classify)?;
            Ok(Attempt::ChildCreated {
                created,
                task: fetch(state, list, task).await.ok(),
            })
        }
        Some(ChildVerb::Update) => {
            let sent = state
                .graph
                .update_child(list, task, collection, id, body, etag(cached))
                .await;
            match sent {
                Err(error) if error.status() == Some(412) => {
                    let (current, _) = fetch(state, list, task).await?;
                    let Some((_, now)) = find_child(&current, collection, id) else {
                        return Err(Failure::Rejected(error_payload(
                            ErrorKind::NotFound,
                            format!("the {} was deleted on another device", noun(collection)),
                        )));
                    };
                    let changed = changed_keys(op, body);
                    if changed.iter().all(|key| now.get(key) == body.get(key)) {
                        return Ok(Attempt::Changed(current, None));
                    }
                    conflict_check(op, collection, id, now, &changed)?;
                    let resend = with_carried(op, body, now);
                    state
                        .graph
                        .update_child(list, task, collection, id, &resend, etag(&current))
                        .await
                        .map_err(classify)?;
                }
                sent => {
                    sent.map_err(classify)?;
                }
            }
            Ok(done(state, list, task).await)
        }
        Some(ChildVerb::Delete) => {
            let deleted = state
                .graph
                .delete_child(list, task, collection, id, etag(cached))
                .await;
            match deleted {
                Err(error) if error.status() == Some(412) => {
                    let (current, _) = fetch(state, list, task).await?;
                    let Some((_, now)) = find_child(&current, collection, id) else {
                        return Ok(Attempt::Changed(current, None));
                    };
                    let fields: Vec<String> = compared(collection)
                        .iter()
                        .map(|&key| key.to_owned())
                        .collect();
                    conflict_check(op, collection, id, now, &fields)?;
                    state
                        .graph
                        .delete_child(list, task, collection, id, etag(&current))
                        .await
                        .map_err(classify)?;
                }
                deleted => deleted.map_err(classify)?,
            }
            Ok(done(state, list, task).await)
        }
        None => Err(Failure::Rejected(error_payload(
            ErrorKind::Internal,
            format!("operation {} doesn't say what it does", op.op_id),
        ))),
    }
}

/// What `outbox list` says of a child create whose answer was lost.
pub(super) fn unknown_note(op: &OutboxRow) -> Option<String> {
    let collection = op.payload["collection"].as_str()?;
    (ChildVerb::of(&op.payload) == Some(ChildVerb::Create)).then(|| {
        let noun = noun(collection);
        format!(
            "Microsoft To Do may have added the {noun}, and a {noun} carries nothing to find it \
             by: check the task, then `ms-todo outbox retry {}` to send it again or \
             `ms-todo outbox discard {}` if it's there",
            op.op_id, op.op_id
        )
    })
}

/// What a user sees of a child: an edit to any of these since it was
/// read means a delete would lose it.
fn compared(collection: &str) -> &'static [&'static str] {
    if collection == ms_todo_store::LINKS {
        crate::task_children::LINK_FIELDS
    } else {
        &["displayName", "isChecked"]
    }
}

fn noun(collection: &str) -> &'static str {
    if collection == ms_todo_store::LINKS {
        "link"
    } else {
        "step"
    }
}

/// The fields `op` changes, less those it only carries.
fn changed_keys(op: &OutboxRow, body: &Value) -> Vec<String> {
    let carried = op.carried();
    body.as_object()
        .map(|fields| {
            fields
                .keys()
                .filter(|key| !carried.contains(&key.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// A conflict if any of `keys` of the child moved on Graph since the
/// operation was queued (its rollback is the task then).
fn conflict_check(
    op: &OutboxRow,
    collection: &str,
    id: &str,
    now: &Value,
    keys: &[String],
) -> Result<(), Failure> {
    let before = op
        .rollback
        .as_ref()
        .and_then(|before| find_child(before, collection, id))
        .map(|(_, child)| child.clone())
        .unwrap_or(Value::Null);
    let moved: Vec<&str> = keys
        .iter()
        .filter(|key| before.get(key.as_str()) != now.get(key.as_str()))
        .map(String::as_str)
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    Err(Failure::Rejected(error_payload(
        ErrorKind::Conflict,
        format!(
            "the {} changed on another device since ms-todo last read it ({}), so nothing was \
             overwritten",
            noun(collection),
            moved.join(", ")
        ),
    )))
}

/// `body` with its carried fields as Graph has them now.
fn with_carried(op: &OutboxRow, body: &Value, now: &Value) -> Value {
    let mut resend: Map<String, Value> = body.as_object().cloned().unwrap_or_default();
    for key in op.carried() {
        if let Some(value) = now.get(key) {
            resend.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(resend)
}

/// The task as Graph has it now, and our extension if it said.
async fn fetch(
    state: &State,
    list: &str,
    task: &str,
) -> Result<(Entity, Option<Option<Value>>), Failure> {
    let fetched = state.graph.get_task(list, task).await.map_err(classify)?;
    Ok(split_extension(fetched))
}

/// The task read back after a write; if it can't be, the next sync
/// brings it.
async fn done(state: &State, list: &str, task: &str) -> Attempt {
    match fetch(state, list, task).await.ok() {
        Some((task, extension)) => Attempt::Changed(task, extension),
        None => Attempt::Sent,
    }
}
