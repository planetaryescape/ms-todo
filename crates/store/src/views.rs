//! The smart views (docs/blueprint/08-tui.md#layout): queries over every
//! live list's tasks, and the counts the TUI's sidebar shows.

use std::borrow::Cow;
use std::collections::BTreeMap;

use chrono::NaiveDate;
use sqlx::AssertSqlSafe;

use crate::tasks::{TaskRecord, TaskRow, task_record_columns};
use crate::{Store, StoreError};

/// A smart view over every list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Tasks in the My Day of this day (`myDay` in our extension), open
    /// or not.
    MyDay(NaiveDate),
    /// Open tasks with `importance = high`.
    Important,
    /// Open tasks with a due date.
    Planned,
    /// Every open task.
    All,
    Completed,
    /// Open tasks with an assignee (`assignee` in our extension), grouped
    /// by person.
    Assigned,
    /// Open tasks deferred (`deferUntil` in our extension) to a day after
    /// this one, and not Someday, soonest back first.
    Upcoming(NaiveDate),
    /// Open tasks parked as Someday (`someday` in our extension).
    Someday,
}

impl View {
    /// Which tasks are in the view: a condition on `tasks`, the one place
    /// each view is defined (its query, its count and a search in it).
    pub(crate) fn condition(self) -> Cow<'static, str> {
        match self {
            // A date formats as digits and dashes only, so it's safe in SQL.
            Self::MyDay(date) => Cow::Owned(format!(
                "{MY_DAY} = '{}'",
                date.format(ms_todo_core::DATE_FORMAT)
            )),
            Self::Important => {
                Cow::Borrowed("tasks.status <> 'completed' AND tasks.importance = 'high'")
            }
            Self::Planned => {
                Cow::Borrowed("tasks.status <> 'completed' AND tasks.due_date IS NOT NULL")
            }
            Self::All => Cow::Borrowed("tasks.status <> 'completed'"),
            Self::Completed => Cow::Borrowed("tasks.status = 'completed'"),
            Self::Assigned => Cow::Owned(format!(
                "tasks.status <> 'completed' AND COALESCE(trim({ASSIGNEE}), '') <> ''"
            )),
            Self::Upcoming(today) => Cow::Owned(format!(
                "tasks.status <> 'completed' AND NOT {SOMEDAY} AND {DEFER_UNTIL} > '{}'",
                today.format(ms_todo_core::DATE_FORMAT)
            )),
            Self::Someday => Cow::Owned(format!("tasks.status <> 'completed' AND {SOMEDAY}")),
        }
    }

    /// The order the view shows.
    fn order(self) -> &'static str {
        match self {
            Self::MyDay(_) => "tasks.status = 'completed', tasks.created_at, tasks.rowid",
            Self::Important => {
                "tasks.due_date IS NULL, tasks.due_date, tasks.created_at, tasks.rowid"
            }
            Self::Planned => "tasks.due_date, tasks.created_at, tasks.rowid",
            Self::All => "tasks.created_at, tasks.rowid",
            // A completion Graph hasn't answered yet has no date: it's
            // the newest, as the TUI's list order has it.
            Self::Completed => {
                "tasks.completed_at_utc IS NOT NULL, tasks.completed_at_utc DESC, tasks.rowid DESC"
            }
            // Each person's tasks together, however their name was typed,
            // soonest due first.
            Self::Assigned => {
                "lower(trim(json_extract(tasks.extension_json, '$.assignee'))), \
                 tasks.due_date IS NULL, tasks.due_date, tasks.created_at, tasks.rowid"
            }
            Self::Upcoming(_) => {
                "json_extract(tasks.extension_json, '$.deferUntil'), \
                 tasks.created_at, tasks.rowid"
            }
            Self::Someday => "tasks.created_at, tasks.rowid",
        }
    }
}

/// Whether a task is out of the everyday views on `today`
/// (`ms_todo_core::deferral::hidden`, as SQL): open, and Someday or
/// deferred to a later day.
fn hidden(today: NaiveDate) -> String {
    format!(
        "(tasks.status <> 'completed' AND ({SOMEDAY} OR {DEFER_UNTIL} > '{}'))",
        today.format(ms_todo_core::DATE_FORMAT)
    )
}

/// How many tasks each view and each list holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskCounts {
    pub important: u64,
    pub planned: u64,
    pub all: u64,
    pub completed: u64,
    pub assigned: u64,
    pub upcoming: u64,
    pub someday: u64,
    /// Open tasks by list local ID; a list with none is absent.
    pub open_by_list: BTreeMap<String, u64>,
}

/// A task's My Day date, `YYYY-MM-DD` or null: the expression
/// `tasks_by_my_day` indexes.
pub(crate) const MY_DAY: &str = "json_extract(tasks.extension_json, '$.myDay')";

/// Who a task waits on, as ms-todo's extension has it, or null.
const ASSIGNEE: &str = "json_extract(tasks.extension_json, '$.assignee')";

/// Whether a task is Someday: JSON's `true` reads as 1.
const SOMEDAY: &str = "COALESCE(json_extract(tasks.extension_json, '$.someday'), 0) = 1";

/// The day a deferred task comes back, `YYYY-MM-DD`, or `''`, which sorts
/// before every day. Anything not shaped like a day is `''` too, as
/// `ms_todo_core::deferral` ignores what it can't read.
const DEFER_UNTIL: &str = "CASE WHEN json_extract(tasks.extension_json, '$.deferUntil') \
     GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]' \
     THEN json_extract(tasks.extension_json, '$.deferUntil') ELSE '' END";

/// Live tasks in live lists.
pub(crate) const LIVE: &str = "FROM tasks JOIN lists ON lists.local_id = tasks.list_local_id \
     WHERE tasks.deleted_at IS NULL AND lists.deleted_at IS NULL";

impl Store {
    /// The tasks of a smart view, in the view's order.
    pub async fn tasks_in_view(&self, view: View) -> Result<Vec<TaskRow>, StoreError> {
        let records: Vec<TaskRecord> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {} {LIVE} AND {} ORDER BY {}",
            task_record_columns(),
            view.condition(),
            view.order()
        )))
        .fetch_all(self.reader())
        .await?;
        records.into_iter().map(TaskRow::try_from).collect()
    }

    /// Every live task in every live list, oldest first: what `tasks
    /// list` filters when it looks in every list.
    pub async fn every_task(&self) -> Result<Vec<TaskRow>, StoreError> {
        let records: Vec<TaskRecord> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {} {LIVE} ORDER BY tasks.created_at, tasks.rowid",
            task_record_columns()
        )))
        .fetch_all(self.reader())
        .await?;
        records.into_iter().map(TaskRow::try_from).collect()
    }

    /// Every view's count and each list's open tasks, in one read. The
    /// everyday views and the lists leave out what's deferred or Someday
    /// on `today`, as they show it.
    pub async fn task_counts(&self, today: NaiveDate) -> Result<TaskCounts, StoreError> {
        let hidden = hidden(today);
        let sum = |view: View| format!("COALESCE(SUM({}), 0)", view.condition());
        let shown = |view: View| format!("COALESCE(SUM({} AND NOT {hidden}), 0)", view.condition());
        let (important, planned, all, completed, assigned, upcoming, someday): (
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ) = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {}, {}, {}, {}, {}, {}, {} {LIVE}",
            shown(View::Important),
            shown(View::Planned),
            shown(View::All),
            sum(View::Completed),
            shown(View::Assigned),
            sum(View::Upcoming(today)),
            sum(View::Someday)
        )))
        .fetch_one(self.reader())
        .await?;
        let by_list: Vec<(String, i64)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT tasks.list_local_id, COUNT(*) {LIVE} AND {} AND NOT {hidden} \
             GROUP BY tasks.list_local_id",
            View::All.condition()
        )))
        .fetch_all(self.reader())
        .await?;
        let count = |value: i64| u64::try_from(value).unwrap_or(0);
        Ok(TaskCounts {
            important: count(important),
            planned: count(planned),
            all: count(all),
            completed: count(completed),
            assigned: count(assigned),
            upcoming: count(upcoming),
            someday: count(someday),
            open_by_list: by_list
                .into_iter()
                .map(|(list, open)| (list, count(open)))
                .collect(),
        })
    }
}
