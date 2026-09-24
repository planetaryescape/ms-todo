//! What each request does. Reads come from the cache (D-034); writes go
//! synchronously to Graph and then to the cache; `raw` goes straight to
//! Graph.

use std::sync::Arc;

use ms_todo_core::{ErrorKind, message_with_causes};
use ms_todo_graph::auth::Authenticator;
use ms_todo_graph::{GraphClient, GraphError, Method};
use ms_todo_protocol::{
    DaemonStatus, ErrorPayload, PROTOCOL_VERSION, RawWriteMethod, Request, Response, ResponseData,
    SyncReport,
};
use ms_todo_store::{LISTS_SCOPE, Store, StoreError};
use serde_json::Value;

use crate::doctor::doctor;
use crate::idempotency::{fingerprint, run_once};
use crate::reads::{list_lists, list_tasks};
use crate::sync::{PassOutcome, Syncer};
use crate::task_writes::{add_task, change_tasks};

pub(crate) struct State {
    pub auth: Arc<Authenticator>,
    pub graph: Arc<GraphClient>,
    pub store: Arc<Store>,
    pub syncer: Syncer,
    pub instance: String,
    pub started_at: i64,
}

impl State {
    /// Whether a credential is stored. A token file we can't read still
    /// counts: the next Graph request reports what's wrong with it.
    pub fn signed_in(&self) -> bool {
        !matches!(self.auth.stored_token(), Ok(None))
    }
}

pub(crate) async fn handle(state: &State, request: Request) -> Response {
    let result = match request {
        Request::Status => Ok(ResponseData::Status(status(state))),
        Request::ListLists => list_lists(state).await,
        Request::ListTasks { list } => list_tasks(state, list.as_deref()).await,
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
        request @ (Request::AddTask { .. } | Request::ChangeTasks { .. }) => {
            mutate(state, request).await
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
        Request::Unknown => Err(error_payload(
            ErrorKind::Unsupported,
            "this daemon doesn't know that request; restart it with `ms-todo daemon stop`".into(),
        )),
    };
    result.into()
}

/// `tasks add|complete|reopen|edit|delete`, run at most once per
/// `--idempotency-key` (a dry run never uses the key).
async fn mutate(state: &State, request: Request) -> Result<ResponseData, ErrorPayload> {
    let fingerprint = fingerprint(&request);
    match request {
        Request::AddTask {
            task,
            dry_run,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let key = idempotency_key.filter(|_| !dry_run);
            let operation = add_task(state, task, dry_run, op_id.clone());
            run_once(state, key.as_deref(), &fingerprint, &op_id, operation).await
        }
        Request::ChangeTasks {
            tasks,
            list,
            change,
            dry_run,
            op_id,
            idempotency_key,
        } => {
            let op_id = op_id.unwrap_or_else(new_op_id);
            let key = idempotency_key.filter(|_| !dry_run);
            let operation = change_tasks(
                state,
                &tasks,
                list.as_deref(),
                change,
                dry_run,
                op_id.clone(),
            );
            run_once(state, key.as_deref(), &fingerprint, &op_id, operation).await
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
    error_payload(ErrorKind::Internal, message_with_causes(&error))
}

pub(crate) fn error_payload(kind: ErrorKind, message: String) -> ErrorPayload {
    ErrorPayload {
        kind: kind.as_str().to_owned(),
        message,
        ..ErrorPayload::default()
    }
}
