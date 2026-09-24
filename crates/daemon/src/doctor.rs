//! The daemon's side of `ms-todo doctor`: the database and every scope's
//! sync state, with its last error.

use ms_todo_protocol::{
    DoctorReport, ErrorPayload, ResponseData, ScopeError, ScopeStatus, SyncMode,
};
use ms_todo_store::scope_list;

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
    Ok(ResponseData::Doctor(DoctorReport {
        database_path: state.store.path().display().to_string(),
        database_bytes: state.store.size_bytes(),
        syncing: state.syncer.status().running(),
        scopes,
    }))
}
