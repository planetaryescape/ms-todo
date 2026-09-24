//! `lists list`, `tasks list` and `search`, answered from the cache
//! (D-034) with the scope's sync state beside the items.

use ms_todo_protocol::{ErrorPayload, ResponseData, SearchStatus, SyncInfo, SyncState};
use ms_todo_store::{LISTS_SCOPE, StatusFilter, TaskSearch, tasks_scope};

use crate::entities::{list_entity, search_entity, task_entity};
use crate::freshness::{all_ready, read_state};
use crate::handlers::{State, store_error};
use crate::list_resolution::resolve_list;

pub(crate) async fn list_lists(state: &State) -> Result<ResponseData, ErrorPayload> {
    let sync = read_state(state, LISTS_SCOPE).await?;
    let lists = state.store.lists().await.map_err(store_error)?;
    Ok(ResponseData::Lists {
        items: lists.iter().map(list_entity).collect(),
        sync,
    })
}

/// A list's tasks, oldest first; with `search`, those matching it, best
/// first, whatever their status.
pub(crate) async fn list_tasks(
    state: &State,
    wanted: Option<&str>,
    search: Option<&str>,
) -> Result<ResponseData, ErrorPayload> {
    let lists_sync = read_state(state, LISTS_SCOPE).await?;
    if lists_sync.state == SyncState::Initial {
        // No list can be resolved until the lists have synced.
        return Ok(ResponseData::Tasks {
            items: Vec::new(),
            sync: lists_sync,
        });
    }
    let lists = state.store.lists().await.map_err(store_error)?;
    let list = resolve_list(&lists, wanted)?;
    let sync = read_state(state, &tasks_scope(&list.graph_id)).await?;
    let items = match search {
        None => state
            .store
            .tasks_in_list(&list.local_id)
            .await
            .map_err(store_error)?
            .iter()
            .map(task_entity)
            .collect(),
        Some(query) => state
            .store
            .search_tasks(&TaskSearch {
                query,
                list_local_id: Some(&list.local_id),
                status: StatusFilter::All,
                limit: None,
            })
            .await
            .map_err(store_error)?
            .iter()
            .map(|hit| task_entity(&hit.task))
            .collect(),
    };
    Ok(ResponseData::Tasks { items, sync })
}

/// `search`: every list's tasks, or one list's, matching `query`. Across
/// lists the answer is `ready` only once every list has synced; before
/// that it has what's cached so far.
pub(crate) async fn search_tasks(
    state: &State,
    query: &str,
    wanted: Option<&str>,
    status: SearchStatus,
    limit: Option<u32>,
) -> Result<ResponseData, ErrorPayload> {
    let lists_sync = read_state(state, LISTS_SCOPE).await?;
    if lists_sync.state == SyncState::Initial {
        return Ok(ResponseData::SearchResults {
            items: Vec::new(),
            sync: lists_sync,
        });
    }
    let (list, sync) = match wanted {
        Some(wanted) => {
            let lists = state.store.lists().await.map_err(store_error)?;
            let list = resolve_list(&lists, Some(wanted))?;
            let sync = read_state(state, &tasks_scope(&list.graph_id)).await?;
            (Some(list.local_id), sync)
        }
        None => {
            let ready = all_ready(state).await?;
            let sync = SyncInfo {
                state: if ready {
                    SyncState::Ready
                } else {
                    SyncState::Initial
                },
                ..lists_sync
            };
            (None, sync)
        }
    };
    let hits = state
        .store
        .search_tasks(&TaskSearch {
            query,
            list_local_id: list.as_deref(),
            status: match status {
                SearchStatus::Open => StatusFilter::Open,
                SearchStatus::Completed => StatusFilter::Completed,
                SearchStatus::All => StatusFilter::All,
            },
            limit,
        })
        .await
        .map_err(store_error)?;
    Ok(ResponseData::SearchResults {
        items: hits.iter().map(search_entity).collect(),
        sync,
    })
}
