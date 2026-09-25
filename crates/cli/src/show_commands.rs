//! `tasks show` and `lists show` (docs/blueprint/07-cli.md): one entity,
//! every field, from the daemon's cache.

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{Entity, Request, ResponseData, TaskFilter};
use serde::Serialize;
use serde_json::{Value, json};

use crate::args::LinkArgs;
use crate::csv_columns::{self, LIST_COLUMNS, TASK_COLUMNS, text};
use crate::daemon_client;
use crate::error::CliError;
use crate::list_args::ShowListArgs;
use crate::output::{OutputFormat, Render, print_ids, print_success, write_csv};

/// One entity as `print_success` prints it: its JSON, and a row per field
/// for a table.
#[derive(Serialize)]
#[serde(transparent)]
struct Shown {
    json: Value,
    #[serde(skip)]
    rows: Vec<(&'static str, String)>,
}

impl Render for Shown {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        self.rows.clone()
    }
}

/// `tasks show T [--list L]`.
pub async fn task(paths: &Paths, args: LinkArgs, format: OutputFormat) -> Result<(), CliError> {
    let (task, _) = crate::link_commands::task(paths, args).await?;
    match format {
        OutputFormat::Ids => print_ids([text(&task, "id")]),
        OutputFormat::Csv => write_csv(TASK_COLUMNS, &[csv_columns::task_row(&task)]),
        _ => print_success(
            format,
            &Shown {
                rows: task_rows(&task),
                json: Value::Object(task),
            },
        ),
    }
}

fn task_rows(task: &Entity) -> Vec<(&'static str, String)> {
    let count = |key: &str| task.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let steps = task
        .get("checklistItems")
        .and_then(Value::as_array)
        .map(|steps| {
            let done = steps
                .iter()
                .filter(|step| step["isChecked"] == true)
                .count();
            format!("{done} of {} done", steps.len())
        })
        .unwrap_or_default();
    let ours = |key: &str| {
        task.get("extensions")
            .and_then(|extensions| extensions.get(0)?.get(key)?.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let categories = task
        .get("categories")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let rows = vec![
        ("Title", text(task, "title").to_owned()),
        ("ID", text(task, "id").to_owned()),
        ("List", text(task, "list_id").to_owned()),
        ("Status", text(task, "status").to_owned()),
        ("Importance", text(task, "importance").to_owned()),
        ("Due", csv_columns::local_due(task)),
        ("Start", local_day(task, "startDateTime")),
        ("Reminder", csv_columns::reminder(task)),
        (
            "Repeats",
            task.get("recurrence")
                .and_then(ms_todo_tui::describe_recurrence)
                .unwrap_or_default(),
        ),
        ("Categories", categories),
        ("Steps", steps),
        (
            "Link",
            task.get("linkedResources")
                .and_then(|links| links.get(0)?.get("webUrl")?.as_str())
                .unwrap_or_default()
                .to_owned(),
        ),
        (
            "Attachments",
            match count("attachments") {
                0 => String::new(),
                files => files.to_string(),
            },
        ),
        ("Waiting on", csv_columns::assignee(task).to_owned()),
        ("My Day", ours("myDay")),
        (
            "Notes",
            task.get("body")
                .and_then(|body| body.get("content")?.as_str())
                .unwrap_or_default()
                .trim()
                .to_owned(),
        ),
        ("Created", text(task, "createdDateTime").to_owned()),
        ("Modified", text(task, "lastModifiedDateTime").to_owned()),
        ("Sync", text(task, "sync_state").to_owned()),
    ];
    rows.into_iter()
        .filter(|(label, value)| !value.is_empty() || matches!(*label, "Title" | "ID"))
        .collect()
}

fn local_day(task: &Entity, key: &str) -> String {
    let mut only = Entity::new();
    if let Some(value) = task.get(key) {
        only.insert("dueDateTime".into(), value.clone());
    }
    csv_columns::local_due(&only)
}

/// `lists show L`: the list, as `lists list` has it, with how many open
/// and completed tasks it holds.
pub async fn list(paths: &Paths, args: ShowListArgs, format: OutputFormat) -> Result<(), CliError> {
    let (lists, _) = crate::data_commands::lists(paths).await?;
    let matching: Vec<&Entity> = lists
        .iter()
        .filter(|list| {
            text(list, "id") == args.list
                || text(list, "graph_id") == args.list
                || text(list, "displayName") == args.list
        })
        .collect();
    let mut list = match matching.as_slice() {
        [list] => (*list).clone(),
        [] => {
            return Err(CliError::message(
                ErrorKind::NotFound,
                format!("no list is called or has the ID {:?}", args.list),
            ));
        }
        several => {
            let ids: Vec<&str> = several.iter().map(|list| text(list, "id")).collect();
            return Err(CliError::message(
                ErrorKind::InvalidInput,
                format!(
                    "{} lists are called {:?}; name one by its ID: {}",
                    several.len(),
                    args.list,
                    ids.join(", ")
                ),
            ));
        }
    };
    let request = Request::ListTasks {
        list: Some(text(&list, "id").to_owned()),
        search: None,
        assignee: None,
        filter: TaskFilter::default(),
    };
    let tasks = match daemon_client::ask(paths, request).await? {
        ResponseData::Tasks { items, .. } => items,
        _ => return Err(crate::unexpected_response()),
    };
    let completed = tasks
        .iter()
        .filter(|task| text(task, "status") == "completed")
        .count();
    list.insert("open_count".into(), json!(tasks.len() - completed));
    list.insert("completed_count".into(), json!(completed));
    match format {
        OutputFormat::Ids => print_ids([text(&list, "id")]),
        OutputFormat::Csv => {
            let headers: Vec<&str> = LIST_COLUMNS
                .iter()
                .copied()
                .chain(["open_count", "completed_count"])
                .collect();
            let mut row = csv_columns::list_row(&list);
            row.push((tasks.len() - completed).to_string());
            row.push(completed.to_string());
            write_csv(&headers, &[row])
        }
        _ => {
            let rows = vec![
                ("Name", text(&list, "displayName").to_owned()),
                ("ID", text(&list, "id").to_owned()),
                ("Folder", text(&list, "folder").to_owned()),
                ("Kind", text(&list, "wellknownListName").to_owned()),
                (
                    "Shared",
                    if list["isShared"] == true {
                        "yes"
                    } else {
                        "no"
                    }
                    .to_owned(),
                ),
                ("Open", (tasks.len() - completed).to_string()),
                ("Completed", completed.to_string()),
                ("Sync", text(&list, "sync_state").to_owned()),
            ];
            print_success(
                format,
                &Shown {
                    json: Value::Object(list),
                    rows,
                },
            )
        }
    }
}
