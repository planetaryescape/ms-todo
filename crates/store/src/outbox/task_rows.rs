//! The task row an operation is on: read, replaced, tombstoned, or its
//! extension written, inside the operation's transaction.

use sqlx::AssertSqlSafe;
use sqlx::{FromRow, SqliteConnection};

use crate::tasks::{TaskRecord, WriteTask, task_record_columns, write_task};
use crate::{Entity, StoreError, TaskRow, now, parse_object, to_json};

/// Make `raw` the task's JSON, keeping its identity and extension, and
/// clear any tombstone.
pub(super) async fn replace_row(
    tx: &mut SqliteConnection,
    local_id: &str,
    raw: &Entity,
    rev: i64,
) -> Result<(), StoreError> {
    let current = row_identity(tx, local_id).await?;
    write_task(
        tx,
        &WriteTask {
            local_id,
            graph_id: current.graph_id.as_deref(),
            list_local_id: &current.list_local_id,
            raw,
            raw_json: &to_json(raw)?,
            extension_json: current.extension_json.as_deref(),
            hydrated_etag: current.hydrated_etag.as_deref(),
            local_rev: rev,
        },
    )
    .await
}

/// Who a cached task is, apart from its JSON's content.
#[derive(FromRow)]
pub(crate) struct RowIdentity {
    pub graph_id: Option<String>,
    pub list_local_id: String,
    pub raw_json: String,
    pub extension_json: Option<String>,
    pub hydrated_etag: Option<String>,
}

pub(crate) async fn row_identity(
    tx: &mut SqliteConnection,
    local_id: &str,
) -> Result<RowIdentity, StoreError> {
    sqlx::query_as(
        "SELECT graph_id, list_local_id, raw_json, extension_json, hydrated_etag FROM tasks \
         WHERE local_id = ?",
    )
    .bind(local_id)
    .fetch_optional(tx)
    .await?
    .ok_or_else(|| StoreError::Invalid(format!("task {local_id} isn't cached")))
}

/// A task's cached extension, `{}` for none.
pub(super) async fn task_extension(
    tx: &mut SqliteConnection,
    local_id: &str,
) -> Result<Entity, StoreError> {
    Ok(row_identity(tx, local_id)
        .await?
        .extension_json
        .as_deref()
        .map(parse_object)
        .transpose()?
        .unwrap_or_default())
}

/// Make `extension` the task's cached extension (none when it's empty),
/// written by the daemon at `rev`, so a pass in flight leaves it alone.
pub(super) async fn write_task_extension(
    tx: &mut SqliteConnection,
    local_id: &str,
    extension: &Entity,
    rev: i64,
) -> Result<(), StoreError> {
    let json = (!extension.is_empty())
        .then(|| to_json(extension))
        .transpose()?;
    sqlx::query("UPDATE tasks SET extension_json = ?, local_rev = ? WHERE local_id = ?")
        .bind(json)
        .bind(rev)
        .bind(local_id)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

pub(super) async fn tombstone_row(
    tx: &mut SqliteConnection,
    local_id: &str,
    rev: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "UPDATE tasks SET deleted_at = COALESCE(deleted_at, ?), local_rev = ? WHERE local_id = ?",
    )
    .bind(now())
    .bind(rev)
    .bind(local_id)
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn fetch_task(
    connection: &mut SqliteConnection,
    local_id: &str,
) -> Result<Option<TaskRow>, StoreError> {
    let record: Option<TaskRecord> = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {} FROM tasks WHERE local_id = ?",
        task_record_columns()
    )))
    .bind(local_id)
    .fetch_optional(connection)
    .await?;
    record.map(TaskRow::try_from).transpose()
}
