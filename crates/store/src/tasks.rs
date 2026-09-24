//! Tasks: reads, the daemon's own writes, and applying a full enumeration
//! of one list (docs/blueprint/04-sync-cache.md#reconciliation-after-a-lost-delta-token).

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::{FromRow, SqliteConnection};

use crate::graph_columns::{TaskColumns, etag, text};
use crate::pool::next_local_rev;
use crate::sync_state::{checkpoint, record_failure};
use crate::{Entity, Store, StoreError, new_local_id, now, parse_object, parse_optional, to_json};

/// A live (not tombstoned) task.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub local_id: String,
    pub graph_id: Option<String>,
    pub list_local_id: String,
    pub title: String,
    /// Graph's JSON as last seen, without the extension.
    pub raw: Entity,
    /// Our open extension, when the task has one and it has been fetched.
    pub extension: Option<Value>,
}

#[derive(FromRow)]
struct TaskRecord {
    local_id: String,
    graph_id: Option<String>,
    list_local_id: String,
    title: String,
    raw_json: String,
    extension_json: Option<String>,
}

impl TryFrom<TaskRecord> for TaskRow {
    type Error = StoreError;

    fn try_from(record: TaskRecord) -> Result<Self, StoreError> {
        Ok(Self {
            local_id: record.local_id,
            graph_id: record.graph_id,
            list_local_id: record.list_local_id,
            title: record.title,
            raw: parse_object(&record.raw_json)?,
            extension: parse_optional(record.extension_json)?,
        })
    }
}

/// Whether a pass fetched a task's extension content.
#[derive(Clone, Debug, PartialEq)]
pub enum Hydration {
    /// Not fetched this pass: keep what the cache has.
    Kept,
    /// Fetched at the task's current etag: `None` means it has none.
    Fetched(Option<Value>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeenTask {
    /// Graph's JSON, without `extensions`.
    pub raw: Entity,
    pub hydration: Hydration,
}

/// One list's full enumeration, ready to apply.
pub struct TasksPass {
    pub scope: String,
    pub list_local_id: String,
    /// [`Store::local_rev`] read before the enumeration was fetched.
    pub rev: i64,
    pub seen: Vec<SeenTask>,
    /// Tasks whose extension fetch found them deleted (404) in between.
    pub gone: Vec<String>,
    /// Why a fetch the pass needed failed: `(ErrorKind string, message)`.
    /// With one, what came back is still applied, but the scope isn't
    /// checkpointed, so its generation doesn't move and the next pass
    /// fetches again (04, the checkpoint rule).
    pub failure: Option<(String, String)>,
}

impl Store {
    /// A list's live tasks, oldest first.
    pub async fn tasks_in_list(&self, list_local_id: &str) -> Result<Vec<TaskRow>, StoreError> {
        let records: Vec<TaskRecord> = sqlx::query_as(
            "SELECT local_id, graph_id, list_local_id, title, raw_json, extension_json FROM tasks \
             WHERE list_local_id = ? AND deleted_at IS NULL ORDER BY created_at, rowid",
        )
        .bind(list_local_id)
        .fetch_all(self.reader())
        .await?;
        records.into_iter().map(TaskRow::try_from).collect()
    }

    /// A live task by its local ID or its Graph ID.
    pub async fn task(&self, id: &str) -> Result<Option<TaskRow>, StoreError> {
        let record: Option<TaskRecord> = sqlx::query_as(
            "SELECT local_id, graph_id, list_local_id, title, raw_json, extension_json FROM tasks \
             WHERE (local_id = ?1 OR graph_id = ?1) AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(self.reader())
        .await?;
        record.map(TaskRow::try_from).transpose()
    }

    /// The Graph IDs, among `tasks` (`(graph_id, etag)`), whose extension
    /// hasn't been fetched at that etag: new tasks and changed ones. Every
    /// cached task is considered, so a task moved from another list counts
    /// as seen.
    pub async fn needing_hydration(
        &self,
        tasks: &[(String, Option<String>)],
    ) -> Result<HashSet<String>, StoreError> {
        let hydrated: HashMap<String, Option<String>> =
            sqlx::query_as("SELECT graph_id, hydrated_etag FROM tasks WHERE graph_id IS NOT NULL")
                .fetch_all(self.reader())
                .await?
                .into_iter()
                .collect();
        Ok(tasks
            .iter()
            .filter(|(graph_id, current)| {
                current.is_none() || hydrated.get(graph_id).cloned().flatten() != *current
            })
            .map(|(graph_id, _)| graph_id.clone())
            .collect())
    }

    /// Apply one list's enumeration in a transaction: upsert what came
    /// back, tombstone what's gone and every task in the list that wasn't
    /// seen, then checkpoint the scope or record why not. Rows the daemon
    /// wrote after `pass.rev`, and rows with no Graph ID, are left alone.
    /// Returns how many rows changed.
    pub async fn apply_tasks(&self, pass: TasksPass) -> Result<i64, StoreError> {
        let mut tx = self.writer().begin().await?;
        let mut changed = 0;
        let mut seen = HashSet::new();
        for task in &pass.seen {
            let Some(graph_id) = text(&task.raw, "id") else {
                continue;
            };
            seen.insert(graph_id.clone());
            if upsert_seen(&mut tx, &pass, &graph_id, task).await? {
                changed += 1;
            }
        }
        let deleted_at = now();
        for graph_id in &pass.gone {
            seen.remove(graph_id);
            let result = sqlx::query(
                "UPDATE tasks SET deleted_at = ? \
                 WHERE graph_id = ? AND deleted_at IS NULL AND local_rev <= ?",
            )
            .bind(deleted_at)
            .bind(graph_id)
            .bind(pass.rev)
            .execute(&mut *tx)
            .await?;
            changed += i64::try_from(result.rows_affected()).unwrap_or(0);
        }
        let live: Vec<String> = sqlx::query_scalar(
            "SELECT graph_id FROM tasks WHERE list_local_id = ? AND deleted_at IS NULL \
             AND graph_id IS NOT NULL AND local_rev <= ?",
        )
        .bind(&pass.list_local_id)
        .bind(pass.rev)
        .fetch_all(&mut *tx)
        .await?;
        for graph_id in live.iter().filter(|id| !seen.contains(*id)) {
            sqlx::query("UPDATE tasks SET deleted_at = ? WHERE graph_id = ?")
                .bind(deleted_at)
                .bind(graph_id)
                .execute(&mut *tx)
                .await?;
            changed += 1;
        }
        match &pass.failure {
            None => checkpoint(&mut tx, &pass.scope, changed).await?,
            Some((kind, message)) => record_failure(&mut tx, &pass.scope, kind, message).await?,
        }
        tx.commit().await?;
        Ok(changed)
    }

    /// Record what a write the daemon made itself returned, and mark the
    /// row as written now so a pass already in flight leaves it alone.
    /// `extension` is `Some` when the write knows the extension at this
    /// etag (a create sends it); `None` keeps the cached one.
    pub async fn upsert_task_local(
        &self,
        list_local_id: &str,
        raw: &Entity,
        extension: Option<Option<Value>>,
    ) -> Result<TaskRow, StoreError> {
        let graph_id = text(raw, "id")
            .ok_or_else(|| StoreError::Invalid("Graph returned a task with no id".into()))?;
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let existing: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT local_id, extension_json, hydrated_etag FROM tasks WHERE graph_id = ?",
        )
        .bind(&graph_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (local_id, extension_json, hydrated_etag) = match (existing, extension) {
            (existing, Some(extension)) => (
                existing.map_or_else(new_local_id, |(local_id, _, _)| local_id),
                extension.as_ref().map(Value::to_string),
                etag(raw),
            ),
            (Some((local_id, extension_json, hydrated_etag)), None) => {
                (local_id, extension_json, hydrated_etag)
            }
            (None, None) => (new_local_id(), None, None),
        };
        write_task(
            &mut tx,
            &WriteTask {
                local_id: &local_id,
                graph_id: &graph_id,
                list_local_id,
                raw,
                raw_json: &to_json(raw)?,
                extension_json: extension_json.as_deref(),
                hydrated_etag: hydrated_etag.as_deref(),
                local_rev: rev,
            },
        )
        .await?;
        tx.commit().await?;
        self.task(&local_id)
            .await?
            .ok_or_else(|| StoreError::Corrupt(format!("task {local_id} vanished after a write")))
    }

    /// Tombstone a task the daemon deleted itself.
    pub async fn tombstone_task_local(&self, local_id: &str) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        sqlx::query("UPDATE tasks SET deleted_at = ?, local_rev = ? WHERE local_id = ?")
            .bind(now())
            .bind(rev)
            .bind(local_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// A cached task as an enumeration compares against it.
#[derive(FromRow)]
struct Cached {
    local_id: String,
    list_local_id: String,
    raw_json: String,
    extension_json: Option<String>,
    hydrated_etag: Option<String>,
    deleted_at: Option<i64>,
    local_rev: i64,
}

/// Upsert one enumerated task. Returns whether the row changed.
async fn upsert_seen(
    tx: &mut SqliteConnection,
    pass: &TasksPass,
    graph_id: &str,
    task: &SeenTask,
) -> Result<bool, StoreError> {
    let existing: Option<Cached> = sqlx::query_as(
        "SELECT local_id, list_local_id, raw_json, extension_json, hydrated_etag, deleted_at, \
         local_rev FROM tasks WHERE graph_id = ?",
    )
    .bind(graph_id)
    .fetch_optional(&mut *tx)
    .await?;
    if existing
        .as_ref()
        .is_some_and(|cached| cached.local_rev > pass.rev)
    {
        return Ok(false);
    }
    let raw_json = to_json(&task.raw)?;
    // `(extension_json, hydrated_etag)` when this pass fetched the extension.
    let fetched = match &task.hydration {
        Hydration::Fetched(extension) => {
            Some((extension.as_ref().map(Value::to_string), etag(&task.raw)))
        }
        Hydration::Kept => None,
    };
    let (local_id, extension_json, hydrated_etag, changed) = match existing {
        Some(cached) => {
            let (extension_json, hydrated_etag) = fetched
                .unwrap_or_else(|| (cached.extension_json.clone(), cached.hydrated_etag.clone()));
            let changed = cached.list_local_id != pass.list_local_id
                || cached.raw_json != raw_json
                || cached.extension_json != extension_json
                || cached.deleted_at.is_some();
            if !changed && cached.hydrated_etag == hydrated_etag {
                return Ok(false);
            }
            (cached.local_id, extension_json, hydrated_etag, changed)
        }
        None => {
            let (extension_json, hydrated_etag) = fetched.unwrap_or((None, None));
            (new_local_id(), extension_json, hydrated_etag, true)
        }
    };
    write_task(
        tx,
        &WriteTask {
            local_id: &local_id,
            graph_id,
            list_local_id: &pass.list_local_id,
            raw: &task.raw,
            raw_json: &raw_json,
            extension_json: extension_json.as_deref(),
            hydrated_etag: hydrated_etag.as_deref(),
            local_rev: 0,
        },
    )
    .await?;
    Ok(changed)
}

struct WriteTask<'a> {
    local_id: &'a str,
    graph_id: &'a str,
    list_local_id: &'a str,
    raw: &'a Entity,
    /// `raw` as JSON text.
    raw_json: &'a str,
    extension_json: Option<&'a str>,
    hydrated_etag: Option<&'a str>,
    /// 0 keeps the row's current value (a sync write).
    local_rev: i64,
}

/// Insert or overwrite a task row, clearing any tombstone.
async fn write_task(tx: &mut SqliteConnection, task: &WriteTask<'_>) -> Result<(), StoreError> {
    let columns = TaskColumns::of(task.raw);
    sqlx::query(
        "INSERT INTO tasks (local_id, graph_id, list_local_id, title, body_content, \
         body_content_type, status, importance, is_reminder_on, reminder_at_utc, due_date, \
         start_date, completed_at_utc, recurrence_json, categories_json, has_attachments, \
         created_at, last_modified_at, extension_json, hydrated_etag, raw_json, etag, local_rev, \
         deleted_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
         ?19, ?20, ?21, ?22, ?23, NULL) \
         ON CONFLICT(local_id) DO UPDATE SET graph_id = ?2, list_local_id = ?3, title = ?4, \
         body_content = ?5, body_content_type = ?6, status = ?7, importance = ?8, \
         is_reminder_on = ?9, reminder_at_utc = ?10, due_date = ?11, start_date = ?12, \
         completed_at_utc = ?13, recurrence_json = ?14, categories_json = ?15, \
         has_attachments = ?16, created_at = ?17, last_modified_at = ?18, extension_json = ?19, \
         hydrated_etag = ?20, raw_json = ?21, etag = ?22, \
         local_rev = CASE WHEN ?23 = 0 THEN local_rev ELSE ?23 END, deleted_at = NULL",
    )
    .bind(task.local_id)
    .bind(task.graph_id)
    .bind(task.list_local_id)
    .bind(&columns.title)
    .bind(&columns.body_content)
    .bind(&columns.body_content_type)
    .bind(&columns.status)
    .bind(&columns.importance)
    .bind(columns.is_reminder_on)
    .bind(&columns.reminder_at_utc)
    .bind(&columns.due_date)
    .bind(&columns.start_date)
    .bind(&columns.completed_at_utc)
    .bind(&columns.recurrence_json)
    .bind(&columns.categories_json)
    .bind(columns.has_attachments)
    .bind(&columns.created_at)
    .bind(&columns.last_modified_at)
    .bind(task.extension_json)
    .bind(task.hydrated_etag)
    .bind(task.raw_json)
    .bind(&columns.etag)
    .bind(task.local_rev)
    .execute(tx)
    .await?;
    Ok(())
}
