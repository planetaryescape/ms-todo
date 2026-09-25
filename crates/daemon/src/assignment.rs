//! Assignment (docs/blueprint/05-custom-features.md#assignment, D-020): who
//! a task waits on, `assignee` in our extension. It means something only
//! to ms-todo: nobody is told, and Microsoft To Do never shows it. What
//! the To Do apps do see is the status it's paired with: setting an
//! assignee makes an open task `waitingOnOthers`, marked
//! `assigneeStatusSet` so that clearing the assignee can make it
//! `notStarted` again, unless its status has changed since.
//!
//! Each change is My Day's shape (D-054): one extension write per task
//! (GET, merge, write, through the outbox), then, when the status changes,
//! a status edit made only while Graph's status is still the one it was
//! planned from, then the flag, written only if that edit was made. The
//! extension goes first: if it fails, the edit waiting on it fails too, so
//! a status is never changed for an assignee that didn't land.

use ms_todo_protocol::{Clearable, ErrorPayload, TaskAction};
use ms_todo_store::{NewOp, TaskRow};
use serde_json::{Map, Value, json};

use crate::my_day::completed;
use crate::outbox::EXPECT_FIELDS;
use crate::outbox::op_id_for;
use crate::task_children::invalid;
use crate::task_fields::{Field, graph_body, user_time_zone};
use crate::task_writes::{task_extension_op, update_op};

/// Who the task waits on: free text or an email.
pub(crate) const ASSIGNEE: &str = "assignee";
/// True when ms-todo made the task `waitingOnOthers` for its assignee.
pub(crate) const STATUS_SET: &str = "assigneeStatusSet";
const WAITING: &str = "waitingOnOthers";
const NOT_STARTED: &str = "notStarted";
/// Longer than any name or email address; a guard against pasted text.
const MAX_CHARS: usize = 200;

/// In a status edit's or flag's payload: the assignee write it pairs
/// with. An undo that leaves that write alone leaves these alone too, so
/// it never takes the status from under another machine's assignee.
pub(crate) const PART_OF: &str = "part_of";

/// `--assignee '*'`: anyone.
pub(crate) const ANYONE: &str = "*";

/// What one assignment change does to one task.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TaskPlan {
    /// The extension's fields to set; a null removes one.
    pub fields: Map<String, Value>,
    /// The new status, when it changes, and the one it's planned from.
    pub status: Option<(&'static str, String)>,
    /// Write `assigneeStatusSet: true` once the status edit is made.
    pub sets_flag: bool,
}

/// An assignee as typed, trimmed, or why it can't be one.
pub(crate) fn clean(name: &str) -> Result<String, ErrorPayload> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(
            "an assignee can't be empty; --clear-assignee removes one".into(),
        ));
    }
    if name == ANYONE {
        return Err(invalid(
            "`*` means anyone when listing; it can't be an assignee".into(),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("an assignee can't hold control characters".into()));
    }
    if name.chars().count() > MAX_CHARS {
        return Err(invalid(format!(
            "an assignee is at most {MAX_CHARS} characters"
        )));
    }
    Ok(name.to_owned())
}

/// A task's assignee, if it has one.
pub(crate) fn assignee_of(row: &TaskRow) -> Option<&str> {
    row.extension
        .as_ref()?
        .get(ASSIGNEE)?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

/// Whether `row` is assigned to `wanted`, already trimmed and lowercased:
/// anyone for `*`, else the same name, whatever its case.
pub(crate) fn assigned_to(row: &TaskRow, wanted: &str) -> bool {
    assignee_of(row).is_some_and(|name| wanted == ANYONE || name.to_lowercase() == wanted)
}

fn status_set(row: &TaskRow) -> bool {
    row.extension
        .as_ref()
        .and_then(|extension| extension.get(STATUS_SET))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn status(row: &TaskRow) -> String {
    row.raw
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or(NOT_STARTED)
        .to_owned()
}

/// Assigning `row` to `name`, or `None` when it's assigned to exactly
/// that already. An open task that isn't waiting yet becomes
/// `waitingOnOthers`, unless `keep_status`; a completed one stays
/// completed.
pub(crate) fn set_plan(row: &TaskRow, name: &str, keep_status: bool) -> Option<TaskPlan> {
    if assignee_of(row) == Some(name) {
        return None;
    }
    let mut fields = Map::new();
    fields.insert(ASSIGNEE.into(), json!(name));
    let now = status(row);
    let changes_status = !keep_status && now != WAITING && !completed(row);
    Some(TaskPlan {
        fields,
        status: changes_status.then_some((WAITING, now)),
        // Already ours (a stale flag from an earlier assignment whose
        // status someone changed) needs no second write.
        sets_flag: changes_status && !status_set(row),
    })
}

/// Clearing `row`'s assignee, or `None` when it has none. A task ms-todo
/// made `waitingOnOthers` and that still is goes back to `notStarted`,
/// unless `keep_status`; a status anyone changed since is left alone.
pub(crate) fn clear_plan(row: &TaskRow, keep_status: bool) -> Option<TaskPlan> {
    let extension = row.extension.as_ref()?;
    let fields: Map<String, Value> = [ASSIGNEE, STATUS_SET]
        .into_iter()
        .filter(|key| extension.get(key).is_some())
        .map(|key| (key.to_owned(), Value::Null))
        .collect();
    if fields.is_empty() {
        return None;
    }
    let now = status(row);
    let restores = !keep_status && status_set(row) && now == WAITING;
    Some(TaskPlan {
        fields,
        status: restores.then_some((NOT_STARTED, now)),
        sets_flag: false,
    })
}

/// The plan for `change` on `row`, if it changes anything.
pub(crate) fn plan(
    row: &TaskRow,
    change: &Clearable<String>,
    keep_status: bool,
) -> Option<TaskPlan> {
    match change {
        Clearable::Set(name) => set_plan(row, name, keep_status),
        Clearable::Clear => clear_plan(row, keep_status),
    }
}

/// The outbox operations for `plan` on `row`, numbered from `first` in
/// the command `command_id`: the extension write, then any status edit,
/// then the flag.
pub(crate) fn plan_ops(
    command_id: &str,
    first: usize,
    row: &TaskRow,
    plan: &TaskPlan,
    action: TaskAction,
) -> Vec<NewOp> {
    let extension_op = |op_id, fields| task_extension_op(op_id, row, fields, action);
    let assignee_id = op_id_for(command_id, first);
    let mut ops = vec![extension_op(assignee_id.clone(), plan.fields.clone())];
    let Some((status, planned_from)) = &plan.status else {
        return ops;
    };
    let body = graph_body(&[Field::Status(status)], &user_time_zone());
    let edit_id = op_id_for(command_id, first + 1);
    let mut edit = update_op(edit_id.clone(), row, &body, action);
    // Only while the status is still the one this was planned from: a
    // change from the phone meanwhile is kept.
    edit.payload[EXPECT_FIELDS] = json!({ "status": planned_from });
    // Undone only with the assignee it pairs with (`undo`).
    edit.payload[PART_OF] = json!(assignee_id);
    ops.push(edit);
    if plan.sets_flag {
        let mut flag = Map::new();
        flag.insert(STATUS_SET.into(), Value::Bool(true));
        let mut flag_op = extension_op(op_id_for(command_id, first + 2), flag);
        // Never on a status ms-todo didn't set (D-054's `after`).
        flag_op.payload["after"] = json!(edit_id);
        flag_op.payload[PART_OF] = json!(assignee_id);
        ops.push(flag_op);
    }
    ops
}

/// A new task's assignment: the extension's fields, and the status it's
/// made with, unless `keep_status`.
pub(crate) fn new_task_fields(
    name: &str,
    keep_status: bool,
    fields: &mut Vec<Field>,
) -> Map<String, Value> {
    let mut extension = Map::new();
    extension.insert(ASSIGNEE.into(), json!(name));
    if !keep_status {
        fields.push(Field::Status(WAITING));
        extension.insert(STATUS_SET.into(), json!(true));
    }
    extension
}

/// What a dry run says an assignment change sends: the extension's
/// fields, and the status where any task's changes.
pub(crate) fn dry_run_changes<'a>(
    plans: impl IntoIterator<Item = &'a TaskPlan>,
) -> Map<String, Value> {
    let mut changes = Map::new();
    for plan in plans {
        for (key, value) in &plan.fields {
            changes.insert(key.clone(), value.clone());
        }
        if let Some((status, _)) = plan.status {
            changes.insert("status".into(), json!(status));
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use ms_todo_store::OpKind;

    fn row(status: &str, extension: Option<Value>) -> TaskRow {
        TaskRow {
            local_id: "t1".into(),
            graph_id: Some("T1".into()),
            list_local_id: "l1".into(),
            title: "Get the quote".into(),
            raw: json!({ "id": "T1", "status": status })
                .as_object()
                .cloned()
                .expect("object"),
            extension,
            sync_state: "synced".into(),
        }
    }

    #[test]
    fn a_name_is_trimmed_and_nothing_else_is_one() {
        assert_eq!(clean("  Sam ").expect("name"), "Sam");
        assert_eq!(clean("sam@example.com").expect("email"), "sam@example.com");
        for bad in ["", "   ", "*", "Sam\u{1b}[31m", &"x".repeat(201)] {
            assert!(clean(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn assigning_an_open_task_makes_it_waiting_and_marks_that_ms_todo_did() {
        let plan = set_plan(&row("notStarted", None), "Sam", false).expect("plan");
        assert_eq!(
            Value::Object(plan.fields.clone()),
            json!({ "assignee": "Sam" })
        );
        assert_eq!(plan.status, Some((WAITING, "notStarted".to_owned())));
        assert!(plan.sets_flag);
        let ops = plan_ops("op", 0, &row("notStarted", None), &plan, TaskAction::Edit);
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].op, OpKind::TaskExtension);
        assert_eq!(ops[1].op, OpKind::Update);
        assert_eq!(
            ops[1].payload["body"],
            json!({ "status": "waitingOnOthers" })
        );
        assert_eq!(
            ops[1].payload["expect_fields"],
            json!({ "status": "notStarted" })
        );
        assert_eq!(ops[2].payload["body"], json!({ "assigneeStatusSet": true }));
        assert_eq!(ops[2].payload["after"], "op.1");
    }

    #[test]
    fn a_status_the_user_chose_or_asked_to_keep_is_left_alone() {
        // Waiting already, by the user's hand: no edit, no flag.
        let waiting = set_plan(&row(WAITING, None), "Sam", false).expect("plan");
        assert_eq!(waiting.status, None);
        assert!(!waiting.sets_flag);
        let done = set_plan(&row("completed", None), "Sam", false).expect("plan");
        assert_eq!(done.status, None);
        let kept = set_plan(&row("inProgress", None), "Sam", true).expect("plan");
        assert_eq!(kept.status, None);
        assert_eq!(
            plan_ops("op", 0, &row("inProgress", None), &kept, TaskAction::Edit).len(),
            1
        );
    }

    #[test]
    fn the_same_name_again_changes_nothing_but_a_new_spelling_does() {
        let sam = Some(json!({ "assignee": "Sam", "assigneeStatusSet": true }));
        assert_eq!(set_plan(&row(WAITING, sam.clone()), "Sam", false), None);
        let renamed = set_plan(&row(WAITING, sam), "sam", false).expect("plan");
        assert_eq!(renamed.status, None, "still ms-todo's waiting");
    }

    #[test]
    fn clearing_restores_only_a_waiting_status_ms_todo_set() {
        let ours = Some(json!({ "assignee": "Sam", "assigneeStatusSet": true, "myDay": "x" }));
        let plan = clear_plan(&row(WAITING, ours.clone()), false).expect("plan");
        assert_eq!(
            Value::Object(plan.fields.clone()),
            json!({ "assignee": null, "assigneeStatusSet": null }),
            "only our fields, so the merge keeps myDay"
        );
        assert_eq!(plan.status, Some((NOT_STARTED, WAITING.to_owned())));
        // The user moved it on since: kept.
        let moved = clear_plan(&row("inProgress", ours.clone()), false).expect("plan");
        assert_eq!(moved.status, None);
        // The user made it waiting, not ms-todo: kept.
        let theirs = Some(json!({ "assignee": "Sam" }));
        assert_eq!(
            clear_plan(&row(WAITING, theirs), false)
                .expect("plan")
                .status,
            None
        );
        assert_eq!(
            clear_plan(&row(WAITING, ours), true).expect("plan").status,
            None
        );
        assert_eq!(clear_plan(&row(WAITING, None), false), None);
    }

    #[test]
    fn matching_is_case_insensitive_and_star_is_anyone() {
        let sam = row("notStarted", Some(json!({ "assignee": "Sam Jones" })));
        assert!(assigned_to(&sam, "sam jones"));
        assert!(!assigned_to(&sam, "Sam Jones"), "the caller folds the name");
        assert!(assigned_to(&sam, ANYONE));
        assert!(!assigned_to(&sam, "sam"));
        assert!(!assigned_to(&row("notStarted", None), ANYONE));
        let blank = row("notStarted", Some(json!({ "assignee": "  " })));
        assert!(!assigned_to(&blank, ANYONE));
    }
}
