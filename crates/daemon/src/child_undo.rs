//! Undoing a step or link write (`undo`, D-055): the write that puts the
//! child back as it was, planned from the operation and the task as
//! cached now. As for a task's fields (D-048), a child the write set that
//! has changed since is left alone, so an undo never overwrites a later
//! change (a step checked on the phone meanwhile, say).
//!
//! - An add is undone by deleting the child it made.
//! - An edit, check or uncheck by setting the fields it changed back. A
//!   link's field it set where there was none can't be cleared (S15), so
//!   that link is deleted and added again as it was.
//! - A delete by adding the child again, with every field it had (a
//!   step's `checkedDateTime` too, S15). It gets a new ID.

use ms_todo_protocol::TaskAction;
use ms_todo_store::{ChildVerb, Entity, LINKS, OutboxRow, STEPS, find_child};
use serde_json::{Map, Value};

use crate::task_children::{ChildWrite, LINK_FIELDS, STEP_FIELDS};

/// The writes that undo `op` on the task `raw`, or why it's left alone.
pub(crate) fn inverse(op: &OutboxRow, raw: &Entity) -> Result<Vec<ChildWrite>, String> {
    let collection = if op.payload["collection"] == LINKS {
        LINKS
    } else {
        STEPS
    };
    let noun = if collection == LINKS { "link" } else { "step" };
    let id = op.payload["id"].as_str().unwrap_or_default();
    let now = find_child(raw, collection, id).map(|(_, child)| child);
    let before = op
        .rollback
        .as_ref()
        .and_then(|before| find_child(before, collection, id))
        .map(|(_, child)| child);
    let body = op.body().as_object().cloned().unwrap_or_default();
    let (add, delete) = actions(collection);
    // Which one, for a refusal: a step by its text, a link by its URL.
    let label = now
        .or(before)
        .and_then(|child| {
            child.get(if collection == LINKS {
                "webUrl"
            } else {
                "displayName"
            })
        })
        .and_then(Value::as_str)
        .map(|label| format!(" {label:?}"))
        .unwrap_or_default();
    let noun = format!("{noun}{label}");
    let noun = noun.as_str();
    match ChildVerb::of(&op.payload) {
        Some(ChildVerb::Create) => {
            let Some(now) = now else {
                return Err(format!("its {noun} was deleted since"));
            };
            not_moved(now, &body, &[], noun)?;
            Ok(vec![ChildWrite::delete(collection, id, delete)])
        }
        Some(ChildVerb::Update) => {
            let (Some(now), Some(before)) = (now, before) else {
                return Err(format!("its {noun} was deleted since"));
            };
            // Sent but not changed (a rename's `isChecked`): it takes
            // what the step has now.
            let carried = op.carried();
            let carried = carried.as_slice();
            not_moved(now, &body, carried, noun)?;
            let mut back = Map::new();
            for key in body.keys() {
                let value = if carried.contains(&key.as_str()) {
                    now.get(key)
                } else {
                    before.get(key)
                };
                match value {
                    Some(value) => back.insert(key.clone(), value.clone()),
                    // A link's field that wasn't there before: Graph can't
                    // clear it, so the link goes back whole.
                    None => {
                        return Ok(vec![
                            ChildWrite::delete(collection, id, delete),
                            ChildWrite::create(collection, creatable(collection, before), add),
                        ]);
                    }
                };
            }
            let mut write = ChildWrite::update(
                collection,
                id,
                Value::Object(back),
                update_action(collection, &body),
            );
            if !carried.is_empty() {
                write.carried = crate::task_children::CARRIED_ON_STEPS;
            }
            Ok(vec![write])
        }
        Some(ChildVerb::Delete) => {
            let Some(before) = before else {
                return Err(format!("ms-todo has no record of the {noun} it deleted"));
            };
            Ok(vec![ChildWrite::create(
                collection,
                creatable(collection, before),
                add,
            )])
        }
        None => Err(format!("ms-todo can't tell what {} did", op.op_id)),
    }
}

/// The actions of the writes that undo a `collection` write: an add and
/// a delete.
fn actions(collection: &str) -> (TaskAction, TaskAction) {
    if collection == LINKS {
        (TaskAction::LinkAdd, TaskAction::LinkDelete)
    } else {
        (TaskAction::StepAdd, TaskAction::StepDelete)
    }
}

/// What an update that sets `body` back on a `collection` child is.
fn update_action(collection: &str, body: &Map<String, Value>) -> TaskAction {
    if collection == LINKS {
        return TaskAction::LinkEdit;
    }
    match (body.contains_key("displayName"), body.get("isChecked")) {
        (false, Some(Value::Bool(true))) => TaskAction::StepUncheck,
        (false, Some(_)) => TaskAction::StepCheck,
        _ => TaskAction::StepEdit,
    }
}

/// Refused if a field `body` set (less those only carried) doesn't hold
/// what it set any more.
fn not_moved(
    now: &Value,
    body: &Map<String, Value>,
    carried: &[&str],
    noun: &str,
) -> Result<(), String> {
    let moved: Vec<&str> = body
        .iter()
        .filter(|(key, set)| !carried.contains(&key.as_str()) && now.get(key.as_str()) != Some(set))
        .map(|(key, _)| key.as_str())
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    Err(format!(
        "its {noun} has changed since ({}); undo would overwrite that",
        moved.join(", ")
    ))
}

/// What a create of `child` again sends.
fn creatable(collection: &str, child: &Value) -> Value {
    let fields = if collection == LINKS {
        LINK_FIELDS
    } else {
        STEP_FIELDS
    };
    Value::Object(
        fields
            .iter()
            .filter_map(|&key| Some((key.to_owned(), child.get(key)?.clone())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use ms_todo_store::{OpKind, OpState, child_payload};
    use serde_json::json;

    use super::*;
    use crate::task_children::CARRIED_ON_STEPS;

    fn op(payload: Value, before: Value) -> OutboxRow {
        OutboxRow {
            op_id: "op-1".into(),
            seq: 1,
            command_id: "op-1".into(),
            created_at: 0,
            entity_local_id: "t1".into(),
            list_local_id: "l1".into(),
            op: OpKind::Child,
            action: "step_check".into(),
            payload,
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

    fn raw(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn a_check_is_undone_unless_the_step_moved_since() {
        let before = json!({ STEPS: [{ "id": "c1", "displayName": "a", "isChecked": false }] });
        let check = op(
            child_payload(
                STEPS,
                ChildVerb::Update,
                "c1",
                json!({ "isChecked": true }),
                &[],
            ),
            before,
        );
        let now = raw(json!({ STEPS: [{ "id": "c1", "displayName": "b", "isChecked": true }] }));
        let writes = inverse(&check, &now).expect("undoable");
        assert_eq!(writes[0].body, json!({ "isChecked": false }));
        assert_eq!(writes[0].action, TaskAction::StepUncheck);
        let unchecked =
            raw(json!({ STEPS: [{ "id": "c1", "displayName": "a", "isChecked": false }] }));
        let refused = inverse(&check, &unchecked).expect_err("moved");
        assert!(refused.contains("isChecked"), "{refused}");
    }

    #[test]
    fn a_rename_is_undone_keeping_the_step_checked_now() {
        let before = json!({ STEPS: [{ "id": "c1", "displayName": "a", "isChecked": false }] });
        let rename = op(
            child_payload(
                STEPS,
                ChildVerb::Update,
                "c1",
                json!({ "displayName": "b", "isChecked": false }),
                CARRIED_ON_STEPS,
            ),
            before,
        );
        let now = raw(json!({ STEPS: [{ "id": "c1", "displayName": "b", "isChecked": true }] }));
        let writes = inverse(&rename, &now).expect("undoable");
        assert_eq!(
            writes[0].body,
            json!({ "displayName": "a", "isChecked": true })
        );
    }

    #[test]
    fn a_delete_is_undone_by_adding_the_step_back_as_it_was() {
        let before = json!({ STEPS: [{
            "id": "c1", "displayName": "a", "isChecked": true,
            "checkedDateTime": "2026-09-20T10:00:00Z", "createdDateTime": "2026-09-01T10:00:00Z"
        }] });
        let delete = op(
            child_payload(STEPS, ChildVerb::Delete, "c1", json!({}), &[]),
            before,
        );
        let writes = inverse(&delete, &raw(json!({}))).expect("undoable");
        assert_eq!(writes[0].verb, ChildVerb::Create);
        assert_eq!(
            writes[0].body,
            json!({ "displayName": "a", "isChecked": true, "checkedDateTime": "2026-09-20T10:00:00Z" })
        );
    }

    #[test]
    fn a_link_field_set_where_there_was_none_puts_the_link_back_whole() {
        let before = json!({ LINKS: [{ "id": "r1", "webUrl": "https://a.example", "applicationName": "ms-todo" }] });
        let name = op(
            child_payload(
                LINKS,
                ChildVerb::Update,
                "r1",
                json!({ "displayName": "Spec" }),
                &[],
            ),
            before,
        );
        let now = raw(json!({ LINKS: [{
            "id": "r1", "webUrl": "https://a.example", "applicationName": "ms-todo", "displayName": "Spec"
        }] }));
        let writes = inverse(&name, &now).expect("undoable");
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].verb, ChildVerb::Delete);
        assert_eq!(
            writes[1].body,
            json!({ "webUrl": "https://a.example", "applicationName": "ms-todo" })
        );
    }

    #[test]
    fn an_add_is_undone_by_deleting_it_unless_it_was_deleted_or_changed() {
        let add = op(
            child_payload(
                STEPS,
                ChildVerb::Create,
                "c9",
                json!({ "displayName": "a", "isChecked": false }),
                &[],
            ),
            json!({}),
        );
        let now = raw(json!({ STEPS: [{ "id": "c9", "displayName": "a", "isChecked": false }] }));
        assert_eq!(
            inverse(&add, &now).expect("undoable")[0].verb,
            ChildVerb::Delete
        );
        assert!(inverse(&add, &raw(json!({}))).is_err(), "gone");
        let checked =
            raw(json!({ STEPS: [{ "id": "c9", "displayName": "a", "isChecked": true }] }));
        assert!(inverse(&add, &checked).is_err(), "checked since");
    }
}
