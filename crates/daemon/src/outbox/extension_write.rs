//! Sending a write of our extension (docs/blueprint/05-custom-features.md):
//! a list's (folders) or a task's (My Day, assignment). Graph replaces an open
//! extension on PATCH (S2), so the worker GETs the extension as Graph has
//! it now, puts the operation's fields over it and writes the whole
//! document back. An entity without our extension gets it by POST, which
//! acts as an upsert, and a document left with no fields is deleted, since
//! Graph refuses an empty PATCH. All three are safe to resend, so an
//! extension write is never `unknown`.
//!
//! A task's write may carry `expect`: fields that must still hold these
//! values in Graph's copy for the write to be made (the rollover's
//! "still in yesterday's My Day"). If one doesn't, nothing is written and
//! the operation is done, with Graph's copy recorded.
//!
//! A write from another device between the GET and the write (issue 001,
//! D-070): a task's extension write sends `If-Match` with the task's etag
//! from the GET, which Graph honours, so on a 412 it reads, merges and
//! writes again, a few times at most. A list's extension ignores
//! `If-Match` (S2), so there the race stays, one round trip wide; the
//! write reads the list again afterwards and notes any field that isn't
//! as it wrote it, since another device wrote around the same moment.

use ms_todo_graph::GraphError;
use ms_todo_store::{OutboxRow, merge_extension};
use serde_json::{Map, Value, json};

use super::send::{Attempt, Failure, classify, etag, fields_not_holding};
use crate::entities::{EXTENSION_NAME, split_extension};
use crate::handlers::State;

/// Whose extension a write changes, by Graph IDs.
#[derive(Clone, Copy)]
enum Owner<'a> {
    List(&'a str),
    Task { list: &'a str, task: &'a str },
}

impl<'a> Owner<'a> {
    async fn get(self, state: &State) -> Result<ms_todo_store::Entity, GraphError> {
        match self {
            Self::List(list) => {
                state
                    .graph
                    .get_list_with_extension(list, EXTENSION_NAME)
                    .await
            }
            Self::Task { list, task } => {
                state
                    .graph
                    .get_task_with_extension(list, task, EXTENSION_NAME)
                    .await
            }
        }
    }

    /// Its path under `/me/todo`, for the extension writes.
    fn path(self) -> Vec<&'a str> {
        match self {
            Self::List(list) => vec!["lists", list],
            Self::Task { list, task } => vec!["lists", list, "tasks", task],
        }
    }
}

/// How many times a task's extension write reads and writes again after
/// Graph's 412 before it waits for the next round: each 412 means the task
/// changed in the moment between, which doesn't keep happening.
const CONDITIONAL_TRIES: usize = 3;

/// A folder write on the list `list_graph_id`.
pub(super) async fn send(
    state: &State,
    list_graph_id: &str,
    op: &OutboxRow,
) -> Result<Attempt, Failure> {
    let owner = Owner::List(list_graph_id);
    let before = current(state, owner).await?;
    let data = write(state, owner, before, op, None)
        .await?
        .ok_or_else(|| Failure::Temporary(changed_meanwhile()))?;
    // Read again: a field that isn't what was written means another
    // device wrote at about the same moment, and one of the two writes
    // replaced the other's (S2: lists ignore `If-Match`). A failed read
    // doesn't undo a write that was made; the next sync brings the list.
    let raced = match current(state, owner).await {
        Ok(now) => raced_fields(&data, now.as_ref()),
        Err(_) => Vec::new(),
    };
    let note = (!raced.is_empty()).then(|| {
        format!(
            "another device changed ms-todo's data on this list as this was written ({}): \
             check its folder and order",
            raced.join(", ")
        )
    });
    Ok(Attempt::ExtensionWritten {
        extension: cached(data),
        raced: note.map(|note| (raced, note)),
    })
}

/// A My Day or assignment write on the task `task_graph_id`, conditional
/// on the task's etag. Graph's task is read again afterwards, since the
/// write moved its etag.
pub(super) async fn send_task(
    state: &State,
    list_graph_id: &str,
    task_graph_id: &str,
    op: &OutboxRow,
) -> Result<Attempt, Failure> {
    let owner = Owner::Task {
        list: list_graph_id,
        task: task_graph_id,
    };
    for _ in 0..CONDITIONAL_TRIES {
        let fetched = owner.get(state).await.map_err(classify)?;
        let (task, extension) = split_extension(fetched);
        let current = extension.flatten();
        if !holds(current.as_ref(), &op.payload["expect"]) {
            // Moved on since the write was queued (another ms-todo, or a
            // later change here): nothing to do.
            return Ok(Attempt::Skipped {
                task,
                extension: Some(current),
                why: "ms-todo's data on the task changed meanwhile, so it was left alone",
            });
        }
        if let Some(edit) = op.payload.get("after").and_then(Value::as_str)
            && !edit_was_made(state, edit).await?
        {
            return Ok(Attempt::Skipped {
                task,
                extension: Some(current),
                why: "the edit it followed wasn't made, so ms-todo didn't mark it as its own",
            });
        }
        if write(state, owner, current, op, etag(&task))
            .await?
            .is_some()
        {
            let (task, extension) = split_extension(owner.get(state).await.map_err(classify)?);
            return Ok(Attempt::Changed(task, Some(extension.flatten())));
        }
    }
    Err(Failure::Temporary(changed_meanwhile()))
}

fn changed_meanwhile() -> ms_todo_protocol::ErrorPayload {
    crate::handlers::error_payload(
        ms_todo_core::ErrorKind::Conflict,
        "the task kept changing on another device while ms-todo's data on it was written; \
         it's tried again shortly"
            .into(),
    )
}

/// Whether the operation `op_id` was sent, not skipped: `myDayDueSet`
/// and `assigneeStatusSet` are written only after ms-todo's own edit
/// (D-054).
async fn edit_was_made(state: &State, op_id: &str) -> Result<bool, Failure> {
    let edit = state
        .store
        .outbox_op(op_id)
        .await
        .map_err(|error| Failure::Temporary(crate::handlers::store_error(error)))?;
    Ok(edit.is_some_and(|edit| !edit.was_skipped()))
}

async fn current(state: &State, owner: Owner<'_>) -> Result<Option<Value>, Failure> {
    let fetched = owner.get(state).await.map_err(classify)?;
    Ok(split_extension(fetched).1.flatten())
}

/// Put `op`'s fields over `current` and write the result, conditional on
/// `if_match` (the task's etag). Returns what was written, our fields
/// only, or `None` when Graph's 412 says the task changed since the read.
async fn write(
    state: &State,
    owner: Owner<'_>,
    current: Option<Value>,
    op: &OutboxRow,
    if_match: Option<&str>,
) -> Result<Option<Map<String, Value>>, Failure> {
    let mut merged = current
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(fields) = op.body().as_object() {
        merge_extension(&mut merged, fields);
    }
    crate::related::rebase(&mut merged, &op.payload, current.as_ref());
    let data = document(&merged, op.body());
    let path = owner.path();
    let written = if data.is_empty() {
        // Graph refuses a PATCH of an empty document: a write that leaves
        // no field removes the extension instead.
        state
            .graph
            .delete_extension(&path, EXTENSION_NAME, if_match)
            .await
    } else {
        match current {
            Some(_) => match state
                .graph
                .replace_extension(
                    &path,
                    EXTENSION_NAME,
                    &Value::Object(data.clone()),
                    if_match,
                )
                .await
            {
                // Deleted between the GET and the PATCH: make it afresh.
                Err(error) if error.status() == Some(404) => {
                    create(state, &path, &data, if_match).await
                }
                other => other,
            },
            None => create(state, &path, &data, if_match).await,
        }
    };
    match written {
        Ok(()) => Ok(Some(data)),
        Err(error) if if_match.is_some() && error.status() == Some(412) => Ok(None),
        Err(error) => Err(classify(error)),
    }
}

/// The fields of `written` that `now` (the extension read back) doesn't
/// hold as written, and those it has that weren't written, by name.
fn raced_fields(written: &Map<String, Value>, now: Option<&Value>) -> Vec<String> {
    let now = now.and_then(Value::as_object).cloned().unwrap_or_default();
    let ours = |key: &String| !key.contains('@') && key != "extensionName" && key != "id";
    let mut raced: Vec<String> = written
        .keys()
        .chain(now.keys())
        .filter(|key| ours(key) && written.get(*key) != now.get(*key))
        .cloned()
        .collect();
    raced.sort();
    raced.dedup();
    raced
}

/// A POST of our extension with `data`, which upserts it (S2).
async fn create(
    state: &State,
    path: &[&str],
    data: &Map<String, Value>,
    if_match: Option<&str>,
) -> Result<(), GraphError> {
    let mut body = data.clone();
    body.insert(
        "@odata.type".into(),
        json!("microsoft.graph.openTypeExtension"),
    );
    body.insert("extensionName".into(), json!(EXTENSION_NAME));
    state
        .graph
        .create_extension(path, &Value::Object(body), if_match)
        .await
}

/// Whether each field of `expected` (an object, or nothing) has that value
/// in `extension`, null meaning absent.
fn holds(extension: Option<&Value>, expected: &Value) -> bool {
    let extension = extension
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    fields_not_holding(expected, &extension).is_empty()
}

/// The document as the cache keeps it, in Graph's shape.
fn cached(data: Map<String, Value>) -> Map<String, Value> {
    if data.is_empty() {
        return data;
    }
    let mut cached = data;
    cached.insert("extensionName".into(), json!(EXTENSION_NAME));
    cached.insert(
        "id".into(),
        json!(format!(
            "microsoft.graph.openTypeExtension.{EXTENSION_NAME}"
        )),
    );
    cached
}

/// Our fields only, as [`crate::entities::extension_document`] writes
/// them; the operation's own fields (`body`) are written anew, so an
/// annotation Graph had for one's old value goes.
fn document(extension: &Map<String, Value>, body: &Value) -> Map<String, Value> {
    crate::entities::extension_document(extension, |key| body.get(key).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_sent_is_our_fields_without_graphs_own_annotations() {
        let extension = json!({
            "extensionName": EXTENSION_NAME,
            "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
            "@odata.type": "#microsoft.graph.openTypeExtension",
            "folder": "Areas",
            "order@odata.type": "#Int64",
            "order": 3
        });
        let extension = extension.as_object().expect("object");
        // The folder written anew: order's type is kept.
        let sent = document(extension, &json!({ "folder": "Areas" }));
        assert_eq!(
            Value::Object(sent),
            json!({ "folder": "Areas", "order": 3, "order@odata.type": "#Int64" })
        );
        // The order written anew: its old type goes.
        let sent = document(extension, &json!({ "order": 3 }));
        assert_eq!(
            Value::Object(sent),
            json!({ "folder": "Areas", "order": 3 })
        );
    }

    #[test]
    fn a_field_read_back_unlike_what_was_written_is_raced() {
        let written = json!({ "folder": "Areas", "order": 3 });
        let written = written.as_object().expect("object");
        let same = json!({ "extensionName": EXTENSION_NAME, "folder": "Areas",
            "order@odata.type": "#Int64", "order": 3 });
        assert!(raced_fields(written, Some(&same)).is_empty());
        let moved = json!({ "folder": "Areas", "order": 5, "folderOrder": 1 });
        assert_eq!(
            raced_fields(written, Some(&moved)),
            ["folderOrder", "order"]
        );
        assert_eq!(raced_fields(written, None), ["folder", "order"]);
    }

    #[test]
    fn an_expectation_holds_only_while_every_field_has_its_value() {
        let extension = json!({ "myDay": "2026-09-24", "opId": "x" });
        assert!(holds(Some(&extension), &Value::Null));
        assert!(holds(Some(&extension), &json!({ "myDay": "2026-09-24" })));
        assert!(!holds(Some(&extension), &json!({ "myDay": "2026-09-25" })));
        assert!(holds(None, &json!({ "myDay": null })));
        assert!(!holds(None, &json!({ "myDay": "2026-09-24" })));
    }
}
