use clap::{Args, Parser, Subcommand, ValueEnum};
use ms_todo_protocol::{Clearable, Importance};

use crate::output::OutputFormat;
use crate::phrases;

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
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sign in to Microsoft, check the sign-in, or sign out
    #[command(subcommand)]
    Auth(AuthCommand),
    /// Your task lists
    #[command(subcommand)]
    Lists(ListsCommand),
    /// Tasks in a list
    #[command(subcommand)]
    Tasks(TasksCommand),
    /// Find tasks by the words in their title or notes, in every list, best
    /// match first
    Search(SearchArgs),
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
    Doctor,
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

#[derive(Debug, Subcommand)]
pub enum OutboxCommand {
    /// Every queued write, newest first, with its state: pending, inflight,
    /// unknown (don't resend it yourself), failed or done
    List {
        /// Only writes in this state
        #[arg(long, value_enum)]
        state: Option<OutboxStateArg>,
    },
    /// Send a write again: an unknown one (it may then happen twice) or a
    /// failed one
    Retry {
        /// The write's op_id from `outbox list`
        #[arg(value_name = "OP")]
        op: String,
    },
    /// Drop a write that isn't done. One that never reached Microsoft To Do
    /// is undone locally. Asks first in a terminal; anywhere else it needs --yes
    Discard {
        /// The write's op_id from `outbox list`
        #[arg(value_name = "OP")]
        op: String,
        /// Discard without asking
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OutboxStateArg {
    Pending,
    Inflight,
    Unknown,
    Failed,
    Done,
}

#[derive(Debug, Args)]
pub struct UndoArgs {
    /// The op_id a change printed [default: the latest change not undone yet]
    #[arg(value_name = "OP_ID")]
    pub op_id: Option<String>,
    /// For a completed recurring task: the ID of the completed copy to
    /// delete, from the candidates `undo` lists without it
    #[arg(long, value_name = "ID")]
    pub copy: Option<String>,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Sign in with a device code: open the printed URL and enter the code
    Login,
    /// Show the signed-in account, token expiry, client ID and scopes
    Status,
    /// Delete the stored sign-in
    Logout,
    /// Print a valid access token for Microsoft Graph, e.g. for curl. With
    /// --format table, prints only the token
    Bearer {
        /// Confirm that the token may be printed. Anyone holding it can act
        /// as you until it expires
        #[arg(long)]
        reveal_secret: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ListsCommand {
    /// Every task list
    List,
}

#[derive(Debug, Subcommand)]
pub enum TasksCommand {
    /// Every task in a list, completed ones included
    List {
        /// The list's exact name or its ID [default: the "Tasks" list]
        #[arg(long, value_name = "NAME|ID")]
        list: Option<String>,
        /// Only tasks whose title or notes match, best match first; the
        /// syntax is `search`'s
        #[arg(long, value_name = "QUERY")]
        search: Option<String>,
    },
    /// Add a task. The text is its title, exactly as given
    Add(AddArgs),
    /// Mark tasks completed. A recurring task moves on to its next due date
    Complete(TargetArgs),
    /// Mark completed tasks as not started again
    Reopen(TargetArgs),
    /// Change a task's title, due date, importance, reminder or notes
    Edit(EditArgs),
    /// Delete tasks. Asks first in a terminal; anywhere else it needs --yes
    Delete {
        #[command(flatten)]
        targets: TargetArgs,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Words that must all appear, in any order. Also: "an exact phrase",
    /// prefix* for words starting with it, OR, NOT and parentheses (the
    /// operators in capitals)
    #[arg(required = true, value_name = "QUERY")]
    pub query: Vec<String>,
    /// Only this list (exact name or ID) [default: every list]
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Which tasks to look through
    #[arg(long, value_enum, default_value_t = SearchStatusArg::Open)]
    pub status: SearchStatusArg,
    /// At most this many results
    #[arg(long, value_name = "N", default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SearchStatusArg {
    /// Not completed
    Open,
    /// Completed only
    Completed,
    /// Open and completed
    All,
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

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The task's title, taken literally
    pub text: String,
    /// The list's exact name or its ID [default: the "Tasks" list]
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Due date: 2026-10-02, today, tomorrow, fri, next mon, in 3 days,
    /// +2w, 12 oct, 12/10 (day first), end of month. Due dates have no
    /// time; put a time in --reminder
    #[arg(long, value_name = "WHEN", value_parser = phrases::due, allow_hyphen_values = true)]
    pub due: Option<Clearable<String>>,
    /// Remind me at this local time: 17:30 (the next one), tomorrow 9am,
    /// fri 5:30pm, 2026-10-02 09:30. A day needs a time
    #[arg(long, value_name = "WHEN", value_parser = phrases::reminder, allow_hyphen_values = true)]
    pub reminder: Option<Clearable<String>>,
    /// How important it is: 1 or p1 (high), 2, 3, p2 or p3 (normal), 4
    /// or p4 (low), or high, normal or low
    #[arg(long, value_name = "LEVEL", value_parser = phrases::importance)]
    pub importance: Option<Importance>,
    /// Notes, as plain text
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    /// Show what would be sent without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct TargetArgs {
    /// Task IDs from `tasks list`, or exact titles when --list is given.
    /// `-` reads IDs from stdin, one per line
    #[arg(required = true, value_name = "TASK")]
    pub tasks: Vec<String>,
    /// Look for the tasks in this list (exact name or ID), which also lets
    /// TASK be an exact title
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct EditArgs {
    /// The task's ID from `tasks list`, or its exact title when --list is given
    #[arg(value_name = "TASK")]
    pub task: String,
    /// Look for the task in this list (exact name or ID), which also lets
    /// TASK be an exact title
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// New title, taken literally
    #[arg(long)]
    pub title: Option<String>,
    /// New due date, in the forms --due takes on `tasks add`; empty or `-`
    /// removes it, as --clear-due does
    #[arg(
        long,
        value_name = "WHEN",
        value_parser = phrases::due,
        allow_hyphen_values = true,
        conflicts_with = "clear_due"
    )]
    pub due: Option<Clearable<String>>,
    /// Remove the due date
    #[arg(long)]
    pub clear_due: bool,
    /// New importance: 1 or p1 (high), 2, 3, p2 or p3 (normal), 4 or p4
    /// (low), or high, normal or low
    #[arg(long, value_name = "LEVEL", value_parser = phrases::importance)]
    pub importance: Option<Importance>,
    /// Remind me at this local time, in the forms --reminder takes on
    /// `tasks add`; empty or `-` turns it off
    #[arg(
        long,
        value_name = "WHEN",
        value_parser = phrases::reminder,
        allow_hyphen_values = true,
        conflicts_with = "clear_reminder"
    )]
    pub reminder: Option<Clearable<String>>,
    /// Turn the reminder off
    #[arg(long)]
    pub clear_reminder: bool,
    /// New notes, as plain text. They replace the old ones
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct RawArgs {
    /// HTTP method. POST, PATCH and DELETE are never resent once they may have
    /// reached Graph
    #[arg(value_enum, ignore_case = true)]
    pub method: RawMethod,
    /// Path under https://graph.microsoft.com/v1.0, such as /me/todo/lists
    pub path: String,
    /// JSON request body, for POST and PATCH
    #[arg(long, value_name = "JSON")]
    pub body: Option<String>,
    /// Send a POST, PATCH or DELETE when not in a terminal
    #[arg(long)]
    pub yes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum RawMethod {
    #[value(name = "GET")]
    Get,
    #[value(name = "POST")]
    Post,
    #[value(name = "PATCH")]
    Patch,
    #[value(name = "DELETE")]
    Delete,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Start the daemon if it isn't running, and wait until it's ready
    Start,
    /// Stop the daemon and wait until its process has exited
    Stop,
    /// Show whether the daemon is running, its PID, version and socket
    Status,
    /// Run the daemon in the foreground (what `start` launches)
    #[command(hide = true)]
    Run,
}

#[derive(Debug, Default, Args)]
pub struct TuiArgs {
    /// Draw with plain ASCII instead of Unicode symbols
    #[arg(long)]
    pub ascii: bool,
    /// Measure the start and a scripted run of keys against your cache,
    /// print the timings and quit
    #[arg(long)]
    pub bench_startup: bool,
}
