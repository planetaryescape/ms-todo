//! What My Day suggests (docs/blueprint/05-custom-features.md#my-day), as
//! the app's Suggestions pane does, from the cache: open tasks due today,
//! then overdue ones, then those the last rollover took out of an earlier
//! My Day still open. None of them is in today's My Day, and each appears
//! once, for its first reason.
//!
//! The rollover's leftovers are this machine's record: a task another
//! machine's ms-todo rolled over first isn't among them.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use ms_todo_core::DATE_FORMAT;
use ms_todo_protocol::{Entity, ErrorPayload};
use ms_todo_store::{ListRow, TaskRow};
use serde_json::{Value, json};

use super::rollover::LEFT_OVER;
use super::{completed, my_day_of};
use crate::entities::task_entity;
use crate::handlers::{State, store_error};
use crate::task_fields::graph_due_date;

/// Why a task is suggested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    DueToday,
    Overdue,
    LeftOver,
}

impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Self::DueToday => "due_today",
            Self::Overdue => "overdue",
            Self::LeftOver => "left_over",
        }
    }
}

/// My Day's suggestions for `today`; `lists` names each task's list.
pub(crate) async fn suggestions(
    state: &State,
    today: NaiveDate,
    lists: &[ListRow],
) -> Result<Vec<Entity>, ErrorPayload> {
    let due = state
        .store
        .due_by_not_in_my_day(today)
        .await
        .map_err(store_error)?;
    let (from, left_ids) = left_over(state).await?;
    let mut left = Vec::new();
    for id in &left_ids {
        if let Some(row) = state.store.task(id).await.map_err(store_error)? {
            left.push(row);
        }
    }
    let names: HashMap<&str, &str> = lists
        .iter()
        .map(|list| (list.local_id.as_str(), list.display_name.as_str()))
        .collect();
    Ok(pick(due, left, today)
        .into_iter()
        .map(|(row, reason)| {
            let mut entity = task_entity(&row);
            entity.insert("suggestion".into(), json!(reason.as_str()));
            entity.insert("list".into(), json!(names.get(row.list_local_id.as_str())));
            if reason == Reason::LeftOver {
                entity.insert("left_from".into(), json!(from));
            }
            entity
        })
        .collect())
}

/// The suggestions, in order: `due` (open tasks due by today, not in its
/// My Day, soonest first) split into due today and overdue, then `left`
/// that are still open and not in today's My Day.
fn pick(due: Vec<TaskRow>, left: Vec<TaskRow>, today: NaiveDate) -> Vec<(TaskRow, Reason)> {
    let (due_today, overdue): (Vec<TaskRow>, Vec<TaskRow>) = due
        .into_iter()
        .partition(|row| graph_due_date(&row.raw) == Some(today));
    let mut seen: HashSet<String> = HashSet::new();
    let mut picked = Vec::new();
    let reasons = [
        (due_today, Reason::DueToday),
        (overdue, Reason::Overdue),
        (left, Reason::LeftOver),
    ];
    for (rows, reason) in reasons {
        for row in rows {
            if !completed(&row)
                && my_day_of(&row) != Some(today)
                && seen.insert(row.local_id.clone())
            {
                picked.push((row, reason));
            }
        }
    }
    picked
}

/// The day the last rollover's leftovers were in My Day for, and their
/// local IDs.
async fn left_over(state: &State) -> Result<(Option<String>, Vec<String>), ErrorPayload> {
    let Some(saved) = state.store.setting(LEFT_OVER).await.map_err(store_error)? else {
        return Ok((None, Vec::new()));
    };
    let saved: Value = serde_json::from_str(&saved).unwrap_or_default();
    let from = saved["from"]
        .as_str()
        .filter(|day| NaiveDate::parse_from_str(day, DATE_FORMAT).is_ok())
        .map(str::to_owned);
    let ids = saved["tasks"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok((from, ids))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, status: &str, due: Option<&str>, my_day: Option<&str>) -> TaskRow {
        let mut raw = json!({ "status": status });
        if let Some(due) = due {
            raw["dueDateTime"] =
                json!({ "dateTime": format!("{due}T00:00:00.0000000"), "timeZone": "UTC" });
        }
        TaskRow {
            local_id: id.into(),
            graph_id: None,
            list_local_id: "l1".into(),
            title: id.into(),
            raw: raw.as_object().cloned().expect("object"),
            extension: my_day.map(|day| json!({ "myDay": day })),
            sync_state: "synced".into(),
        }
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, DATE_FORMAT).expect("day")
    }

    #[test]
    fn due_today_then_overdue_then_what_was_left_each_once() {
        let today = day("2026-09-25");
        let due = vec![
            row("late", "notStarted", Some("2026-09-20"), None),
            row("today", "notStarted", Some("2026-09-25"), None),
            row("both", "notStarted", Some("2026-09-24"), None),
        ];
        let left = vec![
            row("both", "notStarted", Some("2026-09-24"), None),
            row("left", "notStarted", None, None),
            row("finished", "completed", None, None),
            row("back", "notStarted", None, Some("2026-09-25")),
        ];
        let picked: Vec<(String, &str)> = pick(due, left, today)
            .into_iter()
            .map(|(row, reason)| (row.local_id, reason.as_str()))
            .collect();
        assert_eq!(
            picked,
            [
                ("today".to_owned(), "due_today"),
                ("late".to_owned(), "overdue"),
                ("both".to_owned(), "overdue"),
                ("left".to_owned(), "left_over"),
            ]
        );
    }
}
