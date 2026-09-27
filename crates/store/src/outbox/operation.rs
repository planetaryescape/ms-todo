//! An outbox operation as it's stored: what it sends, its state, and its
//! row read back.

use serde_json::Value;
use sqlx::AssertSqlSafe;
use sqlx::{FromRow, SqliteConnection};

use super::SKIPPED_NOTE;
use crate::{Entity, StoreError, parse_object, parse_optional};

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
    /// A list made (or made again): a POST. Its entity is the list.
    ListCreate,
    /// A list renamed: a PATCH of `displayName`.
    ListUpdate,
    /// A list deleted, with its tasks.
    ListDelete,
    /// A category or open-extension write, made straight to Graph and
    /// recorded here `done`, so `undo` can reverse it (D-058). Its
    /// rollback is the thing before, `{}` for none.
    Remote,
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
            Self::ListCreate => "list_create",
            Self::ListUpdate => "list_update",
            Self::ListDelete => "list_delete",
            Self::Remote => "remote",
        }
    }

    /// Whether its entity is a list, not a task.
    pub fn is_list(self) -> bool {
        matches!(
            self,
            Self::Extension | Self::ListCreate | Self::ListUpdate | Self::ListDelete
        )
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
            "list_create" => Ok(Self::ListCreate),
            "list_update" => Ok(Self::ListUpdate),
            "list_delete" => Ok(Self::ListDelete),
            "remote" => Ok(Self::Remote),
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

    pub(super) fn parse(value: &str) -> Result<Self, StoreError> {
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

    /// `unknown` for longer than the lookup window (`lookup_secs`, from
    /// `[outbox] unknown_lookup_hours`), or with nothing that could settle
    /// it but the user: a move paused so, or a child write, which carries
    /// no marker to find it by (04). The user decides.
    pub fn is_flagged(&self, now: i64, lookup_secs: i64) -> bool {
        self.state == OpState::Unknown
            && (self
                .unknown_since
                .is_some_and(|since| since <= now - lookup_secs)
                || self.needs_user()
                || matches!(self.op, OpKind::Child | OpKind::ListCreate))
    }

    /// Done without sending anything: its send-time precondition didn't
    /// hold (My Day's `expect` and `expect_due`, an assignment's
    /// `expect_fields`), so it changed nothing.
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
pub(super) struct OpRecord {
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

pub(super) fn ops_sql(condition: &str) -> String {
    format!("SELECT {OP_COLUMNS} {FROM_OUTBOX} WHERE {condition}")
}

pub(super) fn rows(records: Vec<OpRecord>) -> Result<Vec<OutboxRow>, StoreError> {
    records.into_iter().map(OutboxRow::try_from).collect()
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
