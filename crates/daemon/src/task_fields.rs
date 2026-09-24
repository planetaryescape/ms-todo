//! The task fields rung 2 writes, and how each becomes Graph JSON. Due dates
//! are dates only, written as midnight in the user's IANA zone (S11, D-027);
//! a time goes to the reminder, which sets `isReminderOn`.

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use ms_todo_core::{DATE_FORMAT, ErrorKind, local_date_time, local_due_date};
use ms_todo_protocol::{Clearable, Entity, ErrorPayload, Importance, NewTask, TaskEdit};
use serde_json::{Map, Value, json};

use crate::handlers::error_payload;

const REMINDER_FORMAT: &str = "%Y-%m-%dT%H:%M";

/// One field a mutation sets.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Field {
    Title(String),
    /// Graph's `status`: `completed` or `notStarted`.
    Status(&'static str),
    Importance(Importance),
    Due(Option<NaiveDate>),
    Reminder(Option<NaiveDateTime>),
    Body(String),
}

impl Field {
    /// The Graph properties this field writes, which a concurrent edit on
    /// the server may also have touched.
    pub fn graph_keys(&self) -> &'static [&'static str] {
        match self {
            Self::Title(_) => &["title"],
            Self::Status(_) => &["status"],
            Self::Importance(_) => &["importance"],
            Self::Due(_) => &["dueDateTime"],
            Self::Reminder(_) => &["isReminderOn", "reminderDateTime"],
            Self::Body(_) => &["body"],
        }
    }

    fn write(&self, body: &mut Map<String, Value>, zone: &str) {
        match self {
            Self::Title(title) => {
                body.insert("title".into(), json!(title));
            }
            Self::Status(status) => {
                body.insert("status".into(), json!(status));
            }
            Self::Importance(importance) => {
                body.insert("importance".into(), json!(importance));
            }
            Self::Due(due) => {
                let value = due.map_or(Value::Null, |date| {
                    date_time_time_zone(&format!("{}T00:00:00", date.format(DATE_FORMAT)), zone)
                });
                body.insert("dueDateTime".into(), value);
            }
            Self::Reminder(reminder) => {
                body.insert("isReminderOn".into(), json!(reminder.is_some()));
                let value = reminder.map_or(Value::Null, |at| {
                    date_time_time_zone(&at.format("%Y-%m-%dT%H:%M:00").to_string(), zone)
                });
                body.insert("reminderDateTime".into(), value);
            }
            Self::Body(text) => {
                body.insert(
                    "body".into(),
                    json!({ "content": text, "contentType": "text" }),
                );
            }
        }
    }

    /// Whether `task`, as Graph returned it, already holds this value.
    pub fn is_applied_to(&self, task: &Entity) -> bool {
        match self {
            Self::Title(title) => field(task, "title") == Some(title.as_str()),
            Self::Status(status) => field(task, "status") == Some(*status),
            Self::Importance(importance) => task.get("importance") == Some(&json!(importance)),
            Self::Due(due) => graph_due_date(task) == *due,
            Self::Reminder(None) => task.get("isReminderOn") != Some(&Value::Bool(true)),
            Self::Reminder(Some(at)) => {
                task.get("isReminderOn") == Some(&Value::Bool(true))
                    && graph_reminder(task) == Some(*at)
            }
            Self::Body(content) => {
                task.get("body")
                    .and_then(|body| body.get("content"))
                    .and_then(Value::as_str)
                    == Some(content.as_str())
            }
        }
    }
}

/// The JSON Graph gets for `fields`, in the user's `zone`.
pub(crate) fn graph_body(fields: &[Field], zone: &str) -> Value {
    let mut body = Map::new();
    for field in fields {
        field.write(&mut body, zone);
    }
    Value::Object(body)
}

pub(crate) fn new_task_fields(task: &NewTask) -> Result<Vec<Field>, ErrorPayload> {
    let mut fields = vec![Field::Title(title(&task.title)?)];
    if let Some(due) = &task.due {
        fields.push(Field::Due(Some(parse_due(due)?)));
    }
    if let Some(reminder) = &task.reminder {
        fields.push(Field::Reminder(Some(parse_reminder(reminder)?)));
    }
    if let Some(importance) = task.importance {
        fields.push(Field::Importance(importance));
    }
    if let Some(body) = &task.body {
        fields.push(Field::Body(body.clone()));
    }
    Ok(fields)
}

pub(crate) fn edit_fields(edit: &TaskEdit) -> Result<Vec<Field>, ErrorPayload> {
    let mut fields = Vec::new();
    if let Some(new_title) = &edit.title {
        fields.push(Field::Title(title(new_title)?));
    }
    match &edit.due {
        Some(Clearable::Set(due)) => fields.push(Field::Due(Some(parse_due(due)?))),
        Some(Clearable::Clear) => fields.push(Field::Due(None)),
        None => {}
    }
    if let Some(importance) = edit.importance {
        fields.push(Field::Importance(importance));
    }
    match &edit.reminder {
        Some(Clearable::Set(at)) => fields.push(Field::Reminder(Some(parse_reminder(at)?))),
        Some(Clearable::Clear) => fields.push(Field::Reminder(None)),
        None => {}
    }
    if let Some(body) = &edit.body {
        fields.push(Field::Body(body.clone()));
    }
    if fields.is_empty() {
        return Err(invalid(
            "nothing to change; pass at least one of --title, --due, --clear-due, \
             --importance, --reminder, --clear-reminder or --body"
                .into(),
        ));
    }
    Ok(fields)
}

/// The zone due dates and reminders are written in: `TZ` when it names a
/// zone, else the system's. The CLI starts the daemon, so both see the same
/// `TZ`. chrono's `Local`, which reads dates back, follows the same rule.
pub(crate) fn user_time_zone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        // POSIX allows a leading ':'; a path is a zone file, not a name.
        let name = tz.trim_start_matches(':');
        if !name.is_empty() && !name.starts_with('/') {
            return name.to_owned();
        }
    }
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

/// A task's due date as a local date (S11's round-to-midnight rule).
pub(crate) fn graph_due_date(task: &Entity) -> Option<NaiveDate> {
    let due = task.get("dueDateTime")?;
    local_due_date(
        due.get("dateTime")?.as_str()?,
        due.get("timeZone")?.as_str()?,
    )
}

// A reminder keeps its time (S11); Graph returns it in UTC.
fn graph_reminder(task: &Entity) -> Option<NaiveDateTime> {
    let reminder = task.get("reminderDateTime")?;
    local_date_time(
        reminder.get("dateTime")?.as_str()?,
        reminder.get("timeZone")?.as_str()?,
    )?
    .with_nanosecond(0)
}

/// A string property of a Graph entity.
fn field<'a>(entity: &'a Entity, name: &str) -> Option<&'a str> {
    entity.get(name).and_then(|value| value.as_str())
}

fn date_time_time_zone(date_time: &str, zone: &str) -> Value {
    json!({ "dateTime": date_time, "timeZone": zone })
}

fn title(title: &str) -> Result<String, ErrorPayload> {
    if title.trim().is_empty() {
        return Err(invalid("a task's title can't be empty".into()));
    }
    Ok(title.to_owned())
}

fn parse_due(value: &str) -> Result<NaiveDate, ErrorPayload> {
    NaiveDate::parse_from_str(value, DATE_FORMAT).map_err(|_| {
        invalid(format!(
            "invalid due date {value:?}: use YYYY-MM-DD. A due date has no time; \
             put a time in --reminder"
        ))
    })
}

fn parse_reminder(value: &str) -> Result<NaiveDateTime, ErrorPayload> {
    NaiveDateTime::parse_from_str(value, REMINDER_FORMAT).map_err(|_| {
        invalid(format!(
            "invalid reminder {value:?}: use YYYY-MM-DDTHH:MM, in local time"
        ))
    })
}

fn invalid(message: String) -> ErrorPayload {
    error_payload(ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn a_due_date_is_local_midnight_in_the_given_zone() {
        let body = graph_body(
            &[Field::Due(Some(parse_due("2026-09-26").expect("date")))],
            "Europe/London",
        );
        assert_eq!(
            body,
            json!({ "dueDateTime": { "dateTime": "2026-09-26T00:00:00", "timeZone": "Europe/London" } })
        );
    }

    #[test]
    fn a_reminder_turns_the_reminder_on_and_clearing_turns_it_off() {
        let at = parse_reminder("2026-09-26T17:30").expect("reminder");
        assert_eq!(
            graph_body(&[Field::Reminder(Some(at))], "Europe/London"),
            json!({
                "isReminderOn": true,
                "reminderDateTime": { "dateTime": "2026-09-26T17:30:00", "timeZone": "Europe/London" }
            })
        );
        assert_eq!(
            graph_body(&[Field::Reminder(None), Field::Due(None)], "UTC"),
            json!({ "isReminderOn": false, "reminderDateTime": null, "dueDateTime": null })
        );
    }

    #[test]
    fn a_due_date_with_a_time_is_refused_with_a_pointer_to_the_reminder() {
        let error = parse_due("2026-09-26T17:30").expect_err("time");
        assert_eq!(error.kind, "invalid_input");
        assert!(error.message.contains("--reminder"), "{}", error.message);
        assert!(parse_reminder("tomorrow").is_err());
    }

    #[test]
    fn an_edit_with_nothing_to_change_is_invalid() {
        let error = edit_fields(&TaskEdit::default()).expect_err("empty");
        assert_eq!(error.kind, "invalid_input");
        assert!(title("  ").is_err());
    }

    #[test]
    fn applied_values_are_recognised_in_graphs_shape() {
        let task = entity(json!({
            "title": "Buy milk",
            "status": "completed",
            "importance": "high",
            "isReminderOn": false,
            "dueDateTime": { "dateTime": "2026-09-26T00:00:00.0000000", "timeZone": "UTC" },
            "body": { "content": "2 pints", "contentType": "text" }
        }));
        for field in [
            Field::Title("Buy milk".into()),
            Field::Status("completed"),
            Field::Importance(Importance::High),
            Field::Due(parse_due("2026-09-26").ok()),
            Field::Reminder(None),
            Field::Body("2 pints".into()),
        ] {
            assert!(field.is_applied_to(&task), "{field:?}");
        }
        assert!(!Field::Status("notStarted").is_applied_to(&task));
        assert!(!Field::Due(None).is_applied_to(&task));
    }
}
