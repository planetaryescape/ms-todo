//! Related tasks (D-067): `tasks relate T1 T2` and `tasks unrelate T1 T2`.
//! Graph has no relation between tasks, so each task keeps the other's
//! Graph ID in ms-todo's extension, `related`, written through the
//! `task_extension` GET, merge and whole-document write as `myDay` and
//! `assignee` are: both directions, one operation per task, under one
//! `op_id`, so `undo` reverses both by the changed-since rule.
//!
//! Graph IDs, not local ones, because another machine's ms-todo reads the
//! same extension. A task moved by this machine has a new Graph ID; a link
//! to its old one is still followed, through the finished move.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Entity, ErrorPayload, Plan, ResponseData, TaskAction};
use ms_todo_store::TaskRow;
use serde_json::{Map, Value, json};

use crate::handlers::{State, error_payload, store_error};
use crate::outbox::op_id_for;
use crate::task_children::planned;
use crate::task_resolution::Target;
use crate::task_writes::{queue, task_extension_op};

/// The extension field: the Graph IDs of the tasks this one is linked to.
pub(crate) const RELATED: &str = "related";

/// The Graph IDs `extension` links to, in order.
pub(crate) fn ids(extension: Option<&Value>) -> Vec<String> {
    extension
        .and_then(|extension| extension.get(RELATED))
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Link the two `targets` to each other (`relate`), or take the link
/// away. A direction already as asked is left out, so a repeat queues
/// nothing.
pub(crate) async fn change(
    state: &State,
    targets: Vec<Target>,
    relate: bool,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let [first, second] = <[Target; 2]>::try_from(targets).map_err(|_| {
        error_payload(
            ErrorKind::InvalidInput,
            "name exactly two different tasks to link or unlink".into(),
        )
    })?;
    let first_graph = graph_id(&first)?;
    let second_graph = graph_id(&second)?;
    let action = if relate {
        TaskAction::Relate
    } else {
        TaskAction::Unrelate
    };
    let first_links = links_to(state, &first.row, second.local_id()).await?;
    let second_links = links_to(state, &second.row, first.local_id()).await?;
    let wanted = [
        (
            &first,
            planned_ids(&first.row, &first_links, &second_graph, relate),
        ),
        (
            &second,
            planned_ids(&second.row, &second_links, &first_graph, relate),
        ),
    ];
    if dry_run {
        let mut changes = Map::new();
        for (target, ids) in &wanted {
            let after = ids
                .clone()
                .unwrap_or_else(|| self::ids(target.row.extension.as_ref()));
            changes.insert(target.local_id().to_owned(), json!({ RELATED: after }));
        }
        return Ok(ResponseData::Plan(Plan {
            action,
            list: None,
            targets: [&first, &second].into_iter().map(planned).collect(),
            lists: Vec::new(),
            changes: Value::Object(changes),
        }));
    }
    let mut ops = Vec::new();
    for (target, ids) in wanted {
        let Some(ids) = ids else { continue };
        // An empty list leaves the extension rather than stay as `[]`.
        let value = if ids.is_empty() {
            Value::Null
        } else {
            json!(ids)
        };
        let fields = Map::from_iter([(RELATED.to_owned(), value)]);
        ops.push(task_extension_op(
            op_id_for(&op_id, ops.len()),
            &target.row,
            fields,
            action,
        ));
    }
    queue(state, &op_id, None, ops, action).await
}

fn graph_id(target: &Target) -> Result<String, ErrorPayload> {
    target.row.graph_id.clone().ok_or_else(|| {
        error_payload(
            ErrorKind::InvalidInput,
            format!(
                "{:?} hasn't reached Microsoft To Do yet, so it has no ID another device can \
                 follow; link it once it has (`ms-todo outbox list`)",
                target.title()
            ),
        )
    })
}

/// Which of `row`'s related IDs mean the task `other` (a local ID): its
/// Graph ID now, or one it had before this cache moved it.
async fn links_to(state: &State, row: &TaskRow, other: &str) -> Result<Vec<String>, ErrorPayload> {
    let mut linked = Vec::new();
    for id in ids(row.extension.as_ref()) {
        if find(state, &id)
            .await?
            .is_some_and(|found| found.local_id == other)
        {
            linked.push(id);
        }
    }
    Ok(linked)
}

/// `row`'s related IDs linking to `other` (its Graph ID now) or not,
/// `linked` being those that mean it already; `None` when that changes
/// nothing. Linking again replaces an old ID with the current one.
fn planned_ids(row: &TaskRow, linked: &[String], other: &str, relate: bool) -> Option<Vec<String>> {
    let before = ids(row.extension.as_ref());
    let mut after: Vec<String> = before
        .iter()
        .filter(|id| *id != other && !linked.contains(id))
        .cloned()
        .collect();
    if relate {
        after.push(other.to_owned());
    }
    // Only the order of an ID linked already may differ: that's no change.
    let same = after.len() == before.len() && after.iter().all(|id| before.contains(id));
    (!same).then_some(after)
}

/// Give each of `entities` that links to other tasks `related`: each
/// linked task as `{ id, graph_id, title, list_id, status }`, its fields
/// null when this cache doesn't have it (deleted, or not synced yet).
/// Costs nothing for the tasks that link to none.
pub(crate) async fn annotate(state: &State, entities: &mut [Entity]) -> Result<(), ErrorPayload> {
    for entity in entities {
        let linked = ids(entity
            .get("extensions")
            .and_then(|extensions| extensions.get(0)));
        if linked.is_empty() {
            continue;
        }
        let mut related = Vec::with_capacity(linked.len());
        for graph_id in linked {
            related.push(match find(state, &graph_id).await? {
                Some(row) => json!({
                    "id": row.local_id,
                    "graph_id": graph_id,
                    "title": row.title,
                    "list_id": row.list_local_id,
                    "status": row.raw.get("status").cloned().unwrap_or(Value::Null),
                }),
                None => json!({
                    "id": null,
                    "graph_id": graph_id,
                    "title": null,
                    "list_id": null,
                    "status": null,
                }),
            });
        }
        entity.insert(RELATED.into(), Value::Array(related));
    }
    Ok(())
}

/// The cached task Graph knows as `graph_id`, or the one this cache moved
/// from it.
async fn find(state: &State, graph_id: &str) -> Result<Option<TaskRow>, ErrorPayload> {
    if let Some(row) = state.store.task(graph_id).await.map_err(store_error)? {
        return Ok(Some(row));
    }
    match state
        .store
        .moved_from(graph_id)
        .await
        .map_err(store_error)?
    {
        Some(local_id) => state.store.task(&local_id).await.map_err(store_error),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(extension: Option<Value>) -> TaskRow {
        TaskRow {
            local_id: "t1".into(),
            graph_id: Some("G1".into()),
            list_local_id: "l".into(),
            title: "One".into(),
            raw: Map::new(),
            extension,
            sync_state: "synced".into(),
        }
    }

    #[test]
    fn a_link_is_added_once_and_taken_away_once() {
        let none = row(None);
        assert_eq!(
            planned_ids(&none, &[], "G2", true),
            Some(vec!["G2".to_owned()])
        );
        assert_eq!(
            planned_ids(&none, &[], "G2", false),
            None,
            "nothing to take"
        );
        let linked = row(Some(json!({ "related": ["G2", "G3"], "myDay": "x" })));
        let to_g2 = ["G2".to_owned()];
        assert_eq!(
            planned_ids(&linked, &to_g2, "G2", true),
            None,
            "linked already"
        );
        assert_eq!(
            planned_ids(&linked, &to_g2, "G2", false),
            Some(vec!["G3".to_owned()])
        );
    }

    #[test]
    fn a_link_by_an_old_graph_id_is_the_same_link() {
        // G2 moved and is G9 now.
        let linked = row(Some(json!({ "related": ["G2", "G3"] })));
        let old = ["G2".to_owned()];
        assert_eq!(
            planned_ids(&linked, &old, "G9", true),
            Some(vec!["G3".to_owned(), "G9".to_owned()]),
            "brought up to date"
        );
        assert_eq!(
            planned_ids(&linked, &old, "G9", false),
            Some(vec!["G3".to_owned()])
        );
    }

    #[test]
    fn ids_ignore_what_is_not_a_string() {
        let extension = json!({ "related": ["G2", 3, null, "G4"] });
        assert_eq!(ids(Some(&extension)), ["G2", "G4"]);
        assert!(ids(Some(&json!({ "related": "G2" }))).is_empty());
        assert!(ids(None).is_empty());
    }
}
