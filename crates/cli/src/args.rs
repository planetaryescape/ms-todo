use clap::{Args, Parser, Subcommand};

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
    /// Output format [default: table in a terminal, json when piped]
    #[arg(long, global = true, value_enum)]
    pub format: Option<OutputFormat>,

    /// Use a separate copy of ms-todo's data (also MS_TODO_INSTANCE). Builds
    /// run from Cargo's target/ directory default to "dev"
    #[arg(long, global = true, value_name = "NAME")]
    pub instance: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sign in to Microsoft, check the sign-in, or sign out
    #[command(subcommand)]
    Auth(AuthCommand),
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Sign in with a device code: open the printed URL and enter the code
    Login,
    /// Show the signed-in account, token expiry, client ID and scopes
    Status,
    /// Delete the stored sign-in
    Logout,
}
