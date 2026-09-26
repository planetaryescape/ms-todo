//! The arguments of `auth`, `raw`, `tui` and `daemon`: the commands about
//! ms-todo itself rather than your tasks.

use clap::{Args, Subcommand, ValueEnum};

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
    /// Stop the daemon if it's running, then start it again
    Restart,
    /// Print the daemon's log
    Logs {
        /// Keep printing what's added to it, until interrupted
        #[arg(long)]
        follow: bool,
    },
    /// Run the daemon in the foreground (what `launch` starts)
    #[command(hide = true)]
    Run,
    /// Start `daemon run` in a session of its own and relay an early exit
    /// (what `start` spawns, so no client is ever the daemon's parent)
    #[command(hide = true)]
    Launch,
}

#[derive(Debug, Default, Args)]
pub struct TuiArgs {
    /// Draw with plain ASCII instead of Unicode symbols
    #[arg(long)]
    pub ascii: bool,
    /// Draw with this theme instead of `[tui] theme` in config.toml
    /// (see --list-themes)
    #[arg(long, value_name = "NAME")]
    pub theme: Option<String>,
    /// Print the built-in themes' names, one per line, and quit
    #[arg(long)]
    pub list_themes: bool,
    /// Measure the start and a scripted run of keys against your cache,
    /// print the timings and quit
    #[arg(long)]
    pub bench_startup: bool,
}
