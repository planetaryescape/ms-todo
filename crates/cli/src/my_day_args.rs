//! The arguments of `myday list|add|remove|suggest|rollover`.

use clap::{Args, Subcommand};

use crate::args::IdempotencyArgs;

#[derive(Debug, Subcommand)]
pub enum MyDayCommand {
    /// Today's My Day, open tasks first. The day starts at
    /// `my_day.rollover_time` (00:00 unless config.toml says otherwise)
    List,
    /// Put tasks in today's My Day. One with no due date is due today too,
    /// until it leaves My Day
    Add(MyDayTargetArgs),
    /// Take tasks out of My Day, and the due date My Day gave them if
    /// nobody has changed it since
    Remove(MyDayTargetArgs),
    /// Open tasks that could go in today's My Day: due today, overdue, and
    /// those left from an earlier My Day
    Suggest,
    /// Take every task out of an earlier day's My Day now, as the daemon
    /// does each day at `my_day.rollover_time`
    Rollover {
        /// Show what would change without changing anything
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
pub struct MyDayTargetArgs {
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
