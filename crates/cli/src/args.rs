use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::output::OutputFormat;

/// A local-first, keyboard-native terminal client for Microsoft To Do.
#[derive(Debug, Parser)]
#[command(name = "ms-todo", version, propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
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
    /// Send an authenticated request straight to Microsoft Graph, for debugging
    Raw(RawArgs),
    /// Start, stop or check the background daemon that talks to Microsoft
    #[command(subcommand)]
    Daemon(DaemonCommand),
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
pub struct AddArgs {
    /// The task's title, taken literally
    pub text: String,
    /// The list's exact name or its ID [default: the "Tasks" list]
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Due date. Due dates have no time; put a time in --reminder
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub due: Option<String>,
    /// Remind me at this local time
    #[arg(long, value_name = "YYYY-MM-DDTHH:MM")]
    pub reminder: Option<String>,
    /// How important it is
    #[arg(long, value_enum)]
    pub importance: Option<ImportanceArg>,
    /// Notes, as plain text
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    /// Show what would be sent without changing anything
    #[arg(long)]
    pub dry_run: bool,
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
    /// New due date. Due dates have no time; put a time in --reminder
    #[arg(long, value_name = "YYYY-MM-DD", conflicts_with = "clear_due")]
    pub due: Option<String>,
    /// Remove the due date
    #[arg(long)]
    pub clear_due: bool,
    /// New importance
    #[arg(long, value_enum)]
    pub importance: Option<ImportanceArg>,
    /// Remind me at this local time
    #[arg(
        long,
        value_name = "YYYY-MM-DDTHH:MM",
        conflicts_with = "clear_reminder"
    )]
    pub reminder: Option<String>,
    /// Turn the reminder off
    #[arg(long)]
    pub clear_reminder: bool,
    /// New notes, as plain text. They replace the old ones
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ImportanceArg {
    Low,
    Normal,
    High,
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
