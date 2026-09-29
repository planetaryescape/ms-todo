//! Real Unix socket regressions for requests sharing a subscription.

use super::*;
use ms_todo_protocol::Event;
use tokio::task::JoinHandle;

const STALL: Duration = Duration::from_secs(3);

async fn settle() {
    // Socket readiness and the connection task each need a scheduler turn.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

type Peer = Framed<UnixStream, Codec>;

fn fixture() -> (
    Peer,
    mpsc::UnboundedSender<Effect>,
    mpsc::UnboundedReceiver<Msg>,
    JoinHandle<Option<String>>,
) {
    let (client, server) = UnixStream::pair().expect("socket pair");
    let (outgoing, mut requests) = mpsc::unbounded_channel();
    let (incoming, messages) = mpsc::unbounded_channel();
    let task =
        tokio::spawn(
            async move { serve_with_stall(client, &mut requests, &incoming, STALL).await },
        );
    (Framed::new(server, Codec::new()), outgoing, messages, task)
}

async fn subscribed(peer: &mut Peer, messages: &mut mpsc::UnboundedReceiver<Msg>) -> u64 {
    let message = peer.next().await.expect("subscribe").expect("frame");
    assert!(matches!(
        message.payload,
        Payload::Request(Request::Subscribe)
    ));
    peer.send(Message {
        id: message.id,
        payload: Payload::Response(Response::Ok {
            data: ResponseData::Ack,
        }),
    })
    .await
    .expect("ack");
    assert_eq!(messages.recv().await, Some(Msg::Connected));
    message.id
}

fn seed() -> Effect {
    Effect {
        tag: Tag::Seed(1),
        request: Request::Seed {
            scope: None,
            search: None,
            include_deferred: false,
            semantic: false,
        },
    }
}

#[tokio::test(start_paused = true)]
async fn a_silent_subscribe_is_bounded() {
    let (mut peer, _outgoing, _messages, task) = fixture();
    peer.next().await.expect("subscribe").expect("frame");
    settle().await;
    tokio::time::advance(STALL + Duration::from_secs(1)).await;
    settle().await;
    assert!(task.is_finished(), "subscription must have a deadline");
    assert!(
        task.await
            .expect("task")
            .expect("failure")
            .contains("Subscribe")
    );
}

#[tokio::test(start_paused = true)]
async fn a_silent_seed_or_write_times_out_with_correct_uncertainty() {
    for effect in [
        seed(),
        Effect {
            tag: Tag::Write(crate::app::Write::Complete),
            request: Request::ChangeTasks {
                tasks: vec!["t1".into()],
                list: None,
                select: None,
                change: ms_todo_protocol::TaskChange::Complete,
                dry_run: false,
                op_id: None,
                idempotency_key: None,
            },
        },
    ] {
        let is_write = matches!(effect.tag, Tag::Write(_));
        let (mut peer, outgoing, mut messages, task) = fixture();
        subscribed(&mut peer, &mut messages).await;
        outgoing.send(effect).expect("request");
        peer.next().await.expect("request").expect("frame");
        tokio::time::advance(STALL + Duration::from_secs(1)).await;
        settle().await;
        assert!(task.is_finished(), "silent request must time out");
        task.await.expect("task");
        let Msg::Response {
            result: Err(error), ..
        } = messages.recv().await.expect("failed response")
        else {
            panic!("expected failed request");
        };
        assert_eq!(
            error.message.contains("may or may not"),
            is_write,
            "{}",
            error.message
        );
    }
}

#[tokio::test(start_paused = true)]
async fn relevant_progress_extends_only_its_request_and_idle_subscriptions_survive() {
    let (mut peer, outgoing, mut messages, task) = fixture();
    let subscription = subscribed(&mut peer, &mut messages).await;
    tokio::time::advance(STALL * 3).await;
    assert!(
        !task.is_finished(),
        "idle subscription has no pending deadline"
    );
    outgoing.send(seed()).expect("seed");
    let request = peer.next().await.expect("seed").expect("frame");
    tokio::time::advance(STALL / 2).await;
    peer.send(Message {
        id: request.id,
        payload: Payload::Event(Event::SyncProgress(ms_todo_protocol::SyncProgress {
            scopes_done: 1,
            scopes_total: 2,
            doing: "tasks".into(),
        })),
    })
    .await
    .expect("progress");
    settle().await;
    tokio::time::advance(STALL / 2 + Duration::from_millis(1)).await;
    assert!(!task.is_finished(), "its progress extends the deadline");
    // Subscription traffic cannot keep a silent Seed alive forever.
    peer.send(Message {
        id: subscription,
        payload: Payload::Event(Event::ResyncNeeded),
    })
    .await
    .expect("unrelated event");
    assert!(matches!(
        messages.recv().await,
        Some(Msg::Event(Event::ResyncNeeded))
    ));
    tokio::time::advance(STALL / 2 + Duration::from_millis(1)).await;
    settle().await;
    assert!(
        task.is_finished(),
        "unrelated traffic must not extend a request"
    );
    task.await.expect("task");
}

#[tokio::test(start_paused = true)]
async fn a_socket_that_stops_reading_cannot_block_a_send_forever() {
    let (mut peer, outgoing, mut messages, task) = fixture();
    subscribed(&mut peer, &mut messages).await;
    let mut effect = seed();
    let Request::Seed { search, .. } = &mut effect.request else {
        unreachable!()
    };
    *search = Some("x".repeat(2_000_000));
    outgoing.send(effect).expect("request");
    settle().await;
    tokio::time::advance(STALL + Duration::from_secs(1)).await;
    settle().await;
    assert!(task.is_finished(), "blocked send must time out");
    task.await.expect("task");
}

#[tokio::test(start_paused = true)]
async fn reconnect_subscribes_again_without_replaying_an_uncertain_write() {
    let directory = tempfile::Builder::new()
        .prefix("mt-ipc")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let socket = directory.path().join("daemon.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("listen");
    let (link, mut messages) = connect(socket, None);
    let (stream, _) = listener.accept().await.expect("first connection");
    let mut peer = Framed::new(stream, Codec::new());
    subscribed(&mut peer, &mut messages).await;
    link.send(Effect {
        tag: Tag::TaskStatus(1, crate::app::Write::Complete),
        request: Request::ChangeTasks {
            tasks: vec!["t1".into()],
            list: None,
            select: None,
            change: ms_todo_protocol::TaskChange::Complete,
            dry_run: false,
            op_id: None,
            idempotency_key: None,
        },
    });
    assert!(matches!(
        peer.next().await.expect("write").expect("frame").payload,
        Payload::Request(Request::ChangeTasks { .. })
    ));
    drop(peer); // The write might have landed; the answer was lost.
    let Msg::Response {
        tag: Tag::TaskStatus(1, _),
        result: Err(error),
    } = messages.recv().await.expect("lost answer")
    else {
        panic!("expected write failure");
    };
    assert!(error.message.contains("may or may not"));
    assert!(matches!(messages.recv().await, Some(Msg::Disconnected(_))));
    tokio::time::advance(RECONNECT_EVERY).await;
    let (stream, _) = listener.accept().await.expect("second connection");
    let mut peer = Framed::new(stream, Codec::new());
    subscribed(&mut peer, &mut messages).await;
    link.send(seed());
    assert!(
        matches!(
            peer.next().await.expect("request").expect("frame").payload,
            Payload::Request(Request::Seed { .. })
        ),
        "only the new read is sent after reconnect"
    );
}
