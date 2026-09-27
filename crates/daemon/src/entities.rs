//! Cached rows as clients see them (docs/blueprint/07-cli.md#output-contract):
//! Graph's JSON with every field, `id` replaced by the local ID and Graph's
//! beside it as `graph_id`, plus `sync_state`, and a task's `list_id`,
//! `defer_until` and `someday`. Our extension, when known, is under
//! `extensions` as Graph returns it.
//!
//! Also the other direction: Graph's JSON split into what the store keeps
//! as `raw_json` and our extension.

use ms_todo_core::DATE_FORMAT;
use ms_todo_core::deferral::{defer_until, someday};
use ms_todo_protocol::Entity;
use ms_todo_store::{ListRow, SearchHit, TaskRow};
use serde_json::{Map, Value, json};

/// Our open extension (docs/blueprint/05-custom-features.md).
pub(crate) const EXTENSION_NAME: &str = "com.planetaryescape.mstodo";

/// Our extension as a write sends it: our fields, without Graph's `id`,
/// `extensionName` and `@odata` keys of its own. A field's own annotation
/// (`scores@odata.type`) is kept while `changed` says the field isn't being
/// written anew, since Graph drops a type it isn't sent; a non-empty array
/// of strings with none gets `#Collection(String)`, which Graph needs to
/// take it (S21, D-067).
pub(crate) fn extension_document(
    extension: &Map<String, Value>,
    changed: impl Fn(&str) -> bool,
) -> Map<String, Value> {
    let mut document: Map<String, Value> = extension
        .iter()
        .filter(|(key, _)| {
            if *key == "id" || *key == "extensionName" || key.starts_with('@') {
                return false;
            }
            match key.split_once('@') {
                Some((field, _)) => extension.contains_key(field) && !changed(field),
                None => true,
            }
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    type_collections(&mut document);
    document
}

/// Give each non-empty array of strings in an open extension's `fields`
/// with no annotation the type Graph needs to take it. An empty one says
/// nothing of its type, so it's left for Graph to take or not.
pub(crate) fn type_collections(fields: &mut Map<String, Value>) {
    let arrays: Vec<String> = fields
        .iter()
        .filter(|(key, value)| {
            !key.contains('@')
                && !fields.contains_key(&format!("{key}@odata.type"))
                && value
                    .as_array()
                    .is_some_and(|items| !items.is_empty() && items.iter().all(Value::is_string))
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in arrays {
        fields.insert(format!("{key}@odata.type"), json!("#Collection(String)"));
    }
}

#[cfg(test)]
mod document_tests {
    use super::*;

    #[test]
    fn annotations_of_fields_left_alone_are_kept_and_new_arrays_typed() {
        let extension = json!({
            "id": "microsoft.graph.openTypeExtension.x",
            "extensionName": "x",
            "@odata.type": "#microsoft.graph.openTypeExtension",
            "scores": [],
            "scores@odata.type": "#Collection(Int64)",
            "order": 3,
            "order@odata.type": "#Int64",
            "related": ["G2"],
            "empty": [],
            "orphan@odata.type": "#Int64"
        });
        let sent = extension_document(extension.as_object().expect("object"), |key| key == "order");
        assert_eq!(
            Value::Object(sent),
            json!({
                "scores": [],
                "scores@odata.type": "#Collection(Int64)",
                "order": 3,
                "related": ["G2"],
                "related@odata.type": "#Collection(String)",
                "empty": []
            })
        );
    }
}

/// A list, with `folder`, its folder's name or null, beside Graph's
/// fields (docs/blueprint/07-cli.md#output-contract).
pub(crate) fn list_entity(row: &ListRow) -> Entity {
    let mut entity = row.raw.clone();
    identify(
        &mut entity,
        &row.local_id,
        row.graph_id.as_deref(),
        &row.sync_state,
    );
    entity.insert("folder".into(), json!(row.folder()));
    add_extension(&mut entity, row.extension.as_ref());
    entity
}

pub(crate) fn task_entity(row: &TaskRow) -> Entity {
    let mut entity = row.raw.clone();
    identify(
        &mut entity,
        &row.local_id,
        row.graph_id.as_deref(),
        &row.sync_state,
    );
    entity.insert("list_id".into(), json!(row.list_local_id));
    // On every task, so a client never has to dig in the extension for
    // them (rung 9a).
    let extension = row.extension.as_ref();
    entity.insert(
        "defer_until".into(),
        json!(defer_until(extension).map(|day| day.format(DATE_FORMAT).to_string())),
    );
    entity.insert("someday".into(), json!(someday(extension)));
    add_extension(&mut entity, extension);
    entity
}

/// A task a search found: the task, its list's name as `list`, the
/// passage that matched as `snippet`, and where as `matched`.
pub(crate) fn search_entity(hit: &SearchHit) -> Entity {
    let mut entity = task_entity(&hit.task);
    entity.insert("list".into(), json!(hit.list_name));
    entity.insert("snippet".into(), json!(hit.snippet));
    entity.insert("matched".into(), json!(hit.matched));
    entity
}

/// A task a semantic search found: the task, its list's name as `list`,
/// and `score`, its similarity to the query, to three places.
pub(crate) fn semantic_entity(hit: &crate::semantic::query::Hit) -> Entity {
    let mut entity = task_entity(&hit.candidate.task);
    entity.insert("list".into(), json!(hit.candidate.list_name));
    let score = (f64::from(hit.score) * 1000.0).round() / 1000.0;
    entity.insert("score".into(), json!(score));
    entity
}

/// A completed task for `done`: the task, its list's name as `list`, and
/// `completed_on`, its local day as `YYYY-MM-DD`, or null while the
/// completion hasn't reached Microsoft To Do.
pub(crate) fn completed_entity(
    row: &TaskRow,
    list_name: &str,
    completed_on: Option<&str>,
) -> Entity {
    let mut entity = task_entity(row);
    entity.insert("list".into(), json!(list_name));
    entity.insert("completed_on".into(), json!(completed_on));
    entity
}

fn identify(entity: &mut Entity, local_id: &str, graph_id: Option<&str>, sync_state: &str) {
    entity.insert("id".into(), json!(local_id));
    entity.insert("graph_id".into(), json!(graph_id));
    entity.insert("sync_state".into(), json!(sync_state));
}

fn add_extension(entity: &mut Entity, extension: Option<&Value>) {
    if let Some(extension) = extension {
        entity.insert("extensions".into(), json!([extension]));
    }
}

/// Split Graph's JSON into what the cache stores as `raw_json` and our
/// extension. The outer `Option` is whether Graph's answer said anything
/// about extensions at all: only a filtered `$expand` does (S2).
pub(crate) fn split_extension(mut entity: Entity) -> (Entity, Option<Option<Value>>) {
    // Response metadata, not the task: a single GET adds `@odata.context`,
    // and one with the extension adds `extensions@odata.context` too.
    // Keeping them would make the same task differ between a page and a GET.
    entity.retain(|key, _| key != "@odata.context" && !key.starts_with("extensions@"));
    let extension = entity.remove("extensions").map(|extensions| {
        extensions
            .as_array()
            .and_then(|all| all.iter().find(|extension| is_ours(extension)))
            .cloned()
    });
    (entity, extension)
}

// Graph gives the extension's `id` as
// `microsoft.graph.openTypeExtension.<name>`, and its `extensionName` as
// the plain name.
fn is_ours(extension: &Value) -> bool {
    extension.get("extensionName").and_then(Value::as_str) == Some(EXTENSION_NAME)
        || extension
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.ends_with(EXTENSION_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn a_task_shows_its_local_id_with_graphs_beside_it() {
        let row = TaskRow {
            local_id: "local-1".into(),
            graph_id: Some("AAMk=".into()),
            list_local_id: "list-1".into(),
            title: "Buy milk".into(),
            raw: entity(json!({ "id": "AAMk=", "title": "Buy milk" })),
            extension: Some(json!({ "extensionName": EXTENSION_NAME, "opId": "op-1" })),
            sync_state: "pending".into(),
        };
        let shown = task_entity(&row);
        assert_eq!(shown["id"], "local-1");
        assert_eq!(shown["graph_id"], "AAMk=");
        assert_eq!(shown["list_id"], "list-1");
        assert_eq!(shown["sync_state"], "pending");
        assert_eq!(shown["extensions"][0]["opId"], "op-1");
    }

    #[test]
    fn only_our_extension_is_kept_and_only_when_graph_said_something() {
        let (raw, extension) = split_extension(entity(json!({
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#…",
            "extensions@odata.context": "https://graph.microsoft.com/v1.0/$metadata#…",
            "id": "T1",
            "extensions": [
                { "id": "microsoft.graph.openTypeExtension.other", "extensionName": "other" },
                { "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo", "opId": "op-1" }
            ]
        })));
        assert_eq!(raw, entity(json!({ "id": "T1" })));
        assert_eq!(
            extension,
            Some(Some(json!({
                "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
                "opId": "op-1"
            })))
        );
        assert_eq!(
            split_extension(entity(json!({ "id": "T1", "extensions": [] }))).1,
            Some(None)
        );
        assert_eq!(split_extension(entity(json!({ "id": "T1" }))).1, None);
    }
}
