//! Folders (docs/blueprint/05-custom-features.md#folders-list-groups): how
//! lists are grouped and ordered, and what each folder change writes to
//! each list's extension. A folder is only a name its lists carry, so every
//! change here is a set of per-list field changes; nothing else is stored.
//!
//! Order: folders by `folderOrder`, then lists in each folder by `order`,
//! then the lists in no folder by `order`. A missing number sorts after
//! every number, and ties keep the order lists were first seen in. `lists
//! order` and `folders order` number the whole group 1, 2, 3, … and write
//! only the lists whose number changed.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{Anchor, ErrorPayload};
use ms_todo_store::{FOLDER_FIELD, FOLDER_ORDER_FIELD, ListRow, ORDER_FIELD};
use serde_json::{Map, Value, json};

use crate::handlers::error_payload;

/// A folder and its lists, in order.
#[derive(Debug)]
pub(crate) struct Group<'a> {
    pub name: &'a str,
    pub lists: Vec<&'a ListRow>,
}

/// A change to one list's extension fields; a null removes the field.
pub(crate) type Planned<'a> = Vec<(&'a ListRow, Map<String, Value>)>;

/// The folders in order, each with its lists in order.
pub(crate) fn groups(lists: &[ListRow]) -> Vec<Group<'_>> {
    let mut groups: Vec<Group<'_>> = Vec::new();
    for list in lists {
        let Some(name) = list.folder() else {
            continue;
        };
        match groups.iter_mut().find(|group| group.name == name) {
            Some(group) => group.lists.push(list),
            None => groups.push(Group {
                name,
                lists: vec![list],
            }),
        }
    }
    // Stable sorts: ties keep first-seen order.
    groups.sort_by_key(|group| {
        group
            .lists
            .iter()
            .filter_map(|list| list.folder_order())
            .min()
            .unwrap_or(i64::MAX)
    });
    for group in &mut groups {
        sort_lists(&mut group.lists);
    }
    groups
}

/// Every list in display order: each folder's, then those in none.
pub(crate) fn sorted(lists: &[ListRow]) -> Vec<&ListRow> {
    let mut sorted: Vec<&ListRow> = groups(lists)
        .into_iter()
        .flat_map(|group| group.lists)
        .collect();
    sorted.extend(loose(lists));
    sorted
}

/// The lists in no folder, in order.
fn loose(lists: &[ListRow]) -> Vec<&ListRow> {
    let mut loose: Vec<&ListRow> = lists
        .iter()
        .filter(|list| list.folder().is_none())
        .collect();
    sort_lists(&mut loose);
    loose
}

fn sort_lists(lists: &mut [&ListRow]) {
    lists.sort_by_key(|list| list.order().unwrap_or(i64::MAX));
}

/// The folder `wanted` names, as its lists spell it: names match ignoring
/// case and surrounding spaces, as To Do's do.
pub(crate) fn find<'g, 'a>(groups: &'g [Group<'a>], wanted: &str) -> Option<&'g Group<'a>> {
    let wanted = wanted.trim();
    // An exact match first, in case two folders differ only by case.
    groups
        .iter()
        .find(|group| group.name == wanted)
        .or_else(|| {
            let wanted = wanted.to_lowercase();
            groups
                .iter()
                .find(|group| group.name.to_lowercase() == wanted)
        })
}

/// Put `targets` in the folder `folder` (an existing one's spelling wins),
/// or in none. A list moving in goes after the folder's numbered lists; one
/// already there keeps its place.
pub(crate) fn move_lists<'a>(
    lists: &'a [ListRow],
    targets: &[&'a ListRow],
    folder: Option<&str>,
) -> Result<Planned<'a>, ErrorPayload> {
    let groups = groups(lists);
    let Some(folder) = folder else {
        return Ok(targets.iter().map(|&list| (list, no_folder())).collect());
    };
    let name = folder_name(folder)?;
    let existing = find(&groups, &name);
    let name = existing.map_or(name, |group| group.name.to_owned());
    let folder_order = existing
        .and_then(|group| group.lists.iter().find_map(|list| list.folder_order()))
        .map_or(Value::Null, |order| json!(order));
    Ok(targets
        .iter()
        .map(|&list| {
            let mut fields = Map::new();
            fields.insert(FOLDER_FIELD.into(), json!(name));
            fields.insert(FOLDER_ORDER_FIELD.into(), folder_order.clone());
            if list.folder() != Some(name.as_str()) {
                fields.insert(ORDER_FIELD.into(), Value::Null);
            }
            (list, fields)
        })
        .collect())
}

/// Put `list` just before or after `anchor`, a list in the same folder.
pub(crate) fn order_list<'a>(
    lists: &'a [ListRow],
    list: &'a ListRow,
    anchor: &'a ListRow,
    before: bool,
) -> Result<Planned<'a>, ErrorPayload> {
    if list.local_id == anchor.local_id {
        return Err(invalid("a list can't go next to itself"));
    }
    if list.folder() != anchor.folder() {
        return Err(invalid(&format!(
            "{:?} and {:?} are in different folders; `lists move` it into {} first",
            list.display_name,
            anchor.display_name,
            anchor
                .folder()
                .map_or_else(|| "no folder".to_owned(), |name| format!("{name:?}")),
        )));
    }
    let groups = groups(lists);
    let siblings = match list.folder() {
        Some(name) => find(&groups, name).map(|group| group.lists.clone()),
        None => Some(loose(lists)),
    }
    .unwrap_or_default();
    let placed = place(
        siblings,
        |row| row.local_id == list.local_id,
        |row| row.local_id == anchor.local_id,
        before,
    );
    Ok(placed
        .into_iter()
        .zip(1..)
        .filter(|(row, number)| row.order() != Some(*number))
        .map(|(row, number)| (row, fields([(ORDER_FIELD, json!(number))])))
        .collect())
}

/// Rename a folder: every list in it gets the new name.
pub(crate) fn rename<'a>(
    lists: &'a [ListRow],
    folder: &str,
    new_name: &str,
) -> Result<Planned<'a>, ErrorPayload> {
    let groups = groups(lists);
    let group = find_or_fail(&groups, folder)?;
    let new_name = folder_name(new_name)?;
    if let Some(other) = find(&groups, &new_name).filter(|other| other.name != group.name) {
        return Err(invalid(&format!(
            "a folder named {:?} exists already; move the lists with `lists move … --folder {:?}` \
             to merge them",
            other.name, other.name
        )));
    }
    Ok(group
        .lists
        .iter()
        .map(|&list| (list, fields([(FOLDER_FIELD, json!(new_name))])))
        .collect())
}

/// Delete a folder: its lists stay, in no folder.
pub(crate) fn delete<'a>(lists: &'a [ListRow], folder: &str) -> Result<Planned<'a>, ErrorPayload> {
    let groups = groups(lists);
    let group = find_or_fail(&groups, folder)?;
    Ok(group
        .lists
        .iter()
        .map(|&list| (list, no_folder()))
        .collect())
}

/// Put the folder `folder` just before or after `anchor`, another folder.
pub(crate) fn order_folder<'a>(
    lists: &'a [ListRow],
    folder: &str,
    anchor: &str,
    before: bool,
) -> Result<Planned<'a>, ErrorPayload> {
    let groups = groups(lists);
    let moving = find_or_fail(&groups, folder)?.name;
    let anchor = find_or_fail(&groups, anchor)?.name;
    if moving == anchor {
        return Err(invalid("a folder can't go next to itself"));
    }
    let placed = place(
        groups.iter().collect(),
        |group| group.name == moving,
        |group| group.name == anchor,
        before,
    );
    Ok(placed
        .into_iter()
        .zip(1..)
        .flat_map(|(group, number)| {
            group
                .lists
                .iter()
                .filter(move |list| list.folder_order() != Some(number))
                .map(move |&list| (list, fields([(FOLDER_ORDER_FIELD, json!(number))])))
        })
        .collect())
}

/// Parse a change's `Anchor`: its name and whether it's `before`.
pub(crate) fn anchor(anchor: &Anchor) -> (&str, bool) {
    match anchor {
        Anchor::Before(name) => (name, true),
        Anchor::After(name) => (name, false),
    }
}

/// `items` with the one `is_moving` picks taken out and put just before or
/// after the one `is_anchor` picks.
fn place<T: Copy>(
    items: Vec<T>,
    is_moving: impl Fn(&T) -> bool,
    is_anchor: impl Fn(&T) -> bool,
    before: bool,
) -> Vec<T> {
    let moving: Vec<T> = items
        .iter()
        .copied()
        .filter(|item| is_moving(item))
        .collect();
    let mut rest: Vec<T> = items.into_iter().filter(|item| !is_moving(item)).collect();
    let at = rest
        .iter()
        .position(is_anchor)
        .map_or(rest.len(), |at| if before { at } else { at + 1 });
    rest.splice(at..at, moving);
    rest
}

/// The lists in the folder `folder`, in order.
pub(crate) fn lists_in<'a>(
    lists: &'a [ListRow],
    folder: &str,
) -> Result<Vec<&'a ListRow>, ErrorPayload> {
    let groups = groups(lists);
    Ok(find_or_fail(&groups, folder)?.lists.clone())
}

fn find_or_fail<'g, 'a>(
    groups: &'g [Group<'a>],
    wanted: &str,
) -> Result<&'g Group<'a>, ErrorPayload> {
    find(groups, wanted).ok_or_else(|| {
        error_payload(
            ErrorKind::NotFound,
            format!("no folder is named {wanted:?}; see `ms-todo folders list`"),
        )
    })
}

/// A folder name as typed: trimmed, and not empty.
fn folder_name(name: &str) -> Result<String, ErrorPayload> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid(
            "a folder needs a name; `--no-folder` takes lists out of theirs",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("a folder name can't hold control characters"));
    }
    Ok(name.to_owned())
}

fn no_folder() -> Map<String, Value> {
    fields([
        (FOLDER_FIELD, Value::Null),
        (FOLDER_ORDER_FIELD, Value::Null),
        (ORDER_FIELD, Value::Null),
    ])
}

fn fields<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn invalid(message: &str) -> ErrorPayload {
    error_payload(ErrorKind::InvalidInput, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, extension: Value) -> ListRow {
        ListRow {
            local_id: id.into(),
            graph_id: Some(id.to_uppercase()),
            display_name: id.into(),
            wellknown_list_name: None,
            raw: Map::new(),
            extension: (!extension.is_null()).then_some(extension),
            sync_state: "synced".into(),
        }
    }

    /// Two folders, Areas numbered after Projects, and two loose lists.
    fn lists() -> Vec<ListRow> {
        vec![
            list("tasks", Value::Null),
            list(
                "finances",
                json!({ "folder": "Areas", "folderOrder": 2, "order": 2 }),
            ),
            list(
                "health",
                json!({ "folder": "Areas", "folderOrder": 2, "order": 1 }),
            ),
            list("launch", json!({ "folder": "Projects", "folderOrder": 1 })),
            list("groceries", Value::Null),
        ]
    }

    fn ids(rows: &[&ListRow]) -> Vec<String> {
        rows.iter().map(|row| row.local_id.clone()).collect()
    }

    fn by_id<'a>(lists: &'a [ListRow], id: &str) -> &'a ListRow {
        lists.iter().find(|list| list.local_id == id).expect("list")
    }

    fn changes(planned: &Planned<'_>) -> Vec<(String, Value)> {
        planned
            .iter()
            .map(|(list, fields)| (list.local_id.clone(), Value::Object(fields.clone())))
            .collect()
    }

    #[test]
    fn folders_come_first_in_their_order_then_lists_in_none() {
        let lists = lists();
        assert_eq!(
            ids(&sorted(&lists)),
            ["launch", "health", "finances", "tasks", "groceries"]
        );
        let names: Vec<&str> = groups(&lists).iter().map(|group| group.name).collect();
        assert_eq!(names, ["Projects", "Areas"]);
    }

    #[test]
    fn moving_into_a_folder_takes_its_spelling_and_number() {
        let lists = lists();
        let planned =
            move_lists(&lists, &[by_id(&lists, "groceries")], Some(" areas ")).expect("plan");
        assert_eq!(
            changes(&planned),
            [(
                "groceries".to_owned(),
                json!({ "folder": "Areas", "folderOrder": 2, "order": null })
            )]
        );
        let planned = move_lists(&lists, &[by_id(&lists, "tasks")], Some("Someday")).expect("new");
        assert_eq!(
            changes(&planned)[0].1,
            json!({ "folder": "Someday", "folderOrder": null, "order": null })
        );
        let planned = move_lists(&lists, &[by_id(&lists, "health")], None).expect("out");
        assert_eq!(
            changes(&planned)[0].1,
            json!({ "folder": null, "folderOrder": null, "order": null })
        );
        let blank = move_lists(&lists, &[by_id(&lists, "tasks")], Some("  ")).expect_err("blank");
        assert_eq!(blank.kind, "invalid_input");
    }

    #[test]
    fn ordering_a_list_numbers_its_folder_and_writes_only_what_moved() {
        let lists = lists();
        let finances = by_id(&lists, "finances");
        let health = by_id(&lists, "health");
        let planned = order_list(&lists, finances, health, true).expect("plan");
        assert_eq!(
            changes(&planned),
            [
                ("finances".to_owned(), json!({ "order": 1 })),
                ("health".to_owned(), json!({ "order": 2 }))
            ]
        );
        let other =
            order_list(&lists, finances, by_id(&lists, "launch"), false).expect_err("folder");
        assert_eq!(other.kind, "invalid_input");
    }

    #[test]
    fn renaming_moves_every_list_and_never_merges_by_accident() {
        let lists = lists();
        let planned = rename(&lists, "areas", "Responsibilities").expect("plan");
        assert_eq!(planned.len(), 2);
        assert!(
            planned
                .iter()
                .all(|(_, fields)| fields["folder"] == "Responsibilities")
        );
        assert_eq!(
            rename(&lists, "Areas", "projects").expect_err("taken").kind,
            "invalid_input"
        );
        assert_eq!(
            rename(&lists, "Nope", "X").expect_err("missing").kind,
            "not_found"
        );
        // Only the case changes: the same folder.
        assert_eq!(rename(&lists, "Areas", "AREAS").expect("recase").len(), 2);
    }

    #[test]
    fn deleting_a_folder_only_clears_its_lists_fields() {
        let lists = lists();
        let planned = delete(&lists, "Areas").expect("plan");
        assert_eq!(
            changes(&planned),
            [
                (
                    "health".to_owned(),
                    json!({ "folder": null, "folderOrder": null, "order": null })
                ),
                (
                    "finances".to_owned(),
                    json!({ "folder": null, "folderOrder": null, "order": null })
                )
            ]
        );
    }

    #[test]
    fn ordering_a_folder_numbers_every_folder() {
        let lists = lists();
        let planned = order_folder(&lists, "Areas", "Projects", true).expect("plan");
        assert_eq!(
            changes(&planned),
            [
                ("health".to_owned(), json!({ "folderOrder": 1 })),
                ("finances".to_owned(), json!({ "folderOrder": 1 })),
                ("launch".to_owned(), json!({ "folderOrder": 2 }))
            ]
        );
        assert!(
            order_folder(&lists, "Areas", "Projects", false)
                .expect("same")
                .is_empty()
        );
    }
}
