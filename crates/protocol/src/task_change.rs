//! A task to create, and the changes `ChangeTasks` makes to tasks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    Low,
    Normal,
    High,
}

/// A task to create. The title is taken literally; dates are validated by
/// the daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    /// A list name or ID; `None` is the "Tasks" list (D-022).
    #[serde(default)]
    pub list: Option<String>,
    /// `YYYY-MM-DD`. Due dates are dates only (D-027).
    #[serde(default)]
    pub due: Option<String>,
    /// `YYYY-MM-DDTHH:MM`, local time.
    #[serde(default)]
    pub reminder: Option<String>,
    #[serde(default)]
    pub importance: Option<Importance>,
    /// Plain-text notes.
    #[serde(default)]
    pub body: Option<String>,
    /// `YYYY-MM-DD`. Graph sets the due date to it too when there's none
    /// (S11).
    #[serde(default)]
    pub start: Option<String>,
    /// Graph's `patternedRecurrence`, less `range.recurrenceTimeZone`,
    /// which the daemon sets to the zone it writes the due date in (S12).
    /// `range.startDate` is the first due date.
    #[serde(default)]
    pub recurrence: Option<Value>,
    /// Outlook category names, as the task carries them.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Put it in today's My Day; with no due date, it's due today too, so
    /// the phone shows it in its own My Day (D-037).
    #[serde(default)]
    pub my_day: bool,
    /// Who the task waits on: ms-todo's `assignee` (free text or an
    /// email), which means nothing to Microsoft To Do. Unless
    /// `keep_status`, the task is made `waitingOnOthers` too.
    #[serde(default)]
    pub assignee: Option<String>,
    /// With `assignee`: leave the status as it would be.
    #[serde(default)]
    pub keep_status: bool,
    /// `YYYY-MM-DD`, a local day: hide the task until then (ms-todo's
    /// `deferUntil`; the due date stays the deadline).
    #[serde(default)]
    pub defer_until: Option<String>,
    /// Park it as Someday (ms-todo's `someday`): hidden until taken out.
    #[serde(default)]
    pub someday: bool,
    /// Nag every this many minutes once the reminder is due, until it's
    /// completed ([`NAG_MIN_MINUTES`] to [`NAG_MAX_MINUTES`]); ms-todo's
    /// `nag`. Needs `reminder`.
    #[serde(default)]
    pub nag: Option<u32>,
}

/// The shortest nag interval: more often is noise, not a reminder.
pub const NAG_MIN_MINUTES: u32 = 5;
/// The longest: a day. Past that, a reminder of its own does the job.
pub const NAG_MAX_MINUTES: u32 = 24 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TaskChange {
    Complete,
    Reopen,
    Delete,
    Edit(TaskEdit),
    /// Move the tasks to the list `to` (a name or an ID): each is copied
    /// there with everything it holds, the copy checked, then the original
    /// deleted (docs/blueprint/05-custom-features.md#move-between-lists).
    Move {
        to: String,
    },
    /// Put the tasks in today's My Day; one with no due date is due today
    /// too, which ms-todo takes away again at the rollover (D-037).
    AddToMyDay,
    /// Take the tasks out of My Day, and an open one's due date with it
    /// when ms-todo set that date and nobody has changed it since.
    RemoveFromMyDay,
    /// Add a step (checklist item) to the one task named for each text,
    /// in order, unchecked.
    AddSteps {
        steps: Vec<String>,
    },
    /// Rename a step of the one task named. `step` is its number from 1
    /// as shown, its ID, or its exact text.
    EditStep {
        step: String,
        text: String,
    },
    /// Check the steps named (as `EditStep` names one), or uncheck them.
    CheckSteps {
        steps: Vec<String>,
        checked: bool,
    },
    /// Delete the steps named (as `EditStep` names one).
    DeleteSteps {
        steps: Vec<String>,
    },
    /// Give the one task named its link (linked resource). Graph allows
    /// one per task (S14), so a task with one already is refused.
    AddLink(NewLink),
    /// Change fields of the task's link. Graph keeps a field left out and
    /// can't clear one (S15).
    EditLink(LinkEdit),
    /// Delete the task's link.
    DeleteLink {
        /// Its number from 1 or its ID; `None` is the task's only link.
        #[serde(default)]
        link: Option<String>,
    },
    /// Attach files to the one task named, in order. Each is an absolute
    /// path the daemon reads when it sends it: no bytes cross the socket.
    AddAttachments {
        files: Vec<String>,
    },
    /// Delete attachments of the one task named: each by its number from
    /// 1, its ID or its exact name. The daemon keeps a copy of each first,
    /// for `undo`, and deletes nothing if it can't, unless `no_undo`.
    DeleteAttachments {
        attachments: Vec<String>,
        #[serde(default)]
        no_undo: bool,
    },
    #[serde(other)]
    Unknown,
}

/// A link to give a task: Graph's `linkedResource`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewLink {
    /// `webUrl`. Graph takes any URL; only http, https and mailto open.
    pub url: String,
    /// `displayName`.
    #[serde(default)]
    pub name: Option<String>,
    /// `applicationName`, which Graph requires; `ms-todo` when `None`.
    #[serde(default)]
    pub app: Option<String>,
    /// `externalId`: the item's ID in the app it came from.
    #[serde(default)]
    pub external_id: Option<String>,
}

/// The fields of a link to change; `None` leaves one alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkEdit {
    /// Its number from 1 or its ID; `None` is the task's only link.
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
}

/// Which open tasks a bulk change means, in place of naming them
/// (`--overdue`, `--due-before`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSelect {
    /// Open tasks due before this day, `YYYY-MM-DD` (local); `--overdue`
    /// is before today.
    pub due_before: String,
    /// Only tasks in this folder's lists.
    #[serde(default)]
    pub folder: Option<String>,
}

/// The fields `tasks edit` changes; `None` leaves a field alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdit {
    #[serde(default)]
    pub title: Option<String>,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub due: Option<Clearable<String>>,
    #[serde(default)]
    pub importance: Option<Importance>,
    /// `YYYY-MM-DDTHH:MM`, local time.
    #[serde(default)]
    pub reminder: Option<Clearable<String>>,
    #[serde(default)]
    pub body: Option<String>,
    /// Who the task waits on (`NewTask.assignee`). Setting one makes an
    /// open task `waitingOnOthers`; clearing it makes a task ms-todo made
    /// `waitingOnOthers` `notStarted` again, unless its status has
    /// changed since. `keep_status` leaves the status alone either way.
    #[serde(default)]
    pub assignee: Option<Clearable<String>>,
    #[serde(default)]
    pub keep_status: bool,
    /// Nag every this many minutes (`NewTask.nag`), or stop nagging. A
    /// task needs a reminder to nag, so setting one on a task without
    /// one is refused.
    #[serde(default)]
    pub nag: Option<Clearable<u32>>,
    /// `YYYY-MM-DD`. Microsoft To Do sets the due date to it too when the
    /// task has none (S11).
    #[serde(default)]
    pub start: Option<Clearable<String>>,
    /// Graph's `patternedRecurrence` as `NewTask.recurrence` has it; its
    /// `range.startDate` becomes the due date.
    #[serde(default)]
    pub recurrence: Option<Clearable<Value>>,
    /// The task's categories, replacing those it has; empty clears them.
    #[serde(default)]
    pub categories: Option<Vec<String>>,
    /// `YYYY-MM-DD`: hide the task until then (`NewTask.defer_until`).
    #[serde(default)]
    pub defer_until: Option<Clearable<String>>,
    /// Park it as Someday, or take it out.
    #[serde(default)]
    pub someday: Option<bool>,
}

/// A field an edit either sets or clears.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clearable<T> {
    Set(T),
    Clear,
}
