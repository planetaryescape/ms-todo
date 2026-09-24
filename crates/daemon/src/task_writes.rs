//! `tasks add|complete|reopen|edit|delete`, sent synchronously to Graph
//! (no outbox until rung 4), with what Graph returns written to the cache.
//! Targets are resolved in the cache. Each builds a typed [`Plan`] first; a
//! dry run returns it and the real run applies it, so the preview is what
//! runs.
//!
//! - A create carries its `opId` in our extension (S13) and is never resent
//!   once it may have reached Graph; nor is completing a recurring task
//!   (D-028). Either comes back as `outcome_unknown` with the `op_id`.
//! - A PATCH sends `If-Match` with the cached etag (S6). On a 412 it
//!   re-reads the task once: if nobody touched the fields it changes, it
//!   re-sends; otherwise it's a conflict, and nothing is overwritten.

use ms_todo_core::{DATE_FORMAT, ErrorKind, message_with_causes};
use ms_todo_graph::GraphError;
use ms_todo_protocol::{
    Applied, Entity, ErrorPayload, NewTask, Plan, PlannedTask, ResponseData, Rolled, TaskAction,
    TaskChange,
};
use ms_todo_store::LISTS_SCOPE;
use serde_json::{Value, json};

use crate::entities::{EXTENSION_NAME, split_extension, task_entity};
use crate::freshness::ensure_ready;
use crate::handlers::{State, error_payload, graph_error, store_error};
use crate::list_resolution::{ListRef, resolve_list};
use crate::task_fields::{
    Field, edit_fields, graph_body, graph_due_date, new_task_fields, user_time_zone,
};
use crate::task_resolution::{Target, resolve_tasks};

pub(crate) async fn add_task(
    state: &State,
    task: NewTask,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let fields = new_task_fields(&task)?;
    ensure_ready(state, LISTS_SCOPE).await?;
    let lists = state.store.lists().await.map_err(store_error)?;
    let list = resolve_list(&lists, task.list.as_deref())?;
    let mut body = graph_body(&fields, &user_time_zone());
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action: TaskAction::Add,
            list: Some(list.candidate()),
            targets: Vec::new(),
            changes: body,
        }));
    }
    body["extensions"] = json!([{
        "@odata.type": "microsoft.graph.openTypeExtension",
        "extensionName": EXTENSION_NAME,
        "opId": op_id,
    }]);
    match state.graph.create_task(&list.graph_id, &body).await {
        Ok(created) => {
            let item = record(state, &list, created)
                .await
                .map_err(|error| ErrorPayload {
                    op_id: Some(op_id.clone()),
                    ..error
                })?;
            Ok(ResponseData::Applied(Applied {
                op_id,
                action: TaskAction::Add,
                items: vec![item],
                list_ids: vec![list.local_id],
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
                list.local_id,
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
    op_id: String,
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
                    id: target.local_id().to_owned(),
                    title: target.title().to_owned(),
                    list_id: target.list.local_id.clone(),
                })
                .collect(),
            changes,
        }));
    }

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
                applied_ids.push(target.local_id().to_owned());
                applied.items.push(task);
                applied.list_ids.push(target.list.local_id.clone());
                applied.rolled.extend(rolled);
            }
            Err(mut error) => {
                let not_attempted = targets.len() - done - 1;
                if done > 0 || not_attempted > 0 {
                    error.message = format!(
                        "{} (task {}; {done} changed before it, {not_attempted} not attempted)",
                        error.message,
                        target.local_id(),
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
        .delete_task(&target.list.graph_id, target.graph_id())
        .await
        .map_err(graph_error)?;
    state
        .store
        .tombstone_task_local(target.local_id())
        .await
        .map_err(store_error)?;
    Ok((task_entity(&target.row), None))
}

async fn patch(
    state: &State,
    target: &Target,
    fields: &[Field],
    body: &Value,
    action: TaskAction,
) -> Result<(Entity, Option<Rolled>), ErrorPayload> {
    let seen = &target.row.raw;
    let sent = send_patch(state, target, seen, body, action).await;
    if !matches!(&sent, Err(error) if error.status() == Some(412)) {
        return settle(state, target, seen, sent, action).await;
    }
    // The cached etag is stale: something changed the task since we read it.
    let fetched = state
        .graph
        .get_task(&target.list.graph_id, target.graph_id())
        .await
        .map_err(graph_error)?;
    let (current, extension) = split_extension(fetched);
    let current_item = record_split(state, &target.list, &current, extension).await?;
    if !completes_recurring(&current, action)
        && fields.iter().all(|field| field.is_applied_to(&current))
    {
        // Ours already, e.g. a retried PATCH whose first try went through.
        return Ok((current_item, None));
    }
    let touched = touched_keys(seen, &current, fields, completes_recurring(seen, action));
    if !touched.is_empty() {
        return Err(error_payload(
            ErrorKind::Conflict,
            format!(
                "task {} ({:?}) changed on the server since ms-todo last read it ({}), so \
                 nothing was overwritten; check it with `ms-todo tasks list` and try again",
                target.local_id(),
                target.title(),
                touched.join(", "),
            ),
        ));
    }
    let sent = send_patch(state, target, &current, body, action).await;
    settle(state, target, &current, sent, action).await
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
            &target.list.graph_id,
            target.graph_id(),
            body,
            etag(seen),
            !completes_recurring(seen, action),
        )
        .await
}

/// A PATCH's answer as the command's result, given the task as it was sent.
async fn settle(
    state: &State,
    target: &Target,
    seen: &Entity,
    sent: Result<Entity, GraphError>,
    action: TaskAction,
) -> Result<(Entity, Option<Rolled>), ErrorPayload> {
    let recurring = completes_recurring(seen, action);
    match sent {
        Ok(updated) => {
            let rolled = if recurring {
                recurring_outcome(seen, &updated, target.local_id())?
            } else {
                None
            };
            Ok((record(state, &target.list, updated).await?, rolled))
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
fn recurring_outcome(
    before: &Entity,
    after: &Entity,
    local_id: &str,
) -> Result<Option<Rolled>, ErrorPayload> {
    if after.get("status").and_then(Value::as_str) == Some("completed") {
        return Ok(None);
    }
    match (graph_due_date(before), graph_due_date(after)) {
        (Some(was), Some(next)) if next > was => Ok(Some(Rolled {
            id: local_id.to_owned(),
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
            target.local_id(),
            target.title(),
            target.list.local_id,
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

/// Write what Graph returned for a task in `list` to the cache, and return
/// it as clients see it. If the cache can't take it, the change was still
/// made in Microsoft To Do, and the error says so; the next sync records it.
async fn record(state: &State, list: &ListRef, task: Entity) -> Result<Entity, ErrorPayload> {
    let (raw, extension) = split_extension(task);
    record_split(state, list, &raw, extension).await
}

/// [`record`], for a task already split by [`split_extension`].
async fn record_split(
    state: &State,
    list: &ListRef,
    raw: &Entity,
    extension: Option<Option<Value>>,
) -> Result<Entity, ErrorPayload> {
    match state
        .store
        .upsert_task_local(&list.local_id, raw, extension)
        .await
    {
        Ok(row) => Ok(task_entity(&row)),
        Err(error) => {
            let failure = store_error(error);
            Err(ErrorPayload {
                message: format!(
                    "the change was made in Microsoft To Do (task {}), but the local cache \
                     couldn't record it: {}. The next sync picks it up",
                    raw.get("id").and_then(Value::as_str).unwrap_or_default(),
                    failure.message,
                ),
                ..failure
            })
        }
    }
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
        let rolled = recurring_outcome(&before, &after, "local-T")
            .expect("success")
            .expect("rolled");
        assert_eq!(rolled.next_due, "2026-10-01");
        assert_eq!(rolled.id, "local-T");
        let unmoved = recurring_outcome(&before, &before, "local-T").expect_err("didn't move");
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
