//! Outlook categories and open extensions (rung 8e, D-058). Neither is
//! cached: both live beside the lists and tasks, change rarely, and need
//! Graph anyway to be named (a category by its name, an extension by what
//! it holds now). So a write goes straight to Graph, like `raw`, but typed:
//! a dry run answers the plan, the real run makes the change and records
//! it in the outbox `done` (op `remote`) with what it replaced, so `undo`
//! can reverse it, checking first that nothing changed it since.

mod categories;
mod extensions;

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Applied, ErrorPayload, Plan, ResponseData, TaskAction};
use ms_todo_store::{Entity, OutboxRow};
use serde_json::{Value, json};

use crate::handlers::{State, error_payload};
use crate::task_writes::action_name;
use crate::undo::rejected;

pub(crate) use categories::{change_category, list_categories};
pub(crate) use extensions::{change_extension, get_extension, list_extensions};

/// Undo a category or extension write (`remote`), after checking that
/// what it set still holds.
pub(crate) async fn undo(
    state: &State,
    target: &str,
    ops: &[OutboxRow],
    op_id: &str,
) -> Result<ResponseData, ErrorPayload> {
    let [op] = ops else {
        return Err(error_payload(
            ErrorKind::Internal,
            format!(
                "{target} has {} category or extension writes; one was expected",
                ops.len()
            ),
        ));
    };
    if rejected(op)? {
        return Err(crate::undo::nothing_to_undo(target));
    }
    let before = op.rollback.clone().unwrap_or_default();
    match op.payload["kind"].as_str() {
        Some("category") => categories::undo(state, target, op, before, op_id).await,
        Some("extension") => extensions::undo(state, target, op, before, op_id).await,
        _ => Err(error_payload(
            ErrorKind::Internal,
            format!("{target} isn't a change ms-todo knows how to undo"),
        )),
    }
}

/// Graph's JSON without its response annotations.
pub(super) fn clean(mut entity: Entity) -> Entity {
    entity.retain(|key, _| !key.starts_with("@odata.context"));
    entity
}

pub(super) fn text(entity: &Entity, key: &str) -> String {
    entity
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub(super) fn plan(action: TaskAction, target: Value, body: Value) -> ResponseData {
    ResponseData::Plan(Plan {
        action,
        list: None,
        targets: Vec::new(),
        lists: Vec::new(),
        changes: json!({ "target": target, "body": body }),
    })
}

pub(super) fn applied(
    op_id: String,
    action: TaskAction,
    item: Entity,
    undoes: Option<&str>,
) -> ResponseData {
    ResponseData::Applied(Applied {
        op_id,
        action,
        items: vec![item],
        list_ids: Vec::new(),
        rolled: Vec::new(),
        undoes: undoes.map(str::to_owned),
        refused: Vec::new(),
    })
}

/// Record a write Graph made, for `undo`. Graph has the change whatever
/// happens here, so a failure to record is logged, not returned: the
/// answer is still true, only `undo` can't reach it.
pub(super) async fn record(
    state: &State,
    op_id: &str,
    undoes: Option<&str>,
    entity: &str,
    action: TaskAction,
    payload: &Value,
    rollback: &Entity,
) {
    if let Err(error) = state
        .store
        .record_remote(
            op_id,
            undoes,
            entity,
            action_name(action),
            payload,
            rollback,
        )
        .await
    {
        eprintln!(
            "ms-todo daemon: made {op_id}, but couldn't record it for undo: {}",
            ms_todo_core::message_with_causes(&error)
        );
    }
}

pub(super) use crate::task_children::invalid;

pub(super) fn unknown_change() -> ErrorPayload {
    error_payload(
        ErrorKind::Unsupported,
        "this daemon doesn't know that change; restart it with `ms-todo daemon stop`".into(),
    )
}
