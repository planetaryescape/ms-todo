//! `ms-todo done`: what was completed, day by day, newest first (rung 5d).
//! The daemon places each completion on a local day (S12 keeps only the
//! day); a table groups by it, and every other format gives flat rows with
//! `completed_on` and `list`.

use std::io::Write;

use chrono::NaiveDate;
use ms_todo_core::{DATE_FORMAT, Paths, completion_heading, display_safe};
use ms_todo_protocol::{Entity, Request, ResponseData, SyncState};

use crate::args::DoneArgs;
use crate::csv_columns::{self, text};
use crate::error::CliError;
use crate::output::{OutputFormat, Table, print_collection};
use crate::{daemon_client, phrases};

/// How far back `done` looks without `--since`.
const DEFAULT_DAYS: i64 = 7;

pub const DONE_TABLE: Table = Table {
    headings: &["COMPLETED", "LIST", "TITLE"],
    row: |task| {
        vec![
            text(task, "completed_on").to_owned(),
            text(task, "list").to_owned(),
            text(task, "title").to_owned(),
        ]
    },
    csv_headings: csv_columns::DONE_COLUMNS,
    csv_row: csv_columns::done_row,
};

pub async fn done(paths: &Paths, args: DoneArgs, format: OutputFormat) -> Result<(), CliError> {
    let request = Request::CompletedTasks {
        since: args
            .since
            .unwrap_or_else(|| phrases::days_from_today(-DEFAULT_DAYS)),
        until: args.until,
        list: args.list,
        folder: args.folder,
        limit: args.limit,
    };
    let (items, sync) = match daemon_client::ask(paths, request).await? {
        ResponseData::Tasks { items, sync } => (items, sync),
        _ => return Err(crate::unexpected_response()),
    };
    if format != OutputFormat::Table {
        return print_collection(format, &items, sync, &DONE_TABLE);
    }
    if sync.state == SyncState::Initial {
        eprintln!(
            "ms-todo is still syncing for the first time, so this may be incomplete; \
             `ms-todo sync --wait` waits for it"
        );
    }
    let today = chrono::Local::now().date_naive();
    let mut stdout = std::io::stdout().lock();
    for line in grouped(&items, today) {
        writeln!(stdout, "{}", display_safe(&line))?;
    }
    Ok(())
}

/// A heading per day, newest first, and under it each task with its list.
fn grouped(items: &[Entity], today: NaiveDate) -> Vec<String> {
    if items.is_empty() {
        return vec!["Nothing completed then.".into()];
    }
    let width = items
        .iter()
        .map(|task| text(task, "title").chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    let mut last: Option<&str> = None;
    for task in items {
        let day = text(task, "completed_on");
        if last != Some(day) {
            lines.push(heading(day, today));
            last = Some(day);
        }
        let title = text(task, "title");
        let pad = " ".repeat(width.saturating_sub(title.chars().count()));
        lines.push(format!("  {title}{pad}  {}", text(task, "list")));
    }
    lines
}

fn heading(day: &str, today: NaiveDate) -> String {
    completion_heading(NaiveDate::parse_from_str(day, DATE_FORMAT).ok(), today)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn task(title: &str, list: &str, day: Option<&str>) -> Entity {
        json!({ "title": title, "list": list, "completed_on": day })
            .as_object()
            .cloned()
            .expect("object")
    }

    #[test]
    fn a_table_groups_by_day_newest_first() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 24).expect("date");
        let items = [
            task("Ship it", "Work", None),
            task("Buy milk", "Groceries", Some("2026-09-24")),
            task("Call Sam", "Home", Some("2026-09-23")),
            task("Pay rent", "Home", Some("2026-09-21")),
        ];
        assert_eq!(
            grouped(&items, today),
            [
                "Not synced yet",
                "  Ship it   Work",
                "Today",
                "  Buy milk  Groceries",
                "Yesterday",
                "  Call Sam  Home",
                "Mon 21 Sep",
                "  Pay rent  Home",
            ]
        );
        assert_eq!(grouped(&[], today), ["Nothing completed then."]);
    }
}
