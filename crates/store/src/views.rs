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
        }
    }
}

/// How many tasks each view and each list holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskCounts {
    pub important: u64,
    pub planned: u64,
    pub all: u64,
    pub completed: u64,
    pub assigned: u64,
    /// Open tasks by list local ID; a list with none is absent.
    pub open_by_list: BTreeMap<String, u64>,
}

/// A task's My Day date, `YYYY-MM-DD` or null: the expression
/// `tasks_by_my_day` indexes.
pub(crate) const MY_DAY: &str = "json_extract(tasks.extension_json, '$.myDay')";

/// Who a task waits on, as ms-todo's extension has it, or null.
const ASSIGNEE: &str = "json_extract(tasks.extension_json, '$.assignee')";

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

    /// Every view's count and each list's open tasks, in one read.
    pub async fn task_counts(&self) -> Result<TaskCounts, StoreError> {
        let sum = |view: View| format!("COALESCE(SUM({}), 0)", view.condition());
        let (important, planned, all, completed, assigned): (i64, i64, i64, i64, i64) =
            sqlx::query_as(AssertSqlSafe(format!(
                "SELECT {}, {}, {}, {}, {} {LIVE}",
                sum(View::Important),
                sum(View::Planned),
                sum(View::All),
                sum(View::Completed),
                sum(View::Assigned)
            )))
            .fetch_one(self.reader())
            .await?;
        let by_list: Vec<(String, i64)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT tasks.list_local_id, COUNT(*) {LIVE} AND {} GROUP BY tasks.list_local_id",
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
            open_by_list: by_list
                .into_iter()
                .map(|(list, open)| (list, count(open)))
                .collect(),
        })
    }
}
