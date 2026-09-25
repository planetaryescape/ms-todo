//! A task as the TUI shows it, read once from the daemon's entity (Graph's
//! JSON with ms-todo's `id`, `list_id` and `sync_state`), so drawing never
//! parses JSON.

use chrono::{NaiveDate, NaiveDateTime};
use ms_todo_core::{completion_date, local_date_time, local_due_date};
use ms_todo_protocol::{Entity, Importance};
use serde_json::Value;

/// Wider than any pane, so html notes aren't wrapped twice; the detail
/// pane wraps them to fit.
const NOTES_WIDTH: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMarker {
    Synced,
    /// A write waits to reach Microsoft To Do, or is being sent.
    Pending,
    /// A write's outcome is unknown: resolve it in `ms-todo outbox`.
    Unknown,
    /// Microsoft To Do rejected a write, which was rolled back.
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// ms-todo's local ID.
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub completed: bool,
    pub importance: Importance,
    /// Local date (S11).
    pub due: Option<NaiveDate>,
    /// Local time, when the reminder is on.
    pub reminder: Option<NaiveDateTime>,
    /// "every 2 weeks on Mon", for a recurring task.
    pub recurrence: Option<String>,
    /// The notes as Graph has them; [`Task::notes`] renders them.
    pub body: Option<Body>,
    /// `(checked, total)`; `None` when it has no steps.
    pub steps: Option<(usize, usize)>,
    pub categories: Vec<String>,
    pub sync: SyncMarker,
    /// Graph's `completedDateTime`, local, for ordering.
    pub completed_at: Option<NaiveDateTime>,
    /// The day it was completed: Graph's UTC date (S12), as `done` reads
    /// it; `None` until Graph has answered the completion.
    pub completed_on: Option<NaiveDate>,
}

impl Task {
    /// `None` for an entity with no `id`, which the daemon never sends.
    pub fn from_entity(entity: &Entity) -> Option<Self> {
        let text = |key: &str| entity.get(key).and_then(Value::as_str);
        let date_time = |key: &str| {
            let value = entity.get(key)?;
            Some((
                value.get("dateTime")?.as_str()?,
                value.get("timeZone")?.as_str()?,
            ))
        };
        let steps = entity
            .get("checklistItems")
            .and_then(Value::as_array)
            .filter(|steps| !steps.is_empty())
            .map(|steps| {
                let checked = steps
                    .iter()
                    .filter(|step| step.get("isChecked").and_then(Value::as_bool) == Some(true))
                    .count();
                (checked, steps.len())
            });
        Some(Self {
            id: text("id")?.to_owned(),
            list_id: text("list_id").unwrap_or_default().to_owned(),
            title: text("title").unwrap_or_default().to_owned(),
            completed: text("status") == Some("completed"),
            importance: match text("importance") {
                Some("high") => Importance::High,
                Some("low") => Importance::Low,
                _ => Importance::Normal,
            },
            due: date_time("dueDateTime").and_then(|(at, zone)| local_due_date(at, zone)),
            reminder: date_time("reminderDateTime")
                .filter(|_| entity.get("isReminderOn").and_then(Value::as_bool) == Some(true))
                .and_then(|(at, zone)| local_date_time(at, zone)),
            recurrence: entity.get("recurrence").and_then(describe_recurrence),
            body: entity.get("body").and_then(Body::of),
            steps,
            categories: entity
                .get("categories")
                .and_then(Value::as_array)
                .map(|all| {
                    all.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            sync: match text("sync_state") {
                Some("pending") => SyncMarker::Pending,
                Some("unknown") => SyncMarker::Unknown,
                Some("failed") => SyncMarker::Failed,
                _ => SyncMarker::Synced,
            },
            completed_at: date_time("completedDateTime")
                .and_then(|(at, zone)| local_date_time(at, zone)),
            completed_on: date_time("completedDateTime")
                .and_then(|(at, zone)| completion_date(at, zone)),
        })
    }
}

/// A task's notes, as Graph sends them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    pub content: String,
    pub html: bool,
}

impl Body {
    fn of(body: &Value) -> Option<Self> {
        let content = body.get("content")?.as_str()?;
        let html = body
            .get("contentType")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("html"));
        Some(Self {
            content: content.to_owned(),
            html,
        })
    }
}

impl Task {
    pub fn important(&self) -> bool {
        self.importance == Importance::High
    }

    /// The notes as plain text, html rendered; `None` when there are none.
    /// Rendered when drawn, for the selected task only: rendering every
    /// task's html on each seed cost more than the whole rest of a view
    /// switch.
    pub fn notes(&self) -> Option<String> {
        let body = self.body.as_ref()?;
        let text = if body.html {
            html2text::config::plain_no_decorate()
                .string_from_read(body.content.as_bytes(), NOTES_WIDTH)
                .unwrap_or_else(|_| body.content.clone())
        } else {
            body.content.clone()
        };
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

/// Graph's `patternedRecurrence` in a few words: "daily", "every 2 weeks
/// on Mon, Thu", "monthly on day 1".
fn describe_recurrence(recurrence: &Value) -> Option<String> {
    let pattern = recurrence.get("pattern")?;
    let kind = pattern.get("type")?.as_str()?;
    let interval = pattern.get("interval").and_then(Value::as_u64).unwrap_or(1);
    let unit = match kind {
        "daily" => "day",
        "weekly" => "week",
        "absoluteMonthly" | "relativeMonthly" => "month",
        "absoluteYearly" | "relativeYearly" => "year",
        _ => return Some("repeats".into()),
    };
    let mut described = match (interval, unit) {
        (1, "day") => "daily".to_owned(),
        (1, unit) => format!("{unit}ly"),
        (n, unit) => format!("every {n} {unit}s"),
    };
    let days: Vec<String> = pattern
        .get("daysOfWeek")
        .and_then(Value::as_array)
        .map(|days| {
            days.iter()
                .filter_map(Value::as_str)
                .map(|day| {
                    let mut short: String = day.chars().take(3).collect();
                    if let Some(first) = short.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    short
                })
                .collect()
        })
        .unwrap_or_default();
    if matches!(kind, "weekly") && !days.is_empty() {
        described.push_str(" on ");
        described.push_str(&days.join(", "));
    } else if let Some(day) = pattern
        .get("dayOfMonth")
        .and_then(Value::as_u64)
        .filter(|day| *day > 0 && kind.starts_with("absolute"))
    {
        described.push_str(&format!(" on day {day}"));
    }
    Some(described)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn task(value: Value) -> Task {
        let mut entity = json!({ "id": "t1", "list_id": "l1", "title": "Pay rent" });
        if let (Some(entity), Some(extra)) = (entity.as_object_mut(), value.as_object()) {
            entity.extend(extra.clone());
        }
        Task::from_entity(entity.as_object().expect("object")).expect("task")
    }

    #[test]
    fn fields_are_read_from_graphs_shape() {
        let task = task(json!({
            "status": "completed",
            "importance": "high",
            "dueDateTime": { "dateTime": "2026-10-01T00:00:00.0000000", "timeZone": "Europe/London" },
            "isReminderOn": true,
            "reminderDateTime": { "dateTime": "2026-10-01T09:00:00.0000000", "timeZone": "Europe/London" },
            "body": { "content": "<p>Call <b>Sam</b></p>", "contentType": "html" },
            "checklistItems": [{ "isChecked": true }, { "isChecked": false }],
            "categories": ["Home"],
            "sync_state": "unknown",
        }));
        assert!(task.completed && task.important());
        assert_eq!(
            task.due.map(|due| due.to_string()).as_deref(),
            Some("2026-10-01")
        );
        assert_eq!(
            task.reminder.map(|at| at.to_string()).as_deref(),
            Some("2026-10-01 09:00:00")
        );
        assert_eq!(task.notes().as_deref(), Some("Call Sam"));
        assert_eq!(task.steps, Some((1, 2)));
        assert_eq!(task.categories, ["Home"]);
        assert_eq!(task.sync, SyncMarker::Unknown);
    }

    #[test]
    fn a_reminder_that_is_off_and_empty_notes_are_absent() {
        let task = task(json!({
            "isReminderOn": false,
            "reminderDateTime": { "dateTime": "2026-10-01T09:00:00.0000000", "timeZone": "UTC" },
            "body": { "content": "", "contentType": "text" },
        }));
        assert_eq!(task.reminder, None);
        assert_eq!(task.notes(), None);
        assert_eq!(task.steps, None);
        assert_eq!(task.sync, SyncMarker::Synced);
    }

    #[test]
    fn recurrence_reads_as_words() {
        let describe = |pattern: Value| describe_recurrence(&json!({ "pattern": pattern }));
        assert_eq!(
            describe(json!({ "type": "daily", "interval": 1 })).as_deref(),
            Some("daily")
        );
        assert_eq!(
            describe(
                json!({ "type": "weekly", "interval": 2, "daysOfWeek": ["monday", "thursday"] })
            )
            .as_deref(),
            Some("every 2 weeks on Mon, Thu")
        );
        assert_eq!(
            describe(json!({ "type": "absoluteMonthly", "interval": 1, "dayOfMonth": 1 }))
                .as_deref(),
            Some("monthly on day 1")
        );
    }
}
