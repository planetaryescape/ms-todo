//! My Day's reads (docs/blueprint/05-custom-features.md#my-day): the tasks
//! an earlier day's My Day still holds, for the rollover, and the open
//! tasks it may suggest. Today's My Day itself is [`View::MyDay`].
//!
//! [`View::MyDay`]: crate::View::MyDay

use chrono::NaiveDate;
use ms_todo_core::DATE_FORMAT;
use sqlx::AssertSqlSafe;

use crate::tasks::{TaskRecord, TaskRow, task_record_columns};
use crate::views::{LIVE, MY_DAY, View};
use crate::{Store, StoreError};

impl Store {
    /// Open tasks in `day`'s My Day.
    pub async fn my_day_count(&self, day: NaiveDate) -> Result<u64, StoreError> {
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT COUNT(*) {LIVE} AND {} AND tasks.status <> 'completed'",
            View::MyDay(day).condition()
        )))
        .fetch_one(self.reader())
        .await?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Live tasks in the My Day of a day before `day`, open or not: what
    /// the rollover takes out.
    pub async fn my_day_before(&self, day: NaiveDate) -> Result<Vec<TaskRow>, StoreError> {
        self.rows(&format!(
            "{MY_DAY} < '{}' ORDER BY tasks.created_at, tasks.rowid",
            day.format(DATE_FORMAT)
        ))
        .await
    }

    /// Open tasks due on or before `day` and not in its My Day, soonest
    /// first.
    pub async fn due_by_not_in_my_day(&self, day: NaiveDate) -> Result<Vec<TaskRow>, StoreError> {
        let day = day.format(DATE_FORMAT);
        self.rows(&format!(
            "tasks.status <> 'completed' AND tasks.due_date <= '{day}' \
             AND {MY_DAY} IS NOT '{day}' ORDER BY tasks.due_date, tasks.created_at, tasks.rowid"
        ))
        .await
    }

    /// Live tasks matching `condition` (trusted SQL), in live lists.
    async fn rows(&self, condition: &str) -> Result<Vec<TaskRow>, StoreError> {
        let records: Vec<TaskRecord> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {} {LIVE} AND {condition}",
            task_record_columns()
        )))
        .fetch_all(self.reader())
        .await?;
        records.into_iter().map(TaskRow::try_from).collect()
    }
}
