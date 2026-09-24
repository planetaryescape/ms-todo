//! The IPC protocol between ms-todo's clients and its daemon
//! (docs/blueprint/01-architecture.md#transport): length-delimited JSON over
//! a Unix socket, each frame a [`Message`] envelope `{ id, payload }`.
//!
//! Compatibility rules, so an older client and a newer daemon (or the other
//! way round) fail clearly instead of mis-reading each other:
//!
//! - Every tagged enum has an `Unknown` variant that a tag this build doesn't
//!   know decodes to.
//! - Fields added after rung 1 carry `#[serde(default)]`.
//! - A change that can't follow those rules bumps [`PROTOCOL_VERSION`].
//!   Clients check it through `Status` before anything else, and restart
//!   the daemon when it differs.

mod codec;

pub use codec::{Codec, FrameTooLarge, MAX_FRAME_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Bumped on any change an older peer can't read.
pub const PROTOCOL_VERSION: u32 = 1;

/// A JSON object from Graph, every field kept. Its `id` is the Graph ID in
/// rungs 1 and 2 (docs/blueprint/07-cli.md#output-contract).
pub type Entity = Map<String, Value>;

/// One frame. A client picks `id`, and the daemon echoes it on the response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Payload {
    Request(Request),
    Response(Response),
    Event(Event),
    #[serde(other)]
    Unknown,
}

/// What a client can ask the daemon for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Readiness and version check; answers even while signed out.
    Status,
    ListLists,
    /// Tasks of the list named or identified by `list`, or of the default
    /// list ("Tasks") when `None`.
    ListTasks {
        #[serde(default)]
        list: Option<String>,
    },
    /// An authenticated GET of a path under the Graph v1.0 root.
    RawGet {
        path: String,
    },
    /// A synchronous POST, PATCH or DELETE of a path under the Graph v1.0
    /// root. Never resent after it may have reached Graph.
    RawWrite {
        method: RawWriteMethod,
        path: String,
        #[serde(default)]
        body: Option<Value>,
    },
    /// Create a task. With `dry_run`, answers `Plan` and writes nothing.
    AddTask {
        task: NewTask,
        #[serde(default)]
        dry_run: bool,
    },
    /// Apply one change to each task in `tasks`: Graph IDs, or with `list`,
    /// IDs or exact titles within that list. With `dry_run`, answers `Plan`
    /// and writes nothing.
    ChangeTasks {
        tasks: Vec<String>,
        #[serde(default)]
        list: Option<String>,
        change: TaskChange,
        #[serde(default)]
        dry_run: bool,
    },
    /// A valid access token, for `auth bearer --reveal-secret`.
    Bearer,
    /// Stop the daemon. It answers `Ack`, then exits.
    Shutdown,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok {
        data: ResponseData,
    },
    Error {
        error: ErrorPayload,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseData {
    Status(DaemonStatus),
    Lists {
        items: Vec<Entity>,
    },
    Tasks {
        items: Vec<Entity>,
    },
    Raw {
        body: Value,
    },
    /// What a mutation would do, from a dry run.
    Plan(Plan),
    /// What a mutation did.
    Applied(Applied),
    Bearer {
        access_token: String,
        /// Unix seconds.
        expires_at: i64,
    },
    Ack,
    #[serde(other)]
    Unknown,
}

/// What `Status` reports. Ready means this answers with a compatible
/// `protocol_version` (vault: `Daemon Readiness Is Not Process Liveness`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub protocol_version: u32,
    /// The daemon's package version.
    pub version: String,
    pub pid: u32,
    pub instance: String,
    /// Unix seconds.
    pub started_at: i64,
    /// Whether a credential is stored. It may still be revoked.
    #[serde(default)]
    pub signed_in: bool,
}

/// A list or task by its ID and display name (a task's title).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RawWriteMethod {
    Post,
    Patch,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    Low,
    Normal,
    High,
}

/// A task to create. The title is taken literally; dates are validated by
/// the daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    /// A list name or ID; `None` is the "Tasks" list (D-022).
    #[serde(default)]
    pub list: Option<String>,
    /// `YYYY-MM-DD`. Due dates are dates only (D-027).
    #[serde(default)]
    pub due: Option<String>,
    /// `YYYY-MM-DDTHH:MM`, local time.
    #[serde(default)]
    pub reminder: Option<String>,
    #[serde(default)]
    pub importance: Option<Importance>,
    /// Plain-text notes.
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TaskChange {
    Complete,
    Reopen,
    Delete,
    Edit(TaskEdit),
    #[serde(other)]
    Unknown,
}

/// The fields `tasks edit` changes; `None` leaves a field alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdit {
    #[serde(default)]
    pub title: Option<String>,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub due: Option<Clearable<String>>,
    #[serde(default)]
    pub importance: Option<Importance>,
    /// `YYYY-MM-DDTHH:MM`, local time.
    #[serde(default)]
    pub reminder: Option<Clearable<String>>,
    #[serde(default)]
    pub body: Option<String>,
}

/// A field an edit either sets or clears.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clearable<T> {
    Set(T),
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Add,
    Complete,
    Reopen,
    Edit,
    Delete,
    #[serde(other)]
    Unknown,
}

/// A mutation's typed plan: the verb and its resolved targets. A dry run
/// renders it and the real run applies it, so the preview is what runs
/// (docs/blueprint/07-cli.md#global-flags).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub action: TaskAction,
    /// The list a task is added to. Empty for other actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<Candidate>,
    /// The tasks changed, in order. Empty for `add`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<PlannedTask>,
    /// The Graph fields each target gets (the POST body for `add`, without
    /// the `opId` extension a real run adds). Null for `delete`.
    #[serde(default)]
    pub changes: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTask {
    pub id: String,
    pub title: String,
    pub list_id: String,
}

/// What a mutation did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// The create's `opId`, or a fresh UUID for other actions.
    pub op_id: String,
    pub action: TaskAction,
    /// Each task as Graph returned it after the change. For `delete`, as it
    /// was last read.
    pub items: Vec<Entity>,
    /// The list each of `items` is in, in the same order.
    #[serde(default)]
    pub list_ids: Vec<String>,
    /// Recurring tasks this completed: Graph kept the task, moved its due
    /// date on, and made a completed copy with a new ID (S12).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rolled: Vec<Rolled>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rolled {
    pub id: String,
    /// `YYYY-MM-DD`, local.
    pub next_due: String,
}

/// A failed request. `kind` is an `ms_todo_core::ErrorKind` string; a kind
/// the client doesn't know is treated as `internal`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The lists or tasks an ambiguous name matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    /// The mutation's `op_id`, on a failed or `outcome_unknown` mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<String>,
    /// Tasks a multi-task mutation changed before it failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied: Vec<String>,
}

/// Pushed by the daemon. Rung 1 sends none; the type exists so a client
/// built now skips the events later rungs add.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    #[serde(other)]
    Unknown,
}
