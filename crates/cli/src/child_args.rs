//! The arguments of `steps`, `links` and `attachments`: the parts of one
//! task.

use clap::{Args, Subcommand};

use crate::args::IdempotencyArgs;
use crate::task_args::LinkArgs;

#[derive(Debug, Subcommand)]
pub enum StepsCommand {
    /// The task's steps, in order, numbered from 1
    List(LinkArgs),
    /// Add steps to the end of the task's steps, unchecked, one per TEXT
    Add {
        #[command(flatten)]
        task: LinkArgs,
        /// Each step's text; several add several, in order
        #[arg(required = true, value_name = "TEXT")]
        steps: Vec<String>,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Rename a step; it stays checked or unchecked
    Edit {
        #[command(flatten)]
        task: LinkArgs,
        /// The step: its number from 1, its ID, or its exact text
        #[arg(value_name = "STEP")]
        step: String,
        /// Its new text
        #[arg(value_name = "TEXT")]
        text: String,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Check steps off
    Check(StepTargetArgs),
    /// Mark checked steps as not done again
    Uncheck(StepTargetArgs),
    /// Delete steps. Asks first in a terminal; anywhere else it needs --yes
    Delete {
        #[command(flatten)]
        steps: StepTargetArgs,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Args)]
pub struct StepTargetArgs {
    #[command(flatten)]
    pub task: LinkArgs,
    /// The steps: each its number from 1, its ID, or its exact text
    #[arg(required = true, value_name = "STEP")]
    pub steps: Vec<String>,
    #[command(flatten)]
    pub write: WriteArgs,
}

/// `--dry-run` and `--idempotency-key`, on every step and link write.
#[derive(Debug, Args)]
pub struct WriteArgs {
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Subcommand)]
pub enum LinksCommand {
    /// The task's link (its linked resource), if it has one
    List(LinkArgs),
    /// Give the task a link. It's refused when the task has one already:
    /// Microsoft To Do allows one per task
    Add {
        #[command(flatten)]
        task: LinkArgs,
        /// The link's URL. Any scheme is kept, but only http, https and
        /// mailto open from ms-todo
        #[arg(value_name = "URL")]
        url: String,
        #[command(flatten)]
        fields: LinkFieldArgs,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Change the task's link. Microsoft To Do can set a field but not
    /// clear one
    Edit {
        #[command(flatten)]
        task: LinkArgs,
        /// Which link, by number from 1 or ID [default: the task's only one]
        #[arg(value_name = "LINK")]
        link: Option<String>,
        /// A new URL
        #[arg(long, value_name = "URL")]
        url: Option<String>,
        #[command(flatten)]
        fields: LinkFieldArgs,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Delete the task's link. Asks first in a terminal; anywhere else it
    /// needs --yes
    Delete {
        #[command(flatten)]
        task: LinkArgs,
        /// Which link, by number from 1 or ID [default: the task's only one]
        #[arg(value_name = "LINK")]
        link: Option<String>,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        write: WriteArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum AttachmentsCommand {
    /// The task's attachments, numbered from 1, with Microsoft To Do's
    /// size for each (a little more than the file's bytes)
    List(LinkArgs),
    /// Attach files to the task, up to 25 MB each. The daemon reads each
    /// file when it sends it, and refuses one that changed after this
    /// command
    Add {
        #[command(flatten)]
        task: LinkArgs,
        /// The files; several attach several, in order
        #[arg(required = true, value_name = "FILE")]
        files: Vec<std::path::PathBuf>,
        #[command(flatten)]
        write: WriteArgs,
    },
    /// Save the task's attachments into a directory: every one, or those
    /// named. A file there is never replaced unless --force: the new one
    /// gets a number, as `name (1).pdf`
    Download {
        #[command(flatten)]
        task: LinkArgs,
        /// The attachments: each its number from 1, its ID, or its exact
        /// name [default: all of them]
        #[arg(value_name = "ATTACHMENT")]
        attachments: Vec<String>,
        /// The directory to save into [default: the current directory]
        #[arg(long, value_name = "DIR")]
        out: Option<std::path::PathBuf>,
        /// Replace a file of the same name instead of numbering the new one
        #[arg(long)]
        force: bool,
    },
    /// Delete attachments. Asks first in a terminal; anywhere else it
    /// needs --yes. For a week, `undo` attaches one again from a copy
    /// ms-todo keeps; if it can't keep one, nothing is deleted
    Delete {
        #[command(flatten)]
        task: LinkArgs,
        /// The attachments: each its number from 1, its ID, or its exact
        /// name
        #[arg(required = true, value_name = "ATTACHMENT")]
        attachments: Vec<String>,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
        /// Keep no copy, so `undo` can't bring it back: for when ms-todo
        /// can't keep one
        #[arg(long)]
        no_undo: bool,
        #[command(flatten)]
        write: WriteArgs,
    },
}

#[derive(Debug, Args)]
pub struct LinkFieldArgs {
    /// What the link is called (its display name)
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    /// The app it belongs to (Microsoft To Do requires one) [default on
    /// add: ms-todo]
    #[arg(long, value_name = "APP")]
    pub app: Option<String>,
    /// The item's ID in that app
    #[arg(long, value_name = "ID")]
    pub external_id: Option<String>,
}
