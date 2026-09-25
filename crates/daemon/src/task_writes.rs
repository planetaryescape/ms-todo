//! `tasks add|complete|reopen|edit|delete` (docs/blueprint/04-sync-cache.md#instant-local-writes).
//! Each resolves its targets in the cache and builds a typed [`Plan`]; a
//! dry run returns it. A real run applies the change to the cache and
//! queues it in the outbox in one transaction, then answers at once with
//! the tasks `pending`: the outbox worker sends them to Graph, with or
//! without a network right now.
//!
//! A create carries its `opId` in our extension (S13), so an ambiguous
//! outcome can be attributed later (D-028). Each change to several tasks
//! queues one operation per task, all under the command's `op_id`.

use ms_todo_core::DATE_FORMAT;
use ms_todo_protocol::{
    Applied, Clearable, ErrorPayload, NewTask, Plan, PlannedTask, ResponseData, TaskAction,
    TaskChange, TaskEdit, TaskSelect,
};
use ms_todo_store::{Entity, LISTS_SCOPE, LocalChange, NewOp, OpKind, TaskRow, apply_body};
use serde_json::{Map, Value, json};

use crate::assignment;
use crate::entities::{EXTENSION_NAME, task_entity};
use crate::freshness::ensure_ready;
use crate::handlers::{State, error_payload, store_error};
use crate::list_resolution::resolve_list;
use crate::outbox::op_id_for;
use crate::task_fields::{
    Field, edit_fields, graph_body, graph_due_date, new_task_fields, user_time_zone,
};
use crate::task_resolution::{Target, resolve_tasks, select_tasks};

pub(crate) async fn add_task(
    state: &State,
    task: NewTask,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let mut fields = new_task_fields(&task)?;
    // My Day's and the assignee's fields go in the create's own
    // extension, so the task is made with them and nothing more to send.
    let mut ours = task
        .my_day
        .then(|| crate::my_day::new_task_fields(&mut fields, state.my_day.today()));
    if let Some(name) = &task.assignee {
        let name = assignment::clean(name)?;
        let assigned = assignment::new_task_fields(&name, task.keep_status, &mut fields);
        ours.get_or_insert_default().extend(assigned);
    }
    ensure_ready(state, LISTS_SCOPE).await?;
    let lists = state.store.lists().await.map_err(store_error)?;
    let list = resolve_list(&lists, task.list.as_deref())?;
    let mut body = graph_body(&fields, &user_time_zone());
    if dry_run {
        if let Some(ours) = ours {
            body["extensions"] = json!([ours]);
        }
        return Ok(ResponseData::Plan(Plan {
            action: TaskAction::Add,
            list: Some(list.candidate()),
            targets: Vec::new(),
            lists: Vec::new(),
            changes: body,
        }));
    }
    let mut extension = our_extension(&op_id, None);
    if let (Some(ours), Some(extension)) = (ours, extension.as_object_mut()) {
        extension.extend(ours);
    }
    body["extensions"] = json!([extension]);
    let op = NewOp {
        op_id: op_id.clone(),
        entity_local_id: uuid::Uuid::new_v4().to_string(),
        list_local_id: list.local_id,
        op: OpKind::Create,
        action: action_name(TaskAction::Add).to_owned(),
        change: LocalChange::Insert {
            raw: new_task_raw(&body),
            extension: Some(extension),
        },
        payload: json!({ "body": body }),
    };
    queue(state, &op_id, None, vec![op], TaskAction::Add).await
}

/// The tasks a change names, or with `select` the open tasks it matches
/// (rung 5d's bulk changes).
pub(crate) struct Targets<'a> {
    pub names: &'a [String],
    pub list: Option<&'a str>,
    pub select: Option<&'a TaskSelect>,
}

pub(crate) async fn change_tasks(
    state: &State,
    targets: Targets<'_>,
    change: TaskChange,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let mut assign = None;
    let (action, fields) = match change {
        TaskChange::Complete => (TaskAction::Complete, vec![Field::Status("completed")]),
        TaskChange::Reopen => (TaskAction::Reopen, vec![Field::Status("notStarted")]),
        TaskChange::Edit(edit) => {
            assign = assignee_change(&edit)?;
            (TaskAction::Edit, edit_fields(&edit)?)
        }
        TaskChange::Delete => (TaskAction::Delete, Vec::new()),
        TaskChange::Move { to } => return move_tasks(state, &targets, &to, dry_run, op_id).await,
        TaskChange::AddToMyDay => {
            let targets = resolve(state, &targets).await?;
            let action = TaskAction::MyDayAdd;
            return crate::my_day::change(state, targets, action, dry_run, op_id).await;
        }
        TaskChange::RemoveFromMyDay => {
            let targets = resolve(state, &targets).await?;
            let action = TaskAction::MyDayRemove;
            return crate::my_day::change(state, targets, action, dry_run, op_id).await;
        }
        change @ (TaskChange::AddSteps { .. }
        | TaskChange::EditStep { .. }
        | TaskChange::CheckSteps { .. }
        | TaskChange::DeleteSteps { .. }
        | TaskChange::AddLink(_)
        | TaskChange::EditLink(_)
        | TaskChange::DeleteLink { .. }) => {
            return crate::task_children::change(state, &targets, change, dry_run, op_id).await;
        }
        change @ (TaskChange::AddAttachments { .. } | TaskChange::DeleteAttachments { .. }) => {
            return crate::attachments::change(state, &targets, change, dry_run, op_id).await;
        }
        TaskChange::Unknown => {
            return Err(error_payload(
                ms_todo_core::ErrorKind::Unsupported,
                "this daemon doesn't know that change; restart it with `ms-todo daemon stop`"
                    .into(),
            ));
        }
    };
    let bulk = targets.select.is_some();
    let targets = resolve(state, &targets).await?;
    if (bulk || targets.len() > 1) && fields.iter().any(Field::one_task_only) {
        return Err(error_payload(
            ms_todo_core::ErrorKind::InvalidInput,
            "a title or notes change one task at a time; several tasks together take \
             --due, --importance and --reminder"
                .into(),
        ));
    }
    let mut changes = if action == TaskAction::Delete {
        Value::Null
    } else {
        graph_body(&fields, &user_time_zone())
    };
    // Each task's own body (its start and due dates, S11 and D-058); one
    // task's plan shows exactly what it's sent.
    let bodies = targets
        .iter()
        .map(|target| crate::task_dates::task_body(&changes, &target.row))
        .collect::<Result<Vec<Value>, ErrorPayload>>()?;
    if let [body] = bodies.as_slice() {
        changes = body.clone();
    }
    // Each task's assignment, planned from the task as it is now.
    let plans: Vec<Option<assignment::TaskPlan>> = targets
        .iter()
        .map(|target| {
            let (change, keep_status) = assign.as_ref()?;
            assignment::plan(&target.row, change, *keep_status)
        })
        .collect();
    if dry_run {
        if let Some(changes) = changes.as_object_mut() {
            changes.extend(assignment::dry_run_changes(plans.iter().flatten()));
        }
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
            lists: Vec::new(),
            changes,
        }));
    }
    let mut ops = Vec::new();
    for ((target, plan), body) in targets.iter().zip(&plans).zip(&bodies) {
        let row = &target.row;
        let id = op_id_for(&op_id, ops.len());
        if action == TaskAction::Delete {
            ops.push(delete_op(id, row, action));
        } else if !fields.is_empty() {
            ops.push(update_op(id, row, body, action));
        }
        if let Some(plan) = plan {
            let mut more = assignment::plan_ops(&op_id, ops.len(), row, plan, action);
            ops.append(&mut more);
        }
    }
    queue(state, &op_id, None, ops, action).await
}

/// An edit's assignee change, the name checked, and whether it keeps the
/// status.
fn assignee_change(edit: &TaskEdit) -> Result<Option<(Clearable<String>, bool)>, ErrorPayload> {
    let change = match &edit.assignee {
        None => return Ok(None),
        Some(Clearable::Set(name)) => Clearable::Set(assignment::clean(name)?),
        Some(Clearable::Clear) => Clearable::Clear,
    };
    Ok(Some((change, edit.keep_status)))
}

/// `tasks move`: one move operation per task, all under `op_id`. Each task
/// shows in the list `to` at once, keeping its local ID; the outbox's move
/// job copies it there, checks the copy and deletes the original
/// (docs/blueprint/05-custom-features.md#move-between-lists).
async fn move_tasks(
    state: &State,
    targets: &Targets<'_>,
    to: &str,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    if targets.select.is_some() {
        return Err(error_payload(
            ms_todo_core::ErrorKind::InvalidInput,
            "name the tasks to move; --overdue and --due-before don't pick tasks to move".into(),
        ));
    }
    ensure_ready(state, LISTS_SCOPE).await?;
    let lists = state.store.lists().await.map_err(store_error)?;
    let list = resolve_list(&lists, Some(to))?;
    let targets = resolve(state, targets).await?;
    let already: Vec<String> = targets
        .iter()
        .filter(|target| target.row.list_local_id == list.local_id)
        .map(|target| format!("{:?}", target.title()))
        .collect();
    if !already.is_empty() {
        return Err(error_payload(
            ms_todo_core::ErrorKind::InvalidInput,
            format!(
                "{} {} in {:?} already",
                already.join(", "),
                if already.len() == 1 { "is" } else { "are" },
                list.name
            ),
        ));
    }
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action: TaskAction::Move,
            list: Some(list.candidate()),
            targets: targets
                .iter()
                .map(|target| PlannedTask {
                    id: target.local_id().to_owned(),
                    title: target.title().to_owned(),
                    list_id: target.list.local_id.clone(),
                })
                .collect(),
            lists: Vec::new(),
            changes: json!({ "list_id": list.local_id }),
        }));
    }
    let ops = targets
        .iter()
        .enumerate()
        .map(|(index, target)| move_op(op_id_for(&op_id, index), &target.row, &list.local_id))
        .collect();
    queue(state, &op_id, None, ops, TaskAction::Move).await
}

/// A move of `row` to the list `to` (local IDs), shown there at once.
pub(crate) fn move_op(op_id: String, row: &TaskRow, to: &str) -> NewOp {
    NewOp {
        op_id,
        entity_local_id: row.local_id.clone(),
        list_local_id: to.to_owned(),
        op: OpKind::Move,
        action: action_name(TaskAction::Move).to_owned(),
        payload: json!({ "body": { "from_list": row.list_local_id } }),
        change: LocalChange::Move { to: to.to_owned() },
    }
}

pub(crate) async fn resolve(
    state: &State,
    targets: &Targets<'_>,
) -> Result<Vec<Target>, ErrorPayload> {
    match targets.select {
        Some(_) if !targets.names.is_empty() => Err(error_payload(
            ms_todo_core::ErrorKind::InvalidInput,
            "name the tasks or select them (--overdue, --due-before), not both".into(),
        )),
        Some(select) => select_tasks(state, select, targets.list).await,
        None => resolve_tasks(state, targets.names, targets.list).await,
    }
}

/// Queue `ops`, one command's, wake the worker, and answer with what the
/// tasks look like now.
pub(crate) async fn queue(
    state: &State,
    command_id: &str,
    undoes: Option<&str>,
    ops: Vec<NewOp>,
    action: TaskAction,
) -> Result<ResponseData, ErrorPayload> {
    // A selection that matched nothing queues nothing, and has nothing
    // to undo.
    let mut rows = if ops.is_empty() {
        Vec::new()
    } else {
        let rows = state
            .store
            .enqueue(command_id, undoes, ops)
            .await
            .map_err(store_error)?;
        state.outbox.wake();
        rows
    };
    // A task with two operations (My Day's extension write and due date)
    // is answered once, as it is after both: its last row.
    let mut seen = std::collections::HashSet::new();
    rows.reverse();
    rows.retain(|row| seen.insert(row.local_id.clone()));
    rows.reverse();
    state
        .events
        .tasks_changed(rows.iter().map(|row| row.local_id.clone()).collect());
    Ok(ResponseData::Applied(Applied {
        op_id: command_id.to_owned(),
        action,
        items: rows.iter().map(task_entity).collect(),
        list_ids: rows.iter().map(|row| row.list_local_id.clone()).collect(),
        rolled: Vec::new(),
        undoes: undoes.map(str::to_owned),
        refused: Vec::new(),
    }))
}

/// A PATCH of `body`'s fields on `row`, applied to the cache at once.
pub(crate) fn update_op(op_id: String, row: &TaskRow, body: &Value, action: TaskAction) -> NewOp {
    let mut payload = json!({ "body": body });
    if completes_recurring(&row.raw, action) {
        // Never resent after an ambiguous answer, and its outcome is
        // judged by whether the due date moved (S12).
        payload["recurring"] = json!(true);
        payload["due_before"] =
            json!(graph_due_date(&row.raw).map(|date| date.format(DATE_FORMAT).to_string()));
    }
    NewOp {
        op_id,
        entity_local_id: row.local_id.clone(),
        list_local_id: row.list_local_id.clone(),
        op: OpKind::Update,
        action: action_name(action).to_owned(),
        payload,
        change: LocalChange::Update,
    }
}

/// A write of `fields` over `row`'s extension (My Day, assignment),
/// applied to the cache at once.
pub(crate) fn task_extension_op(
    op_id: String,
    row: &TaskRow,
    fields: Map<String, Value>,
    action: TaskAction,
) -> NewOp {
    NewOp {
        op_id,
        entity_local_id: row.local_id.clone(),
        list_local_id: row.list_local_id.clone(),
        op: OpKind::TaskExtension,
        action: action_name(action).to_owned(),
        payload: json!({ "body": Value::Object(fields) }),
        change: LocalChange::Extension,
    }
}

pub(crate) fn delete_op(op_id: String, row: &TaskRow, action: TaskAction) -> NewOp {
    NewOp {
        op_id,
        entity_local_id: row.local_id.clone(),
        list_local_id: row.list_local_id.clone(),
        op: OpKind::Delete,
        action: action_name(action).to_owned(),
        payload: json!({}),
        change: LocalChange::Tombstone,
    }
}

/// Our open extension as a create sends it, holding the operation's ID.
/// `kept` is the rest of an extension the task had, for a re-create.
pub(crate) fn our_extension(op_id: &str, kept: Option<&Value>) -> Value {
    let mut extension = Map::new();
    if let Some(Value::Object(kept)) = kept {
        for (key, value) in kept {
            // Graph's own bookkeeping, not our data.
            if key != "id" && !key.starts_with('@') {
                extension.insert(key.clone(), value.clone());
            }
        }
    }
    extension.insert(
        "@odata.type".into(),
        json!("microsoft.graph.openTypeExtension"),
    );
    extension.insert("extensionName".into(), json!(EXTENSION_NAME));
    extension.insert("opId".into(), json!(op_id));
    Value::Object(extension)
}

/// A task not yet sent, as the cache shows it: what the POST sets, over
/// Graph's defaults for a new task.
pub(crate) fn new_task_raw(body: &Value) -> Entity {
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string();
    let mut raw = Map::new();
    raw.insert("title".into(), json!(""));
    raw.insert("status".into(), json!("notStarted"));
    raw.insert("importance".into(), json!("normal"));
    raw.insert("isReminderOn".into(), json!(false));
    raw.insert("categories".into(), json!([]));
    raw.insert("createdDateTime".into(), json!(now));
    raw.insert("lastModifiedDateTime".into(), json!(now));
    apply_body(&mut raw, body);
    raw
}

fn completes_recurring(task: &Entity, action: TaskAction) -> bool {
    action == TaskAction::Complete && task.get("recurrence").is_some_and(Value::is_object)
}

/// The outbox's `action` for a user's action.
pub(crate) fn action_name(action: TaskAction) -> &'static str {
    match action {
        TaskAction::Add => "add",
        TaskAction::Complete => "complete",
        TaskAction::Reopen => "reopen",
        TaskAction::Edit => "edit",
        TaskAction::Delete => "delete",
        TaskAction::Move => "move",
        TaskAction::MoveList => "move_list",
        TaskAction::OrderList => "order_list",
        TaskAction::RenameFolder => "rename_folder",
        TaskAction::DeleteFolder => "delete_folder",
        TaskAction::OrderFolder => "order_folder",
        TaskAction::MyDayAdd => "my_day_add",
        TaskAction::MyDayRemove => "my_day_remove",
        TaskAction::MyDayRollover => "my_day_rollover",
        TaskAction::StepAdd => "step_add",
        TaskAction::StepEdit => "step_edit",
        TaskAction::StepCheck => "step_check",
        TaskAction::StepUncheck => "step_uncheck",
        TaskAction::StepDelete => "step_delete",
        TaskAction::LinkAdd => "link_add",
        TaskAction::LinkEdit => "link_edit",
        TaskAction::LinkDelete => "link_delete",
        TaskAction::AttachmentAdd => "attachment_add",
        TaskAction::AttachmentDelete => "attachment_delete",
        TaskAction::CreateList => "create_list",
        TaskAction::RenameList => "rename_list",
        TaskAction::DeleteList => "delete_list",
        TaskAction::CategoryCreate => "category_create",
        TaskAction::CategoryRecolor => "category_recolor",
        TaskAction::CategoryDelete => "category_delete",
        TaskAction::ExtensionSet => "extension_set",
        TaskAction::ExtensionDelete => "extension_delete",
        TaskAction::Undo | TaskAction::Unknown => "change",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ms_todo_store::TaskRow;

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

    #[test]
    fn an_update_sends_only_its_fields() {
        let task = row(json!({ "id": "T1", "title": "Milk", "importance": "low" }));
        let op = update_op(
            "op-1".into(),
            &task,
            &json!({ "title": "Oat milk" }),
            TaskAction::Edit,
        );
        assert!(matches!(op.change, LocalChange::Update));
        assert_eq!(op.payload, json!({ "body": { "title": "Oat milk" } }));
    }

    #[test]
    fn completing_a_recurring_task_records_its_due_date_before() {
        let task = row(json!({
            "id": "T1", "recurrence": { "pattern": {} },
            "dueDateTime": { "dateTime": "2026-09-24T00:00:00.0000000", "timeZone": "UTC" }
        }));
        let body = json!({ "status": "completed" });
        let op = update_op("op-1".into(), &task, &body, TaskAction::Complete);
        assert_eq!(op.payload["recurring"], true);
        assert_eq!(op.payload["due_before"], "2026-09-24");
        let edit = update_op(
            "op-2".into(),
            &task,
            &json!({ "title": "x" }),
            TaskAction::Edit,
        );
        assert!(edit.payload.get("recurring").is_none());
    }

    #[test]
    fn a_task_not_yet_sent_has_graphs_defaults_under_what_it_sets() {
        let raw =
            new_task_raw(&json!({ "title": "Buy milk", "importance": "high", "extensions": [] }));
        assert_eq!(raw["title"], "Buy milk");
        assert_eq!(raw["importance"], "high");
        assert_eq!(raw["status"], "notStarted");
        assert!(raw.get("extensions").is_none());
        let created = raw["createdDateTime"].as_str().expect("created");
        assert!(
            chrono::DateTime::parse_from_rfc3339(created).is_ok(),
            "{created}"
        );
    }

    #[test]
    fn a_re_create_keeps_our_extension_data_but_takes_a_new_op_id() {
        let kept = json!({
            "@odata.type": "#microsoft.graph.openTypeExtension",
            "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
            "extensionName": EXTENSION_NAME,
            "opId": "old",
            "myDay": "2026-09-24"
        });
        let extension = our_extension("new", Some(&kept));
        assert_eq!(extension["opId"], "new");
        assert_eq!(extension["myDay"], "2026-09-24");
        assert_eq!(
            extension["@odata.type"],
            "microsoft.graph.openTypeExtension"
        );
        assert!(extension.get("id").is_none());
    }
}
