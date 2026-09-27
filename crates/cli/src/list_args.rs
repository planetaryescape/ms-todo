//! The arguments of `tasks list` and of `lists` (rung 8e).

use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use ms_todo_protocol::{DeferredFilter, DueFilter, Importance, StatusFilter, TaskSort};

use crate::args::IdempotencyArgs;
use crate::phrases;

#[derive(Debug, Args)]
pub struct TaskListArgs {
    /// The list's exact name or its ID [default: the "Tasks" list, or with
    /// --status, --due, --importance, --category, --deferred only or
    /// --nagging, every list]
    #[arg(long, value_name = "NAME|ID", conflicts_with = "my_day")]
    pub list: Option<String>,
    /// Today's My Day instead of a list, as `myday list` gives it
    #[arg(long, conflicts_with_all = ["search", "assignee", "status", "completed", "due", "importance", "category", "nagging", "sort", "limit", "deferred"])]
    pub my_day: bool,
    /// Only tasks whose title or notes match, best match first; the
    /// syntax is `search`'s
    #[arg(long, value_name = "QUERY")]
    pub search: Option<String>,
    /// Only tasks assigned to this person (a case-insensitive exact
    /// match), or to anyone with `*`. Without --list, the open tasks
    /// of every list, grouped by person
    #[arg(long, value_name = "PERSON")]
    pub assignee: Option<String>,
    /// Only tasks with this status
    #[arg(long, value_enum, conflicts_with = "completed")]
    pub status: Option<StatusArg>,
    /// Only completed tasks, as --status completed
    #[arg(long)]
    pub completed: bool,
    /// Only tasks due: today, overdue (open and due before today), none,
    /// any, "before fri", "after 12 oct", or on a day (tomorrow,
    /// 2026-10-02)
    #[arg(long, value_name = "WHEN", value_parser = phrases::due_filter)]
    pub due: Option<DueFilter>,
    /// Only tasks of this importance: high, normal or low (or 1–4, p1–p4)
    #[arg(long, value_name = "LEVEL", value_parser = phrases::importance)]
    pub importance: Option<Importance>,
    /// Only tasks with this category (ignoring case)
    #[arg(long, value_name = "NAME")]
    pub category: Option<String>,
    /// Only open tasks set to nag (`tasks nag`); each has `nag`, its
    /// interval in minutes, in its extension
    #[arg(long)]
    pub nagging: bool,
    /// Order by: due (soonest first, none last), importance (high first),
    /// created or modified (newest first), title [default: the list's
    /// order; every list's is by due date]
    #[arg(long, value_enum)]
    pub sort: Option<SortArg>,
    /// At most this many, after sorting
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    /// Tasks deferred to a later day, or parked as Someday: hide (the
    /// default; the table says how many), include, or only them (every
    /// list without --list, by the day they come back, then Someday). A
    /// search always includes them
    #[arg(long, value_enum, value_name = "WHICH")]
    pub deferred: Option<DeferredArg>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DeferredArg {
    Hide,
    Include,
    Only,
}

impl From<DeferredArg> for DeferredFilter {
    fn from(deferred: DeferredArg) -> Self {
        match deferred {
            DeferredArg::Hide => Self::Hide,
            DeferredArg::Include => Self::Include,
            DeferredArg::Only => Self::Only,
        }
    }
}

impl TaskListArgs {
    pub fn filter(&self) -> ms_todo_protocol::TaskFilter {
        ms_todo_protocol::TaskFilter {
            status: match (self.status, self.completed) {
                (Some(status), _) => Some(status.into()),
                (None, true) => Some(StatusFilter::Completed),
                (None, false) => None,
            },
            due: self.due.clone(),
            importance: self.importance,
            category: self.category.clone(),
            nagging: self.nagging,
            sort: self.sort.map(Into::into),
            limit: self.limit,
            deferred: self.deferred.map(Into::into).unwrap_or_default(),
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum StatusArg {
    /// Not completed
    Open,
    Completed,
    /// Open and completed
    All,
    NotStarted,
    InProgress,
    /// Waiting on others
    Waiting,
    Deferred,
}

impl From<StatusArg> for StatusFilter {
    fn from(status: StatusArg) -> Self {
        match status {
            StatusArg::Open => Self::Open,
            StatusArg::Completed => Self::Completed,
            StatusArg::All => Self::All,
            StatusArg::NotStarted => Self::NotStarted,
            StatusArg::InProgress => Self::InProgress,
            StatusArg::Waiting => Self::WaitingOnOthers,
            StatusArg::Deferred => Self::Deferred,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SortArg {
    Due,
    Importance,
    Created,
    Modified,
    Title,
}

impl From<SortArg> for TaskSort {
    fn from(sort: SortArg) -> Self {
        match sort {
            SortArg::Due => Self::Due,
            SortArg::Importance => Self::Importance,
            SortArg::Created => Self::Created,
            SortArg::Modified => Self::Modified,
            SortArg::Title => Self::Title,
        }
    }
}

#[derive(Debug, Args)]
pub struct ShowListArgs {
    /// The list's exact name or its ID
    #[arg(value_name = "LIST")]
    pub list: String,
}

#[derive(Debug, Args)]
pub struct CreateListArgs {
    /// The new list's name, which no other list may have
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Put it in this folder (made if it's new; an existing one matches
    /// ignoring case)
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct RenameListArgs {
    /// The list's exact name or its ID
    #[arg(value_name = "LIST")]
    pub list: String,
    /// Its new name, which no other list may have
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct DeleteListArgs {
    /// The list's exact name or its ID
    #[arg(value_name = "LIST")]
    pub list: String,
    /// Delete without asking
    #[arg(long)]
    pub yes: bool,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Subcommand)]
pub enum ListsCommand {
    /// Every task list, folder by folder, then those in no folder
    List,
    /// One list, with its open and completed task counts
    Show(crate::list_args::ShowListArgs),
    /// Make a new list, in a folder if --folder says so
    Create(crate::list_args::CreateListArgs),
    /// Rename a list. The default list (Tasks) and Flagged Emails can't be
    Rename(crate::list_args::RenameListArgs),
    /// Delete a list and every task in it. Asks first in a terminal;
    /// anywhere else it needs --yes. Only an empty list's delete can be
    /// undone
    Delete(crate::list_args::DeleteListArgs),
    /// Put lists in a folder (made if it's new), or take them out of theirs
    Move(MoveListsArgs),
    /// Put a list just before or after another list in its folder
    Order(OrderListArgs),
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("to").required(true).args(["folder", "no_folder"])))]
pub struct MoveListsArgs {
    /// The lists, by exact name or ID; several move together. `-` reads
    /// IDs from stdin, one per line
    #[arg(required = true, value_name = "LIST")]
    pub lists: Vec<String>,
    /// The folder to put them in. A folder that exists matches ignoring
    /// case; any other name makes a new one
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,
    /// Take them out of their folder
    #[arg(long)]
    pub no_folder: bool,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("place").required(true).args(["before", "after"])))]
pub struct OrderListArgs {
    /// The list to move, by exact name or ID
    #[arg(value_name = "LIST")]
    pub list: String,
    /// Put it just before this list, in the same folder
    #[arg(long, value_name = "LIST")]
    pub before: Option<String>,
    /// Put it just after this list, in the same folder
    #[arg(long, value_name = "LIST")]
    pub after: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}
