//! What undoing an operation's local change means for its task (or, for a
//! folder write, its list's extension), and failing an operation Graph
//! rejected (docs/blueprint/02-data-model.md#outbox-semantics).

use ms_todo_core::message_with_causes;
use ms_todo_protocol::ErrorPayload;
use ms_todo_store::{
    Entity, LISTS_SCOPE, OpKind, OutboxRow, Restore, StoreError, revert_child, tasks_scope,
};
use serde_json::Value;

use crate::handlers::State;

/// Put back what `op`'s local change did to the task, whose JSON is now
/// `current`: a create goes, a delete comes back, and an update's fields
/// return to what they were before it (other fields are left as they are,
/// since later operations may have changed them).
pub(super) fn undo_local(op: &OutboxRow, current: Option<&Entity>) -> Restore {
    match (op.op, current, &op.rollback) {
        (OpKind::Create, ..) => Restore::Tombstone,
        (
            OpKind::Update | OpKind::Extension | OpKind::TaskExtension,
            Some(current),
            Some(before),
        ) => Restore::Replace(revert_fields(current, op.body(), before)),
        (OpKind::Delete, _, Some(before)) => Restore::Replace(before.clone()),
        (OpKind::Child, Some(current), Some(before)) => {
            Restore::Replace(revert_child(current, &op.payload, before))
        }
        // A move's is `move_job::move_back`, which needs its saved steps.
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
    let current = match current_of(state, op).await {
        Ok(current) => current,
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
    reconcile(state, op).await;
    announce(state, op);
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

/// What `op`'s local change applies to as it is now: its task's JSON, or
/// for an extension write its list's or task's extension (`{}` for none). `None` if it's
/// not cached, or the list is gone.
pub(super) async fn current_of(
    state: &State,
    op: &OutboxRow,
) -> Result<Option<Entity>, StoreError> {
    if op.op == OpKind::Extension {
        return Ok(state.store.list(&op.entity_local_id).await?.map(|list| {
            list.extension
                .and_then(|extension| extension.as_object().cloned())
                .unwrap_or_default()
        }));
    }
    if op.op == OpKind::TaskExtension {
        return Ok(state
            .store
            .task_any(&op.entity_local_id)
            .await?
            .map(|(row, _)| {
                row.extension
                    .and_then(|extension| extension.as_object().cloned())
                    .unwrap_or_default()
            }));
    }
    Ok(state
        .store
        .task_any(&op.entity_local_id)
        .await?
        .map(|(row, _)| row.raw))
}

/// Tell subscribers that `op`'s task or list changed.
pub(crate) fn announce(state: &State, op: &OutboxRow) {
    announce_entity(state, op.op, op.entity_local_id.clone());
}

/// Tell subscribers that the task, or for a folder write (`kind`
/// `Extension`) the list, `local_id` changed.
pub(crate) fn announce_entity(state: &State, kind: OpKind, local_id: String) {
    if kind == OpKind::Extension {
        state.events.changed(vec![local_id], Vec::new());
    } else {
        state.events.tasks_changed(vec![local_id]);
    }
}

/// After `op`'s outcome was decided without Graph's copy, have the next
/// pass read what it changed whole: its task's list, or for a folder
/// write every list, since the lists are one scope.
pub(super) async fn reconcile(state: &State, op: &OutboxRow) {
    if op.op != OpKind::Extension {
        return reconcile_list(state, &op.list_local_id).await;
    }
    if let Err(error) = state.store.reset_scope(LISTS_SCOPE).await {
        eprintln!(
            "ms-todo daemon: cannot reset the sync of the lists: {}",
            message_with_causes(&error)
        );
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
