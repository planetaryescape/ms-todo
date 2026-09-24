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

/// Bumped on any change an older peer can't read. 5: `Seed` and
/// `Subscribe`, which the TUI needs from its first request, and the events
/// a subscription streams.
pub const PROTOCOL_VERSION: u32 = 5;

/// The socket buffer both ends ask for: room for a large list's `Seed` in
/// one write. macOS gives a Unix socket 8 KiB, so a 350 KiB seed crossed
/// in 45 wake-ups of both processes, which cost more than the query.
pub const SOCKET_BUFFER_BYTES: usize = 1024 * 1024;

/// The most IDs one `EntityChanged` carries. A change to more is sent as
/// `ResyncNeeded` instead: past this, reading the view again is cheaper
/// than patching it.
pub const MAX_CHANGED_IDS: usize = 500;

/// The daemon's exit status when its database was upgraded by a newer
/// ms-todo (a migration this build doesn't know). The client that started
/// it reports `database_too_new` instead of the daemon's log (78 is
/// sysexits' `EX_CONFIG`).
pub const EXIT_DATABASE_TOO_NEW: u8 = 78;

/// A list or task: Graph's JSON with every field kept, except that `id` is
/// ms-todo's local ID, with Graph's beside it as `graph_id`, and
/// `sync_state` says whether it's in step with Graph
/// (docs/blueprint/07-cli.md#output-contract). A task also has `list_id`,
/// its list's local ID.
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
    /// Lists from the cache.
    ListLists,
    /// Tasks from the cache, of the list named or identified by `list`, or
    /// of the default list ("Tasks") when `None`.
    ListTasks {
        #[serde(default)]
        list: Option<String>,
        /// Only tasks matching this search (see `SearchTasks`), best match
        /// first.
        #[serde(default)]
        search: Option<String>,
    },
    /// Tasks whose title or notes match `query`, best match first, from the
    /// cache. `query` is FTS5's syntax: words (all must match), `"phrases"`,
    /// `prefix*`, `AND`, `OR`, `NOT` and parentheses.
    SearchTasks {
        query: String,
        /// Only this list (a name or ID); `None` is every list.
        #[serde(default)]
        list: Option<String>,
        #[serde(default)]
        status: SearchStatus,
        /// At most this many; `None` is every match.
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Refresh the cache from Graph. With `wait`, the daemon sends
    /// `SyncProgress` events while it works and answers when a pass that
    /// started after this request has finished.
    Sync {
        #[serde(default)]
        wait: bool,
    },
    /// The daemon's view of its own health, for `ms-todo doctor`.
    Doctor,
    /// An authenticated GET of a path under the Graph v1.0 root.
    RawGet { path: String },
    /// A synchronous POST, PATCH or DELETE of a path under the Graph v1.0
    /// root. Never resent after it may have reached Graph.
    RawWrite {
        method: RawWriteMethod,
        path: String,
        #[serde(default)]
        body: Option<Value>,
        /// Chosen by the client before sending (see `AddTask`).
        #[serde(default)]
        op_id: Option<String>,
    },
    /// Create a task. With `dry_run`, answers `Plan` and writes nothing.
    AddTask {
        task: NewTask,
        #[serde(default)]
        dry_run: bool,
        /// Chosen by the client before sending, so it can report it even if
        /// the answer is lost; the daemon makes one when it's missing.
        #[serde(default)]
        op_id: Option<String>,
        /// `--idempotency-key`: a repeat of the same request with the same
        /// key gets the first one's result.
        #[serde(default)]
        idempotency_key: Option<String>,
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
        /// Chosen by the client before sending (see `AddTask`).
        #[serde(default)]
        op_id: Option<String>,
        /// See `AddTask`.
        #[serde(default)]
        idempotency_key: Option<String>,
    },
    /// The outbox's operations, newest first, optionally only those in
    /// `state`.
    OutboxList {
        #[serde(default)]
        state: Option<OutboxState>,
    },
    /// Send an `unknown` or `failed` operation again (a resend the user
    /// chose). `conflict` if its state changed meanwhile.
    OutboxRetry { op_id: String },
    /// Drop an operation that isn't `done` or `inflight`, undoing its local
    /// change where it never reached Graph.
    OutboxDiscard { op_id: String },
    /// Queue the inverse of `target` (an `op_id` from a mutation; `None`
    /// is the latest one not undone yet). Undoing a recurring completion
    /// needs `copy`, the completed copy to delete.
    Undo {
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        copy: Option<String>,
        /// The undo's own `op_id`, chosen by the client (see `AddTask`).
        #[serde(default)]
        op_id: Option<String>,
        #[serde(default)]
        idempotency_key: Option<String>,
    },
    /// Everything a client needs to draw its first screen, from the cache
    /// (spotuify's `ClientSeed`): the lists, the smart views' and lists'
    /// counts, and the tasks of `scope` (the default list when `None`),
    /// only those matching `search` when given (see `SearchTasks`).
    Seed {
        #[serde(default)]
        scope: Option<Scope>,
        #[serde(default)]
        search: Option<String>,
    },
    /// Stream events on this connection from now on: `EntityChanged`,
    /// `ResyncNeeded`, `SyncState` and `WriteRejected`, each with this
    /// request's message ID. Answered `Ack`, then a `SyncState`. The
    /// connection still takes requests.
    Subscribe,
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

impl From<Result<ResponseData, ErrorPayload>> for Response {
    fn from(result: Result<ResponseData, ErrorPayload>) -> Self {
        match result {
            Ok(data) => Self::Ok { data },
            Err(error) => Self::Error { error },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseData {
    Status(DaemonStatus),
    Lists {
        items: Vec<Entity>,
        sync: SyncInfo,
    },
    Tasks {
        items: Vec<Entity>,
        sync: SyncInfo,
    },
    /// Tasks a search matched, best first: each task entity with `list`,
    /// its list's name, and `snippet`, the passage that matched on one
    /// line with each match between `**`s.
    SearchResults {
        items: Vec<Entity>,
        sync: SyncInfo,
    },
    Sync(SyncReport),
    Doctor(DoctorReport),
    Raw {
        body: Value,
    },
    /// What a mutation would do, from a dry run.
    Plan(Plan),
    /// What a mutation did.
    Applied(Applied),
    /// Outbox operations, newest first.
    Outbox {
        items: Vec<OutboxOp>,
    },
    /// The operation `outbox retry` or `outbox discard` acted on, as it is
    /// now (as it was, for a discard).
    OutboxOp(OutboxOp),
    Bearer {
        access_token: String,
        /// Unix seconds.
        expires_at: i64,
    },
    Seed(Seed),
    Ack,
    #[serde(other)]
    Unknown,
}

/// What a client shows: a smart view over every list, or one list.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum Scope {
    /// Open tasks marked important.
    Important,
    /// Open tasks with a due date, soonest first.
    Planned,
    /// Every open task.
    All,
    /// Every completed task, most recently completed first.
    Completed,
    /// One list's tasks, open and completed, by its local ID or name.
    List { id: String },
    #[serde(other)]
    Unknown,
}

/// The answer to `Seed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seed {
    /// The scope answered, a list always by its local ID. `None` only for
    /// the default list before the lists have synced once, when nobody
    /// knows which list that is yet.
    pub scope: Option<Scope>,
    /// Every live list, as `ListLists` gives them.
    pub lists: Vec<Entity>,
    /// The lists' own sync state.
    pub lists_sync: SyncInfo,
    pub counts: Counts,
    /// The scope's tasks, as `ListTasks` gives them; best match first with
    /// `search`.
    pub tasks: Vec<Entity>,
    /// The scope's sync state: `initial` until it has synced once, so an
    /// empty `tasks` isn't mistaken for an empty list.
    pub sync: SyncInfo,
    pub activity: SyncActivity,
    pub outbox: OutboxDepth,
}

/// How many tasks each smart view and each list holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub important: u64,
    pub planned: u64,
    pub all: u64,
    pub completed: u64,
    /// Open tasks by list local ID; a list with none is absent.
    #[serde(default)]
    pub lists: std::collections::BTreeMap<String, u64>,
}

/// Whether the daemon is syncing, and how its last pass went.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncActivity {
    /// Sync passes finished since the daemon started; goes up by one per
    /// pass, even one that changed nothing.
    pub generation: u64,
    pub in_progress: bool,
    /// Unix seconds: when the last pass finished.
    #[serde(default)]
    pub last_finished_at: Option<i64>,
    /// Why the last pass failed, if it did.
    #[serde(default)]
    pub last_error: Option<OpError>,
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

/// Whether a scope's cache has been filled yet. `initial` until its first
/// sync finishes, so an empty result isn't mistaken for an empty list
/// (docs/blueprint/07-cli.md#output-contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Initial,
    Ready,
}

/// Which tasks a search considers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStatus {
    /// Not completed.
    #[default]
    Open,
    Completed,
    All,
}

/// The sync state of the scope a collection came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncInfo {
    pub state: SyncState,
    /// Goes up by one each time a sync of the scope finishes.
    pub generation: u64,
}

/// What `sync` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// False when a sync was only asked for (no `--wait`).
    pub waited: bool,
    /// Scopes the pass finished: the lists, and each list's tasks.
    pub scopes: u32,
    /// Lists and tasks the pass added, changed or removed.
    pub changed: u64,
    /// The `lists` scope's generation afterwards.
    pub generation: u64,
}

/// Sent while a sync runs, to a client waiting for it. Each one is
/// progress, so it resets the client's stall deadline.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncProgress {
    pub scopes_done: u32,
    /// Zero until the lists are known.
    pub scopes_total: u32,
    /// What it's doing, for people.
    pub doing: String,
}

/// The daemon's side of `ms-todo doctor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub database_path: String,
    pub database_bytes: u64,
    /// Whether a sync pass is running now.
    pub syncing: bool,
    pub scopes: Vec<ScopeStatus>,
    #[serde(default)]
    pub outbox: OutboxDepth,
}

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeStatus {
    /// `lists`, or `tasks:<list graph ID>`.
    pub scope: String,
    /// For a tasks scope, its list's local ID and name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_name: Option<String>,
    pub state: SyncState,
    pub generation: u64,
    pub in_progress: bool,
    /// Unix seconds.
    pub last_success_at: Option<i64>,
    pub last_changed_count: u64,
    pub last_error: Option<ScopeError>,
    #[serde(default)]
    pub mode: SyncMode,
    /// Unix seconds: when a delta round of this scope last checkpointed.
    #[serde(default)]
    pub last_delta_at: Option<i64>,
}

/// How a scope's next pass reads it (docs/blueprint/04-sync-cache.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// No delta link: the next pass reads the whole scope and reconciles.
    #[default]
    Enumeration,
    /// The next pass replays the saved delta link, for changes only.
    Delta,
    #[serde(other)]
    Unknown,
}

/// Why a scope's last sync failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeError {
    /// An `ms_todo_core::ErrorKind` string.
    pub kind: String,
    pub message: String,
    /// Unix seconds.
    pub at: Option<i64>,
}

/// A list or task by its local ID and display name (a task's title).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    /// For a task: when Graph created it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// For a task: its list's local ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_id: Option<String>,
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
    Undo,
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

/// A task a plan changes, by local IDs.
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
    /// Each task as Graph returned it after the change, in the entity shape
    /// (local `id`, `graph_id`). For `delete`, as it was last read.
    pub items: Vec<Entity>,
    /// The local ID of the list each of `items` is in, in the same order.
    #[serde(default)]
    pub list_ids: Vec<String>,
    /// Recurring tasks this completed: Graph kept the task, moved its due
    /// date on, and made a completed copy with a new ID (S12).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rolled: Vec<Rolled>,
    /// For an undo: the `op_id` it undoes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undoes: Option<String>,
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
    /// For an undo that needs `copy`: the `op_id` it would undo, so the
    /// retry with the chosen copy undoes that one and no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_target: Option<String>,
}

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
    /// outbox operation settling, or a sync. At most [`MAX_CHANGED_IDS`].
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
