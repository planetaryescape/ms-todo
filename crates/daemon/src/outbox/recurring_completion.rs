//! A completion belongs to the occurrence the user saw (S12, D-040).
//! A newer cache etag must never let an older queued request complete the next.

use ms_todo_core::{DATE_FORMAT, ErrorKind};
use ms_todo_store::{Entity, OutboxRow};
use serde_json::Value;

use super::send::Failure;
use crate::handlers::error_payload;
use crate::task_fields::graph_due_date;

pub(super) fn ensure_occurrence(op: &OutboxRow, current: &Entity) -> Result<(), Failure> {
    if !op.is_recurring_completion() {
        return Ok(());
    }
    let due = graph_due_date(current).map(|day| day.format(DATE_FORMAT).to_string());
    if op.payload["due_before"].as_str() != due.as_deref()
        || current.get("status").and_then(Value::as_str) == Some("completed")
    {
        return Err(Failure::Rejected(error_payload(
            ErrorKind::Conflict,
            "the recurring task changed occurrence since ms-todo last read it, so nothing was completed".into(),
        )));
    }
    Ok(())
}
