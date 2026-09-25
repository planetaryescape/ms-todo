use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use ms_todo_protocol::{Clearable, Importance};

use crate::output::OutputFormat;
use crate::phrases;

/// A local-first, keyboard-native terminal client for Microsoft To Do.
#[derive(Debug, Parser)]
#[command(name = "ms-todo", version, propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// With none, a terminal opens the TUI; anything else gets this help
    /// and exit 2, so a script never blocks on a full-screen view.
    #[command(subcommand)]
    pub command: Option<Command>,
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
    /// Your task lists, and which folder each is in
    #[command(subcommand)]
    Lists(ListsCommand),
    /// Folders that group your lists, as in the To Do app. Only ms-todo
    /// sees them, on every machine it runs on
    #[command(subcommand)]
    Folders(FoldersCommand),
    /// Tasks in a list
    #[command(subcommand)]
    Tasks(TasksCommand),
    /// Plan today: the tasks in My Day, what could go in it, and taking
    /// them in and out. A task with no due date is due today while it's
    /// there, so the To Do app shows it in its own My Day too
    #[command(subcommand, name = "myday")]
    MyDay(MyDayCommand),
    /// A task's steps: list, add, rename, check, uncheck and delete them.
    /// A step is named by its number from 1 (as `steps list` shows them),
    /// its ID, or its exact text
    #[command(subcommand)]
    Steps(StepsCommand),
    /// A task's link (its linked resource): list, add, change or delete
    /// it. Microsoft To Do allows one per task; `tasks links` also lists
    /// the URLs in the notes
    #[command(subcommand)]
    Links(LinksCommand),
    /// A task's files: list, attach, download and delete them. An
    /// attachment is named by its number from 1 (as `attachments list`
    /// shows them), its ID, or its exact name. Files go by path: the
    /// daemon reads and writes them
    #[command(subcommand)]
    Attachments(AttachmentsCommand),
    /// Find tasks by the words in their title or notes, in every list, best
    /// match first
    Search(SearchArgs),
    /// What you completed, by day, newest first: for a standup or a weekly
    /// review. Microsoft To Do keeps the day of a completion, not its time
    Done(DoneArgs),
    /// Move open tasks to a new due date: the overdue ones, those due
    /// before a day, or the ones named. One `undo` puts them all back
    Reschedule(RescheduleArgs),
    /// Writes waiting to reach Microsoft To Do, and those that didn't
    #[command(subcommand)]
    Outbox(OutboxCommand),
    /// Reverse a change: the last one by default, or the one with this op_id
    Undo(UndoArgs),
    /// Refresh the local cache from Microsoft To Do
    Sync {
        /// Wait until a sync that started after this command has finished
        #[arg(long)]
        wait: bool,
    },
    /// Check sign-in, the daemon, the local cache and each list's sync
    Doctor,
    /// Print the JSON schemas of a command's input and output (every
    /// command's without CMD)
    Schema {
        /// The command, such as `tasks list`
        #[arg(value_name = "CMD")]
        command: Vec<String>,
    },
    /// Send an authenticated request straight to Microsoft Graph, for debugging
    Raw(RawArgs),
    /// Browse and change your tasks in a full-screen, keyboard-driven view
    /// (press ? inside for the keys)
    Tui(TuiArgs),
    /// Start, stop or check the background daemon that talks to Microsoft
    #[command(subcommand)]
    Daemon(DaemonCommand),
}

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
    /// Every task list, folder by folder, then those in no folder
    List,
    /// Put lists in a folder (made if it's new), or take them out of theirs
    Move(MoveListsArgs),
    /// Put a list just before or after another list in its folder
    Order(OrderListArgs),
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("to").required(true).args(["folder", "no_folder"])))]
pub struct MoveListsArgs {
    /// The lists, by exact name or ID; several move together. `-` reads
    /// IDs from stdin, one per line
    #[arg(required = true, value_name = "LIST")]
    pub lists: Vec<String>,
    /// The folder to put them in. A folder that exists matches ignoring
    /// case; any other name makes a new one
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,
    /// Take them out of their folder
    #[arg(long)]
    pub no_folder: bool,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("place").required(true).args(["before", "after"])))]
pub struct OrderListArgs {
    /// The list to move, by exact name or ID
    #[arg(value_name = "LIST")]
    pub list: String,
    /// Put it just before this list, in the same folder
    #[arg(long, value_name = "LIST")]
    pub before: Option<String>,
    /// Put it just after this list, in the same folder
    #[arg(long, value_name = "LIST")]
    pub after: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

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

#[derive(Debug, Subcommand)]
pub enum TasksCommand {
    /// Every task in a list, completed ones included
    List {
        /// The list's exact name or its ID [default: the "Tasks" list]
        #[arg(long, value_name = "NAME|ID", conflicts_with = "my_day")]
        list: Option<String>,
        /// Today's My Day instead of a list, as `myday list` gives it
        #[arg(long, conflicts_with = "search")]
        my_day: bool,
        /// Only tasks whose title or notes match, best match first; the
        /// syntax is `search`'s
        #[arg(long, value_name = "QUERY")]
        search: Option<String>,
    },
    /// Add a task, read from text the way you'd say it
    ///
    /// `Pay rent every 1st #Finances p1 9am` is "Pay rent" in Finances,
    /// importance high, every month on the 1st, due the next 1st with a
    /// reminder at 09:00. --no-parse takes the text as the title, exactly
    /// as given; flags always win over what the text says. `tasks parse`
    /// shows the reading without adding anything
    Add(AddArgs),
    /// Show how `tasks add` would read the text, without adding anything
    Parse(ParseArgs),
    /// Suggest a list for a task with this title, without adding anything
    ///
    /// Asks TypeSafe's Jev model, through the daemon, which of your lists
    /// the task belongs in. Off unless `[suggest]` turns it on in
    /// config.toml; it sends the title and your lists' folders, names and a
    /// few open task titles. Prints the list only when the model is at
    /// least `min_confidence` sure, else no suggestion
    SuggestList(SuggestListArgs),
    /// Mark tasks completed. A recurring task moves on to its next due date
    Complete(TargetArgs),
    /// Mark completed tasks as not started again
    Reopen(TargetArgs),
    /// Change a task's title, due date, importance, reminder or notes; or
    /// the due date, importance or reminder of several tasks at once
    Edit(EditArgs),
    /// Move tasks to another list, keeping everything they hold: steps,
    /// link, attachments and ms-todo's own fields. Each is copied, the copy
    /// checked, and only then the original deleted; `undo` moves it back
    Move(MoveArgs),
    /// Delete tasks. Asks first in a terminal; anywhere else it needs --yes
    Delete {
        #[command(flatten)]
        targets: TargetArgs,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
    },
    /// A task's links: its linked resources' web addresses, then the URLs
    /// in its notes, each once
    Links(LinkArgs),
    /// Open a task's link in the browser or mail app (http, https and
    /// mailto only). With several, --index picks one; without it they're
    /// listed and it exits 2
    Open {
        #[command(flatten)]
        task: LinkArgs,
        /// Which link, from 1, as `tasks links` numbers them
        #[arg(long, value_name = "N")]
        index: Option<usize>,
    },
}

#[derive(Debug, Args)]
pub struct LinkArgs {
    /// The task's ID from `tasks list`, or its exact title when --list is
    /// given
    #[arg(value_name = "TASK")]
    pub task: String,
    /// Look for the task in this list (exact name or ID), which also lets
    /// TASK be an exact title
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Words that must all appear, in any order. Also: "an exact phrase",
    /// prefix* for words starting with it, OR, NOT and parentheses (the
    /// operators in capitals)
    #[arg(required = true, value_name = "QUERY")]
    pub query: Vec<String>,
    /// Only this list (exact name or ID) [default: every list]
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Which tasks to look through
    #[arg(long, value_enum, default_value_t = SearchStatusArg::Open)]
    pub status: SearchStatusArg,
    /// At most this many results
    #[arg(long, value_name = "N", default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,
}

#[derive(Debug, Args)]
pub struct DoneArgs {
    /// From this day: yesterday, mon (the latest Monday, today included),
    /// last week (its Monday), 12 sep, 3 days ago, 2026-09-01 [default: 7
    /// days ago]
    #[arg(long, value_name = "WHEN", value_parser = phrases::past_day, allow_hyphen_values = true)]
    pub since: Option<String>,
    /// Up to and including this day, in the same forms [default: today]
    #[arg(long, value_name = "WHEN", value_parser = phrases::past_day, allow_hyphen_values = true)]
    pub until: Option<String>,
    /// Only this list (exact name or ID) [default: every list]
    #[arg(long, value_name = "NAME|ID", conflicts_with = "folder")]
    pub list: Option<String>,
    /// Only the lists in this folder
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,
    /// At most this many, newest first
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
}

/// `--overdue`, `--due-before` and `--folder`: open tasks picked by their
/// due date, in place of naming them.
#[derive(Debug, Args)]
pub struct SelectArgs {
    /// Every open task due before today
    #[arg(long)]
    pub overdue: bool,
    /// Every open task due before this day: fri, next mon, +1w, 2026-10-02
    #[arg(long, value_name = "WHEN", value_parser = phrases::day, allow_hyphen_values = true)]
    pub due_before: Option<String>,
    /// With --overdue or --due-before: only the lists in this folder
    #[arg(
        long,
        value_name = "FOLDER",
        requires = "selector",
        conflicts_with = "list"
    )]
    pub folder: Option<String>,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("selector").args(["overdue", "due_before"])))]
#[command(group(ArgGroup::new("which").required(true).args(["tasks", "overdue", "due_before"])))]
pub struct RescheduleArgs {
    /// Task IDs from `tasks list`, or exact titles when --list is given.
    /// `-` reads IDs from stdin, one per line
    #[arg(value_name = "TASK")]
    pub tasks: Vec<String>,
    #[command(flatten)]
    pub select: SelectArgs,
    /// The new due date: today, tomorrow, fri, next mon, +3d, 12 oct,
    /// 2026-10-02
    #[arg(long, value_name = "WHEN", value_parser = phrases::day, allow_hyphen_values = true)]
    pub to: String,
    /// Only tasks in this list (exact name or ID), which also lets TASK be
    /// an exact title
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Show which tasks would move without changing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Move several tasks without asking. Off a terminal, moving more than
    /// one needs it
    #[arg(long)]
    pub yes: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SearchStatusArg {
    /// Not completed
    Open,
    /// Completed only
    Completed,
    /// Open and completed
    All,
}

/// `--idempotency-key`, on every mutation.
#[derive(Debug, Args)]
pub struct IdempotencyArgs {
    /// Run this change at most once: repeating the command with the same key
    /// returns the first result for 24 hours, and the same key with a
    /// different change exits 2
    #[arg(long, value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The task, as you'd say it. Read out of it: #List or #"Two words"
    /// (its name or a prefix only it has), @category, p1–p4, every …
    /// (a recurrence), !time-or-date (a reminder), start <date>, and a
    /// date and time (the due date; a time also sets a reminder then).
    /// "Quoted text" and \# \@ \! stay as typed. What's left is the title
    pub text: String,
    /// Take the text as the title, exactly as given: for text you didn't
    /// type yourself, such as an agent's, with flags for the fields
    #[arg(long)]
    pub no_parse: bool,
    /// Create an @category that isn't one of your Outlook categories yet.
    /// Without it the task still gets the name, which Outlook shows
    /// without a colour
    #[arg(long, conflicts_with = "no_parse")]
    pub create_categories: bool,
    /// The list's exact name or its ID, over any #List in the text
    /// [default: the "Tasks" list]
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Due date, over any in the text: 2026-10-02, today, tomorrow, fri,
    /// next mon, in 3 days, +2w, 12 oct, 12/10 (day first), end of month.
    /// Due dates have no time; put a time in --reminder
    #[arg(long, value_name = "WHEN", value_parser = phrases::due, allow_hyphen_values = true)]
    pub due: Option<Clearable<String>>,
    /// Remind me at this local time, over any in the text: 17:30 (the
    /// next one), tomorrow 9am, fri 5:30pm, 2026-10-02 09:30. A day alone,
    /// such as tomorrow or in 2 days, is 09:00 on it
    #[arg(long, value_name = "WHEN", value_parser = phrases::reminder, allow_hyphen_values = true)]
    pub reminder: Option<Clearable<String>>,
    /// How important it is, over any p1–p4 in the text: 1 or p1 (high),
    /// 2, 3, p2 or p3 (normal), 4 or p4 (low), or high, normal or low
    #[arg(long, value_name = "LEVEL", value_parser = phrases::importance)]
    pub importance: Option<Importance>,
    /// Notes, as plain text
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    /// Put it in today's My Day, as +myday or * in the text does. With no
    /// due date, it's due today too
    #[arg(long)]
    pub my_day: bool,
    /// Show what would be sent without changing anything
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct SuggestListArgs {
    /// The task's title
    pub title: String,
}

#[derive(Debug, Args)]
pub struct ParseArgs {
    /// The text, as you'd give it to `tasks add`
    pub text: String,
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
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
pub struct MoveArgs {
    /// Task IDs from `tasks list`, or exact titles when --list is given.
    /// Several move together; `-` reads IDs from stdin, one per line
    #[arg(required = true, value_name = "TASK")]
    pub tasks: Vec<String>,
    /// The list to move them to (exact name or ID)
    #[arg(long, value_name = "NAME|ID")]
    pub to: String,
    /// Look for the tasks in this list (exact name or ID), which also lets
    /// TASK be an exact title
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Show which tasks would move without changing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Move several tasks without asking. Off a terminal, moving more than
    /// one needs it
    #[arg(long)]
    pub yes: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("selector").args(["overdue", "due_before"])))]
pub struct EditArgs {
    /// The task's ID from `tasks list`, or its exact title when --list is
    /// given. Several change together; `-` reads IDs from stdin, one per line
    #[arg(
        value_name = "TASK",
        required_unless_present = "selector",
        conflicts_with = "selector"
    )]
    pub task: Vec<String>,
    #[command(flatten)]
    pub select: SelectArgs,
    /// Look for the task in this list (exact name or ID), which also lets
    /// TASK be an exact title
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// New title, taken literally
    #[arg(long)]
    pub title: Option<String>,
    /// New due date, in the forms --due takes on `tasks add`; empty or `-`
    /// removes it, as --clear-due does
    #[arg(
        long,
        value_name = "WHEN",
        value_parser = phrases::due,
        allow_hyphen_values = true,
        conflicts_with = "clear_due"
    )]
    pub due: Option<Clearable<String>>,
    /// Remove the due date
    #[arg(long)]
    pub clear_due: bool,
    /// New importance: 1 or p1 (high), 2, 3, p2 or p3 (normal), 4 or p4
    /// (low), or high, normal or low
    #[arg(long, value_name = "LEVEL", value_parser = phrases::importance)]
    pub importance: Option<Importance>,
    /// Remind me at this local time, in the forms --reminder takes on
    /// `tasks add`; empty or `-` turns it off
    #[arg(
        long,
        value_name = "WHEN",
        value_parser = phrases::reminder,
        allow_hyphen_values = true,
        conflicts_with = "clear_reminder"
    )]
    pub reminder: Option<Clearable<String>>,
    /// Turn the reminder off
    #[arg(long)]
    pub clear_reminder: bool,
    /// New notes, as plain text. They replace the old ones
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    /// Show what would change without changing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Change several tasks without asking. Off a terminal, changing more
    /// than one needs it
    #[arg(long)]
    pub yes: bool,
    #[command(flatten)]
    pub idempotency: IdempotencyArgs,
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
