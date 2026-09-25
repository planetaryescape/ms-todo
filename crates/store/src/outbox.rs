//! The outbox (docs/blueprint/02-data-model.md#outbox-semantics): each task
//! write is applied to `tasks` and queued here in one transaction, and the
//! daemon's worker sends the queue to Graph. What an operation's outcome
//! means is the daemon's business; this applies each outcome atomically:
//! the operation's new state together with what it does to the task row.
//!
//! A task with an operation `pending`, `inflight` or `unknown` is never
//! overwritten or tombstoned by a sync pass ([`NO_UNRESOLVED_OPS`]).

use serde_json::Value;
use sqlx::AssertSqlSafe;
use sqlx::{FromRow, SqliteConnection};

use crate::children::{apply_child, rename_child};
use crate::graph_columns::{etag, text};
use crate::list_extension::{merge_extension, restore_list_extension};
use crate::moves::set_list;
use crate::pool::next_local_rev;
use crate::tasks::{TaskRecord, WriteTask, task_record_columns, write_task};
use crate::{Entity, Store, StoreError, TaskRow, now, parse_object, parse_optional, to_json};

/// How the note of an operation that [`OutboxRow::was_skipped`] starts.
pub const SKIPPED_NOTE: &str = "skipped:";

/// How long an `unknown` operation is looked for before it's flagged for
/// the user (`outbox.unknown_lookup_hours`, 04).
pub const UNKNOWN_LOOKUP_SECS: i64 = 24 * 60 * 60;

/// The states of an operation not resolved yet, as an SQL list. Its task
/// stays as ms-todo has it until they're all resolved.
macro_rules! unresolved {
    () => {
        "('pending', 'inflight', 'unknown')"
    };
}

/// A condition on `tasks`: no outbox operation of the row is unresolved.
pub(crate) const NO_UNRESOLVED_OPS: &str = concat!(
    "NOT EXISTS (SELECT 1 FROM outbox o \
     WHERE o.entity_local_id = tasks.local_id AND o.state IN ",
    unresolved!(),
    ")"
);

/// The unresolved states as an SQL list, for queries built at run time.
pub(crate) const UNRESOLVED_STATES: &str = unresolved!();

/// The sync state of a row from its operations, as a column of
/// `SELECT … FROM <table>`: `unknown` beats `pending`, which beats `failed`.
macro_rules! sync_state {
    ($table:literal) => {
        concat!(
            "(SELECT CASE \
             WHEN SUM(o.state = 'unknown') > 0 THEN 'unknown' \
             WHEN SUM(o.state IN ('pending', 'inflight')) > 0 THEN 'pending' \
             WHEN SUM(o.state = 'failed') > 0 THEN 'failed' ELSE 'synced' END \
             FROM outbox o WHERE o.entity_local_id = ",
            $table,
            ".local_id AND o.state != 'done') AS sync_state"
        )
    };
}

/// A column of `SELECT … FROM tasks`: the task's sync state.
pub(crate) const SYNC_STATE: &str = sync_state!("tasks");

/// A column of `SELECT … FROM lists`: the list's sync state, from its
/// extension writes (folders).
pub(crate) const LIST_SYNC_STATE: &str = sync_state!("lists");

// An operation on a list (a folder change) is titled by the list's name.
const OP_COLUMNS: &str = "o.op_id, o.seq, o.command_id, o.created_at, o.entity_local_id, \
     o.list_local_id, o.op, o.action, o.payload_json, o.depends_on_op_id, o.undoes_command_id, \
     o.attempts, o.next_attempt_at, o.state, o.last_error_kind, o.last_error, o.rollback_json, \
     o.sent_at, o.unknown_since, o.note, o.finished_at, o.progress_json, \
     COALESCE(t.title, l.display_name) AS title";

const FROM_OUTBOX: &str = "FROM outbox o LEFT JOIN tasks t ON t.local_id = o.entity_local_id \
     LEFT JOIN lists l ON l.local_id = o.entity_local_id AND o.entity_kind = 'list'";

/// What an operation sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    /// A POST that makes the task.
    Create,
    /// A PATCH of some of its fields.
    Update,
    Delete,
    /// A change to some fields of a list's extension (its folder and
    /// order), sent as GET, merge and a write of the whole document.
    Extension,
    /// A move to another list: a job that copies the task there, checks
    /// the copy and deletes the source, saving each step (`progress`).
    Move,
    /// A change to some fields of a task's extension (My Day), sent as a
    /// list's is. Its rollback is the whole extension before, `{}` for
    /// none.
    TaskExtension,
    /// A write to one of the task's children, a step or its link: a POST,
    /// PATCH or DELETE under the task (`crate::children`). Its rollback is
    /// the task's JSON before.
    Child,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Extension => "extension",
            Self::Move => "move",
            Self::TaskExtension => "task_extension",
            Self::Child => "child",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "create" => Ok(Self::Create),
            "update" => Ok(Self::Update),
            "delete" => Ok(Self::Delete),
            "extension" => Ok(Self::Extension),
            "move" => Ok(Self::Move),
            "task_extension" => Ok(Self::TaskExtension),
            "child" => Ok(Self::Child),
            other => Err(StoreError::Corrupt(format!("unknown outbox op {other:?}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpState {
    Pending,
    Inflight,
    Unknown,
    Failed,
    Done,
}

impl OpState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Inflight => "inflight",
            Self::Unknown => "unknown",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "pending" => Ok(Self::Pending),
            "inflight" => Ok(Self::Inflight),
            "unknown" => Ok(Self::Unknown),
            "failed" => Ok(Self::Failed),
            "done" => Ok(Self::Done),
            other => Err(StoreError::Corrupt(format!(
                "unknown outbox state {other:?}"
            ))),
        }
    }
}

/// One queued operation.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboxRow {
    pub op_id: String,
    pub seq: i64,
    pub command_id: String,
    pub created_at: i64,
    pub entity_local_id: String,
    pub list_local_id: String,
    pub op: OpKind,
    pub action: String,
    pub payload: Value,
    pub depends_on: Option<String>,
    pub undoes: Option<String>,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub state: OpState,
    /// `(ErrorKind string, message)`.
    pub last_error: Option<(String, String)>,
    pub rollback: Option<Entity>,
    pub sent_at: Option<i64>,
    pub unknown_since: Option<i64>,
    pub note: Option<String>,
    pub finished_at: Option<i64>,
    /// A move's steps so far, as the daemon saved them.
    pub progress: Option<Value>,
    /// The task's title as cached.
    pub title: Option<String>,
}

impl OutboxRow {
    /// The Graph JSON it sends.
    pub fn body(&self) -> &Value {
        &self.payload["body"]
    }

    /// For a child write: the fields of `body` sent only because Graph
    /// resets what a PATCH leaves out, not changed by it (S1).
    pub fn carried(&self) -> Vec<&str> {
        self.payload["carried"]
            .as_array()
            .map(|keys| keys.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    /// Whether it completes a recurring task, which is never resent after
    /// an ambiguous answer (D-028).
    pub fn is_recurring_completion(&self) -> bool {
        self.payload["recurring"] == Value::Bool(true)
    }

    /// `unknown` for longer than the lookup window, or with nothing that
    /// could settle it but the user: a move paused so, or a child write,
    /// which carries no marker to find it by (04). The user decides.
    pub fn is_flagged(&self, now: i64) -> bool {
        self.state == OpState::Unknown
            && (self
                .unknown_since
                .is_some_and(|since| since <= now - UNKNOWN_LOOKUP_SECS)
                || self.needs_user()
                || self.op == OpKind::Child)
    }

    /// Done without sending anything: its send-time precondition didn't
    /// hold (My Day's `expect` and `expect_due`), so it changed nothing.
    pub fn was_skipped(&self) -> bool {
        self.state == OpState::Done
            && self
                .note
                .as_deref()
                .is_some_and(|note| note.starts_with(SKIPPED_NOTE))
    }

    /// A move paused where no lookup can find the answer (04).
    pub fn needs_user(&self) -> bool {
        self.progress
            .as_ref()
            .is_some_and(|progress| progress["needs_user"] == Value::Bool(true))
    }
}

#[derive(FromRow)]
struct OpRecord {
    op_id: String,
    seq: i64,
    command_id: String,
    created_at: i64,
    entity_local_id: String,
    list_local_id: String,
    op: String,
    action: String,
    payload_json: String,
    depends_on_op_id: Option<String>,
    undoes_command_id: Option<String>,
    attempts: i64,
    next_attempt_at: i64,
    state: String,
    last_error_kind: Option<String>,
    last_error: Option<String>,
    rollback_json: Option<String>,
    sent_at: Option<i64>,
    unknown_since: Option<i64>,
    note: Option<String>,
    finished_at: Option<i64>,
    progress_json: Option<String>,
    title: Option<String>,
}

impl TryFrom<OpRecord> for OutboxRow {
    type Error = StoreError;

    fn try_from(record: OpRecord) -> Result<Self, StoreError> {
        Ok(Self {
            op_id: record.op_id,
            seq: record.seq,
            command_id: record.command_id,
            created_at: record.created_at,
            entity_local_id: record.entity_local_id,
            list_local_id: record.list_local_id,
            op: OpKind::parse(&record.op)?,
            action: record.action,
            payload: parse_optional(Some(record.payload_json))?.unwrap_or(Value::Null),
            depends_on: record.depends_on_op_id,
            undoes: record.undoes_command_id,
            attempts: record.attempts,
            next_attempt_at: record.next_attempt_at,
            state: OpState::parse(&record.state)?,
            last_error: record.last_error_kind.zip(record.last_error),
            rollback: record
                .rollback_json
                .as_deref()
                .map(parse_object)
                .transpose()?,
            sent_at: record.sent_at,
            unknown_since: record.unknown_since,
            note: record.note,
            finished_at: record.finished_at,
            progress: parse_optional(record.progress_json)?,
            title: record.title,
        })
    }
}

/// An operation to queue, with the local change it makes at once.
#[derive(Clone, Debug)]
pub struct NewOp {
    pub op_id: String,
    pub entity_local_id: String,
    pub list_local_id: String,
    pub op: OpKind,
    pub action: String,
    pub payload: Value,
    pub change: LocalChange,
}

/// What a queued operation does to the task row now, before Graph has it.
#[derive(Clone, Debug)]
pub enum LocalChange {
    /// A new task only ms-todo knows: no Graph ID yet.
    Insert {
        raw: Entity,
        extension: Option<Value>,
    },
    /// The task's JSON becomes `raw`, and it's live.
    Replace(Entity),
    /// The payload's `body` goes over the task's JSON as it is when the
    /// operation is queued, which is also its rollback: a snapshot read
    /// earlier could undo an answer from Graph recorded meanwhile.
    Update,
    /// Tombstone the task; its JSON when queued is the rollback.
    Tombstone,
    /// The task shows in the list `to` at once; its JSON when queued is
    /// the rollback. A move puts it there on Graph.
    Move { to: String },
    /// The payload's `body` goes over the task's extension (a null
    /// removing a field); the extension when queued is the rollback.
    Extension,
    /// The payload's child change goes into the task's JSON
    /// ([`apply_child`]); its JSON when queued is the rollback.
    Child,
}

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

/// Overlay the fields of a PATCH or POST `body` on a task's JSON, as Graph
/// would apply them. Our extension isn't a field.
pub fn apply_body(raw: &mut Entity, body: &Value) {
    if let Value::Object(fields) = body {
        for (key, value) in fields {
            if key != "extensions" {
                raw.insert(key.clone(), value.clone());
            }
        }
    }
}

impl Store {
    /// Queue `ops`, one command's, and make each one's local change, all in
    /// one transaction. Each waits for the latest unresolved operation on
    /// its task. Returns each task as it is now, tombstoned or not.
    pub async fn enqueue(
        &self,
        command_id: &str,
        undoes: Option<&str>,
        ops: Vec<NewOp>,
    ) -> Result<Vec<TaskRow>, StoreError> {
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let now = now();
        let mut seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM outbox")
            .fetch_one(&mut *tx)
            .await?;
        let mut rows = Vec::with_capacity(ops.len());
        for op in ops {
            seq += 1;
            // What a rejection restores and undo inverts: the task before this.
            let mut rollback = None;
            match &op.change {
                LocalChange::Insert { raw, extension } => {
                    let extension_json = extension.as_ref().map(Value::to_string);
                    write_task(
                        &mut tx,
                        &WriteTask {
                            local_id: &op.entity_local_id,
                            graph_id: None,
                            list_local_id: &op.list_local_id,
                            raw,
                            raw_json: &to_json(raw)?,
                            extension_json: extension_json.as_deref(),
                            hydrated_etag: None,
                            local_rev: rev,
                        },
                    )
                    .await?;
                }
                LocalChange::Replace(raw) => {
                    replace_row(&mut tx, &op.entity_local_id, raw, rev).await?;
                }
                LocalChange::Update => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    let mut raw = current.clone();
                    apply_body(&mut raw, &op.payload["body"]);
                    replace_row(&mut tx, &op.entity_local_id, &raw, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Tombstone => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    tombstone_row(&mut tx, &op.entity_local_id, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Move { to } => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    set_list(&mut tx, &op.entity_local_id, to, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Child => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    let mut raw = current.clone();
                    apply_child(&mut raw, &op.payload);
                    replace_row(&mut tx, &op.entity_local_id, &raw, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Extension => {
                    let current = task_extension(&mut tx, &op.entity_local_id).await?;
                    let mut merged = current.clone();
                    if let Some(fields) = op.payload["body"].as_object() {
                        merge_extension(&mut merged, fields);
                    }
                    write_task_extension(&mut tx, &op.entity_local_id, &merged, rev).await?;
                    rollback = Some(current);
                }
            }
            insert_op(
                &mut tx,
                &Queued {
                    op_id: &op.op_id,
                    seq,
                    command_id,
                    undoes,
                    created_at: now,
                    entity_local_id: &op.entity_local_id,
                    list_local_id: &op.list_local_id,
                    op: op.op,
                    action: &op.action,
                    payload: &op.payload,
                    rollback: rollback.as_ref(),
                },
            )
            .await?;
            rows.push(
                fetch_task(&mut tx, &op.entity_local_id)
                    .await?
                    .ok_or_else(|| {
                        StoreError::Corrupt(format!("task {} isn't cached", op.entity_local_id))
                    })?,
            );
        }
        tx.commit().await?;
        Ok(rows)
    }

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
    /// happened.
    pub async fn ready_ops(&self, now: i64) -> Result<Vec<OutboxRow>, StoreError> {
        rows(
            sqlx::query_as(AssertSqlSafe(ops_sql(
                "o.state = 'pending' AND o.next_attempt_at <= ? AND (o.depends_on_op_id IS NULL \
                 OR EXISTS (SELECT 1 FROM outbox d WHERE d.op_id = o.depends_on_op_id \
                 AND d.state = 'done')) ORDER BY o.seq",
            )))
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
    /// are flagged for the user.
    pub async fn outbox_depth(&self) -> Result<(Vec<(OpState, i64)>, i64), StoreError> {
        let counts: Vec<(String, i64)> =
            sqlx::query_as("SELECT state, COUNT(*) FROM outbox GROUP BY state")
                .fetch_all(self.reader())
                .await?;
        let flagged: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outbox WHERE state = 'unknown' AND (unknown_since <= ? \
             OR json_extract(progress_json, '$.needs_user') = 1 OR op = 'child')",
        )
        .bind(now() - UNKNOWN_LOOKUP_SECS)
        .fetch_one(self.reader())
        .await?;
        let counts = counts
            .into_iter()
            .map(|(state, count)| Ok((OpState::parse(&state)?, count)))
            .collect::<Result<_, StoreError>>()?;
        Ok((counts, flagged))
    }

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
    pub async fn set_note(&self, op_id: &str, note: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE outbox SET note = ? WHERE op_id = ?")
            .bind(note)
            .bind(op_id)
            .execute(self.writer())
            .await?;
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

    /// Graph created child operation `op_id`'s step or link as `created`:
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
            StoreError::Invalid("Graph returned a step or link with no id".into())
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
        match task {
            Some((raw, extension)) => record_in(&mut tx, op_id, &raw, extension).await?,
            None => {
                let rev = next_local_rev(&mut tx).await?;
                let mut raw =
                    parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                rename_child(&mut raw, &collection, &placeholder, created);
                replace_row(&mut tx, &op.entity_local_id, &raw, rev).await?;
            }
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
        tx.commit().await?;
        Ok(())
    }

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
        let cascaded = cascade(&mut tx, op_id, &cause).await?;
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
        let cascaded = cascade(&mut tx, op_id, &cause).await?;
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

    /// After a restart, nothing is being sent. A create or a recurring
    /// completion that was may have reached Graph, so it's `unknown`; any
    /// other operation is safe to send again, so it's `pending` (04).
    /// Returns how many became `unknown`.
    pub async fn recover_inflight(&self) -> Result<u64, StoreError> {
        let mut tx = self.writer().begin().await?;
        let unknown = sqlx::query(
            "UPDATE outbox SET state = 'unknown', unknown_since = ?, \
             note = 'the daemon stopped while this was being sent' \
             WHERE state = 'inflight' \
             AND (op = 'create' OR json_extract(payload_json, '$.recurring') = 1 \
             OR (op = 'child' AND json_extract(payload_json, '$.verb') = 'create') \
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

fn ops_sql(condition: &str) -> String {
    format!("SELECT {OP_COLUMNS} {FROM_OUTBOX} WHERE {condition}")
}

fn rows(records: Vec<OpRecord>) -> Result<Vec<OutboxRow>, StoreError> {
    records.into_iter().map(OutboxRow::try_from).collect()
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

/// An operation as it's queued.
pub(crate) struct Queued<'a> {
    pub op_id: &'a str,
    pub seq: i64,
    pub command_id: &'a str,
    pub undoes: Option<&'a str>,
    pub created_at: i64,
    pub entity_local_id: &'a str,
    pub list_local_id: &'a str,
    pub op: OpKind,
    pub action: &'a str,
    pub payload: &'a Value,
    pub rollback: Option<&'a Entity>,
}

/// Add `op` to the queue, `pending`, waiting for the latest unresolved
/// operation on the same entity.
pub(crate) async fn insert_op(
    tx: &mut SqliteConnection,
    op: &Queued<'_>,
) -> Result<(), StoreError> {
    let depends_on: Option<String> = sqlx::query_scalar(concat!(
        "SELECT op_id FROM outbox WHERE entity_local_id = ? AND state IN ",
        unresolved!(),
        " ORDER BY seq DESC LIMIT 1"
    ))
    .bind(op.entity_local_id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO outbox (op_id, seq, command_id, created_at, entity_kind, entity_local_id, \
         list_local_id, op, action, payload_json, depends_on_op_id, undoes_command_id, \
         state, rollback_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
    )
    .bind(op.op_id)
    .bind(op.seq)
    .bind(op.command_id)
    .bind(op.created_at)
    // A folder write is the only operation on a list.
    .bind(if op.op == OpKind::Extension {
        "list"
    } else {
        "task"
    })
    .bind(op.entity_local_id)
    .bind(op.list_local_id)
    .bind(op.op.as_str())
    .bind(op.action)
    .bind(op.payload.to_string())
    .bind(depends_on)
    .bind(op.undoes)
    .bind(op.rollback.map(to_json).transpose()?)
    .execute(&mut *tx)
    .await?;
    Ok(())
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
/// through another, with `cause` as its error and note. Returns them.
async fn cascade(
    tx: &mut SqliteConnection,
    op_id: &str,
    cause: &str,
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
        finish(tx, waiter, OpState::Failed, Some(("rejected", cause))).await?;
        sqlx::query("UPDATE outbox SET note = ? WHERE op_id = ?")
            .bind(cause)
            .bind(waiter)
            .execute(&mut *tx)
            .await?;
    }
    Ok(waiting)
}

pub(crate) async fn op_in(tx: &mut SqliteConnection, op_id: &str) -> Result<OutboxRow, StoreError> {
    let record: Option<OpRecord> = sqlx::query_as(AssertSqlSafe(ops_sql("o.op_id = ?")))
        .bind(op_id)
        .fetch_optional(&mut *tx)
        .await?;
    record
        .map(OutboxRow::try_from)
        .transpose()?
        .ok_or_else(|| StoreError::Invalid(format!("no outbox operation {op_id}")))
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
    merge_duplicate(tx, task.graph_id, local_id).await?;
    let mut raw = task.raw;
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
            OpKind::Create | OpKind::Extension | OpKind::Move => {}
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
    if op.op == OpKind::TaskExtension {
        return match restore {
            Restore::Replace(extension) => write_task_extension(tx, local_id, extension, rev).await,
            Restore::Nothing | Restore::Tombstone | Restore::MoveBack { .. } => Ok(()),
        };
    }
    // A child write's restore is the task's JSON, as an update's is.
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

/// Make `raw` the task's JSON, keeping its identity and extension, and
/// clear any tombstone.
async fn replace_row(
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
async fn task_extension(tx: &mut SqliteConnection, local_id: &str) -> Result<Entity, StoreError> {
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
async fn write_task_extension(
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

async fn tombstone_row(
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

async fn fetch_task(
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
