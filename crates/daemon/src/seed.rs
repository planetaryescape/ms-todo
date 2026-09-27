//! `Seed`: everything the TUI needs for a screen, in one answer from the
//! cache (spotuify's `ClientSeed`; D-031). The TUI starts from it, and
//! reads it again when events say the cache changed.

use chrono::NaiveDate;
use ms_todo_core::{DATE_FORMAT, ErrorKind};
use ms_todo_protocol::{
    Counts, DeferredFilter, ErrorPayload, MyDaySeed, ResponseData, Scope, Seed, SyncState,
};
use ms_todo_store::{LISTS_SCOPE, StatusFilter, TaskSearch, View};

use crate::doctor::outbox_depth;
use crate::entities::{list_entity, task_entity};
use crate::freshness::{all_lists_state, read_state};
use crate::handlers::{State, error_payload, store_error};
use crate::reads::list_rows;

/// With `include_deferred`, the views and lists that leave out deferred
/// and Someday tasks show them; a search always finds them.
pub(crate) async fn seed(
    state: &State,
    scope: Option<Scope>,
    search: Option<&str>,
    include_deferred: bool,
) -> Result<ResponseData, ErrorPayload> {
    let today = state.my_day.today();
    let local = crate::deferral::today();
    let (lists_sync, outbox, counts, my_day_count, lists) = tokio::try_join!(
        read_state(state, LISTS_SCOPE),
        outbox_depth(state),
        async { state.store.task_counts(local).await.map_err(store_error) },
        async { state.store.my_day_count(today).await.map_err(store_error) },
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
        Some(Scope::Next) => {
            let rows = match search {
                None => state.store.tasks_in_view(View::All).await,
                Some(query) => crate::reads::search_every_list(state, query).await,
            }
            .map_err(store_error)?;
            (
                Some(Scope::Next),
                rows,
                all_lists_state(state, lists_sync).await?,
            )
        }
        Some(scope) => {
            let view = view_of(&scope, today, local).ok_or_else(|| {
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
    let hides = scope.as_ref().is_none_or(Scope::hides_deferred) && !include_deferred;
    let filter = if hides {
        DeferredFilter::Hide
    } else {
        DeferredFilter::Include
    };
    let (rows, _) = crate::deferral::keep(rows, crate::deferral::searched(filter, search), local);
    let tasks = if scope == Some(Scope::Next) {
        let days = crate::next::days(state);
        crate::next::pick(rows, days, crate::next::DEFAULT_LIMIT)
            .into_iter()
            .map(|(row, why)| crate::next::entity(&row, why))
            .collect()
    } else {
        rows.iter().map(task_entity).collect()
    };
    let my_day = match &scope {
        Some(Scope::MyDay) => Some(MyDaySeed {
            date: today.format(DATE_FORMAT).to_string(),
            suggestions: crate::my_day::suggestions(state, today, &lists).await?,
        }),
        _ => None,
    };
    Ok(ResponseData::Seed(Seed {
        scope,
        lists: crate::folders::sorted(&lists)
            .into_iter()
            .map(list_entity)
            .collect(),
        lists_sync,
        counts: Counts {
            my_day: my_day_count,
            important: counts.important,
            planned: counts.planned,
            all: counts.all,
            completed: counts.completed,
            assigned: counts.assigned,
            upcoming: counts.upcoming,
            someday: counts.someday,
            lists: counts.open_by_list,
        },
        tasks,
        sync,
        activity,
        outbox,
        my_day,
    }))
}

/// The store's view for `scope`: `today` is My Day's day, `local` the
/// local day deferral reads against.
fn view_of(scope: &Scope, today: NaiveDate, local: NaiveDate) -> Option<View> {
    match scope {
        Scope::MyDay => Some(View::MyDay(today)),
        Scope::Important => Some(View::Important),
        Scope::Planned => Some(View::Planned),
        Scope::All => Some(View::All),
        Scope::Completed => Some(View::Completed),
        Scope::Assigned => Some(View::Assigned),
        Scope::Upcoming => Some(View::Upcoming(local)),
        Scope::Someday => Some(View::Someday),
        Scope::Next | Scope::List { .. } | Scope::Unknown => None,
    }
}
