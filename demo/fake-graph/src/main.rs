//! A fake Microsoft Graph for the demo recordings: the integration tests'
//! double, filled from `demo/seed.json` and listening on a fixed local port,
//! so `demo/run.sh` can point a throwaway ms-todo instance at it. Nothing
//! here ever talks to Microsoft.
//!
//! Usage: `demo-fake-graph <seed.json> [port]` (port 47812 by default).
//!
//! Seed dates are days relative to today, so a recording always looks
//! current: `due: 0` is today, `-2` two days overdue, and `completed: -1`
//! finished yesterday.

use std::net::TcpListener;

use chrono::{Datelike, Days, Local, NaiveDate, TimeZone, Utc};
use ms_todo_fake_graph::{FakeGraph, list};
use serde::Deserialize;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const DEFAULT_PORT: u16 = 47812;
/// Our open extension, where folders live (docs/blueprint/05-custom-features.md).
const EXTENSION: &str = "com.planetaryescape.mstodo";

#[derive(Deserialize)]
struct Seed {
    me: Value,
    lists: Vec<SeedList>,
}

#[derive(Deserialize)]
struct SeedList {
    name: String,
    #[serde(default)]
    wellknown: Option<String>,
    #[serde(default)]
    folder: Option<String>,
    tasks: Vec<SeedTask>,
}

#[derive(Deserialize)]
struct SeedTask {
    title: String,
    /// Days from today.
    #[serde(default)]
    due: Option<i64>,
    /// The next such day of the month, for a monthly task.
    #[serde(default)]
    due_day_of_month: Option<u32>,
    /// Days from today it was completed (0 or less).
    #[serde(default)]
    completed: Option<i64>,
    #[serde(default)]
    importance: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    steps: Vec<SeedStep>,
    /// Graph's recurrence `pattern`; the range starts on the due date.
    #[serde(default)]
    recurrence: Option<Value>,
    /// Days ago it was created; two weeks by default.
    #[serde(default = "two_weeks")]
    created: i64,
}

#[derive(Deserialize)]
struct SeedStep {
    title: String,
    #[serde(default)]
    done: bool,
}

fn two_weeks() -> i64 {
    14
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seed_path = args
        .next()
        .ok_or("usage: demo-fake-graph <seed.json> [port]")?;
    let port = match args.next() {
        Some(port) => port.parse()?,
        None => DEFAULT_PORT,
    };
    let seed: Seed = serde_json::from_slice(&std::fs::read(&seed_path)?)?;
    let today = Local::now().date_naive();

    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let server = MockServer::builder()
        .listener(listener)
        // It runs for a whole recording; nothing reads the history.
        .disable_request_recording()
        .start()
        .await;
    let base = format!("{}/v1.0", server.uri());

    Mock::given(method("GET"))
        .and(path("/v1.0/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(seed.me.clone()))
        .mount(&server)
        .await;
    // Anything the double doesn't answer is logged, so a recording that
    // needs a new endpoint says which.
    Mock::given(wiremock::matchers::any())
        .respond_with(|request: &Request| {
            eprintln!(
                "demo-fake-graph: no answer for {} {}",
                request.method,
                request.url.path()
            );
            ResponseTemplate::new(404).set_body_json(json!({ "error": {
                "code": "ErrorItemNotFound", "message": "not in the demo"
            } }))
        })
        .with_priority(u8::MAX)
        .mount(&server)
        .await;

    let lists = seed_lists(&seed, today)?;
    let graph = FakeGraph::serve(
        server,
        lists.iter().map(|(list, ..)| list.clone()).collect(),
    )
    .await;
    graph.accept_task_patches().await;
    graph.accept_moves().await;
    graph.edit(|data| {
        for (list, tasks, extension) in lists {
            let id = list["id"].as_str().unwrap_or_default().to_owned();
            if let Some(extension) = extension {
                data.extensions.insert(id.clone(), extension);
            }
            data.tasks.insert(id, tasks);
        }
    });

    println!("demo-fake-graph: listening on {base}");
    std::future::pending::<()>().await;
    Ok(())
}

type SeededList = (Value, Vec<Value>, Option<Value>);

/// Each list in Graph's shape, its tasks, and our extension (the folder and
/// the order in it) when it's in a folder.
fn seed_lists(seed: &Seed, today: NaiveDate) -> Result<Vec<SeededList>, String> {
    let mut folders: Vec<&str> = Vec::new();
    let mut in_folder: Vec<usize> = Vec::new();
    let mut next_task: u32 = 0;
    let mut seeded = Vec::new();
    for (index, seed_list) in seed.lists.iter().enumerate() {
        let id = format!("L-{}", index + 1);
        let graph_list = list(
            &id,
            &seed_list.name,
            seed_list.wellknown.as_deref().unwrap_or("none"),
        );
        let extension = seed_list.folder.as_deref().map(|folder| {
            let folder_index = folders
                .iter()
                .position(|known| *known == folder)
                .unwrap_or_else(|| {
                    folders.push(folder);
                    in_folder.push(0);
                    folders.len() - 1
                });
            in_folder[folder_index] += 1;
            json!({
                "extensionName": EXTENSION,
                "id": format!("microsoft.graph.openTypeExtension.{EXTENSION}"),
                "folder": folder,
                "folderOrder": folder_index + 1,
                "order": in_folder[folder_index],
            })
        });
        let mut tasks = Vec::new();
        for seed_task in &seed_list.tasks {
            next_task += 1;
            tasks.push(task(next_task, seed_task, today)?);
        }
        seeded.push((graph_list, tasks, extension));
    }
    Ok(seeded)
}

/// The `number`th task in the seed. Lists show tasks in the order they
/// were created, so each is a minute after the one before.
fn task(number: u32, seed: &SeedTask, today: NaiveDate) -> Result<Value, String> {
    let id = format!("T-{number}");
    let created = format!(
        "{}T{:02}:{:02}:00.0000000Z",
        day(today, -seed.created)?,
        8 + number / 60,
        number % 60
    );
    let mut task = json!({
        "@odata.etag": format!("W/\"{id}-1\""),
        "id": id,
        "title": seed.title,
        "status": "notStarted",
        "importance": seed.importance.as_deref().unwrap_or("normal"),
        "isReminderOn": false,
        "hasAttachments": false,
        "categories": [],
        "createdDateTime": created,
        "lastModifiedDateTime": created,
        "body": { "content": seed.notes.as_deref().unwrap_or(""), "contentType": "text" },
    });
    let due = match (seed.due, seed.due_day_of_month) {
        (Some(offset), _) => Some(day(today, offset)?),
        (None, Some(day_of_month)) => Some(next_day_of_month(today, day_of_month)?),
        (None, None) => None,
    };
    if let Some(due) = due {
        task["dueDateTime"] = local_midnight(due)?;
        if let Some(pattern) = &seed.recurrence {
            task["recurrence"] = json!({
                "pattern": pattern,
                "range": {
                    "type": "noEnd",
                    "startDate": due.format("%Y-%m-%d").to_string(),
                    "endDate": "0001-01-01",
                    "recurrenceTimeZone": "UTC",
                    "numberOfOccurrences": 0
                }
            });
        }
    }
    if let Some(offset) = seed.completed {
        task["status"] = json!("completed");
        // As Graph reports it: the day, at midnight UTC (S12).
        task["completedDateTime"] = json!({
            "dateTime": format!("{}T00:00:00.0000000", day(today, offset)?),
            "timeZone": "UTC"
        });
    }
    if !seed.steps.is_empty() {
        let steps: Vec<Value> = seed
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                json!({
                    "id": format!("{id}-step-{}", index + 1),
                    "displayName": step.title,
                    "isChecked": step.done,
                    "createdDateTime": created,
                })
            })
            .collect();
        task["checklistItems"] = json!(steps);
    }
    Ok(task)
}

fn day(today: NaiveDate, offset: i64) -> Result<NaiveDate, String> {
    let days = Days::new(offset.unsigned_abs());
    if offset < 0 {
        today.checked_sub_days(days)
    } else {
        today.checked_add_days(days)
    }
    .ok_or_else(|| format!("{offset} days from {today} is out of range"))
}

fn next_day_of_month(today: NaiveDate, day_of_month: u32) -> Result<NaiveDate, String> {
    let this_month = today.with_day(day_of_month);
    match this_month {
        Some(date) if date >= today => Ok(date),
        _ => {
            let (year, month) = if today.month() == 12 {
                (today.year() + 1, 1)
            } else {
                (today.year(), today.month() + 1)
            };
            NaiveDate::from_ymd_opt(year, month, day_of_month)
                .ok_or_else(|| format!("no day {day_of_month} in {year}-{month:02}"))
        }
    }
}

/// A due date as the phone writes it: local midnight, in UTC (S11).
fn local_midnight(date: NaiveDate) -> Result<Value, String> {
    let midnight = date.and_hms_opt(0, 0, 0).ok_or("no midnight")?;
    let local = Local
        .from_local_datetime(&midnight)
        .earliest()
        .ok_or_else(|| format!("no local midnight on {date}"))?;
    Ok(json!({
        "dateTime": local.with_timezone(&Utc).format("%Y-%m-%dT%H:%M:%S.0000000").to_string(),
        "timeZone": "UTC"
    }))
}
