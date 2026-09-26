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
    /// These lists or tasks (local IDs) changed in the cache: a write, an
    /// outbox operation settling, or a sync. At most [`MAX_CHANGED_IDS`](crate::MAX_CHANGED_IDS).
    EntityChanged(EntityChanged),
    /// More changed than an `EntityChanged` carries, or the subscriber fell
    /// behind and missed events: read everything again.
    ResyncNeeded,
    /// A sync pass started or finished.
    SyncState(SyncActivity),
    #[serde(other)]
    Unknown,
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
