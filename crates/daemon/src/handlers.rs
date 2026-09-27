//! What each request does. Reads come from the cache (D-034); writes go
//! to the cache and the outbox, which sends them to Graph; `raw` goes
//! straight to Graph.

use std::sync::Arc;

use ms_todo_core::{ErrorKind, message_with_causes};
use ms_todo_graph::auth::Authenticator;
use ms_todo_graph::{GraphClient, GraphError, Method};
use ms_todo_protocol::{
    ContextChoice, DaemonStatus, ErrorPayload, PROTOCOL_VERSION, RawWriteMethod, Request, Response,
    ResponseData, SyncReport,
};
use ms_todo_store::{LISTS_SCOPE, Store, StoreError};
use serde_json::Value;

use crate::doctor::doctor;
use crate::events::Events;
use crate::idempotency::{fingerprint, run_once};
use crate::list_writes::change_lists;
use crate::outbox::Outbox;
use crate::reads::{list_lists, list_tasks, search_tasks};
use crate::sync::{PassOutcome, Syncer};
use crate::task_writes::{Targets, add_task, change_tasks};
use crate::undo::undo;

pub(crate) struct State {
    pub auth: Arc<Authenticator>,
    pub graph: Arc<GraphClient>,
    pub store: Arc<Store>,
    pub syncer: Syncer,
    pub outbox: Outbox,
    pub events: Events,
    pub instance: String,
    pub started_at: i64,
    /// Where a move keeps its attachments' bytes until it's settled.
    pub moves_dir: std::path::PathBuf,
    /// Where a deleted attachment's bytes are kept for undo (D-056).
    pub kept_dir: std::path::PathBuf,
    /// List suggestions, when `[suggest]` turns them on.
    pub suggest: crate::suggest::Suggester,
    /// `[my_day]`: when a day's My Day ends.
    pub my_day: crate::my_day::Config,
    /// Semantic search, when `[search]` turns it on.
    pub semantic: crate::semantic::Semantic,
    /// Nag reminders: `[nag]`, and how this machine notifies.
    pub nag: crate::nag::Nagger,
    /// config.toml, read again for each request that uses a context.
    pub config_file: std::path::PathBuf,
    /// The active context (rung 9d).
    pub context: crate::contexts::Active,
}

impl State {
    /// Whether a credential is stored. A token file we can't read still
    /// counts: the next Graph request reports what's wrong with it.
    pub fn signed_in(&self) -> bool {
        !matches!(self.auth.stored_token(), Ok(None))
    }
}

pub(crate) async fn handle(state: &State, request: Request) -> Response {
    // `--context`: the reads a context narrows, and a new task's default
    // list, use this rather than the active context.
    let (choice, request) = match request {
        Request::InContext { context, request } => (Some(context), *request),
        request => (None, request),
    };
    let choice = choice.as_ref();
    if let Err(error) = crate::contexts::check(state, choice) {
        return Response::Error { error };
    }
    let result = match request {
        Request::Status => Ok(ResponseData::Status(status(state))),
        Request::ListLists => list_lists(state).await,
        Request::ListFolders => crate::list_writes::list_folders(state).await,
        Request::GetTasks { tasks, list } => {
            crate::reads::get_tasks(state, &tasks, list.as_deref()).await
        }
        Request::ListTasks {
            list,
            search,
            assignee,
            filter,
        } => {
            list_tasks(
                state,
                choice,
                list.as_deref(),
                search.as_deref(),
                assignee.as_deref(),
                &filter,
            )
            .await
        }
        Request::NextTasks { list, limit } => {
            crate::next::next_tasks(state, choice, list.as_deref(), limit).await
        }
        Request::SearchTasks {
            query,
            list,
            status,
            limit,
            semantic,
        } => {
            search_tasks(
                state,
                choice,
                &query,
                list.as_deref(),
                status,
                limit,
                semantic,
            )
            .await
        }
        Request::CompletedTasks {
            since,
            until,
            list,
            folder,
            limit,
        } => {
            let (until, list, folder) = (until.as_deref(), list.as_deref(), folder.as_deref());
            crate::completed::completed_tasks(state, &since, until, list, folder, limit).await
        }
        Request::Sync { wait: false } => {
            state.syncer.request();
            sync_report(state, false, &PassOutcome::default()).await
        }
        Request::Sync { wait: true } => {
            let number = state.syncer.request();
            let outcome = state.syncer.wait_for(number).await;
            sync_outcome(state, &outcome).await
        }
        Request::Doctor => doctor(state).await,
        Request::Focus { list } => focus(state, list.as_deref()).await,
        Request::NotifyTest => crate::nag::notify_test(state).await,
        Request::RawGet { path } => state
            .graph
            .get_raw(&path)
            .await
            .map(|body| ResponseData::Raw { body })
            .map_err(graph_error),
        Request::RawWrite {
            method,
            path,
            body,
            op_id,
        } => raw_write(state, method, &path, body, op_id).await,
        request @ (Request::AddTask { .. }
        | Request::ChangeTasks { .. }
        | Request::ChangeLists { .. }
        | Request::ChangeCategory { .. }
        | Request::ChangeExtension { .. }
        | Request::Undo { .. }) => mutate(state, choice, request).await,
        Request::Seed {
            scope,
            search,
            include_deferred,
            semantic,
        } => {
            let search = search.as_deref();
            crate::seed::seed(state, choice, scope, search, include_deferred, semantic).await
        }
        // The connection loop answers `Subscribe` itself, and starts the
        // stream.
        Request::Subscribe => Ok(ResponseData::Ack),
        Request::OutboxList { state: wanted } => crate::outbox::list(state, wanted).await,
        Request::OutboxRetry { op_id } => crate::outbox::retry(state, &op_id).await,
        Request::OutboxDiscard { op_id } => crate::outbox::discard(state, &op_id).await,
        Request::SuggestList { title } => {
            let generation = state.syncer.status().finished;
            state
                .suggest
                .suggest(&state.store, generation, &title)
                .await
                .map(|suggestion| ResponseData::ListSuggestion { suggestion })
        }
        Request::MyDay => crate::my_day::my_day(state, choice).await,
        Request::MyDayRollover { dry_run, op_id } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            crate::my_day::rollover(state, dry_run, op_id, crate::my_day::Origin::User).await
        }
        Request::DownloadAttachments {
            task,
            list,
            attachments,
            out_dir,
            force,
        } => {
            crate::attachments::download(
                state,
                &task,
                list.as_deref(),
                &attachments,
                &out_dir,
                force,
            )
            .await
        }
        Request::Bearer => match state.auth.valid_token().await {
            Ok(token) => Ok(ResponseData::Bearer {
                access_token: token.access_token,
                expires_at: token.expires_at,
            }),
            Err(error) => Err(graph_error(error.into())),
        },
        // The connection loop answers `Shutdown` itself, before stopping.
        Request::Shutdown => Ok(ResponseData::Ack),
        Request::ListCategories => crate::catalog::list_categories(state).await,
        Request::ListExtensions { owner } => crate::catalog::list_extensions(state, &owner).await,
        Request::GetExtension { owner, name } => {
            crate::catalog::get_extension(state, &owner, &name).await
        }
        Request::Contexts => crate::contexts::contexts(state).await,
        Request::SetContext { name } => crate::contexts::set_context(state, name).await,
        Request::InContext { .. } => Err(error_payload(
            ErrorKind::InvalidInput,
            "a request is read in one context; `InContext` can't hold another".into(),
        )),
        Request::Unknown => Err(error_payload(
            ErrorKind::Unsupported,
            "this daemon doesn't know that request; restart it with `ms-todo daemon stop`".into(),
        )),
    };
    result.into()
}

/// The TUI's focus hint. It's only a hint: a list that isn't cached
/// (deleted meanwhile, say) clears the focus rather than failing.
async fn focus(state: &State, list: Option<&str>) -> Result<ResponseData, ErrorPayload> {
    let focus = match list {
        // The same list again (paging past it): nothing to look up.
        Some(list) if state.syncer.focused().as_deref() == Some(list) => {
            return Ok(ResponseData::Ack);
        }
        Some(list) => {
            let lists = state.store.lists().await.map_err(store_error)?;
            crate::list_resolution::resolve_list(&lists, Some(list))
                .ok()
                .map(|list| list.local_id)
        }
        None => None,
    };
    state.syncer.focus(focus);
    Ok(ResponseData::Ack)
}

/// `tasks add|complete|reopen|edit|delete`, the folder changes and
/// `undo`, run at most once per `--idempotency-key` (a dry run never uses
/// the key).
async fn mutate(
    state: &State,
    choice: Option<&ContextChoice>,
    request: Request,
) -> Result<ResponseData, ErrorPayload> {
    let fingerprint = match choice {
        Some(context) => fingerprint(&Request::InContext {
            context: context.clone(),
            request: Box::new(request.clone()),
        }),
        None => fingerprint(&request),
    };
    match request {
        Request::AddTask {
            task,
            dry_run,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let key = idempotency_key.filter(|_| !dry_run);
            let operation = add_task(state, choice, task, dry_run, op_id.clone());
            run_once(state, key.as_deref(), &fingerprint, &op_id, operation).await
        }
        Request::ChangeTasks {
            tasks,
            list,
            select,
            change,
            dry_run,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let key = idempotency_key.filter(|_| !dry_run);
            let targets = Targets {
                names: &tasks,
                list: list.as_deref(),
                select: select.as_ref(),
                context: choice,
            };
            let operation = change_tasks(state, targets, change, dry_run, op_id.clone());
            run_once(state, key.as_deref(), &fingerprint, &op_id, operation).await
        }
        Request::ChangeLists {
            change,
            dry_run,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let key = idempotency_key.filter(|_| !dry_run);
            let operation = change_lists(state, change, dry_run, op_id.clone());
            run_once(state, key.as_deref(), &fingerprint, &op_id, operation).await
        }
        Request::ChangeCategory {
            change,
            dry_run,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let key = idempotency_key.filter(|_| !dry_run);
            let operation = crate::catalog::change_category(state, change, dry_run, op_id.clone());
            run_once(state, key.as_deref(), &fingerprint, &op_id, operation).await
        }
        Request::ChangeExtension {
            owner,
            change,
            dry_run,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let key = idempotency_key.filter(|_| !dry_run);
            let operation =
                crate::catalog::change_extension(state, &owner, change, dry_run, op_id.clone());
            run_once(state, key.as_deref(), &fingerprint, &op_id, operation).await
        }
        Request::Undo {
            target,
            copy,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let operation = undo(state, target, copy.as_deref(), op_id.clone());
            run_once(
                state,
                idempotency_key.as_deref(),
                &fingerprint,
                &op_id,
                operation,
            )
            .await
        }
        _ => Err(error_payload(
            ErrorKind::Internal,
            "only a task mutation is run once per idempotency key".into(),
        )),
    }
}

/// The answer to `sync --wait`, once the pass it waited for has finished.
async fn sync_outcome(state: &State, outcome: &PassOutcome) -> Result<ResponseData, ErrorPayload> {
    match &outcome.failure {
        Some(failure) => Err(failure.clone()),
        None => sync_report(state, true, outcome).await,
    }
}

async fn sync_report(
    state: &State,
    waited: bool,
    outcome: &PassOutcome,
) -> Result<ResponseData, ErrorPayload> {
    let generation = state
        .store
        .scope(LISTS_SCOPE)
        .await
        .map_err(store_error)?
        .map_or(0, |row| row.generation());
    Ok(ResponseData::Sync(SyncReport {
        waited,
        scopes: outcome.scopes,
        changed: outcome.changed,
        generation,
    }))
}

fn status(state: &State) -> DaemonStatus {
    DaemonStatus {
        protocol_version: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        instance: state.instance.clone(),
        started_at: state.started_at,
        signed_in: state.signed_in(),
    }
}

async fn raw_write(
    state: &State,
    method: RawWriteMethod,
    path: &str,
    body: Option<Value>,
    op_id: Option<String>,
) -> Result<ResponseData, ErrorPayload> {
    let method = match method {
        RawWriteMethod::Post => Method::POST,
        RawWriteMethod::Patch => Method::PATCH,
        RawWriteMethod::Delete => Method::DELETE,
    };
    match state.graph.write_raw(method, path, body.as_ref()).await {
        Ok(body) => Ok(ResponseData::Raw { body }),
        Err(error @ GraphError::OutcomeUnknown(_)) => Err(ErrorPayload {
            message: format!(
                "{}. The request may or may not have been applied, and ms-todo never resends it; check with `ms-todo raw GET` before sending it again",
                message_with_causes(&error)
            ),
            op_id,
            ..graph_error(error)
        }),
        Err(error) => Err(graph_error(error)),
    }
}

fn new_op_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn graph_error(error: GraphError) -> ErrorPayload {
    ErrorPayload {
        kind: error.kind().as_str().to_owned(),
        message: message_with_causes(&error),
        graph_code: error.graph_code().map(str::to_owned),
        request_id: error.request_id().map(str::to_owned),
        ..ErrorPayload::default()
    }
}

pub(crate) fn store_error(error: StoreError) -> ErrorPayload {
    let kind = match error {
        StoreError::InvalidQuery(_) => ErrorKind::InvalidInput,
        _ => ErrorKind::Internal,
    };
    error_payload(kind, message_with_causes(&error))
}

pub(crate) fn error_payload(kind: ErrorKind, message: String) -> ErrorPayload {
    ErrorPayload {
        kind: kind.as_str().to_owned(),
        message,
        ..ErrorPayload::default()
    }
}
