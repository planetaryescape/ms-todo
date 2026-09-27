//! What the daemon pushes to a subscribed client.

use serde::{Deserialize, Serialize};

use crate::{OpError, SyncActivity, SyncProgress};

/// Pushed by the daemon. A client skips events it doesn't know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Progress of the sync a `Sync { wait: true }` request waits for,
    /// sent with that request's message ID.
    SyncProgress(SyncProgress),
    /// Graph rejected an outbox operation for good: it's `failed`, and its
    /// local change was rolled back.
    WriteRejected(WriteRejected),
    /// An edit was made over a change another device made to the same
    /// fields since ms-todo last read the task: last write wins (04,
    /// D-065). The operation is `done`, and its note in `outbox list`
    /// says what it overwrote.
    ConflictOverwritten(ConflictOverwritten),
    /// These lists or tasks (local IDs) changed in the cache: a write, an
    /// outbox operation settling, or a sync. At most [`MAX_CHANGED_IDS`](crate::MAX_CHANGED_IDS).
    EntityChanged(EntityChanged),
    /// More changed than an `EntityChanged` carries, or the subscriber fell
    /// behind and missed events: read everything again.
    ResyncNeeded,
    /// A sync pass started or finished.
    SyncState(SyncActivity),
    /// Semantic search's index changed: its model finished loading, or
    /// tasks were embedded for their new text. A search by meaning shown
    /// now may rank differently (D-061).
    IndexChanged,
    /// How far an attachment's upload session has got: sent as it starts,
    /// after each chunk Graph takes, and once when the attempt ends,
    /// finished or not (`done`).
    UploadProgress(UploadProgress),
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadProgress {
    /// The outbox operation adding the attachment.
    pub op_id: String,
    /// The task's local ID.
    pub task_id: String,
    /// The attachment's name.
    pub name: String,
    /// Bytes Graph has taken so far.
    pub sent: u64,
    pub total: u64,
    /// The attempt is over: `sent == total` if it finished; otherwise it
    /// stopped, and the outbox says why and what happens next.
    #[serde(default)]
    pub done: bool,
}

impl UploadProgress {
    /// How far it's got, from 0 to 100.
    pub fn percent(&self) -> u64 {
        (self.sent.saturating_mul(100))
            .checked_div(self.total)
            .unwrap_or(0)
            .min(100)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityChanged {
    #[serde(default)]
    pub lists: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteRejected {
    pub op_id: String,
    pub task_id: String,
    pub error: OpError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictOverwritten {
    pub op_id: String,
    pub task_id: String,
    /// Graph's names of the fields both changed (`title`, `dueDateTime`).
    pub fields: Vec<String>,
    /// For people: what was overwritten, as the operation's note has it.
    pub message: String,
}
