//! The options a suggestion chooses from: each list that's a filing
//! target, described by its folder, its name and a few of its open tasks.
//! List names alone chose poorly in the calibration; with the folder and
//! up to five example titles, 7 of 8 answers over 0.8 were right (D-053).

use std::collections::HashMap;

use serde_json::{Map, Value};

/// Example titles per list.
pub(crate) const EXAMPLES: usize = 5;
/// TypeSafe's limit on a Choice's options.
pub(crate) const MAX_OPTIONS: usize = 255;
/// An example title's length, in characters: enough to say what it is.
const EXAMPLE_CHARS: usize = 80;

/// A list as the criteria need it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ListInfo {
    /// The local ID.
    pub id: String,
    pub name: String,
    pub folder: Option<String>,
    /// Graph's `wellknownListName`: `defaultList` is the inbox itself.
    pub wellknown: Option<String>,
    /// Its open tasks' titles, newest first.
    pub open_titles: Vec<String>,
}

/// The Choice's options, and the list each one means.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Candidates {
    pub criteria: Map<String, Value>,
    /// Option key to the list's local ID and name.
    pub lists: HashMap<String, (String, String)>,
}

impl Candidates {
    pub fn len(&self) -> usize {
        self.criteria.len()
    }
}

/// The options for `lists`, leaving out the inbox and other built-in
/// lists, and every list in a folder named in `exclude_folders` (any case).
pub(crate) fn build(lists: &[ListInfo], exclude_folders: &[String]) -> Candidates {
    let excluded = |folder: &str| {
        exclude_folders
            .iter()
            .any(|name| name.trim().eq_ignore_ascii_case(folder.trim()))
    };
    let mut candidates = Candidates::default();
    for list in lists {
        if candidates.len() == MAX_OPTIONS {
            break;
        }
        let built_in = list.wellknown.as_deref().is_some_and(|name| name != "none");
        if built_in || list.folder.as_deref().is_some_and(excluded) {
            continue;
        }
        let key = unique_key(&candidates.criteria, list.name.trim());
        candidates
            .criteria
            .insert(key.clone(), Value::String(describe(list)));
        candidates
            .lists
            .insert(key, (list.id.clone(), list.name.clone()));
    }
    candidates
}

/// `Home list "Garden". Example tasks: Mow; Weed`.
fn describe(list: &ListInfo) -> String {
    let mut text = match &list.folder {
        Some(folder) => format!("{folder} list \"{}\".", list.name),
        None => format!("List \"{}\".", list.name),
    };
    let examples: Vec<String> = list
        .open_titles
        .iter()
        .map(|title| ms_todo_core::one_line_safe(title).trim().to_owned())
        .filter(|title| !title.is_empty())
        .take(EXAMPLES)
        .map(|title| title.chars().take(EXAMPLE_CHARS).collect())
        .collect();
    if !examples.is_empty() {
        text.push_str(&format!(" Example tasks: {}", examples.join("; ")));
    }
    text
}

/// The list's name, or with a number when another option has it already:
/// options must differ, and two lists can share a name.
fn unique_key(taken: &Map<String, Value>, name: &str) -> String {
    let name = if name.is_empty() { "Untitled" } else { name };
    std::iter::once(name.to_owned())
        .chain((2..).map(|n| format!("{name} ({n})")))
        .find(|key| !taken.contains_key(key))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(id: &str, name: &str, folder: Option<&str>, titles: &[&str]) -> ListInfo {
        ListInfo {
            id: id.into(),
            name: name.into(),
            folder: folder.map(str::to_owned),
            wellknown: Some("none".into()),
            open_titles: titles.iter().map(|title| (*title).to_owned()).collect(),
        }
    }

    #[test]
    fn each_list_is_its_folder_name_and_newest_open_tasks() {
        let lists = [
            list(
                "l1",
                "Garden",
                Some("Home"),
                &["Mow", "Weed", "Prune", "Seed", "Water", "Rake"],
            ),
            list("l2", "Reading", None, &[]),
        ];
        let candidates = build(&lists, &[]);
        assert_eq!(
            candidates.criteria.get("Garden"),
            Some(&Value::String(
                "Home list \"Garden\". Example tasks: Mow; Weed; Prune; Seed; Water".into()
            ))
        );
        assert_eq!(
            candidates.criteria.get("Reading"),
            Some(&Value::String("List \"Reading\".".into()))
        );
        assert_eq!(
            candidates.lists.get("Garden"),
            Some(&("l1".to_owned(), "Garden".to_owned()))
        );
    }

    #[test]
    fn the_inbox_built_in_lists_and_excluded_folders_are_left_out() {
        let mut inbox = list("l0", "Tasks", None, &["Call the bank"]);
        inbox.wellknown = Some("defaultList".into());
        let mut flagged = list("lf", "Flagged email", None, &[]);
        flagged.wellknown = Some("flaggedEmails".into());
        let lists = [
            inbox,
            flagged,
            list("l1", "Old project", Some("archive"), &["Ship v1"]),
            list("l2", "Someday list", Some("Someday"), &[]),
            list("l3", "Garden", Some("Home"), &[]),
            list("l4", "Groceries", None, &[]),
        ];
        let candidates = build(&lists, &[" Archive ".into(), "Someday".into()]);
        let mut keys: Vec<&String> = candidates.criteria.keys().collect();
        keys.sort();
        assert_eq!(keys, ["Garden", "Groceries"]);
    }

    #[test]
    fn lists_sharing_a_name_get_their_own_options() {
        let lists = [
            list("a", "Ideas", Some("Work"), &[]),
            list("b", "Ideas", Some("Home"), &[]),
            list("c", " ", None, &[]),
        ];
        let candidates = build(&lists, &[]);
        assert_eq!(
            candidates.lists.get("Ideas (2)"),
            Some(&("b".to_owned(), "Ideas".to_owned()))
        );
        assert_eq!(
            candidates.lists.get("Untitled").map(|(id, _)| id.as_str()),
            Some("c")
        );
    }

    #[test]
    fn titles_are_made_one_line_and_short() {
        let long = "x".repeat(200);
        let lists = [list(
            "a",
            "Notes",
            None,
            &["line\none\u{1b}[31m", &long, " "],
        )];
        let described = build(&lists, &[])
            .criteria
            .get("Notes")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_default();
        assert!(!described.contains('\n') && !described.contains('\u{1b}'));
        assert!(described.ends_with(&format!("; {}", "x".repeat(EXAMPLE_CHARS))));
    }

    #[test]
    fn at_most_the_providers_limit_of_options() {
        let lists: Vec<ListInfo> = (0..300)
            .map(|n| list(&format!("l{n}"), &format!("List {n}"), None, &[]))
            .collect();
        assert_eq!(build(&lists, &[]).len(), MAX_OPTIONS);
    }
}
