//! Contexts in the CLI (docs/blueprint/05-custom-features.md#contexts,
//! rung 9d): `ctx [show|list|NAME|none]`, which the daemon answers from
//! config.toml and keeps the active one of, and the global `--context`,
//! which reads one command in another context by wrapping its request in
//! `InContext`.

use std::sync::OnceLock;

use ms_todo_core::Paths;
use ms_todo_protocol::{ContextChoice, ContextInfo, Contexts, Request, ResponseData};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::args::{CtxArgs, CtxCommand};
use crate::daemon_client;
use crate::error::CliError;
use crate::output::{OutputFormat, Render, Table, csv_cell, print_ids, print_live_collection};

/// `--context`, set once before a command runs.
static OVERRIDE: OnceLock<Option<ContextChoice>> = OnceLock::new();

/// Record `--context NAME|none`. Called once, first thing.
pub fn configure(flag: Option<&str>) {
    let choice = flag.map(|name| match name.trim() {
        "none" => ContextChoice::None,
        name => ContextChoice::Named {
            name: name.to_owned(),
        },
    });
    let _ = OVERRIDE.set(choice);
}

/// `request`, read in `--context`'s context when it was given. Every
/// request the CLI asks goes through here; the daemon checks the name
/// once and only the reads a context narrows, and `AddTask`, use it.
pub fn scoped(request: Request) -> Request {
    match OVERRIDE.get().cloned().flatten() {
        Some(context) => Request::InContext {
            context,
            request: Box::new(request),
        },
        None => request,
    }
}

/// `ctx list`'s CSV columns: each context's fields, in this order.
const CSV_COLUMNS: &[&str] = &[
    "name",
    "active",
    "folders",
    "lists",
    "default_list",
    "list_count",
    "resolved",
    "problems",
];

pub const CONTEXTS_TABLE: Table = Table {
    headings: &["ACTIVE", "NAME", "LISTS", "DEFAULT LIST", "WARNINGS"],
    row: |context| {
        let active = if context.get("active").and_then(Value::as_bool) == Some(true) {
            "*"
        } else {
            ""
        };
        vec![
            active.to_owned(),
            cell(context, "name"),
            cell(context, "list_count"),
            cell(context, "default_list"),
            cell(context, "problems"),
        ]
    },
    csv_headings: CSV_COLUMNS,
    csv_row: |context| CSV_COLUMNS.iter().map(|key| cell(context, key)).collect(),
    bold_matches: None,
};

fn cell(context: &Map<String, Value>, key: &str) -> String {
    csv_cell(context.get(key).unwrap_or(&Value::Null))
}

pub async fn run(paths: &Paths, args: CtxArgs, format: OutputFormat) -> Result<(), CliError> {
    let request = match (args.command, args.name) {
        (Some(CtxCommand::List), _) => {
            let contexts = ask(paths, Request::Contexts).await?;
            let items: Vec<Map<String, Value>> = contexts.items.iter().map(item).collect();
            print_live_collection(format, &items, &CONTEXTS_TABLE)?;
            warn(format, &contexts.problems);
            return Ok(());
        }
        (Some(CtxCommand::Show) | None, None) => Request::Contexts,
        (_, Some(name)) if name.trim() == "none" => Request::SetContext { name: None },
        (_, Some(name)) => Request::SetContext {
            name: Some(name.trim().to_owned()),
        },
    };
    let shown = Shown::of(ask(paths, request).await?);
    if format == OutputFormat::Ids {
        return print_ids(shown.list_ids.iter().map(String::as_str));
    }
    crate::output::print_success(format, &shown)
}

async fn ask(paths: &Paths, request: Request) -> Result<Contexts, CliError> {
    match daemon_client::ask(paths, request).await? {
        ResponseData::Contexts(contexts) => Ok(contexts),
        _ => Err(crate::unexpected_response()),
    }
}

/// A context as `ctx list` prints it.
fn item(context: &ContextInfo) -> Map<String, Value> {
    let resolved: Vec<&str> = context
        .resolved
        .iter()
        .map(|list| list.name.as_str())
        .collect();
    [
        ("name", json!(context.name)),
        ("active", json!(context.active)),
        ("folders", json!(context.folders)),
        ("lists", json!(context.lists)),
        ("default_list", json!(context.default_list)),
        ("list_count", json!(context.resolved.len())),
        ("resolved", json!(resolved)),
        ("problems", json!(context.problems)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

/// Warnings beyond one context, for people: on stderr, not in JSON.
fn warn(format: OutputFormat, problems: &[String]) {
    for problem in problems {
        crate::terminal::note_unless_json(format, &format!("warning: {problem}"));
    }
}

/// `ctx`, `ctx show`, and `ctx NAME|none` after the change: the active
/// context and the lists it covers now.
#[derive(Default, Serialize)]
pub struct Shown {
    /// The active context's name; null shows everything.
    pub active: Option<String>,
    /// Its lists' names, in the sidebar's order.
    pub lists: Vec<String>,
    /// Their local IDs, in the same order.
    pub list_ids: Vec<String>,
    /// As configured.
    pub folders: Vec<String>,
    pub default_list: Option<String>,
    /// What doesn't resolve, in this context or config.toml.
    pub problems: Vec<String>,
}

impl Shown {
    fn of(contexts: Contexts) -> Self {
        let mut problems = contexts.problems;
        let active = contexts.items.into_iter().find(|context| context.active);
        let Some(active) = active else {
            return Self {
                active: contexts.active,
                problems,
                ..Self::default()
            };
        };
        problems.extend(active.problems);
        Self {
            active: Some(active.name),
            lists: active
                .resolved
                .iter()
                .map(|list| list.name.clone())
                .collect(),
            list_ids: active.resolved.into_iter().map(|list| list.id).collect(),
            folders: active.folders,
            default_list: active.default_list,
            problems,
        }
    }
}

impl Render for Shown {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let Some(name) = &self.active else {
            let mut rows = vec![("Context", "none: every list is shown".to_owned())];
            rows.extend(
                self.problems
                    .iter()
                    .map(|problem| ("Warning", problem.clone())),
            );
            return rows;
        };
        let mut rows = vec![
            ("Context", name.clone()),
            (
                "Lists",
                format!("{} ({})", self.lists.join(", "), self.lists.len()),
            ),
        ];
        if !self.folders.is_empty() {
            rows.push(("Folders", self.folders.join(", ")));
        }
        rows.push((
            "Default list",
            self.default_list
                .clone()
                .unwrap_or_else(|| "Tasks".to_owned()),
        ));
        rows.extend(
            self.problems
                .iter()
                .map(|problem| ("Warning", problem.clone())),
        );
        rows
    }
}
