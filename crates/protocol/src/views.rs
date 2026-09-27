//! What a client shows: a [`Scope`], the [`Seed`] it starts from, My Day,
//! and how many tasks each view holds.

use serde::{Deserialize, Serialize};

use crate::{Entity, OutboxDepth, SyncActivity, SyncInfo};

/// My Day: today's tasks and what's suggested for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MyDay {
    /// My Day's day, `YYYY-MM-DD`: today, or yesterday before
    /// `my_day.rollover_time`.
    pub date: String,
    /// The tasks in today's My Day, open ones first.
    pub tasks: Vec<Entity>,
    /// Open tasks not in it that could be: each has `suggestion`, why
    /// (`due_today`, `overdue` or `left_over`, from an earlier My Day the
    /// rollover took it out of), and `list`, its list's name.
    pub suggestions: Vec<Entity>,
    /// Every list's sync state, as a view over every list has it.
    pub sync: SyncInfo,
    /// The day the last rollover ran for, `YYYY-MM-DD`.
    #[serde(default)]
    pub last_rollover: Option<String>,
}

/// What a client shows: a smart view over every list, or one list.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum Scope {
    /// The tasks in today's My Day, open ones first.
    MyDay,
    /// Open tasks marked important.
    Important,
    /// Open tasks with a due date, soonest first.
    Planned,
    /// Every open task.
    All,
    /// Every completed task, most recently completed first.
    Completed,
    /// Open tasks with an assignee, grouped by person.
    Assigned,
    /// What to do now: the top open tasks by `next`'s urgency order,
    /// each with `why`.
    Next,
    /// Open tasks deferred to a later day, by that day.
    Upcoming,
    /// Open tasks parked as Someday.
    Someday,
    /// One list's tasks, open and completed, by its local ID or name.
    List { id: String },
    #[serde(other)]
    Unknown,
}

impl Scope {
    /// Whether the scope leaves out deferred and Someday tasks (rung 9a):
    /// every list and view but those that are about them, or where a task
    /// was put by hand (My Day).
    pub fn hides_deferred(&self) -> bool {
        !matches!(
            self,
            Self::MyDay | Self::Next | Self::Upcoming | Self::Someday
        )
    }
}

/// The answer to `Seed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seed {
    /// The scope answered, a list always by its local ID. `None` only for
    /// the default list before the lists have synced once, when nobody
    /// knows which list that is yet.
    pub scope: Option<Scope>,
    /// Every live list, as `ListLists` gives them.
    pub lists: Vec<Entity>,
    /// The lists' own sync state.
    pub lists_sync: SyncInfo,
    pub counts: Counts,
    /// The scope's tasks, as `ListTasks` gives them; best match first with
    /// `search`.
    pub tasks: Vec<Entity>,
    /// The scope's sync state: `initial` until it has synced once, so an
    /// empty `tasks` isn't mistaken for an empty list.
    pub sync: SyncInfo,
    pub activity: SyncActivity,
    pub outbox: OutboxDepth,
    /// For `Scope::MyDay`: its day and suggestions; `None` for any other
    /// scope.
    #[serde(default)]
    pub my_day: Option<MyDaySeed>,
}

/// What the My Day view shows besides its tasks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MyDaySeed {
    /// `YYYY-MM-DD`, as `MyDay.date`.
    pub date: String,
    /// As `MyDay.suggestions`.
    pub suggestions: Vec<Entity>,
}

/// How many tasks each smart view and each list holds. Every count but
/// My Day's, Upcoming's and Someday's leaves out deferred and Someday
/// tasks, as the views do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Open tasks in today's My Day.
    #[serde(default)]
    pub my_day: u64,
    pub important: u64,
    pub planned: u64,
    pub all: u64,
    pub completed: u64,
    /// Open tasks with an assignee.
    #[serde(default)]
    pub assigned: u64,
    /// Open tasks deferred to a later day (not Someday).
    #[serde(default)]
    pub upcoming: u64,
    /// Open tasks parked as Someday.
    #[serde(default)]
    pub someday: u64,
    /// Open tasks by list local ID; a list with none is absent.
    #[serde(default)]
    pub lists: std::collections::BTreeMap<String, u64>,
}
