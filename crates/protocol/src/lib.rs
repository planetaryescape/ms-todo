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
/// a subscription streams. 6: folders (`ListFolders`, `ChangeLists`), and
/// lists in folder order, so a client restarts an older daemon rather
/// than show lists ungrouped. 7: `CompletedTasks`, `ChangeTasks.select`
/// and `Applied.refused` (rung 5d), so an older daemon never reads a bulk
/// change without its selection as a change to no task. 8: `GetTasks`,
/// for `tasks links` and `tasks open`. 9: moving tasks between lists
/// (`TaskChange::Move`, rung 5e), so a client restarts an older daemon
/// rather than have a move refused as unknown. 10: `NewTask.start`,
/// `recurrence` and `categories` (quick add, rung 6a), so an older daemon
/// never drops a recurrence it doesn't know and creates a one-off task.
/// 11: `SuggestList` (rung 6b), so a client restarts an older daemon
/// rather than have the request refused as unknown. 12: My Day (rung 7):
/// `Scope::MyDay`, `TaskChange::AddToMyDay` and `RemoveFromMyDay`,
/// `NewTask.my_day`, `MyDay`, `MyDayRollover` and `Counts.my_day`, so an
/// older daemon never adds a task without putting it in My Day. 13: steps
/// and links (rung 8a): `TaskChange::AddSteps`, `EditStep`, `CheckSteps`,
/// `DeleteSteps`, `AddLink`, `EditLink` and `DeleteLink`, and their
/// `TaskAction`s, so a client restarts an older daemon rather than have
/// them refused as unknown. 14: attachments (rung 8b):
/// `TaskChange::AddAttachments` and `DeleteAttachments`, their
/// `TaskAction`s, and `DownloadAttachments`, so a client restarts an older
/// daemon rather than have them refused as unknown. 15: assignment (rung
/// 8d): `NewTask.assignee`, `TaskEdit.assignee`, `ListTasks.assignee`,
/// `Scope::Assigned` and `Counts.assigned`, so an older daemon never
/// drops an assignee it doesn't know and makes the change without it.
pub const PROTOCOL_VERSION: u32 = 15;

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
    /// Lists from the cache, in the sidebar's order: each folder's lists,
    /// folder by folder, then the lists in no folder. Each list has
    /// `folder`, its folder's name or null.
    ListLists,
    /// The folders, in order, from the cache.
    ListFolders,
    /// The tasks named, from the cache, as `Tasks`: IDs, or with `list`,
    /// IDs or exact titles in that list, resolved as `ChangeTasks` does.
    GetTasks {
        tasks: Vec<String>,
        #[serde(default)]
        list: Option<String>,
    },
    /// Tasks from the cache, of the list named or identified by `list`, or
    /// of the default list ("Tasks") when `None`.
    ListTasks {
        #[serde(default)]
        list: Option<String>,
        /// Only tasks matching this search (see `SearchTasks`), best match
        /// first.
        #[serde(default)]
        search: Option<String>,
        /// Only tasks assigned to this person (a case-insensitive exact
        /// match), or to anyone for `*`. With no `list`, it's the open
        /// tasks of every list, grouped by person, as the Assigned view
        /// has them, each with `list`, its list's name.
        #[serde(default)]
        assignee: Option<String>,
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
    /// Completed tasks from the cache, newest completion first, each with
    /// `completed_on` (its local day, or null while the completion hasn't
    /// reached Microsoft To Do) and `list`, its list's name. Only those
    /// completed from `since` to `until` (`YYYY-MM-DD`, local, both
    /// included; `until` open-ended when `None`), in `list` or `folder`
    /// (every list when neither).
    CompletedTasks {
        since: String,
        #[serde(default)]
        until: Option<String>,
        #[serde(default)]
        list: Option<String>,
        #[serde(default)]
        folder: Option<String>,
        /// At most this many; `None` is all of them.
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
    /// IDs or exact titles within that list. Or, with `select` and no
    /// `tasks`, to the open tasks it matches (in `list` when given), found
    /// when the change runs. With `dry_run`, answers `Plan` and writes
    /// nothing.
    ChangeTasks {
        tasks: Vec<String>,
        #[serde(default)]
        list: Option<String>,
        #[serde(default)]
        select: Option<TaskSelect>,
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
    /// Change lists' folders or order, or rename, delete or reorder a
    /// folder (docs/blueprint/05-custom-features.md#folders-list-groups):
    /// one extension write per list changed, through the outbox. With
    /// `dry_run`, answers `Plan` and writes nothing.
    ChangeLists {
        change: ListChange,
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
    /// Which list a task with this title might belong in, from the
    /// optional suggestion provider (rung 6b), answered `ListSuggestion`.
    /// Only a suggestion: nothing is filed. An error when suggestions are
    /// off; a failure to reach the provider is no suggestion, not an
    /// error. Answered out of order, so a slow provider never holds up
    /// the connection's other requests.
    SuggestList { title: String },
    /// My Day from the cache, answered `MyDay`: today's tasks (by My
    /// Day's day, which starts at `my_day.rollover_time`) and the
    /// suggestions for it.
    MyDay,
    /// Run the My Day rollover now: every task in an earlier day's My Day
    /// leaves it, and loses a due date ms-todo set for it if it's still
    /// open (docs/blueprint/05-custom-features.md#my-day). With `dry_run`,
    /// answers `Plan` and writes nothing; otherwise `Applied`, one outbox
    /// operation or two per task under one `op_id`.
    MyDayRollover {
        #[serde(default)]
        dry_run: bool,
        /// Chosen by the client before sending (see `AddTask`).
        #[serde(default)]
        op_id: Option<String>,
    },
    /// Download attachments of the one task named (as `GetTasks` names
    /// it) into the directory `out_dir`, an absolute path: each named
    /// attachment (its number from 1, ID or exact name), or every one
    /// when `attachments` is empty. The daemon writes the files; no bytes
    /// cross the socket. Each file gets a safe name in `out_dir`, made
    /// unique (`name (1).pdf`) unless `force` lets it replace a file there.
    /// Answered `Downloaded`, out of order, like `SuggestList`.
    DownloadAttachments {
        task: String,
        #[serde(default)]
        list: Option<String>,
        #[serde(default)]
        attachments: Vec<String>,
        out_dir: String,
        #[serde(default)]
        force: bool,
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

impl Request {
    /// Whether the daemon may answer this after requests sent later on
    /// the same connection: slow, read-only, and needing nothing in order.
    pub fn answered_out_of_order(&self) -> bool {
        matches!(
            self,
            Self::SuggestList { .. } | Self::DownloadAttachments { .. }
        )
    }
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
    Folders {
        items: Vec<Folder>,
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
    /// The answer to `SuggestList`: `None` when no list is likely enough.
    ListSuggestion {
        suggestion: Option<ListSuggestion>,
    },
    MyDay(MyDay),
    /// The files `DownloadAttachments` wrote, in order.
    Downloaded {
        /// The task's local ID.
        task_id: String,
        files: Vec<DownloadedFile>,
    },
    Ack,
    #[serde(other)]
    Unknown,
}

/// An attachment written to disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadedFile {
    /// The attachment's ID.
    pub id: String,
    /// Its name, as Microsoft To Do has it.
    pub name: String,
    /// Where it was written: an absolute path.
    pub path: String,
    /// How many bytes were written.
    pub bytes: u64,
    /// The bytes' sha256, as hex.
    pub sha256: String,
}

/// A list a task might belong in, and how sure the provider is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListSuggestion {
    /// The list's local ID.
    pub list_id: String,
    pub list_name: String,
    /// From 0 to 1; at least the configured `min_confidence`.
    pub confidence: f64,
}

/// My Day: today's tasks and what's suggested for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MyDay {
    /// My Day's day, `YYYY-MM-DD`: today, or yesterday before
    /// `my_day.rollover_time`.
    pub date: String,
    /// The tasks in today's My Day, open ones first.
    pub tasks: Vec<Entity>,
    /// Open tasks not in it that could be: each has `suggestion`, why
    /// (`due_today`, `overdue` or `left_over`, from an earlier My Day the
    /// rollover took it out of), and `list`, its list's name.
    pub suggestions: Vec<Entity>,
    /// Every list's sync state, as a view over every list has it.
    pub sync: SyncInfo,
    /// The day the last rollover ran for, `YYYY-MM-DD`.
    #[serde(default)]
    pub last_rollover: Option<String>,
}

/// What a client shows: a smart view over every list, or one list.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum Scope {
    /// The tasks in today's My Day, open ones first.
    MyDay,
    /// Open tasks marked important.
    Important,
    /// Open tasks with a due date, soonest first.
    Planned,
    /// Every open task.
    All,
    /// Every completed task, most recently completed first.
    Completed,
    /// Open tasks with an assignee, grouped by person.
    Assigned,
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
    /// For `Scope::MyDay`: its day and suggestions; `None` for any other
    /// scope.
    #[serde(default)]
    pub my_day: Option<MyDaySeed>,
}

/// What the My Day view shows besides its tasks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MyDaySeed {
    /// `YYYY-MM-DD`, as `MyDay.date`.
    pub date: String,
    /// As `MyDay.suggestions`.
    pub suggestions: Vec<Entity>,
}

/// How many tasks each smart view and each list holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Open tasks in today's My Day.
    #[serde(default)]
    pub my_day: u64,
    pub important: u64,
    pub planned: u64,
    pub all: u64,
    pub completed: u64,
    /// Open tasks with an assignee.
    #[serde(default)]
    pub assigned: u64,
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
    /// List suggestions (rung 6b); `None` from a daemon before them.
    #[serde(default)]
    pub suggest: Option<SuggestStatus>,
    /// My Day (rung 7); `None` from a daemon before it.
    #[serde(default)]
    pub my_day: Option<MyDayStatus>,
}

/// How My Day stands, for `doctor`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyDayStatus {
    /// My Day's day now, `YYYY-MM-DD`.
    pub date: String,
    /// Open tasks in it.
    pub count: u64,
    /// `HH:MM`, local: when a day's My Day ends.
    pub rollover_time: String,
    /// The day the last rollover ran for.
    #[serde(default)]
    pub last_rollover: Option<String>,
    /// Why `my_day.rollover_time` in config.toml wasn't used.
    #[serde(default)]
    pub problem: Option<String>,
}

/// How list suggestions stand, for `doctor`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestStatus {
    pub enabled: bool,
    /// Who suggests, when enabled: `typesafe`.
    #[serde(default)]
    pub provider: Option<String>,
    /// Why suggestions are off or failing: a bad `[suggest]` setting, or
    /// the last failure since the last suggestion that worked.
    #[serde(default)]
    pub problem: Option<String>,
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// `YYYY-MM-DD`. Graph sets the due date to it too when there's none
    /// (S11).
    #[serde(default)]
    pub start: Option<String>,
    /// Graph's `patternedRecurrence`, less `range.recurrenceTimeZone`,
    /// which the daemon sets to the zone it writes the due date in (S12).
    /// `range.startDate` is the first due date.
    #[serde(default)]
    pub recurrence: Option<Value>,
    /// Outlook category names, as the task carries them.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Put it in today's My Day; with no due date, it's due today too, so
    /// the phone shows it in its own My Day (D-037).
    #[serde(default)]
    pub my_day: bool,
    /// Who the task waits on: ms-todo's `assignee` (free text or an
    /// email), which means nothing to Microsoft To Do. Unless
    /// `keep_status`, the task is made `waitingOnOthers` too.
    #[serde(default)]
    pub assignee: Option<String>,
    /// With `assignee`: leave the status as it would be.
    #[serde(default)]
    pub keep_status: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TaskChange {
    Complete,
    Reopen,
    Delete,
    Edit(TaskEdit),
    /// Move the tasks to the list `to` (a name or an ID): each is copied
    /// there with everything it holds, the copy checked, then the original
    /// deleted (docs/blueprint/05-custom-features.md#move-between-lists).
    Move {
        to: String,
    },
    /// Put the tasks in today's My Day; one with no due date is due today
    /// too, which ms-todo takes away again at the rollover (D-037).
    AddToMyDay,
    /// Take the tasks out of My Day, and an open one's due date with it
    /// when ms-todo set that date and nobody has changed it since.
    RemoveFromMyDay,
    /// Add a step (checklist item) to the one task named for each text,
    /// in order, unchecked.
    AddSteps {
        steps: Vec<String>,
    },
    /// Rename a step of the one task named. `step` is its number from 1
    /// as shown, its ID, or its exact text.
    EditStep {
        step: String,
        text: String,
    },
    /// Check the steps named (as `EditStep` names one), or uncheck them.
    CheckSteps {
        steps: Vec<String>,
        checked: bool,
    },
    /// Delete the steps named (as `EditStep` names one).
    DeleteSteps {
        steps: Vec<String>,
    },
    /// Give the one task named its link (linked resource). Graph allows
    /// one per task (S14), so a task with one already is refused.
    AddLink(NewLink),
    /// Change fields of the task's link. Graph keeps a field left out and
    /// can't clear one (S15).
    EditLink(LinkEdit),
    /// Delete the task's link.
    DeleteLink {
        /// Its number from 1 or its ID; `None` is the task's only link.
        #[serde(default)]
        link: Option<String>,
    },
    /// Attach files to the one task named, in order. Each is an absolute
    /// path the daemon reads when it sends it: no bytes cross the socket.
    AddAttachments {
        files: Vec<String>,
    },
    /// Delete attachments of the one task named: each by its number from
    /// 1, its ID or its exact name. The daemon keeps a copy of each first,
    /// for `undo`, and deletes nothing if it can't, unless `no_undo`.
    DeleteAttachments {
        attachments: Vec<String>,
        #[serde(default)]
        no_undo: bool,
    },
    #[serde(other)]
    Unknown,
}

/// A link to give a task: Graph's `linkedResource`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewLink {
    /// `webUrl`. Graph takes any URL; only http, https and mailto open.
    pub url: String,
    /// `displayName`.
    #[serde(default)]
    pub name: Option<String>,
    /// `applicationName`, which Graph requires; `ms-todo` when `None`.
    #[serde(default)]
    pub app: Option<String>,
    /// `externalId`: the item's ID in the app it came from.
    #[serde(default)]
    pub external_id: Option<String>,
}

/// The fields of a link to change; `None` leaves one alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkEdit {
    /// Its number from 1 or its ID; `None` is the task's only link.
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
}

/// Which open tasks a bulk change means, in place of naming them
/// (`--overdue`, `--due-before`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSelect {
    /// Open tasks due before this day, `YYYY-MM-DD` (local); `--overdue`
    /// is before today.
    pub due_before: String,
    /// Only tasks in this folder's lists.
    #[serde(default)]
    pub folder: Option<String>,
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
    /// Who the task waits on (`NewTask.assignee`). Setting one makes an
    /// open task `waitingOnOthers`; clearing it makes a task ms-todo made
    /// `waitingOnOthers` `notStarted` again, unless its status has
    /// changed since. `keep_status` leaves the status alone either way.
    #[serde(default)]
    pub assignee: Option<Clearable<String>>,
    #[serde(default)]
    pub keep_status: bool,
}

/// A field an edit either sets or clears.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clearable<T> {
    Set(T),
    Clear,
}

/// What a mutation did, for tasks or, since protocol 6, lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Add,
    Complete,
    Reopen,
    Edit,
    Delete,
    Undo,
    /// Tasks to another list.
    Move,
    /// Lists into a folder, or out of any.
    MoveList,
    /// A list before or after another in its folder.
    OrderList,
    RenameFolder,
    /// A folder emptied: its lists stay, in no folder.
    DeleteFolder,
    /// A folder before or after another.
    OrderFolder,
    /// Tasks into today's My Day.
    MyDayAdd,
    /// Tasks out of My Day.
    MyDayRemove,
    /// An earlier day's My Day emptied.
    MyDayRollover,
    StepAdd,
    StepEdit,
    StepCheck,
    StepUncheck,
    StepDelete,
    LinkAdd,
    LinkEdit,
    LinkDelete,
    AttachmentAdd,
    AttachmentDelete,
    #[serde(other)]
    Unknown,
}

/// A folder of lists (docs/blueprint/05-custom-features.md#folders-list-groups).
/// It exists only as a name its lists carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    /// Its lists' local IDs, in order.
    pub lists: Vec<String>,
    /// Open tasks in all its lists.
    pub open_count: u64,
}

/// A change to lists' folders or order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ListChange {
    /// Put `lists` (names or IDs) in the folder `folder`, made if it's
    /// new, or in no folder for `None`. Each goes last in its folder.
    MoveList {
        lists: Vec<String>,
        #[serde(default)]
        folder: Option<String>,
    },
    /// Put `list` next to `anchor`, a list in the same folder (or both in
    /// none).
    OrderList { list: String, anchor: Anchor },
    /// Rename the folder `folder` (its lists all move) to `name`, which no
    /// other folder has.
    RenameFolder { folder: String, name: String },
    /// Take every list out of the folder `folder`; no list is deleted.
    DeleteFolder { folder: String },
    /// Put the folder `folder` next to the folder in `anchor`.
    OrderFolder { folder: String, anchor: Anchor },
    #[serde(other)]
    Unknown,
}

/// Where a list or folder goes: just before or just after this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Before(String),
    After(String),
}

/// A list a plan changes, and the extension fields it gets (a null
/// removes one).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedList {
    pub id: String,
    pub name: String,
    pub changes: Value,
}

/// A mutation's typed plan: the verb and its resolved targets. A dry run
/// renders it and the real run applies it, so the preview is what runs
/// (docs/blueprint/07-cli.md#global-flags).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub action: TaskAction,
    /// The list a task is added to, or tasks are moved to. Empty for other
    /// actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<Candidate>,
    /// The tasks changed, in order. Empty for `add`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<PlannedTask>,
    /// For a change to lists: each list changed, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<PlannedList>,
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
    /// (local `id`, `graph_id`). For `delete`, as it was last read. For a
    /// change to lists, each list as it is now.
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
    /// For an undo of a change to several tasks: the tasks it left alone,
    /// because a field the change set has changed since.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<Refused>,
}

/// A task an undo left alone, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    /// The task's local ID.
    pub id: String,
    pub title: String,
    pub reason: String,
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
