//! What each request does. Rung 1 reads straight from Graph, with no cache
//! (D-034); rung 3a replaces the list and task handlers with store reads.

use std::sync::Arc;

use ms_todo_core::{ErrorKind, message_with_causes};
use ms_todo_graph::auth::Authenticator;
use ms_todo_graph::{GraphClient, GraphError};
use ms_todo_protocol::{
    DaemonStatus, ErrorPayload, PROTOCOL_VERSION, Request, Response, ResponseData,
};

use crate::list_resolution::resolve_list;

pub(crate) struct State {
    pub auth: Arc<Authenticator>,
    pub graph: GraphClient,
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
    Ok(ResponseData::Tasks { items })
}

pub(crate) fn graph_error(error: GraphError) -> ErrorPayload {
    ErrorPayload {
        kind: error.kind().as_str().to_owned(),
        message: message_with_causes(&error),
        graph_code: error.graph_code().map(str::to_owned),
        request_id: error.request_id().map(str::to_owned),
        candidates: Vec::new(),
    }
}

pub(crate) fn error_payload(kind: ErrorKind, message: String) -> ErrorPayload {
    ErrorPayload {
        kind: kind.as_str().to_owned(),
        message,
        graph_code: None,
        request_id: None,
        candidates: Vec::new(),
    }
}
