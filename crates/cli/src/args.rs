//! The command line: `Cli`, its global flags and every command. Most
//! commands' arguments live in the `*_args` modules by job and are
//! re-exported here, so `crate::args::X` still names them.

use clap::{Args, Parser, Subcommand};

use crate::output::OutputFormat;

pub use crate::child_args::{
    AttachmentsCommand, LinksCommand, StepTargetArgs, StepsCommand, WriteArgs,
};
pub use crate::find_args::{DoneArgs, NextArgs, SearchArgs, SearchStatusArg};
pub use crate::folder_args::{DeleteFolderArgs, FoldersCommand, OrderFolderArgs, RenameFolderArgs};
pub use crate::list_args::{ListsCommand, MoveListsArgs, OrderListArgs};
pub use crate::my_day_args::{MyDayCommand, MyDayTargetArgs};
pub use crate::outbox_args::{OutboxCommand, OutboxStateArg, UndoArgs};
pub use crate::system_args::{AuthCommand, DaemonCommand, RawArgs, RawMethod, TuiArgs};
pub use crate::task_args::{
    AddArgs, EditArgs, LinkArgs, MoveArgs, NagArgs, ParseArgs, RescheduleArgs, SelectArgs,
    TargetArgs, TasksCommand,
};

/// A local-first, keyboard-native terminal client for Microsoft To Do.
#[derive(Debug, Parser)]
#[command(name = "ms-todo", version, propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// With none, a terminal opens the TUI; anything else gets this help
    /// and exit 2, so a script never blocks on a full-screen view.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Output format: jsonl is one object per line, ids one ID per line,
    /// csv has a header row [default: table in a terminal, json when piped]
    #[arg(long, global = true, value_enum)]
    pub format: Option<OutputFormat>,

    /// Use a separate copy of ms-todo's data (also MS_TODO_INSTANCE). Builds
    /// run from Cargo's target/ directory default to "dev"; "default" is the
    /// installed copy
    #[arg(long, global = true, value_name = "NAME")]
    pub instance: Option<String>,

    /// Before reading, sync and wait, so the answer is as fresh as Microsoft
    /// To Do's (a read is otherwise answered from the cache)
    #[arg(long, global = true)]
    pub fresh: bool,

    /// Print nothing on stderr but errors: no notes and no progress
    #[arg(long, global = true)]
    pub quiet: bool,

    /// No colour or bold in tables (also NO_COLOR)
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sign in to Microsoft, check the sign-in, or sign out
    #[command(subcommand)]
    Auth(AuthCommand),
    /// Your task lists, and which folder each is in
    #[command(subcommand)]
    Lists(ListsCommand),
    /// Folders that group your lists, as in the To Do app. Only ms-todo
    /// sees them, on every machine it runs on
    #[command(subcommand)]
    Folders(FoldersCommand),
    /// Tasks in a list
    #[command(subcommand)]
    Tasks(TasksCommand),
    /// Tasks you're waiting on: every open task with an assignee, grouped
    /// by person. The same as `tasks list --assignee '*'`, or with PERSON,
    /// `tasks list --assignee PERSON`
    Waiting {
        /// Only this person's (a case-insensitive exact match)
        #[arg(value_name = "PERSON")]
        person: Option<String>,
    },
    /// Plan today: the tasks in My Day, what could go in it, and taking
    /// them in and out. A task with no due date is due today while it's
    /// there, so the To Do app shows it in its own My Day too
    #[command(subcommand, name = "myday")]
    MyDay(MyDayCommand),
    /// A task's steps: list, add, rename, check, uncheck and delete them.
    /// A step is named by its number from 1 (as `steps list` shows them),
    /// its ID, or its exact text
    #[command(subcommand)]
    Steps(StepsCommand),
    /// A task's link (its linked resource): list, add, change or delete
    /// it. Microsoft To Do allows one per task; `tasks links` also lists
    /// the URLs in the notes
    #[command(subcommand)]
    Links(LinksCommand),
    /// A task's files: list, attach, download and delete them. An
    /// attachment is named by its number from 1 (as `attachments list`
    /// shows them), its ID, or its exact name. Files go by path: the
    /// daemon reads and writes them
    #[command(subcommand)]
    Attachments(AttachmentsCommand),
    /// Your Outlook categories: list, create, recolour and delete them.
    /// Microsoft To Do can't rename one; the iPhone app shows none
    #[command(subcommand)]
    Categories(crate::catalog_args::CategoriesCommand),
    /// Open extensions on a list or task: data other apps (or you) keep
    /// there. ms-todo's own, com.planetaryescape.mstodo, can be read but
    /// not written here
    #[command(subcommand)]
    Extensions(crate::catalog_args::ExtensionsCommand),
    /// Find tasks by the words in their title or notes, in every list, best
    /// match first
    Search(SearchArgs),
    /// What you completed, by day, newest first: for a standup or a weekly
    /// review. Microsoft To Do keeps the day of a completion, not its time
    Done(DoneArgs),
    /// What to do now: the few open tasks that matter most, each with why.
    /// Overdue first (most overdue first), then due today, in My Day, high
    /// importance, due within 3 days, then the oldest. Deferred and
    /// Someday tasks are never next
    Next(NextArgs),
    /// Move open tasks to a new due date: the overdue ones, those due
    /// before a day, or the ones named. One `undo` puts them all back
    Reschedule(RescheduleArgs),
    /// Writes waiting to reach Microsoft To Do, and those that didn't
    #[command(subcommand)]
    Outbox(OutboxCommand),
    /// Reverse a change: the last one by default, or the one with this op_id
    Undo(UndoArgs),
    /// Refresh the local cache from Microsoft To Do
    Sync {
        /// Wait until a sync that started after this command has finished
        #[arg(long)]
        wait: bool,
    },
    /// Check sign-in, the daemon, the local cache and each list's sync
    Doctor {
        /// First show a test notification the way a nag does, to check
        /// that nag reminders reach you on this machine
        #[arg(long)]
        notify_test: bool,
    },
    /// Print the JSON schemas of a command's input and output (every
    /// command's without CMD)
    Schema {
        /// The command, such as `tasks list`
        #[arg(value_name = "CMD")]
        command: Vec<String>,
    },
    /// Send an authenticated request straight to Microsoft Graph, for debugging
    Raw(RawArgs),
    /// Browse and change your tasks in a full-screen, keyboard-driven view
    /// (press ? inside for the keys)
    Tui(TuiArgs),
    /// Start, stop or check the background daemon that talks to Microsoft
    #[command(subcommand)]
    Daemon(DaemonCommand),
}

/// `--idempotency-key`, on every mutation.
#[derive(Debug, Args)]
pub struct IdempotencyArgs {
    /// Run this change at most once: repeating the command with the same key
    /// returns the first result for 24 hours, and the same key with a
    /// different change exits 2
    #[arg(long, value_name = "KEY")]
    pub idempotency_key: Option<String>,
}
