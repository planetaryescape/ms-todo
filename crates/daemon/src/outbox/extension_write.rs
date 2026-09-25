//! Sending a folder write (docs/blueprint/05-custom-features.md): Graph
//! replaces an open extension on PATCH (S2), so the worker GETs the list's
//! extension as Graph has it now, puts the operation's fields over it and
//! writes the whole document back. A list without our extension gets it by
//! POST, which acts as an upsert, and a document left with no fields is
//! deleted, since Graph refuses an empty PATCH. All three are safe to
//! resend, so a folder write is never `unknown`.
//!
//! A write from another device between the GET and the write is lost: the
//! accepted race of docs/issues/001-extension-write-race.md.

use ms_todo_store::{OutboxRow, merge_extension};
use serde_json::{Map, Value, json};

use super::send::{Attempt, Failure, classify};
use crate::entities::{EXTENSION_NAME, split_extension};
use crate::handlers::State;

pub(super) async fn send(
    state: &State,
    list_graph_id: &str,
    op: &OutboxRow,
) -> Result<Attempt, Failure> {
    let fetched = state
        .graph
        .get_list_with_extension(list_graph_id, EXTENSION_NAME)
        .await
        .map_err(classify)?;
    let current = split_extension(fetched).1.flatten();
    let mut merged = current
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(fields) = op.body().as_object() {
        merge_extension(&mut merged, fields);
    }
    let data = document(&merged);
    if data.is_empty() {
        // Graph refuses a PATCH of an empty document: a write that leaves
        // no field removes the extension instead.
        state
            .graph
            .delete_list_extension(list_graph_id, EXTENSION_NAME)
            .await
            .map_err(classify)?;
        return Ok(Attempt::ExtensionWritten(Map::new()));
    }
    let written = match current {
        Some(_) => {
            let body = Value::Object(data.clone());
            match state
                .graph
                .replace_list_extension(list_graph_id, EXTENSION_NAME, &body)
                .await
            {
                // Deleted between the GET and the PATCH: make it afresh.
                Err(error) if error.status() == Some(404) => {
                    create(state, list_graph_id, &data).await
                }
                other => other,
            }
        }
        None => create(state, list_graph_id, &data).await,
    };
    written.map_err(classify)?;
    let mut cached = data;
    cached.insert("extensionName".into(), json!(EXTENSION_NAME));
    cached.insert(
        "id".into(),
        json!(format!(
            "microsoft.graph.openTypeExtension.{EXTENSION_NAME}"
        )),
    );
    Ok(Attempt::ExtensionWritten(cached))
}

async fn create(
    state: &State,
    list_graph_id: &str,
    data: &Map<String, Value>,
) -> Result<(), ms_todo_graph::GraphError> {
    let mut body = data.clone();
    body.insert(
        "@odata.type".into(),
        json!("microsoft.graph.openTypeExtension"),
    );
    body.insert("extensionName".into(), json!(EXTENSION_NAME));
    state
        .graph
        .create_list_extension(list_graph_id, &Value::Object(body))
        .await
}

/// Our fields only: not Graph's `id`, `extensionName` or `@odata`
/// annotations (`order@odata.type`), which it adds itself.
fn document(extension: &Map<String, Value>) -> Map<String, Value> {
    extension
        .iter()
        .filter(|(key, _)| {
            *key != "id"
                && *key != "extensionName"
                && !key.starts_with('@')
                && !key.contains("@odata.")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_sent_is_our_fields_without_graphs_annotations() {
        let extension = json!({
            "extensionName": EXTENSION_NAME,
            "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
            "@odata.type": "#microsoft.graph.openTypeExtension",
            "folder": "Areas",
            "order@odata.type": "#Int64",
            "order": 3
        });
        let sent = document(extension.as_object().expect("object"));
        assert_eq!(
            Value::Object(sent),
            json!({ "folder": "Areas", "order": 3 })
        );
    }
}
