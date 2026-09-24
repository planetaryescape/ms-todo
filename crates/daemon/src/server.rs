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
use ms_todo_protocol::{
    Codec, Event, FrameTooLarge, Message, Payload, Request, Response, ResponseData,
    SOCKET_BUFFER_BYTES,
};
use ms_todo_store::Store;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Notify, broadcast, mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::codec::Framed;

use crate::events::Events;
use crate::handlers::{State, error_payload, handle};
use crate::idempotency::settle_unfinished;
use crate::outbox::{self, Outbox};
use crate::sync::{PROGRESS, PassContext, SyncStatus, Syncer};

/// Overrides Graph's base URL in debug builds, so tests can point a real
/// daemon at a mock server. Release builds ignore it: it would let whoever
/// sets the environment send the token elsewhere.
const GRAPH_URL_ENV: &str = "MS_TODO_GRAPH_URL";

/// `sun_path` is 104 bytes on macOS (108 on Linux), including the final NUL.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Why the daemon stopped for good.
pub(crate) enum Fatal {
    /// The database has a migration this build doesn't know. The daemon
    /// exits with `EXIT_DATABASE_TOO_NEW` so the client that started it can
    /// say so plainly.
    DatabaseTooNew(String),
    Other(String),
}

impl From<String> for Fatal {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

pub(crate) async fn serve(paths: Paths) -> Result<(), Fatal> {
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
        Err(error) => return Err(describe(&socket, &error).into()),
    }

    if socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
        return Err(format!(
            "the socket path {} is longer than the {MAX_SOCKET_PATH_BYTES} bytes Unix sockets allow; \
             use a shorter home or data directory",
            socket.display()
        )
        .into());
    }
    // Open the sign-in and the database before binding. A daemon that
    // can't start (a database a newer ms-todo migrated, say) then never has
    // a socket: the client that started it sees no connection, only the
    // process's exit status, on every OS. Bound first, a client could
    // connect and have the connection reset as the daemon exits, and read
    // that instead (Linux). Opening both is quick; a slow start is still a
    // live process, which the client waits for.
    atomic_write_mode_0600(&paths.pid_file(), std::process::id().to_string().as_bytes())
        .map_err(|error| describe(&paths.pid_file(), &error))?;
    let state = match build_state(&paths).await {
        Ok(state) => Arc::new(state),
        Err(fatal) => {
            let _ = std::fs::remove_file(paths.pid_file());
            return Err(fatal);
        }
    };
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = std::fs::remove_file(paths.pid_file());
            return Err(describe(&socket, &error).into());
        }
    };
    let served = async {
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| describe(&socket, &error))?;
        settle_unfinished(&state).await;
        // Before anything is sent, so nothing new is mistaken for left over.
        outbox::recover(&state).await;
        eprintln!(
            "ms-todo daemon {} (pid {}) listening on {}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            socket.display()
        );
        accept_until_shutdown(listener, state)
            .await
            .map_err(Fatal::Other)
    }
    .await;

    // Every exit after the bind comes through here, so none leaves a socket
    // behind. We still hold the lock, so both files are ours to remove.
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(paths.pid_file());
    served
}

async fn build_state(paths: &Paths) -> Result<State, Fatal> {
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
    // The cache holds the user's tasks: keep its directory private too.
    ensure_private_dir(&paths.data_dir).map_err(|error| describe(&paths.data_dir, &error))?;
    let store = match Store::open(&paths.database_file()).await {
        Ok(store) => store,
        Err(error @ ms_todo_store::StoreError::NewerDatabase) => {
            return Err(Fatal::DatabaseTooNew(describe_store(
                &paths.database_file(),
                &error,
            )));
        }
        Err(error) => return Err(describe_store(&paths.database_file(), &error).into()),
    };
    // Nothing is running yet, whatever a daemon that died left behind.
    store
        .clear_in_progress()
        .await
        .map_err(|error| describe_store(&paths.database_file(), &error))?;
    Ok(State {
        auth,
        graph: Arc::new(graph),
        store: Arc::new(store),
        syncer: Syncer::new(),
        outbox: Outbox::new(),
        events: Events::new(),
        instance: paths.instance.label().to_owned(),
        started_at: chrono::Utc::now().timestamp(),
    })
}

fn describe_store(path: &Path, error: &ms_todo_store::StoreError) -> String {
    format!(
        "{}: {}",
        path.display(),
        ms_todo_core::message_with_causes(error)
    )
}

async fn accept_until_shutdown(listener: UnixListener, state: Arc<State>) -> Result<(), String> {
    let shutdown = Arc::new(Notify::new());
    let mut terminate = signal(SignalKind::terminate()).map_err(|error| error.to_string())?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|error| error.to_string())?;
    // Dropping the set when we return aborts connections still open, and
    // the sync loop with them.
    let mut connections = JoinSet::new();
    let syncing = Arc::clone(&state);
    connections.spawn(async move {
        let context = PassContext {
            graph: Arc::clone(&syncing.graph),
            store: Arc::clone(&syncing.store),
            events: syncing.events.clone(),
        };
        syncing.syncer.run(context).await;
    });
    connections.spawn(outbox::run(Arc::clone(&state)));
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
    // A connected client, and each request it sends, keep sync polling at
    // the active cadence.
    let _client = state.syncer.client_connected();
    // See SOCKET_BUFFER_BYTES. Best effort: a small buffer is only slower.
    let _ = socket2::SockRef::from(&stream).set_send_buffer_size(SOCKET_BUFFER_BYTES);
    let mut framed = Framed::new(stream, Codec::new());
    let mut subscription: Option<Subscription> = None;
    loop {
        let frame = tokio::select! {
            frame = framed.next() => frame,
            Some((id, event)) = pushed(&mut subscription) => {
                if push(&mut framed, id, event).await.is_err() {
                    return;
                }
                continue;
            }
        };
        let Some(frame) = frame else {
            return;
        };
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
        state.syncer.client_request();
        if request == Request::Subscribe {
            // Subscribed before answering, so nothing that happens after
            // the `Ack` is missed.
            let started = Subscription::new(message.id, &state);
            let first = started.sync.borrow().activity();
            subscription = Some(started);
            let answer = Response::Ok {
                data: ResponseData::Ack,
            };
            if send(&mut framed, message.id, answer).await.is_err()
                || push(&mut framed, message.id, Event::SyncState(first))
                    .await
                    .is_err()
            {
                return;
            }
            continue;
        }
        let stopping = request == Request::Shutdown;
        // Progress of any sync the request waits for goes back as events
        // with its ID, each resetting the client's stall deadline.
        let (progress, mut updates) = mpsc::unbounded_channel();
        let work = PROGRESS.scope(progress, handle(&state, request));
        tokio::pin!(work);
        let response = loop {
            tokio::select! {
                response = &mut work => break response,
                Some(update) = updates.recv() => {
                    let event = Message {
                        id: message.id,
                        payload: Payload::Event(Event::SyncProgress(update)),
                    };
                    if framed.send(event).await.is_err() {
                        // The client went away; nobody to answer.
                        return;
                    }
                }
                // A slow request doesn't hold up the subscriber's events.
                Some((id, event)) = pushed(&mut subscription) => {
                    if push(&mut framed, id, event).await.is_err() {
                        return;
                    }
                }
            }
        };
        if send(&mut framed, message.id, response).await.is_err() {
            return;
        }
        if stopping {
            shutdown.notify_one();
            return;
        }
    }
}

/// A connection's `Subscribe`: the daemon's events, and a `SyncState`
/// whenever a sync pass starts or finishes, all with the `Subscribe`
/// request's message ID.
struct Subscription {
    id: u64,
    events: broadcast::Receiver<Event>,
    sync: watch::Receiver<SyncStatus>,
    /// `(started, finished)` of the last `SyncState` sent: the status
    /// also changes with each progress update, which isn't one.
    passes: (u64, u64),
}

impl Subscription {
    fn new(id: u64, state: &State) -> Self {
        let mut sync = state.syncer.subscribe();
        let passes = {
            let status = sync.borrow_and_update();
            (status.started, status.finished)
        };
        Self {
            id,
            events: state.events.subscribe(),
            sync,
            passes,
        }
    }

    /// The next event to push. Cancel-safe: both receivers are.
    async fn next(&mut self) -> Event {
        loop {
            tokio::select! {
                received = self.events.recv() => match received {
                    Ok(event) => return event,
                    // It missed some: all it can do is read everything again.
                    Err(broadcast::error::RecvError::Lagged(_)) => return Event::ResyncNeeded,
                    // The sender lives as long as the daemon.
                    Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
                },
                changed = self.sync.changed() => {
                    if changed.is_err() {
                        std::future::pending::<()>().await;
                    }
                    let status = self.sync.borrow_and_update().clone();
                    let passes = (status.started, status.finished);
                    if passes != self.passes {
                        self.passes = passes;
                        return Event::SyncState(status.activity());
                    }
                }
            }
        }
    }
}

/// The subscriber's next event with its ID, or never without one.
async fn pushed(subscription: &mut Option<Subscription>) -> Option<(u64, Event)> {
    match subscription {
        Some(subscription) => Some((subscription.id, subscription.next().await)),
        None => std::future::pending().await,
    }
}

async fn push(
    framed: &mut Framed<UnixStream, Codec>,
    id: u64,
    event: Event,
) -> Result<(), std::io::Error> {
    framed
        .send(Message {
            id,
            payload: Payload::Event(event),
        })
        .await
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
