//! What a client can ask the daemon for: [`Request`] and the small enums
//! only its commands carry.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CategoryChange, ExtensionChange, ExtensionOwner, ListChange, NewTask, OutboxState, Scope,
    TaskChange, TaskFilter, TaskSelect,
};

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
        /// Only the tasks it matches, in its order. One that narrows,
        /// with no `list`, looks in every list, each task with `list`, its
        /// list's name.
        #[serde(default, skip_serializing_if = "TaskFilter::is_empty")]
        filter: TaskFilter,
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
    /// The user's Outlook categories, from Graph, answered `Categories`.
    ListCategories,
    /// Create, recolour or delete a category, straight to Graph. With
    /// `dry_run`, answers `Plan`; else `Applied` with the category.
    ChangeCategory {
        change: CategoryChange,
        #[serde(default)]
        dry_run: bool,
        /// Chosen by the client before sending (see `AddTask`).
        #[serde(default)]
        op_id: Option<String>,
        #[serde(default)]
        idempotency_key: Option<String>,
    },
    /// A list's or task's open extensions, answered `Extensions`. Graph
    /// lists neither's (S2; a list's is a 404 too), so it's ms-todo's own,
    /// as cached.
    ListExtensions { owner: ExtensionOwner },
    /// One open extension by name, answered `Extensions` with it alone.
    GetExtension { owner: ExtensionOwner, name: String },
    /// Set or delete an open extension, straight to Graph. With
    /// `dry_run`, answers `Plan`; else `Applied`.
    ChangeExtension {
        owner: ExtensionOwner,
        change: ExtensionChange,
        #[serde(default)]
        dry_run: bool,
        #[serde(default)]
        op_id: Option<String>,
        #[serde(default)]
        idempotency_key: Option<String>,
    },
    /// A valid access token, for `auth bearer --reveal-secret`.
    Bearer,
    /// Stop the daemon. It answers `Ack`, then exits.
    Shutdown,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RawWriteMethod {
    Post,
    Patch,
    Delete,
}
