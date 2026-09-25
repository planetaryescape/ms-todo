//! `lists list`, `tasks list` and `search`, answered from the cache
//! (D-034) with the scope's sync state beside the items.

use ms_todo_protocol::{ErrorPayload, ResponseData, SearchStatus, SyncInfo, SyncState};
use ms_todo_store::{LISTS_SCOPE, ListRow, StatusFilter, TaskRow, TaskSearch, tasks_scope};

use crate::entities::{list_entity, search_entity, task_entity};
use crate::freshness::{all_lists_state, read_state};
use crate::handlers::{State, store_error};
use crate::list_resolution::{ListRef, resolve_list};

pub(crate) async fn list_lists(state: &State) -> Result<ResponseData, ErrorPayload> {
    let sync = read_state(state, LISTS_SCOPE).await?;
    let lists = state.store.lists().await.map_err(store_error)?;
    Ok(ResponseData::Lists {
        items: crate::folders::sorted(&lists)
            .into_iter()
            .map(list_entity)
            .collect(),
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
    let (_, rows, sync) = list_rows(state, &lists, wanted, search).await?;
    Ok(ResponseData::Tasks {
        items: rows.iter().map(task_entity).collect(),
        sync,
    })
}

/// The list `wanted` names among `lists` (the default list for `None`),
/// its tasks, oldest first, or with `search` those matching it, best
/// first, and its sync state. `tasks list` and the TUI's `Seed` share it.
pub(crate) async fn list_rows(
    state: &State,
    lists: &[ListRow],
    wanted: Option<&str>,
    search: Option<&str>,
) -> Result<(ListRef, Vec<TaskRow>, SyncInfo), ErrorPayload> {
    let list = resolve_list(lists, wanted)?;
    let sync = read_state(state, &tasks_scope(&list.graph_id)).await?;
    let rows = match search {
        None => state.store.tasks_in_list(&list.local_id).await,
        Some(query) => state
            .store
            .search_tasks(&TaskSearch {
                query,
                list_local_id: Some(&list.local_id),
                status: StatusFilter::All,
                view: None,
                limit: None,
            })
            .await
            .map(|hits| hits.into_iter().map(|hit| hit.task).collect()),
    }
    .map_err(store_error)?;
    Ok((list, rows, sync))
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
        None => (None, all_lists_state(state, lists_sync).await?),
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
            view: None,
            limit,
        })
        .await
        .map_err(store_error)?;
    Ok(ResponseData::SearchResults {
        items: hits.iter().map(search_entity).collect(),
        sync,
    })
}
