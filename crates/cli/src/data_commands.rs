//! `lists list`, `tasks list` and `search`, which the daemon answers from
//! its cache, and `raw GET` and `auth bearer`, which it takes to Graph
//! (D-031).

use std::io::IsTerminal;
use std::sync::LazyLock;

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{Entity, Request, ResponseData, SearchStatus, SyncInfo};
use serde::Serialize;
use serde_json::Value;

use crate::args::{SearchArgs, SearchStatusArg};
use crate::csv_columns::{self, text};
use crate::daemon_client;
use crate::error::CliError;
use crate::output::{Render, Table};
use crate::time::rfc3339;

pub const LISTS_TABLE: Table = Table {
    headings: &["NAME", "SHARED", "ID"],
    row: |list| {
        let mut name = text(list, "displayName").to_owned();
        if text(list, "wellknownListName") == "defaultList" {
            name.push_str(" (default)");
        }
        let shared = if list.get("isShared").and_then(Value::as_bool) == Some(true) {
            "yes"
        } else {
            ""
        };
        vec![name, shared.to_owned(), text(list, "id").to_owned()]
    },
    csv_headings: csv_columns::LIST_COLUMNS,
    csv_row: csv_columns::list_row,
};

pub const TASKS_TABLE: Table = Table {
    headings: &["DONE", "DUE", "IMPORTANT", "SYNC", "TITLE"],
    row: |task| {
        let important = if text(task, "importance") == "high" {
            "yes"
        } else {
            ""
        };
        let due = csv_columns::local_due(task);
        // Only what needs attention: a synced task's cell is empty.
        let sync = match text(task, "sync_state") {
            "synced" => "",
            other => other,
        };
        vec![
            done(task),
            due,
            important.to_owned(),
            sync.to_owned(),
            text(task, "title").to_owned(),
        ]
    },
    csv_headings: csv_columns::TASK_COLUMNS,
    csv_row: csv_columns::task_row,
};

pub const SEARCH_TABLE: Table = Table {
    headings: &["DONE", "DUE", "LIST", "TITLE", "MATCH"],
    row: |task| {
        vec![
            done(task),
            csv_columns::local_due(task),
            text(task, "list").to_owned(),
            text(task, "title").to_owned(),
            emphasise(text(task, "snippet")),
        ]
    },
    csv_headings: csv_columns::SEARCH_COLUMNS,
    csv_row: csv_columns::search_row,
};

fn done(task: &Entity) -> String {
    if text(task, "status") == "completed" {
        "x".into()
    } else {
        String::new()
    }
}

/// Whether the table may use bold: a terminal that allows it
/// (https://no-color.org). Decided once, not per row.
static BOLD: LazyLock<bool> = LazyLock::new(|| {
    std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
});

/// The snippet's `**`-marked matches (`ResponseData::SearchResults`) in
/// bold where allowed; elsewhere the marks stay, as in JSON.
fn emphasise(snippet: &str) -> String {
    if !*BOLD {
        return snippet.to_owned();
    }
    snippet
        .split("**")
        .enumerate()
        .map(|(index, part)| {
            if index % 2 == 1 {
                format!("\x1b[1m{part}\x1b[22m")
            } else {
                part.to_owned()
            }
        })
        .collect()
}

pub async fn lists(paths: &Paths) -> Result<(Vec<Entity>, SyncInfo), CliError> {
    match daemon_client::ask(paths, Request::ListLists).await? {
        ResponseData::Lists { items, sync } => Ok((items, sync)),
        _ => Err(crate::unexpected_response()),
    }
}

pub async fn tasks(
    paths: &Paths,
    list: Option<String>,
    search: Option<String>,
) -> Result<(Vec<Entity>, SyncInfo), CliError> {
    let request = Request::ListTasks { list, search };
    match daemon_client::ask(paths, request).await? {
        ResponseData::Tasks { items, sync } => Ok((items, sync)),
        _ => Err(crate::unexpected_response()),
    }
}

pub async fn search(paths: &Paths, args: SearchArgs) -> Result<(Vec<Entity>, SyncInfo), CliError> {
    let request = Request::SearchTasks {
        query: args.query.join(" "),
        list: args.list,
        status: match args.status {
            SearchStatusArg::Open => SearchStatus::Open,
            SearchStatusArg::Completed => SearchStatus::Completed,
            SearchStatusArg::All => SearchStatus::All,
        },
        limit: Some(args.limit),
    };
    match daemon_client::ask(paths, request).await? {
        ResponseData::SearchResults { items, sync } => Ok((items, sync)),
        _ => Err(crate::unexpected_response()),
    }
}

pub async fn raw_get(paths: &Paths, path: String) -> Result<Value, CliError> {
    match daemon_client::ask(paths, Request::RawGet { path }).await? {
        ResponseData::Raw { body } => Ok(body),
        _ => Err(crate::unexpected_response()),
    }
}

#[derive(Serialize)]
pub struct Bearer {
    pub access_token: String,
    /// RFC 3339, UTC.
    pub expires_at: String,
}

impl Render for Bearer {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Access token", self.access_token.clone()),
            ("Expires", self.expires_at.clone()),
        ]
    }
}

pub async fn bearer(paths: &Paths, reveal_secret: bool) -> Result<Bearer, CliError> {
    if !reveal_secret {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "`auth bearer` prints a live access token that lets anyone act as you until it \
             expires; pass --reveal-secret to print it"
                .into(),
        ));
    }
    match daemon_client::ask(paths, Request::Bearer).await? {
        ResponseData::Bearer {
            access_token,
            expires_at,
        } => Ok(Bearer {
            access_token,
            expires_at: rfc3339(expires_at),
        }),
        _ => Err(crate::unexpected_response()),
    }
}
