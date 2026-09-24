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
    /// Output format: jsonl is one object per line, ids one ID per line
    /// [default: table in a terminal, json when piped]
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
}

#[derive(Debug, Args)]
pub struct RawArgs {
    /// HTTP method. Only GET until writes arrive
    #[arg(value_enum, ignore_case = true)]
    pub method: RawMethod,
    /// Path under https://graph.microsoft.com/v1.0, such as /me/todo/lists
    pub path: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RawMethod {
    #[value(name = "GET")]
    Get,
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
