//! `--list <name|id>`: a local ID, a Graph ID, or an exact name that
//! exactly one list has, looked up in the cache. An ambiguous name is an
//! error listing the candidates; it never picks the first match
//! (docs/blueprint/07-cli.md#global-flags).

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Candidate, ErrorPayload};
use ms_todo_store::ListRow;

use crate::handlers::error_payload as error;

/// Graph's `wellknownListName` for the built-in "Tasks" list.
const DEFAULT_LIST: &str = "defaultList";

/// A cached list that Graph knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListRef {
    pub local_id: String,
    pub graph_id: String,
    pub name: String,
}

impl ListRef {
    /// `None` for a list Graph doesn't know yet.
    pub fn of(row: &ListRow) -> Option<Self> {
        Some(Self {
            local_id: row.local_id.clone(),
            graph_id: row.graph_id.clone()?,
            name: row.display_name.clone(),
        })
    }

    pub fn candidate(&self) -> Candidate {
        Candidate {
            id: self.local_id.clone(),
            name: self.name.clone(),
            ..Candidate::default()
        }
    }
}

pub(crate) fn resolve_list(
    lists: &[ListRow],
    wanted: Option<&str>,
) -> Result<ListRef, ErrorPayload> {
    let Some(wanted) = wanted else {
        return lists
            .iter()
            .find(|list| list.wellknown_list_name.as_deref() == Some(DEFAULT_LIST))
            .and_then(ListRef::of)
            .ok_or_else(|| {
                error(
                    ErrorKind::NotFound,
                    "the cache has no default \"Tasks\" list; run `ms-todo sync --wait`".into(),
                )
            });
    };
    let refs: Vec<ListRef> = lists.iter().filter_map(ListRef::of).collect();
    if let Some(by_id) = refs
        .iter()
        .find(|list| list.local_id == wanted || list.graph_id == wanted)
    {
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
            candidates: named.iter().map(ListRef::candidate).collect(),
            ..error(
                ErrorKind::InvalidInput,
                format!("{count} lists are named {wanted:?}; pass one of their IDs with --list"),
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn list(local: &str, graph: &str, name: &str, wellknown: &str) -> ListRow {
        ListRow {
            local_id: local.into(),
            graph_id: Some(graph.into()),
            display_name: name.into(),
            wellknown_list_name: Some(wellknown.into()),
            raw: Map::new(),
            extension: None,
            sync_state: "synced".into(),
        }
    }

    fn lists() -> Vec<ListRow> {
        vec![
            list("l-tasks", "L-tasks", "Tasks", "defaultList"),
            list("l-a", "L-a", "Groceries", "none"),
            list("l-b", "L-b", "Groceries", "none"),
            list("l-work", "L-work", "Work", "none"),
        ]
    }

    #[test]
    fn no_list_means_the_default_list() {
        assert_eq!(
            resolve_list(&lists(), None).expect("default").local_id,
            "l-tasks"
        );
    }

    #[test]
    fn a_unique_exact_name_resolves() {
        assert_eq!(
            resolve_list(&lists(), Some("Work")).expect("work").graph_id,
            "L-work"
        );
    }

    #[test]
    fn a_local_or_graph_id_resolves_even_when_it_is_not_a_name() {
        assert_eq!(
            resolve_list(&lists(), Some("l-b")).expect("local").graph_id,
            "L-b"
        );
        assert_eq!(
            resolve_list(&lists(), Some("L-b")).expect("graph").local_id,
            "l-b"
        );
    }

    #[test]
    fn an_ambiguous_name_is_invalid_input_with_every_candidate_by_local_id() {
        let error = resolve_list(&lists(), Some("Groceries")).expect_err("ambiguous");
        assert_eq!(error.kind, "invalid_input");
        let ids: Vec<_> = error
            .candidates
            .iter()
            .map(|list| list.id.as_str())
            .collect();
        assert_eq!(ids, ["l-a", "l-b"]);
    }

    #[test]
    fn names_match_exactly() {
        let error = resolve_list(&lists(), Some("work")).expect_err("case differs");
        assert_eq!(error.kind, "not_found");
    }
}
