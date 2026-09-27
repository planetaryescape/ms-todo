//! The outbox as clients see it (docs/blueprint/02-data-model.md#outbox-semantics):
//! its operations, their states, and how many are in each.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How many outbox operations are in each state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxDepth {
    pub pending: u64,
    pub inflight: u64,
    pub unknown: u64,
    pub failed: u64,
    pub done: u64,
    /// `unknown` for over 24 hours, or with no way to be attributed: the
    /// user resolves them with `outbox retry` or `outbox discard`.
    pub flagged: u64,
}

/// How the outbox is kept, for `doctor` (D-065).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxUpkeep {
    /// Operations stored, in every state.
    pub rows: u64,
    /// `[outbox] retention_days`: finished commands are kept this long,
    /// and `undo` reaches this far back.
    pub retention_days: u32,
    /// `[outbox] unknown_lookup_hours`: how long an `unknown` operation is
    /// looked for before it's flagged.
    pub unknown_lookup_hours: u32,
    /// Unix seconds: when finished operations were last pruned, since the
    /// daemon started.
    #[serde(default)]
    pub last_pruned_at: Option<i64>,
    /// How many that prune removed.
    #[serde(default)]
    pub last_pruned: Option<u64>,
    /// Why an `[outbox]` setting wasn't used.
    #[serde(default)]
    pub problem: Option<String>,
}

/// An outbox operation's state (docs/blueprint/02-data-model.md#outbox-semantics).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxState {
    /// Waiting to be sent, or to be sent again after a temporary failure.
    Pending,
    /// Being sent now.
    Inflight,
    /// Sent, but nothing says whether Graph applied it. Never resent by
    /// itself.
    Unknown,
    /// Graph rejected it for good; its local change was rolled back.
    Failed,
    Done,
    /// A state from a newer daemon.
    #[serde(other)]
    Other,
}

/// One outbox operation: one change to one task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutboxOp {
    pub op_id: String,
    /// The `op_id` the mutation returned. A change to several tasks has one
    /// operation per task, `<op_id>` then `<op_id>.1`, `<op_id>.2`, ….
    pub command_id: String,
    /// `add`, `edit`, `complete`, `reopen` or `delete`.
    pub action: String,
    /// The task's local ID.
    pub task_id: String,
    pub list_id: String,
    /// The task's title as ms-todo has it.
    #[serde(default)]
    pub title: Option<String>,
    pub state: OutboxState,
    pub attempts: u32,
    /// Unix seconds.
    pub created_at: i64,
    #[serde(default)]
    pub next_attempt_at: Option<i64>,
    #[serde(default)]
    pub sent_at: Option<i64>,
    #[serde(default)]
    pub unknown_since: Option<i64>,
    /// The operation this one waits for.
    #[serde(default)]
    pub depends_on: Option<String>,
    /// For an undo: the `op_id` it undoes.
    #[serde(default)]
    pub undoes: Option<String>,
    #[serde(default)]
    pub last_error: Option<OpError>,
    /// What was seen while it was `unknown`, for the user.
    #[serde(default)]
    pub note: Option<String>,
    /// It needs the user: `unknown` for over 24 hours.
    #[serde(default)]
    pub flagged: bool,
    /// What it sends: the Graph fields, so a failed add keeps the task's
    /// content.
    #[serde(default)]
    pub changes: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpError {
    /// An `ms_todo_core::ErrorKind` string.
    pub kind: String,
    pub message: String,
}
