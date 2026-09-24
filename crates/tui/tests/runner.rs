//! The event loop end to end, against a fake daemon on a real Unix
//! socket: it subscribes, draws the seed, sends `x` as a `ChangeTasks`,
//! draws the answer at once and follows the `EntityChanged` that comes
//! after it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use futures_util::{SinkExt, StreamExt};
use ms_todo_protocol::{
    Applied, Codec, Counts, Entity, EntityChanged, Event, Message, OutboxDepth, Payload, Request,
    Response, ResponseData, Scope, Seed, SyncActivity, SyncInfo, SyncState, TaskAction, TaskChange,
};
use ms_todo_tui::testing::{App, Clock, UNICODE, connect, run_loop};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::Framed;

fn task(id: &str, title: &str, status: &str, sync: &str) -> Entity {
    let value = json!({
        "id": id, "list_id": "home", "title": title, "status": status,
        "importance": "normal", "sync_state": sync
    });
    value.as_object().cloned().expect("object")
}

/// A daemon with one list, Home, whose tasks are `tasks`. It answers
/// `Subscribe`, `Seed` and `ChangeTasks` (complete), records every request,
/// and after a change sends `EntityChanged`, then serves the change as
/// synced.
async fn fake_daemon(listener: UnixListener, requests: Arc<Mutex<Vec<Request>>>) {
    let (stream, _) = listener.accept().await.expect("accept");
    let mut framed = Framed::new(stream, Codec::new());
    let mut tasks = vec![
        task("t1", "Pay rent", "notStarted", "synced"),
        task("t2", "Call Sam", "notStarted", "synced"),
    ];
    let mut subscription = 0;
    while let Some(Ok(message)) = framed.next().await {
        let Payload::Request(request) = message.payload else {
            continue;
        };
        requests.lock().expect("lock").push(request.clone());
        let reply = |data| Message {
            id: message.id,
            payload: Payload::Response(Response::Ok { data }),
        };
        match request {
            Request::Subscribe => {
                subscription = message.id;
                framed.send(reply(ResponseData::Ack)).await.expect("send");
            }
            Request::Seed { .. } => {
                let ready = SyncInfo {
                    state: SyncState::Ready,
                    generation: 1,
                };
                let seed = Seed {
                    scope: Some(Scope::List { id: "home".into() }),
                    lists: vec![
                        json!({ "id": "home", "displayName": "Home" })
                            .as_object()
                            .cloned()
                            .expect("object"),
                    ],
                    lists_sync: ready,
                    counts: Counts::default(),
                    tasks: tasks.clone(),
                    sync: ready,
                    activity: SyncActivity::default(),
                    outbox: OutboxDepth::default(),
                };
                framed
                    .send(reply(ResponseData::Seed(seed)))
                    .await
                    .expect("send");
            }
            Request::ChangeTasks {
                tasks: ids,
                change: TaskChange::Complete,
                ..
            } => {
                let mut items = Vec::new();
                for task in tasks.iter_mut().filter(|task| ids.contains(&text(task))) {
                    task.insert("status".into(), json!("completed"));
                    task.insert("sync_state".into(), json!("pending"));
                    items.push(task.clone());
                }
                let applied = Applied {
                    op_id: "op-1".into(),
                    action: TaskAction::Complete,
                    list_ids: vec!["home".into(); items.len()],
                    items,
                    rolled: Vec::new(),
                    undoes: None,
                };
                framed
                    .send(reply(ResponseData::Applied(applied)))
                    .await
                    .expect("send");
                // Graph took it: synced, and subscribers hear.
                for task in &mut tasks {
                    task.insert("sync_state".into(), json!("synced"));
                }
                framed
                    .send(Message {
                        id: subscription,
                        payload: Payload::Event(Event::EntityChanged(EntityChanged {
                            lists: Vec::new(),
                            tasks: ids,
                        })),
                    })
                    .await
                    .expect("send");
            }
            _ => {}
        }
    }
}

fn text(task: &Entity) -> String {
    task.get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn key(ch: char) -> std::io::Result<TermEvent> {
    Ok(TermEvent::Key(KeyEvent::new(
        KeyCode::Char(ch),
        KeyModifiers::NONE,
    )))
}

#[tokio::test]
async fn keys_drive_the_daemon_and_the_screen_follows() {
    let dir = tempfile::Builder::new()
        .prefix("mt")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let socket = dir.path().join("daemon.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let daemon = tokio::spawn(fake_daemon(listener, Arc::clone(&requests)));

    let (link, messages) = connect(socket);
    let (keys, scripted) = mpsc::unbounded_channel();
    let input = Box::pin(futures_util::stream::unfold(
        scripted,
        |mut keys| async move { keys.recv().await.map(|key| (key, keys)) },
    ));
    let (painted, first_paint) = oneshot::channel();
    let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("terminal");
    let driver = tokio::spawn(async move {
        first_paint.await.expect("painted");
        for ch in ['j', 'x'] {
            keys.send(key(ch)).expect("send");
        }
        // Let the answer and the event land, then quit.
        tokio::time::sleep(Duration::from_millis(300)).await;
        keys.send(key('q')).expect("send");
    });
    let app = App::new(UNICODE, Clock::now());
    let latency = tokio::time::timeout(
        Duration::from_secs(10),
        run_loop(
            &mut terminal,
            input,
            link,
            messages,
            app,
            Instant::now(),
            Some(painted),
        ),
    )
    .await
    .expect("the loop ends on q")
    .expect("runs");
    driver.await.expect("driver");

    let screen = terminal.backend().to_string();
    assert!(screen.contains("✓ Call Sam"), "{screen}");
    assert!(screen.contains("○ Pay rent"), "{screen}");
    assert!(!screen.contains('◌'), "synced after the event: {screen}");

    let requests = requests.lock().expect("lock").clone();
    assert_eq!(requests[0], Request::Subscribe);
    assert!(matches!(
        &requests[1],
        Request::Seed {
            scope: None,
            search: None
        }
    ));
    let change = requests
        .iter()
        .find(|request| matches!(request, Request::ChangeTasks { .. }))
        .expect("a change");
    assert!(
        matches!(change, Request::ChangeTasks { tasks, change: TaskChange::Complete, .. } if tasks == &["t2".to_owned()])
    );
    // The event brought a fresh seed after the change.
    let seeds = requests
        .iter()
        .filter(|request| matches!(request, Request::Seed { .. }))
        .count();
    assert!(seeds >= 2, "{requests:?}");

    assert!(latency.cold_start.is_some());
    assert_eq!(latency.keypress.len(), 3);
    assert_eq!(latency.write.len(), 1);
    daemon.abort();
}
