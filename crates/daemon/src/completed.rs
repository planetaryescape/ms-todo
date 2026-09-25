//! `ms-todo done`: the tasks completed in a window of days, newest first,
//! from the cache (rung 5d). Graph keeps a completion to the day, as
//! midnight UTC (S12), so each is placed on a local day by the rounding
//! due dates use, and no time of day is claimed. A completion Graph hasn't
//! answered yet has no day: it counts as today's news, with
//! `completed_on` null, whenever the window reaches today.

use chrono::{Local, NaiveDate};
use ms_todo_core::{DATE_FORMAT, ErrorKind};
use ms_todo_protocol::{ErrorPayload, ResponseData, SyncState};
use ms_todo_store::{LISTS_SCOPE, View};

use crate::entities::completed_entity;
use crate::freshness::read_state;
use crate::handlers::{State, error_payload, store_error};
use crate::list_scope::ListScope;
use crate::task_fields::{graph_completion_date, parse_day};

/// The days a `done` covers, both included; `until` open-ended when
/// `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    pub since: NaiveDate,
    pub until: Option<NaiveDate>,
}

impl Window {
    pub fn parse(since: &str, until: Option<&str>) -> Result<Self, ErrorPayload> {
        let window = Self {
            since: parse_day(since)?,
            until: until.map(parse_day).transpose()?,
        };
        if window.until.is_some_and(|until| until < window.since) {
            return Err(error_payload(
                ErrorKind::InvalidInput,
                format!(
                    "--until ({}) is before --since ({since})",
                    until.unwrap_or_default()
                ),
            ));
        }
        Ok(window)
    }

    /// Whether a completion on `day` is in the window; `None` (not yet
    /// answered by Graph) is today's.
    pub fn holds(self, day: Option<NaiveDate>, today: NaiveDate) -> bool {
        match day {
            Some(day) => day >= self.since && self.until.is_none_or(|until| day <= until),
            None => self.until.is_none_or(|until| until >= today),
        }
    }
}

pub(crate) async fn completed_tasks(
    state: &State,
    since: &str,
    until: Option<&str>,
    list: Option<&str>,
    folder: Option<&str>,
    limit: Option<u32>,
) -> Result<ResponseData, ErrorPayload> {
    let window = Window::parse(since, until)?;
    let lists_sync = read_state(state, LISTS_SCOPE).await?;
    if lists_sync.state == SyncState::Initial {
        return Ok(ResponseData::Tasks {
            items: Vec::new(),
            sync: lists_sync,
        });
    }
    let lists = state.store.lists().await.map_err(store_error)?;
    let scope = ListScope::of(&lists, list, folder)?;
    let sync = scope.read_state(state, lists_sync).await?;
    let today = Local::now().date_naive();
    // Newest first: the view orders by the UTC instant, and the local
    // day follows it.
    let rows = state
        .store
        .tasks_in_view(View::Completed)
        .await
        .map_err(store_error)?;
    let limit = limit.map_or(usize::MAX, |limit| limit as usize);
    let items = rows
        .iter()
        .filter_map(|row| {
            let list = scope.lists.get(&row.list_local_id)?;
            let day = graph_completion_date(&row.raw);
            window.holds(day, today).then(|| {
                let day = day.map(|day| day.format(DATE_FORMAT).to_string());
                completed_entity(row, &list.name, day.as_deref())
            })
        })
        .take(limit)
        .collect();
    Ok(ResponseData::Tasks { items, sync })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        parse_day(value).expect("date")
    }

    #[test]
    fn the_window_holds_both_ends_and_an_unanswered_completion_is_todays() {
        let today = date("2026-09-24");
        let open = Window::parse("2026-09-21", None).expect("window");
        assert!(open.holds(Some(date("2026-09-21")), today));
        assert!(!open.holds(Some(date("2026-09-20")), today));
        assert!(open.holds(None, today));
        let closed = Window::parse("2026-09-21", Some("2026-09-23")).expect("window");
        assert!(closed.holds(Some(date("2026-09-23")), today));
        assert!(!closed.holds(Some(date("2026-09-24")), today));
        assert!(!closed.holds(None, today), "not yet answered: today's");
    }

    #[test]
    fn until_before_since_is_invalid() {
        let error = Window::parse("2026-09-24", Some("2026-09-21")).expect_err("backwards");
        assert_eq!(error.kind, "invalid_input");
        assert_eq!(
            Window::parse("yesterday", None)
                .expect_err("not a day")
                .kind,
            "invalid_input"
        );
    }
}
