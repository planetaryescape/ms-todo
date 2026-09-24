//! Which tasks a command means. Without `--list`, each name is a Graph task
//! ID: one this daemon has seen, or else one found by asking every list
//! (Graph has no path to a task without its list). With `--list`, a name is
//! an ID or an exact title within that list, and a title several tasks share
//! is an error listing them; it never picks one
//! (docs/blueprint/07-cli.md#global-flags).

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Candidate, Entity, ErrorPayload};

use crate::handlers::{State, error_payload, graph_error};
use crate::known_tasks::Target;
use crate::list_resolution::{field, resolve_list};

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
    let mut targets = match list {
        Some(list) => resolve_in_list(state, names, list).await?,
        None => resolve_by_id(state, names).await?,
    };
    let mut seen = std::collections::HashSet::new();
    targets.retain(|target| seen.insert(target.id().to_owned()));
    Ok(targets)
}

async fn resolve_in_list(
    state: &State,
    names: &[String],
    wanted: &str,
) -> Result<Vec<Target>, ErrorPayload> {
    let lists = state.graph.list_lists().await.map_err(graph_error)?;
    let list = resolve_list(&lists, Some(wanted))?;
    let tasks = state
        .graph
        .list_tasks(&list.id)
        .await
        .map_err(graph_error)?;
    state.known.remember_all(&list.id, &tasks);
    names
        .iter()
        .map(|name| {
            let task = match_in_list(&tasks, name, &list.name)?;
            Ok(Target {
                list_id: list.id.clone(),
                task: task.clone(),
            })
        })
        .collect()
}

fn match_in_list<'a>(
    tasks: &'a [Entity],
    name: &str,
    list_name: &str,
) -> Result<&'a Entity, ErrorPayload> {
    if let Some(by_id) = tasks.iter().find(|task| text(task, "id") == name) {
        return Ok(by_id);
    }
    let mut titled: Vec<&Entity> = tasks
        .iter()
        .filter(|task| text(task, "title") == name)
        .collect();
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
                    id: text(task, "id").to_owned(),
                    name: text(task, "title").to_owned(),
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

async fn resolve_by_id(state: &State, ids: &[String]) -> Result<Vec<Target>, ErrorPayload> {
    let mut lists = None;
    let mut targets = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(known) = state.known.get(id) {
            targets.push(known);
            continue;
        }
        if lists.is_none() {
            lists = Some(state.graph.list_lists().await.map_err(graph_error)?);
        }
        let lists = lists.as_deref().unwrap_or_default();
        targets.push(locate(state, lists, id).await?);
    }
    Ok(targets)
}

/// Ask every list for the task, and stop at the first that has it. The
/// client's concurrency cap (4, S9) bounds how many of these run at once.
async fn locate(state: &State, lists: &[Entity], id: &str) -> Result<Target, ErrorPayload> {
    let list_ids: Vec<&str> = lists.iter().filter_map(|list| field(list, "id")).collect();
    let mut answers: FuturesUnordered<_> = list_ids
        .iter()
        .map(|list_id| async move { (*list_id, state.graph.get_task(list_id, id).await) })
        .collect();
    let mut malformed = 0;
    while let Some((list_id, answer)) = answers.next().await {
        match answer {
            Ok(task) => {
                state.known.remember(list_id, &task);
                return Ok(Target {
                    list_id: list_id.to_owned(),
                    task,
                });
            }
            // Not in this list. A malformed ID is a 400, not a 404 (S10).
            Err(error) if error.status() == Some(404) => {}
            Err(error) if error.status() == Some(400) => malformed += 1,
            Err(error) => return Err(graph_error(error)),
        }
    }
    if malformed > 0 && malformed == list_ids.len() {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            format!(
                "{id:?} isn't a task ID; to pick a task by its exact title, add --list. \
                 IDs come from `ms-todo tasks list`"
            ),
        ));
    }
    Err(error_payload(
        ErrorKind::NotFound,
        format!("no list has a task with ID {id:?}; see `ms-todo tasks list`"),
    ))
}

fn text<'a>(entity: &'a Entity, key: &str) -> &'a str {
    field(entity, key).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tasks() -> Vec<Entity> {
        [
            json!({ "id": "T1", "title": "Buy milk" }),
            json!({ "id": "T2", "title": "Call mum" }),
            json!({ "id": "T3", "title": "Call mum" }),
        ]
        .into_iter()
        .filter_map(|value| value.as_object().cloned())
        .collect()
    }

    #[test]
    fn an_id_or_a_unique_title_matches() {
        let tasks = tasks();
        assert_eq!(
            text(match_in_list(&tasks, "T2", "L").expect("id"), "id"),
            "T2"
        );
        assert_eq!(
            text(match_in_list(&tasks, "Buy milk", "L").expect("title"), "id"),
            "T1"
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
        assert_eq!(ids, ["T2", "T3"]);
    }

    #[test]
    fn titles_match_exactly() {
        let error = match_in_list(&tasks(), "buy milk", "L").expect_err("case");
        assert_eq!(error.kind, "not_found");
    }
}
