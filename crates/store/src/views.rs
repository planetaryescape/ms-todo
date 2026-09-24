//! The smart views (docs/blueprint/08-tui.md#layout): queries over every
//! live list's tasks, and the counts the TUI's sidebar shows.

use std::collections::BTreeMap;

use sqlx::AssertSqlSafe;

use crate::tasks::{TaskRecord, TaskRow, task_record_columns};
use crate::{Store, StoreError};

/// A smart view over every list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Open tasks with `importance = high`.
    Important,
    /// Open tasks with a due date.
    Planned,
    /// Every open task.
    All,
    Completed,
}

impl View {
    /// Which tasks are in the view: a condition on `tasks`, the one place
    /// each view is defined (its query, its count and a search in it).
    pub(crate) fn condition(self) -> &'static str {
        match self {
            Self::Important => "tasks.status <> 'completed' AND tasks.importance = 'high'",
            Self::Planned => "tasks.status <> 'completed' AND tasks.due_date IS NOT NULL",
            Self::All => "tasks.status <> 'completed'",
            Self::Completed => "tasks.status = 'completed'",
        }
    }

    /// The order the view shows.
    fn order(self) -> &'static str {
        match self {
            Self::Important => {
                "tasks.due_date IS NULL, tasks.due_date, tasks.created_at, tasks.rowid"
            }
            Self::Planned => "tasks.due_date, tasks.created_at, tasks.rowid",
            Self::All => "tasks.created_at, tasks.rowid",
            Self::Completed => {
                "tasks.completed_at_utc IS NULL, tasks.completed_at_utc DESC, tasks.rowid DESC"
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
    /// Open tasks by list local ID; a list with none is absent.
    pub open_by_list: BTreeMap<String, u64>,
}

/// Live tasks in live lists.
const LIVE: &str = "FROM tasks JOIN lists ON lists.local_id = tasks.list_local_id \
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

    /// Every view's count and each list's open tasks, in one read.
    pub async fn task_counts(&self) -> Result<TaskCounts, StoreError> {
        let sum = |view: View| format!("COALESCE(SUM({}), 0)", view.condition());
        let (important, planned, all, completed): (i64, i64, i64, i64) =
            sqlx::query_as(AssertSqlSafe(format!(
                "SELECT {}, {}, {}, {} {LIVE}",
                sum(View::Important),
                sum(View::Planned),
                sum(View::All),
                sum(View::Completed)
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
            open_by_list: by_list
                .into_iter()
                .map(|(list, open)| (list, count(open)))
                .collect(),
        })
    }
}
