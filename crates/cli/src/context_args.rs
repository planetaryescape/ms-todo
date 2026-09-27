//! The arguments of `ctx`: show, list or switch the context that narrows
//! the everyday reads (rung 9d).

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct CtxArgs {
    #[command(subcommand)]
    pub command: Option<CtxCommand>,
    /// Make this context active (as named in config.toml), or `none` to
    /// show everything again
    #[arg(value_name = "NAME")]
    pub name: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum CtxCommand {
    /// The active context and the lists it covers (the same as `ctx`)
    Show,
    /// Every context config.toml defines, with the lists each covers now
    List,
}
