//! Moves between lists (docs/blueprint/05-custom-features.md#move-between-lists):
//! what each step of a move job does to the outbox and the task, atomically.
//! The task keeps its local ID throughout. It shows in the target list from
//! the moment the move is queued; once the copy exists it takes the copy's
//! Graph ID, and a move undone puts back the Graph ID and list it had.
//!
//! What the steps mean is the daemon's business (`outbox::move_job`); the
//! store only saves them.

use serde_json::Value;
use sqlx::SqliteConnection;

use crate::outbox::{OpState, finish, merge_duplicate, record_in, row_identity};
use crate::tasks::{WriteTask, write_task};
use crate::{Entity, Store, StoreError, parse_optional, to_json};

impl Store {
    /// Save a move's `progress`, the steps it has taken.
    pub async fn set_progress(&self, op_id: &str, progress: &Value) -> Result<(), StoreError> {
        save_progress(&mut *self.writer().acquire().await?, op_id, progress).await
    }

    /// Graph answered a move's copy (`raw`, in the target list): the task
    /// becomes it, merging any row sync made for it, and `progress` is
    /// saved, in one transaction. With `done`, the move is `done`.
    pub async fn record_move(
        &self,
        op_id: &str,
        raw: &Entity,
        extension: Option<Option<Value>>,
        progress: &Value,
        done: bool,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        record_in(&mut tx, op_id, raw, extension).await?;
        save_progress(&mut tx, op_id, progress).await?;
        if done {
            finish(&mut tx, op_id, OpState::Done, None).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A paused (`unknown`) move whose copy was found by its `opId` on the
    /// cached task `found_local_id`: the task becomes the copy, `progress`
    /// is saved, and the move goes back to `pending` to carry on. Returns
    /// false, changing nothing, if it isn't `unknown` any more.
    pub async fn resume_move(
        &self,
        op_id: &str,
        found_local_id: &str,
        progress: &Value,
    ) -> Result<bool, StoreError> {
        let mut tx = self.writer().begin().await?;
        let claimed = sqlx::query(
            "UPDATE outbox SET state = 'pending', next_attempt_at = 0, unknown_since = NULL, \
             note = NULL, last_error_kind = NULL, last_error = NULL \
             WHERE op_id = ? AND state = 'unknown'",
        )
        .bind(op_id)
        .execute(&mut *tx)
        .await?;
        if claimed.rows_affected() == 0 {
            return Ok(false);
        }
        let found = row_identity(&mut tx, found_local_id).await?;
        let raw = crate::parse_object(&found.raw_json)?;
        let extension = parse_optional(found.extension_json)?;
        record_in(&mut tx, op_id, &raw, Some(extension)).await?;
        save_progress(&mut tx, op_id, progress).await?;
        tx.commit().await?;
        Ok(true)
    }
}

async fn save_progress(
    tx: &mut SqliteConnection,
    op_id: &str,
    progress: &Value,
) -> Result<(), StoreError> {
    sqlx::query("UPDATE outbox SET progress_json = ? WHERE op_id = ?")
        .bind(progress.to_string())
        .bind(op_id)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// Show the task `local_id` in the list `list_local_id`, as a queued move
/// does at once.
pub(crate) async fn set_list(
    tx: &mut SqliteConnection,
    local_id: &str,
    list_local_id: &str,
    rev: i64,
) -> Result<(), StoreError> {
    sqlx::query("UPDATE tasks SET list_local_id = ?, local_rev = ? WHERE local_id = ?")
        .bind(list_local_id)
        .bind(rev)
        .bind(local_id)
        .execute(tx)
        .await?;
    Ok(())
}

/// Put the task `local_id` back where a move found it: in `list_local_id`,
/// as `raw`, with the Graph ID `graph_id`, live. A row sync made for that
/// Graph ID meanwhile (it re-read the source list during the move) merges
/// into this one. The extension is read again on the next sync.
pub(crate) async fn move_back(
    tx: &mut SqliteConnection,
    local_id: &str,
    graph_id: Option<&str>,
    list_local_id: &str,
    raw: &Entity,
    rev: i64,
) -> Result<(), StoreError> {
    if let Some(graph_id) = graph_id {
        merge_duplicate(tx, graph_id, local_id).await?;
    }
    let current = row_identity(tx, local_id).await?;
    write_task(
        tx,
        &WriteTask {
            local_id,
            graph_id,
            list_local_id,
            raw,
            raw_json: &to_json(raw)?,
            extension_json: current.extension_json.as_deref(),
            hydrated_etag: None,
            local_rev: rev,
        },
    )
    .await
}
