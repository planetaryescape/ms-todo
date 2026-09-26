//! The arguments of `folders list|rename|delete|order`.

use clap::{ArgGroup, Args, Subcommand};

use crate::args::IdempotencyArgs;

#[derive(Debug, Subcommand)]
pub enum FoldersCommand {
    /// Every folder in order, with its lists and open tasks
    List,
    /// Rename a folder; every list in it moves with it
    Rename(RenameFolderArgs),
    /// Delete a folder. Its lists stay, in no folder; no list or task is
    /// deleted. Asks first in a terminal; anywhere else it needs --yes
    Delete(DeleteFolderArgs),
    /// Put a folder just before or after another folder
    Order(OrderFolderArgs),
}

#[derive(Debug, Args)]
pub struct RenameFolderArgs {
    /// The folder's name (case doesn't matter)
    #[arg(value_name = "FOLDER")]
    pub folder: String,
    /// Its new name, which no other folder may have
    #[arg(value_name = "NEW")]
    pub name: String,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct DeleteFolderArgs {
    /// The folder's name (case doesn't matter)
    #[arg(value_name = "FOLDER")]
    pub folder: String,
    /// Delete without asking
    #[arg(long)]
    pub yes: bool,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("place").required(true).args(["before", "after"])))]
pub struct OrderFolderArgs {
    /// The folder to move (case doesn't matter)
    #[arg(value_name = "FOLDER")]
    pub folder: String,
    /// Put it just before this folder
    #[arg(long, value_name = "FOLDER")]
    pub before: Option<String>,
    /// Put it just after this folder
    #[arg(long, value_name = "FOLDER")]
    pub after: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}
