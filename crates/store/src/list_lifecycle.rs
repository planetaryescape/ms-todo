//! Creating, renaming and deleting lists (rung 8e): each is applied to
//! `lists` and queued in the outbox in one transaction, as a folder write
//! is, so it's offline-safe and undone by `undo`.
//!
//! - A create inserts a list with no Graph ID yet; a task or folder write
//!   queued on it waits for the create (`insert_op`).
//! - A rename changes the name at once; its rollback is the list before.
//! - A delete tombstones the list and its tasks; its rollback is the list
//!   before and the tasks it tombstoned, so a rejection brings them back.
//! - A re-create (undoing the delete of an empty list) brings the row back
//!   with its local ID, name and extension, and no Graph ID until it's sent.
//!
//! A list operation's rollback is `{ "raw": <the list's JSON>, "tasks":
//! [<local IDs>] }`.

use serde_json::{Map, Value, json};
use sqlx::{AssertSqlSafe, SqliteConnection};

use crate::graph_columns::{ListColumns, text};
use crate::list_extension::{ListExtensionOp, merge_extension, write_extension};
use crate::lists::list_in;
use crate::outbox::{
    NO_UNRESOLVED_OPS, OpKind, OpState, Queued, Restore, finish, insert_op, later_ops, op_in,
};
use crate::pool::next_local_rev;
use crate::sync_state::tasks_scope;
use crate::{Entity, ListRow, Store, StoreError, now, parse_object, to_json};

/// A change to a list, to queue: a lifecycle write or a folder write.
#[derive(Clone, Debug)]
pub enum ListOp {
    Extension(ListExtensionOp),
    Lifecycle(ListLifecycleOp),
}

/// A create, re-create, rename or delete of one list.
#[derive(Clone, Debug)]
pub struct ListLifecycleOp {
    pub op_id: String,
    pub list_local_id: String,
    /// What the user asked for, such as `create_list`.
    pub action: String,
    pub write: ListWrite,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListWrite {
    /// A new list called `name`, only ms-todo knows yet.
    Create {
        name: String,
    },
    /// A deleted list made again, with its name and extension.
    Recreate,
    Rename {
        name: String,
    },
    Delete,
}

impl ListWrite {
    fn kind(&self) -> OpKind {
        match self {
            Self::Create { .. } | Self::Recreate => OpKind::ListCreate,
            Self::Rename { .. } => OpKind::ListUpdate,
            Self::Delete => OpKind::ListDelete,
        }
    }
}

impl Store {
    /// Queue `ops`, one command's, and make each one's change to the
    /// cached lists at once, all in one transaction. Returns each list as
    /// it is now, a deleted one as it was.
    pub async fn enqueue_lists(
        &self,
        command_id: &str,
        undoes: Option<&str>,
        ops: Vec<ListOp>,
    ) -> Result<Vec<ListRow>, StoreError> {
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let now = now();
        let mut seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM outbox")
            .fetch_one(&mut *tx)
            .await?;
        let mut rows = Vec::with_capacity(ops.len());
        for op in ops {
            seq += 1;
            let (op_id, list_local_id, action, kind, payload, rollback) = match op {
                ListOp::Extension(op) => {
                    let before = extension_now(&mut tx, &op.list_local_id).await?;
                    let mut merged = before.clone();
                    merge_extension(&mut merged, &op.fields);
                    write_extension(&mut tx, &op.list_local_id, &merged, rev).await?;
                    let payload = json!({ "body": Value::Object(op.fields) });
                    (
                        op.op_id,
                        op.list_local_id,
                        op.action,
                        OpKind::Extension,
                        payload,
                        Some(before),
                    )
                }
                ListOp::Lifecycle(op) => {
                    let kind = op.write.kind();
                    let (payload, rollback) =
                        apply_write(&mut tx, &op.list_local_id, &op.write, rev).await?;
                    (
                        op.op_id,
                        op.list_local_id,
                        op.action,
                        kind,
                        payload,
                        rollback,
                    )
                }
            };
            insert_op(
                &mut tx,
                &Queued {
                    op_id: &op_id,
                    seq,
                    command_id,
                    undoes,
                    created_at: now,
                    entity_local_id: &list_local_id,
                    list_local_id: &list_local_id,
                    op: kind,
                    action: &action,
                    payload: &payload,
                    rollback: rollback.as_ref(),
                },
            )
            .await?;
            rows.push(list_any(&mut tx, &list_local_id).await?);
        }
        tx.commit().await?;
        Ok(rows)
    }

    /// Graph answered list operation `op_id` (a create or a rename) with
    /// `raw`: the list takes its Graph ID and JSON, with the names of its
    /// later unresolved renames on top, and the operation is `done`, in
    /// one transaction. A row a sync inserted meanwhile for the same Graph
    /// ID merges into this one.
    pub async fn record_list_written(&self, op_id: &str, raw: &Entity) -> Result<(), StoreError> {
        let graph_id = text(raw, "id")
            .ok_or_else(|| StoreError::Invalid("Graph returned a list with no id".into()))?;
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let op = op_in(&mut tx, op_id).await?;
        let local_id = op.entity_local_id.clone();
        merge_duplicate_list(&mut tx, &graph_id, &local_id).await?;
        let mut raw = raw.clone();
        for (kind, payload) in later_ops(&mut tx, &op).await? {
            if kind == OpKind::ListUpdate
                && let Some(name) = payload["body"]["displayName"].as_str()
            {
                raw.insert("displayName".into(), json!(name));
            }
        }
        let columns = ListColumns::of(&raw);
        sqlx::query(
            "UPDATE lists SET graph_id = ?, display_name = ?, wellknown_list_name = ?, \
             is_owner = ?, is_shared = ?, raw_json = ?, etag = ?, local_rev = ? WHERE local_id = ?",
        )
        .bind(&graph_id)
        .bind(&columns.display_name)
        .bind(&columns.wellknown_list_name)
        .bind(columns.is_owner)
        .bind(columns.is_shared)
        .bind(to_json(&raw)?)
        .bind(&columns.etag)
        .bind(rev)
        .bind(&local_id)
        .execute(&mut *tx)
        .await?;
        finish(&mut tx, op_id, OpState::Done, None).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Graph deleted the list of operation `op_id`: its tasks scope goes,
    /// cursor and all, and the operation is `done`.
    pub async fn record_list_deleted(&self, op_id: &str) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        let op = op_in(&mut tx, op_id).await?;
        let graph_id: Option<String> =
            sqlx::query_scalar("SELECT graph_id FROM lists WHERE local_id = ?")
                .bind(&op.entity_local_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
        if let Some(graph_id) = graph_id {
            sqlx::query("DELETE FROM sync_state WHERE scope = ?")
                .bind(tasks_scope(&graph_id))
                .execute(&mut *tx)
                .await?;
        }
        finish(&mut tx, op_id, OpState::Done, None).await?;
        tx.commit().await?;
        Ok(())
    }

    /// A list by local ID, tombstoned or not, and whether it's tombstoned.
    pub async fn list_any(&self, local_id: &str) -> Result<Option<(ListRow, bool)>, StoreError> {
        let mut connection = self.reader().acquire().await?;
        let Some(list) = crate::lists::list_including_deleted(&mut connection, local_id).await?
        else {
            return Ok(None);
        };
        let deleted: bool =
            sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM lists WHERE local_id = ?")
                .bind(local_id)
                .fetch_one(&mut *connection)
                .await?;
        Ok(Some((list, deleted)))
    }

    /// How many live tasks the list `local_id` holds.
    pub async fn list_task_count(&self, local_id: &str) -> Result<i64, StoreError> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM tasks WHERE list_local_id = ? AND deleted_at IS NULL",
        )
        .bind(local_id)
        .fetch_one(self.reader())
        .await?)
    }

    /// Whether any task in the list `local_id` has an outbox operation not
    /// resolved yet.
    pub async fn list_has_unresolved_task_ops(&self, local_id: &str) -> Result<bool, StoreError> {
        Ok(sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT EXISTS (SELECT 1 FROM tasks WHERE list_local_id = ? AND NOT {NO_UNRESOLVED_OPS})"
        )))
        .bind(local_id)
        .fetch_one(self.reader())
        .await?)
    }
}

/// Make `write`'s change to the list `local_id` now. Returns the payload
/// to queue and the rollback.
async fn apply_write(
    tx: &mut SqliteConnection,
    local_id: &str,
    write: &ListWrite,
    rev: i64,
) -> Result<(Value, Option<Entity>), StoreError> {
    match write {
        ListWrite::Create { name } => {
            let raw = json!({
                "displayName": name,
                "wellknownListName": "none",
                "isOwner": true,
                "isShared": false,
            });
            sqlx::query(
                "INSERT INTO lists (local_id, graph_id, display_name, wellknown_list_name, \
                 is_owner, is_shared, raw_json, local_rev) VALUES (?, NULL, ?, 'none', 1, 0, ?, ?)",
            )
            .bind(local_id)
            .bind(name)
            .bind(raw.to_string())
            .bind(rev)
            .execute(&mut *tx)
            .await?;
            Ok((json!({ "body": { "displayName": name } }), None))
        }
        ListWrite::Recreate => {
            let before = snapshot(tx, local_id, Vec::new()).await?;
            let mut raw = before["raw"].as_object().cloned().unwrap_or_default();
            // Graph's, for the list that's gone.
            raw.retain(|key, _| key != "id" && !key.starts_with('@'));
            let name = text(&raw, "displayName").unwrap_or_default();
            sqlx::query(
                "UPDATE lists SET graph_id = NULL, etag = NULL, deleted_at = NULL, raw_json = ?, \
                 local_rev = ? WHERE local_id = ?",
            )
            .bind(to_json(&raw)?)
            .bind(rev)
            .bind(local_id)
            .execute(&mut *tx)
            .await?;
            Ok((json!({ "body": { "displayName": name } }), Some(before)))
        }
        ListWrite::Rename { name } => {
            let before = snapshot(tx, local_id, Vec::new()).await?;
            let mut raw = before["raw"].as_object().cloned().unwrap_or_default();
            raw.insert("displayName".into(), json!(name));
            sqlx::query(
                "UPDATE lists SET display_name = ?, raw_json = ?, local_rev = ? WHERE local_id = ?",
            )
            .bind(name)
            .bind(to_json(&raw)?)
            .bind(rev)
            .bind(local_id)
            .execute(&mut *tx)
            .await?;
            Ok((json!({ "body": { "displayName": name } }), Some(before)))
        }
        ListWrite::Delete => {
            let tasks: Vec<String> = sqlx::query_scalar(
                "SELECT local_id FROM tasks WHERE list_local_id = ? AND deleted_at IS NULL \
                 ORDER BY rowid",
            )
            .bind(local_id)
            .fetch_all(&mut *tx)
            .await?;
            let before = snapshot(tx, local_id, tasks).await?;
            tombstone(tx, local_id, rev).await?;
            Ok((json!({ "body": {} }), Some(before)))
        }
    }
}

/// Undo a list operation's local change by `restore`: `Tombstone` takes
/// the list (and its tasks) away, `Replace` brings back the list and the
/// tasks a rollback names.
pub(crate) async fn restore_list(
    tx: &mut SqliteConnection,
    local_id: &str,
    restore: &Restore,
    rev: i64,
) -> Result<(), StoreError> {
    match restore {
        Restore::Nothing | Restore::MoveBack { .. } => Ok(()),
        Restore::Tombstone => tombstone(tx, local_id, rev).await,
        Restore::Replace(before) => {
            let raw = before
                .get("raw")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let columns = ListColumns::of(&raw);
            sqlx::query(
                "UPDATE lists SET display_name = ?, raw_json = ?, deleted_at = NULL, local_rev = ? \
                 WHERE local_id = ?",
            )
            .bind(&columns.display_name)
            .bind(to_json(&raw)?)
            .bind(rev)
            .bind(local_id)
            .execute(&mut *tx)
            .await?;
            for task in before
                .get("tasks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                sqlx::query("UPDATE tasks SET deleted_at = NULL, local_rev = ? WHERE local_id = ?")
                    .bind(rev)
                    .bind(task)
                    .execute(&mut *tx)
                    .await?;
            }
            Ok(())
        }
    }
}

/// The list and its live tasks go: tombstoned, written at `rev`.
async fn tombstone(tx: &mut SqliteConnection, local_id: &str, rev: i64) -> Result<(), StoreError> {
    let at = now();
    sqlx::query(
        "UPDATE lists SET deleted_at = COALESCE(deleted_at, ?), local_rev = ? WHERE local_id = ?",
    )
    .bind(at)
    .bind(rev)
    .bind(local_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE tasks SET deleted_at = ?, local_rev = ? \
         WHERE list_local_id = ? AND deleted_at IS NULL",
    )
    .bind(at)
    .bind(rev)
    .bind(local_id)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// What a list operation rolls back to: the list's JSON, and `tasks`.
async fn snapshot(
    tx: &mut SqliteConnection,
    local_id: &str,
    tasks: Vec<String>,
) -> Result<Entity, StoreError> {
    let raw: Option<String> = sqlx::query_scalar("SELECT raw_json FROM lists WHERE local_id = ?")
        .bind(local_id)
        .fetch_optional(&mut *tx)
        .await?;
    let raw = raw.ok_or_else(|| StoreError::Invalid(format!("list {local_id} isn't cached")))?;
    let mut before = Map::new();
    before.insert("raw".into(), Value::Object(parse_object(&raw)?));
    before.insert("tasks".into(), json!(tasks));
    Ok(before)
}

/// A list's cached extension, `{}` for none.
async fn extension_now(tx: &mut SqliteConnection, local_id: &str) -> Result<Entity, StoreError> {
    let list = list_in(tx, local_id)
        .await?
        .ok_or_else(|| StoreError::Invalid(format!("list {local_id} isn't cached")))?;
    Ok(list
        .extension
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

/// The list `local_id`, tombstoned or not, as a [`ListRow`].
async fn list_any(tx: &mut SqliteConnection, local_id: &str) -> Result<ListRow, StoreError> {
    crate::lists::list_including_deleted(tx, local_id)
        .await?
        .ok_or_else(|| StoreError::Corrupt(format!("list {local_id} vanished")))
}

/// Merge into the list `local_id` any other row with the Graph ID
/// `graph_id` (a sync inserted it before the create's answer was
/// recorded): its tasks and operations move to `local_id`, and it goes.
async fn merge_duplicate_list(
    tx: &mut SqliteConnection,
    graph_id: &str,
    local_id: &str,
) -> Result<(), StoreError> {
    let duplicate: Option<String> =
        sqlx::query_scalar("SELECT local_id FROM lists WHERE graph_id = ? AND local_id != ?")
            .bind(graph_id)
            .bind(local_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(duplicate) = duplicate else {
        return Ok(());
    };
    sqlx::query("UPDATE tasks SET list_local_id = ? WHERE list_local_id = ?")
        .bind(local_id)
        .bind(&duplicate)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE outbox SET list_local_id = ? WHERE list_local_id = ?")
        .bind(local_id)
        .bind(&duplicate)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE outbox SET entity_local_id = ? WHERE entity_local_id = ?")
        .bind(local_id)
        .bind(&duplicate)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM lists WHERE local_id = ?")
        .bind(&duplicate)
        .execute(&mut *tx)
        .await?;
    Ok(())
}
