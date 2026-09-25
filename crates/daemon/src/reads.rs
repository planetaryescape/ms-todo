//! `lists list`, `tasks list` and `search`, answered from the cache
//! (D-034) with the scope's sync state beside the items.

use ms_todo_protocol::{
    ErrorPayload, ResponseData, SearchStatus, SyncInfo, SyncState, TaskFilter, TaskSort,
};
use ms_todo_store::{LISTS_SCOPE, ListRow, StatusFilter, TaskRow, TaskSearch, View};
use std::collections::HashMap;

use serde_json::json;

use crate::assignment::assigned_to;
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
/// first, whatever their status. With `assignee`, only the tasks assigned
/// to that person (anyone for `*`), and with no `list`, the open ones in
/// every list, as the Assigned view has them. `filter` narrows and orders
/// what's left; one that narrows, with no `list`, looks in every list,
/// soonest due first unless it sorts.
pub(crate) async fn list_tasks(
    state: &State,
    wanted: Option<&str>,
    search: Option<&str>,
    assignee: Option<&str>,
    filter: &TaskFilter,
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
    let every_list = wanted.is_none() && (assignee.is_some() || filter.narrows());
    let mut filter = filter.clone();
    let (rows, sync) = if !every_list {
        let (_, rows, sync) = list_rows(state, &lists, wanted, search).await?;
        (rows, sync)
    } else {
        let rows = match (assignee, search) {
            (Some(_), None) => state.store.tasks_in_view(View::Assigned).await,
            (Some(_), Some(query)) => search_view(state, query, View::Assigned).await,
            (None, None) => {
                filter.sort.get_or_insert(TaskSort::Due);
                state.store.every_task().await
            }
            (None, Some(query)) => search_every_list(state, query).await,
        }
        .map_err(store_error)?;
        (rows, all_lists_state(state, lists_sync).await?)
    };
    let rows = match assignee {
        Some(assignee) => {
            let wanted = assignee.trim().to_lowercase();
            rows.into_iter()
                .filter(|row| assigned_to(row, &wanted))
                .collect()
        }
        None => rows,
    };
    let rows = crate::task_filter::apply(rows, &filter, chrono::Local::now().date_naive())?;
    if !every_list && assignee.is_none() {
        return Ok(ResponseData::Tasks {
            items: rows.iter().map(task_entity).collect(),
            sync,
        });
    }
    let names: HashMap<&str, &str> = lists
        .iter()
        .map(|list| (list.local_id.as_str(), list.display_name.as_str()))
        .collect();
    Ok(ResponseData::Tasks {
        items: rows
            .iter()
            .map(|row| {
                let mut entity = task_entity(row);
                entity.insert("list".into(), json!(names.get(row.list_local_id.as_str())));
                entity
            })
            .collect(),
        sync,
    })
}

/// Every list's tasks matching `query`, best first, whatever their status.
async fn search_every_list(
    state: &State,
    query: &str,
) -> Result<Vec<TaskRow>, ms_todo_store::StoreError> {
    let hits = state
        .store
        .search_tasks(&TaskSearch {
            query,
            list_local_id: None,
            status: StatusFilter::All,
            view: None,
            limit: None,
        })
        .await?;
    Ok(hits.into_iter().map(|hit| hit.task).collect())
}

/// A view's tasks matching `query`, best first, whatever their status.
async fn search_view(
    state: &State,
    query: &str,
    view: View,
) -> Result<Vec<TaskRow>, ms_todo_store::StoreError> {
    let hits = state
        .store
        .search_tasks(&TaskSearch {
            query,
            list_local_id: None,
            status: StatusFilter::All,
            view: Some(view),
            limit: None,
        })
        .await?;
    Ok(hits.into_iter().map(|hit| hit.task).collect())
}

/// The tasks `names` names, as `ChangeTasks` finds them.
pub(crate) async fn get_tasks(
    state: &State,
    names: &[String],
    list: Option<&str>,
) -> Result<ResponseData, ErrorPayload> {
    let targets = crate::task_resolution::resolve_tasks(state, names, list).await?;
    Ok(ResponseData::Tasks {
        items: targets
            .iter()
            .map(|target| task_entity(&target.row))
            .collect(),
        sync: read_state(state, LISTS_SCOPE).await?,
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
    let sync = crate::freshness::list_read_state(state, &list).await?;
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
            let sync = crate::freshness::list_read_state(state, &list).await?;
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
