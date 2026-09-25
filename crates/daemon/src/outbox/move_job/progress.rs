//! A move's saved steps (`outbox.progress_json`). Every step that may
//! change Graph is recorded before it's sent and after it's answered, so
//! a daemon that stops at any point knows on start what may have happened:
//! `in_doubt` is set while a create is on its way, and `delete_started`
//! from the moment the source's DELETE may be sent.

use ms_todo_store::{Entity, OutboxRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    /// Read the source and its attachments; nothing is written yet.
    #[default]
    Prepare,
    /// Create the copy, with its fields and children, in one POST.
    Create,
    /// Add each attachment to the copy.
    Attachments,
    /// Read the copy back and compare it with the source; check the
    /// target list is live.
    Verify,
    /// Delete the source.
    Delete,
    /// Delete the partial copy, then fail the move: every step's outcome
    /// is known and the source was never touched.
    RollBack,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepState {
    #[default]
    Pending,
    /// Sent, and not answered yet.
    Sent,
    Done,
}

/// One of the source's attachments, and its copy's state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttachmentStep {
    pub name: String,
    pub content_type: String,
    /// Its bytes, as downloaded from the source.
    pub bytes: usize,
    pub sha256: String,
    /// The file in the move's spool that holds its bytes.
    pub spool: String,
    pub state: StepState,
}

/// The source as the move found it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Source {
    pub list_graph_id: String,
    /// `None` once the source is known to be gone and the move re-creates
    /// the task from its local copy (`outbox retry` after both went).
    pub graph_id: Option<String>,
    /// Graph's JSON for it, without our extension: the full local copy.
    pub raw: Entity,
    pub extension: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Progress {
    #[serde(default)]
    pub stage: Stage,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub attachments: Vec<AttachmentStep>,
    /// The copy's Graph ID, once its create was answered or attributed.
    #[serde(default)]
    pub copy_id: Option<String>,
    /// A create (the copy, or an attachment) may have reached Graph, and
    /// its answer wasn't recorded.
    #[serde(default)]
    pub in_doubt: bool,
    /// Paused where nothing but the user can settle it (read by the store
    /// for `outbox list`'s flag).
    #[serde(default)]
    pub needs_user: bool,
    /// The source's DELETE may have been sent: from here on the copy is
    /// never deleted (04).
    #[serde(default)]
    pub delete_started: bool,
    /// Both the source and the copy were found gone after the DELETE may
    /// have started.
    #[serde(default)]
    pub both_missing: bool,
    /// Why the move is being rolled back, `(ErrorKind, message)`.
    #[serde(default)]
    pub rollback: Option<(String, String)>,
    /// The copy as verified: what the task is once the move is done, and
    /// what undo checks hasn't changed since.
    #[serde(default)]
    pub copy: Option<Entity>,
    #[serde(default)]
    pub copy_extension: Option<Value>,
}

impl Progress {
    pub fn of(op: &OutboxRow) -> Self {
        op.progress
            .as_ref()
            .and_then(|value| Self::deserialize(value).ok())
            .unwrap_or_default()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// Whether anything may exist on Graph that the move made.
    pub fn wrote_anything(&self) -> bool {
        self.copy_id.is_some() || self.in_doubt
    }
}

/// The local ID of the list a move takes its task from, from its payload;
/// the operation's own list is the one it goes to.
pub(crate) fn from_list(op: &OutboxRow) -> String {
    op.body()["from_list"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}
