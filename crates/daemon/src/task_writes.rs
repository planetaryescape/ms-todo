//! `tasks add|complete|reopen|edit|delete`, sent synchronously to Graph
//! (rung 2 has no outbox). Each builds a typed [`Plan`] first; a dry run
//! returns it and the real run applies it, so the preview is what runs.
//!
//! - A create carries its `opId` in our extension (S13) and is never resent
//!   once it may have reached Graph; nor is completing a recurring task
//!   (D-028). Either comes back as `outcome_unknown` with the `op_id`.
//! - A PATCH sends `If-Match` with the etag from our last read or write
//!   (S6). On a 412 it re-reads the task once: if nobody touched the fields
//!   it changes, it re-sends; otherwise it's a conflict, and nothing is
//!   overwritten.

use ms_todo_core::{DATE_FORMAT, ErrorKind, message_with_causes};
use ms_todo_graph::GraphError;
use ms_todo_protocol::{
    Applied, Entity, ErrorPayload, NewTask, Plan, PlannedTask, ResponseData, Rolled, TaskAction,
    TaskChange,
};
use serde_json::{Value, json};

use crate::handlers::{State, error_payload, graph_error};
use crate::known_tasks::Target;
use crate::list_resolution::resolve_list;
use crate::task_fields::{
    Field, edit_fields, graph_body, graph_due_date, new_task_fields, user_time_zone,
};
use crate::task_resolution::resolve_tasks;

/// Our open extension (docs/blueprint/05-custom-features.md).
const EXTENSION_NAME: &str = "com.planetaryescape.mstodo";

pub(crate) async fn add_task(
    state: &State,
    task: NewTask,
    dry_run: bool,
) -> Result<ResponseData, ErrorPayload> {
    let fields = new_task_fields(&task)?;
    let lists = state.graph.list_lists().await.map_err(graph_error)?;
    let list = resolve_list(&lists, task.list.as_deref())?;
    let mut body = graph_body(&fields, &user_time_zone());
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action: TaskAction::Add,
            list: Some(list),
            targets: Vec::new(),
            changes: body,
        }));
    }
    let op_id = new_op_id();
    body["extensions"] = json!([{
        "@odata.type": "microsoft.graph.openTypeExtension",
        "extensionName": EXTENSION_NAME,
        "opId": op_id,
    }]);
    match state.graph.create_task(&list.id, &body).await {
        Ok(created) => {
            state.known.remember(&list.id, &created);
            Ok(ResponseData::Applied(Applied {
                op_id,
                action: TaskAction::Add,
                items: vec![created],
                list_ids: vec![list.id],
                rolled: Vec::new(),
            }))
        }
        Err(error @ GraphError::OutcomeUnknown(_)) => Err(ErrorPayload {
            message: format!(
                "{}. The task may or may not have been created in {:?}. Check with \
                 `ms-todo tasks list --list {}` before adding it again: a blind retry can \
                 make a duplicate. If it's there, its {EXTENSION_NAME} extension holds \
                 opId {op_id}",
                message_with_causes(&error),
                list.name,
                list.id,
            ),
            op_id: Some(op_id),
            ..graph_error(error)
        }),
        Err(error) => Err(ErrorPayload {
            op_id: Some(op_id),
            ..graph_error(error)
        }),
    }
}

pub(crate) async fn change_tasks(
    state: &State,
    names: &[String],
    list: Option<&str>,
    change: TaskChange,
    dry_run: bool,
) -> Result<ResponseData, ErrorPayload> {
    let (action, fields) = match change {
        TaskChange::Complete => (TaskAction::Complete, vec![Field::Status("completed")]),
        TaskChange::Reopen => (TaskAction::Reopen, vec![Field::Status("notStarted")]),
        TaskChange::Edit(edit) => (TaskAction::Edit, edit_fields(&edit)?),
        TaskChange::Delete => (TaskAction::Delete, Vec::new()),
        TaskChange::Unknown => {
            return Err(error_payload(
                ErrorKind::Unsupported,
                "this daemon doesn't know that change; restart it with `ms-todo daemon stop`"
                    .into(),
            ));
        }
    };
    let targets = resolve_tasks(state, names, list).await?;
    let changes = if action == TaskAction::Delete {
        Value::Null
    } else {
        graph_body(&fields, &user_time_zone())
    };
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action,
            list: None,
            targets: targets
                .iter()
                .map(|target| PlannedTask {
                    id: target.id().to_owned(),
                    title: target.title().to_owned(),
                    list_id: target.list_id.clone(),
                })
                .collect(),
            changes,
        }));
    }

    let op_id = new_op_id();
    let mut applied = Applied {
        op_id: op_id.clone(),
        action,
        items: Vec::new(),
        list_ids: Vec::new(),
        rolled: Vec::new(),
    };
    let mut applied_ids = Vec::new();
    for (done, target) in targets.iter().enumerate() {
        let result = if action == TaskAction::Delete {
            delete(state, target).await
        } else {
            patch(state, target, &fields, &changes, action).await
        };
        match result {
            Ok((task, rolled)) => {
                applied_ids.push(target.id().to_owned());
                applied.items.push(task);
                applied.list_ids.push(target.list_id.clone());
                applied.rolled.extend(rolled);
            }
            Err(mut error) => {
                let not_attempted = targets.len() - done - 1;
                if done > 0 || not_attempted > 0 {
                    error.message = format!(
                        "{} (task {}; {done} changed before it, {not_attempted} not attempted)",
                        error.message,
                        target.id(),
                    );
                }
                error.op_id = Some(op_id);
                error.applied = applied_ids;
                return Err(error);
            }
        }
    }
    Ok(ResponseData::Applied(applied))
}

async fn delete(state: &State, target: &Target) -> Result<(Entity, Option<Rolled>), ErrorPayload> {
    state
        .graph
        .delete_task(&target.list_id, target.id())
        .await
        .map_err(graph_error)?;
    state.known.forget(target.id());
    Ok((target.task.clone(), None))
}

async fn patch(
    state: &State,
    target: &Target,
    fields: &[Field],
    body: &Value,
    action: TaskAction,
) -> Result<(Entity, Option<Rolled>), ErrorPayload> {
    let seen = &target.task;
    let sent = send_patch(state, target, seen, body, action).await;
    if !matches!(&sent, Err(error) if error.status() == Some(412)) {
        return settle(state, target, seen, sent, action);
    }
    // Our etag is stale: something changed the task since we read it.
    let current = state
        .graph
        .get_task(&target.list_id, target.id())
        .await
        .map_err(graph_error)?;
    state.known.remember(&target.list_id, &current);
    if !completes_recurring(&current, action)
        && fields.iter().all(|field| field.is_applied_to(&current))
    {
        // Ours already, e.g. a retried PATCH whose first try went through.
        return Ok((current, None));
    }
    let touched = touched_keys(seen, &current, fields, completes_recurring(seen, action));
    if !touched.is_empty() {
        return Err(error_payload(
            ErrorKind::Conflict,
            format!(
                "task {} ({:?}) changed on the server since ms-todo last read it ({}), so \
                 nothing was overwritten; check it with `ms-todo tasks list` and try again",
                target.id(),
                target.title(),
                touched.join(", "),
            ),
        ));
    }
    let sent = send_patch(state, target, &current, body, action).await;
    settle(state, target, &current, sent, action)
}

/// PATCH with `seen`'s etag. Completing a recurring task isn't idempotent:
/// a repeat would complete the next occurrence too.
async fn send_patch(
    state: &State,
    target: &Target,
    seen: &Entity,
    body: &Value,
    action: TaskAction,
) -> Result<Entity, GraphError> {
    state
        .graph
        .update_task(
            &target.list_id,
            target.id(),
            body,
            etag(seen),
            !completes_recurring(seen, action),
        )
        .await
}

/// A PATCH's answer as the command's result, given the task as it was sent.
fn settle(
    state: &State,
    target: &Target,
    seen: &Entity,
    sent: Result<Entity, GraphError>,
    action: TaskAction,
) -> Result<(Entity, Option<Rolled>), ErrorPayload> {
    let recurring = completes_recurring(seen, action);
    match sent {
        Ok(updated) => {
            state.known.remember(&target.list_id, &updated);
            let rolled = if recurring {
                recurring_outcome(seen, &updated)?
            } else {
                None
            };
            Ok((updated, rolled))
        }
        Err(error) if recurring => Err(recurring_error(error, target)),
        Err(error) => Err(graph_error(error)),
    }
}

/// The fields we're changing whose server value moved between our read and
/// now. Completing a recurring task also watches the due date: if it moved,
/// someone else completed that occurrence, and re-sending would complete
/// the next one too.
fn touched_keys(
    before: &Entity,
    now: &Entity,
    fields: &[Field],
    completes_recurring: bool,
) -> Vec<&'static str> {
    let due: &[&'static str] = if completes_recurring {
        &["dueDateTime"]
    } else {
        &[]
    };
    let mut keys: Vec<&'static str> = fields
        .iter()
        .flat_map(Field::graph_keys)
        .chain(due)
        .copied()
        .filter(|key| before.get(*key) != now.get(*key))
        .collect();
    keys.dedup();
    keys
}

/// S12: completing a recurring task keeps its ID, moves its due date to the
/// next occurrence and leaves it `notStarted`; Graph adds a completed copy
/// with a new ID. Success is a 200 with either `completed` or a moved due
/// date (docs/blueprint/04-sync-cache.md#completing-a-recurring-task).
fn recurring_outcome(before: &Entity, after: &Entity) -> Result<Option<Rolled>, ErrorPayload> {
    if after.get("status").and_then(Value::as_str) == Some("completed") {
        return Ok(None);
    }
    match (graph_due_date(before), graph_due_date(after)) {
        (Some(was), Some(next)) if next > was => Ok(Some(Rolled {
            id: after
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            next_due: next.format(DATE_FORMAT).to_string(),
        })),
        _ => Err(error_payload(
            ErrorKind::OutcomeUnknown,
            "Graph accepted completing this recurring task, but it's still not completed and \
             its due date didn't move on. Check it with `ms-todo tasks list` before trying again"
                .into(),
        )),
    }
}

fn recurring_error(error: GraphError, target: &Target) -> ErrorPayload {
    if !matches!(error, GraphError::OutcomeUnknown(_)) {
        return graph_error(error);
    }
    ErrorPayload {
        message: format!(
            "{}. Recurring task {} ({:?}) may or may not have been completed. Check with \
             `ms-todo tasks list --list {}` before trying again: if its due date moved on and \
             a completed copy appeared, it worked, and a retry would complete the next \
             occurrence too",
            message_with_causes(&error),
            target.id(),
            target.title(),
            target.list_id,
        ),
        ..graph_error(error)
    }
}

fn completes_recurring(task: &Entity, action: TaskAction) -> bool {
    action == TaskAction::Complete && task.get("recurrence").is_some_and(Value::is_object)
}

fn etag(task: &Entity) -> Option<&str> {
    task.get("@odata.etag").and_then(Value::as_str)
}

fn new_op_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn a_rolled_recurring_completion_reports_the_next_due_date() {
        let before = entity(json!({
            "id": "T", "status": "notStarted", "recurrence": {},
            "dueDateTime": { "dateTime": "2026-09-24T00:00:00.0000000", "timeZone": "UTC" }
        }));
        let after = entity(json!({
            "id": "T", "status": "notStarted", "recurrence": {},
            "dueDateTime": { "dateTime": "2026-10-01T00:00:00.0000000", "timeZone": "UTC" }
        }));
        let rolled = recurring_outcome(&before, &after)
            .expect("success")
            .expect("rolled");
        assert_eq!(rolled.next_due, "2026-10-01");
        let unmoved = recurring_outcome(&before, &before).expect_err("didn't move");
        assert_eq!(unmoved.kind, "outcome_unknown");
    }

    #[test]
    fn only_the_fields_we_change_count_as_touched() {
        let before = entity(json!({ "title": "a", "status": "notStarted", "importance": "low" }));
        let now = entity(json!({ "title": "b", "status": "notStarted", "importance": "low" }));
        let status = [Field::Status("completed")];
        assert!(touched_keys(&before, &now, &status, false).is_empty());
        let title = [Field::Title("c".into())];
        assert_eq!(touched_keys(&before, &now, &title, false), ["title"]);
    }
}
