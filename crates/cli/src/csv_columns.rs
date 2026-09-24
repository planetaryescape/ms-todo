//! `--format csv` columns: one fixed set per entity type, in a fixed order,
//! so the header row is the schema. Arrays are joined with `;`, and dates
//! are ISO 8601: due dates as local `YYYY-MM-DD` (S11's rounding rule),
//! instants as Graph gives them.
//!
//! A task's body isn't a column: it's free text or HTML that often spans
//! many lines, which breaks the row-per-record use CSV is for (sort, grep,
//! spreadsheets). `--format json` has it.

use ms_todo_core::{DATE_FORMAT, local_due_date, parse_graph_date_time};
use ms_todo_protocol::Entity;
use serde_json::Value;

pub const TASK_COLUMNS: &[&str] = &[
    "id",
    "title",
    "status",
    "importance",
    "due",
    "reminder",
    "categories",
    "created",
    "modified",
    "sync_state",
];

pub const LIST_COLUMNS: &[&str] = &["id", "name", "wellknown", "is_owner", "is_shared"];

pub fn task_row(task: &Entity) -> Vec<String> {
    vec![
        text(task, "id").to_owned(),
        text(task, "title").to_owned(),
        text(task, "status").to_owned(),
        text(task, "importance").to_owned(),
        local_due(task),
        reminder(task),
        task.get("categories")
            .and_then(Value::as_array)
            .map(|categories| {
                categories
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(";")
            })
            .unwrap_or_default(),
        text(task, "createdDateTime").to_owned(),
        text(task, "lastModifiedDateTime").to_owned(),
        text(task, "sync_state").to_owned(),
    ]
}

pub fn list_row(list: &Entity) -> Vec<String> {
    vec![
        text(list, "id").to_owned(),
        text(list, "displayName").to_owned(),
        text(list, "wellknownListName").to_owned(),
        boolean(list, "isOwner"),
        boolean(list, "isShared"),
    ]
}

/// The due date as `YYYY-MM-DD` in the local zone, or empty. Graph returns
/// midnight in the writer's zone as UTC (S11), so the date part alone can be
/// the day before.
pub fn local_due(task: &Entity) -> String {
    task.get("dueDateTime")
        .and_then(|due| {
            local_due_date(
                due.get("dateTime")?.as_str()?,
                due.get("timeZone")?.as_str()?,
            )
        })
        .map(|date| date.format(DATE_FORMAT).to_string())
        .unwrap_or_default()
}

// The reminder keeps its time (S11). Graph returns it in UTC unless asked
// for another zone, so it's written with its offset.
fn reminder(task: &Entity) -> String {
    if task.get("isReminderOn") != Some(&Value::Bool(true)) {
        return String::new();
    }
    let Some(reminder) = task.get("reminderDateTime") else {
        return String::new();
    };
    let at = reminder
        .get("dateTime")
        .and_then(Value::as_str)
        .and_then(parse_graph_date_time);
    let zone = reminder.get("timeZone").and_then(Value::as_str);
    match (at, zone) {
        (Some(at), Some("UTC")) => at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        (Some(at), _) => at.format("%Y-%m-%dT%H:%M:%S").to_string(),
        (None, _) => String::new(),
    }
}

/// A string property of a Graph entity, or empty.
pub fn text<'a>(entity: &'a Entity, key: &str) -> &'a str {
    entity.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn boolean(entity: &Entity, key: &str) -> String {
    entity
        .get(key)
        .and_then(Value::as_bool)
        .map(|value| value.to_string())
        .unwrap_or_default()
}
