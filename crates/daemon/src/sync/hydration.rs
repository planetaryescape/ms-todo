//! Filling in what an enumeration doesn't carry: our extension's content
//! (docs/blueprint/04-sync-cache.md#children-of-a-task). Delta and plain
//! collection GETs only show that a task changed; the extension comes back
//! only from a GET with the filtered `$expand` (S2), one per task, grouped
//! into sequential `$batch` calls of 20.
//!
//! Attachment metadata, the other thing 04 fetches here, waits for the
//! attachments table in rung 8b.

use std::collections::{HashMap, HashSet};

use ms_todo_graph::GraphClient;
use ms_todo_protocol::{Entity, ErrorPayload};
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
            Err(error) if error.status() == Some(404) => hydrated.gone.push(id.clone()),
            Err(error) => {
                if hydrated.failure.is_none() {
                    hydrated.failure = Some(graph_error(error));
                }
            }
        }
    }
    hydrated
}
