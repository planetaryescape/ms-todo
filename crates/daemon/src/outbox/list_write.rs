//! Sending a list's create, rename or delete (rung 8e). A create is a
//! POST, never resent after it may have reached Graph: a lost answer is
//! `unknown` and flagged for the user, since a list carries no marker to
//! find it by. A rename and a delete are safe to resend.

use ms_todo_store::{OpKind, OutboxRow};

use super::send::{Attempt, Failure, classify};
use crate::handlers::{State, error_payload};

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
        (OpKind::ListDelete, Some(graph_id)) => state
            .graph
            .delete_list(&graph_id)
            .await
            .map(|()| Attempt::ListDeleted)
            .map_err(classify),
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
