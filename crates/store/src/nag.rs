//! Nag reminders' reads (rung 9b): the open tasks whose extension sets
//! `nag`. A scan rather than an index: the nagger asks twice a minute,
//! and few tasks nag.

use sqlx::AssertSqlSafe;

use crate::tasks::{TaskRecord, TaskRow, task_record_columns};
use crate::views::LIVE;
use crate::{Store, StoreError};

/// Open, with `nag` in our extension.
const NAGGING: &str = "tasks.status <> 'completed' \
     AND json_extract(tasks.extension_json, '$.nag') IS NOT NULL";

impl Store {
    /// Open live tasks with `nag` in our extension, oldest first.
    pub async fn nagging_tasks(&self) -> Result<Vec<TaskRow>, StoreError> {
        let records: Vec<TaskRecord> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {} {LIVE} AND {NAGGING} ORDER BY tasks.created_at, tasks.rowid",
            task_record_columns()
        )))
        .fetch_all(self.reader())
        .await?;
        records.into_iter().map(TaskRow::try_from).collect()
    }

    /// How many open live tasks nag, for `doctor`.
    pub async fn nagging_count(&self) -> Result<u64, StoreError> {
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT COUNT(*) {LIVE} AND {NAGGING}"
        )))
        .fetch_one(self.reader())
        .await?;
        Ok(u64::try_from(count).unwrap_or(0))
    }
}
