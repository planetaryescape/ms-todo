//! Reading the outbox, and the rows its operations are on.

use sqlx::{AssertSqlSafe, FromRow};

use super::operation::{OpState, OutboxRow, ops_sql, rows};
use crate::tasks::{TaskRecord, task_record_columns};
use crate::{Store, StoreError, TaskRow, now};

impl Store {
    /// A task by local ID, tombstoned or not, and whether it's tombstoned.
    pub async fn task_any(&self, local_id: &str) -> Result<Option<(TaskRow, bool)>, StoreError> {
        #[derive(FromRow)]
        struct Any {
            #[sqlx(flatten)]
            record: TaskRecord,
            deleted: bool,
        }
        let found: Option<Any> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {}, deleted_at IS NOT NULL AS deleted FROM tasks WHERE local_id = ?",
            task_record_columns()
        )))
        .bind(local_id)
        .fetch_optional(self.reader())
        .await?;
        found
            .map(|any| Ok((TaskRow::try_from(any.record)?, any.deleted)))
            .transpose()
    }

    /// A list's Graph ID and whether it's tombstoned, by local ID.
    pub async fn list_state(
        &self,
        local_id: &str,
    ) -> Result<Option<(Option<String>, bool)>, StoreError> {
        Ok(
            sqlx::query_as("SELECT graph_id, deleted_at IS NOT NULL FROM lists WHERE local_id = ?")
                .bind(local_id)
                .fetch_optional(self.reader())
                .await?,
        )
    }

    /// `pending` operations due by `now` whose dependency, if any, is
    /// `done`, in queue order. Only `done` unblocks: one queued on a change
    /// that failed or was discarded would build on something that never
    /// happened. One that waits for a whole command (`after_command`, a
    /// merge's list delete) waits while any of its operations is
    /// unresolved; the send then checks how they ended.
    pub async fn ready_ops(&self, now: i64) -> Result<Vec<OutboxRow>, StoreError> {
        rows(
            sqlx::query_as(AssertSqlSafe(ops_sql(concat!(
                "o.state = 'pending' AND o.next_attempt_at <= ? AND (o.depends_on_op_id IS NULL \
                 OR EXISTS (SELECT 1 FROM outbox d WHERE d.op_id = o.depends_on_op_id \
                 AND d.state = 'done')) \
                 AND NOT EXISTS (SELECT 1 FROM outbox w \
                 WHERE w.command_id = json_extract(o.payload_json, '$.after_command') \
                 AND w.state IN ",
                unresolved!(),
                ") ORDER BY o.seq"
            ))))
            .bind(now)
            .fetch_all(self.reader())
            .await?,
        )
    }

    /// When the next `pending` operation waiting out a backoff is due, if
    /// any is. One due already that isn't sent waits for another operation
    /// instead, and is woken by that one's outcome.
    pub async fn next_attempt_at(&self, now: i64) -> Result<Option<i64>, StoreError> {
        Ok(sqlx::query_scalar(
            "SELECT MIN(next_attempt_at) FROM outbox WHERE state = 'pending' AND next_attempt_at > ?",
        )
        .bind(now)
        .fetch_one(self.reader())
        .await?)
    }

    /// The staged files (D-067) that attachment adds not done yet read:
    /// what the daemon must keep.
    pub async fn staged_files_in_use(&self) -> Result<Vec<String>, StoreError> {
        Ok(sqlx::query_scalar(
            "SELECT json_extract(payload_json, '$.file.path') FROM outbox \
             WHERE op = 'child' AND state != 'done' \
             AND json_extract(payload_json, '$.file.staged') = 1 \
             AND json_extract(payload_json, '$.file.path') IS NOT NULL",
        )
        .fetch_all(self.reader())
        .await?)
    }

    pub async fn outbox_op(&self, op_id: &str) -> Result<Option<OutboxRow>, StoreError> {
        Ok(self.ops_with("o.op_id = ?", op_id).await?.pop())
    }

    /// Every operation, newest first, or only those in `state`.
    pub async fn outbox(&self, state: Option<OpState>) -> Result<Vec<OutboxRow>, StoreError> {
        match state {
            Some(state) => {
                self.ops_with("o.state = ? ORDER BY o.seq DESC", state.as_str())
                    .await
            }
            None => rows(
                sqlx::query_as(AssertSqlSafe(ops_sql("1 ORDER BY o.seq DESC")))
                    .fetch_all(self.reader())
                    .await?,
            ),
        }
    }

    /// The operations of the command `id`, or the one operation `id`, in
    /// queue order.
    pub async fn command_ops(&self, id: &str) -> Result<Vec<OutboxRow>, StoreError> {
        let command = self.ops_with("o.command_id = ? ORDER BY o.seq", id).await?;
        if !command.is_empty() {
            return Ok(command);
        }
        self.ops_with("o.op_id = ?", id).await
    }

    /// The latest command that isn't an undo, hasn't been undone and was
    /// the user's own: one the daemon made by itself (the My Day rollover)
    /// has `"origin": "auto"` in its payload, and is undone only by name.
    pub async fn last_undoable_command(&self) -> Result<Option<String>, StoreError> {
        Ok(sqlx::query_scalar(
            "SELECT command_id FROM outbox o WHERE undoes_command_id IS NULL \
             AND json_extract(o.payload_json, '$.origin') IS NOT 'auto' \
             AND NOT EXISTS (SELECT 1 FROM outbox u WHERE u.undoes_command_id = o.command_id) \
             GROUP BY command_id ORDER BY MAX(seq) DESC LIMIT 1",
        )
        .fetch_optional(self.reader())
        .await?)
    }

    /// The command that undid `command_id`, if one did.
    pub async fn undone_by(&self, command_id: &str) -> Result<Option<String>, StoreError> {
        Ok(
            sqlx::query_scalar("SELECT command_id FROM outbox WHERE undoes_command_id = ? LIMIT 1")
                .bind(command_id)
                .fetch_optional(self.reader())
                .await?,
        )
    }

    /// How many operations are in each state, and how many `unknown` ones
    /// are flagged for the user: unknown for over `lookup_secs`, or with
    /// no way to be attributed ([`OutboxRow::is_flagged`]).
    pub async fn outbox_depth(
        &self,
        lookup_secs: i64,
    ) -> Result<(Vec<(OpState, i64)>, i64), StoreError> {
        let counts: Vec<(String, i64)> =
            sqlx::query_as("SELECT state, COUNT(*) FROM outbox GROUP BY state")
                .fetch_all(self.reader())
                .await?;
        let flagged: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outbox WHERE state = 'unknown' AND (unknown_since <= ? \
             OR json_extract(progress_json, '$.needs_user') = 1 OR op IN ('child', 'list_create'))",
        )
        .bind(now() - lookup_secs)
        .fetch_one(self.reader())
        .await?;
        let counts = counts
            .into_iter()
            .map(|(state, count)| Ok((OpState::parse(&state)?, count)))
            .collect::<Result<_, StoreError>>()?;
        Ok((counts, flagged))
    }

    /// Live cached tasks with a Graph ID whose extension carries `opId`
    /// `op_id`: what an `unknown` create may have made.
    pub async fn tasks_with_op_id(&self, op_id: &str) -> Result<Vec<TaskRow>, StoreError> {
        let records: Vec<TaskRecord> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {} FROM tasks WHERE json_extract(extension_json, '$.opId') = ? \
             AND graph_id IS NOT NULL AND deleted_at IS NULL ORDER BY rowid",
            task_record_columns()
        )))
        .bind(op_id)
        .fetch_all(self.reader())
        .await?;
        records.into_iter().map(TaskRow::try_from).collect()
    }

    /// Operations matching `condition`, which has one `?`, bound to `value`.
    async fn ops_with(&self, condition: &str, value: &str) -> Result<Vec<OutboxRow>, StoreError> {
        rows(
            sqlx::query_as(AssertSqlSafe(ops_sql(condition)))
                .bind(value)
                .fetch_all(self.reader())
                .await?,
        )
    }
}
