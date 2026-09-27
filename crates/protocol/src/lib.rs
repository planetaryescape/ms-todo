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

mod catalog;
mod codec;
mod contexts;
mod events;
mod list_change;
mod mutation;
mod outbox;
mod request;
mod response;
mod status;
mod task_change;
mod task_filter;
mod views;

pub use catalog::{CategoryChange, ExtensionChange, ExtensionOwner, OwnerKind};
pub use codec::{Codec, FrameTooLarge, MAX_FRAME_BYTES};
pub use contexts::{AppliedContext, ContextChoice, ContextInfo, Contexts};
pub use events::{EntityChanged, Event, WriteRejected};
pub use list_change::{Anchor, Folder, ListChange};
pub use mutation::{Applied, Plan, PlannedList, PlannedTask, Refused, Rolled, TaskAction};
pub use outbox::{OpError, OutboxDepth, OutboxOp, OutboxState};
pub use request::{RawWriteMethod, Request, SearchStatus};
pub use response::{
    Candidate, DownloadedFile, ErrorPayload, ListSuggestion, Response, ResponseData, SemanticIndex,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
pub use status::{
    ContextsStatus, DaemonStatus, DoctorReport, ModelState, MyDayStatus, NagStatus, ScopeError,
    ScopeStatus, SemanticStatus, SuggestStatus, SyncActivity, SyncInfo, SyncMode, SyncProgress,
    SyncReport, SyncState,
};
pub use task_change::{
    Clearable, Importance, LinkEdit, NAG_MAX_MINUTES, NAG_MIN_MINUTES, NewLink, NewTask,
    TaskChange, TaskEdit, TaskSelect,
};
pub use task_filter::{DeferredFilter, DueFilter, StatusFilter, TaskFilter, TaskSort};
pub use views::{Counts, MyDay, MyDaySeed, Scope, Seed};

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
/// 16: the rest of the API (rung 8e): `TaskEdit.start`, `recurrence` and
/// `categories`, `ListTasks.filter`, `ListChange::CreateList`,
/// `RenameList` and `DeleteList`, and the category and extension
/// requests, so an older daemon never drops a field or a filter it
/// doesn't know and answers as if it were asked for less. 17: defer,
/// Someday and next (rung 9a): `NewTask.defer_until` and `someday`,
/// `TaskEdit.defer_until` and `someday`, `TaskFilter.deferred`,
/// `Scope::Next`, `Upcoming` and `Someday`, `Counts.upcoming` and
/// `someday`, `Seed.include_deferred` and `NextTasks`, so an older daemon
/// never drops a defer it doesn't know and adds the task in plain view.
/// 18: semantic search (rung 9c): `SearchTasks.semantic` and
/// `Seed.semantic`, so an older daemon never answers a search by meaning
/// with a keyword search. 19: nag reminders (rung 9b): `NewTask.nag`,
/// `TaskEdit.nag`, `TaskFilter.nagging`, `NotifyTest` and
/// `DoctorReport.nag`, so an older daemon never adds or edits a task
/// without its nag, or lists every task for `--nagging`. 20: contexts
/// (rung 9d): `Contexts`, `SetContext`, `InContext`,
/// `ResponseData::Contexts`, and `context` on the answers a context
/// narrows, so an older daemon never answers `--context` as if everything
/// were asked for. 21: quick add's steps and search's reach (D-068):
/// `NewTask.steps`, and `matched` on each keyword search result, so an
/// older daemon never adds a task without the steps typed with it.
pub const PROTOCOL_VERSION: u32 = 21;

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
