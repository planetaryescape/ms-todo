//! Which tasks a command means, looked up in the cache. Without `--list`,
//! each name is a task's local ID or Graph ID. With `--list`, a name may
//! also be an exact title within that list, and a title several tasks
//! share is an error listing them; it never picks one
//! (docs/blueprint/07-cli.md#global-flags).

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Candidate, ErrorPayload};
use ms_todo_store::{LISTS_SCOPE, ListRow, TaskRow, tasks_scope};

use crate::freshness::{all_ready, ensure_ready};
use crate::handlers::{State, error_payload, store_error};
use crate::list_resolution::{ListRef, resolve_list};

/// A task a command changes, as cached, with its list.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub row: TaskRow,
    pub list: ListRef,
}

impl Target {
    /// Graph's ID, for the request.
    pub fn graph_id(&self) -> &str {
        self.row.graph_id.as_deref().unwrap_or_default()
    }

    pub fn local_id(&self) -> &str {
        &self.row.local_id
    }

    pub fn title(&self) -> &str {
        &self.row.title
    }
}

/// Resolve every name before anything is written, so a bad name fails the
/// whole command. A task named twice is changed once.
pub(crate) async fn resolve_tasks(
    state: &State,
    names: &[String],
    list: Option<&str>,
) -> Result<Vec<Target>, ErrorPayload> {
    if names.is_empty() {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            "name at least one task: its ID from `ms-todo tasks list`, or `-` to read IDs from stdin"
                .into(),
        ));
    }
    ensure_ready(state, LISTS_SCOPE).await?;
    let lists = state.store.lists().await.map_err(store_error)?;
    let mut targets = match list {
        Some(list) => resolve_in_list(state, names, &resolve_list(&lists, Some(list))?).await?,
        None => resolve_by_id(state, names, lists).await?,
    };
    let mut seen = std::collections::HashSet::new();
    targets.retain(|target| seen.insert(target.row.local_id.clone()));
    Ok(targets)
}

async fn resolve_in_list(
    state: &State,
    names: &[String],
    list: &ListRef,
) -> Result<Vec<Target>, ErrorPayload> {
    ensure_ready(state, &tasks_scope(&list.graph_id)).await?;
    let tasks = state
        .store
        .tasks_in_list(&list.local_id)
        .await
        .map_err(store_error)?;
    names
        .iter()
        .map(|name| {
            Ok(Target {
                row: match_in_list(&tasks, name, &list.name)?.clone(),
                list: list.clone(),
            })
        })
        .collect()
}

fn match_in_list<'a>(
    tasks: &'a [TaskRow],
    name: &str,
    list_name: &str,
) -> Result<&'a TaskRow, ErrorPayload> {
    if let Some(by_id) = tasks
        .iter()
        .find(|task| task.local_id == name || task.graph_id.as_deref() == Some(name))
    {
        return Ok(by_id);
    }
    let mut titled: Vec<&TaskRow> = tasks.iter().filter(|task| task.title == name).collect();
    match titled.len() {
        1 => Ok(titled.remove(0)),
        0 => Err(error_payload(
            ErrorKind::NotFound,
            format!("no task in {list_name:?} has the ID or exact title {name:?}"),
        )),
        count => Err(ErrorPayload {
            candidates: titled
                .iter()
                .map(|task| Candidate {
                    id: task.local_id.clone(),
                    name: task.title.clone(),
                })
                .collect(),
            ..error_payload(
                ErrorKind::InvalidInput,
                format!(
                    "{count} tasks in {list_name:?} are titled {name:?}; pass one of their IDs"
                ),
            )
        }),
    }
}

async fn resolve_by_id(
    state: &State,
    ids: &[String],
    mut lists: Vec<ListRow>,
) -> Result<Vec<Target>, ErrorPayload> {
    let mut targets = Vec::with_capacity(ids.len());
    let mut settled = false;
    for id in ids {
        let mut found = lookup(state, &lists, id).await?;
        // A task in a list whose first sync hasn't finished may just not
        // be cached yet: wait for that, once.
        if found.is_none() && !settled && !all_ready(state).await? {
            settled = true;
            state.syncer.settle().await;
            lists = state.store.lists().await.map_err(store_error)?;
            found = lookup(state, &lists, id).await?;
        }
        targets.push(found.ok_or_else(|| {
            error_payload(
                ErrorKind::NotFound,
                format!(
                    "no task has the ID {id:?}. A task added elsewhere shows up after the next \
                     sync (`ms-todo sync --wait`); to pick a task by its exact title, add --list"
                ),
            )
        })?);
    }
    Ok(targets)
}

async fn lookup(
    state: &State,
    lists: &[ListRow],
    id: &str,
) -> Result<Option<Target>, ErrorPayload> {
    let Some(row) = state.store.task(id).await.map_err(store_error)? else {
        return Ok(None);
    };
    let list = lists
        .iter()
        .find(|list| list.local_id == row.list_local_id)
        .and_then(ListRef::of);
    Ok(list.map(|list| Target { row, list }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn task(local: &str, graph: &str, title: &str) -> TaskRow {
        TaskRow {
            local_id: local.into(),
            graph_id: Some(graph.into()),
            list_local_id: "l".into(),
            title: title.into(),
            raw: Map::new(),
            extension: None,
        }
    }

    fn tasks() -> Vec<TaskRow> {
        vec![
            task("t1", "T1", "Buy milk"),
            task("t2", "T2", "Call mum"),
            task("t3", "T3", "Call mum"),
        ]
    }

    #[test]
    fn a_local_id_graph_id_or_unique_title_matches() {
        let tasks = tasks();
        assert_eq!(
            match_in_list(&tasks, "t2", "L").expect("local").local_id,
            "t2"
        );
        assert_eq!(
            match_in_list(&tasks, "T2", "L").expect("graph").local_id,
            "t2"
        );
        assert_eq!(
            match_in_list(&tasks, "Buy milk", "L")
                .expect("title")
                .local_id,
            "t1"
        );
    }

    #[test]
    fn a_shared_title_is_invalid_input_with_every_candidate() {
        let error = match_in_list(&tasks(), "Call mum", "L").expect_err("ambiguous");
        assert_eq!(error.kind, "invalid_input");
        let ids: Vec<_> = error
            .candidates
            .iter()
            .map(|task| task.id.as_str())
            .collect();
        assert_eq!(ids, ["t2", "t3"]);
    }

    #[test]
    fn titles_match_exactly() {
        let error = match_in_list(&tasks(), "buy milk", "L").expect_err("case");
        assert_eq!(error.kind, "not_found");
    }
}
