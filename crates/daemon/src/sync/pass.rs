//! One full-enumeration pass (docs/blueprint/04-sync-cache.md#reconciliation-after-a-lost-delta-token):
//!
//! 1. Page every list, with our extension inline, and apply: upsert, and
//!    tombstone lists not seen along with their tasks.
//! 2. For each list, four at a time: page every task (`Prefer` is resent
//!    on every page by the client), fetch the extension of each task that
//!    changed since it was last fetched, then apply in one transaction:
//!    upsert, tombstone what wasn't seen or answered 404, and checkpoint
//!    the scope, but only if every fetch succeeded.
//!
//! Each scope gets the store's `local_rev` from `begin_scope`, before it
//! fetches, so a write the daemon makes meanwhile is never undone or
//! tombstoned by a stale page. That's rung 3a's "pending work"; the outbox
//! adds its operations in rung 4.

use std::sync::Arc;

use futures_util::StreamExt;
use futures_util::stream;
use ms_todo_core::message_with_causes;
use ms_todo_graph::GraphClient;
use ms_todo_protocol::{ErrorPayload, SyncProgress};
use ms_todo_store::{
    Hydration, LISTS_SCOPE, ListRow, ListsPass, SeenTask, Store, TasksPass, tasks_scope,
};

use super::hydration::hydrate;
use super::scheduler::PassOutcome;
use crate::entities::{EXTENSION_NAME, split_extension};
use crate::handlers::{graph_error, store_error};

/// Lists synced at once. The Graph client's own cap (4, S9) bounds the
/// requests; this bounds the work in flight.
const LISTS_AT_ONCE: usize = 4;

pub(crate) struct PassContext {
    pub graph: Arc<GraphClient>,
    pub store: Arc<Store>,
}

pub(super) async fn run_pass(context: &PassContext, report: impl Fn(SyncProgress)) -> PassOutcome {
    report(SyncProgress {
        scopes_done: 0,
        scopes_total: 0,
        doing: "lists".into(),
    });
    let mut outcome = PassOutcome::default();
    match sync_lists(context).await {
        Ok(changed) => {
            outcome.scopes = 1;
            outcome.changed = changed;
        }
        Err(failure) => {
            outcome.failure = Some(failure);
            return outcome;
        }
    }
    let lists = match context.store.lists().await {
        Ok(lists) => lists,
        Err(error) => {
            outcome.failure = Some(store_error(error));
            return outcome;
        }
    };
    let total = u32::try_from(lists.len() + 1).unwrap_or(u32::MAX);
    let mut done = 1;
    let mut results = stream::iter(lists)
        .map(|list| async move {
            let result = sync_list(context, &list).await;
            (list.display_name, result)
        })
        .buffer_unordered(LISTS_AT_ONCE);
    while let Some((name, result)) = results.next().await {
        done += 1;
        match result {
            Ok(changed) => {
                outcome.scopes += 1;
                outcome.changed += changed;
            }
            Err(failure) => {
                if outcome.failure.is_none() {
                    outcome.failure = Some(ErrorPayload {
                        message: format!("syncing {name:?}: {}", failure.message),
                        ..failure
                    });
                }
            }
        }
        report(SyncProgress {
            scopes_done: done,
            scopes_total: total,
            doing: format!("tasks in {name:?}"),
        });
    }
    outcome
}

async fn sync_lists(context: &PassContext) -> Result<u64, ErrorPayload> {
    let store = &context.store;
    let rev = store.begin_scope(LISTS_SCOPE).await.map_err(store_error)?;
    let lists = match context
        .graph
        .list_lists_with_extension(EXTENSION_NAME)
        .await
    {
        Ok(lists) => lists,
        Err(error) => return Err(fail(store, LISTS_SCOPE, graph_error(error)).await),
    };
    let lists = lists
        .into_iter()
        .map(|list| {
            let (raw, extension) = split_extension(list);
            (raw, extension.flatten())
        })
        .collect();
    let applied = store
        .apply_lists(ListsPass { rev, lists })
        .await
        .map_err(store_error)?;
    Ok(u64::try_from(applied.changed).unwrap_or(0))
}

async fn sync_list(context: &PassContext, list: &ListRow) -> Result<u64, ErrorPayload> {
    let store = &context.store;
    // Every cached list came from Graph in rung 3a; one without an ID
    // would be an offline create, which arrives with the outbox.
    let Some(graph_id) = &list.graph_id else {
        return Ok(0);
    };
    let scope = tasks_scope(graph_id);
    let rev = store.begin_scope(&scope).await.map_err(store_error)?;
    let tasks = match context.graph.list_tasks(graph_id).await {
        Ok(tasks) => tasks,
        Err(error) => return Err(fail(store, &scope, graph_error(error)).await),
    };
    let etags: Vec<(String, Option<String>)> = tasks
        .iter()
        .filter_map(|task| {
            let id = task.get("id")?.as_str()?.to_owned();
            let etag = task
                .get("@odata.etag")
                .and_then(|etag| etag.as_str())
                .map(str::to_owned);
            Some((id, etag))
        })
        .collect();
    let needed = store.needing_hydration(&etags).await.map_err(store_error)?;
    let mut hydrated = hydrate(&context.graph, graph_id, &needed).await;
    let seen = tasks
        .into_iter()
        .map(|task| {
            let (raw, _) = split_extension(task);
            let id = raw.get("id").and_then(|id| id.as_str()).unwrap_or_default();
            match hydrated.fetched.remove(id) {
                // Fresher than the page, and with the extension.
                Some((raw, extension)) => SeenTask {
                    raw,
                    hydration: Hydration::Fetched(extension),
                },
                None => SeenTask {
                    raw,
                    hydration: Hydration::Kept,
                },
            }
        })
        .collect();
    let failure = hydrated.failure.take();
    let changed = store
        .apply_tasks(TasksPass {
            scope,
            list_local_id: list.local_id.clone(),
            rev,
            seen,
            gone: hydrated.gone,
            failure: failure
                .as_ref()
                .map(|failure| (failure.kind.clone(), failure.message.clone())),
        })
        .await
        .map_err(store_error)?;
    match failure {
        Some(failure) => Err(failure),
        None => Ok(u64::try_from(changed).unwrap_or(0)),
    }
}

/// Record that `scope` failed, and pass the failure on.
async fn fail(store: &Store, scope: &str, failure: ErrorPayload) -> ErrorPayload {
    if let Err(error) = store
        .fail_scope(scope, &failure.kind, &failure.message)
        .await
    {
        eprintln!(
            "ms-todo daemon: cannot record a sync failure for {scope}: {}",
            message_with_causes(&error)
        );
    }
    failure
}
