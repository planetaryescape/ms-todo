//! `myday list|add|remove|suggest|rollover` and `tasks list --my-day`
//! (docs/blueprint/05-custom-features.md#my-day). The daemon keeps My Day
//! in our extension and answers from its cache; adding and removing are
//! outbox writes like any other, so `undo` reverses them.

use ms_todo_core::{DATE_FORMAT, Paths};
use ms_todo_protocol::{MyDay, Request, ResponseData, TaskChange};

use crate::args::{MyDayCommand, MyDayTargetArgs};
use crate::csv_columns::{self, text};
use crate::daemon_client;
use crate::data_commands::TASKS_TABLE;
use crate::error::CliError;
use crate::output::{OutputFormat, Table, print_collection};
use crate::task_commands::{change_request, expand_stdin, op_id_unless, send};

pub const SUGGESTIONS_TABLE: Table = Table {
    headings: &["WHY", "DUE", "LIST", "TITLE", "ID"],
    row: |task| {
        let why = match text(task, "suggestion") {
            "due_today" => "due today".to_owned(),
            "overdue" => "overdue".to_owned(),
            "left_over" => match text(task, "left_from") {
                "" => "left over".to_owned(),
                from => format!("left from {from}"),
            },
            other => other.to_owned(),
        };
        vec![
            why,
            csv_columns::local_due(task),
            text(task, "list").to_owned(),
            text(task, "title").to_owned(),
            text(task, "id").to_owned(),
        ]
    },
    csv_headings: csv_columns::SUGGESTION_COLUMNS,
    csv_row: csv_columns::suggestion_row,
    bold_matches: None,
};

pub async fn run(
    paths: &Paths,
    command: MyDayCommand,
    format: OutputFormat,
) -> Result<(), CliError> {
    match command {
        MyDayCommand::List => list(paths, format).await,
        MyDayCommand::Add(args) => change(paths, args, TaskChange::AddToMyDay, format).await,
        MyDayCommand::Remove(args) => {
            change(paths, args, TaskChange::RemoveFromMyDay, format).await
        }
        MyDayCommand::Suggest => {
            let my_day = my_day(paths).await?;
            heading(format, "Suggestions for My Day", &my_day);
            print_collection(format, &my_day.suggestions, my_day.sync, &SUGGESTIONS_TABLE)
        }
        MyDayCommand::Rollover { dry_run } => {
            let request = Request::MyDayRollover {
                dry_run,
                op_id: op_id_unless(dry_run),
            };
            send(paths, request, format).await
        }
    }
}

/// `myday list` and `tasks list --my-day`.
pub async fn list(paths: &Paths, format: OutputFormat) -> Result<(), CliError> {
    let my_day = my_day(paths).await?;
    heading(format, "My Day", &my_day);
    print_collection(format, &my_day.tasks, my_day.sync, &TASKS_TABLE)
}

async fn change(
    paths: &Paths,
    args: MyDayTargetArgs,
    change: TaskChange,
    format: OutputFormat,
) -> Result<(), CliError> {
    let (tasks, _) = expand_stdin(args.tasks)?;
    let request = change_request(
        tasks,
        args.list,
        None,
        change,
        args.dry_run,
        args.idempotency.idempotency_key,
    );
    send(paths, request, format).await
}

async fn my_day(paths: &Paths) -> Result<MyDay, CliError> {
    match daemon_client::ask(paths, Request::MyDay).await? {
        ResponseData::MyDay(my_day) => Ok(my_day),
        _ => Err(crate::unexpected_response()),
    }
}

/// A table's first line names the day, since the day turns over at the
/// rollover time rather than at midnight. Other formats have each task's
/// `myDay` in its extension.
fn heading(format: OutputFormat, what: &str, my_day: &MyDay) {
    if format != OutputFormat::Table {
        return;
    }
    let day = chrono::NaiveDate::parse_from_str(&my_day.date, DATE_FORMAT)
        .map(|day| day.format("%a %-d %b %Y").to_string())
        .unwrap_or_else(|_| my_day.date.clone());
    println!("{what}, {day}");
}
