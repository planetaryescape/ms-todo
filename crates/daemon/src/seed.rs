//! `Seed`: everything the TUI needs for a screen, in one answer from the
//! cache (spotuify's `ClientSeed`; D-031). The TUI starts from it, and
//! reads it again when events say the cache changed.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Counts, ErrorPayload, ResponseData, Scope, Seed, SyncState};
use ms_todo_store::{LISTS_SCOPE, StatusFilter, TaskSearch, View};

use crate::doctor::outbox_depth;
use crate::entities::{list_entity, task_entity};
use crate::freshness::{all_lists_state, read_state};
use crate::handlers::{State, error_payload, store_error};
use crate::reads::list_rows;

pub(crate) async fn seed(
    state: &State,
    scope: Option<Scope>,
    search: Option<&str>,
) -> Result<ResponseData, ErrorPayload> {
    let (lists_sync, outbox, counts, lists) = tokio::try_join!(
        read_state(state, LISTS_SCOPE),
        outbox_depth(state),
        async { state.store.task_counts().await.map_err(store_error) },
        async { state.store.lists().await.map_err(store_error) },
    )?;
    let activity = state.syncer.status().activity();
    let (scope, rows, sync) = match scope {
        // No list resolves before the lists have synced, and no view is
        // whole: say so rather than answer with a confident nothing.
        scope if lists_sync.state == SyncState::Initial => (
            scope.filter(|scope| !matches!(scope, Scope::List { .. })),
            Vec::new(),
            lists_sync,
        ),
        None | Some(Scope::List { .. }) => {
            let wanted = match &scope {
                Some(Scope::List { id }) => Some(id.as_str()),
                _ => None,
            };
            let (list, rows, sync) = list_rows(state, &lists, wanted, search).await?;
            (Some(Scope::List { id: list.local_id }), rows, sync)
        }
        Some(scope) => {
            let view = view_of(&scope).ok_or_else(|| {
                error_payload(
                    ErrorKind::Unsupported,
                    "this daemon doesn't know that view; restart it with `ms-todo daemon stop`"
                        .into(),
                )
            })?;
            let rows = match search {
                None => state.store.tasks_in_view(view).await,
                Some(query) => state
                    .store
                    .search_tasks(&TaskSearch {
                        query,
                        list_local_id: None,
                        status: StatusFilter::All,
                        view: Some(view),
                        limit: None,
                    })
                    .await
                    .map(|hits| hits.into_iter().map(|hit| hit.task).collect()),
            }
            .map_err(store_error)?;
            (Some(scope), rows, all_lists_state(state, lists_sync).await?)
        }
    };
    Ok(ResponseData::Seed(Seed {
        scope,
        lists: crate::folders::sorted(&lists)
            .into_iter()
            .map(list_entity)
            .collect(),
        lists_sync,
        counts: Counts {
            important: counts.important,
            planned: counts.planned,
            all: counts.all,
            completed: counts.completed,
            lists: counts.open_by_list,
        },
        tasks: rows.iter().map(task_entity).collect(),
        sync,
        activity,
        outbox,
    }))
}

fn view_of(scope: &Scope) -> Option<View> {
    match scope {
        Scope::Important => Some(View::Important),
        Scope::Planned => Some(View::Planned),
        Scope::All => Some(View::All),
        Scope::Completed => Some(View::Completed),
        Scope::List { .. } | Scope::Unknown => None,
    }
}
