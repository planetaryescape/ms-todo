//! `lists merge FROM --into TO` (D-067): every open task of one list (and
//! its completed ones, when asked) moved to another through the move job
//! of D-051, one move per task under one `op_id`, so one `undo` moves them
//! all back. Each move copies, checks and only then deletes, and resumes
//! after a crash as any move does.
//!
//! With `--delete-source`, the emptied list is deleted by an operation of
//! its own, `<op_id>.delete`, queued now but sent only once none of the
//! merge's moves is unresolved (`after_command`), and only if every move
//! is done, the cache holds no task in the list, and Microsoft To Do shows
//! it empty (`outbox::list_write`). Otherwise it's refused and the list
//! kept. It's its own command so that `undo` makes the list again before
//! the moves can go back into it.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, Plan, PlannedList, PlannedTask, ResponseData, TaskAction};
use ms_todo_store::{ListRow, ListWrite, TaskRow};
use serde_json::json;

use crate::handlers::{State, error_payload, store_error};
use crate::list_lifecycle::{lifecycle, refuse_built_in};
use crate::list_resolution::ListRef;
use crate::list_writes::{find, queue_lists};
use crate::outbox::op_id_for;
use crate::task_children::invalid;
use crate::task_writes::{move_op, queue};

/// What the merge was asked to do.
pub(crate) struct Merge<'a> {
    pub from: &'a str,
    pub into: &'a str,
    pub include_completed: bool,
    pub delete_source: bool,
}

/// The `op_id` of a merge's delete of its emptied list.
pub(crate) fn delete_op_id(op_id: &str) -> String {
    format!("{op_id}.delete")
}

pub(crate) async fn merge(
    state: &State,
    lists: &[ListRow],
    merge: &Merge<'_>,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let source = find(lists, merge.from)?;
    let target = find(lists, merge.into)?;
    if source.local_id == target.local_id {
        return Err(invalid("a list can't be merged into itself".into()));
    }
    if merge.delete_source {
        refuse_built_in(source, "deleted")?;
    }
    let source_ref = ListRef::of(source).ok_or_else(|| {
        error_payload(ErrorKind::Internal, "a cached list has no reference".into())
    })?;
    crate::freshness::ensure_list_ready(state, &source_ref).await?;
    let rows = state
        .store
        .tasks_in_list(&source.local_id)
        .await
        .map_err(store_error)?;
    let (moving, left): (Vec<TaskRow>, Vec<TaskRow>) = rows
        .into_iter()
        .partition(|row| merge.include_completed || !crate::my_day::completed(row));
    if merge.delete_source && !left.is_empty() {
        return Err(invalid(format!(
            "{:?} holds {} completed task(s), which deleting it would lose; add \
             --include-completed to move them too, or leave out --delete-source",
            source.display_name,
            left.len()
        )));
    }
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action: TaskAction::MergeList,
            list: ListRef::of(target).map(|target| target.candidate()),
            targets: moving
                .iter()
                .map(|row| PlannedTask {
                    id: row.local_id.clone(),
                    title: row.title.clone(),
                    list_id: row.list_local_id.clone(),
                })
                .collect(),
            lists: vec![PlannedList {
                id: source.local_id.clone(),
                name: source.display_name.clone(),
                changes: json!({
                    "deleted": merge.delete_source,
                    "completed_left": left.len(),
                }),
            }],
            changes: json!({ "list_id": target.local_id }),
        }));
    }
    let ops = moving
        .iter()
        .enumerate()
        .map(|(index, row)| move_op(op_id_for(&op_id, index), row, &target.local_id))
        .collect();
    let answer = queue(state, &op_id, None, ops, TaskAction::MergeList).await?;
    if merge.delete_source {
        let write = ListWrite::DeleteWhenMoved {
            after: op_id.clone(),
            moves: moving.len() as u64,
        };
        let delete_id = delete_op_id(&op_id);
        let op = lifecycle(delete_id.clone(), source, TaskAction::DeleteList, write);
        queue_lists(state, &delete_id, None, vec![op], TaskAction::DeleteList).await?;
    }
    Ok(answer)
}
