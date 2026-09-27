//! Undoing an operation's local change: when Graph rejects it, when the
//! user discards or retries it, and for every operation that waited on it.

use serde_json::Value;
use sqlx::SqliteConnection;

use super::operation::{OpKind, OpState, OutboxRow, op_in};
use super::outcomes::finish;
use super::task_rows::{replace_row, row_identity, tombstone_row, write_task_extension};
use crate::children::revert_child;
use crate::list_extension::restore_list_extension;
use crate::pool::next_local_rev;
use crate::{Entity, Store, StoreError, now, parse_object};

/// What a resolved operation does to its task row.
#[derive(Clone, Debug, PartialEq)]
pub enum Restore {
    Nothing,
    Tombstone,
    /// The task's JSON becomes this, and it's live.
    Replace(Entity),
    /// A move undone: the task is back in `list_local_id` as `raw`, with
    /// the Graph ID it had there (none if it had none), and it's live.
    MoveBack {
        graph_id: Option<String>,
        list_local_id: String,
        raw: Entity,
    },
}

impl Store {
    /// Graph rejected `op_id` for good: mark it `failed` with `error` and
    /// roll its task back by `restore`, in one transaction. Every operation
    /// that waits on it, directly or not, fails with it: each was queued
    /// on top of a change that won't happen (an undo's re-create of a task
    /// whose delete was rejected would otherwise make a second copy).
    /// Returns those.
    pub async fn fail_op(
        &self,
        op_id: &str,
        error: (&str, &str),
        restore: &Restore,
    ) -> Result<Vec<String>, StoreError> {
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let op = op_in(&mut tx, op_id).await?;
        apply_restore(&mut tx, &op, restore, rev).await?;
        finish(&mut tx, op_id, OpState::Failed, Some(error)).await?;
        let cause = format!(
            "not sent: it waited on {op_id}, which Graph rejected ({})",
            error.1
        );
        let cascaded = cascade(&mut tx, op_id, &cause, rev).await?;
        if op.op == OpKind::Create {
            // Nothing queued after it can bring back a task never created.
            tombstone_row(&mut tx, &op.entity_local_id, rev).await?;
        }
        tx.commit().await?;
        Ok(cascaded)
    }

    /// Drop `op_id`, undoing its local change by `restore`, if it's still
    /// in state `expected`: one conditional delete, so it can't race the
    /// worker claiming it. Every operation that waits on it fails, in the
    /// same transaction. Its idempotency key is kept for 24 hours from now.
    /// Returns the operations failed with it, or `None` if its state moved.
    pub async fn discard_op(
        &self,
        op_id: &str,
        expected: OpState,
        restore: &Restore,
    ) -> Result<Option<Vec<String>>, StoreError> {
        let mut tx = self.writer().begin().await?;
        let op = op_in(&mut tx, op_id).await?;
        let deleted = sqlx::query("DELETE FROM outbox WHERE op_id = ? AND state = ?")
            .bind(op_id)
            .bind(expected.as_str())
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() == 0 {
            return Ok(None);
        }
        let rev = next_local_rev(&mut tx).await?;
        apply_restore(&mut tx, &op, restore, rev).await?;
        let cause = format!("not sent: it waited on {op_id}, which was discarded");
        let cascaded = cascade(&mut tx, op_id, &cause, rev).await?;
        sqlx::query(
            "UPDATE idempotency_keys SET finished_at = ? \
             WHERE op_id = ? AND finished_at IS NOT NULL",
        )
        .bind(now())
        .bind(&op.command_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(cascaded))
    }

    /// Queue `op_id` again, now, making `restore` its local change again,
    /// if it's still in state `expected`: one conditional update, so it
    /// can't race a send. Returns false, changing nothing, if its state
    /// moved.
    /// A move's `progress` is replaced with the one given, if any.
    pub async fn requeue(
        &self,
        op_id: &str,
        expected: OpState,
        restore: &Restore,
        progress: Option<&Value>,
    ) -> Result<bool, StoreError> {
        let mut tx = self.writer().begin().await?;
        let requeued = sqlx::query(
            "UPDATE outbox SET state = 'pending', next_attempt_at = 0, last_error_kind = NULL, \
             last_error = NULL, unknown_since = NULL, note = NULL, finished_at = NULL, \
             progress_json = COALESCE(?, progress_json) WHERE op_id = ? AND state = ?",
        )
        .bind(progress.map(Value::to_string))
        .bind(op_id)
        .bind(expected.as_str())
        .execute(&mut *tx)
        .await?;
        if requeued.rows_affected() == 0 {
            return Ok(false);
        }
        let rev = next_local_rev(&mut tx).await?;
        let op = op_in(&mut tx, op_id).await?;
        apply_restore(&mut tx, &op, restore, rev).await?;
        tx.commit().await?;
        Ok(true)
    }
}

/// Fail the `pending` and `unknown` operations of tasks in the list
/// `list_local_id`, which is gone, and tombstone their tasks, unless one
/// is still being sent. Returns the operations failed.
pub(crate) async fn fail_ops_in_list(
    tx: &mut SqliteConnection,
    list_local_id: &str,
) -> Result<Vec<String>, StoreError> {
    let failed: Vec<(String, String)> = sqlx::query_as(
        "SELECT op_id, entity_local_id FROM outbox \
         WHERE list_local_id = ? AND state IN ('pending', 'unknown') ORDER BY seq",
    )
    .bind(list_local_id)
    .fetch_all(&mut *tx)
    .await?;
    let rev = next_local_rev(tx).await?;
    for (op_id, entity) in &failed {
        finish(
            tx,
            op_id,
            OpState::Failed,
            Some((
                "rejected",
                "the list was deleted on another device, so this was never sent; the task's \
                 content is kept here",
            )),
        )
        .await?;
        sqlx::query(
            "UPDATE tasks SET deleted_at = COALESCE(deleted_at, ?), local_rev = ? \
             WHERE local_id = ? AND NOT EXISTS (SELECT 1 FROM outbox o \
             WHERE o.entity_local_id = tasks.local_id AND o.state = 'inflight')",
        )
        .bind(now())
        .bind(rev)
        .bind(entity)
        .execute(&mut *tx)
        .await?;
    }
    Ok(failed.into_iter().map(|(op_id, _)| op_id).collect())
}

/// Fail every operation not yet sent that waits on `op_id`, directly or
/// through another, with `cause` as its error and note. A step or link
/// write among them is taken back out of its task too, so a step never
/// sent doesn't stay on screen (a quick add's later steps, D-068).
/// Returns them.
async fn cascade(
    tx: &mut SqliteConnection,
    op_id: &str,
    cause: &str,
    rev: i64,
) -> Result<Vec<String>, StoreError> {
    let waiting: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE waiting(op_id) AS ( \
             SELECT op_id FROM outbox WHERE depends_on_op_id = ?1 \
             UNION SELECT o.op_id FROM outbox o JOIN waiting w ON o.depends_on_op_id = w.op_id) \
         SELECT op_id FROM outbox WHERE op_id IN waiting AND state IN ('pending', 'unknown') \
         ORDER BY seq",
    )
    .bind(op_id)
    .fetch_all(&mut *tx)
    .await?;
    for waiter in &waiting {
        let op = op_in(tx, waiter).await?;
        if op.op == OpKind::Child
            && let Some(before) = &op.rollback
            && is_live(tx, &op.entity_local_id).await?
        {
            let current = parse_object(&row_identity(tx, &op.entity_local_id).await?.raw_json)?;
            let reverted = revert_child(&current, &op.payload, before);
            replace_row(tx, &op.entity_local_id, &reverted, rev).await?;
        }
        finish(tx, waiter, OpState::Failed, Some(("rejected", cause))).await?;
        sqlx::query("UPDATE outbox SET note = ? WHERE op_id = ?")
            .bind(cause)
            .bind(waiter)
            .execute(&mut *tx)
            .await?;
    }
    Ok(waiting)
}

/// Whether the task `local_id` is cached and not tombstoned.
async fn is_live(tx: &mut SqliteConnection, local_id: &str) -> Result<bool, StoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM tasks WHERE local_id = ? AND deleted_at IS NULL",
    )
    .bind(local_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some())
}

/// Undo `op`'s local change by `restore`: on its task, or for a list
/// extension write, on the list's extension.
async fn apply_restore(
    tx: &mut SqliteConnection,
    op: &OutboxRow,
    restore: &Restore,
    rev: i64,
) -> Result<(), StoreError> {
    let local_id = op.entity_local_id.as_str();
    if op.op == OpKind::Extension {
        return restore_list_extension(tx, local_id, restore, rev).await;
    }
    if op.op.is_list() {
        return crate::list_lifecycle::restore_list(tx, local_id, restore, rev).await;
    }
    if op.op == OpKind::Remote {
        return Ok(());
    }
    if op.op == OpKind::TaskExtension {
        return match restore {
            Restore::Replace(extension) => write_task_extension(tx, local_id, extension, rev).await,
            Restore::Nothing | Restore::Tombstone | Restore::MoveBack { .. } => Ok(()),
        };
    }
    // A child write's restore is the task's JSON, as an update's is. On a
    // task deleted since, it's moot, and writing the JSON would bring the
    // task back.
    if op.op == OpKind::Child && !is_live(tx, local_id).await? {
        return Ok(());
    }
    match restore {
        Restore::Nothing => Ok(()),
        Restore::Tombstone => tombstone_row(tx, local_id, rev).await,
        Restore::Replace(raw) => replace_row(tx, local_id, raw, rev).await,
        Restore::MoveBack {
            graph_id,
            list_local_id,
            raw,
        } => {
            crate::moves::move_back(tx, local_id, graph_id.as_deref(), list_local_id, raw, rev)
                .await
        }
    }
}
