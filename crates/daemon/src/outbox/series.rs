//! Moving a recurring task's due date without splitting it (S20, D-069).
//! Graph answers a PATCH of `dueDateTime` alone on a recurring task by
//! moving the series on and creating a new open recurring task with that
//! date. Sent with the recurrence cleared, the date is an ordinary write;
//! a second PATCH then sets the recurrence again, its range starting on
//! the new date (with the old start, Graph moves the task back to the
//! first occurrence on or after it).
//!
//! - **Graph's recurrence, not the cache's.** The task is read first, so
//!   a recurrence another device changed since the cache last saw it is
//!   the one set again; a 412 on the first PATCH judges only the date.
//! - **The recurrence to set is kept in the operation** (`series`) before
//!   the first PATCH, so when the first landed and the second didn't (a
//!   rejection, a stopped daemon), a retry finds the task without its
//!   recurrence and finishes the job from the operation.
//! - **Where the date landed** is kept too (`landed`) when Graph moved it
//!   to the pattern's next occurrence (a Wednesday for a Sunday task), so
//!   undo's changed-since check compares what Graph made, not what was
//!   asked.

use ms_todo_core::{DATE_FORMAT, local_due_date};
use ms_todo_store::{Entity, OutboxRow};
use serde_json::{Value, json};

use super::send::{Attempt, Failure, classify, etag, fields_not_holding, patch_with, store};
use crate::entities::split_extension;
use crate::handlers::State;

/// In an operation's payload: the recurrence it sets again.
const SERIES: &str = "series";
/// In an operation's payload: the due date Graph gave the task, when it
/// isn't the one asked for.
const LANDED: &str = "landed";

/// Whether `op`'s PATCH sets a due date, and not the recurrence, on a task
/// that recurs, or whose recurrence an earlier attempt cleared.
pub(super) fn applies(op: &OutboxRow, cached: &Entity) -> bool {
    let Some(fields) = op.body().as_object() else {
        return false;
    };
    !fields.contains_key("recurrence")
        && fields.get("dueDateTime").is_some_and(|due| !due.is_null())
        && (recurs(cached.get("recurrence")) || op.payload.get(SERIES).is_some())
}

/// `op`'s body as it holds on Graph: with the date it landed on, where
/// that isn't the one asked for. What undo checks is still there.
pub(crate) fn as_landed(op: &OutboxRow) -> Value {
    let mut body = op.body().clone();
    if let (Some(body), Some(Value::Object(landed))) =
        (body.as_object_mut(), op.payload.get(LANDED))
    {
        body.extend(landed.clone());
    }
    body
}

/// Send `op` to the recurring task `graph_id` (see the module docs).
pub(super) async fn patch(
    state: &State,
    op: &OutboxRow,
    list_graph_id: &str,
    graph_id: &str,
    cached: &Entity,
) -> Result<Attempt, Failure> {
    let body = op.body();
    let (current, _) = split_extension(
        state
            .graph
            .get_task(list_graph_id, graph_id)
            .await
            .map_err(classify)?,
    );
    let from_graph = current
        .get("recurrence")
        .filter(|recurrence| recurrence.is_object());
    let recurrence = match (from_graph, op.payload.get(SERIES)) {
        (Some(recurrence), _) => match starting_on(recurrence.clone(), body) {
            Some(recurrence) => recurrence,
            None => return patch_with(state, op, list_graph_id, graph_id, cached, body).await,
        },
        // The first PATCH landed and the second didn't: finish it.
        (None, Some(stored)) if fields_not_holding(body, &current).is_empty() => {
            let (task, extension) =
                restore(state, list_graph_id, graph_id, &current, stored).await?;
            note_landed(state, op, &task).await;
            return Ok(Attempt::Changed(task, extension));
        }
        // It doesn't recur on Graph any more: an ordinary date.
        (None, _) => return patch_with(state, op, list_graph_id, graph_id, cached, body).await,
    };
    state
        .store
        .set_payload(&op.op_id, SERIES, &recurrence)
        .await
        .map_err(store)?;
    let mut first = body.clone();
    first["recurrence"] = Value::Null;
    let attempt = patch_with(state, op, list_graph_id, graph_id, cached, &first).await?;
    let (sent_to, extension) = match &attempt {
        Attempt::Changed(task, extension)
        | Attempt::Overwrote {
            task, extension, ..
        } => (task, extension.clone()),
        _ => return Ok(attempt),
    };
    let (task, again) = restore(state, list_graph_id, graph_id, sent_to, &recurrence).await?;
    note_landed(state, op, &task).await;
    let extension = again.or(extension);
    Ok(match attempt {
        Attempt::Overwrote {
            fields,
            note,
            theirs,
            ..
        } => Attempt::Overwrote {
            task,
            extension,
            fields,
            note,
            theirs,
        },
        _ => Attempt::Changed(task, extension),
    })
}

/// Set `recurrence` on the task, last seen as `task`. On a 412 (the task
/// moved on, or this is the HTTP client's retry of a PATCH whose answer
/// was lost) the task is read: holding it already is done, else it's sent
/// again with the new etag.
async fn restore(
    state: &State,
    list_graph_id: &str,
    graph_id: &str,
    task: &Entity,
    recurrence: &Value,
) -> Result<(Entity, Option<Option<Value>>), Failure> {
    let body = json!({ "recurrence": recurrence });
    match state
        .graph
        .update_task(list_graph_id, graph_id, &body, etag(task), true)
        .await
    {
        Ok(sent) => return Ok(split_extension(sent)),
        Err(error) if error.status() == Some(412) => {}
        Err(error) => return Err(classify(error)),
    }
    let (current, extension) = split_extension(
        state
            .graph
            .get_task(list_graph_id, graph_id)
            .await
            .map_err(classify)?,
    );
    if same_recurrence(current.get("recurrence"), recurrence) {
        return Ok((current, extension));
    }
    state
        .graph
        .update_task(list_graph_id, graph_id, &body, etag(&current), true)
        .await
        .map(split_extension)
        .map_err(classify)
}

/// Keep where the date landed when it isn't the one `op` asked for. Best
/// effort: without it, undo only refuses more than it must.
async fn note_landed(state: &State, op: &OutboxRow, task: &Entity) {
    let Some(landed) = task.get("dueDateTime") else {
        return;
    };
    let asked = json!({ "dueDateTime": op.body()["dueDateTime"] });
    if fields_not_holding(&asked, task).is_empty() {
        return;
    }
    let landed = json!({ "dueDateTime": landed });
    if let Err(error) = state.store.set_payload(&op.op_id, LANDED, &landed).await {
        eprintln!(
            "ms-todo daemon: cannot note where {}'s due date landed: {}",
            op.op_id,
            ms_todo_core::message_with_causes(&error)
        );
    }
}

fn recurs(recurrence: Option<&Value>) -> bool {
    recurrence.is_some_and(Value::is_object)
}

/// `recurrence`, its range starting on the day of `body`'s due date.
fn starting_on(mut recurrence: Value, body: &Value) -> Option<Value> {
    let due = &body["dueDateTime"];
    let day = local_due_date(due["dateTime"].as_str()?, due["timeZone"].as_str()?)?;
    recurrence["range"]["startDate"] = json!(day.format(DATE_FORMAT).to_string());
    Some(recurrence)
}

/// The same pattern starting the same day: Graph fills in the range's
/// other fields its own way.
fn same_recurrence(held: Option<&Value>, wanted: &Value) -> bool {
    held.is_some_and(|held| {
        held["pattern"] == wanted["pattern"]
            && held["range"]["startDate"] == wanted["range"]["startDate"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recurrence_starts_on_the_new_due_date() {
        let recurrence = json!({
            "pattern": { "type": "daily", "interval": 2 },
            "range": { "type": "noEnd", "startDate": "2026-09-27", "recurrenceTimeZone": "UTC" }
        });
        let body = json!({
            "dueDateTime": { "dateTime": "2026-09-30T00:00:00", "timeZone": "Europe/London" }
        });
        let moved = starting_on(recurrence.clone(), &body).expect("a day");
        assert_eq!(moved["range"]["startDate"], "2026-09-30");
        assert_eq!(moved["pattern"], recurrence["pattern"]);
        assert!(same_recurrence(Some(&moved), &moved));
        assert!(!same_recurrence(Some(&recurrence), &moved));
        assert!(!same_recurrence(None, &moved));
    }
}
