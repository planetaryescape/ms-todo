//! Contexts (docs/blueprint/05-custom-features.md#contexts, rung 9d): a
//! named set of lists from config.toml, one of them active, which
//! narrows the everyday reads (`tasks list` without `--list`, search,
//! `next`, `waiting`, My Day's suggestions and the TUI's sidebar and
//! views) to its lists, and gives a task added with no list its
//! `default_list`. An explicit `--list` always wins, and My Day itself is
//! never narrowed: it's what the user chose for today.
//!
//! The daemon holds the active context, so the CLI and the TUI share it;
//! it's kept in the store's settings, so it survives a restart.
//! [`within`] is the one predicate every narrowed read uses.

mod config;

use std::collections::HashSet;
use std::sync::Mutex;

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{
    AppliedContext, ContextChoice, ContextInfo, Contexts, ContextsStatus, ErrorPayload,
    ResponseData,
};
use ms_todo_store::{ListRow, Store, TaskRow};

use crate::handlers::{State, error_payload, store_error};
use crate::list_resolution::ListRef;
pub(crate) use config::{Config, Definition};

/// The settings key holding the active context's name; empty is none.
const ACTIVE: &str = "context.active";

/// The active context's name, held in memory, so a read with no context
/// active costs nothing more than before.
#[derive(Debug, Default)]
pub(crate) struct Active(Mutex<Option<String>>);

impl Active {
    /// As the store last kept it.
    pub async fn load(store: &Store) -> Result<Self, ms_todo_store::StoreError> {
        let name = store.setting(ACTIVE).await?.filter(|name| !name.is_empty());
        Ok(Self(Mutex::new(name)))
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().map(|name| name.clone()).unwrap_or_default()
    }

    fn set(&self, name: Option<String>) {
        if let Ok(mut active) = self.0.lock() {
            *active = name;
        }
    }
}

/// A context resolved against the cache's lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub name: String,
    /// The lists it covers, in the sidebar's order.
    pub lists: Vec<ListRef>,
    /// The same lists' local IDs, to test a task against.
    pub ids: HashSet<String>,
    /// Where a task goes when none is named: `default_list`, when it's
    /// one of the context's lists.
    pub default_list: Option<ListRef>,
    /// What didn't resolve.
    pub problems: Vec<String>,
}

impl Resolved {
    /// Resolve `definition` against `lists`: its folders' lists and its
    /// named lists, each name matched exactly.
    pub fn of(name: &str, definition: &Definition, lists: &[ListRow]) -> Self {
        let mut problems = Vec::new();
        for folder in &definition.folders {
            if !lists
                .iter()
                .any(|list| list.folder() == Some(folder.as_str()))
            {
                problems.push(format!(
                    "context {name:?}: no list is in a folder named {folder:?}"
                ));
            }
        }
        for wanted in &definition.lists {
            if !lists.iter().any(|list| &list.display_name == wanted) {
                problems.push(format!("context {name:?}: no list is named {wanted:?}"));
            }
        }
        if definition.folders.is_empty() && definition.lists.is_empty() {
            problems.push(format!(
                "context {name:?} names no folders or lists, so it shows nothing"
            ));
        }
        let covered: Vec<ListRef> = crate::folders::sorted(lists)
            .into_iter()
            .filter(|list| {
                list.folder()
                    .is_some_and(|folder| definition.folders.iter().any(|f| f == folder))
                    || definition.lists.contains(&list.display_name)
            })
            .filter_map(ListRef::of)
            .collect();
        let default_list = definition.default_list.as_ref().and_then(|wanted| {
            let named: Vec<&ListRef> = covered.iter().filter(|list| &list.name == wanted).collect();
            match named.as_slice() {
                [one] => Some((*one).clone()),
                [] => {
                    problems.push(format!(
                        "context {name:?}: default_list {wanted:?} isn't one of its lists"
                    ));
                    None
                }
                _ => {
                    problems.push(format!(
                        "context {name:?}: default_list {wanted:?} names more than one list"
                    ));
                    None
                }
            }
        });
        Self {
            name: name.to_owned(),
            ids: covered.iter().map(|list| list.local_id.clone()).collect(),
            lists: covered,
            default_list,
            problems,
        }
    }

    /// The list the TUI opens in the context: its default list, else
    /// its first.
    pub fn home(&self) -> Option<&ListRef> {
        self.default_list.as_ref().or(self.lists.first())
    }

    /// As an answer names it.
    pub fn applied(&self) -> AppliedContext {
        AppliedContext {
            name: self.name.clone(),
            lists: u64::try_from(self.lists.len()).unwrap_or(u64::MAX),
            default_list: self.default_list.as_ref().map(|list| list.name.clone()),
        }
    }
}

/// Whether a task in the list `list_local_id` is in `context`: always,
/// with none. The one predicate every narrowed read uses.
pub(crate) fn within(context: Option<&Resolved>, list_local_id: &str) -> bool {
    context.is_none_or(|context| context.ids.contains(list_local_id))
}

/// `rows` in `context`, in their order.
pub(crate) fn keep(context: Option<&Resolved>, rows: Vec<TaskRow>) -> Vec<TaskRow> {
    match context {
        None => rows,
        Some(_) => rows
            .into_iter()
            .filter(|row| within(context, &row.list_local_id))
            .collect(),
    }
}

/// `--context`'s choice, checked once per request: a name config.toml
/// doesn't define is `not_found`, whatever the command.
pub(crate) fn check(state: &State, choice: Option<&ContextChoice>) -> Result<(), ErrorPayload> {
    match choice {
        None | Some(ContextChoice::None) => Ok(()),
        Some(ContextChoice::Named { name }) => {
            let config = Config::load(&state.config_file);
            if config.definitions.contains_key(name) {
                Ok(())
            } else {
                Err(unknown(name, &config))
            }
        }
        Some(ContextChoice::Unknown) => Err(error_payload(
            ErrorKind::Unsupported,
            "this daemon doesn't know that context choice; restart it with \
             `ms-todo daemon stop`"
                .into(),
        )),
    }
}

/// The context a read is narrowed by: none when it names a list
/// (`wanted`), since an explicit list always wins; else `choice` when the
/// request carries one (`--context`), else the active one. An active
/// context that config.toml no longer defines narrows nothing (`doctor`
/// says so); a `--context` naming one it doesn't define is an error.
pub(crate) fn applied(
    state: &State,
    choice: Option<&ContextChoice>,
    wanted: Option<&str>,
    lists: &[ListRow],
) -> Result<Option<Resolved>, ErrorPayload> {
    applied_in(state, choice, wanted, lists, None)
}

/// [`applied`] with config.toml already read, when the caller needs it too.
pub(crate) fn applied_in(
    state: &State,
    choice: Option<&ContextChoice>,
    wanted: Option<&str>,
    lists: &[ListRow],
    config: Option<&Config>,
) -> Result<Option<Resolved>, ErrorPayload> {
    if wanted.is_some() {
        return Ok(None);
    }
    let (name, named) = match choice {
        None => match state.context.get() {
            Some(name) => (name, false),
            None => return Ok(None),
        },
        Some(ContextChoice::None) => return Ok(None),
        Some(ContextChoice::Named { name }) => (name.clone(), true),
        Some(ContextChoice::Unknown) => {
            return Err(error_payload(
                ErrorKind::Unsupported,
                "this daemon doesn't know that context choice; restart it with \
                 `ms-todo daemon stop`"
                    .into(),
            ));
        }
    };
    let loaded;
    let config = match config {
        Some(config) => config,
        None => {
            loaded = Config::load(&state.config_file);
            &loaded
        }
    };
    match config.definitions.get(&name) {
        Some(definition) => Ok(Some(Resolved::of(&name, definition, lists))),
        None if named => Err(unknown(&name, config)),
        None => Ok(None),
    }
}

fn unknown(name: &str, config: &Config) -> ErrorPayload {
    let known = config.names();
    let known = if known.is_empty() {
        "config.toml defines none; add a [contexts.<name>] section".to_owned()
    } else {
        format!("the contexts are {}", known.join(", "))
    };
    error_payload(
        ErrorKind::NotFound,
        format!("no context is named {name:?}: {known}"),
    )
}

/// `Contexts`: every context, resolved, and which is active.
pub(crate) async fn contexts(state: &State) -> Result<ResponseData, ErrorPayload> {
    Ok(ResponseData::Contexts(report(state).await?))
}

async fn report(state: &State) -> Result<Contexts, ErrorPayload> {
    let lists = state.store.lists().await.map_err(store_error)?;
    let config = Config::load(&state.config_file);
    let active = state.context.get();
    let mut problems: Vec<String> = config.problem.iter().cloned().collect();
    problems.extend(config.rejected.iter().cloned());
    if let Some(name) = active.as_deref()
        && !config.definitions.contains_key(name)
    {
        problems.push(format!(
            "the active context {name:?} isn't in config.toml any more, so nothing is \
             narrowed; `ms-todo ctx none` clears it"
        ));
    }
    let items = config
        .definitions
        .iter()
        .map(|(name, definition)| {
            let resolved = Resolved::of(name, definition, &lists);
            ContextInfo {
                name: name.clone(),
                active: active.as_deref() == Some(name.as_str()),
                folders: definition.folders.clone(),
                lists: definition.lists.clone(),
                default_list: definition.default_list.clone(),
                resolved: resolved.lists.iter().map(ListRef::candidate).collect(),
                problems: resolved.problems,
            }
        })
        .collect();
    Ok(Contexts {
        active,
        items,
        problems,
    })
}

/// `SetContext`: make `name` active (none for `None`), keep it, and tell
/// every subscriber to read its view again.
pub(crate) async fn set_context(
    state: &State,
    name: Option<String>,
) -> Result<ResponseData, ErrorPayload> {
    if let Some(name) = &name {
        let config = Config::load(&state.config_file);
        if !config.definitions.contains_key(name) {
            return Err(unknown(name, &config));
        }
    }
    state
        .store
        .set_settings(&[(ACTIVE, name.as_deref().unwrap_or_default())])
        .await
        .map_err(store_error)?;
    state.context.set(name);
    state.events.resync_needed();
    contexts(state).await
}

/// `doctor`'s view: the active context and every warning.
pub(crate) async fn status(state: &State) -> Result<ContextsStatus, ErrorPayload> {
    let report = report(state).await?;
    let defined = u64::try_from(report.items.len()).unwrap_or(u64::MAX);
    let mut problems = report.problems;
    for item in report.items {
        problems.extend(item.problems);
    }
    Ok(ContextsStatus {
        active: report.active,
        defined,
        problems,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;

    fn list(name: &str, folder: Option<&str>) -> ListRow {
        ListRow {
            local_id: format!("id-{name}"),
            graph_id: Some(name.to_uppercase()),
            display_name: name.into(),
            wellknown_list_name: None,
            raw: Map::new(),
            extension: folder.map(|folder| json!({ "folder": folder })),
            sync_state: "synced".into(),
        }
    }

    fn lists() -> Vec<ListRow> {
        vec![
            list("Tasks", None),
            list("Contentful", None),
            list("Money", Some("Areas")),
            list("Health", Some("Areas")),
            list("Garden", Some("Home")),
        ]
    }

    fn definition(folders: &[&str], lists: &[&str], default_list: Option<&str>) -> Definition {
        Definition {
            folders: folders.iter().map(|&name| name.to_owned()).collect(),
            lists: lists.iter().map(|&name| name.to_owned()).collect(),
            default_list: default_list.map(str::to_owned),
        }
    }

    fn names(resolved: &Resolved) -> Vec<&str> {
        resolved
            .lists
            .iter()
            .map(|list| list.name.as_str())
            .collect()
    }

    #[test]
    fn a_context_is_its_folders_lists_and_its_named_lists_in_sidebar_order() {
        let work = definition(&["Areas"], &["Contentful"], Some("Contentful"));
        let resolved = Resolved::of("work", &work, &lists());
        assert_eq!(names(&resolved), ["Money", "Health", "Contentful"]);
        assert!(resolved.problems.is_empty(), "{:?}", resolved.problems);
        assert_eq!(
            resolved
                .default_list
                .as_ref()
                .map(|list| list.name.as_str()),
            Some("Contentful")
        );
        assert_eq!(
            resolved.home().map(|list| list.name.as_str()),
            Some("Contentful")
        );
        assert_eq!(
            resolved.applied(),
            AppliedContext {
                name: "work".into(),
                lists: 3,
                default_list: Some("Contentful".into()),
            }
        );
        assert!(within(Some(&resolved), "id-Money"));
        assert!(!within(Some(&resolved), "id-Garden"));
        assert!(within(None, "id-Garden"), "no context holds every list");
    }

    #[test]
    fn names_match_exactly_and_what_doesnt_resolve_is_a_warning() {
        let loose = definition(&["areas", "Gone"], &["contentful", "Garden"], Some("Tasks"));
        let resolved = Resolved::of("loose", &loose, &lists());
        assert_eq!(names(&resolved), ["Garden"]);
        let problems = resolved.problems.join("\n");
        assert!(
            problems.contains("no list is in a folder named \"areas\""),
            "{problems}"
        );
        assert!(
            problems.contains("no list is in a folder named \"Gone\""),
            "{problems}"
        );
        assert!(
            problems.contains("no list is named \"contentful\""),
            "{problems}"
        );
        assert!(
            problems.contains("default_list \"Tasks\" isn't one of its lists"),
            "{problems}"
        );
        // Without a default list, the first of its lists is home.
        assert_eq!(resolved.default_list, None);
        assert_eq!(
            resolved.home().map(|list| list.name.as_str()),
            Some("Garden")
        );
    }

    #[test]
    fn keep_filters_rows_by_the_same_predicate() {
        let home = Resolved::of("home", &definition(&["Home"], &[], None), &lists());
        let row = |id: &str, list: &str| TaskRow {
            local_id: id.into(),
            graph_id: None,
            list_local_id: list.into(),
            title: id.into(),
            raw: Map::new(),
            extension: None,
            sync_state: "synced".into(),
        };
        let rows = || vec![row("mow", "id-Garden"), row("pay", "id-Money")];
        let kept: Vec<String> = keep(Some(&home), rows())
            .into_iter()
            .map(|row| row.local_id)
            .collect();
        assert_eq!(kept, ["mow"]);
        assert_eq!(keep(None, rows()).len(), 2);
    }
}
