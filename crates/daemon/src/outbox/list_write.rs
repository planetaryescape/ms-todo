//! Sending a list's create, rename or delete (rung 8e). A create is a
//! POST, never resent after it may have reached Graph: a lost answer is
//! `unknown` and flagged for the user, since a list carries no marker to
//! find it by. A rename and a delete are safe to resend.

use ms_todo_store::{AFTER_COMMAND, OpKind, OpState, OutboxRow};

use super::send::{Attempt, Failure, classify};
use crate::handlers::{State, error_payload, store_error};

pub(super) async fn send(state: &State, op: &OutboxRow) -> Result<Attempt, Failure> {
    let list = state
        .store
        .list_state(&op.entity_local_id)
        .await
        .map_err(|error| Failure::Temporary(crate::handlers::store_error(error)))?;
    let graph_id = list.and_then(|(graph_id, _)| graph_id);
    match (op.op, graph_id) {
        (OpKind::ListCreate, _) => state
            .graph
            .create_list(op.body())
            .await
            .map(Attempt::ListWritten)
            .map_err(classify),
        (OpKind::ListUpdate, Some(graph_id)) => state
            .graph
            .update_list(&graph_id, op.body())
            .await
            .map(Attempt::ListWritten)
            .map_err(classify),
        (OpKind::ListDelete, Some(graph_id)) => {
            if let Some(after) = op.payload[AFTER_COMMAND].as_str() {
                check_emptied(state, op, after, &graph_id).await?;
            }
            state
                .graph
                .delete_list(&graph_id)
                .await
                .map(|()| Attempt::ListDeleted)
                .map_err(classify)
        }
        (_, None) => Err(Failure::Rejected(error_payload(
            ms_todo_core::ErrorKind::Rejected,
            "the list was never created in Microsoft To Do, so there's nothing to change".into(),
        ))),
        _ => Err(Failure::Rejected(error_payload(
            ms_todo_core::ErrorKind::Internal,
            format!("{} isn't a list write", op.op.as_str()),
        ))),
    }
}

/// A merge's delete (D-067) goes ahead only when every move of the merge
/// `after` is done, the cache holds no task in the list, and Microsoft To
/// Do shows it empty: a delete takes whatever the list holds with it.
async fn check_emptied(
    state: &State,
    op: &OutboxRow,
    after: &str,
    graph_id: &str,
) -> Result<(), Failure> {
    let store = |error| Failure::Temporary(store_error(error));
    let expected = op.payload["moves"].as_u64().unwrap_or(0);
    let moved = state
        .store
        .command_ops(after)
        .await
        .map_err(store)?
        .iter()
        .filter(|move_op| move_op.op == OpKind::Move && move_op.state == OpState::Done)
        .count() as u64;
    let kept = |why: String| {
        Failure::Rejected(error_payload(
            ms_todo_core::ErrorKind::Rejected,
            format!(
                "{why}, so the list was kept; delete it with `ms-todo lists delete` if you mean to"
            ),
        ))
    };
    if moved < expected {
        return Err(kept(format!(
            "{} of the {expected} task(s) merge {after} was moving didn't move",
            expected - moved
        )));
    }
    let cached = state
        .store
        .list_task_count(&op.entity_local_id)
        .await
        .map_err(store)?;
    if cached > 0 {
        return Err(kept(format!("it holds {cached} task(s) again")));
    }
    let on_graph = state.graph.list_tasks(graph_id).await.map_err(classify)?;
    if !on_graph.is_empty() {
        return Err(kept(format!(
            "Microsoft To Do still shows {} task(s) in it",
            on_graph.len()
        )));
    }
    Ok(())
}
