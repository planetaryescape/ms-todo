//! Moving a recurring task's due date without splitting it (S20, D-069).
//! Graph answers a PATCH of `dueDateTime` alone on a recurring task by
//! moving the series on and creating a new open recurring task with that
//! date. Sent with the recurrence cleared, the date is an ordinary write;
//! a second PATCH then sets the recurrence again, its range starting on
//! the new date (with the old start, Graph moves the task back to the
//! first occurrence on or after it).

use ms_todo_core::{DATE_FORMAT, local_due_date};
use ms_todo_store::Entity;
use serde_json::{Value, json};

/// For a PATCH `body` that sets a due date on `task`, a recurring task,
/// the two bodies to send instead: the date with `recurrence: null`, then
/// the recurrence starting on that date. `None` when `body` doesn't need
/// it: no due date in it, one that clears it, a recurrence of its own, or
/// a task that doesn't recur.
pub(super) fn keep(body: &Value, task: &Entity) -> Option<(Value, Value)> {
    let fields = body.as_object()?;
    if fields.contains_key("recurrence") {
        return None;
    }
    let due = fields.get("dueDateTime").filter(|due| !due.is_null())?;
    let mut recurrence = task
        .get("recurrence")
        .filter(|value| value.is_object())?
        .clone();
    let day = local_due_date(
        due.get("dateTime")?.as_str()?,
        due.get("timeZone")?.as_str()?,
    )?;
    recurrence["range"]["startDate"] = json!(day.format(DATE_FORMAT).to_string());
    let mut first = body.clone();
    first["recurrence"] = Value::Null;
    Some((first, json!({ "recurrence": recurrence })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daily() -> Entity {
        json!({
            "dueDateTime": { "dateTime": "2026-09-26T23:00:00.0000000", "timeZone": "UTC" },
            "recurrence": {
                "pattern": { "type": "daily", "interval": 1 },
                "range": { "type": "noEnd", "startDate": "2026-09-27", "recurrenceTimeZone": "UTC" }
            }
        })
        .as_object()
        .cloned()
        .expect("object")
    }

    #[test]
    fn a_new_due_date_clears_the_recurrence_then_sets_it_from_that_day() {
        let body = json!({
            "title": "Water plants",
            "dueDateTime": { "dateTime": "2026-09-30T00:00:00", "timeZone": "Europe/London" }
        });
        let (first, second) = keep(&body, &daily()).expect("recurring");
        assert_eq!(first["recurrence"], Value::Null);
        assert_eq!(first["title"], "Water plants");
        assert_eq!(first["dueDateTime"], body["dueDateTime"]);
        assert_eq!(second["recurrence"]["range"]["startDate"], "2026-09-30");
        assert_eq!(second["recurrence"]["pattern"]["type"], "daily");
    }

    #[test]
    fn anything_else_is_sent_as_it_is() {
        let task = daily();
        assert!(keep(&json!({ "title": "x" }), &task).is_none());
        assert!(keep(&json!({ "dueDateTime": null }), &task).is_none());
        let with_recurrence = json!({
            "dueDateTime": { "dateTime": "2026-09-30T00:00:00", "timeZone": "UTC" },
            "recurrence": null
        });
        assert!(keep(&with_recurrence, &task).is_none());
        let mut once = task.clone();
        once.insert("recurrence".into(), Value::Null);
        let due =
            json!({ "dueDateTime": { "dateTime": "2026-09-30T00:00:00", "timeZone": "UTC" } });
        assert!(keep(&due, &once).is_none());
    }
}
