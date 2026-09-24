//! `lists list` and `tasks list`, answered from the cache (D-034) with the
//! scope's sync state beside the items.

use ms_todo_protocol::{ErrorPayload, ResponseData, SyncState};
use ms_todo_store::{LISTS_SCOPE, tasks_scope};

use crate::entities::{list_entity, task_entity};
use crate::freshness::read_state;
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

pub(crate) async fn list_tasks(
    state: &State,
    wanted: Option<&str>,
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
    let tasks = state
        .store
        .tasks_in_list(&list.local_id)
        .await
        .map_err(store_error)?;
    Ok(ResponseData::Tasks {
        items: tasks.iter().map(task_entity).collect(),
        sync,
    })
}
