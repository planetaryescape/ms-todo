//! What undoing an operation's local change means for its task, and
//! failing an operation Graph rejected
//! (docs/blueprint/02-data-model.md#outbox-semantics).

use ms_todo_core::message_with_causes;
use ms_todo_protocol::ErrorPayload;
use ms_todo_store::{Entity, OpKind, OutboxRow, Restore, tasks_scope};
use serde_json::Value;

use crate::handlers::State;

/// Put back what `op`'s local change did to the task, whose JSON is now
/// `current`: a create goes, a delete comes back, and an update's fields
/// return to what they were before it (other fields are left as they are,
/// since later operations may have changed them).
pub(super) fn undo_local(op: &OutboxRow, current: Option<&Entity>) -> Restore {
    match (op.op, current, &op.rollback) {
        (OpKind::Create, ..) => Restore::Tombstone,
        (OpKind::Update, Some(current), Some(before)) => {
            Restore::Replace(revert_fields(current, op.body(), before))
        }
        (OpKind::Delete, _, Some(before)) => Restore::Replace(before.clone()),
        _ => Restore::Nothing,
    }
}

/// `current` with each field in `body` set back to its value in `before`,
/// or removed if `before` didn't have it.
pub(crate) fn revert_fields(current: &Entity, body: &Value, before: &Entity) -> Entity {
    let mut reverted = current.clone();
    if let Value::Object(fields) = body {
        for key in fields.keys() {
            match before.get(key) {
                Some(value) => reverted.insert(key.clone(), value.clone()),
                None => reverted.remove(key),
            };
        }
    }
    reverted
}

/// Graph rejected `op` for good: mark it `failed`, roll its local change
/// back, and send `WriteRejected`. The task's list is then read whole on
/// the next pass, since its changes were skipped while `op` was pending.
pub(crate) async fn reject(state: &State, op: &OutboxRow, error: &ErrorPayload) {
    let current = match state.store.task_any(&op.entity_local_id).await {
        Ok(row) => row.map(|(row, _)| row.raw),
        Err(error) => {
            eprintln!(
                "ms-todo daemon: cannot read task {}: {}",
                op.entity_local_id,
                message_with_causes(&error)
            );
            None
        }
    };
    let restore = undo_local(op, current.as_ref());
    let cascaded = match state
        .store
        .fail_op(&op.op_id, (&error.kind, &error.message), &restore)
        .await
    {
        Ok(cascaded) => cascaded,
        Err(store) => {
            eprintln!(
                "ms-todo daemon: cannot record that Graph rejected {}: {}",
                op.op_id,
                message_with_causes(&store)
            );
            return;
        }
    };
    reconcile_list(state, &op.list_local_id).await;
    state.events.tasks_changed(vec![op.entity_local_id.clone()]);
    state
        .events
        .write_rejected(&op.op_id, &op.entity_local_id, &error.kind, &error.message);
    for later in cascaded {
        let message = format!("the task was never created: its add ({}) failed", op.op_id);
        state
            .events
            .write_rejected(&later, &op.entity_local_id, &error.kind, &message);
    }
}

/// Have the next pass read `list_local_id`'s tasks whole: after an
/// operation's outcome was decided without Graph's copy of its task, the
/// cache may hold what Graph doesn't.
pub(super) async fn reconcile_list(state: &State, list_local_id: &str) {
    let Ok(Some((Some(graph_id), _))) = state.store.list_state(list_local_id).await else {
        return;
    };
    if let Err(error) = state.store.reset_scope(&tasks_scope(&graph_id)).await {
        eprintln!(
            "ms-todo daemon: cannot reset the sync of list {list_local_id}: {}",
            message_with_causes(&error)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn reverting_touches_only_the_fields_sent() {
        let before = entity(json!({ "title": "Milk", "importance": "low" }));
        let current = entity(json!({
            "title": "Oat milk", "importance": "high", "dueDateTime": { "dateTime": "x" }
        }));
        let body = json!({ "title": "Oat milk", "dueDateTime": { "dateTime": "x" } });
        assert_eq!(
            revert_fields(&current, &body, &before),
            entity(json!({ "title": "Milk", "importance": "high" }))
        );
    }
}
