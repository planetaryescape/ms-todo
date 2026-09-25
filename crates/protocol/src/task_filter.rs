//! What `tasks list` narrows and orders by (docs/blueprint/07-cli.md):
//! `--status`, `--due`, `--importance`, `--category`, `--sort` and
//! `--limit`, resolved by the daemon over the cache.

use serde::{Deserialize, Serialize};

use crate::Importance;

/// A `ListTasks` filter. Every field left out matches every task; one
/// that narrows (all but `sort` and `limit`) without a list looks in every
/// list, as `--assignee` does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFilter {
    #[serde(default)]
    pub status: Option<StatusFilter>,
    #[serde(default)]
    pub due: Option<DueFilter>,
    #[serde(default)]
    pub importance: Option<Importance>,
    /// Only tasks carrying this category, ignoring case.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub sort: Option<TaskSort>,
    /// At most this many, after sorting.
    #[serde(default)]
    pub limit: Option<u32>,
}

impl TaskFilter {
    /// Whether it narrows which tasks come back (not only their order or
    /// number).
    pub fn narrows(&self) -> bool {
        self.status.is_some()
            || self.due.is_some()
            || self.importance.is_some()
            || self.category.is_some()
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Which tasks by status: open or completed, or one of Graph's statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusFilter {
    /// Anything not completed.
    Open,
    Completed,
    All,
    NotStarted,
    InProgress,
    WaitingOnOthers,
    Deferred,
}

impl StatusFilter {
    /// Whether a task with Graph's `status` matches.
    pub fn matches(self, status: &str) -> bool {
        match self {
            Self::Open => status != "completed",
            Self::Completed => status == "completed",
            Self::All => true,
            Self::NotStarted => status == "notStarted",
            Self::InProgress => status == "inProgress",
            Self::WaitingOnOthers => status == "waitingOnOthers",
            Self::Deferred => status == "deferred",
        }
    }
}

/// Which tasks by due date, each a local day (`YYYY-MM-DD`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "due", content = "day", rename_all = "snake_case")]
pub enum DueFilter {
    /// Due today.
    Today,
    /// Open and due before today.
    Overdue,
    /// With no due date.
    None,
    /// With any due date.
    Any,
    /// Due on this day.
    On(String),
    /// Due before this day.
    Before(String),
    /// Due after this day.
    After(String),
}

/// How `tasks list` orders what it finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSort {
    /// Soonest due first; no due date last.
    Due,
    /// High first.
    Importance,
    /// Newest first.
    Created,
    /// Most recently changed first.
    Modified,
    /// A to Z, ignoring case.
    Title,
}
