//! `next` (docs/blueprint/05-custom-features.md#next, rung 9a): the few
//! open tasks to do now, by one fixed urgency order with no weights to
//! tune. Each task falls in the first tier that fits it:
//!
//! 1. overdue, most overdue first;
//! 2. due today;
//! 3. in today's My Day;
//! 4. high importance;
//! 5. due within the next 3 days, soonest first;
//! 6. everything else.
//!
//! Ties go to the oldest task. Deferred and Someday tasks are never next.
//! Each comes with `why`: every reason that applies, not only its tier's.

use chrono::{DateTime, NaiveDate};
use ms_todo_protocol::{ContextChoice, Entity, ErrorPayload, ResponseData, SyncState};
use ms_todo_store::{LISTS_SCOPE, TaskRow, View};
use serde_json::{Value, json};

use crate::entities::task_entity;
use crate::freshness::read_state;
use crate::handlers::{State, store_error};
use crate::list_scope::ListScope;
use crate::my_day::{completed, my_day_of};
use crate::task_fields::graph_due_date;

/// How many `next` gives when not told.
pub(crate) const DEFAULT_LIMIT: u32 = 5;
/// "Due soon": within this many days after today.
const SOON_DAYS: i64 = 3;

/// The days a task is read against: the local day for due dates and
/// deferral, and My Day's day for My Day (they differ only before
/// `my_day.rollover_time`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Days {
    pub today: NaiveDate,
    pub my_day: NaiveDate,
}

/// `next [--list L] [--limit N]`: each task with `why` and `list`. With
/// no list, a context (`choice`, else the active one) narrows it.
pub(crate) async fn next_tasks(
    state: &State,
    choice: Option<&ContextChoice>,
    wanted: Option<&str>,
    limit: Option<u32>,
) -> Result<ResponseData, ErrorPayload> {
    let lists_sync = read_state(state, LISTS_SCOPE).await?;
    if lists_sync.state == SyncState::Initial {
        return Ok(ResponseData::Tasks {
            items: Vec::new(),
            sync: lists_sync,
            deferred_hidden: None,
            context: None,
        });
    }
    let lists = state.store.lists().await.map_err(store_error)?;
    let scope = ListScope::of(&lists, wanted, None)?;
    let context = crate::contexts::applied(state, choice, wanted, &lists)?;
    let sync = scope.read_state(state, lists_sync).await?;
    // Open tasks only: completed history grows without end, and is never next.
    let rows: Vec<TaskRow> = state
        .store
        .tasks_in_view(View::All)
        .await
        .map_err(store_error)?
        .into_iter()
        .filter(|row| {
            scope.lists.contains_key(&row.list_local_id)
                && crate::contexts::within(context.as_ref(), &row.list_local_id)
        })
        .collect();
    let days = days(state);
    let items = pick(rows, days, limit.unwrap_or(DEFAULT_LIMIT))
        .into_iter()
        .map(|(row, why)| {
            let mut entity = entity(&row, why);
            let list = scope.lists.get(&row.list_local_id).map(|list| &list.name);
            entity.insert("list".into(), json!(list));
            entity
        })
        .collect();
    Ok(ResponseData::Tasks {
        items,
        sync,
        deferred_hidden: None,
        context: context.as_ref().map(crate::contexts::Resolved::applied),
    })
}

/// Today as `next` reads it.
pub(crate) fn days(state: &State) -> Days {
    Days {
        today: crate::deferral::today(),
        my_day: state.my_day.today(),
    }
}

/// A task as `next` gives it: with `why`.
pub(crate) fn entity(row: &TaskRow, why: String) -> Entity {
    let mut entity = task_entity(row);
    entity.insert("why".into(), json!(why));
    entity
}

/// The top `limit` of `rows` that could be next, in order, each with why.
pub(crate) fn pick(rows: Vec<TaskRow>, days: Days, limit: u32) -> Vec<(TaskRow, String)> {
    let mut ranked: Vec<(Rank, TaskRow)> = rows
        .into_iter()
        .filter(|row| !completed(row) && !crate::deferral::hidden(row, days.today))
        .map(|row| (Rank::of(&row, days), row))
        .collect();
    ranked.sort_by_key(|(rank, _)| rank.key());
    ranked.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    ranked
        .into_iter()
        .map(|(rank, row)| (row, rank.why.join(", ")))
        .collect()
}

/// Where a task stands, read once.
struct Rank {
    tier: u8,
    /// Within the overdue and due-soon tiers: the due date, soonest (the
    /// most overdue) first.
    due: Option<NaiveDate>,
    /// Whether Graph's creation time is unknown, then that time: the
    /// oldest first, an unknown one last.
    created: (bool, i64),
    why: Vec<String>,
}

impl Rank {
    fn of(row: &TaskRow, days: Days) -> Self {
        let due = graph_due_date(&row.raw);
        let days_to_due = due.map(|due| (due - days.today).num_days());
        let in_my_day = my_day_of(row) == Some(days.my_day);
        let high = row.raw.get("importance").and_then(Value::as_str) == Some("high");
        let soon = days_to_due.is_some_and(|days| (1..=SOON_DAYS).contains(&days));
        let created = row
            .raw
            .get("createdDateTime")
            .and_then(Value::as_str)
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok());
        let mut why = Vec::new();
        match days_to_due {
            Some(days) if days < 0 => why.push(format!("overdue {}d", -days)),
            Some(0) => why.push("due today".to_owned()),
            _ => {}
        }
        if in_my_day {
            why.push("in My Day".to_owned());
        }
        if high {
            why.push("high".to_owned());
        }
        match days_to_due {
            Some(1) if soon => why.push("due tomorrow".to_owned()),
            Some(days) if soon => why.push(format!("due in {days}d")),
            _ => {}
        }
        if why.is_empty() {
            let age = created.map_or(0, |at| {
                (days.today - at.with_timezone(&chrono::Local).date_naive()).num_days()
            });
            why.push(match age {
                ..=0 => "added today".to_owned(),
                age => format!("added {age}d ago"),
            });
        }
        let tier = match days_to_due {
            Some(days) if days < 0 => 0,
            Some(0) => 1,
            _ if in_my_day => 2,
            _ if high => 3,
            _ if soon => 4,
            _ => 5,
        };
        Self {
            tier,
            due: due.filter(|_| matches!(tier, 0 | 4)),
            created: (created.is_none(), created.map_or(0, |at| at.timestamp())),
            why,
        }
    }

    fn key(&self) -> (u8, Option<NaiveDate>, (bool, i64)) {
        (self.tier, self.due, self.created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("day")
    }

    fn row(title: &str, created: &str, raw: Value, extension: Option<Value>) -> TaskRow {
        let mut raw = raw.as_object().cloned().expect("object");
        raw.insert("title".into(), json!(title));
        raw.insert("status".into(), json!("notStarted"));
        raw.insert(
            "createdDateTime".into(),
            json!(format!("{created}T12:00:00.0000000Z")),
        );
        TaskRow {
            local_id: title.into(),
            graph_id: None,
            list_local_id: "l1".into(),
            title: title.into(),
            raw,
            extension,
            sync_state: "synced".into(),
        }
    }

    fn due(day: &str) -> Value {
        json!({ "dueDateTime": { "dateTime": format!("{day}T00:00:00.0000000"), "timeZone": "UTC" } })
    }

    fn days() -> Days {
        Days {
            today: day("2026-10-01"),
            my_day: day("2026-10-01"),
        }
    }

    #[test]
    fn tiers_come_in_the_documented_order_with_the_oldest_first_within() {
        let rows = vec![
            row("old plain", "2026-09-01", json!({}), None),
            row("new plain", "2026-09-20", json!({}), None),
            row("soon", "2026-09-10", due("2026-10-03"), None),
            row("high", "2026-09-10", json!({ "importance": "high" }), None),
            row(
                "my day",
                "2026-09-10",
                json!({}),
                Some(json!({ "myDay": "2026-10-01" })),
            ),
            row("today", "2026-09-10", due("2026-10-01"), None),
            row("a bit late", "2026-09-10", due("2026-09-30"), None),
            row("very late", "2026-09-15", due("2026-09-20"), None),
            row(
                "deferred",
                "2026-08-01",
                due("2026-09-01"),
                Some(json!({ "deferUntil": "2026-10-02" })),
            ),
            row(
                "parked",
                "2026-08-01",
                json!({}),
                Some(json!({ "someday": true })),
            ),
        ];
        let picked = pick(rows, days(), 20);
        let shown: Vec<(&str, &str)> = picked
            .iter()
            .map(|(row, why)| (row.title.as_str(), why.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("very late", "overdue 11d"),
                ("a bit late", "overdue 1d"),
                ("today", "due today"),
                ("my day", "in My Day"),
                ("high", "high"),
                ("soon", "due in 2d"),
                ("old plain", "added 30d ago"),
                ("new plain", "added 11d ago"),
            ]
        );
        assert_eq!(
            pick(picked.into_iter().map(|(row, _)| row).collect(), days(), 2).len(),
            2
        );
    }

    #[test]
    fn why_names_every_reason_that_applies() {
        let mut both = due("2026-09-29");
        both["importance"] = json!("high");
        let rows = vec![row(
            "late and high",
            "2026-09-10",
            both,
            Some(json!({ "myDay": "2026-10-01" })),
        )];
        assert_eq!(pick(rows, days(), 5)[0].1, "overdue 2d, in My Day, high");
    }
}
