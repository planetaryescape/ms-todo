//! The task fields rung 2 writes, and how each becomes Graph JSON. Due dates
//! are dates only, written as midnight in the user's IANA zone (S11, D-027);
//! a time goes to the reminder, which sets `isReminderOn`.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use ms_todo_core::{
    DATE_FORMAT, ErrorKind, REMINDER_FORMAT, completion_date, local_due_date, parse_graph_date_time,
};
use ms_todo_protocol::{Clearable, Entity, ErrorPayload, Importance, NewTask, TaskEdit};
use serde_json::{Map, Value, json};

use crate::handlers::error_payload;

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
    /// A field that makes sense for one task only: several tasks never
    /// get one title or one set of notes.
    pub fn one_task_only(&self) -> bool {
        matches!(self, Self::Title(_) | Self::Body(_))
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
                let value = due.map_or(Value::Null, |date| midnight(date, zone));
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
    graph_date(task, "dueDateTime", local_due_date)
}

/// The day a task was completed: Graph's UTC date (S12; see
/// `ms_todo_core::completion_date`). `None` for a completion Graph hasn't
/// answered yet.
pub(crate) fn graph_completion_date(task: &Entity) -> Option<NaiveDate> {
    graph_date(task, "completedDateTime", completion_date)
}

pub(crate) fn graph_date(
    task: &Entity,
    key: &str,
    read: fn(&str, &str) -> Option<NaiveDate>,
) -> Option<NaiveDate> {
    let value = task.get(key)?;
    read(
        value.get("dateTime")?.as_str()?,
        value.get("timeZone")?.as_str()?,
    )
}

/// A day as the daemon receives it, `YYYY-MM-DD`: `done`'s window and a
/// bulk change's `due_before`.
pub(crate) fn parse_day(value: &str) -> Result<NaiveDate, ErrorPayload> {
    NaiveDate::parse_from_str(value, DATE_FORMAT)
        .map_err(|_| invalid(format!("invalid day {value:?}: use YYYY-MM-DD")))
}

/// A task's date-only fields: Graph keeps only the date part of what it's
/// sent, in the zone sent (S11).
pub(crate) const DATE_ONLY: [&str; 2] = ["dueDateTime", "startDateTime"];

/// Everything a task POST can set that a task read from Graph has: what a
/// re-created or copied task gets back.
const CREATABLE: [&str; 11] = [
    "title",
    "body",
    "importance",
    "status",
    "isReminderOn",
    "reminderDateTime",
    "dueDateTime",
    "startDateTime",
    "completedDateTime",
    "categories",
    "recurrence",
];

/// The fields of `task`, Graph's JSON, that a create can set, each as it's
/// written back ([`as_written`]); nulls left out.
pub(crate) fn creatable_fields(task: &Entity) -> Map<String, Value> {
    CREATABLE
        .iter()
        .filter_map(|&key| {
            let value = task.get(key).filter(|value| !value.is_null())?;
            Some((key.to_owned(), as_written(key, value.clone())))
        })
        .collect()
}

/// `value`, as Graph gave it for the field `key`, as it's written back.
/// Graph gives a date as an instant in UTC, midnight London in summer
/// being 23:00 the day before, and keeps only the date part of what it's
/// sent, so sending back what it gave would move the date a day earlier.
/// Such a date is written as its local date at midnight in the user's
/// zone, as a new due date is. One already at midnight in its own zone,
/// as Graph re-bases recurring tasks (S12), and any other field, go back
/// as they came.
pub(crate) fn as_written(key: &str, value: Value) -> Value {
    if !DATE_ONLY.contains(&key) {
        return value;
    }
    let Some((at, zone)) = value
        .get("dateTime")
        .and_then(Value::as_str)
        .zip(value.get("timeZone").and_then(Value::as_str))
    else {
        return value;
    };
    if parse_graph_date_time(at).is_some_and(|at| at.time() == NaiveTime::MIN) {
        return value;
    }
    match local_due_date(at, zone) {
        Some(date) => midnight(date, &user_time_zone()),
        None => value,
    }
}

/// A date-only field's value: midnight of `date` in `zone` (D-027).
pub(crate) fn midnight(date: NaiveDate, zone: &str) -> Value {
    date_time_time_zone(&format!("{}T00:00:00", date.format(DATE_FORMAT)), zone)
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
    fn a_date_read_from_graph_is_written_back_as_its_local_day() {
        // Midnight London in summer, as Graph reads it back. Wherever the
        // tests run, within 12 hours of UTC, it's the 24th.
        let read = json!({ "dateTime": "2026-09-23T23:00:00.0000000", "timeZone": "UTC" });
        assert_eq!(
            as_written("dueDateTime", read.clone()),
            json!({ "dateTime": "2026-09-24T00:00:00", "timeZone": user_time_zone() })
        );
        // Already midnight in its own zone: its date part is the day.
        let midnight = json!({ "dateTime": "2026-09-24T00:00:00.0000000", "timeZone": "UTC" });
        assert_eq!(as_written("dueDateTime", midnight.clone()), midnight);
        assert_eq!(as_written("dueDateTime", Value::Null), Value::Null);
        assert_eq!(as_written("reminderDateTime", read.clone()), read);
    }

    #[test]
    fn an_edit_with_nothing_to_change_is_invalid() {
        let error = edit_fields(&TaskEdit::default()).expect_err("empty");
        assert_eq!(error.kind, "invalid_input");
        assert!(title("  ").is_err());
    }
}
