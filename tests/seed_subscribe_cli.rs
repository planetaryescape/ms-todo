//! `Seed` and `Subscribe` against the real daemon and a fake Graph: every
//! scope from the cache, and the events a subscriber hears (a write's
//! `EntityChanged`, a rejection, sync passes, and `ResyncNeeded` past the
//! 500-ID cap).

mod support;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ms_todo_protocol::{
    Codec, Event, MAX_CHANGED_IDS, Message, Payload, Request, Response, ResponseData, Scope, Seed,
    SyncState,
};
use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

struct Client {
    framed: Framed<UnixStream, Codec>,
    next_id: u64,
}

impl Client {
    async fn connect(env: &Env) -> Self {
        let stream = UnixStream::connect(env.socket()).await.expect("connect");
        Self {
            framed: Framed::new(stream, Codec::new()),
            next_id: 0,
        }
    }

    async fn send(&mut self, request: Request) -> u64 {
        self.next_id += 1;
        self.framed
            .send(Message {
                id: self.next_id,
                payload: Payload::Request(request),
            })
            .await
            .expect("send");
        self.next_id
    }

    /// The next message, within 10 seconds.
    async fn next(&mut self) -> Message {
        tokio::time::timeout(Duration::from_secs(10), self.framed.next())
            .await
            .expect("a message within 10 seconds")
            .expect("the connection is open")
            .expect("readable")
    }

    async fn ask(&mut self, request: Request) -> ResponseData {
        let id = self.send(request).await;
        loop {
            let message = self.next().await;
            match message.payload {
                Payload::Response(Response::Ok { data }) if message.id == id => return data,
                Payload::Response(response) if message.id == id => {
                    unreachable!("{response:?}")
                }
                _ => {}
            }
        }
    }

    async fn seed(&mut self, scope: Option<Scope>, search: Option<&str>) -> Seed {
        match self
            .ask(Request::Seed {
                scope,
                search: search.map(str::to_owned),
            })
            .await
        {
            ResponseData::Seed(seed) => seed,
            other => unreachable!("{other:?}"),
        }
    }

    /// Subscribe; returns the subscription's message ID.
    async fn subscribe(&mut self) -> u64 {
        let id = self.send(Request::Subscribe).await;
        let ack = self.next().await;
        assert_eq!(
            (ack.id, ack.payload),
            (
                id,
                Payload::Response(Response::Ok {
                    data: ResponseData::Ack
                })
            )
        );
        let first = self.next().await;
        assert!(
            matches!(first.payload, Payload::Event(Event::SyncState(_))) && first.id == id,
            "a SyncState follows the Ack: {first:?}"
        );
        id
    }

    /// Events on subscription `id` until one satisfies `wanted`.
    async fn event_until(&mut self, id: u64, wanted: impl Fn(&Event) -> bool) -> Event {
        loop {
            let message = self.next().await;
            if let Payload::Event(event) = message.payload
                && message.id == id
                && wanted(&event)
            {
                return event;
            }
        }
    }
}

fn titles(seed: &Seed) -> Vec<&str> {
    seed.tasks
        .iter()
        .map(|task| task["title"].as_str().expect("title"))
        .collect()
}

fn with(mut task: Value, extra: Value) -> Value {
    if let (Some(task), Some(extra)) = (task.as_object_mut(), extra.as_object()) {
        task.extend(extra.clone());
    }
    task
}

/// "Tasks" and "Home", synced: an important task, a planned one, an open
/// one and a completed one.
async fn synced_env() -> (Env, FakeGraph) {
    let mut env = Env::new();
    let graph = FakeGraph::start(
        &mut env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-home", "Home", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        data.tasks.insert(
            "L-tasks".into(),
            vec![
                with(
                    task("T1", "Call the broker", "W/\"T1\""),
                    json!({ "importance": "high" }),
                ),
                with(
                    task("T2", "Buy milk", "W/\"T2\""),
                    json!({ "status": "completed" }),
                ),
            ],
        );
        data.tasks.insert(
            "L-home".into(),
            vec![
                with(
                    task("H1", "Renew car insurance", "W/\"H1\""),
                    json!({ "dueDateTime": { "dateTime": "2026-09-25T23:00:00.0000000", "timeZone": "UTC" } }),
                ),
                task("H2", "Fix the boiler", "W/\"H2\""),
            ],
        );
    });
    env.synced();
    (env, graph)
}

#[tokio::test]
async fn seed_answers_every_scope_with_the_counts_from_the_cache() {
    let (env, _graph) = synced_env().await;
    let mut client = Client::connect(&env).await;

    let default = client.seed(None, None).await;
    let tasks_id = env.local_id(&["lists", "list"], "L-tasks");
    let home_id = env.local_id(&["lists", "list"], "L-home");
    assert_eq!(default.scope, Some(Scope::List { id: tasks_id }));
    assert_eq!(default.lists.len(), 2);
    assert_eq!(default.lists_sync.state, SyncState::Ready);
    assert_eq!(default.sync.state, SyncState::Ready);
    assert_eq!(titles(&default), ["Call the broker", "Buy milk"]);
    let counts = &default.counts;
    assert_eq!(
        (
            counts.important,
            counts.planned,
            counts.all,
            counts.completed
        ),
        (1, 1, 3, 1)
    );
    assert_eq!(counts.lists.get(&home_id), Some(&2));
    assert!(!default.activity.in_progress);
    assert!(default.activity.generation >= 1);
    assert!(default.activity.last_finished_at.is_some());

    let planned = client.seed(Some(Scope::Planned), None).await;
    assert_eq!(planned.scope, Some(Scope::Planned));
    assert_eq!(titles(&planned), ["Renew car insurance"]);
    let all = client.seed(Some(Scope::All), None).await;
    assert_eq!(titles(&all).len(), 3);
    let completed = client.seed(Some(Scope::Completed), None).await;
    assert_eq!(titles(&completed), ["Buy milk"]);
    let important = client.seed(Some(Scope::Important), None).await;
    assert_eq!(titles(&important), ["Call the broker"]);

    // A list by name, and a search within a scope.
    let home = client
        .seed(Some(Scope::List { id: "Home".into() }), None)
        .await;
    assert_eq!(
        home.scope,
        Some(Scope::List {
            id: home_id.clone()
        })
    );
    let found = client
        .seed(Some(Scope::List { id: home_id }), Some("boil*"))
        .await;
    assert_eq!(titles(&found), ["Fix the boiler"]);
    let found = client.seed(Some(Scope::All), Some("insurance")).await;
    assert_eq!(titles(&found), ["Renew car insurance"]);
    assert!(
        client
            .seed(Some(Scope::Important), Some("insurance"))
            .await
            .tasks
            .is_empty(),
        "a match outside the view isn't in it"
    );
}

#[tokio::test]
async fn a_subscriber_hears_a_write_its_rejection_and_the_sync_passes() {
    let (env, _graph) = synced_env().await;
    let mut client = Client::connect(&env).await;
    let subscription = client.subscribe().await;

    // The fake Graph has no POST, so the create is rejected (404) after
    // it was queued.
    let added = env.json(&["tasks", "add", "Book the MOT", "--list", "Home"]);
    let id = added["items"][0]["id"].as_str().expect("id").to_owned();
    let changed = client
        .event_until(
            subscription,
            |event| matches!(event, Event::EntityChanged(changed) if changed.tasks.contains(&id)),
        )
        .await;
    assert!(matches!(changed, Event::EntityChanged(_)));
    let rejected = client
        .event_until(subscription, |event| {
            matches!(event, Event::WriteRejected(_))
        })
        .await;
    let Event::WriteRejected(rejected) = rejected else {
        unreachable!("{rejected:?}")
    };
    assert_eq!(rejected.task_id, id);

    // The connection still answers requests while subscribed.
    let seed = client.seed(None, None).await;
    assert_eq!(seed.lists.len(), 2);

    env.json(&["sync"]);
    let started = client
        .event_until(
            subscription,
            |event| matches!(event, Event::SyncState(activity) if activity.in_progress),
        )
        .await;
    let Event::SyncState(started) = started else {
        unreachable!("{started:?}")
    };
    let finished = client
        .event_until(
            subscription,
            |event| matches!(event, Event::SyncState(activity) if !activity.in_progress),
        )
        .await;
    let Event::SyncState(finished) = finished else {
        unreachable!("{finished:?}")
    };
    assert_eq!(finished.generation, started.generation + 1);
    assert!(finished.last_finished_at.is_some());
}

#[tokio::test]
async fn a_change_to_more_tasks_than_the_cap_is_a_resync() {
    let (env, graph) = synced_env().await;
    let mut client = Client::connect(&env).await;
    let subscription = client.subscribe().await;

    // A phone (or a script) adds more tasks at once than one event names.
    graph.edit(|data| {
        let home = data.tasks.entry("L-home".into()).or_default();
        for n in 0..=MAX_CHANGED_IDS {
            home.push(task(&format!("B{n}"), &format!("Bulk {n}"), "W/\"b\""));
        }
    });
    env.synced();
    let event = client
        .event_until(subscription, |event| {
            matches!(event, Event::ResyncNeeded)
                || matches!(event, Event::EntityChanged(changed) if !changed.tasks.is_empty())
        })
        .await;
    assert_eq!(event, Event::ResyncNeeded);
}
