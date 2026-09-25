//! A task's children as the cache holds them: its steps (Graph's
//! `checklistItems`) and its link (`linkedResources`), inline in the
//! task's JSON as Graph sends them in every task read and delta round
//! (S1), and its attachments' metadata (`attachments`), which Graph never
//! sends inline: the daemon fetches it and keeps it there (D-056). There
//! are no tables for them (D-055): a child write is an outbox operation on
//! its task, and what it does to the task's JSON is [`apply_child`], whose
//! inverse for a rejection or a discard is [`revert_child`].
//!
//! Since Graph's JSON never has `attachments`, the key is ms-todo's own:
//! present, even empty, it's the task's attachments as last known; absent,
//! they haven't been fetched, and a write of Graph's JSON keeps what the
//! cache had ([`crate::tasks`]).
//!
//! A child ms-todo creates has no Graph ID until its POST is answered, so
//! it's cached under a placeholder ID ([`LOCAL_CHILD_PREFIX`]) that later
//! operations on it name too; the store swaps in Graph's ID, in the task
//! and in those operations, when the create is recorded.

use serde_json::{Map, Value};

use crate::Entity;

pub use ms_todo_core::LOCAL_CHILD_PREFIX;

/// The collections a child operation writes to.
pub const STEPS: &str = "checklistItems";
pub const LINKS: &str = "linkedResources";
pub const ATTACHMENTS: &str = "attachments";

/// What a child operation does, as its payload's `verb` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildVerb {
    Create,
    Update,
    Delete,
}

impl ChildVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    pub fn of(payload: &Value) -> Option<Self> {
        match payload["verb"].as_str()? {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// A child operation's payload: which collection, what it does, to which
/// child, and the fields it sends. `carried` names fields sent only
/// because Graph resets what a PATCH leaves out (a step's `isChecked`,
/// S1), which aren't the change itself.
pub fn child_payload(
    collection: &str,
    verb: ChildVerb,
    id: &str,
    body: Value,
    carried: &[&str],
) -> Value {
    serde_json::json!({
        "collection": collection,
        "verb": verb.as_str(),
        "id": id,
        "body": body,
        "carried": carried,
    })
}

/// A task's children in `collection`: none when Graph left the key out,
/// which it does for an empty collection (S1).
pub fn children<'a>(task: &'a Entity, collection: &str) -> &'a [Value] {
    task.get(collection)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The child `id` of `task` in `collection`, and where it is.
pub fn find_child<'a>(task: &'a Entity, collection: &str, id: &str) -> Option<(usize, &'a Value)> {
    children(task, collection)
        .iter()
        .enumerate()
        .find(|(_, child)| child["id"] == id)
}

/// Make a child operation's change to a task's JSON, as Graph will.
pub fn apply_child(raw: &mut Entity, payload: &Value) {
    let (Some(collection), Some(verb), Some(id)) = (
        payload["collection"].as_str(),
        ChildVerb::of(payload),
        payload["id"].as_str(),
    ) else {
        return;
    };
    let body = payload["body"].as_object().cloned().unwrap_or_default();
    let mut items = children(raw, collection).to_vec();
    match verb {
        ChildVerb::Create => {
            let mut item = body;
            item.insert("id".into(), Value::String(id.to_owned()));
            // Once, however often it's applied: a cached attachment list
            // already holds the creates it was written with.
            match items.iter_mut().find(|child| child["id"] == id) {
                Some(existing) => *existing = Value::Object(item),
                None => items.push(Value::Object(item)),
            }
        }
        ChildVerb::Update => {
            if let Some(Value::Object(item)) = items.iter_mut().find(|child| child["id"] == id) {
                merge_fields(item, &body);
            }
        }
        ChildVerb::Delete => items.retain(|child| child["id"] != id),
    }
    set_children(raw, collection, items);
}

/// Take a child operation's change back out of `current`, the task's JSON
/// now, from `before`, its JSON when the operation was queued: only that
/// child, so later changes to others stay.
pub fn revert_child(current: &Entity, payload: &Value, before: &Entity) -> Entity {
    let mut reverted = current.clone();
    let (Some(collection), Some(verb), Some(id)) = (
        payload["collection"].as_str(),
        ChildVerb::of(payload),
        payload["id"].as_str(),
    ) else {
        return reverted;
    };
    let mut items = children(current, collection).to_vec();
    let was = find_child(before, collection, id);
    match (verb, was) {
        (ChildVerb::Create, _) => items.retain(|child| child["id"] != id),
        (ChildVerb::Update, Some((_, Value::Object(was)))) => {
            if let Some(Value::Object(item)) = items.iter_mut().find(|child| child["id"] == id) {
                // Checking a step also stamps when; unchecking clears it.
                let mut keys: Vec<String> = payload["body"]
                    .as_object()
                    .map(|sent| sent.keys().cloned().collect())
                    .unwrap_or_default();
                keys.push("checkedDateTime".into());
                for key in &keys {
                    match was.get(key) {
                        Some(value) => item.insert(key.clone(), value.clone()),
                        None => item.remove(key),
                    };
                }
            }
        }
        (ChildVerb::Delete, Some((at, was))) if !items.iter().any(|child| child["id"] == id) => {
            items.insert(at.min(items.len()), was.clone());
        }
        _ => {}
    }
    set_children(&mut reverted, collection, items);
    reverted
}

/// Give the child `from` the ID `to`, keeping the rest of it, and Graph's
/// other fields for it (`created`, the POST's answer) on top.
pub(crate) fn rename_child(raw: &mut Entity, collection: &str, from: &str, created: &Entity) {
    let mut items = children(raw, collection).to_vec();
    if let Some(Value::Object(item)) = items.iter_mut().find(|child| child["id"] == from) {
        merge_fields(item, created);
    }
    set_children(raw, collection, items);
}

/// `fields` over `item`, as a PATCH is. Unchecking a step clears when it
/// was checked, as Graph does (S1).
fn merge_fields(item: &mut Map<String, Value>, fields: &Map<String, Value>) {
    for (key, value) in fields {
        item.insert(key.clone(), value.clone());
    }
    if fields.get("isChecked") == Some(&Value::Bool(false)) {
        item.remove("checkedDateTime");
    }
}

/// Graph leaves an empty collection out (S1), so the cache does too,
/// except for attachments, whose empty list says there are none (see the
/// module's docs), and which also set `hasAttachments` as Graph will.
fn set_children(raw: &mut Entity, collection: &str, items: Vec<Value>) {
    if collection == ATTACHMENTS {
        raw.insert("hasAttachments".into(), Value::Bool(!items.is_empty()));
        raw.insert(collection.to_owned(), Value::Array(items));
    } else if items.is_empty() {
        raw.remove(collection);
    } else {
        raw.insert(collection.to_owned(), Value::Array(items));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    fn step(id: &str, name: &str, checked: bool) -> Value {
        json!({ "id": id, "displayName": name, "isChecked": checked })
    }

    #[test]
    fn a_create_adds_the_child_last_and_a_delete_of_the_last_drops_the_key() {
        let mut raw = entity(json!({ "title": "Paint" }));
        let payload = child_payload(
            STEPS,
            ChildVerb::Create,
            "local-1",
            json!({ "displayName": "Buy paint", "isChecked": false }),
            &[],
        );
        apply_child(&mut raw, &payload);
        assert_eq!(raw[STEPS], json!([step("local-1", "Buy paint", false)]));
        apply_child(
            &mut raw,
            &child_payload(STEPS, ChildVerb::Delete, "local-1", json!({}), &[]),
        );
        assert!(raw.get(STEPS).is_none(), "Graph leaves an empty one out");
    }

    #[test]
    fn unchecking_clears_when_it_was_checked() {
        let mut raw = entity(json!({ STEPS: [{
            "id": "c1", "displayName": "a", "isChecked": true,
            "checkedDateTime": "2026-09-25T07:16:03Z"
        }] }));
        let payload = child_payload(
            STEPS,
            ChildVerb::Update,
            "c1",
            json!({ "isChecked": false }),
            &[],
        );
        apply_child(&mut raw, &payload);
        assert_eq!(raw[STEPS], json!([step("c1", "a", false)]));
    }

    #[test]
    fn reverting_touches_only_the_child_it_changed() {
        let before = entity(json!({ STEPS: [step("c1", "a", false), step("c2", "b", false)] }));
        // Since queued: c1 checked by this operation, c2 renamed by another.
        let current = entity(json!({ STEPS: [step("c1", "a", true), step("c2", "B", false)] }));
        let check = child_payload(
            STEPS,
            ChildVerb::Update,
            "c1",
            json!({ "isChecked": true }),
            &[],
        );
        assert_eq!(
            revert_child(&current, &check, &before)[STEPS],
            json!([step("c1", "a", false), step("c2", "B", false)])
        );
        let delete = child_payload(STEPS, ChildVerb::Delete, "c1", json!({}), &[]);
        let deleted = entity(json!({ STEPS: [step("c2", "B", false)] }));
        assert_eq!(
            revert_child(&deleted, &delete, &before)[STEPS],
            json!([step("c1", "a", false), step("c2", "B", false)]),
            "back where it was"
        );
        let create = child_payload(STEPS, ChildVerb::Create, "local-9", json!({}), &[]);
        let created = entity(json!({ STEPS: [step("local-9", "new", false)] }));
        assert!(
            revert_child(&created, &create, &entity(json!({})))
                .get(STEPS)
                .is_none()
        );
    }

    #[test]
    fn attachments_stay_as_an_empty_list_and_a_create_lands_once() {
        let mut raw = entity(json!({ "title": "Invoice", "hasAttachments": false }));
        let add = child_payload(
            ATTACHMENTS,
            ChildVerb::Create,
            "local-1",
            json!({ "name": "a.pdf", "contentType": "application/pdf", "size": 3 }),
            &[],
        );
        apply_child(&mut raw, &add);
        apply_child(&mut raw, &add);
        assert_eq!(raw[ATTACHMENTS].as_array().map(Vec::len), Some(1));
        assert_eq!(raw["hasAttachments"], true);
        let delete = child_payload(ATTACHMENTS, ChildVerb::Delete, "local-1", json!({}), &[]);
        apply_child(&mut raw, &delete);
        assert_eq!(raw[ATTACHMENTS], json!([]), "known to have none");
        assert_eq!(raw["hasAttachments"], false);
    }

    #[test]
    fn a_created_child_takes_graphs_id_and_fields() {
        let mut raw =
            entity(json!({ LINKS: [{ "id": "local-1", "webUrl": "https://a.example" }] }));
        let created = entity(json!({
            "id": "r1", "webUrl": "https://a.example", "applicationName": "ms-todo"
        }));
        rename_child(&mut raw, LINKS, "local-1", &created);
        assert_eq!(raw[LINKS], json!([created]));
    }
}
