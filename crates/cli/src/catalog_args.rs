//! The arguments of `categories` and `extensions` (rung 8e).

use clap::{Args, Subcommand, ValueEnum};

use crate::args::IdempotencyArgs;

#[derive(Debug, Subcommand)]
pub enum CategoriesCommand {
    /// Every category, with its colour
    List,
    /// Make a category. Names are unique ignoring case
    Create {
        /// Its name
        #[arg(value_name = "NAME")]
        name: String,
        /// Its colour: preset0 to preset24 (preset3 is yellow, preset4
        /// green), or none [default: Outlook's]
        #[arg(long, value_name = "COLOR")]
        color: Option<String>,
        #[command(flatten)]
        write: CatalogWriteArgs,
    },
    /// Change a category's colour
    Recolor {
        /// Its name (ignoring case) or ID
        #[arg(value_name = "CATEGORY")]
        category: String,
        /// The colour: preset0 to preset24, or none
        #[arg(long, value_name = "COLOR")]
        color: String,
        #[command(flatten)]
        write: CatalogWriteArgs,
    },
    /// Delete a category. Tasks keep its name as a label, shown without a
    /// colour. Asks first in a terminal; anywhere else it needs --yes
    Delete {
        /// Its name (ignoring case) or ID
        #[arg(value_name = "CATEGORY")]
        category: String,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        write: CatalogWriteArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum ExtensionsCommand {
    /// ms-todo's own extension on a list or task. Microsoft To Do can't
    /// list the others; `get` reads any by name
    List(OwnerArgs),
    /// One open extension, by name
    Get {
        #[command(flatten)]
        owner: OwnerArgs,
        /// The extension's name, such as com.example.app
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Make an open extension hold exactly this JSON object, creating it
    /// if it's missing
    Set {
        #[command(flatten)]
        owner: OwnerArgs,
        /// The extension's name, such as com.example.app
        #[arg(value_name = "NAME")]
        name: String,
        /// Its fields, as a JSON object
        #[arg(long, value_name = "JSON")]
        json: String,
        #[command(flatten)]
        write: CatalogWriteArgs,
    },
    /// Delete an open extension. Asks first in a terminal; anywhere else
    /// it needs --yes
    Delete {
        #[command(flatten)]
        owner: OwnerArgs,
        /// The extension's name
        #[arg(value_name = "NAME")]
        name: String,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        write: CatalogWriteArgs,
    },
}

/// Whose extensions: a list, by name or ID, or a task, by ID or exact
/// title with --list.
#[derive(Debug, Args)]
pub struct OwnerArgs {
    #[arg(value_enum, value_name = "KIND")]
    pub kind: OwnerKindArg,
    /// The list's exact name or ID; the task's ID, or its exact title with
    /// --list
    #[arg(value_name = "ID")]
    pub id: String,
    /// For a task: look for it in this list (exact name or ID)
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OwnerKindArg {
    List,
    Task,
}

/// `--dry-run` and `--idempotency-key`, on every category and extension
/// write.
#[derive(Debug, Args)]
pub struct CatalogWriteArgs {
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}
