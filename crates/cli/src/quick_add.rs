//! Quick add in the CLI: `tasks add` reads its text with `ms-todo-nlp`
//! unless `--no-parse`, and `tasks parse` shows the reading without
//! writing (docs/blueprint/06-natural-language.md#agent-safety). Explicit
//! flags always win over what the text says (D-018).

use chrono::NaiveDate;
use ms_todo_core::{DATE_FORMAT, ErrorKind, Paths, REMINDER_FORMAT};
use ms_todo_nlp::{
    DeterministicParser, ListRef, ParsedTask, QuickAddContext, QuickAddParser, Recurrence,
};
use ms_todo_protocol::{Clearable, NewTask, RawWriteMethod, Request, ResponseData, SyncState};
use serde::Serialize;
use serde_json::{Value, json};

use crate::args::{AddArgs, ParseArgs};
use crate::error::CliError;
use crate::output::{OutputFormat, Render, print_success};
use crate::{daemon_client, data_commands, phrases};

/// Graph's path for the user's Outlook categories.
const CATEGORIES: &str = "/me/outlook/masterCategories";

/// The text read against the lists and categories, and the categories
/// known, or `None` when they couldn't be read.
struct Reading {
    parsed: ParsedTask,
    known: Option<Vec<String>>,
    /// With a `due` given, the due date the text alone would have set.
    text_due: Option<NaiveDate>,
}

/// Read `text` against the daemon's lists (when it has a `#`) and, when
/// it names a category, the user's Outlook categories. `due` is `--due`,
/// which wins over the text's.
async fn read(paths: &Paths, text: &str, due: Option<NaiveDate>) -> Result<Reading, CliError> {
    let lists = if text.contains('#') {
        list_refs(paths).await?
    } else {
        Vec::new()
    };
    let mut ctx = QuickAddContext {
        when: phrases::now(),
        lists: &lists,
        categories: None,
        due,
    };
    let text_due = due.and_then(|_| {
        let alone = QuickAddContext { due: None, ..ctx };
        DeterministicParser.parse(text, &alone).due
    });
    let parsed = DeterministicParser.parse(text, &ctx);
    if parsed.categories.is_empty() {
        return Ok(Reading {
            parsed,
            known: None,
            text_due,
        });
    }
    // Only when a label was typed: it's a request to Graph.
    let (known, unread) = match data_commands::raw_get(paths, CATEGORIES.into()).await {
        Ok(body) => (Some(category_names(&body)), None),
        Err(error) => (None, Some(error.message)),
    };
    ctx.categories = known.as_deref();
    let mut parsed = DeterministicParser.parse(text, &ctx);
    if let Some(why) = unread {
        parsed.warnings.push(format!(
            "couldn't read your Outlook categories ({why}), so no @label was checked"
        ));
    }
    Ok(Reading {
        parsed,
        known,
        text_due,
    })
}

/// The daemon's lists, for `#List`. A daemon just started has none yet,
/// so it's asked to sync first.
async fn list_refs(paths: &Paths) -> Result<Vec<ListRef>, CliError> {
    let (mut lists, sync) = data_commands::lists(paths).await?;
    if sync.state == SyncState::Initial {
        daemon_client::ask(paths, Request::Sync { wait: true }).await?;
        lists = data_commands::lists(paths).await?.0;
    }
    Ok(lists
        .iter()
        .filter_map(|list| {
            Some(ListRef {
                id: list.get("id")?.as_str()?.to_owned(),
                name: list.get("displayName")?.as_str()?.to_owned(),
            })
        })
        .collect())
}

fn category_names(body: &Value) -> Vec<String> {
    body.get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|category| category.get("displayName")?.as_str())
        .map(str::to_owned)
        .collect()
}

/// `tasks add` without `--no-parse`: the task the text and flags make,
/// after creating any category `--create-categories` asks for.
pub async fn new_task(
    paths: &Paths,
    args: &AddArgs,
    format: OutputFormat,
) -> Result<NewTask, CliError> {
    let due = match &args.due {
        Some(Clearable::Set(day)) => NaiveDate::parse_from_str(day, DATE_FORMAT).ok(),
        _ => None,
    };
    let Reading {
        parsed,
        known,
        text_due,
    } = read(paths, &args.text, due).await?;
    let (task, notes) = merge(&parsed, text_due, args)?;
    if args.create_categories && !args.dry_run {
        let missing = known.iter().flat_map(|known| {
            task.categories.iter().filter(|name| {
                !known
                    .iter()
                    .any(|category| category.eq_ignore_ascii_case(name))
            })
        });
        for name in missing {
            create_category(paths, name).await?;
        }
    }
    // JSON's stdout and stderr are the output contract; `tasks parse`
    // gives the warnings there.
    if !matches!(format, OutputFormat::Json | OutputFormat::Jsonl) {
        for note in parsed.warnings.iter().chain(&notes) {
            eprintln!("note: {note}");
        }
    }
    Ok(task)
}

/// The task to send: the flags, else what the text says (D-018), and
/// what the flags overrode.
fn merge(
    parsed: &ParsedTask,
    text_due: Option<NaiveDate>,
    args: &AddArgs,
) -> Result<(NewTask, Vec<String>), CliError> {
    if parsed.title.is_empty() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            format!(
                "nothing is left for the title once \"{}\" is read; add words, put the text in \
                 quotes, or pass --no-parse",
                args.text
            ),
        ));
    }
    let mut notes = Vec::new();
    let mut overrode = |flag: &str, given: bool, parsed: bool| {
        if given && parsed {
            notes.push(format!("{flag} wins over the text's"));
        }
    };
    overrode("--list", args.list.is_some(), parsed.list.is_some());
    overrode("--due", args.due.is_some(), text_due.is_some());
    overrode(
        "--reminder",
        args.reminder.is_some(),
        parsed.reminder.is_some(),
    );
    overrode(
        "--importance",
        args.importance.is_some(),
        parsed.importance.is_some(),
    );
    let clear_due = matches!(args.due, Some(Clearable::Clear));
    if clear_due && (parsed.recurrence.is_some() || parsed.start.is_some()) {
        // Refused rather than dropping what the text asked for: a
        // recurrence starts on its due date, and a start date makes
        // Microsoft To Do set one anyway (S11).
        let what = if parsed.recurrence.is_some() {
            "a recurrence needs a due date to start on"
        } else {
            "a start date sets the due date too"
        };
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            format!(
                "--due - clears the due date, but {what}; take that part out of the text, or \
                 pass --no-parse"
            ),
        ));
    }
    let task = NewTask {
        title: parsed.title.clone(),
        list: args
            .list
            .clone()
            .or_else(|| parsed.list.as_ref().map(|list| list.id.clone())),
        // `--due` was read into the parse, so the reminder and a
        // recurrence's start already follow it.
        // `--due -` and `--reminder -` clear what the text set (D-018).
        due: parsed
            .due
            .filter(|_| !clear_due)
            .map(|due| due.format(DATE_FORMAT).to_string()),
        reminder: match &args.reminder {
            Some(Clearable::Set(at)) => Some(at.clone()),
            Some(Clearable::Clear) => None,
            None => parsed
                .reminder
                .map(|at| at.format(REMINDER_FORMAT).to_string()),
        },
        importance: args
            .importance
            .or(parsed.importance.map(phrases::protocol_importance)),
        body: args.body.clone(),
        start: parsed.start.map(|day| day.format(DATE_FORMAT).to_string()),
        recurrence: parsed.recurrence.as_ref().map(Recurrence::to_graph),
        categories: parsed.categories.clone(),
        my_day: parsed.my_day || args.my_day,
    };
    Ok((task, notes))
}

/// Add `name` to the user's Outlook categories, sent once, as `raw` is.
async fn create_category(paths: &Paths, name: &str) -> Result<(), CliError> {
    let op_id = uuid::Uuid::new_v4().to_string();
    let request = Request::RawWrite {
        method: RawWriteMethod::Post,
        path: CATEGORIES.into(),
        body: Some(json!({ "displayName": name })),
        op_id: Some(op_id.clone()),
    };
    let check = "with `ms-todo raw GET /me/outlook/masterCategories`";
    match daemon_client::ask_mutation(paths, request, &op_id, check).await? {
        ResponseData::Raw { .. } => Ok(()),
        _ => Err(crate::unexpected_response()),
    }
}

/// What `tasks parse` prints.
#[derive(Serialize)]
#[serde(transparent)]
pub struct Parsed {
    json: Value,
    #[serde(skip)]
    rows: Vec<(&'static str, String)>,
}

impl Render for Parsed {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        self.rows.clone()
    }
}

/// `tasks parse`: the reading, in every format, and nothing written.
pub async fn parse(paths: &Paths, args: ParseArgs, format: OutputFormat) -> Result<(), CliError> {
    let Reading { parsed, .. } = read(paths, &args.text, None).await?;
    let now = phrases::now();
    let mut json = parsed.to_json(&args.text);
    json["input"] = json!(args.text);
    let recognised: Vec<String> = parsed
        .spans
        .iter()
        .map(|span| {
            format!(
                "{} {:?}",
                span.kind.name(),
                &args.text[span.start..span.end]
            )
        })
        .collect();
    let rows = vec![
        ("Title", parsed.title.clone()),
        (
            "List",
            parsed
                .list
                .as_ref()
                .map_or_else(|| "Tasks (the default)".into(), |list| list.name.clone()),
        ),
        ("Summary", parsed.summary(&now).join(" \u{b7} ")),
        ("Recognised", recognised.join(", ")),
        ("Warnings", parsed.warnings.join("; ")),
    ];
    print_success(format, &Parsed { json, rows })
}
