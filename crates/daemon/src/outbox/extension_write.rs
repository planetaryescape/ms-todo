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
//! A write from another device between the GET and the write is lost: the
//! accepted race of docs/issues/001-extension-write-race.md.

use ms_todo_graph::GraphError;
use ms_todo_store::{OutboxRow, merge_extension};
use serde_json::{Map, Value, json};

use super::send::{Attempt, Failure, classify, fields_not_holding};
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

/// A folder write on the list `list_graph_id`.
pub(super) async fn send(
    state: &State,
    list_graph_id: &str,
    op: &OutboxRow,
) -> Result<Attempt, Failure> {
    let current = current(state, Owner::List(list_graph_id)).await?;
    let data = write(state, Owner::List(list_graph_id), current, op).await?;
    Ok(Attempt::ExtensionWritten(cached(data)))
}

/// A My Day or assignment write on the task `task_graph_id`. Graph's task is read again
/// afterwards, since the write moved its etag.
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
    write(state, owner, current, op).await?;
    let (task, extension) = split_extension(owner.get(state).await.map_err(classify)?);
    Ok(Attempt::Changed(task, Some(extension.flatten())))
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

/// Put `op`'s fields over `current` and write the result. Returns what was
/// written: our fields only.
async fn write(
    state: &State,
    owner: Owner<'_>,
    current: Option<Value>,
    op: &OutboxRow,
) -> Result<Map<String, Value>, Failure> {
    let mut merged = current
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(fields) = op.body().as_object() {
        merge_extension(&mut merged, fields);
    }
    let data = document(&merged);
    let path = owner.path();
    if data.is_empty() {
        // Graph refuses a PATCH of an empty document: a write that leaves
        // no field removes the extension instead.
        state
            .graph
            .delete_extension(&path, EXTENSION_NAME)
            .await
            .map_err(classify)?;
        return Ok(data);
    }
    let written = match current {
        Some(_) => match state
            .graph
            .replace_extension(&path, EXTENSION_NAME, &Value::Object(data.clone()))
            .await
        {
            // Deleted between the GET and the PATCH: make it afresh.
            Err(error) if error.status() == Some(404) => create(state, &path, &data).await,
            other => other,
        },
        None => create(state, &path, &data).await,
    };
    written.map_err(classify)?;
    Ok(data)
}

/// A POST of our extension with `data`, which upserts it (S2).
async fn create(state: &State, path: &[&str], data: &Map<String, Value>) -> Result<(), GraphError> {
    let mut body = data.clone();
    body.insert(
        "@odata.type".into(),
        json!("microsoft.graph.openTypeExtension"),
    );
    body.insert("extensionName".into(), json!(EXTENSION_NAME));
    state
        .graph
        .create_extension(path, &Value::Object(body))
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

/// Our fields only: not Graph's `id`, `extensionName` or `@odata`
/// annotations (`order@odata.type`), which it adds itself.
fn document(extension: &Map<String, Value>) -> Map<String, Value> {
    extension
        .iter()
        .filter(|(key, _)| {
            *key != "id"
                && *key != "extensionName"
                && !key.starts_with('@')
                && !key.contains("@odata.")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_sent_is_our_fields_without_graphs_annotations() {
        let extension = json!({
            "extensionName": EXTENSION_NAME,
            "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
            "@odata.type": "#microsoft.graph.openTypeExtension",
            "folder": "Areas",
            "order@odata.type": "#Int64",
            "order": 3
        });
        let sent = document(extension.as_object().expect("object"));
        assert_eq!(
            Value::Object(sent),
            json!({ "folder": "Areas", "order": 3 })
        );
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
