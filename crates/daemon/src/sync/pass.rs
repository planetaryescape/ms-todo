//! One sync pass (docs/blueprint/04-sync-cache.md#delta-sync). Each scope
//! replays its saved `deltaLink` and applies only what changed. A scope with
//! no link, or whose link Graph rejects, is read whole instead: a fresh
//! delta paged to the end, which also ends with a link. That whole read is
//! the reset path (#reconciliation-after-a-lost-delta-token), and it
//! tombstones what it didn't see.
//!
//! 1. Lists: a delta round. If it named any list, one enumeration with our
//!    extension inline brings every list's extension (delta never carries
//!    it, S2), and since that's the whole set, it's applied as one: upsert,
//!    and tombstone lists not in it along with their tasks and cursors.
//! 2. For each list, four at a time: a delta round, then the extension of
//!    each task it named whose etag moved, then apply in one transaction:
//!    upsert, tombstone `@removed` and 404s, and checkpoint the scope with
//!    its new link, but only if every fetch succeeded. Otherwise the next
//!    pass replays the old link, which is safe because applying is an
//!    upsert.
//!
//! Each scope gets the store's `local_rev` from `begin_scope`, before it
//! fetches, so a write the daemon makes meanwhile is never undone or
//! tombstoned by a stale page, and a task with an outbox operation
//! `pending`, `inflight` or `unknown` is left alone altogether (04). When
//! a list is found deleted, its queued operations fail with
//! `WriteRejected`.

use std::collections::HashSet;
use std::sync::Arc;

use futures_util::StreamExt;
use futures_util::stream;
use ms_todo_core::{ErrorKind, message_with_causes};
use ms_todo_graph::{Entity, GraphClient};
use ms_todo_protocol::{ErrorPayload, SyncProgress};
use ms_todo_store::{
    Cursor, Hydration, LISTS_SCOPE, ListRow, ListsPass, SeenTask, Store, TasksPass, tasks_scope,
};

use super::hydration::hydrate;
use super::scheduler::PassOutcome;
use crate::entities::{EXTENSION_NAME, split_extension};
use crate::events::Events;
use crate::handlers::{graph_error, store_error};

/// Lists synced at once. The Graph client's own cap (4, S9) bounds the
/// requests; this bounds the work in flight.
const LISTS_AT_ONCE: usize = 4;

pub(crate) struct PassContext {
    pub graph: Arc<GraphClient>,
    pub store: Arc<Store>,
    pub events: Events,
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
    let start = context.graph.lists_delta_start();
    let round = match round(context, LISTS_SCOPE, &start).await {
        Ok(round) => round,
        Err(failure) => return Err(fail(store, LISTS_SCOPE, failure).await),
    };
    if round.cursor.replayed && round.items.is_empty() {
        store
            .advance_scope(LISTS_SCOPE, &round.cursor)
            .await
            .map_err(store_error)?;
        return Ok(0);
    }
    let removed: HashSet<String> = round
        .items
        .iter()
        .filter(|item| is_removed(item))
        .filter_map(graph_id)
        .collect();
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
        // The enumeration came after the delta round, but a list delta
        // called deleted stays deleted: IDs aren't reused.
        .filter(|list| graph_id(list).is_none_or(|id| !removed.contains(&id)))
        .map(|list| {
            let (raw, extension) = split_extension(list);
            (raw, extension.flatten())
        })
        .collect();
    let applied = store
        .apply_lists(ListsPass {
            rev,
            lists,
            cursor: round.cursor,
        })
        .await
        .map_err(store_error)?;
    rejected_with_list(context, &applied.failed_ops).await;
    let count = u64::try_from(applied.changed.len()).unwrap_or(u64::MAX);
    context.events.changed(applied.changed, Vec::new());
    Ok(count)
}

/// Say that `op_ids`, queued for a list found deleted, were rejected.
async fn rejected_with_list(context: &PassContext, op_ids: &[String]) {
    for op_id in op_ids {
        let Ok(Some(op)) = context.store.outbox_op(op_id).await else {
            continue;
        };
        let (kind, message) = op.last_error.clone().unwrap_or_default();
        context
            .events
            .write_rejected(op_id, &op.entity_local_id, &kind, &message);
    }
}

async fn sync_list(context: &PassContext, list: &ListRow) -> Result<u64, ErrorPayload> {
    let store = &context.store;
    // Every cached list came from Graph in rung 3b; one without an ID
    // would be an offline create, which arrives with the outbox.
    let Some(list_graph_id) = &list.graph_id else {
        return Ok(0);
    };
    let scope = tasks_scope(list_graph_id);
    let rev = store.begin_scope(&scope).await.map_err(store_error)?;
    let start = context.graph.tasks_delta_start(list_graph_id);
    let round = match round(context, &scope, &start).await {
        Ok(round) => round,
        Err(failure) if failure.kind == ErrorKind::NotFound.as_str() => {
            return list_not_found(context, list, &scope, rev, failure).await;
        }
        Err(failure) => return Err(fail(store, &scope, failure).await),
    };
    if round.cursor.replayed && round.items.is_empty() {
        store
            .advance_scope(&scope, &round.cursor)
            .await
            .map_err(store_error)?;
        return Ok(0);
    }
    let (removed, tasks): (Vec<Entity>, Vec<Entity>) =
        round.items.into_iter().partition(is_removed);
    let etags: Vec<(String, Option<String>, bool)> = tasks
        .iter()
        .filter_map(|task| {
            let etag = task
                .get("@odata.etag")
                .and_then(|etag| etag.as_str())
                .map(str::to_owned);
            Some((graph_id(task)?, etag, has_attachments(task)))
        })
        .collect();
    let needed = store.needing_hydration(&etags).await.map_err(store_error)?;
    let mut hydrated = hydrate(&context.graph, list_graph_id, &needed).await;
    let seen = tasks
        .into_iter()
        .map(|task| {
            let (raw, _) = split_extension(task);
            match graph_id(&raw).and_then(|id| hydrated.fetched.remove(&id)) {
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
    let mut gone: Vec<String> = removed.iter().filter_map(graph_id).collect();
    gone.append(&mut hydrated.gone);
    let failure = hydrated.failure.take();
    let changed = store
        .apply_tasks(TasksPass {
            scope,
            list_local_id: list.local_id.clone(),
            rev,
            seen,
            gone,
            failure: failure
                .as_ref()
                .map(|failure| (failure.kind.clone(), failure.message.clone())),
            cursor: round.cursor,
        })
        .await
        .map_err(store_error)?;
    let count = u64::try_from(changed.len()).unwrap_or(u64::MAX);
    context.events.tasks_changed(changed);
    match failure {
        Some(failure) => Err(failure),
        None => Ok(count),
    }
}

/// A delta round, and the cursor to checkpoint after it.
struct Round {
    items: Vec<Entity>,
    cursor: Cursor,
}

/// One delta round of `scope`: from its saved link, or from `start` when it
/// has none or Graph rejects it. A 404 or 5xx on the saved link isn't a
/// rejection (S4): the round fails and the link is kept for the next pass,
/// which retries it. The client has already retried a 5xx with backoff.
async fn round(context: &PassContext, scope: &str, start: &str) -> Result<Round, ErrorPayload> {
    let saved = context
        .store
        .scope(scope)
        .await
        .map_err(store_error)?
        .and_then(|row| row.delta_link);
    let mut replayed = false;
    let mut delta = None;
    if let Some(link) = saved {
        match context.graph.delta(&link).await {
            Ok(changes) => {
                replayed = true;
                delta = Some(changes);
            }
            Err(error) if error.is_delta_reset() => {
                eprintln!(
                    "ms-todo daemon: Graph rejected the delta link of {scope} ({}); reading it whole",
                    message_with_causes(&error)
                );
                context
                    .store
                    .reset_scope(scope)
                    .await
                    .map_err(store_error)?;
            }
            Err(error) => return Err(graph_error(error)),
        }
    }
    let delta = match delta {
        Some(delta) => delta,
        None => context.graph.delta(start).await.map_err(graph_error)?,
    };
    Ok(Round {
        items: delta.items,
        cursor: Cursor {
            delta_link: delta.delta_link,
            replayed,
        },
    })
}

/// A list's tasks delta answered 404. That's usually a passing glitch
/// (S4), but if the list itself is gone, tombstone it with its tasks and
/// drop its cursor. Otherwise the scope fails and keeps its link.
async fn list_not_found(
    context: &PassContext,
    list: &ListRow,
    scope: &str,
    rev: i64,
    failure: ErrorPayload,
) -> Result<u64, ErrorPayload> {
    let Some(list_graph_id) = list.graph_id.as_deref() else {
        return Err(fail(&context.store, scope, failure).await);
    };
    match context.graph.get_list(list_graph_id).await {
        Err(error) if error.status() == Some(404) => {
            eprintln!("ms-todo daemon: a list was deleted in Graph; removing it from the cache");
            let removed = context
                .store
                .remove_list(list_graph_id, rev)
                .await
                .map_err(store_error)?;
            if let Some(failed) = &removed {
                rejected_with_list(context, failed).await;
                context
                    .events
                    .changed(vec![list.local_id.clone()], Vec::new());
            }
            Ok(u64::from(removed.is_some()))
        }
        _ => Err(fail(&context.store, scope, failure).await),
    }
}

fn is_removed(item: &Entity) -> bool {
    item.contains_key("@removed")
}

fn has_attachments(item: &Entity) -> bool {
    item.get("hasAttachments").and_then(|flag| flag.as_bool()) == Some(true)
}

fn graph_id(item: &Entity) -> Option<String> {
    item.get("id")?.as_str().map(str::to_owned)
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
