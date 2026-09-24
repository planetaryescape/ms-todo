//! What the sidebar offers, which tasks belong where, and how a scope's
//! tasks are ordered and grouped.

use chrono::{Datelike, Duration, NaiveDate};
use ms_todo_protocol::Scope;

use super::task::Task;

/// A sidebar row: a smart view, or a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    View(Scope),
    List { id: String, name: String },
}

impl Entry {
    pub fn scope(&self) -> Scope {
        match self {
            Self::View(scope) => scope.clone(),
            Self::List { id, .. } => Scope::List { id: id.clone() },
        }
    }
}

/// The smart views, in sidebar order (docs/blueprint/08-tui.md#layout;
/// My Day and Assigned come in later rungs).
pub const VIEWS: [Scope; 4] = [
    Scope::Important,
    Scope::Planned,
    Scope::All,
    Scope::Completed,
];

pub fn view_name(scope: &Scope) -> &'static str {
    match scope {
        Scope::Important => "Important",
        Scope::Planned => "Planned",
        Scope::All => "All",
        Scope::Completed => "Completed",
        Scope::List { .. } | Scope::Unknown => "",
    }
}

/// Whether `task`, as it is now, is in `scope`.
pub fn belongs(scope: &Scope, task: &Task) -> bool {
    match scope {
        Scope::Important => task.important && !task.completed,
        Scope::Planned => task.due.is_some() && !task.completed,
        Scope::All => !task.completed,
        Scope::Completed => task.completed,
        Scope::List { id } => task.list_id == *id,
        Scope::Unknown => false,
    }
}

/// A list shows its open tasks in the cache's order, then its completed
/// ones, most recently completed first, as To Do does. Views keep the
/// daemon's order, and so does a filtered list: best match first.
pub fn order(scope: &Scope, filtered: bool, tasks: &mut [Task]) {
    if filtered || !matches!(scope, Scope::List { .. }) {
        return;
    }
    // Stable, so open tasks keep their order. A task completed here but
    // not yet answered by Graph has no completion time: it's the newest.
    let recency = |task: &Task| (task.completed_at.is_none(), task.completed_at);
    tasks.sort_by(|a, b| match (a.completed, b.completed) {
        (false, false) => std::cmp::Ordering::Equal,
        (true, true) => recency(b).cmp(&recency(a)),
        (a_done, b_done) => a_done.cmp(&b_done),
    });
}

/// A group of the Planned view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DueGroup {
    Overdue,
    Today,
    Tomorrow,
    /// After tomorrow, up to Sunday.
    ThisWeek,
    Later,
}

impl DueGroup {
    pub fn of(due: NaiveDate, today: NaiveDate) -> Self {
        let days_left_in_week = i64::from(6 - today.weekday().num_days_from_monday());
        match due {
            due if due < today => Self::Overdue,
            due if due == today => Self::Today,
            due if due == today + Duration::days(1) => Self::Tomorrow,
            due if due <= today + Duration::days(days_left_in_week) => Self::ThisWeek,
            _ => Self::Later,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Overdue => "Overdue",
            Self::Today => "Today",
            Self::Tomorrow => "Tomorrow",
            Self::ThisWeek => "This week",
            Self::Later => "Later",
        }
    }
}

/// The Planned view's groups: each group with the tasks in it, soonest
/// first, as indexes into `tasks` (which the daemon sorts by due date).
pub fn planned_groups(tasks: &[Task], today: NaiveDate) -> Vec<(DueGroup, Vec<usize>)> {
    let mut groups: Vec<(DueGroup, Vec<usize>)> = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        let Some(due) = task.due else {
            continue;
        };
        let group = DueGroup::of(due, today);
        match groups.iter_mut().find(|(existing, _)| *existing == group) {
            Some((_, members)) => members.push(index),
            None => groups.push((group, vec![index])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date")
    }

    #[test]
    fn due_dates_group_by_how_soon_they_are() {
        // Thursday 24 September 2026.
        let today = date("2026-09-24");
        let group = |due: &str| DueGroup::of(date(due), today);
        assert_eq!(group("2026-09-23"), DueGroup::Overdue);
        assert_eq!(group("2026-09-24"), DueGroup::Today);
        assert_eq!(group("2026-09-25"), DueGroup::Tomorrow);
        assert_eq!(group("2026-09-27"), DueGroup::ThisWeek, "Sunday");
        assert_eq!(group("2026-09-28"), DueGroup::Later, "next Monday");
    }
}
