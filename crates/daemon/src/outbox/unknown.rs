//! Looking for the outcome of `unknown` operations, after each sync pass
//! (docs/blueprint/04-sync-cache.md#unknown-outcome-d-028). The invariant:
//! an ambiguous outcome stays `unknown` until it's attributed or the user
//! resolves it. Not finding something never allows a resend or a delete.
//!
//! - **A create** is attributed only by an exact `opId` match. Every task
//!   sync sees new gets our extension fetched (hydration), so the cache
//!   itself is where to look: a live task with a Graph ID whose extension
//!   holds this `opId`. On a match, its list must answer a `GET` with 200
//!   (a 404 rejects it: the list is gone), then the task is adopted: an
//!   atomic merge into the operation's own task, which keeps its local ID.
//!   Two matches are a duplicate, left for the user.
//! - **A recurring completion** is never attributed, since the phone may
//!   have moved the due date. Each round records what was seen, for
//!   `outbox list`.
//! - After 24 hours an operation is flagged, and looking stops.

use ms_todo_core::{ErrorKind, message_with_causes};
use ms_todo_store::{OpKind, OpState, OutboxRow};

use super::now;
use super::rollback::reject;
use crate::handlers::{State, error_payload};
use crate::task_fields::graph_due_date;

/// Look for each `unknown` operation's outcome. Returns whether any was
/// resolved.
pub(super) async fn resolve(state: &State) -> bool {
    let ops = match state.store.outbox(Some(OpState::Unknown)).await {
        Ok(ops) => ops,
        Err(error) => {
            eprintln!(
                "ms-todo daemon: cannot read unknown operations: {}",
                message_with_causes(&error)
            );
            return false;
        }
    };
    let mut resolved = false;
    for op in ops.iter().filter(|op| !op.is_flagged(now())) {
        let outcome = match op.op {
            OpKind::Create => attribute_create(state, op).await,
            OpKind::Update if op.is_recurring_completion() => {
                observe_recurring(state, op).await;
                Ok(false)
            }
            _ => Ok(false),
        };
        match outcome {
            Ok(done) => resolved |= done,
            Err(error) => eprintln!(
                "ms-todo daemon: looking for the outcome of {}: {error}",
                op.op_id
            ),
        }
    }
    resolved
}

async fn attribute_create(state: &State, op: &OutboxRow) -> Result<bool, String> {
    let found = state
        .store
        .tasks_with_op_id(&op.op_id)
        .await
        .map_err(|error| message_with_causes(&error))?;
    let task = match found.as_slice() {
        [] => return Ok(false),
        [task] => task,
        several => {
            let ids: Vec<&str> = several.iter().map(|task| task.local_id.as_str()).collect();
            let note = format!(
                "DuplicateDetected: tasks {} all carry this opId; nothing was deleted. Keep one \
                 and delete the others, then `ms-todo outbox discard {}`",
                ids.join(", "),
                op.op_id
            );
            eprintln!("ms-todo daemon: {note}");
            state
                .store
                .set_note(&op.op_id, &note)
                .await
                .map_err(|error| message_with_causes(&error))?;
            return Ok(false);
        }
    };
    let list = state
        .store
        .list_state(&task.list_local_id)
        .await
        .map_err(|error| message_with_causes(&error))?;
    let Some((Some(list_graph_id), _)) = list else {
        return Ok(false);
    };
    match state.graph.get_list(&list_graph_id).await {
        Ok(_) => {
            let adopted = state
                .store
                .adopt(&op.op_id, &task.local_id)
                .await
                .map_err(|error| message_with_causes(&error))?;
            if !adopted {
                // The user retried or discarded it meanwhile.
                return Ok(false);
            }
            eprintln!(
                "ms-todo daemon: operation {} was found in Microsoft To Do by its opId and adopted",
                op.op_id
            );
            Ok(true)
        }
        // An absent list never leads to a resend or a re-create.
        Err(error) if error.status() == Some(404) => {
            let error = error_payload(
                ErrorKind::Rejected,
                "the task was created, but its list was deleted on another device; its content \
                 is kept in this failed entry"
                    .into(),
            );
            reject(state, op, &error).await;
            Ok(true)
        }
        Err(error) => Err(message_with_causes(&error)),
    }
}

/// Record whether the task's due date has moved since the completion was
/// sent. It stays `unknown` either way: the phone may have moved it.
async fn observe_recurring(state: &State, op: &OutboxRow) {
    let Ok(Some((row, _))) = state.store.task_any(&op.entity_local_id).await else {
        return;
    };
    let (Some(graph_id), Ok(Some((Some(list_graph_id), _)))) = (
        row.graph_id.as_deref(),
        state.store.list_state(&op.list_local_id).await,
    ) else {
        return;
    };
    let before = op.payload["due_before"].as_str().unwrap_or("none");
    let note = match state.graph.get_task(&list_graph_id, graph_id).await {
        Ok(task) => match graph_due_date(&task) {
            Some(due) if due.format(ms_todo_core::DATE_FORMAT).to_string() != before => format!(
                "may already be completed: due moved from {before} to {}",
                due.format(ms_todo_core::DATE_FORMAT)
            ),
            _ => format!(
                "the due date hasn't moved (still {before}), so it may not have been completed"
            ),
        },
        Err(error) if error.status() == Some(404) => "the task is gone from Microsoft To Do".into(),
        Err(_) => return,
    };
    if let Err(error) = state.store.set_note(&op.op_id, &note).await {
        eprintln!(
            "ms-todo daemon: cannot record what was seen for {}: {}",
            op.op_id,
            message_with_causes(&error)
        );
    }
}
