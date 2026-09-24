// Skeleton adapted from mxr crates/daemon/src/server.rs (`run_daemon_with_overrides`:
// singleton guard before touching the socket, bind, 0600, pid file, ordered
// teardown on every exit path) @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a.
// Left out: TCP and stdio transports, the HTTP bridge, the search index and
// the startup maintenance. The singleton guard is a lock file rather than
// mxr's search-index lock, since ms-todo has no index.

use std::fs::File;
use std::io::ErrorKind as IoErrorKind;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use fs2::FileExt;
use futures_util::{SinkExt, StreamExt};
use ms_todo_core::{ErrorKind, Paths};
use ms_todo_graph::GraphClient;
use ms_todo_graph::auth::{Authenticator, Endpoints};
use ms_todo_graph::private_file::{atomic_write_mode_0600, ensure_private_dir};
use ms_todo_protocol::{Codec, FrameTooLarge, Message, Payload, Request, Response};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::codec::Framed;

use crate::handlers::{State, error_payload, handle};

/// Overrides Graph's base URL in debug builds, so tests can point a real
/// daemon at a mock server. Release builds ignore it: it would let whoever
/// sets the environment send the token elsewhere.
const GRAPH_URL_ENV: &str = "MS_TODO_GRAPH_URL";

/// `sun_path` is 104 bytes on macOS (108 on Linux), including the final NUL.
const MAX_SOCKET_PATH_BYTES: usize = 103;

pub(crate) async fn serve(paths: Paths) -> Result<(), String> {
    ensure_private_dir(&paths.run_dir).map_err(|error| describe(&paths.run_dir, &error))?;

    // The lock, not the socket, decides who the daemon is. Taking it first
    // means a second daemon started by a racing client exits here, before it
    // can touch the first one's socket.
    let Some(_lock) = take_singleton_lock(&paths.daemon_lock_file())? else {
        eprintln!("ms-todo daemon: another daemon already runs for this instance; exiting");
        return Ok(());
    };

    // We hold the lock, so any socket file left here belongs to a daemon
    // that died without cleaning up.
    let socket = paths.socket_path();
    match std::fs::remove_file(&socket) {
        Ok(()) => eprintln!(
            "ms-todo daemon: removed a stale socket at {}",
            socket.display()
        ),
        Err(error) if error.kind() == IoErrorKind::NotFound => {}
        Err(error) => return Err(describe(&socket, &error)),
    }

    // Bind before any slow work, so clients can connect and ask `Status`
    // (vault: `Daemon Readiness Is Not Process Liveness`).
    if socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
        return Err(format!(
            "the socket path {} is longer than the {MAX_SOCKET_PATH_BYTES} bytes Unix sockets allow; \
             use a shorter home or data directory",
            socket.display()
        ));
    }
    let listener = UnixListener::bind(&socket).map_err(|error| describe(&socket, &error))?;
    let served = async {
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| describe(&socket, &error))?;
        atomic_write_mode_0600(&paths.pid_file(), std::process::id().to_string().as_bytes())
            .map_err(|error| describe(&paths.pid_file(), &error))?;
        let state = Arc::new(build_state(&paths)?);
        eprintln!(
            "ms-todo daemon {} (pid {}) listening on {}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            socket.display()
        );
        accept_until_shutdown(listener, state).await
    }
    .await;

    // Every exit after the bind comes through here, so none leaves a socket
    // behind. We still hold the lock, so both files are ours to remove.
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(paths.pid_file());
    served
}

fn build_state(paths: &Paths) -> Result<State, String> {
    let mut endpoints = Endpoints::default();
    if cfg!(debug_assertions)
        && let Ok(url) = std::env::var(GRAPH_URL_ENV)
    {
        endpoints.graph = url;
    }
    let graph_base = endpoints.graph.clone();
    let auth = Arc::new(
        Authenticator::new(paths.auth_dir(), endpoints)
            .map_err(|error| ms_todo_core::message_with_causes(&error))?,
    );
    let graph = GraphClient::new(Arc::clone(&auth), &graph_base)
        .map_err(|error| ms_todo_core::message_with_causes(&error))?;
    Ok(State {
        auth,
        graph,
        known: Default::default(),
        instance: paths.instance.label().to_owned(),
        started_at: chrono::Utc::now().timestamp(),
    })
}

async fn accept_until_shutdown(listener: UnixListener, state: Arc<State>) -> Result<(), String> {
    let shutdown = Arc::new(Notify::new());
    let mut terminate = signal(SignalKind::terminate()).map_err(|error| error.to_string())?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|error| error.to_string())?;
    // Dropping the set when we return aborts connections still open.
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    connections.spawn(serve_connection(stream, Arc::clone(&state), Arc::clone(&shutdown)));
                }
                // One failed accept (e.g. out of file descriptors) mustn't
                // take the daemon down.
                Err(error) => eprintln!("ms-todo daemon: accept failed: {error}"),
            },
            () = shutdown.notified() => break,
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            // Reap finished connection tasks so the set doesn't grow forever.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    eprintln!("ms-todo daemon: shutting down");
    Ok(())
}

async fn serve_connection(stream: UnixStream, state: Arc<State>, shutdown: Arc<Notify>) {
    let mut framed = Framed::new(stream, Codec::new());
    while let Some(frame) = framed.next().await {
        let message = match frame {
            Ok(message) => message,
            Err(error) => {
                // Framing can't recover after a bad frame; say why, then close.
                let error = error_payload(
                    ErrorKind::InvalidInput,
                    format!("unreadable IPC frame: {error}"),
                );
                let _ = send(&mut framed, 0, Response::Error { error }).await;
                return;
            }
        };
        let request = match message.payload {
            Payload::Request(request) => request,
            // Clients only send requests; ignore anything else.
            _ => continue,
        };
        let stopping = request == Request::Shutdown;
        let response = handle(&state, request).await;
        if send(&mut framed, message.id, response).await.is_err() {
            return;
        }
        if stopping {
            shutdown.notify_one();
            return;
        }
    }
}

async fn send(
    framed: &mut Framed<UnixStream, Codec>,
    id: u64,
    response: Response,
) -> Result<(), std::io::Error> {
    let message = Message {
        id,
        payload: Payload::Response(response),
    };
    match framed.send(message).await {
        Err(error)
            if error
                .get_ref()
                .is_some_and(|inner| inner.is::<FrameTooLarge>()) =>
        {
            // Answer with the reason rather than leaving the client waiting.
            let payload = error_payload(
                ErrorKind::Internal,
                format!("the response was too large to send: {error}"),
            );
            framed
                .send(Message {
                    id,
                    payload: Payload::Response(Response::Error { error: payload }),
                })
                .await
        }
        other => other,
    }
}

/// `Ok(None)` when another daemon holds the lock.
fn take_singleton_lock(path: &Path) -> Result<Option<File>, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|error| describe(path, &error))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == IoErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(describe(path, &error)),
    }
}

fn describe(path: &Path, error: &std::io::Error) -> String {
    format!("{}: {error}", path.display())
}
