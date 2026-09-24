//! Task lists: reads, and applying a full enumeration of them
//! (docs/blueprint/04-sync-cache.md#reconciliation-after-a-lost-delta-token).

use std::collections::HashSet;

use serde_json::Value;
use sqlx::AssertSqlSafe;
use sqlx::FromRow;

use crate::graph_columns::{ListColumns, text};
use sqlx::SqliteConnection;

use crate::outbox::{NO_UNRESOLVED_OPS, fail_ops_in_list};
use crate::sync_state::{Cursor, LISTS_SCOPE, checkpoint, tasks_scope};
use crate::{Entity, Store, StoreError, new_local_id, now, parse_object, parse_optional, to_json};

/// A live (not tombstoned) list.
#[derive(Clone, Debug, PartialEq)]
pub struct ListRow {
    pub local_id: String,
    pub graph_id: Option<String>,
    pub display_name: String,
    pub wellknown_list_name: Option<String>,
    /// Graph's JSON as last seen, without the extension.
    pub raw: Entity,
    /// Our open extension, when the list has one.
    pub extension: Option<Value>,
}

#[derive(FromRow)]
struct ListRecord {
    local_id: String,
    graph_id: Option<String>,
    display_name: String,
    wellknown_list_name: Option<String>,
    raw_json: String,
    extension_json: Option<String>,
}

impl TryFrom<ListRecord> for ListRow {
    type Error = StoreError;

    fn try_from(record: ListRecord) -> Result<Self, StoreError> {
        Ok(Self {
            local_id: record.local_id,
            graph_id: record.graph_id,
            display_name: record.display_name,
            wellknown_list_name: record.wellknown_list_name,
            raw: parse_object(&record.raw_json)?,
            extension: parse_optional(record.extension_json)?,
        })
    }
}

/// A cached list as an enumeration compares against it.
#[derive(FromRow)]
struct Cached {
    local_id: String,
    raw_json: String,
    extension_json: Option<String>,
    deleted_at: Option<i64>,
    local_rev: i64,
}

/// Every list Graph returned in one enumeration, each with its extension.
/// Enumerating lists with the filtered `$expand` carries the extension, so
/// lists never need a separate fetch.
pub struct ListsPass {
    /// [`Store::local_rev`] read before the enumeration was fetched.
    pub rev: i64,
    pub lists: Vec<(Entity, Option<Value>)>,
    /// Saved at the checkpoint.
    pub cursor: Cursor,
}

/// What applying a [`ListsPass`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ListsApplied {
    /// Local IDs of the lists it added, changed or tombstoned.
    pub changed: Vec<String>,
    /// Graph IDs of the lists it tombstoned, with their tasks.
    pub removed: Vec<String>,
    /// Outbox operations failed because their list is gone.
    pub failed_ops: Vec<String>,
}

impl Store {
    /// Every live list, in the order they were first seen.
    pub async fn lists(&self) -> Result<Vec<ListRow>, StoreError> {
        let records: Vec<ListRecord> = sqlx::query_as(
            "SELECT local_id, graph_id, display_name, wellknown_list_name, raw_json, extension_json \
             FROM lists WHERE deleted_at IS NULL ORDER BY rowid",
        )
        .fetch_all(self.reader())
        .await?;
        records.into_iter().map(ListRow::try_from).collect()
    }

    /// Apply a full enumeration of lists in one transaction: upsert what
    /// came back, tombstone every list not seen (with its tasks, and drop
    /// its tasks scope), and checkpoint the `lists` scope. Rows the daemon
    /// wrote after `pass.rev` are left alone, and so are rows with no Graph
    /// ID yet.
    pub async fn apply_lists(&self, pass: ListsPass) -> Result<ListsApplied, StoreError> {
        let mut tx = self.writer().begin().await?;
        let mut applied = ListsApplied::default();
        let mut seen = HashSet::new();
        for (raw, extension) in &pass.lists {
            let Some(graph_id) = text(raw, "id") else {
                continue;
            };
            seen.insert(graph_id.clone());
            let raw_json = to_json(raw)?;
            let extension_json = extension.as_ref().map(Value::to_string);
            let existing: Option<Cached> = sqlx::query_as(
                "SELECT local_id, raw_json, extension_json, deleted_at, local_rev \
                 FROM lists WHERE graph_id = ?",
            )
            .bind(&graph_id)
            .fetch_optional(&mut *tx)
            .await?;
            let columns = ListColumns::of(raw);
            let local_id = match existing {
                Some(cached) if cached.local_rev > pass.rev => continue,
                Some(cached) => {
                    if cached.raw_json == raw_json
                        && cached.extension_json == extension_json
                        && cached.deleted_at.is_none()
                    {
                        continue;
                    }
                    cached.local_id
                }
                None => {
                    let local_id = new_local_id();
                    sqlx::query(
                        "INSERT INTO lists (local_id, graph_id, display_name, raw_json) \
                         VALUES (?, ?, '', '{}')",
                    )
                    .bind(&local_id)
                    .bind(&graph_id)
                    .execute(&mut *tx)
                    .await?;
                    local_id
                }
            };
            sqlx::query(
                "UPDATE lists SET display_name = ?, wellknown_list_name = ?, is_owner = ?, \
                 is_shared = ?, extension_json = ?, raw_json = ?, etag = ?, deleted_at = NULL \
                 WHERE local_id = ?",
            )
            .bind(&columns.display_name)
            .bind(&columns.wellknown_list_name)
            .bind(columns.is_owner)
            .bind(columns.is_shared)
            .bind(&extension_json)
            .bind(&raw_json)
            .bind(&columns.etag)
            .bind(&local_id)
            .execute(&mut *tx)
            .await?;
            applied.changed.push(local_id);
        }

        let live: Vec<(String, String)> = sqlx::query_as(
            "SELECT local_id, graph_id FROM lists \
             WHERE deleted_at IS NULL AND graph_id IS NOT NULL AND local_rev <= ?",
        )
        .bind(pass.rev)
        .fetch_all(&mut *tx)
        .await?;
        for (local_id, graph_id) in live {
            if seen.contains(&graph_id) {
                continue;
            }
            let mut failed = tombstone_list(&mut tx, &local_id, &graph_id, pass.rev).await?;
            applied.failed_ops.append(&mut failed);
            applied.changed.push(local_id);
            applied.removed.push(graph_id);
        }
        let count = i64::try_from(applied.changed.len()).unwrap_or(i64::MAX);
        checkpoint(&mut tx, LISTS_SCOPE, count, &pass.cursor).await?;
        tx.commit().await?;
        Ok(applied)
    }

    /// Tombstone the list whose Graph ID is `graph_id`, found deleted
    /// outside a lists pass (a 404 on `GET /me/todo/lists/{id}`, S4), with
    /// its tasks, and drop its tasks scope. Returns `None` if it wasn't
    /// live, else the outbox operations failed because it's gone.
    pub async fn remove_list(
        &self,
        graph_id: &str,
        rev: i64,
    ) -> Result<Option<Vec<String>>, StoreError> {
        let mut tx = self.writer().begin().await?;
        let live: Option<String> = sqlx::query_scalar(
            "SELECT local_id FROM lists WHERE graph_id = ? AND deleted_at IS NULL AND local_rev <= ?",
        )
        .bind(graph_id)
        .bind(rev)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(local_id) = live else {
            return Ok(None);
        };
        let failed = tombstone_list(&mut tx, &local_id, graph_id, rev).await?;
        tx.commit().await?;
        Ok(Some(failed))
    }
}

/// Tombstone a list and its tasks not written since `rev`, and drop its
/// tasks scope, cursor and all: a deleted list's tasks delta keeps
/// answering 200 with nothing (S4), so it would never say so itself.
///
/// Its `pending` and `unknown` outbox operations fail, since there's no
/// list left to write to (04: "Pending operations for a list that gets an
/// `@removed` fail the same way"), and their tasks go too; the operations
/// keep the content. An `inflight` one is left to the worker, which sees
/// the 404 or the ghost-write check. Returns the operations failed.
async fn tombstone_list(
    tx: &mut SqliteConnection,
    local_id: &str,
    graph_id: &str,
    rev: i64,
) -> Result<Vec<String>, StoreError> {
    let deleted_at = now();
    sqlx::query("UPDATE lists SET deleted_at = ? WHERE local_id = ?")
        .bind(deleted_at)
        .bind(local_id)
        .execute(&mut *tx)
        .await?;
    let failed = fail_ops_in_list(tx, local_id).await?;
    sqlx::query(AssertSqlSafe(format!(
        "UPDATE tasks SET deleted_at = ? \
         WHERE list_local_id = ? AND deleted_at IS NULL AND local_rev <= ? AND {NO_UNRESOLVED_OPS}"
    )))
    .bind(deleted_at)
    .bind(local_id)
    .bind(rev)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM sync_state WHERE scope = ?")
        .bind(tasks_scope(graph_id))
        .execute(&mut *tx)
        .await?;
    Ok(failed)
}
