//! Related tasks (D-067): `tasks relate T1 T2` and `tasks unrelate T1 T2`.
//! Graph has no relation between tasks, so each task keeps the other's
//! Graph ID in ms-todo's extension, `related`, written through the
//! `task_extension` GET, merge and whole-document write as `myDay` and
//! `assignee` are: both directions, one operation per task, under one
//! `op_id`. Each is a change to a set ([`Link`]): it adds or takes away
//! the one ID, on whatever the task holds when it's sent, and `undo` does
//! the opposite the same way, so a later link to a third task is kept and
//! a link never ends up one-way.
//!
//! Graph IDs, not local ones, because another machine's ms-todo reads the
//! same extension. A task moved by this machine has a new Graph ID; a link
//! to its old one is still followed, through the finished move.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Entity, ErrorPayload, Plan, ResponseData, TaskAction};
use ms_todo_store::{NewOp, TaskRow};
use serde::{Deserialize, Serialize};
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
        (&first, planned_link(&first_links, &second_graph, relate)),
        (&second, planned_link(&second_links, &first_graph, relate)),
    ];
    if dry_run {
        let mut changes = Map::new();
        for (target, link) in &wanted {
            let after = link.apply(&ids(target.row.extension.as_ref()));
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
    for (target, link) in wanted {
        let id = op_id_for(&op_id, ops.len());
        ops.extend(link_op(id, &target.row, &link, action));
    }
    queue(state, &op_id, None, ops, action).await
}

/// One task's change to its `related`, as a set: `other` linked or not,
/// and `stale`, older Graph IDs meaning the same task, taken away.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Link {
    pub other: String,
    pub linked: bool,
    #[serde(default)]
    pub stale: Vec<String>,
}

/// Where an operation's payload keeps its [`Link`].
const LINK: &str = "link";

impl Link {
    /// The link `payload` changes, if it's a relate or an unrelate.
    pub(crate) fn of(payload: &Value) -> Option<Self> {
        serde_json::from_value(payload.get(LINK)?.clone()).ok()
    }

    /// What undoes it: the other way round, leaving the old IDs gone.
    pub(crate) fn inverse(&self) -> Self {
        Self {
            other: self.other.clone(),
            linked: !self.linked,
            stale: Vec::new(),
        }
    }

    /// `ids` with this change made, in order; one linked already keeps its
    /// place, so an order that's the same is no change.
    fn apply(&self, ids: &[String]) -> Vec<String> {
        let mut after: Vec<String> = Vec::with_capacity(ids.len() + 1);
        for id in ids {
            let keep = if *id == self.other {
                self.linked && !after.contains(id)
            } else {
                !self.stale.contains(id)
            };
            if keep {
                after.push(id.clone());
            }
        }
        if self.linked && !after.contains(&self.other) {
            after.push(self.other.clone());
        }
        after
    }
}

/// The write making `link` on `row`, as it is now, or `None` when it
/// holds already.
pub(crate) fn link_op(
    op_id: String,
    row: &TaskRow,
    link: &Link,
    action: TaskAction,
) -> Option<NewOp> {
    let before = ids(row.extension.as_ref());
    let after = link.apply(&before);
    if after == before {
        return None;
    }
    let fields = Map::from_iter([(RELATED.to_owned(), related_value(after))]);
    let mut op = task_extension_op(op_id, row, fields, action);
    op.payload[LINK] = json!(link);
    Some(op)
}

/// An empty list leaves the extension rather than stay as `[]`.
fn related_value(ids: Vec<String>) -> Value {
    if ids.is_empty() {
        Value::Null
    } else {
        json!(ids)
    }
}

/// When a relate or unrelate is sent: `related` in `document` (Graph's
/// copy with the operation's fields over it) made from Graph's own list,
/// `graph`, so a link another device made meanwhile is kept.
pub(crate) fn rebase(document: &mut Map<String, Value>, payload: &Value, graph: Option<&Value>) {
    let Some(link) = Link::of(payload) else {
        return;
    };
    match related_value(link.apply(&ids(graph))) {
        Value::Null => document.remove(RELATED),
        value => document.insert(RELATED.to_owned(), value),
    };
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

/// What linking to `other` (its Graph ID now), or unlinking it, changes,
/// `linked` being the IDs that mean that task already. Linking again
/// replaces an old ID with the current one.
fn planned_link(linked: &[String], other: &str, relate: bool) -> Link {
    Link {
        other: other.to_owned(),
        linked: relate,
        stale: linked.iter().filter(|id| *id != other).cloned().collect(),
    }
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
        assert_eq!(planned_link(&[], "G2", true).apply(&[]), ["G2"]);
        let unlink = planned_link(&[], "G2", false);
        assert!(link_op("op".into(), &none, &unlink, TaskAction::Unrelate).is_none());
        let linked = row(Some(json!({ "related": ["G2", "G3"], "myDay": "x" })));
        let to_g2 = ["G2".to_owned()];
        let again = planned_link(&to_g2, "G2", true);
        assert!(link_op("op".into(), &linked, &again, TaskAction::Relate).is_none());
        let now = ids(linked.extension.as_ref());
        assert_eq!(planned_link(&to_g2, "G2", false).apply(&now), ["G3"]);
    }

    #[test]
    fn a_link_by_an_old_graph_id_is_the_same_link() {
        // G2 moved and is G9 now.
        let old = ["G2".to_owned()];
        let now = ["G2".to_owned(), "G3".to_owned()];
        assert_eq!(planned_link(&old, "G9", true).apply(&now), ["G3", "G9"]);
        assert_eq!(planned_link(&old, "G9", false).apply(&now), ["G3"]);
    }

    #[test]
    fn undo_takes_away_only_that_link_whatever_came_after() {
        // relate A B, then relate A C: undoing the first leaves C.
        let now = ["B".to_owned(), "C".to_owned()];
        assert_eq!(planned_link(&[], "B", true).inverse().apply(&now), ["C"]);
        // Undoing an unrelate links it again.
        let unlink = planned_link(&[], "B", false);
        assert_eq!(unlink.inverse().apply(&["C".to_owned()]), ["C", "B"]);
    }

    #[test]
    fn a_send_starts_from_graph_s_list() {
        let mut document = Map::new();
        document.insert("related".into(), json!(["B"]));
        let payload = json!({ "link": planned_link(&[], "B", true) });
        // Another device linked D meanwhile.
        rebase(&mut document, &payload, Some(&json!({ "related": ["D"] })));
        assert_eq!(document["related"], json!(["D", "B"]));
        let payload = json!({ "link": planned_link(&[], "D", false) });
        rebase(&mut document, &payload, Some(&json!({ "related": ["D"] })));
        assert!(!document.contains_key("related"), "none left");
    }

    #[test]
    fn ids_ignore_what_is_not_a_string() {
        let extension = json!({ "related": ["G2", 3, null, "G4"] });
        assert_eq!(ids(Some(&extension)), ["G2", "G4"]);
        assert!(ids(Some(&json!({ "related": "G2" }))).is_empty());
        assert!(ids(None).is_empty());
    }
}
