//! Whether the cache can answer yet (docs/blueprint/07-cli.md#output-contract).
//! A scope is `initial` until its first sync finishes.
//!
//! - A read answers at once. While the first sync runs, that's an empty
//!   `initial` result, never a confident empty list. With nothing running
//!   (the last attempt failed), it runs a sync and answers from that, so a
//!   fixed network or a new sign-in is picked up, and a failure is
//!   reported rather than hidden behind `initial`.
//! - A write, which must resolve names against the cache, waits for the
//!   scope's first sync.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, SyncInfo, SyncState};
use ms_todo_store::{LISTS_SCOPE, ScopeRow};

use crate::handlers::{State, error_payload, store_error};

/// The sync state a read of `scope` answers with.
pub(crate) async fn read_state(state: &State, scope: &str) -> Result<SyncInfo, ErrorPayload> {
    if let Some(ready) = ready(state, scope).await? {
        return Ok(ready);
    }
    if !state.signed_in() {
        return Err(error_payload(
            ErrorKind::AuthRequired,
            "not signed in, and nothing is cached yet; run `ms-todo auth login`".into(),
        ));
    }
    if state.syncer.status().running() {
        return Ok(SyncInfo {
            state: SyncState::Initial,
            generation: 0,
        });
    }
    state.syncer.settle().await;
    match ready(state, scope).await? {
        Some(ready) => Ok(ready),
        None => Err(not_synced(state, scope).await),
    }
}

/// Wait until `scope` has synced once, running a sync if none is running.
pub(crate) async fn ensure_ready(state: &State, scope: &str) -> Result<(), ErrorPayload> {
    if ready(state, scope).await?.is_some() {
        return Ok(());
    }
    state.syncer.settle().await;
    match ready(state, scope).await? {
        Some(_) => Ok(()),
        None => Err(not_synced(state, scope).await),
    }
}

/// The sync state of an answer across every list: `ready` once every list
/// has synced once, with the lists' generation.
pub(crate) async fn all_lists_state(
    state: &State,
    lists_sync: SyncInfo,
) -> Result<SyncInfo, ErrorPayload> {
    Ok(SyncInfo {
        state: if all_ready(state).await? {
            SyncState::Ready
        } else {
            SyncState::Initial
        },
        ..lists_sync
    })
}

/// Whether every scope, the lists and each list's tasks, has synced once.
pub(crate) async fn all_ready(state: &State) -> Result<bool, ErrorPayload> {
    let lists = state.store.lists().await.map_err(store_error)?;
    let scopes = state.store.scopes().await.map_err(store_error)?;
    let ready = |scope: &str| {
        scopes
            .iter()
            .any(|row| row.scope == scope && row.is_ready())
    };
    Ok(ready(LISTS_SCOPE)
        && lists
            .iter()
            .filter_map(|list| list.graph_id.as_deref())
            .all(|graph_id| ready(&ms_todo_store::tasks_scope(graph_id))))
}

async fn ready(state: &State, scope: &str) -> Result<Option<SyncInfo>, ErrorPayload> {
    let row = state.store.scope(scope).await.map_err(store_error)?;
    Ok(row.filter(ScopeRow::is_ready).as_ref().map(sync_info))
}

/// A scope's row as the sync state clients see.
pub(crate) fn sync_info(row: &ScopeRow) -> SyncInfo {
    SyncInfo {
        state: if row.is_ready() {
            SyncState::Ready
        } else {
            SyncState::Initial
        },
        generation: row.generation(),
    }
}

/// Why `scope` still isn't ready after a sync: its own error, else the
/// lists' (a tasks scope never starts if the lists fail), else the pass's.
async fn not_synced(state: &State, scope: &str) -> ErrorPayload {
    for candidate in [scope, LISTS_SCOPE] {
        if let Ok(Some(ScopeRow {
            last_error: Some(message),
            last_error_kind,
            ..
        })) = state.store.scope(candidate).await
        {
            let kind = last_error_kind
                .as_deref()
                .and_then(ErrorKind::parse)
                .unwrap_or(ErrorKind::Internal);
            return error_payload(kind, format!("the first sync failed: {message}"));
        }
    }
    state
        .syncer
        .status()
        .last
        .and_then(|last| last.failure)
        .unwrap_or_else(|| {
            error_payload(
                ErrorKind::Internal,
                "the first sync finished without filling the cache; see `ms-todo doctor`".into(),
            )
        })
}
