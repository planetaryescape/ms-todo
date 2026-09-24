//! The columns a Graph task or list fills, next to its `raw_json`
//! (docs/blueprint/02-data-model.md#principles). Due and start dates become
//! local dates by S11's round-to-midnight rule; reminder and completion
//! times become UTC instants.

use ms_todo_core::{DATE_FORMAT, local_due_date, parse_graph_date_time};
use serde_json::Value;

use crate::Entity;

pub(crate) struct TaskColumns {
    pub title: String,
    pub body_content: Option<String>,
    pub body_content_type: Option<String>,
    pub status: String,
    pub importance: String,
    pub is_reminder_on: bool,
    pub reminder_at_utc: Option<String>,
    pub due_date: Option<String>,
    pub start_date: Option<String>,
    pub completed_at_utc: Option<String>,
    pub recurrence_json: Option<String>,
    pub categories_json: String,
    pub has_attachments: bool,
    pub created_at: Option<String>,
    pub last_modified_at: Option<String>,
    pub etag: Option<String>,
}

impl TaskColumns {
    pub fn of(task: &Entity) -> Self {
        let body = task.get("body");
        Self {
            title: text(task, "title").unwrap_or_default(),
            body_content: body
                .and_then(|body| body.get("content"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            body_content_type: body
                .and_then(|body| body.get("contentType"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: text(task, "status").unwrap_or_else(|| "notStarted".into()),
            importance: text(task, "importance").unwrap_or_else(|| "normal".into()),
            is_reminder_on: flag(task, "isReminderOn"),
            reminder_at_utc: utc_instant(task.get("reminderDateTime")),
            due_date: local_date(task.get("dueDateTime")),
            start_date: local_date(task.get("startDateTime")),
            completed_at_utc: utc_instant(task.get("completedDateTime")),
            recurrence_json: task
                .get("recurrence")
                .filter(|value| value.is_object())
                .map(Value::to_string),
            categories_json: task
                .get("categories")
                .filter(|value| value.is_array())
                .map_or_else(|| "[]".to_owned(), Value::to_string),
            has_attachments: flag(task, "hasAttachments"),
            created_at: text(task, "createdDateTime"),
            last_modified_at: text(task, "lastModifiedDateTime"),
            etag: etag(task),
        }
    }
}

pub(crate) struct ListColumns {
    pub display_name: String,
    pub wellknown_list_name: Option<String>,
    pub is_owner: bool,
    pub is_shared: bool,
    pub etag: Option<String>,
}

impl ListColumns {
    pub fn of(list: &Entity) -> Self {
        Self {
            display_name: text(list, "displayName").unwrap_or_default(),
            wellknown_list_name: text(list, "wellknownListName"),
            is_owner: flag(list, "isOwner"),
            is_shared: flag(list, "isShared"),
            etag: etag(list),
        }
    }
}

pub(crate) fn text(entity: &Entity, key: &str) -> Option<String> {
    entity.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub(crate) fn etag(entity: &Entity) -> Option<String> {
    text(entity, "@odata.etag")
}

fn flag(entity: &Entity, key: &str) -> bool {
    entity.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn date_time_parts(value: Option<&Value>) -> Option<(&str, &str)> {
    let value = value?;
    Some((
        value.get("dateTime")?.as_str()?,
        value.get("timeZone")?.as_str()?,
    ))
}

fn local_date(value: Option<&Value>) -> Option<String> {
    let (date_time, zone) = date_time_parts(value)?;
    local_due_date(date_time, zone).map(|date| date.format(DATE_FORMAT).to_string())
}

// Graph returns these in UTC unless asked otherwise (no `Prefer:
// outlook.timezone` is ever sent). A value in another zone would need a zone
// database to convert; it stays in `raw_json` only.
fn utc_instant(value: Option<&Value>) -> Option<String> {
    let (date_time, zone) = date_time_parts(value)?;
    if !zone.eq_ignore_ascii_case("UTC") {
        return None;
    }
    parse_graph_date_time(date_time).map(|at| at.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_task_fills_its_columns_from_graphs_shape() {
        let task = json!({
            "@odata.etag": "W/\"e1\"",
            "title": "Buy milk",
            "status": "completed",
            "importance": "high",
            "isReminderOn": true,
            "reminderDateTime": { "dateTime": "2026-09-26T16:30:00.0000000", "timeZone": "UTC" },
            // Midnight in London during BST (S11).
            "dueDateTime": { "dateTime": "2026-09-25T23:00:00.0000000", "timeZone": "UTC" },
            "categories": ["Home"],
            "body": { "content": "2 pints", "contentType": "text" }
        });
        let columns = TaskColumns::of(task.as_object().expect("object"));
        assert_eq!(columns.title, "Buy milk");
        assert_eq!(columns.status, "completed");
        assert_eq!(
            columns.reminder_at_utc.as_deref(),
            Some("2026-09-26T16:30:00Z")
        );
        assert_eq!(columns.categories_json, r#"["Home"]"#);
        assert_eq!(columns.body_content.as_deref(), Some("2 pints"));
        assert_eq!(columns.etag.as_deref(), Some("W/\"e1\""));
        assert!(columns.due_date.is_some());
        assert_eq!(columns.start_date, None);
        assert_eq!(columns.recurrence_json, None);
    }
}
