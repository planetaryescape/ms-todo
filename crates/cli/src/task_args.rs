//! The arguments of `tasks` (but `tasks list`, in `list_args`) and
//! `reschedule`, and the ways they name the tasks they act on.

use clap::{ArgGroup, Args, Subcommand};
use ms_todo_protocol::{Clearable, Importance};

use crate::args::IdempotencyArgs;
use crate::phrases;

#[derive(Debug, Subcommand)]
pub enum TasksCommand {
    /// Every task in a list, completed ones included; or with a filter
    /// and no --list, the tasks in every list that match it
    List(crate::list_args::TaskListArgs),
    /// One task, every field
    Show(LinkArgs),
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
    /// Change a task's title, dates, importance, reminder, notes,
    /// recurrence, categories, assignee, defer date, Someday or nag; or all
    /// but the title and notes of several at once
    Edit(EditArgs),
    /// Move tasks to another list, keeping everything they hold: steps,
    /// link, attachments and ms-todo's own fields. Each is copied, the copy
    /// checked, and only then the original deleted; `undo` moves it back
    Move(MoveArgs),
    /// Nag me about tasks: once a task's reminder is due, this machine
    /// notifies me every so often until it's completed
    ///
    /// Only machines running the ms-todo daemon nag; the setting itself
    /// syncs with the task. A task needs a reminder to nag. Quiet hours
    /// (`[nag] quiet_hours`, 22:00-07:00 unless set) hold notifications
    /// back. `doctor --notify-test` checks notifications reach you
    Nag(NagArgs),
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

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The task, as you'd say it. Read out of it: #List or #"Two words"
    /// (its name or a prefix only it has), @category, p1–p4, every …
    /// (a recurrence), !time-or-date (a reminder), ^date (hidden until
    /// then), +someday, +myday, start <date>, and a date and time (the due
    /// date; a time also sets a reminder then). "Quoted text" and \# \@
    /// \! \^ stay as typed. What's left is the title
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
    #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,
    /// Notes from a file, as plain text; `-` reads stdin
    #[arg(long, value_name = "FILE")]
    pub body_file: Option<std::path::PathBuf>,
    /// Start date, over any `start <date>` in the text, in the forms --due
    /// takes. With no due date, Microsoft To Do makes it the due date too
    #[arg(long, value_name = "WHEN", value_parser = phrases::day, allow_hyphen_values = true)]
    pub start: Option<String>,
    /// A category (an Outlook category's name); several give several.
    /// Over any @category in the text
    #[arg(long = "category", value_name = "NAME")]
    pub categories: Vec<String>,
    /// Repeat it, over any `every …` in the text: every mon, every 2 weeks
    /// on tue, thu, every month on the 1st, weekday, daily. It's first due
    /// on --due, or the next day it falls on
    #[arg(long, value_name = "EVERY")]
    pub recur: Option<String>,
    /// Refuse (exit 2) text the parser warns about, such as an unknown
    /// #List, instead of adding the task with a note
    #[arg(long, conflicts_with = "no_parse")]
    pub strict: bool,
    /// Put it in today's My Day, as +myday or * in the text does. With no
    /// due date, it's due today too
    #[arg(long)]
    pub my_day: bool,
    /// Who it waits on: a name or an email, only ms-todo sees it and
    /// nobody is told. The task is made "waiting on others" too, which
    /// the To Do app shows
    #[arg(long, value_name = "PERSON")]
    pub assignee: Option<String>,
    /// With --assignee: don't make it "waiting on others"
    #[arg(long, requires = "assignee")]
    pub keep_status: bool,
    /// Hide it until this day, over any ^date in the text, in the forms
    /// --due takes: fri, next week, in 3 days, 2026-10-02. Only ms-todo
    /// hides it; the due date stays the deadline
    #[arg(long, value_name = "WHEN", value_parser = phrases::day, allow_hyphen_values = true)]
    pub defer: Option<String>,
    /// Park it as Someday, as +someday in the text does: hidden until you
    /// take it out with `tasks edit --no-someday`
    #[arg(long)]
    pub someday: bool,
    /// Once its reminder is due, notify me on this machine every so often
    /// until it's done, as +nag15m in the text does: 15m, 1h, 1h30m (5m
    /// to 24h). Needs a reminder
    #[arg(long, value_name = "EVERY", value_parser = phrases::nag_every)]
    pub nag: Option<u32>,
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
#[command(group(ArgGroup::new("nag_change").required(true).args(["every", "off"])))]
pub struct NagArgs {
    /// Task IDs from `tasks list`, or exact titles when --list is given.
    /// `-` reads IDs from stdin, one per line
    #[arg(required = true, value_name = "TASK")]
    pub tasks: Vec<String>,
    /// How often: 15m, 1h, 1h30m (5m to 24h)
    #[arg(long, value_name = "EVERY", value_parser = phrases::nag_every)]
    pub every: Option<u32>,
    /// Stop nagging
    #[arg(long)]
    pub off: bool,
    /// Look for the tasks in this list (exact name or ID), which also lets
    /// TASK be an exact title
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
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
#[command(group(ArgGroup::new("assignee_change").args(["assignee", "clear_assignee"])))]
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
    #[arg(long, value_name = "TEXT", conflicts_with = "body_file")]
    pub body: Option<String>,
    /// New notes from a file, as plain text; `-` reads stdin
    #[arg(long, value_name = "FILE")]
    pub body_file: Option<std::path::PathBuf>,
    /// New start date, in the forms --due takes; empty or `-` removes it.
    /// The task's due date is sent with it, or with none, the start date
    /// becomes the due date too, as Microsoft To Do makes it
    #[arg(
        long,
        value_name = "WHEN",
        value_parser = phrases::due,
        allow_hyphen_values = true,
        conflicts_with = "clear_start"
    )]
    pub start: Option<Clearable<String>>,
    /// Remove the start date
    #[arg(long)]
    pub clear_start: bool,
    /// Make it repeat: every mon, every 2 weeks on tue, thu, every month on
    /// the 1st, weekday, daily. The due date becomes its first time: --due,
    /// or the next day it falls on from today
    #[arg(long, value_name = "EVERY", conflicts_with = "clear_recur")]
    pub recur: Option<String>,
    /// Stop it repeating. Its due date stays
    #[arg(long)]
    pub clear_recur: bool,
    /// Its categories, replacing those it has; several give several
    #[arg(
        long = "category",
        value_name = "NAME",
        conflicts_with = "clear_categories"
    )]
    pub categories: Vec<String>,
    /// Remove every category
    #[arg(long)]
    pub clear_categories: bool,
    /// Who it waits on: a name or an email, only ms-todo sees it and
    /// nobody is told. An open task becomes "waiting on others" too,
    /// which the To Do app shows
    #[arg(long, value_name = "PERSON", conflicts_with = "clear_assignee")]
    pub assignee: Option<String>,
    /// Remove the assignee. A task ms-todo made "waiting on others" is
    /// "not started" again, unless its status has changed since
    #[arg(long)]
    pub clear_assignee: bool,
    /// With --assignee or --clear-assignee: leave the status as it is
    #[arg(long, requires = "assignee_change")]
    pub keep_status: bool,
    /// Hide it until this day, in the forms --due takes; empty or `-`
    /// shows it again, as --clear-defer does. The due date isn't touched
    #[arg(
        long,
        value_name = "WHEN",
        value_parser = phrases::due,
        allow_hyphen_values = true,
        conflicts_with = "clear_defer"
    )]
    pub defer: Option<Clearable<String>>,
    /// Show it again now: remove its defer date
    #[arg(long)]
    pub clear_defer: bool,
    /// Park it as Someday: hidden until --no-someday
    #[arg(long, conflicts_with = "no_someday")]
    pub someday: bool,
    /// Take it out of Someday
    #[arg(long)]
    pub no_someday: bool,
    /// Once its reminder is due, notify me on this machine every so often
    /// until it's done: 15m, 1h, 1h30m (5m to 24h). Needs a reminder
    #[arg(long, value_name = "EVERY", value_parser = phrases::nag_every, conflicts_with_all = ["clear_nag", "clear_reminder"])]
    pub nag: Option<u32>,
    /// Stop nagging
    #[arg(long)]
    pub clear_nag: bool,
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
