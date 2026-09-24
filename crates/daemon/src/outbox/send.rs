//! Sending one outbox operation to Graph and settling it (the outcomes are
//! in the module docs of `outbox`).
//!
//! A PATCH sends `If-Match` with the etag the cache has for the task (S6);
//! task DELETE ignores it, so it's not sent. On a 412 the task is read
//! again once: if our fields already hold our values, it's done; if nobody
//! touched them, it's re-sent with the new etag; otherwise Graph's copy
//! changed the same field, and the operation is rejected as a conflict,
//! overwriting nothing.

use std::collections::BTreeMap;

use ms_todo_core::{DATE_FORMAT, ErrorKind, local_date_time, local_due_date, message_with_causes};
use ms_todo_graph::GraphError;
use ms_todo_protocol::ErrorPayload;
use ms_todo_store::{Entity, OpKind, OutboxRow};
use serde_json::Value;

use super::rollback::reject;
use super::{backoff, now};
use crate::entities::split_extension;
use crate::handlers::{State, error_payload, graph_error};
use crate::task_fields::graph_due_date;

/// How one attempt ended, before it's recorded.
enum Attempt {
    /// Graph's task after the change, and our extension if it said.
    Changed(Entity, Option<Option<Value>>),
    /// A 201: recorded, but not `done` until its list is confirmed live.
    Created {
        task: Entity,
        extension: Option<Option<Value>>,
        list_graph_id: String,
    },
    Deleted,
}

enum Failure {
    Temporary(ErrorPayload),
    Rejected(ErrorPayload),
    /// Nothing says whether Graph applied it; `note` says what was seen.
    Unknown(ErrorPayload),
}

/// Send every operation that's ready, in queue order, round after round
/// until none is. Returns whether anything was sent.
pub(super) async fn send_ready(state: &State) -> bool {
    let mut sent = false;
    loop {
        let ops = match state.store.ready_ops(now()).await {
            Ok(ops) => ops,
            Err(error) => {
                log_store(&error);
                return sent;
            }
        };
        if ops.is_empty() {
            return sent;
        }
        // Creates that got a 201 this round, by list: each list is checked
        // once, after all of them.
        let mut created: BTreeMap<String, Vec<OutboxRow>> = BTreeMap::new();
        let mut deferred = false;
        for op in ops {
            match state.store.mark_inflight(&op.op_id).await {
                Ok(true) => {}
                // Discarded or retried since it was read.
                Ok(false) => continue,
                Err(error) => {
                    log_store(&error);
                    return sent;
                }
            }
            sent = true;
            match attempt(state, &op).await {
                Ok(Attempt::Created {
                    task,
                    extension,
                    list_graph_id,
                }) => {
                    if record(state, &op, &task, extension, false).await {
                        created.entry(list_graph_id).or_default().push(op);
                    }
                }
                Ok(Attempt::Changed(task, extension)) => {
                    record(state, &op, &task, extension, true).await;
                }
                Ok(Attempt::Deleted) => {
                    if let Err(error) = state.store.mark_done(&op.op_id).await {
                        log_store(&error);
                    }
                }
                Err(Failure::Temporary(error)) => {
                    let until = now() + backoff(op.attempts + 1);
                    if let Err(store) = state
                        .store
                        .defer(&op.op_id, until, (&error.kind, &error.message))
                        .await
                    {
                        log_store(&store);
                    }
                    // Whatever stopped this stops the rest too.
                    deferred = true;
                    break;
                }
                Err(Failure::Rejected(error)) => reject(state, &op, &error).await,
                Err(Failure::Unknown(error)) => mark_unknown(state, &op, &error, None).await,
            }
        }
        for (list_graph_id, ops) in created {
            confirm_list(state, &list_graph_id, &ops).await;
        }
        if deferred {
            return sent;
        }
    }
}

/// The ghost-write check (04): a POST into a list deleted meanwhile still
/// answers 201 for a while (S4), so a create is `done` only when a GET of
/// its list, sent after the 201, answers 200. A 404 rejects it, keeping
/// the task's content in the `failed` operation.
async fn confirm_list(state: &State, list_graph_id: &str, ops: &[OutboxRow]) {
    match state.graph.get_list(list_graph_id).await {
        Ok(_) => {
            for op in ops {
                if let Err(error) = state.store.mark_done(&op.op_id).await {
                    log_store(&error);
                }
            }
        }
        Err(error) if error.status() == Some(404) => {
            let error = ErrorPayload {
                kind: ErrorKind::Rejected.as_str().to_owned(),
                message: "the list was deleted on another device, so the task can't be seen \
                          there; its content is kept in this failed entry"
                    .into(),
                ..graph_error(error)
            };
            for op in ops {
                reject(state, op, &error).await;
            }
        }
        Err(error) => {
            let error = graph_error(error);
            for op in ops {
                let note = "Graph created the task (201), but its list couldn't be confirmed \
                            live yet; it's checked again after the next sync";
                mark_unknown(state, op, &error, Some(note)).await;
            }
        }
    }
}

async fn attempt(state: &State, op: &OutboxRow) -> Result<Attempt, Failure> {
    let list = state
        .store
        .list_state(&op.list_local_id)
        .await
        .map_err(store)?;
    let Some((Some(list_graph_id), false)) = list else {
        return Err(rejected(
            "the list was deleted on another device, so this was never sent; the task's \
             content is kept here"
                .into(),
        ));
    };
    let task = state
        .store
        .task_any(&op.entity_local_id)
        .await
        .map_err(store)?
        .map(|(row, _)| row)
        .ok_or_else(|| rejected(format!("task {} isn't cached", op.entity_local_id)))?;
    if op.op == OpKind::Create {
        return match state.graph.create_task(&list_graph_id, op.body()).await {
            Ok(created) => {
                let (task, extension) = split_extension(created);
                Ok(Attempt::Created {
                    task,
                    extension,
                    list_graph_id,
                })
            }
            Err(error) => Err(classify(error)),
        };
    }
    let Some(graph_id) = task.graph_id.clone() else {
        return Err(rejected(
            "the task was never created in Microsoft To Do".into(),
        ));
    };
    if op.op == OpKind::Delete {
        // A 404 counts as deleted (the client says so).
        return match state.graph.delete_task(&list_graph_id, &graph_id).await {
            Ok(()) => Ok(Attempt::Deleted),
            Err(error) => Err(classify(error)),
        };
    }
    patch(state, op, &list_graph_id, &graph_id, &task.raw).await
}

async fn patch(
    state: &State,
    op: &OutboxRow,
    list_graph_id: &str,
    graph_id: &str,
    cached: &Entity,
) -> Result<Attempt, Failure> {
    let body = op.body();
    let recurring = op.is_recurring_completion();
    let sent = state
        .graph
        .update_task(list_graph_id, graph_id, body, etag(cached), !recurring)
        .await;
    let (seen, sent) = match sent {
        Err(error) if error.status() == Some(412) => {
            // The cached etag is stale: the task changed since we read it.
            let fetched = state
                .graph
                .get_task(list_graph_id, graph_id)
                .await
                .map_err(classify)?;
            let (current, extension) = split_extension(fetched);
            if !recurring && is_applied(body, &current) {
                // Ours already, e.g. a resend whose first try went through.
                return Ok(Attempt::Changed(current, extension));
            }
            let before = op.rollback.as_ref().unwrap_or(cached);
            let touched = touched_keys(before, &current, body, recurring);
            if !touched.is_empty() {
                return Err(Failure::Rejected(error_payload(
                    ErrorKind::Conflict,
                    format!(
                        "the task changed on another device since ms-todo last read it ({}), so \
                         nothing was overwritten",
                        touched.join(", ")
                    ),
                )));
            }
            let sent = state
                .graph
                .update_task(list_graph_id, graph_id, body, etag(&current), !recurring)
                .await;
            (current, sent)
        }
        sent => (cached.clone(), sent),
    };
    match sent {
        Ok(updated) => {
            let (updated, extension) = split_extension(updated);
            if recurring && !rolled_on(&seen, &updated) {
                return Err(Failure::Unknown(error_payload(
                    ErrorKind::OutcomeUnknown,
                    "Graph accepted completing this recurring task, but it's still not \
                     completed and its due date didn't move on"
                        .into(),
                )));
            }
            Ok(Attempt::Changed(updated, extension))
        }
        Err(error) => Err(classify(error)),
    }
}

/// S12: completing a recurring task keeps its ID, moves its due date to
/// the next occurrence and leaves it `notStarted`. Success is a 200 with
/// either `completed` or a moved due date
/// (docs/blueprint/04-sync-cache.md#completing-a-recurring-task).
fn rolled_on(before: &Entity, after: &Entity) -> bool {
    if after.get("status").and_then(Value::as_str) == Some("completed") {
        return true;
    }
    matches!(
        (graph_due_date(before), graph_due_date(after)),
        (Some(was), Some(next)) if next > was
    )
}

/// Whether `task`, as Graph returned it, already holds every field of
/// `body`. Dates are compared as dates: Graph writes them back in its own
/// shape.
fn is_applied(body: &Value, task: &Entity) -> bool {
    let Value::Object(fields) = body else {
        return false;
    };
    fields.iter().all(|(key, sent)| {
        let current = task.get(key.as_str()).unwrap_or(&Value::Null);
        match key.as_str() {
            "dueDateTime" | "startDateTime" => as_date(sent) == as_date(current),
            "reminderDateTime" => as_time(sent) == as_time(current),
            "body" => sent.get("content") == current.get("content"),
            _ => sent == current,
        }
    })
}

fn as_date(value: &Value) -> Option<String> {
    local_due_date(
        value.get("dateTime")?.as_str()?,
        value.get("timeZone")?.as_str()?,
    )
    .map(|date| date.format(DATE_FORMAT).to_string())
}

fn as_time(value: &Value) -> Option<String> {
    local_date_time(
        value.get("dateTime")?.as_str()?,
        value.get("timeZone")?.as_str()?,
    )
    .map(|at| at.format("%Y-%m-%dT%H:%M").to_string())
}

/// The fields we're changing whose server value moved between our read and
/// now. Completing a recurring task also watches the due date: if it moved,
/// someone else completed that occurrence, and re-sending would complete
/// the next one too.
fn touched_keys(before: &Entity, now: &Entity, body: &Value, recurring: bool) -> Vec<String> {
    let mut keys: Vec<String> = body
        .as_object()
        .map(|fields| fields.keys().cloned().collect())
        .unwrap_or_default();
    if recurring {
        keys.push("dueDateTime".into());
    }
    keys.retain(|key| before.get(key) != now.get(key));
    keys.dedup();
    keys
}

/// A failure's meaning for the outbox. Only a create or a recurring
/// completion can be `OutcomeUnknown`: every other request is idempotent,
/// so the client retries it itself.
fn classify(error: GraphError) -> Failure {
    match error.kind() {
        ErrorKind::OutcomeUnknown => Failure::Unknown(graph_error(error)),
        ErrorKind::InvalidInput
        | ErrorKind::NotFound
        | ErrorKind::Conflict
        | ErrorKind::Rejected
        | ErrorKind::Unsupported => Failure::Rejected(graph_error(error)),
        _ => Failure::Temporary(graph_error(error)),
    }
}

fn rejected(message: String) -> Failure {
    Failure::Rejected(error_payload(ErrorKind::Rejected, message))
}

fn store(error: ms_todo_store::StoreError) -> Failure {
    Failure::Temporary(crate::handlers::store_error(error))
}

fn etag(task: &Entity) -> Option<&str> {
    task.get("@odata.etag").and_then(Value::as_str)
}

/// Record what Graph returned for `op`'s task. Returns false if the cache
/// couldn't: Graph has the change, so the operation mustn't look safe to
/// resend, and it goes to `unknown` (a restart would do the same).
async fn record(
    state: &State,
    op: &OutboxRow,
    task: &Entity,
    extension: Option<Option<Value>>,
    done: bool,
) -> bool {
    match state
        .store
        .record_sent(&op.op_id, task, extension, done)
        .await
    {
        Ok(()) => true,
        Err(error) => {
            log_store(&error);
            let error = error_payload(
                ErrorKind::OutcomeUnknown,
                format!(
                    "Graph took the change, but the cache couldn't record it: {}",
                    message_with_causes(&error)
                ),
            );
            mark_unknown(state, op, &error, None).await;
            false
        }
    }
}

async fn mark_unknown(state: &State, op: &OutboxRow, error: &ErrorPayload, note: Option<&str>) {
    eprintln!(
        "ms-todo daemon: the outcome of operation {} is unknown ({}); it's never resent by itself",
        op.op_id, error.message
    );
    if let Err(store) = state
        .store
        .mark_unknown(&op.op_id, (&error.kind, &error.message), note)
        .await
    {
        log_store(&store);
    }
}

fn log_store(error: &ms_todo_store::StoreError) {
    eprintln!(
        "ms-todo daemon: the outbox couldn't use the cache: {}",
        message_with_causes(error)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn a_rolled_recurring_completion_counts_as_done() {
        let before = entity(json!({
            "status": "notStarted",
            "dueDateTime": { "dateTime": "2026-09-24T00:00:00.0000000", "timeZone": "UTC" }
        }));
        let after = entity(json!({
            "status": "notStarted",
            "dueDateTime": { "dateTime": "2026-10-01T00:00:00.0000000", "timeZone": "UTC" }
        }));
        assert!(rolled_on(&before, &after));
        assert!(!rolled_on(&before, &before));
    }

    #[test]
    fn applied_values_are_recognised_in_graphs_shape() {
        let task = entity(json!({
            "title": "Buy milk",
            "dueDateTime": { "dateTime": "2026-09-26T00:00:00.0000000", "timeZone": "UTC" },
            "body": { "content": "2 pints", "contentType": "text" }
        }));
        let sent = json!({
            "title": "Buy milk",
            "dueDateTime": { "dateTime": "2026-09-26T00:00:00", "timeZone": "Europe/London" },
            "body": { "content": "2 pints", "contentType": "text" }
        });
        assert!(is_applied(&sent, &task));
        assert!(!is_applied(&json!({ "title": "Oat milk" }), &task));
    }

    #[test]
    fn only_the_fields_we_change_count_as_touched() {
        let before = entity(json!({ "title": "a", "importance": "low" }));
        let now = entity(json!({ "title": "b", "importance": "low" }));
        assert!(touched_keys(&before, &now, &json!({ "status": "completed" }), false).is_empty());
        assert_eq!(
            touched_keys(&before, &now, &json!({ "title": "c" }), false),
            ["title"]
        );
    }
}
