//! Cached rows as clients see them (docs/blueprint/07-cli.md#output-contract):
//! Graph's JSON with every field, `id` replaced by the local ID and Graph's
//! beside it as `graph_id`, plus `sync_state`, and a task's `list_id`. Our
//! extension, when known, is under `extensions` as Graph returns it.
//!
//! Also the other direction: Graph's JSON split into what the store keeps
//! as `raw_json` and our extension.

use ms_todo_protocol::Entity;
use ms_todo_store::{ListRow, SearchHit, TaskRow};
use serde_json::{Value, json};

/// Our open extension (docs/blueprint/05-custom-features.md).
pub(crate) const EXTENSION_NAME: &str = "com.planetaryescape.mstodo";

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
    add_extension(&mut entity, row.extension.as_ref());
    entity
}

/// A task a search found: the task, its list's name as `list`, and the
/// passage that matched as `snippet`.
pub(crate) fn search_entity(hit: &SearchHit) -> Entity {
    let mut entity = task_entity(&hit.task);
    entity.insert("list".into(), json!(hit.list_name));
    entity.insert("snippet".into(), json!(hit.snippet));
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
