//! `lists create|rename|delete` (rung 8e): planned here against the
//! cached lists, then queued in the outbox like a folder change
//! (`list_writes::queue_lists`). A dry run answers the plan.
//!
//! The default list ("Tasks") and Flagged Emails are Microsoft To Do's
//! own: they can't be renamed or deleted. A name another list has already
//! is refused, since `--list NAME` would then be ambiguous. A delete takes
//! every task in the list with it; the plan says how many, and only an
//! empty list's delete can be undone.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, ListChange, Plan, PlannedList, ResponseData, TaskAction};
use ms_todo_store::{ListExtensionOp, ListLifecycleOp, ListOp, ListRow, ListWrite};
use serde_json::{Map, Value, json};

use crate::folders;
use crate::handlers::{State, error_payload, store_error};
use crate::list_writes::{find, queue_lists};
use crate::outbox::op_id_for;
use crate::task_writes::action_name;

/// `wellknownListName`s of the lists Microsoft To Do keeps for itself.
const BUILT_IN: [(&str, &str); 2] = [
    ("defaultList", "Tasks"),
    ("flaggedEmails", "Flagged Emails"),
];

pub(crate) async fn change(
    state: &State,
    lists: &[ListRow],
    change: &ListChange,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let (action, planned, ops) = match change {
        ListChange::CreateList { name, folder } => {
            let name = new_name(lists, name, None)?;
            let new = new_list(&name);
            let mut changes = json!({ "displayName": name });
            let mut ops = vec![lifecycle(
                op_id_for(&op_id, 0),
                &new,
                TaskAction::CreateList,
                ListWrite::Create { name },
            )];
            if let Some(folder) = folder {
                let planned = folders::move_lists(lists, &[&new], Some(folder))?;
                let fields: Map<String, Value> =
                    planned.into_iter().flat_map(|(_, fields)| fields).collect();
                changes["folder"] = fields
                    .get(ms_todo_store::FOLDER_FIELD)
                    .cloned()
                    .unwrap_or(Value::Null);
                ops.push(ListOp::Extension(ListExtensionOp {
                    op_id: op_id_for(&op_id, 1),
                    list_local_id: new.local_id.clone(),
                    action: action_name(TaskAction::MoveList).to_owned(),
                    fields,
                }));
            }
            (TaskAction::CreateList, planned(&new, changes), ops)
        }
        ListChange::RenameList { list, name } => {
            let list = find(lists, list)?;
            refuse_built_in(list, "renamed")?;
            let name = new_name(lists, name, Some(list))?;
            let changes = json!({ "displayName": name });
            let ops = if list.display_name == name {
                // Already so: nothing to send.
                Vec::new()
            } else {
                vec![lifecycle(
                    op_id_for(&op_id, 0),
                    list,
                    TaskAction::RenameList,
                    ListWrite::Rename { name },
                )]
            };
            (TaskAction::RenameList, planned(list, changes), ops)
        }
        ListChange::DeleteList { list } => {
            let list = find(lists, list)?;
            refuse_built_in(list, "deleted")?;
            let (busy, tasks) = tokio::try_join!(
                state.store.list_has_unresolved_task_ops(&list.local_id),
                state.store.list_task_count(&list.local_id),
            )
            .map_err(store_error)?;
            if busy {
                return Err(error_payload(
                    ErrorKind::InvalidInput,
                    format!(
                        "{:?} has changes to its tasks still waiting to be sent; let them reach \
                         Microsoft To Do first (`ms-todo outbox list`)",
                        list.display_name
                    ),
                ));
            }
            let changes = json!({ "deleted": true, "tasks": tasks, "undoable": tasks == 0 });
            let ops = vec![lifecycle(
                op_id_for(&op_id, 0),
                list,
                TaskAction::DeleteList,
                ListWrite::Delete,
            )];
            (TaskAction::DeleteList, planned(list, changes), ops)
        }
        _ => {
            return Err(error_payload(
                ErrorKind::Internal,
                "not a list's create, rename or delete".into(),
            ));
        }
    };
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action,
            list: None,
            targets: Vec::new(),
            lists: vec![planned],
            changes: Value::Null,
        }));
    }
    queue_lists(state, &op_id, None, ops, action).await
}

fn planned(list: &ListRow, changes: Value) -> PlannedList {
    PlannedList {
        id: list.local_id.clone(),
        name: list.display_name.clone(),
        changes,
    }
}

/// A list only ms-todo knows yet, called `name`, with a fresh local ID.
fn new_list(name: &str) -> ListRow {
    ListRow {
        local_id: uuid::Uuid::new_v4().to_string(),
        graph_id: None,
        display_name: name.to_owned(),
        wellknown_list_name: Some("none".into()),
        raw: Map::new(),
        extension: None,
        sync_state: "pending".into(),
    }
}

/// `name`, trimmed, if it's a name a list can take: not empty, and no
/// other list's (`renamed` may keep its own).
fn new_name(
    lists: &[ListRow],
    name: &str,
    renamed: Option<&ListRow>,
) -> Result<String, ErrorPayload> {
    let name = name.trim();
    if name.is_empty() {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            "a list's name can't be empty".into(),
        ));
    }
    let taken = lists.iter().any(|list| {
        list.display_name == name && renamed.is_none_or(|renamed| renamed.local_id != list.local_id)
    });
    if taken {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            format!(
                "a list is called {name:?} already; pick another name, so `--list` can tell them \
                 apart"
            ),
        ));
    }
    Ok(name.to_owned())
}

fn refuse_built_in(list: &ListRow, verb: &str) -> Result<(), ErrorPayload> {
    let built_in = BUILT_IN
        .iter()
        .find(|(wellknown, _)| list.wellknown_list_name.as_deref() == Some(*wellknown));
    match built_in {
        Some((_, what)) => Err(error_payload(
            ErrorKind::InvalidInput,
            format!(
                "{:?} is Microsoft To Do's own {what} list, so it can't be {verb}",
                list.display_name
            ),
        )),
        None => Ok(()),
    }
}

/// A create, rename or delete of `list`, as `action`.
pub(crate) fn lifecycle(
    op_id: String,
    list: &ListRow,
    action: TaskAction,
    write: ListWrite,
) -> ListOp {
    ListOp::Lifecycle(ListLifecycleOp {
        op_id,
        list_local_id: list.local_id.clone(),
        action: action_name(action).to_owned(),
        write,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(local: &str, name: &str, wellknown: &str) -> ListRow {
        ListRow {
            local_id: local.into(),
            graph_id: Some(local.to_uppercase()),
            display_name: name.into(),
            wellknown_list_name: Some(wellknown.into()),
            raw: Map::new(),
            extension: None,
            sync_state: "synced".into(),
        }
    }

    #[test]
    fn a_name_must_be_new_and_not_blank() {
        let lists = vec![
            list("l1", "Garden", "none"),
            list("l2", "Tasks", "defaultList"),
        ];
        assert!(new_name(&lists, "  ", None).is_err());
        assert!(new_name(&lists, "Garden", None).is_err());
        assert_eq!(new_name(&lists, " Shed ", None).expect("new"), "Shed");
        // A rename may keep the list's own name.
        assert!(new_name(&lists, "Garden", Some(&lists[0])).is_ok());
    }

    #[test]
    fn microsoft_to_dos_own_lists_are_refused() {
        let tasks = list("l2", "Tasks", "defaultList");
        let error = refuse_built_in(&tasks, "deleted").expect_err("default");
        assert!(
            error.message.contains("can't be deleted"),
            "{}",
            error.message
        );
        assert!(refuse_built_in(&list("l3", "Flagged", "flaggedEmails"), "renamed").is_err());
        assert!(refuse_built_in(&list("l1", "Garden", "none"), "renamed").is_ok());
    }
}
