//! Recording what happened to a sent operation: in flight, deferred,
//! `unknown`, or answered by Graph and written to its task.

use serde_json::Value;
use sqlx::SqliteConnection;

use super::SKIPPED_NOTE;
use super::enqueue::apply_body;
use super::operation::{OpKind, OpState, OutboxRow, op_in};
use super::task_rows::{replace_row, row_identity, tombstone_row};
use crate::children::{ATTACHMENTS, apply_child, rename_child};
use crate::graph_columns::{etag, text};
use crate::list_extension::merge_extension;
use crate::pool::next_local_rev;
use crate::tasks::{WriteTask, carry_attachments, write_task};
use crate::{Entity, Store, StoreError, now, parse_object, parse_optional, to_json};

impl Store {
    /// `pending` → `inflight`, counting the attempt. False if the operation
    /// isn't `pending` any more (discarded, or retried meanwhile).
    pub async fn mark_inflight(&self, op_id: &str) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE outbox SET state = 'inflight', attempts = attempts + 1, sent_at = ? \
             WHERE op_id = ? AND state = 'pending'",
        )
        .bind(now())
        .bind(op_id)
        .execute(self.writer())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// A temporary failure: back to `pending`, and nothing is sent before
    /// `until`, since whatever stopped this (the network, throttling, the
    /// sign-in) stops the rest too.
    pub async fn defer(
        &self,
        op_id: &str,
        until: i64,
        error: (&str, &str),
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        sqlx::query(
            "UPDATE outbox SET state = 'pending', next_attempt_at = ?, last_error_kind = ?, \
             last_error = ? WHERE op_id = ?",
        )
        .bind(until)
        .bind(error.0)
        .bind(error.1)
        .bind(op_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE outbox SET next_attempt_at = MAX(next_attempt_at, ?) WHERE state = 'pending'",
        )
        .bind(until)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Sent, but nothing says whether Graph applied it.
    pub async fn mark_unknown(
        &self,
        op_id: &str,
        error: (&str, &str),
        note: Option<&str>,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE outbox SET state = 'unknown', unknown_since = COALESCE(unknown_since, ?), \
             last_error_kind = ?, last_error = ?, note = COALESCE(?, note) WHERE op_id = ?",
        )
        .bind(now())
        .bind(error.0)
        .bind(error.1)
        .bind(note)
        .bind(op_id)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// What was seen while looking for an `unknown` operation's outcome.
    /// Set `key` in operation `op_id`'s payload to `value`: what the
    /// daemon learns while sending it and needs again on a retry or undo.
    pub async fn set_payload(
        &self,
        op_id: &str,
        key: &str,
        value: &Value,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE outbox SET payload_json = json_set(payload_json, ?, json(?)) WHERE op_id = ?",
        )
        .bind(format!("$.{key}"))
        .bind(value.to_string())
        .bind(op_id)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    pub async fn set_note(&self, op_id: &str, note: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE outbox SET note = ? WHERE op_id = ?")
            .bind(note)
            .bind(op_id)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    /// [`Store::record_sent`] for an operation that overwrote a change
    /// another device made (last write wins, D-065), done, in the same
    /// transaction as `note`, which says what it overwrote, and `theirs`,
    /// Graph's copy just before ours landed, which becomes what `undo`
    /// puts back: undoing the edit restores the other device's change, and
    /// no crash between two writes can leave it restoring the older value.
    pub async fn record_overwrite(
        &self,
        op_id: &str,
        raw: &Entity,
        extension: Option<Option<Value>>,
        note: &str,
        theirs: &Entity,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        record_in(&mut tx, op_id, raw, extension).await?;
        finish(&mut tx, op_id, OpState::Done, None).await?;
        sqlx::query("UPDATE outbox SET note = ?, rollback_json = ? WHERE op_id = ?")
            .bind(note)
            .bind(to_json(theirs)?)
            .bind(op_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Graph answered operation `op_id` with the task `raw` (and our
    /// extension, when the answer said): write it to the operation's task,
    /// with the fields of its later unresolved operations on top, and, if
    /// `done`, mark the operation `done`. If sync already cached the task
    /// under another local ID, that row merges into this one (04: the
    /// atomic identity merge).
    pub async fn record_sent(
        &self,
        op_id: &str,
        raw: &Entity,
        extension: Option<Option<Value>>,
        done: bool,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        record_in(&mut tx, op_id, raw, extension).await?;
        if done {
            finish(&mut tx, op_id, OpState::Done, None).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Graph created child operation `op_id`'s step, link or attachment
    /// as `created`:
    /// its placeholder ID becomes Graph's, in the task and in every
    /// operation that names it (so an undo, or a check queued before the
    /// answer, reaches it), and the operation is `done`, in one
    /// transaction. `task` is the task as Graph has it now, when it could
    /// be read after the create (the create moved its etag, S1).
    pub async fn record_child_created(
        &self,
        op_id: &str,
        created: &Entity,
        task: Option<(Entity, Option<Option<Value>>)>,
    ) -> Result<(), StoreError> {
        let created_id = text(created, "id").ok_or_else(|| {
            StoreError::Invalid("Graph returned a step, link or attachment with no id".into())
        })?;
        let mut tx = self.writer().begin().await?;
        let op = op_in(&mut tx, op_id).await?;
        let placeholder = op.payload["id"].as_str().unwrap_or_default().to_owned();
        let collection = op.payload["collection"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        sqlx::query(
            "UPDATE outbox SET payload_json = json_set(payload_json, '$.id', ?) \
             WHERE entity_local_id = ? AND op = 'child' AND json_extract(payload_json, '$.id') = ?",
        )
        .bind(&created_id)
        .bind(&op.entity_local_id)
        .bind(&placeholder)
        .execute(&mut *tx)
        .await?;
        // What they roll back to names it too. A placeholder is a fresh
        // UUID, so replacing it as a JSON string can't touch anything else.
        sqlx::query(
            "UPDATE outbox SET rollback_json = replace(rollback_json, ?, ?) \
             WHERE entity_local_id = ? AND op = 'child' AND rollback_json IS NOT NULL",
        )
        .bind(
            serde_json::to_string(&placeholder)
                .map_err(|error| StoreError::Invalid(error.to_string()))?,
        )
        .bind(
            serde_json::to_string(&created_id)
                .map_err(|error| StoreError::Invalid(error.to_string()))?,
        )
        .bind(&op.entity_local_id)
        .execute(&mut *tx)
        .await?;
        // Graph's JSON read back has a new step or link inline, but never
        // the attachments: those are renamed in the cached task, which the
        // write of Graph's JSON then carries.
        if task.is_none() || collection == ATTACHMENTS {
            let rev = next_local_rev(&mut tx).await?;
            let mut raw =
                parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
            rename_child(&mut raw, &collection, &placeholder, created);
            replace_row(&mut tx, &op.entity_local_id, &raw, rev).await?;
        }
        if let Some((raw, extension)) = task {
            record_in(&mut tx, op_id, &raw, extension).await?;
        }
        finish(&mut tx, op_id, OpState::Done, None).await?;
        tx.commit().await?;
        Ok(())
    }

    /// An `unknown` create found by its `opId` on the cached task
    /// `found_local_id`, whose list was just confirmed live: that's the
    /// task it made. Merge it into the operation's own task, which keeps
    /// its local ID, and mark the operation `done`, in one transaction.
    /// Returns false, changing nothing, if the operation isn't `unknown`
    /// any more (the user retried or discarded it meanwhile).
    pub async fn adopt(&self, op_id: &str, found_local_id: &str) -> Result<bool, StoreError> {
        let mut tx = self.writer().begin().await?;
        let claimed = sqlx::query(
            "UPDATE outbox SET state = 'done', finished_at = ? WHERE op_id = ? AND state = 'unknown'",
        )
        .bind(now())
        .bind(op_id)
        .execute(&mut *tx)
        .await?;
        if claimed.rows_affected() == 0 {
            return Ok(false);
        }
        let rev = next_local_rev(&mut tx).await?;
        let op = op_in(&mut tx, op_id).await?;
        if found_local_id != op.entity_local_id {
            let found = row_identity(&mut tx, found_local_id).await?;
            let graph_id = found.graph_id.ok_or_else(|| {
                StoreError::Invalid(format!("task {found_local_id} has no Graph ID to adopt"))
            })?;
            let attributed = Attributed {
                graph_id: &graph_id,
                list_local_id: &found.list_local_id,
                raw: parse_object(&found.raw_json)?,
                extension_json: found.extension_json,
                hydrated_etag: found.hydrated_etag,
            };
            write_attributed(&mut tx, &op, attributed, rev).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Mark an operation `done` without changing its task (a delete, or a
    /// create whose task is already recorded).
    pub async fn mark_done(&self, op_id: &str) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        finish(&mut tx, op_id, OpState::Done, None).await?;
        // A task deleted takes its steps, link and files with it: a child
        // write queued before the delete and not sent, or unknown, is moot.
        sqlx::query(
            "UPDATE outbox SET state = 'done', finished_at = ?, note = ? \
             WHERE op = 'child' AND state IN ('pending', 'unknown') AND EXISTS \
             (SELECT 1 FROM outbox d WHERE d.op_id = ? AND d.op = 'delete' \
             AND d.entity_local_id = outbox.entity_local_id AND d.seq > outbox.seq)",
        )
        .bind(now())
        .bind(format!("{SKIPPED_NOTE} its task was deleted"))
        .bind(op_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// After a restart, nothing is being sent. A create or a recurring
    /// completion that was may have reached Graph, so it's `unknown`; any
    /// other operation is safe to send again, so it's `pending` (04). So
    /// is an attachment's upload session that hadn't begun its final PUT,
    /// the only part that commits anything (D-067): it goes on.
    /// Returns how many became `unknown`.
    pub async fn recover_inflight(&self) -> Result<u64, StoreError> {
        let mut tx = self.writer().begin().await?;
        let unknown = sqlx::query(
            "UPDATE outbox SET state = 'unknown', unknown_since = ?, \
             note = 'the daemon stopped while this was being sent' \
             WHERE state = 'inflight' \
             AND (op IN ('create', 'list_create') OR json_extract(payload_json, '$.recurring') = 1 \
             OR (op = 'child' AND json_extract(payload_json, '$.verb') = 'create' \
                 AND NOT (json_extract(progress_json, '$.upload.total') IS NOT NULL \
                          AND COALESCE(json_extract(progress_json, '$.upload.committing'), 0) = 0)) \
             OR (op = 'move' AND json_extract(progress_json, '$.in_doubt') = 1))",
        )
        .bind(now())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query("UPDATE outbox SET state = 'pending' WHERE state = 'inflight'")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(unknown)
    }
}

/// Write Graph's answer `raw` to operation `op_id`'s task (see
/// [`Store::record_sent`]), inside `tx`.
pub(crate) async fn record_in(
    tx: &mut SqliteConnection,
    op_id: &str,
    raw: &Entity,
    extension: Option<Option<Value>>,
) -> Result<(), StoreError> {
    let graph_id = text(raw, "id")
        .ok_or_else(|| StoreError::Invalid("Graph returned a task with no id".into()))?;
    let rev = next_local_rev(tx).await?;
    let op = op_in(tx, op_id).await?;
    let (extension_json, hydrated_etag) = match extension {
        Some(extension) => (extension.as_ref().map(Value::to_string), etag(raw)),
        None => sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT extension_json, hydrated_etag FROM tasks WHERE local_id = ?",
        )
        .bind(&op.entity_local_id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or_default(),
    };
    let attributed = Attributed {
        graph_id: &graph_id,
        list_local_id: &op.list_local_id,
        raw: raw.clone(),
        extension_json,
        hydrated_etag,
    };
    write_attributed(tx, &op, attributed, rev).await
}

/// The operations on `op`'s entity queued after it and not resolved yet,
/// in order, with their payloads: what goes on top of Graph's answer to it.
pub(crate) async fn later_ops(
    tx: &mut SqliteConnection,
    op: &OutboxRow,
) -> Result<Vec<(OpKind, Value)>, StoreError> {
    let later: Vec<(String, String)> = sqlx::query_as(concat!(
        "SELECT op, payload_json FROM outbox WHERE entity_local_id = ? AND seq > ? AND state IN ",
        unresolved!(),
        " ORDER BY seq"
    ))
    .bind(&op.entity_local_id)
    .bind(op.seq)
    .fetch_all(&mut *tx)
    .await?;
    later
        .into_iter()
        .map(|(kind, payload)| {
            Ok((
                OpKind::parse(&kind)?,
                parse_optional(Some(payload))?.unwrap_or_default(),
            ))
        })
        .collect()
}

pub(crate) async fn finish(
    tx: &mut SqliteConnection,
    op_id: &str,
    state: OpState,
    error: Option<(&str, &str)>,
) -> Result<(), StoreError> {
    sqlx::query(
        "UPDATE outbox SET state = ?, finished_at = ?, last_error_kind = ?, last_error = ? \
         WHERE op_id = ?",
    )
    .bind(state.as_str())
    .bind(now())
    .bind(error.map(|(kind, _)| kind))
    .bind(error.map(|(_, message)| message))
    .bind(op_id)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// A task as Graph has it, for the operation that made or changed it.
struct Attributed<'a> {
    graph_id: &'a str,
    list_local_id: &'a str,
    raw: Entity,
    extension_json: Option<String>,
    hydrated_etag: Option<String>,
}

/// Write `task` to `op`'s task row, merging in any other row that has its
/// Graph ID, with the fields of `op`'s later unresolved operations on top.
async fn write_attributed(
    tx: &mut SqliteConnection,
    op: &OutboxRow,
    task: Attributed<'_>,
    rev: i64,
) -> Result<(), StoreError> {
    let local_id = &op.entity_local_id;
    let mut raw = task.raw;
    // Before the merge, which may drop the row that holds them.
    carry_attachments(tx, task.graph_id, &mut raw).await?;
    merge_duplicate(tx, task.graph_id, local_id).await?;
    let mut deleted = false;
    let mut extension: Option<Entity> = task
        .extension_json
        .as_deref()
        .map(parse_object)
        .transpose()?;
    let mut extension_changed = false;
    for (kind, payload) in later_ops(tx, op).await? {
        match kind {
            OpKind::Update => apply_body(&mut raw, &payload["body"]),
            OpKind::Delete => deleted = true,
            OpKind::TaskExtension => {
                if let Some(fields) = payload["body"].as_object() {
                    merge_extension(extension.get_or_insert_with(Entity::new), fields);
                    extension_changed = true;
                }
            }
            OpKind::Child => apply_child(&mut raw, &payload),
            OpKind::Create
            | OpKind::Extension
            | OpKind::Move
            | OpKind::ListCreate
            | OpKind::ListUpdate
            | OpKind::ListDelete
            | OpKind::Remote => {}
        }
    }
    let extension_json = if extension_changed {
        extension
            .filter(|extension| !extension.is_empty())
            .map(|extension| to_json(&extension))
            .transpose()?
    } else {
        task.extension_json
    };
    write_task(
        tx,
        &WriteTask {
            local_id,
            graph_id: Some(task.graph_id),
            list_local_id: task.list_local_id,
            raw: &raw,
            raw_json: &to_json(&raw)?,
            extension_json: extension_json.as_deref(),
            hydrated_etag: task.hydrated_etag.as_deref(),
            local_rev: rev,
        },
    )
    .await?;
    if deleted {
        tombstone_row(tx, local_id, rev).await?;
    }
    Ok(())
}

/// Merge into the task `local_id` any other row with the Graph ID
/// `graph_id` (one sync inserted meanwhile): its operations are re-pointed
/// to `local_id` and the row goes, so `graph_id` stays unique.
pub(crate) async fn merge_duplicate(
    tx: &mut SqliteConnection,
    graph_id: &str,
    local_id: &str,
) -> Result<(), StoreError> {
    let duplicate: Option<String> =
        sqlx::query_scalar("SELECT local_id FROM tasks WHERE graph_id = ? AND local_id != ?")
            .bind(graph_id)
            .bind(local_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(duplicate) = duplicate {
        sqlx::query("UPDATE outbox SET entity_local_id = ? WHERE entity_local_id = ?")
            .bind(local_id)
            .bind(&duplicate)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM tasks WHERE local_id = ?")
            .bind(&duplicate)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}
