//! `--list <name|id>`: an exact ID, or an exact name that exactly one list
//! has. An ambiguous name is an error listing the candidates; it never picks
//! the first match (docs/blueprint/07-cli.md#global-flags).

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Entity, ErrorPayload, ListRef};

use crate::handlers::error_payload as error;

/// Graph's `wellknownListName` for the built-in "Tasks" list.
const DEFAULT_LIST: &str = "defaultList";

pub(crate) fn resolve_list(
    lists: &[Entity],
    wanted: Option<&str>,
) -> Result<ListRef, ErrorPayload> {
    let Some(wanted) = wanted else {
        return lists
            .iter()
            .find(|list| field(list, "wellknownListName") == Some(DEFAULT_LIST))
            .and_then(list_ref)
            .ok_or_else(|| {
                error(
                    ErrorKind::NotFound,
                    "Graph returned no default \"Tasks\" list".into(),
                )
            });
    };
    let refs: Vec<ListRef> = lists.iter().filter_map(list_ref).collect();
    if let Some(by_id) = refs.iter().find(|list| list.id == wanted) {
        return Ok(by_id.clone());
    }
    let mut named: Vec<ListRef> = refs
        .into_iter()
        .filter(|list| list.name == wanted)
        .collect();
    match named.len() {
        1 => Ok(named.remove(0)),
        0 => Err(error(
            ErrorKind::NotFound,
            format!("no list is named {wanted:?} or has that ID; see `ms-todo lists list`"),
        )),
        count => Err(ErrorPayload {
            candidates: named,
            ..error(
                ErrorKind::InvalidInput,
                format!("{count} lists are named {wanted:?}; pass one of their IDs with --list"),
            )
        }),
    }
}

fn list_ref(list: &Entity) -> Option<ListRef> {
    Some(ListRef {
        id: field(list, "id")?.to_owned(),
        name: field(list, "displayName").unwrap_or_default().to_owned(),
    })
}

fn field<'a>(entity: &'a Entity, name: &str) -> Option<&'a str> {
    entity.get(name).and_then(|value| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lists() -> Vec<Entity> {
        [
            json!({ "id": "L-tasks", "displayName": "Tasks", "wellknownListName": "defaultList" }),
            json!({ "id": "L-a", "displayName": "Groceries", "wellknownListName": "none" }),
            json!({ "id": "L-b", "displayName": "Groceries", "wellknownListName": "none" }),
            json!({ "id": "L-work", "displayName": "Work", "wellknownListName": "none" }),
        ]
        .into_iter()
        .filter_map(|value| value.as_object().cloned())
        .collect()
    }

    #[test]
    fn no_list_means_the_default_list() {
        assert_eq!(resolve_list(&lists(), None).expect("default").id, "L-tasks");
    }

    #[test]
    fn a_unique_exact_name_resolves() {
        assert_eq!(
            resolve_list(&lists(), Some("Work")).expect("work").id,
            "L-work"
        );
    }

    #[test]
    fn an_id_resolves_even_when_it_is_not_a_name() {
        assert_eq!(
            resolve_list(&lists(), Some("L-b")).expect("by id").id,
            "L-b"
        );
    }

    #[test]
    fn an_ambiguous_name_is_invalid_input_with_every_candidate() {
        let error = resolve_list(&lists(), Some("Groceries")).expect_err("ambiguous");
        assert_eq!(error.kind, "invalid_input");
        let ids: Vec<_> = error
            .candidates
            .iter()
            .map(|list| list.id.as_str())
            .collect();
        assert_eq!(ids, ["L-a", "L-b"]);
    }

    #[test]
    fn names_match_exactly() {
        let error = resolve_list(&lists(), Some("work")).expect_err("case differs");
        assert_eq!(error.kind, "not_found");
    }
}
