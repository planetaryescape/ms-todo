//! Filling in what an enumeration doesn't carry: our extension's content
//! and the attachments' metadata
//! (docs/blueprint/04-sync-cache.md#children-of-a-task). Delta and plain
//! collection GETs only show that a task changed; the extension comes back
//! only from a GET with the filtered `$expand` (S2), one per task, and the
//! attachments only from `GET …/attachments` (S1), one per task that has
//! any, each grouped into sequential `$batch` calls of 20. A task without
//! attachments gets an empty list, so the cache knows it has none
//! (D-056).

use std::collections::{HashMap, HashSet};

use ms_todo_graph::{GraphClient, GraphError, attachment_metadata};
use ms_todo_protocol::{Entity, ErrorPayload};
use ms_todo_store::ATTACHMENTS;
use serde_json::Value;

use crate::entities::{EXTENSION_NAME, split_extension};
use crate::handlers::graph_error;

#[derive(Debug, Default)]
pub(super) struct Hydrated {
    /// Each fetched task, by Graph ID: its JSON and its extension, if any.
    pub fetched: HashMap<String, (Entity, Option<Value>)>,
    /// Tasks deleted between the enumeration and the fetch (404). That
    /// counts as a successful fetch (04).
    pub gone: Vec<String>,
    /// Why a fetch failed. With one, the scope isn't checkpointed.
    pub failure: Option<ErrorPayload>,
}

/// Fetch the extension of each task in `needed`, in list `list_graph_id`.
pub(super) async fn hydrate(
    graph: &GraphClient,
    list_graph_id: &str,
    needed: &HashSet<String>,
) -> Hydrated {
    let mut hydrated = Hydrated::default();
    if needed.is_empty() {
        return hydrated;
    }
    let mut ids: Vec<&String> = needed.iter().collect();
    ids.sort();
    let urls: Vec<_> = ids
        .iter()
        .map(|id| graph.task_with_extension_url(list_graph_id, id, EXTENSION_NAME))
        .collect();
    let results = match graph.get_each(&urls).await {
        Ok(results) => results,
        Err(error) => {
            hydrated.failure = Some(graph_error(error));
            return hydrated;
        }
    };
    for (id, result) in ids.into_iter().zip(results) {
        match result {
            Ok(task) => {
                let (raw, extension) = split_extension(task);
                hydrated
                    .fetched
                    .insert(id.clone(), (raw, extension.flatten()));
            }
            Err(error) => hydrated.fail(id, error),
        }
    }
    attachments(graph, list_graph_id, &mut hydrated).await;
    hydrated
}

impl Hydrated {
    /// A fetch for task `id` failed: a 404 means it was deleted since.
    fn fail(&mut self, id: &str, error: GraphError) {
        if error.status() == Some(404) {
            self.gone.push(id.to_owned());
        } else if self.failure.is_none() {
            self.failure = Some(graph_error(error));
        }
    }
}

/// Give each fetched task its attachments' metadata under `attachments`:
/// fetched for those with `hasAttachments`, empty for the rest.
async fn attachments(graph: &GraphClient, list_graph_id: &str, hydrated: &mut Hydrated) {
    let mut ids: Vec<String> = Vec::new();
    for (id, (raw, _)) in &mut hydrated.fetched {
        if raw.get("hasAttachments").and_then(Value::as_bool) == Some(true) {
            ids.push(id.clone());
        } else {
            raw.insert(ATTACHMENTS.into(), Value::Array(Vec::new()));
        }
    }
    if ids.is_empty() {
        return;
    }
    ids.sort();
    let urls: Vec<_> = ids
        .iter()
        .map(|id| graph.attachments_list_url(list_graph_id, id))
        .collect();
    let results = match graph.get_each(&urls).await {
        Ok(results) => results,
        Err(error) => {
            hydrated.failure.get_or_insert(graph_error(error));
            return;
        }
    };
    for (id, result) in ids.iter().zip(results) {
        let listed = match result {
            // A page of them (rare: a task has a handful) is read whole.
            Ok(page) if page.contains_key("@odata.nextLink") => {
                graph.list_attachments(list_graph_id, id).await
            }
            Ok(mut page) => serde_json::from_value::<Vec<Entity>>(
                page.remove("value")
                    .unwrap_or_else(|| Value::Array(Vec::new())),
            )
            .map_err(|error| GraphError::Decode(format!("an attachments page: {error}"))),
            Err(error) => Err(error),
        };
        match listed {
            Ok(listed) => {
                let metadata = listed
                    .into_iter()
                    .map(|item| Value::Object(attachment_metadata(item)))
                    .collect();
                if let Some((raw, _)) = hydrated.fetched.get_mut(id) {
                    raw.insert(ATTACHMENTS.into(), Value::Array(metadata));
                }
            }
            Err(error) => {
                hydrated.fetched.remove(id);
                hydrated.fail(id, error);
            }
        }
    }
}
