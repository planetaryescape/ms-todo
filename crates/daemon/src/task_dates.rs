//! A task's start date, due date and recurrence together (rung 8e, D-058),
//! as Microsoft To Do keeps them, found live on 2026-09-25:
//!
//! - A start date sent alone sets the due date to it when the task has
//!   none, and leaves a due date the task has alone (S11). So the due
//!   date always goes with it: the task's own, or the start date.
//! - On a repeating task the start date is the occurrence: Graph moves the
//!   start to the recurrence's first day and the due date by the same
//!   distance, and ignores a start date sent on its own. So a recurrence
//!   written on a task with a start date sends the start as its first due
//!   date too, and a start date on a repeating task is refused.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::ErrorPayload;
use ms_todo_store::TaskRow;
use serde_json::Value;

use crate::handlers::error_payload;
use crate::task_fields::as_written;

/// `changes`, one change's Graph fields, as `row` is sent them.
pub(crate) fn task_body(changes: &Value, row: &TaskRow) -> Result<Value, ErrorPayload> {
    let mut body = changes.clone();
    let Some(fields) = body.as_object_mut() else {
        return Ok(body);
    };
    let set = |key: &str| fields.get(key).filter(|value| !value.is_null()).cloned();
    let (start, due, recurrence) = (set("startDateTime"), set("dueDateTime"), set("recurrence"));
    let has = |key: &str| row.raw.get(key).is_some_and(|value| !value.is_null());
    let repeats = match fields.get("recurrence") {
        Some(recurrence) => !recurrence.is_null(),
        None => has("recurrence"),
    };
    if start.is_some() && repeats && recurrence.is_none() {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            format!(
                "{:?} repeats, and Microsoft To Do keeps no start date of its own on a repeating \
                 task; --clear-recur first, or set the recurrence and the start together",
                row.title
            ),
        ));
    }
    if let (Some(start), None) = (&start, &due) {
        let due = row
            .raw
            .get("dueDateTime")
            .filter(|due| !due.is_null())
            .map_or_else(
                || start.clone(),
                |due| as_written("dueDateTime", due.clone()),
            );
        fields.insert("dueDateTime".into(), due);
    }
    if let (Some(_), None, Some(due)) = (&recurrence, &start, &due)
        && has("startDateTime")
        && !fields.contains_key("startDateTime")
    {
        // The start moves to the first occurrence with the due date.
        fields.insert("startDateTime".into(), due.clone());
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row(raw: Value) -> TaskRow {
        TaskRow {
            local_id: "t1".into(),
            graph_id: Some("T1".into()),
            list_local_id: "l1".into(),
            title: "Water plants".into(),
            raw: raw.as_object().cloned().expect("object"),
            extension: None,
            sync_state: "synced".into(),
        }
    }

    fn day(date: &str) -> Value {
        json!({ "dateTime": format!("{date}T00:00:00"), "timeZone": "Europe/London" })
    }

    #[test]
    fn a_start_date_goes_with_the_due_date_the_task_has_or_itself() {
        let changes = json!({ "startDateTime": day("2026-10-01") });
        assert_eq!(
            task_body(&changes, &row(json!({}))).expect("body"),
            json!({ "startDateTime": day("2026-10-01"), "dueDateTime": day("2026-10-01") })
        );
        let dated = row(json!({
            "dueDateTime": { "dateTime": "2026-10-05T00:00:00.0000000", "timeZone": "UTC" }
        }));
        assert_eq!(
            task_body(&changes, &dated).expect("body")["dueDateTime"],
            json!({ "dateTime": "2026-10-05T00:00:00.0000000", "timeZone": "UTC" })
        );
        let cleared = json!({ "startDateTime": null });
        assert_eq!(task_body(&cleared, &dated).expect("body"), cleared);
    }

    #[test]
    fn a_recurrence_takes_a_start_date_along_and_a_repeating_task_refuses_one() {
        let recurring =
            json!({ "dueDateTime": day("2026-10-05"), "recurrence": { "pattern": {} } });
        let started = row(json!({ "startDateTime": day("2026-10-01") }));
        assert_eq!(
            task_body(&recurring, &started).expect("body")["startDateTime"],
            day("2026-10-05")
        );
        assert!(
            task_body(&recurring, &row(json!({})))
                .expect("body")
                .get("startDateTime")
                .is_none()
        );
        let repeats = row(json!({ "recurrence": { "pattern": {} } }));
        let start = json!({ "startDateTime": day("2026-10-01") });
        let error = task_body(&start, &repeats).expect_err("refused");
        assert!(error.message.contains("repeats"), "{}", error.message);
        // Clearing the recurrence in the same edit is fine.
        let both = json!({ "startDateTime": day("2026-10-01"), "recurrence": null });
        assert!(task_body(&both, &repeats).is_ok());
    }
}
