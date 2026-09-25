//! My Day (docs/blueprint/05-custom-features.md#my-day, D-037): a task is in
//! a day's My Day when `myDay` in our extension is that day. A task with no
//! due date put in My Day is also due that day, marked `myDayDueSet`, so
//! the phone's "Show 'Due Today' tasks in My Day" shows it too; ms-todo
//! takes that due date away again when the task leaves My Day, unless it's
//! completed or someone has changed it since.
//!
//! Each change is one extension write per task (GET, merge, write, through
//! the outbox) and, when the due date changes, a due-date edit after it.
//! The extension goes first: if it fails, the edit waiting on it fails too,
//! and a due date set without `myDay` never reaches Graph.

mod config;
mod rollover;
mod suggestions;

use chrono::NaiveDate;
use ms_todo_core::DATE_FORMAT;
use ms_todo_protocol::{ErrorPayload, MyDay, Plan, PlannedTask, ResponseData, TaskAction};
use ms_todo_store::{LISTS_SCOPE, NewOp, TaskRow, View};
use serde_json::{Map, Value, json};

pub(crate) use config::Config;
pub(crate) use rollover::{Origin, rollover, run};
pub(crate) use suggestions::suggestions;

use crate::freshness::{all_lists_state, read_state};
use crate::handlers::{State, store_error};
use crate::outbox::op_id_for;
use crate::task_fields::{Field, graph_body, graph_due_date, user_time_zone};
use crate::task_resolution::Target;
use crate::task_writes::{queue, task_extension_op, update_op};

/// The day a task is in My Day for, `YYYY-MM-DD`.
pub(crate) const MY_DAY: &str = "myDay";
/// True when ms-todo set the task's due date for My Day (D-037).
pub(crate) const DUE_SET: &str = "myDayDueSet";

/// What one My Day change does to one task.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TaskPlan {
    /// The extension's fields to set; a null removes one.
    pub fields: Map<String, Value>,
    /// Fields that must still hold these values in Graph's copy for the
    /// extension write to be made.
    pub expect: Option<Value>,
    /// The new due date, when it changes: `Some(None)` removes it.
    pub due: Option<Option<NaiveDate>>,
}

/// A task's `myDay`, if it has one.
pub(crate) fn my_day_of(row: &TaskRow) -> Option<NaiveDate> {
    let day = row.extension.as_ref()?.get(MY_DAY)?.as_str()?;
    NaiveDate::parse_from_str(day, DATE_FORMAT).ok()
}

fn due_set(row: &TaskRow) -> bool {
    row.extension
        .as_ref()
        .and_then(|extension| extension.get(DUE_SET))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn has_due_set(row: &TaskRow) -> bool {
    row.extension
        .as_ref()
        .is_some_and(|extension| extension.get(DUE_SET).is_some())
}

pub(crate) fn completed(row: &TaskRow) -> bool {
    row.raw.get("status").and_then(Value::as_str) == Some("completed")
}

/// Putting `row` in `today`'s My Day, or `None` if it's there already. A
/// task with no due date, or with the one ms-todo set for an earlier My
/// Day, becomes due today.
pub(crate) fn add_plan(row: &TaskRow, today: NaiveDate) -> Option<TaskPlan> {
    let current = my_day_of(row);
    if current == Some(today) {
        return None;
    }
    let due = graph_due_date(&row.raw);
    let ours = due_set(row) && due.is_some() && due == current;
    let mut fields = Map::new();
    fields.insert(MY_DAY.into(), json!(today.format(DATE_FORMAT).to_string()));
    let due = if due.is_none() || ours {
        fields.insert(DUE_SET.into(), json!(true));
        Some(Some(today))
    } else {
        if has_due_set(row) {
            // The user's due date: nothing of ours to take away later.
            fields.insert(DUE_SET.into(), Value::Null);
        }
        None
    };
    Some(TaskPlan {
        fields,
        expect: None,
        due,
    })
}

/// Taking `row` out of My Day, or `None` if it isn't in one. The due date
/// goes too when ms-todo set it, it's still that day, and the task is open
/// (Q13; the rollover's rule).
pub(crate) fn remove_plan(row: &TaskRow) -> Option<TaskPlan> {
    let day = my_day_of(row).or_else(|| {
        // A `myDay` that isn't a date is still ours to remove.
        row.extension
            .as_ref()
            .and_then(|extension| extension.get(MY_DAY))
            .map(|_| NaiveDate::MIN)
    })?;
    let mut fields = Map::new();
    fields.insert(MY_DAY.into(), Value::Null);
    if has_due_set(row) {
        fields.insert(DUE_SET.into(), Value::Null);
    }
    let clears_due = due_set(row) && !completed(row) && graph_due_date(&row.raw) == Some(day);
    Some(TaskPlan {
        fields,
        expect: None,
        due: clears_due.then_some(None),
    })
}

/// The outbox operations for `plan` on `row`, numbered from `first` in the
/// command `command_id`: the extension write, then any due-date edit.
pub(crate) fn plan_ops(
    command_id: &str,
    first: usize,
    row: &TaskRow,
    plan: &TaskPlan,
    action: TaskAction,
) -> Vec<NewOp> {
    let extension_op = |op_id, fields| task_extension_op(op_id, row, fields, action);
    // `myDayDueSet: true` is written last, and only once ms-todo's own
    // due-date edit was made: an edit skipped because the phone set a
    // date meanwhile must never leave the flag on that date (D-054).
    let mut fields = plan.fields.clone();
    let sets_flag = fields.get(DUE_SET) == Some(&Value::Bool(true)) && plan.due.is_some();
    if sets_flag {
        if has_due_set(row) {
            fields.insert(DUE_SET.into(), Value::Null);
        } else {
            fields.remove(DUE_SET);
        }
    }
    let mut first_op = extension_op(op_id_for(command_id, first), fields);
    if let Some(expect) = &plan.expect {
        first_op.payload["expect"] = expect.clone();
    }
    let mut ops = vec![first_op];
    if let Some(due) = plan.due {
        let body = graph_body(&[Field::Due(due)], &user_time_zone());
        let edit_id = op_id_for(command_id, first + 1);
        let mut edit = update_op(edit_id.clone(), row, &body, action);
        // Only while the due date is still the one this was planned from:
        // a change from the phone meanwhile is kept.
        let planned_from = graph_due_date(&row.raw).map(|day| day.format(DATE_FORMAT).to_string());
        edit.payload["expect_due"] = json!(planned_from);
        ops.push(edit);
        if sets_flag {
            let mut flag = Map::new();
            flag.insert(DUE_SET.into(), Value::Bool(true));
            let mut flag_op = extension_op(op_id_for(command_id, first + 2), flag);
            flag_op.payload["after"] = json!(edit_id);
            ops.push(flag_op);
        }
    }
    ops
}

/// `myday add` and `myday remove` (and the TUI's toggle), `action` being
/// `MyDayAdd` or `MyDayRemove`: one command, an extension write per task
/// that changes, and a due-date edit where the due date changes. Tasks
/// already where they're asked to be are left alone.
pub(crate) async fn change(
    state: &State,
    targets: Vec<Target>,
    action: TaskAction,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let today = state.my_day.today();
    let add = action == TaskAction::MyDayAdd;
    let planned: Vec<(&TaskRow, TaskPlan)> = targets
        .iter()
        .filter_map(|target| {
            let plan = if add {
                add_plan(&target.row, today)
            } else {
                remove_plan(&target.row)
            };
            plan.map(|plan| (&target.row, plan))
        })
        .collect();
    if dry_run {
        let due = due_changed(&planned);
        let changes = if add {
            json!({ MY_DAY: today.format(DATE_FORMAT).to_string(), "due_today": due })
        } else {
            json!({ MY_DAY: null, "due_cleared": due })
        };
        return Ok(ResponseData::Plan(dry_run_plan(action, &planned, changes)));
    }
    let ops = command_ops(&op_id, &planned, action);
    queue(state, &op_id, None, ops, action).await
}

/// The local IDs of the tasks whose due date `planned` changes.
pub(crate) fn due_changed<'a>(planned: &[(&'a TaskRow, TaskPlan)]) -> Vec<&'a str> {
    planned
        .iter()
        .filter(|(_, plan)| plan.due.is_some())
        .map(|(row, _)| row.local_id.as_str())
        .collect()
}

/// What a dry run of `planned` answers.
pub(crate) fn dry_run_plan(
    action: TaskAction,
    planned: &[(&TaskRow, TaskPlan)],
    changes: Value,
) -> Plan {
    Plan {
        action,
        list: None,
        targets: planned
            .iter()
            .map(|(row, _)| PlannedTask {
                id: row.local_id.clone(),
                title: row.title.clone(),
                list_id: row.list_local_id.clone(),
            })
            .collect(),
        lists: Vec::new(),
        changes,
    }
}

/// Every task's operations in `planned`, as one command `op_id`.
pub(crate) fn command_ops(
    op_id: &str,
    planned: &[(&TaskRow, TaskPlan)],
    action: TaskAction,
) -> Vec<NewOp> {
    let mut ops = Vec::new();
    for (row, plan) in planned {
        let mut more = plan_ops(op_id, ops.len(), row, plan, action);
        ops.append(&mut more);
    }
    ops
}

/// `Request::MyDay`: today's My Day, and what could go in it.
pub(crate) async fn my_day(state: &State) -> Result<ResponseData, ErrorPayload> {
    let lists_sync = read_state(state, LISTS_SCOPE).await?;
    let today = state.my_day.today();
    let lists = state.store.lists().await.map_err(store_error)?;
    let (rows, suggestions, sync, last_rollover) = tokio::try_join!(
        async {
            state
                .store
                .tasks_in_view(View::MyDay(today))
                .await
                .map_err(store_error)
        },
        suggestions(state, today, &lists),
        all_lists_state(state, lists_sync),
        rollover::last_rollover(state),
    )?;
    Ok(ResponseData::MyDay(MyDay {
        date: today.format(DATE_FORMAT).to_string(),
        tasks: rows.iter().map(crate::entities::task_entity).collect(),
        suggestions,
        sync,
        last_rollover: last_rollover.map(|day| day.format(DATE_FORMAT).to_string()),
    }))
}

/// My Day as `doctor` reports it.
pub(crate) async fn status(state: &State) -> Result<ms_todo_protocol::MyDayStatus, ErrorPayload> {
    let today = state.my_day.today();
    Ok(ms_todo_protocol::MyDayStatus {
        date: today.format(DATE_FORMAT).to_string(),
        count: state.store.my_day_count(today).await.map_err(store_error)?,
        rollover_time: state.my_day.rollover_label(),
        last_rollover: rollover::last_rollover(state)
            .await?
            .map(|day| day.format(DATE_FORMAT).to_string()),
        problem: state.my_day.problem.clone(),
    })
}

/// A new task's My Day fields for `today`, and whether it's due today:
/// when it has no due date (or start date, which Graph makes the due
/// date, S11) of its own.
pub(crate) fn new_task_fields(fields: &mut Vec<Field>, today: NaiveDate) -> Map<String, Value> {
    let mut extension = Map::new();
    extension.insert(MY_DAY.into(), json!(today.format(DATE_FORMAT).to_string()));
    let dated = fields
        .iter()
        .any(|field| matches!(field, Field::Due(Some(_)) | Field::Start(_)));
    if !dated {
        fields.push(Field::Due(Some(today)));
        extension.insert(DUE_SET.into(), json!(true));
    }
    extension
}

#[cfg(test)]
mod tests {
    use super::*;
    use ms_todo_store::OpKind;

    fn row(raw: Value, extension: Option<Value>) -> TaskRow {
        TaskRow {
            local_id: "t1".into(),
            graph_id: Some("T1".into()),
            list_local_id: "l1".into(),
            title: "Call the bank".into(),
            raw: raw.as_object().cloned().expect("object"),
            extension,
            sync_state: "synced".into(),
        }
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, DATE_FORMAT).expect("day")
    }

    fn due(date: &str) -> Value {
        json!({ "dateTime": format!("{date}T00:00:00.0000000"), "timeZone": "UTC" })
    }

    #[test]
    fn adding_a_task_with_no_due_date_makes_it_due_today() {
        let plan = add_plan(
            &row(json!({ "status": "notStarted" }), None),
            day("2026-09-25"),
        )
        .expect("plan");
        assert_eq!(
            Value::Object(plan.fields),
            json!({ "myDay": "2026-09-25", "myDayDueSet": true })
        );
        assert_eq!(plan.due, Some(Some(day("2026-09-25"))));
    }

    #[test]
    fn adding_a_task_with_a_due_date_keeps_it() {
        let task = row(json!({ "dueDateTime": due("2026-10-02") }), None);
        let plan = add_plan(&task, day("2026-09-25")).expect("plan");
        assert_eq!(Value::Object(plan.fields), json!({ "myDay": "2026-09-25" }));
        assert_eq!(plan.due, None);
        // Already there: nothing to do.
        let there = row(json!({}), Some(json!({ "myDay": "2026-09-25" })));
        assert_eq!(add_plan(&there, day("2026-09-25")), None);
    }

    #[test]
    fn adding_again_moves_a_due_date_ms_todo_set_but_not_the_users() {
        let ours = row(
            json!({ "dueDateTime": due("2026-09-24") }),
            Some(json!({ "myDay": "2026-09-24", "myDayDueSet": true })),
        );
        let plan = add_plan(&ours, day("2026-09-25")).expect("plan");
        assert_eq!(plan.due, Some(Some(day("2026-09-25"))));
        let moved = row(
            json!({ "dueDateTime": due("2026-09-30") }),
            Some(json!({ "myDay": "2026-09-24", "myDayDueSet": true })),
        );
        let plan = add_plan(&moved, day("2026-09-25")).expect("plan");
        assert_eq!(plan.due, None);
        assert_eq!(
            Value::Object(plan.fields),
            json!({ "myDay": "2026-09-25", "myDayDueSet": null })
        );
    }

    #[test]
    fn removing_clears_only_a_due_date_ms_todo_set_and_nobody_changed() {
        let ours = row(
            json!({ "status": "notStarted", "dueDateTime": due("2026-09-25") }),
            Some(json!({ "myDay": "2026-09-25", "myDayDueSet": true })),
        );
        let plan = remove_plan(&ours).expect("plan");
        assert_eq!(
            Value::Object(plan.fields.clone()),
            json!({ "myDay": null, "myDayDueSet": null })
        );
        assert_eq!(plan.due, Some(None));

        let changed = row(
            json!({ "status": "notStarted", "dueDateTime": due("2026-09-28") }),
            Some(json!({ "myDay": "2026-09-25", "myDayDueSet": true })),
        );
        assert_eq!(remove_plan(&changed).expect("plan").due, None);

        let users = row(
            json!({ "status": "notStarted", "dueDateTime": due("2026-09-25") }),
            Some(json!({ "myDay": "2026-09-25" })),
        );
        let plan = remove_plan(&users).expect("plan");
        assert_eq!(plan.due, None);
        assert_eq!(Value::Object(plan.fields), json!({ "myDay": null }));

        let done = row(
            json!({ "status": "completed", "dueDateTime": due("2026-09-25") }),
            Some(json!({ "myDay": "2026-09-25", "myDayDueSet": true })),
        );
        assert_eq!(remove_plan(&done).expect("plan").due, None);

        assert_eq!(remove_plan(&row(json!({}), None)), None);
    }

    #[test]
    fn a_new_task_is_due_today_only_without_a_date_of_its_own() {
        let mut fields = vec![Field::Title("Call the bank".into())];
        let extension = new_task_fields(&mut fields, day("2026-09-25"));
        assert_eq!(
            Value::Object(extension),
            json!({ "myDay": "2026-09-25", "myDayDueSet": true })
        );
        assert!(fields.contains(&Field::Due(Some(day("2026-09-25")))));

        let mut dated = vec![Field::Due(Some(day("2026-10-01")))];
        let extension = new_task_fields(&mut dated, day("2026-09-25"));
        assert_eq!(Value::Object(extension), json!({ "myDay": "2026-09-25" }));
        assert_eq!(dated.len(), 1);
    }

    #[test]
    fn the_extension_write_goes_before_the_due_date_edit() {
        let task = row(json!({ "status": "notStarted" }), None);
        let plan = add_plan(&task, day("2026-09-25")).expect("plan");
        let ops = plan_ops("op", 0, &task, &plan, TaskAction::MyDayAdd);
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].op, OpKind::TaskExtension);
        assert_eq!(ops[0].op_id, "op");
        assert_eq!(ops[0].payload["body"], json!({ "myDay": "2026-09-25" }));
        assert_eq!(ops[1].op, OpKind::Update);
        assert_eq!(ops[1].op_id, "op.1");
        assert_eq!(ops[1].action, "my_day_add");
        // The flag only after ms-todo's own due-date edit.
        assert_eq!(ops[2].op, OpKind::TaskExtension);
        assert_eq!(ops[2].payload["body"], json!({ "myDayDueSet": true }));
        assert_eq!(ops[2].payload["after"], "op.1");
        assert_eq!(ops[1].payload["expect_due"], Value::Null);
        assert!(ops[1].payload.get("expect_due").is_some());
    }
}
