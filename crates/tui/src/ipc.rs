// Request/response correlation by message ID is adapted from mxr
// crates/tui/src/ipc.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a. Changes:
// one connection carries both the requests and the `Subscribe` stream, so
// events and answers arrive in the order the daemon sent them; it
// reconnects by itself, re-subscribing, if the daemon restarts; and it
// starts a daemon that stays away (D-065).

//! The TUI's one connection to the daemon. Requests go out tagged, and
//! every answer and event comes back to the runner as a [`Msg`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;

use futures_util::{SinkExt, StreamExt};
use ms_todo_core::ErrorKind;
use ms_todo_protocol::{
    Codec, ErrorPayload, Message, Payload, Request, Response, ResponseData, SOCKET_BUFFER_BYTES,
};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::codec::Framed;

use crate::app::{Effect, Msg, Tag};

/// Between attempts to reach a daemon that went away.
const RECONNECT_EVERY: Duration = Duration::from_millis(500);
/// On macOS a connect that races the daemon closing its socket can wait
/// forever (docs/issues/003-flaky-task-writes-tests.md), so it's bounded.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the daemon may stay unreachable before it's started again: a
/// restart (`daemon restart`, an upgrade) is back well within it.
const RESTART_AFTER: Duration = Duration::from_secs(3);

/// Starts the daemon as a command does when it finds none (the detached
/// launcher, D-046), and says why it couldn't. Supplied by the CLI, which
/// owns starting it.
pub type Restart = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Sends requests to the daemon. Dropping it closes the connection.
#[derive(Clone)]
pub struct DaemonLink {
    outgoing: mpsc::UnboundedSender<Effect>,
}

impl DaemonLink {
    pub fn send(&self, effect: Effect) {
        // The connection task only ends when the runner does.
        let _ = self.outgoing.send(effect);
    }
}

/// Connect to the daemon at `socket`, and keep connecting after it goes
/// away; with `restart`, start it again once it has stayed away for
/// [`RESTART_AFTER`]. Answers and events arrive on the receiver.
pub fn connect(
    socket: PathBuf,
    restart: Option<Restart>,
) -> (DaemonLink, mpsc::UnboundedReceiver<Msg>) {
    let (outgoing, requests) = mpsc::unbounded_channel();
    let (incoming, messages) = mpsc::unbounded_channel();
    tokio::spawn(run(socket, restart, requests, incoming));
    (DaemonLink { outgoing }, messages)
}

async fn run(
    socket: PathBuf,
    restart: Option<Restart>,
    mut requests: mpsc::UnboundedReceiver<Effect>,
    incoming: mpsc::UnboundedSender<Msg>,
) {
    // Since when the daemon has been unreachable, and whether it was
    // started again in that time: once per outage, so a daemon that
    // can't start isn't started over and over.
    let mut away_since: Option<tokio::time::Instant> = None;
    let mut restarted = false;
    // Why starting it again failed, kept on the status line after.
    let mut failed: Option<String> = None;
    loop {
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&socket)).await;
        let why = match connected {
            Ok(Ok(stream)) => {
                away_since = None;
                restarted = false;
                failed = None;
                // See SOCKET_BUFFER_BYTES; best effort, as the daemon's side.
                let _ = socket2::SockRef::from(&stream).set_recv_buffer_size(SOCKET_BUFFER_BYTES);
                serve(stream, &mut requests, &incoming).await
            }
            Ok(Err(error)) => Some(format!("cannot reach the daemon: {error}")),
            Err(_) => Some("the daemon didn't accept a connection".to_owned()),
        };
        let Some(mut why) = why else {
            // The runner is gone.
            return;
        };
        if let Some(failed) = &failed {
            why = format!("{why}; starting it again failed: {failed}");
        }
        if incoming.send(Msg::Disconnected(why)).is_err() {
            return;
        }
        let away = away_since.get_or_insert_with(tokio::time::Instant::now);
        if let Some(restart) = restart.as_ref().filter(|_| !restarted)
            && away.elapsed() >= RESTART_AFTER
        {
            restarted = true;
            let _ = incoming.send(Msg::Disconnected(
                "the daemon went away; starting it again…".to_owned(),
            ));
            if let Err(why) = restart().await {
                let _ = incoming.send(Msg::Disconnected(format!(
                    "the daemon went away and couldn't be started again: {why}"
                )));
                failed = Some(why);
            }
        }
        // Requests made while disconnected can't be sent; say so rather
        // than leave them waiting.
        tokio::time::sleep(RECONNECT_EVERY).await;
        while let Ok(effect) = requests.try_recv() {
            let _ = incoming.send(lost(effect.tag, "the daemon isn't running"));
        }
    }
}

/// Serve one connection until it fails, and say why; `None` when the
/// runner has gone away.
async fn serve(
    stream: UnixStream,
    requests: &mut mpsc::UnboundedReceiver<Effect>,
    incoming: &mpsc::UnboundedSender<Msg>,
) -> Option<String> {
    serve_with_stall(stream, requests, incoming, Duration::from_secs(300)).await
}

async fn serve_with_stall(
    stream: UnixStream,
    requests: &mut mpsc::UnboundedReceiver<Effect>,
    incoming: &mpsc::UnboundedSender<Msg>,
    stall: Duration,
) -> Option<String> {
    let mut framed = Framed::new(stream, Codec::new());
    let mut next_id: u64 = 1;
    let subscribe_id = next_id;
    let subscribed = tokio::time::timeout(
        CONNECT_TIMEOUT,
        framed.send(Message {
            id: subscribe_id,
            payload: Payload::Request(Request::Subscribe),
        }),
    )
    .await;
    match subscribed {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Some(format!("cannot subscribe to the daemon: {error}")),
        Err(_) => return Some("the daemon stalled while sending Subscribe".into()),
    }
    let mut subscription_deadline = Some(tokio::time::Instant::now() + CONNECT_TIMEOUT);
    let mut waiting: HashMap<u64, (Tag, tokio::time::Instant)> = HashMap::new();
    let why = loop {
        let deadline = subscription_deadline
            .into_iter()
            .chain(waiting.values().map(|(_, deadline)| *deadline))
            .min();
        tokio::select! {
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                break if subscription_deadline.is_some_and(|deadline| deadline <= tokio::time::Instant::now()) {
                    "the daemon didn't acknowledge Subscribe before its deadline".to_owned()
                } else {
                    "the daemon stalled before answering a request".to_owned()
                };
            }
            effect = requests.recv() => {
                let effect = effect?;
                next_id += 1;
                let message = Message { id: next_id, payload: Payload::Request(effect.request) };
                // A blocked send must not hide an older unanswered request's deadline.
                let send_deadline = (tokio::time::Instant::now() + stall)
                    .min(deadline.unwrap_or(tokio::time::Instant::now() + stall));
                match tokio::time::timeout_at(send_deadline, framed.send(message)).await {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => {
                        let _ = incoming.send(lost(effect.tag, "the daemon connection broke"));
                        break format!("the daemon connection broke: {error}");
                    }
                    Err(_) => {
                        let _ = incoming.send(lost(effect.tag, "the daemon stalled while sending a request"));
                        break "the daemon stalled while sending a request".into();
                    }
                }
                waiting.insert(next_id, (effect.tag, tokio::time::Instant::now() + stall));
            }
            frame = framed.next() => {
                let message = match frame {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => break format!("the daemon sent something unreadable: {error}"),
                    None => break "the daemon closed the connection".to_owned(),
                };
                let msg = match message.payload {
                    Payload::Event(event) if message.id == subscribe_id => Some(Msg::Event(event)),
                    Payload::Response(Response::Ok { data: ResponseData::Ack })
                        if message.id == subscribe_id => {
                            subscription_deadline = None;
                            Some(Msg::Connected)
                        },
                    // Anything but `Ack` is a daemon that doesn't know
                    // `Subscribe`: without events the TUI would go stale.
                    Payload::Response(response) if message.id == subscribe_id => {
                        let said = match response {
                            Response::Error { error } => format!(" ({})", error.message),
                            _ => String::new(),
                        };
                        break format!(
                            "the daemon is too old or incompatible: it didn't accept a \
                             subscription{said}; restart it with `ms-todo daemon stop`"
                        );
                    }
                    Payload::Response(response) => waiting
                        .remove(&message.id)
                        .map(|(tag, _)| Msg::Response { tag, result: result(response) }),
                    Payload::Event(ms_todo_protocol::Event::SyncProgress(_)) => {
                        if let Some((_, deadline)) = waiting.get_mut(&message.id) {
                            *deadline = tokio::time::Instant::now() + stall;
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(msg) = msg
                    && incoming.send(msg).is_err()
                {
                    return None;
                }
            }
        }
    };
    for (_, (tag, _)) in waiting {
        let _ = incoming.send(lost(tag, &why));
    }
    Some(why)
}

fn result(response: Response) -> Result<ResponseData, ErrorPayload> {
    match response {
        Response::Ok { data } => Ok(data),
        Response::Error { error } => Err(error),
        Response::Unknown => Err(ErrorPayload {
            kind: ErrorKind::DaemonUnavailable.as_str().to_owned(),
            message: "the daemon sent an answer this version can't read; run `ms-todo daemon stop`"
                .into(),
            ..ErrorPayload::default()
        }),
    }
}

/// The answer to a request the connection lost. A write may still have
/// been made, so it says to check rather than to retry.
fn lost(tag: Tag, why: &str) -> Msg {
    let message = match tag {
        Tag::Write(_)
        | Tag::TaskStatus(_, _)
        | Tag::Folders
        | Tag::Lists
        | Tag::Order
        | Tag::Undo => {
            format!("{why}; the change may or may not have been made, so check before trying again")
        }
        Tag::Seed(_)
        | Tag::Prefetch
        | Tag::Sync
        | Tag::Diagnostics(_)
        | Tag::Categories
        | Tag::ListHint
        | Tag::Focus
        | Tag::Download
        | Tag::Context
        | Tag::TriageInbox(_)
        | Tag::TriageSuggest(_) => why.to_owned(),
    };
    Msg::Response {
        tag,
        result: Err(ErrorPayload {
            kind: ErrorKind::DaemonUnavailable.as_str().to_owned(),
            message,
            ..ErrorPayload::default()
        }),
    }
}

#[cfg(test)]
#[path = "ipc_tests.rs"]
mod tests;
