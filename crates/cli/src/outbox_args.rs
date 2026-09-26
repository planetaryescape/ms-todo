//! The arguments of `outbox list|retry|discard` and `undo`: the writes
//! ms-todo has made and is making.

use clap::{Args, Subcommand, ValueEnum};

use crate::args::IdempotencyArgs;

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
