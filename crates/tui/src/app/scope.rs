//! What the sidebar offers, which tasks belong where, and how a scope's
//! tasks are ordered and grouped.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, Duration, NaiveDate};
use ms_todo_protocol::{Entity, Scope};
use serde_json::Value;

use super::task::Task;

/// A list as the sidebar knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarList {
    pub id: String,
    pub name: String,
    /// Its folder's name, if it's in one.
    pub folder: Option<String>,
    /// The default "Tasks" list, the inbox.
    pub default: bool,
}

impl SidebarList {
    /// A list as the daemon sends it: `id`, `displayName` and `folder`.
    pub fn from_entity(list: &Entity) -> Option<Self> {
        Some(Self {
            id: list.get("id").and_then(Value::as_str)?.to_owned(),
            name: list.get("displayName").and_then(Value::as_str)?.to_owned(),
            folder: list
                .get("folder")
                .and_then(Value::as_str)
                .map(str::to_owned),
            default: list.get("wellknownListName").and_then(Value::as_str) == Some("defaultList"),
        })
    }
}

/// Every folder `lists` are in, in the order they first come: the
/// daemon's folder order.
pub fn folder_names(lists: &[SidebarList]) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for folder in lists.iter().filter_map(|list| list.folder.as_deref()) {
        if !names.contains(&folder) {
            names.push(folder);
        }
    }
    names
}

/// A sidebar row: a smart view, a folder, or a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    View(Scope),
    /// A folder's heading; its lists follow it unless it's collapsed.
    Folder {
        name: String,
        collapsed: bool,
        /// Open tasks in all its lists.
        count: u64,
    },
    List {
        id: String,
        name: String,
        /// Whether it's drawn under a folder's heading.
        in_folder: bool,
    },
}

impl Entry {
    /// What choosing the row shows; a folder's heading shows nothing.
    pub fn scope(&self) -> Option<Scope> {
        match self {
            Self::View(scope) => Some(scope.clone()),
            Self::Folder { .. } => None,
            Self::List { id, .. } => Some(Scope::List { id: id.clone() }),
        }
    }

    /// Whether `other` is the same row, whatever its count or state.
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::View(a), Self::View(b)) => a == b,
            (Self::Folder { name: a, .. }, Self::Folder { name: b, .. }) => a == b,
            (Self::List { id: a, .. }, Self::List { id: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// The sidebar's rows (docs/blueprint/08-tui.md#layout): the smart views,
/// then each folder with its lists under it unless it's `collapsed`, then
/// the lists in no folder. `lists` come in the daemon's order, folder by
/// folder; `counts` are open tasks by list.
pub fn sidebar_entries(
    lists: &[SidebarList],
    collapsed: &HashSet<String>,
    counts: &BTreeMap<String, u64>,
) -> Vec<Entry> {
    let mut entries: Vec<Entry> = VIEWS.iter().cloned().map(Entry::View).collect();
    let row = |list: &SidebarList| Entry::List {
        id: list.id.clone(),
        name: list.name.clone(),
        in_folder: list.folder.is_some(),
    };
    for folder in folder_names(lists) {
        let members = lists
            .iter()
            .filter(|list| list.folder.as_deref() == Some(folder));
        let is_collapsed = collapsed.contains(folder);
        entries.push(Entry::Folder {
            name: folder.to_owned(),
            collapsed: is_collapsed,
            count: members
                .clone()
                .map(|list| counts.get(&list.id).copied().unwrap_or(0))
                .sum(),
        });
        if !is_collapsed {
            entries.extend(members.map(row));
        }
    }
    entries.extend(lists.iter().filter(|list| list.folder.is_none()).map(row));
    entries
}

/// The smart views, in sidebar order (docs/blueprint/08-tui.md#layout).
pub const VIEWS: [Scope; 6] = [
    Scope::MyDay,
    Scope::Important,
    Scope::Planned,
    Scope::All,
    Scope::Assigned,
    Scope::Completed,
];

pub fn view_name(scope: &Scope) -> &'static str {
    match scope {
        Scope::MyDay => "My Day",
        Scope::Important => "Important",
        Scope::Planned => "Planned",
        Scope::All => "All",
        Scope::Completed => "Completed",
        Scope::Assigned => "Assigned",
        Scope::List { .. } | Scope::Unknown => "",
    }
}

/// Whether `task`, as it is now, is in `scope`; `my_day` is My Day's day.
/// The My Day view also holds its suggestions while they're open and out
/// of it.
pub fn belongs(scope: &Scope, task: &Task, my_day: NaiveDate) -> bool {
    match scope {
        Scope::MyDay if task.suggestion.is_some() => !task.completed && task.my_day != Some(my_day),
        Scope::MyDay => task.my_day == Some(my_day),
        Scope::Important => task.important() && !task.completed,
        Scope::Planned => task.due.is_some() && !task.completed,
        Scope::All => !task.completed,
        Scope::Completed => task.completed,
        Scope::Assigned => task.assignee.is_some() && !task.completed,
        Scope::List { id } => task.list_id == *id,
        Scope::Unknown => false,
    }
}

/// A list shows its open tasks in the cache's order, then its completed
/// ones, most recently completed first, as To Do does. Views keep the
/// daemon's order, and so does a filtered list: best match first.
pub fn order(scope: &Scope, filtered: bool, tasks: &mut [Task]) {
    if !filtered && *scope == Scope::MyDay {
        // Today's tasks, open ones first, then the suggestions. Stable,
        // so each keeps the daemon's order within.
        tasks.sort_by_key(|task| (task.suggestion.is_some(), task.completed));
        return;
    }
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

/// The My Day view's groups: its tasks, then "Suggestions", as indexes
/// into `tasks`; a group with no tasks is left out.
pub fn my_day_groups(tasks: &[Task]) -> Vec<(String, Vec<usize>)> {
    let (suggested, planned): (Vec<usize>, Vec<usize>) =
        (0..tasks.len()).partition(|&index| tasks[index].suggestion.is_some());
    [("Today", planned), ("Suggestions", suggested)]
        .into_iter()
        .filter(|(_, members)| !members.is_empty())
        .map(|(heading, members)| (heading.to_owned(), members))
        .collect()
}

/// The Assigned view's groups: each person's tasks under their name, as
/// indexes into `tasks`, which the daemon sorts by person. Names that
/// differ only in case are one person, under the spelling seen first.
pub fn assigned_groups(tasks: &[Task]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    // Each name folded once: drawn every frame.
    let mut by_person: HashMap<String, usize> = HashMap::new();
    for (index, task) in tasks.iter().enumerate() {
        let Some(name) = &task.assignee else {
            continue;
        };
        let at = *by_person.entry(name.to_lowercase()).or_insert_with(|| {
            groups.push((name.clone(), Vec::new()));
            groups.len() - 1
        });
        groups[at].1.push(index);
    }
    groups
}

/// The Completed view's groups, newest day first: each day's heading
/// ("Today", "Yesterday", "Mon 21 Sep") with its tasks, as indexes into
/// `tasks`, which the daemon sorts newest first. A completion Graph hasn't
/// answered yet has no day, and the daemon puts it first.
pub fn completed_groups(tasks: &[Task], today: NaiveDate) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(Option<NaiveDate>, Vec<usize>)> = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        match groups.last_mut() {
            Some((day, members)) if *day == task.completed_on => members.push(index),
            _ => groups.push((task.completed_on, vec![index])),
        }
    }
    groups
        .into_iter()
        .map(|(day, members)| (ms_todo_core::completion_heading(day, today), members))
        .collect()
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

    fn completed(id: &str, day: Option<&str>) -> Task {
        let mut entity = serde_json::json!({ "id": id, "status": "completed" });
        if let Some(day) = day {
            entity["completedDateTime"] = serde_json::json!({ "dateTime": format!("{day}T00:00:00.0000000"), "timeZone": "UTC" });
        }
        Task::from_entity(entity.as_object().expect("object")).expect("task")
    }

    fn assigned(id: &str, who: &str) -> Task {
        let entity = serde_json::json!({ "id": id, "extensions": [{ "assignee": who }] });
        Task::from_entity(entity.as_object().expect("object")).expect("task")
    }

    #[test]
    fn assigned_tasks_group_by_person_whatever_the_case() {
        let tasks = [
            assigned("a", "Sam"),
            assigned("b", "sam"),
            assigned("c", "Ada"),
            assigned("d", "SAM"),
        ];
        assert_eq!(
            assigned_groups(&tasks),
            [
                ("Sam".to_owned(), vec![0, 1, 3]),
                ("Ada".to_owned(), vec![2])
            ]
        );
        let loose = assigned("e", "  Kim\u{1b}[31m ");
        assert_eq!(
            loose.assignee.as_deref(),
            Some("Kim[31m"),
            "trimmed, one safe line"
        );
    }

    #[test]
    fn completed_tasks_group_by_the_day_graph_kept() {
        let tasks = [
            completed("new", None),
            completed("a", Some("2026-09-24")),
            completed("b", Some("2026-09-24")),
            completed("c", Some("2026-09-23")),
            completed("d", Some("2026-09-21")),
        ];
        let groups = completed_groups(&tasks, date("2026-09-24"));
        let shown: Vec<(&str, &[usize])> = groups
            .iter()
            .map(|(heading, members)| (heading.as_str(), members.as_slice()))
            .collect();
        assert_eq!(
            shown,
            [
                ("Not synced yet", &[0][..]),
                ("Today", &[1, 2][..]),
                ("Yesterday", &[3][..]),
                ("Mon 21 Sep", &[4][..]),
            ]
        );
    }
}
