//! What each request does. Rungs 1 and 2 read and write straight to Graph,
//! with no cache (D-034); rung 3a replaces the list and task handlers with
//! store reads.

use std::sync::Arc;

use ms_todo_core::{ErrorKind, message_with_causes};
use ms_todo_graph::auth::Authenticator;
use ms_todo_graph::{GraphClient, GraphError, Method};
use ms_todo_protocol::{
    DaemonStatus, ErrorPayload, PROTOCOL_VERSION, RawWriteMethod, Request, Response, ResponseData,
};
use serde_json::Value;

use crate::known_tasks::KnownTasks;
use crate::list_resolution::resolve_list;
use crate::task_writes::{add_task, change_tasks};

pub(crate) struct State {
    pub auth: Arc<Authenticator>,
    pub graph: GraphClient,
    pub known: KnownTasks,
    pub instance: String,
    pub started_at: i64,
}

pub(crate) async fn handle(state: &State, request: Request) -> Response {
    let result = match request {
        Request::Status => Ok(ResponseData::Status(status(state))),
        Request::ListLists => state
            .graph
            .list_lists()
            .await
            .map(|items| ResponseData::Lists { items })
            .map_err(graph_error),
        Request::ListTasks { list } => list_tasks(state, list.as_deref()).await,
        Request::RawGet { path } => state
            .graph
            .get_raw(&path)
            .await
            .map(|body| ResponseData::Raw { body })
            .map_err(graph_error),
        Request::RawWrite { method, path, body } => raw_write(state, method, &path, body).await,
        Request::AddTask { task, dry_run } => add_task(state, task, dry_run).await,
        Request::ChangeTasks {
            tasks,
            list,
            change,
            dry_run,
        } => change_tasks(state, &tasks, list.as_deref(), change, dry_run).await,
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
    match result {
        Ok(data) => Response::Ok { data },
        Err(error) => Response::Error { error },
    }
}

fn status(state: &State) -> DaemonStatus {
    DaemonStatus {
        protocol_version: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        instance: state.instance.clone(),
        started_at: state.started_at,
        // A token file we can't read still counts as signed in here: the
        // next data request reports what's wrong with it.
        signed_in: !matches!(state.auth.stored_token(), Ok(None)),
    }
}

async fn list_tasks(state: &State, wanted: Option<&str>) -> Result<ResponseData, ErrorPayload> {
    let lists = state.graph.list_lists().await.map_err(graph_error)?;
    let list = resolve_list(&lists, wanted)?;
    let items = state
        .graph
        .list_tasks(&list.id)
        .await
        .map_err(graph_error)?;
    state.known.remember_all(&list.id, &items);
    Ok(ResponseData::Tasks { items })
}

async fn raw_write(
    state: &State,
    method: RawWriteMethod,
    path: &str,
    body: Option<Value>,
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
            ..graph_error(error)
        }),
        Err(error) => Err(graph_error(error)),
    }
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

pub(crate) fn error_payload(kind: ErrorKind, message: String) -> ErrorPayload {
    ErrorPayload {
        kind: kind.as_str().to_owned(),
        message,
        ..ErrorPayload::default()
    }
}
