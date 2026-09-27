//! The daemon's side of `ms-todo doctor`: the database and every scope's
//! sync state, with its last error.

use ms_todo_protocol::{
    DoctorReport, ErrorPayload, OutboxDepth, OutboxUpkeep, ResponseData, ScopeError, ScopeStatus,
    SyncMode,
};
use ms_todo_store::{OpState, scope_list};

use crate::freshness::sync_info;
use crate::handlers::{State, store_error};

pub(crate) async fn doctor(state: &State) -> Result<ResponseData, ErrorPayload> {
    let scopes = state.store.scopes().await.map_err(store_error)?;
    let lists = state.store.lists().await.map_err(store_error)?;
    let scopes = scopes
        .into_iter()
        .map(|row| {
            let sync = sync_info(&row);
            let list = scope_list(&row.scope).and_then(|graph_id| {
                lists
                    .iter()
                    .find(|list| list.graph_id.as_deref() == Some(graph_id))
            });
            ScopeStatus {
                mode: if row.is_delta() {
                    SyncMode::Delta
                } else {
                    SyncMode::Enumeration
                },
                last_delta_at: row.last_delta_at,
                list_id: list.map(|list| list.local_id.clone()),
                list_name: list.map(|list| list.display_name.clone()),
                state: sync.state,
                generation: sync.generation,
                in_progress: row.in_progress,
                last_success_at: row.last_success_at,
                last_changed_count: u64::try_from(row.last_changed_count).unwrap_or(0),
                last_error: row.last_error.map(|message| ScopeError {
                    kind: row.last_error_kind.unwrap_or_else(|| "internal".into()),
                    message,
                    at: row.last_error_at,
                }),
                scope: row.scope,
            }
        })
        .collect();
    let outbox = outbox_depth(state).await?;
    Ok(ResponseData::Doctor(Box::new(DoctorReport {
        database_path: state.store.path().display().to_string(),
        database_bytes: state.store.size_bytes(),
        syncing: state.syncer.status().running(),
        scopes,
        outbox_upkeep: Some(outbox_upkeep(state, &outbox)),
        outbox,
        suggest: Some(state.suggest.status()),
        my_day: Some(crate::my_day::status(state).await?),
        semantic: Some(state.semantic.status(&state.store).await?),
        nag: Some(Box::new(crate::nag::status(state).await?)),
        contexts: Some(crate::contexts::status(state).await?),
    })))
}

/// How many outbox operations are in each state.
pub(crate) async fn outbox_depth(state: &State) -> Result<OutboxDepth, ErrorPayload> {
    let (counts, flagged) = state
        .store
        .outbox_depth(state.outbox.config.unknown_lookup_secs())
        .await
        .map_err(store_error)?;
    let mut outbox = OutboxDepth {
        flagged: u64::try_from(flagged).unwrap_or(0),
        ..OutboxDepth::default()
    };
    for (op_state, count) in counts {
        let count = u64::try_from(count).unwrap_or(0);
        match op_state {
            OpState::Pending => outbox.pending = count,
            OpState::Inflight => outbox.inflight = count,
            OpState::Unknown => outbox.unknown = count,
            OpState::Failed => outbox.failed = count,
            OpState::Done => outbox.done = count,
        }
    }
    Ok(outbox)
}

/// The outbox's size and `[outbox]` settings, and the last prune.
fn outbox_upkeep(state: &State, depth: &OutboxDepth) -> OutboxUpkeep {
    let config = &state.outbox.config;
    let last = state.outbox.last_prune();
    OutboxUpkeep {
        rows: depth.pending + depth.inflight + depth.unknown + depth.failed + depth.done,
        retention_days: config.retention_days,
        unknown_lookup_hours: config.unknown_lookup_hours,
        last_pruned_at: last.map(|(at, _)| at),
        last_pruned: last.map(|(_, removed)| removed),
        problem: config.problem.clone(),
    }
}
