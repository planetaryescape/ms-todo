//! How the daemon stands: `Status`, sync activity and reports, and the
//! daemon's side of `doctor`.

use serde::{Deserialize, Serialize};

use crate::{OpError, OutboxDepth};

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
    /// Semantic search (rung 9c); `None` from a daemon before it.
    #[serde(default)]
    pub semantic: Option<SemanticStatus>,
    /// Nag reminders (rung 9b); `None` from a daemon before them.
    #[serde(default)]
    pub nag: Option<NagStatus>,
}

/// How semantic search stands, for `doctor` (D-062).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticStatus {
    /// `[search] semantic` in config.toml.
    pub enabled: bool,
    /// The embedding model, as `name@revision`.
    pub model: String,
    pub state: ModelState,
    /// Where the model's files are, or will be once downloaded.
    pub model_dir: String,
    /// How many bytes the download is.
    pub download_bytes: u64,
    /// Live tasks embedded for their current text, and those not yet.
    #[serde(default)]
    pub indexed: u64,
    #[serde(default)]
    pub pending: u64,
    /// Why it's off or failing: a bad `[search]` setting, or the last
    /// failure to download or load the model.
    #[serde(default)]
    pub problem: Option<String>,
}

/// Where the embedding model is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    /// Semantic search is off: nothing is downloaded.
    #[default]
    Off,
    /// On, but not loaded yet: downloading, or reading it from disk.
    Loading,
    /// Loaded; the daemon embeds tasks as they change.
    Ready,
    /// The last try to download or load it failed (see `problem`); the
    /// next search, or the next sync's changes, try again.
    Failed,
    #[serde(other)]
    Unknown,
}

/// How nag reminders stand, for `doctor`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NagStatus {
    /// `[nag] enabled`, and notifications possible on this system.
    pub active: bool,
    /// How this machine shows notifications (`osascript`), or `None`
    /// where it can't.
    #[serde(default)]
    pub notifier: Option<String>,
    /// `HH:MM-HH:MM`, local, when nothing is shown; `None` when there are
    /// none.
    #[serde(default)]
    pub quiet_hours: Option<String>,
    /// Open tasks set to nag.
    pub count: u64,
    /// Why nagging is off or failing: a bad `[nag]` setting, no way to
    /// notify here, or the last notification that failed.
    #[serde(default)]
    pub problem: Option<String>,
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
