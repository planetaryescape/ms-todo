//! Which lists a read or a bulk change over tasks covers: one list
//! (`--list`), a folder's lists (`--folder`), or every list
//! (docs/blueprint/07-cli.md, rung 5d).

use std::collections::HashMap;

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, SyncInfo};
use ms_todo_store::{LISTS_SCOPE, ListRow};

use crate::freshness::{
    all_lists_state, all_ready, ensure_list_ready, ensure_ready, list_read_state,
};
use crate::handlers::{State, error_payload, store_error};
use crate::list_resolution::{ListRef, resolve_list};

pub(crate) struct ListScope {
    /// Each list covered that Graph knows, by local ID.
    pub lists: HashMap<String, ListRef>,
    /// The list, when `--list` named one.
    pub one: Option<ListRef>,
}

impl ListScope {
    pub fn of(
        lists: &[ListRow],
        list: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Self, ErrorPayload> {
        let (rows, one) = match (list, folder) {
            (Some(_), Some(_)) => {
                return Err(error_payload(
                    ErrorKind::InvalidInput,
                    "give --list or --folder, not both".into(),
                ));
            }
            (Some(list), None) => {
                let one = resolve_list(lists, Some(list))?;
                return Ok(Self {
                    lists: HashMap::from([(one.local_id.clone(), one.clone())]),
                    one: Some(one),
                });
            }
            (None, Some(folder)) => (crate::folders::lists_in(lists, folder)?, None),
            (None, None) => (lists.iter().collect(), None),
        };
        Ok(Self {
            lists: rows
                .into_iter()
                .filter_map(ListRef::of)
                .map(|list| (list.local_id.clone(), list))
                .collect(),
            one,
        })
    }

    /// The sync state a read of these lists answers with: the one list's,
    /// or `ready` once every list has synced.
    pub async fn read_state(
        &self,
        state: &State,
        lists_sync: SyncInfo,
    ) -> Result<SyncInfo, ErrorPayload> {
        match &self.one {
            Some(list) => list_read_state(state, list).await,
            None => all_lists_state(state, lists_sync).await,
        }
    }
}

/// The lists a write covers, once their tasks have synced once, as a
/// write must wait for before it picks its tasks.
pub(crate) async fn ready_scope(
    state: &State,
    list: Option<&str>,
    folder: Option<&str>,
) -> Result<ListScope, ErrorPayload> {
    ensure_ready(state, LISTS_SCOPE).await?;
    if list.is_none() && !all_ready(state).await? {
        state.syncer.settle().await;
    }
    let lists = state.store.lists().await.map_err(store_error)?;
    let scope = ListScope::of(&lists, list, folder)?;
    if let Some(one) = &scope.one {
        ensure_list_ready(state, one).await?;
    }
    Ok(scope)
}
