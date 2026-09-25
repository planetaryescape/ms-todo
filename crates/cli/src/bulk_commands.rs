//! `reschedule` and `tasks edit`: one change to the tasks named, or to the
//! open tasks `--overdue` or `--due-before` picks (rung 5d). Either way
//! it's one command, one outbox operation per task under one `op_id`, so
//! one `undo` reverses the batch. A change that may reach more than one
//! task is previewed first: in a terminal it asks, and anywhere else it
//! needs `--yes`. What runs after a yes is the IDs the preview showed.

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{Request, ResponseData, TaskChange, TaskSelect};

use crate::args::SelectArgs;
use crate::confirm::{can_prompt, confirm};
use crate::error::CliError;
use crate::output::OutputFormat;
use crate::task_commands::{change_request, send};
use crate::task_output::describe_plan;
use crate::{daemon_client, phrases};

/// One change, to the tasks named or those `select` picks.
pub struct Bulk {
    pub tasks: Vec<String>,
    /// Whether `tasks` came from stdin, which then can't take an answer.
    pub from_stdin: bool,
    pub list: Option<String>,
    pub select: Option<TaskSelect>,
    pub change: TaskChange,
    pub dry_run: bool,
    pub yes: bool,
    pub idempotency_key: Option<String>,
    /// The question's verb: "Reschedule", "Change".
    pub verb: &'static str,
}

impl Bulk {
    fn request(&self, dry_run: bool) -> Request {
        let key = self.idempotency_key.clone().filter(|_| !dry_run);
        let (tasks, list, select) = (self.tasks.clone(), self.list.clone(), self.select.clone());
        change_request(tasks, list, select, self.change.clone(), dry_run, key)
    }

    /// The same change to exactly these tasks, by ID.
    fn to_ids(&self, ids: Vec<String>) -> Request {
        let key = self.idempotency_key.clone();
        change_request(ids, None, None, self.change.clone(), false, key)
    }
}

/// `--overdue` or `--due-before` as the daemon takes them.
pub fn select(args: SelectArgs) -> Option<TaskSelect> {
    let due_before = match (args.overdue, args.due_before) {
        (_, Some(day)) => day,
        (true, None) => phrases::days_from_today(0),
        (false, None) => return None,
    };
    Some(TaskSelect {
        due_before,
        folder: args.folder,
    })
}

pub async fn apply(paths: &Paths, bulk: Bulk, format: OutputFormat) -> Result<(), CliError> {
    let one_named = bulk.select.is_none() && bulk.tasks.len() <= 1;
    if bulk.dry_run || bulk.yes || one_named {
        return send(paths, bulk.request(bulk.dry_run), format).await;
    }
    let plan = match daemon_client::ask(paths, bulk.request(true)).await? {
        ResponseData::Plan(plan) => plan,
        _ => return Err(crate::unexpected_response()),
    };
    let count = plan.targets.len();
    if count <= 1 {
        return send(paths, bulk.request(false), format).await;
    }
    // stdin can't be both the IDs and the answer.
    if bulk.from_stdin || !can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            format!(
                "this changes {count} tasks and there's no terminal to ask in: pass --yes \
                 (one `ms-todo undo` reverses them all), or --dry-run to see them"
            ),
        ));
    }
    eprintln!("{}", describe_plan(&plan).join("\n"));
    if !confirm(&format!("{} {count} tasks?", bulk.verb))? {
        eprintln!("Nothing was changed.");
        return Ok(());
    }
    let ids = plan.targets.into_iter().map(|target| target.id).collect();
    send(paths, bulk.to_ids(ids), format).await
}
