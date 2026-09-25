//! Open extensions by name, on a list or a task. Graph replaces a
//! document on PATCH (S2) and can't list a task's (S2); ms-todo's own
//! extension is read-only here.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{
    ErrorPayload, ExtensionChange, ExtensionOwner, OwnerKind, ResponseData, TaskAction,
};
use ms_todo_store::{Entity, LISTS_SCOPE, OutboxRow};
use serde_json::{Map, Value, json};

use super::{applied, clean, invalid, plan, record, unknown_change};
use crate::entities::EXTENSION_NAME;
use crate::freshness::ensure_ready;
use crate::handlers::{State, error_payload, graph_error, store_error};
use crate::list_resolution::resolve_list;
use crate::task_resolution::resolve_tasks;
use crate::undo::conflict;

pub(crate) async fn list_extensions(
    state: &State,
    owner: &ExtensionOwner,
) -> Result<ResponseData, ErrorPayload> {
    // Graph lists neither a task's (S2) nor a list's (404, seen live on
    // 2026-09-25): ms-todo's own, as cached, is all that can be listed.
    let found = owner_of(state, owner).await?;
    Ok(ResponseData::Extensions {
        items: found.ours.into_iter().collect(),
    })
}

pub(crate) async fn get_extension(
    state: &State,
    owner: &ExtensionOwner,
    name: &str,
) -> Result<ResponseData, ErrorPayload> {
    let found = owner_of(state, owner).await?;
    let extension = read_extension(state, &found.path(), name)
        .await?
        .ok_or_else(|| {
            error_payload(
                ErrorKind::NotFound,
                format!("{} has no extension {name:?}", found.label),
            )
        })?;
    Ok(ResponseData::Extensions {
        items: vec![extension],
    })
}

pub(crate) async fn change_extension(
    state: &State,
    owner: &ExtensionOwner,
    change: ExtensionChange,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let found = owner_of(state, owner).await?;
    let path = found.path();
    let (name, data) = match &change {
        ExtensionChange::Set { name, data } => (name.trim(), Some(check_data(data)?)),
        ExtensionChange::Delete { name } => (name.trim(), None),
        ExtensionChange::Unknown => return Err(unknown_change()),
    };
    if name.is_empty() {
        return Err(invalid("an extension needs a name".into()));
    }
    if name.eq_ignore_ascii_case(EXTENSION_NAME) {
        return Err(invalid(format!(
            "{EXTENSION_NAME} is ms-todo's own: it holds My Day, folders and assignees, which \
             `myday`, `lists move` and `tasks edit --assignee` change"
        )));
    }
    let current = read_extension(state, &path, name).await?;
    let action = match (&data, &current) {
        (Some(_), _) => TaskAction::ExtensionSet,
        (None, Some(_)) => TaskAction::ExtensionDelete,
        (None, None) => {
            return Err(error_payload(
                ErrorKind::NotFound,
                format!("{} has no extension {name:?}", found.label),
            ));
        }
    };
    let target = current
        .clone()
        .map_or_else(|| json!({ "extensionName": name }), Value::Object);
    if dry_run {
        let body = data.clone().map_or(Value::Null, Value::Object);
        return Ok(plan(action, target, body));
    }
    let before = current.as_ref().map(data_of).unwrap_or_default();
    write_extension(state, &path, name, data.as_ref(), current.is_some()).await?;
    let after = match &data {
        Some(data) => document(name, data),
        None => current.clone().unwrap_or_default(),
    };
    let payload = json!({
        "kind": "extension",
        "path": path,
        "name": name,
        "after": data.clone().map_or(Value::Null, Value::Object),
    });
    let entity = format!("{}/{name}", path.join("/"));
    record(state, &op_id, None, &entity, action, &payload, &before).await;
    Ok(applied(op_id, action, after, None))
}

pub(super) async fn undo(
    state: &State,
    target: &str,
    op: &OutboxRow,
    before: Entity,
    op_id: &str,
) -> Result<ResponseData, ErrorPayload> {
    let path: Vec<String> = op.payload["path"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|segment| segment.as_str().map(str::to_owned))
        .collect();
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    let name = op.payload["name"].as_str().unwrap_or_default().to_owned();
    let current = read_extension(state, &path, &name).await?;
    let set = op.payload["after"].as_object();
    let holds = match (set, &current) {
        (Some(set), Some(current)) => data_of(current) == *set,
        (None, None) => true,
        _ => false,
    };
    if !holds {
        return Err(conflict(format!(
            "the extension {name:?} changed since; undo would overwrite that"
        )));
    }
    let restore = (!before.is_empty()).then_some(&before);
    write_extension(state, &path, &name, restore, current.is_some()).await?;
    let (action, result) = match restore {
        Some(data) => (TaskAction::ExtensionSet, document(&name, data)),
        None => (
            TaskAction::ExtensionDelete,
            current.clone().unwrap_or_default(),
        ),
    };
    let payload = json!({
        "kind": "extension",
        "path": path,
        "name": name,
        "after": restore.cloned().map_or(Value::Null, Value::Object),
    });
    let rollback = current.as_ref().map(data_of).unwrap_or_default();
    let entity = format!("{}/{name}", path.join("/"));
    record(
        state,
        op_id,
        Some(target),
        &entity,
        action,
        &payload,
        &rollback,
    )
    .await;
    Ok(applied(op_id.to_owned(), action, result, Some(target)))
}

/// A list or task an extension command names, and where it is in Graph.
struct Owner {
    list: String,
    task: Option<String>,
    /// For people: `the list "Home"`.
    label: String,
    /// ms-todo's own extension on a task, as cached.
    ours: Option<Entity>,
}

impl Owner {
    fn path(&self) -> Vec<&str> {
        let mut path = vec!["lists", self.list.as_str()];
        if let Some(task) = &self.task {
            path.extend(["tasks", task.as_str()]);
        }
        path
    }
}

async fn owner_of(state: &State, owner: &ExtensionOwner) -> Result<Owner, ErrorPayload> {
    let not_yet = |what: &str| {
        error_payload(
            ErrorKind::InvalidInput,
            format!("{what} isn't in Microsoft To Do yet; try again once it has synced"),
        )
    };
    match owner.kind {
        OwnerKind::List => {
            ensure_ready(state, LISTS_SCOPE).await?;
            let lists = state.store.lists().await.map_err(store_error)?;
            let list = resolve_list(&lists, Some(&owner.id))?;
            let label = format!("the list {:?}", list.name);
            let ours = lists
                .iter()
                .find(|row| row.local_id == list.local_id)
                .and_then(|row| ours(row.extension.as_ref()));
            let list = list.graph_id.ok_or_else(|| not_yet(&label))?;
            Ok(Owner {
                list,
                task: None,
                label,
                ours,
            })
        }
        OwnerKind::Task => {
            let targets = resolve_tasks(
                state,
                std::slice::from_ref(&owner.id),
                owner.list.as_deref(),
            )
            .await?;
            let Some(target) = targets.into_iter().next() else {
                return Err(error_payload(ErrorKind::NotFound, "no such task".into()));
            };
            let label = format!("the task {:?}", target.row.title);
            let list = target
                .list
                .graph_id
                .clone()
                .ok_or_else(|| not_yet(&label))?;
            let task = target.row.graph_id.clone().ok_or_else(|| not_yet(&label))?;
            let ours = ours(target.row.extension.as_ref());
            Ok(Owner {
                list,
                task: Some(task),
                label,
                ours,
            })
        }
    }
}

/// ms-todo's own extension as cached, named as Graph names it.
fn ours(cached: Option<&Value>) -> Option<Entity> {
    let mut extension = cached?.as_object()?.clone();
    extension
        .entry("extensionName")
        .or_insert_with(|| json!(EXTENSION_NAME));
    Some(extension)
}

/// The extension `name` on `path`, or none.
async fn read_extension(
    state: &State,
    path: &[&str],
    name: &str,
) -> Result<Option<Entity>, ErrorPayload> {
    match state.graph.get_extension(path, name).await {
        Ok(extension) => Ok(Some(clean(extension))),
        Err(error) if error.status() == Some(404) => Ok(None),
        Err(error) => Err(graph_error(error)),
    }
}

/// Make the extension `name` hold `data`, or with none, delete it. Graph
/// replaces a document on PATCH (S2); one that doesn't exist is POSTed.
async fn write_extension(
    state: &State,
    path: &[&str],
    name: &str,
    data: Option<&Map<String, Value>>,
    exists: bool,
) -> Result<(), ErrorPayload> {
    let written = match (data, exists) {
        (Some(data), true) => {
            state
                .graph
                .replace_extension(path, name, &Value::Object(data.clone()))
                .await
        }
        (Some(data), false) => {
            let mut body = data.clone();
            body.insert(
                "@odata.type".into(),
                json!("microsoft.graph.openTypeExtension"),
            );
            body.insert("extensionName".into(), json!(name));
            state
                .graph
                .create_extension(path, &Value::Object(body))
                .await
        }
        (None, _) => state.graph.delete_extension(path, name).await,
    };
    written.map_err(graph_error)
}

/// An extension's own fields: not Graph's `id`, `extensionName` or
/// annotations.
fn data_of(extension: &Entity) -> Map<String, Value> {
    extension
        .iter()
        .filter(|(key, _)| !is_reserved(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn is_reserved(key: &str) -> bool {
    matches!(key, "id" | "extensionName") || key.contains('@')
}

/// `--json` as an extension can hold it: at least one field, none of
/// Graph's own.
fn check_data(data: &Map<String, Value>) -> Result<Map<String, Value>, ErrorPayload> {
    if let Some(key) = data.keys().find(|key| is_reserved(key)) {
        return Err(invalid(format!(
            "{key:?} is Graph's, not a field an extension can be given"
        )));
    }
    if data.is_empty() {
        return Err(invalid(
            "an extension needs at least one field; `extensions delete` removes one".into(),
        ));
    }
    Ok(data.clone())
}

/// An extension as it reads back: its name and fields.
fn document(name: &str, data: &Map<String, Value>) -> Entity {
    let mut document = data.clone();
    document.insert("extensionName".into(), json!(name));
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn an_extensions_data_leaves_out_graphs_fields() {
        let read = entity(json!({
            "@odata.type": "#microsoft.graph.openTypeExtension",
            "id": "com.example",
            "extensionName": "com.example",
            "colour": "red",
            "count@odata.type": "#Int32",
            "count": 2
        }));
        assert_eq!(
            Value::Object(data_of(&read)),
            json!({ "colour": "red", "count": 2 })
        );
        assert!(check_data(&entity(json!({ "id": "x" }))).is_err());
        assert!(check_data(&Map::new()).is_err());
        assert!(check_data(&entity(json!({ "colour": "red" }))).is_ok());
    }
}
