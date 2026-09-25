//! `tasks add|complete|reopen|edit|delete`, `reschedule` and `raw` writes.
//! The daemon resolves the targets and builds the plan; `--dry-run` asks
//! it for the plan only. A destructive command without `--yes` asks in a
//! terminal and exits 2 anywhere else, before contacting the daemon.

use std::io::BufRead;

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{
    Clearable, NewTask, RawWriteMethod, Request, ResponseData, TaskChange, TaskEdit, TaskSelect,
};
use serde_json::Value;

use crate::args::{
    AddArgs, EditArgs, MoveArgs, RawArgs, RawMethod, RescheduleArgs, TargetArgs, UndoArgs,
};
use crate::bulk_commands::{self, Bulk};
use crate::confirm::{can_prompt, confirm};
use crate::error::CliError;
use crate::output::OutputFormat;
use crate::task_output::{describe_plan, print_applied, print_plan};
use crate::{daemon_client, data_commands, phrases, quick_add, suggest_commands};

/// Read from stdin in place of a task argument.
const STDIN_MARKER: &str = "-";

/// `tasks add`: the text read for its fields (`quick_add`), or with
/// `--no-parse` taken as the title.
pub async fn add(paths: &Paths, args: AddArgs, format: OutputFormat) -> Result<(), CliError> {
    let task = if args.no_parse {
        NewTask {
            title: args.text.clone(),
            list: args.list.clone(),
            due: phrases::set_only(args.due.clone()),
            reminder: phrases::set_only(args.reminder.clone()),
            importance: args.importance,
            body: args.body.clone(),
            ..NewTask::default()
        }
    } else {
        quick_add::new_task(paths, &args, format).await?
    };
    // Headed for the inbox: once it's there, a likely list is worth a
    // note (rung 6b). Only for people; JSON's stderr is the contract.
    let inbox_title = (task.list.is_none()
        && !matches!(format, OutputFormat::Json | OutputFormat::Jsonl))
    .then(|| task.title.clone());
    let request = Request::AddTask {
        task,
        dry_run: args.dry_run,
        op_id: op_id_unless(args.dry_run),
        idempotency_key: args.idempotency.idempotency_key,
    };
    // Asked alongside the add, so the add never waits on the provider.
    let suggest = async {
        match &inbox_title {
            Some(title) => suggest_commands::ask(paths, title).await.ok().flatten(),
            None => None,
        }
    };
    let (answer, suggestion) = tokio::join!(send_and_print(paths, request, format), suggest);
    let answer = answer?;
    // Quiet when suggestions are off or fail: the add has done its job.
    if let Some(list) = suggestion {
        let added = match &answer {
            ResponseData::Applied(applied) => applied
                .items
                .first()
                .and_then(|task| task.get("id")?.as_str()),
            _ => None,
        };
        eprintln!("note: {}", suggest_commands::add_note(&list, added));
    }
    Ok(())
}

/// `tasks edit`: one task, several named, or those `--overdue` or
/// `--due-before` picks.
pub async fn edit(paths: &Paths, args: EditArgs, format: OutputFormat) -> Result<(), CliError> {
    let edit = TaskEdit {
        title: args.title,
        due: clearable(args.due, args.clear_due),
        importance: args.importance,
        reminder: clearable(args.reminder, args.clear_reminder),
        body: args.body,
    };
    let (tasks, from_stdin) = expand_stdin(args.task)?;
    let bulk = Bulk {
        tasks,
        from_stdin,
        list: args.list,
        select: bulk_commands::select(args.select),
        change: TaskChange::Edit(edit),
        dry_run: args.dry_run,
        yes: args.yes,
        idempotency_key: args.idempotency.idempotency_key,
        verb: "Change",
    };
    bulk_commands::apply(paths, bulk, format).await
}

/// `tasks move`: the tasks named, to the list `--to`.
pub async fn move_tasks(
    paths: &Paths,
    args: MoveArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let (tasks, from_stdin) = expand_stdin(args.tasks)?;
    let bulk = Bulk {
        tasks,
        from_stdin,
        list: args.list,
        select: None,
        change: TaskChange::Move { to: args.to },
        dry_run: args.dry_run,
        yes: args.yes,
        idempotency_key: args.idempotency.idempotency_key,
        verb: "Move",
    };
    bulk_commands::apply(paths, bulk, format).await
}

/// `reschedule`: a due-date edit, to the tasks named or picked.
pub async fn reschedule(
    paths: &Paths,
    args: RescheduleArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let (tasks, from_stdin) = expand_stdin(args.tasks)?;
    let bulk = Bulk {
        tasks,
        from_stdin,
        list: args.list,
        select: bulk_commands::select(args.select),
        change: TaskChange::Edit(TaskEdit {
            due: Some(Clearable::Set(args.to)),
            ..TaskEdit::default()
        }),
        dry_run: args.dry_run,
        yes: args.yes,
        idempotency_key: args.idempotency.idempotency_key,
        verb: "Reschedule",
    };
    bulk_commands::apply(paths, bulk, format).await
}

/// `complete` and `reopen`.
pub async fn change(
    paths: &Paths,
    args: TargetArgs,
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

pub async fn delete(
    paths: &Paths,
    args: TargetArgs,
    yes: bool,
    format: OutputFormat,
) -> Result<(), CliError> {
    let (tasks, from_stdin) = expand_stdin(args.tasks)?;
    let key = args.idempotency.idempotency_key;
    if yes || args.dry_run {
        let request = change_request(
            tasks,
            args.list,
            None,
            TaskChange::Delete,
            args.dry_run,
            key,
        );
        return send(paths, request, format).await;
    }
    // stdin can't be both the IDs and the answer.
    if from_stdin || !can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "there's no terminal to ask in: pass --yes to delete (`ms-todo undo` brings it \
             back), or --dry-run to see what would be deleted"
                .into(),
        ));
    }
    let preview = change_request(tasks, args.list, None, TaskChange::Delete, true, None);
    let plan = match daemon_client::ask(paths, preview).await? {
        ResponseData::Plan(plan) => plan,
        _ => return Err(crate::unexpected_response()),
    };
    eprintln!("{}", describe_plan(&plan).join("\n"));
    if !confirm("Delete?")? {
        eprintln!("Nothing was deleted.");
        return Ok(());
    }
    // The IDs the preview resolved, so what runs is what was shown.
    let ids = plan.targets.into_iter().map(|target| target.id).collect();
    send(
        paths,
        change_request(ids, None, None, TaskChange::Delete, false, key),
        format,
    )
    .await
}

/// `raw METHOD PATH`: a GET, or a write that's sent once and needs `--yes`
/// when there's no terminal.
pub async fn raw(paths: &Paths, args: RawArgs) -> Result<Value, CliError> {
    let write = match args.method {
        RawMethod::Get => None,
        RawMethod::Post => Some(RawWriteMethod::Post),
        RawMethod::Patch => Some(RawWriteMethod::Patch),
        RawMethod::Delete => Some(RawWriteMethod::Delete),
    };
    let takes_body = matches!(write, Some(RawWriteMethod::Post | RawWriteMethod::Patch));
    if args.body.is_some() && !takes_body {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "--body only goes with POST or PATCH".into(),
        ));
    }
    let Some(method) = write else {
        return data_commands::raw_get(paths, args.path).await;
    };
    let body = args
        .body
        .map(|text| {
            serde_json::from_str::<Value>(&text).map_err(|error| {
                CliError::message(
                    ErrorKind::InvalidInput,
                    format!("--body isn't valid JSON: {error}"),
                )
            })
        })
        .transpose()?;
    if !args.yes && !can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "a raw write is sent once, can't be undone, and isn't checked by ms-todo; \
             pass --yes to send it when not in a terminal"
                .into(),
        ));
    }
    let op_id = new_op_id();
    let request = Request::RawWrite {
        method,
        path: args.path,
        body,
        op_id: Some(op_id.clone()),
    };
    match daemon_client::ask_mutation(paths, request, &op_id, "with `ms-todo raw GET`").await? {
        ResponseData::Raw { body } => Ok(body),
        _ => Err(crate::unexpected_response()),
    }
}

/// `undo [OP_ID] [--copy ID]`: queue the inverse of a change.
pub async fn undo(paths: &Paths, args: UndoArgs, format: OutputFormat) -> Result<(), CliError> {
    let request = Request::Undo {
        target: args.op_id,
        copy: args.copy,
        op_id: Some(new_op_id()),
        idempotency_key: args.idempotency.idempotency_key,
    };
    send(paths, request, format).await
}

pub(crate) fn change_request(
    tasks: Vec<String>,
    list: Option<String>,
    select: Option<TaskSelect>,
    change: TaskChange,
    dry_run: bool,
    idempotency_key: Option<String>,
) -> Request {
    Request::ChangeTasks {
        tasks,
        list,
        select,
        change,
        dry_run,
        op_id: op_id_unless(dry_run),
        idempotency_key,
    }
}

/// A real run's `op_id`, made here before sending so it can be reported
/// even if the daemon's answer is lost. A dry run changes nothing, so it
/// has none.
pub(crate) fn op_id_unless(dry_run: bool) -> Option<String> {
    (!dry_run).then(new_op_id)
}

fn new_op_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) async fn send(
    paths: &Paths,
    request: Request,
    format: OutputFormat,
) -> Result<(), CliError> {
    send_and_print(paths, request, format).await.map(|_| ())
}

/// [`send`], giving back what was printed.
async fn send_and_print(
    paths: &Paths,
    request: Request,
    format: OutputFormat,
) -> Result<ResponseData, CliError> {
    let op_id = match &request {
        Request::AddTask { op_id, .. }
        | Request::ChangeTasks { op_id, .. }
        | Request::ChangeLists { op_id, .. }
        | Request::Undo { op_id, .. } => op_id.clone(),
        _ => None,
    };
    let answer = match op_id {
        Some(op_id) => {
            daemon_client::ask_mutation(paths, request, &op_id, "`ms-todo outbox list`").await?
        }
        None => daemon_client::ask(paths, request).await?,
    };
    match &answer {
        ResponseData::Plan(plan) => print_plan(format, plan)?,
        ResponseData::Applied(applied) => print_applied(format, applied)?,
        _ => return Err(crate::unexpected_response()),
    }
    Ok(answer)
}

/// Replace `-` with the IDs on stdin, one per line, blank lines skipped, so
/// `ms-todo tasks list --format ids | ms-todo tasks complete -` works. Also
/// says whether stdin was read.
pub(crate) fn expand_stdin(tasks: Vec<String>) -> Result<(Vec<String>, bool), CliError> {
    if !tasks.iter().any(|task| task == STDIN_MARKER) {
        return Ok((tasks, false));
    }
    let mut from_stdin = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let id = line.trim();
        if !id.is_empty() {
            from_stdin.push(id.to_owned());
        }
    }
    if from_stdin.is_empty() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "`-` reads task IDs from stdin, but stdin had none".into(),
        ));
    }
    let mut expanded = Vec::new();
    for task in tasks {
        if task == STDIN_MARKER {
            expanded.append(&mut from_stdin);
        } else {
            expanded.push(task);
        }
    }
    Ok((expanded, true))
}

fn clearable(value: Option<Clearable<String>>, clear: bool) -> Option<Clearable<String>> {
    match (value, clear) {
        (Some(value), _) => Some(value),
        (None, true) => Some(Clearable::Clear),
        (None, false) => None,
    }
}
