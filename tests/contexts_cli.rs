//! Contexts through the real binary and daemon, against a fake Graph
//! (docs/blueprint/05-custom-features.md#contexts, rung 9d): `[contexts]`
//! in config.toml, the active one kept by the daemon across restarts,
//! what it narrows and what it doesn't, `--context`, the default list, and
//! the warnings for names that don't resolve.

mod support;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ms_todo_protocol::{
    Codec, Event, Message, Payload, Request, Response, ResponseData, Scope, Seed,
};
use serde_json::{Value, json};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

const CONFIG: &str = r#"
[contexts.work]
folders = ["Areas"]
lists = ["Contentful", "Nope"]
default_list = "Contentful"

[contexts.home]
lists = ["Garden"]
"#;

fn write_config(env: &Env, contents: &str) {
    let dir = env.home.path().join("config").join("ms-todo");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("config.toml"), contents).expect("config");
}

fn with(mut task: Value, extra: Value) -> Value {
    if let (Some(task), Some(extra)) = (task.as_object_mut(), extra.as_object()) {
        task.extend(extra.clone());
    }
    task
}

fn sorted_titles(items: &Value) -> Vec<String> {
    let mut titles: Vec<String> = items
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["title"].as_str().unwrap_or_default().to_owned())
        .collect();
    titles.sort();
    titles
}

/// Graph with the inbox, "Contentful" in no folder, "Money" in Areas,
/// "Garden" in Home and "Books"; an open task in each, due today but in
/// Books, and one waiting on Sam in Garden.
async fn setup() -> (Env, FakeGraph) {
    let mut env = Env::new();
    write_config(&env, CONFIG);
    let graph = FakeGraph::start(
        &mut env,
        vec![
            list("L-tasks", "Tasks", "defaultList"),
            list("L-work", "Contentful", "none"),
            list("L-money", "Money", "none"),
            list("L-garden", "Garden", "none"),
            list("L-books", "Books", "none"),
        ],
    )
    .await;
    graph.edit(|data| {
        for id in ["L-tasks", "L-work", "L-money", "L-garden", "L-books"] {
            data.tasks.insert(id.into(), Vec::new());
        }
        for (list, folder) in [("L-money", "Areas"), ("L-garden", "Home")] {
            data.extensions.insert(
                list.into(),
                json!({ "extensionName": "com.planetaryescape.mstodo", "folder": folder }),
            );
        }
    });
    graph.accept_task_patches().await;
    env.synced();
    let today = env.today().format("%Y-%m-%d").to_string();
    let due = json!({ "dueDateTime": {
        "dateTime": format!("{today}T00:00:00.0000000"),
        "timeZone": "Europe/London"
    } });
    graph.edit(|data| {
        let tasks = [
            ("L-tasks", "T1", "Buy milk"),
            ("L-work", "T2", "Ship the release"),
            ("L-money", "T3", "Pay the invoice"),
            ("L-garden", "T4", "Mow the lawn"),
        ];
        for (list, id, title) in tasks {
            data.tasks.insert(
                list.into(),
                vec![with(task(id, title, "W/\"1\""), due.clone())],
            );
        }
        data.tasks.insert(
            "L-books".into(),
            vec![task("T5", "Read the release notes", "W/\"1\"")],
        );
        data.extensions.insert(
            "T4".into(),
            json!({
                "extensionName": "com.planetaryescape.mstodo",
                "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
                "assignee": "Sam"
            }),
        );
    });
    env.synced();
    (env, graph)
}

fn table(env: &Env, args: &[&str]) -> String {
    let output = env
        .cmd()
        .args(["--format", "table"])
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("utf8")
}

#[tokio::test]
async fn a_context_narrows_the_everyday_reads_and_none_brings_everything_back() {
    let (env, _graph) = setup().await;

    let shown = env.json(&["ctx"]);
    assert_eq!(shown["active"], Value::Null);

    let listed = env.json(&["ctx", "list"]);
    let names: Vec<&str> = listed["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, ["home", "work"]);
    let work = &listed["items"][1];
    assert_eq!(work["list_count"], 2);
    assert_eq!(work["resolved"], json!(["Money", "Contentful"]));
    assert!(
        work["problems"][0]
            .as_str()
            .is_some_and(|problem| problem.contains("no list is named \"Nope\"")),
        "{work}"
    );

    let set = env.json(&["ctx", "work"]);
    assert_eq!(set["active"], "work");
    assert_eq!(set["lists"], json!(["Money", "Contentful"]));
    assert_eq!(set["default_list"], "Contentful");

    // `tasks list` with no --list: the context's open tasks, each with its list.
    let tasks = env.json(&["tasks", "list"]);
    assert_eq!(
        sorted_titles(&tasks["items"]),
        ["Pay the invoice", "Ship the release"]
    );
    assert_eq!(tasks["context"]["name"], "work");
    assert_eq!(tasks["context"]["lists"], 2);
    assert!(tasks["items"][0]["list"].is_string());
    // An explicit --list always wins, even outside the context.
    let inbox = env.json(&["tasks", "list", "--list", "Tasks"]);
    assert_eq!(sorted_titles(&inbox["items"]), ["Buy milk"]);
    assert_eq!(inbox["context"], Value::Null);

    let next = env.json(&["next"]);
    assert_eq!(
        sorted_titles(&next["items"]),
        ["Pay the invoice", "Ship the release"]
    );
    assert_eq!(next["context"]["name"], "work");
    let found = env.json(&["search", "release"]);
    assert_eq!(sorted_titles(&found["items"]), ["Ship the release"]);
    let waiting = env.json(&["waiting"]);
    assert_eq!(sorted_titles(&waiting["items"]), Vec::<String>::new());
    // My Day's suggestions are narrowed; My Day itself isn't.
    let suggested = env.json(&["myday", "suggest"]);
    assert_eq!(
        sorted_titles(&suggested["items"]),
        ["Pay the invoice", "Ship the release"]
    );
    assert_eq!(suggested["context"]["name"], "work");

    // Tables end with one line naming the context.
    let printed = table(&env, &["tasks", "list"]);
    assert_eq!(printed.lines().last(), Some("context: work (2 lists)"));
    assert!(printed.contains("Contentful"), "{printed}");

    // `--context` reads one command in another context.
    let home = env.json(&["--context", "home", "waiting"]);
    assert_eq!(sorted_titles(&home["items"]), ["Mow the lawn"]);
    let everything = env.json(&["--context", "none", "next", "--limit", "10"]);
    assert_eq!(everything["items"].as_array().map(Vec::len), Some(5));
    assert_eq!(everything["context"], Value::Null);
    let error = env.failure(&["--context", "play", "next"], 3);
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("the contexts are home, work")),
        "{error}"
    );

    // An undefined --context is refused on any command, not ignored.
    env.failure(&["--context", "play", "lists", "list"], 3);

    // A bulk change by filter picks from the context's lists too, as
    // `tasks list` shows them; an explicit --folder still wins.
    let bulk = ["reschedule", "--due-before", "tomorrow", "--to", "+7d"];
    let titles_of = |plan: &Value| {
        let mut titles: Vec<String> = plan["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .map(|target| target["title"].as_str().unwrap_or_default().to_owned())
            .collect();
        titles.sort();
        titles
    };
    let plan = env.json(&[&bulk[..], &["--dry-run"]].concat());
    assert_eq!(titles_of(&plan), ["Pay the invoice", "Ship the release"]);
    let plan = env.json(&[&bulk[..], &["--folder", "Home", "--dry-run"]].concat());
    assert_eq!(titles_of(&plan), ["Mow the lawn"]);
    env.json(&[&bulk[..], &["--yes"]].concat());
    let garden = env.json(&["tasks", "list", "--list", "Garden"]);
    let today = env.today().format("%Y-%m-%d").to_string();
    assert!(
        garden["items"][0]["dueDateTime"]["dateTime"]
            .as_str()
            .is_some_and(|due| due.starts_with(&today)),
        "a task outside the context kept its due date: {garden}"
    );

    // A task with no list goes to the context's default list.
    let plan = env.json(&["tasks", "add", "Write the notes", "--dry-run"]);
    assert_eq!(plan["list"]["name"], "Contentful");
    let plan = env.json(&["--context", "none", "tasks", "add", "Write it", "--dry-run"]);
    assert_eq!(plan["list"]["name"], "Tasks");

    let doctor = env.json(&["doctor"]);
    assert_eq!(doctor["contexts"]["active"], "work");
    assert!(
        doctor["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .any(|problem| problem.as_str().is_some_and(|text| text.contains("Nope"))),
        "{doctor}"
    );

    // The daemon keeps it across a restart.
    env.json(&["daemon", "stop"]);
    assert_eq!(env.json(&["ctx"])["active"], "work");

    let cleared = env.json(&["ctx", "none"]);
    assert_eq!(cleared["active"], Value::Null);
    let tasks = env.json(&["tasks", "list"]);
    assert_eq!(sorted_titles(&tasks["items"]), ["Buy milk"]);
    assert_eq!(tasks["context"], Value::Null);
    assert_eq!(
        env.json(&["next", "--limit", "10"])["items"]
            .as_array()
            .map(Vec::len),
        Some(5)
    );
    let printed = table(&env, &["tasks", "list"]);
    assert!(!printed.contains("context:"), "{printed}");

    let unknown = env.failure(&["ctx", "play"], 3);
    assert!(
        unknown["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("no context is named \"play\"")),
        "{unknown}"
    );
}

#[tokio::test]
async fn a_bad_config_is_a_warning_not_a_crash() {
    let (env, _graph) = setup().await;
    env.json(&["ctx", "work"]);
    write_config(&env, "[contexts.work\n");
    // The active context is no longer defined: nothing is narrowed.
    let tasks = env.json(&["tasks", "list"]);
    assert_eq!(tasks["context"], Value::Null);
    let shown = env.json(&["ctx", "show"]);
    let problems = shown["problems"].to_string();
    assert!(problems.contains("config.toml"), "{shown}");
    assert!(
        problems.contains("isn't in config.toml any more"),
        "{shown}"
    );
    let doctor = env.json(&["doctor"]);
    assert!(
        doctor["problems"].to_string().contains("contexts:"),
        "{doctor}"
    );
}

/// One request on its own connection to the daemon, as the TUI sends it.
async fn ask(env: &Env, request: Request) -> ResponseData {
    let stream = UnixStream::connect(env.socket()).await.expect("connect");
    let mut framed = Framed::new(stream, Codec::new());
    framed
        .send(Message {
            id: 1,
            payload: Payload::Request(request),
        })
        .await
        .expect("send");
    let message = tokio::time::timeout(Duration::from_secs(10), framed.next())
        .await
        .expect("an answer within 10 seconds")
        .expect("open")
        .expect("readable");
    match message.payload {
        Payload::Response(Response::Ok { data }) => data,
        other => unreachable!("{other:?}"),
    }
}

async fn seed(env: &Env, scope: Option<Scope>) -> Seed {
    let request = Request::Seed {
        scope,
        search: None,
        include_deferred: false,
        semantic: false,
    };
    match ask(env, request).await {
        ResponseData::Seed(seed) => *seed,
        other => unreachable!("{other:?}"),
    }
}

fn names(entities: &[ms_todo_protocol::Entity]) -> Vec<String> {
    let mut names: Vec<String> = entities
        .iter()
        .map(|entity| {
            entity
                .get("displayName")
                .or_else(|| entity.get("title"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn the_tuis_seed_shows_only_the_contexts_lists_and_counts_them() {
    let (env, _graph) = setup().await;
    let everything = seed(&env, Some(Scope::All)).await;
    assert_eq!(everything.counts.all, 5);
    assert_eq!(everything.contexts, ["home", "work"]);
    assert_eq!(everything.context, None);

    // A subscriber hears that its view changed.
    let stream = UnixStream::connect(env.socket()).await.expect("connect");
    let mut subscriber = Framed::new(stream, Codec::new());
    subscriber
        .send(Message {
            id: 1,
            payload: Payload::Request(Request::Subscribe),
        })
        .await
        .expect("subscribe");
    env.json(&["ctx", "work"]);
    let heard = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(message)) = subscriber.next().await {
            if message.payload == Payload::Event(Event::ResyncNeeded) {
                return true;
            }
        }
        false
    })
    .await
    .expect("within 10 seconds");
    assert!(heard, "no ResyncNeeded after `ctx work`");

    let all = seed(&env, Some(Scope::All)).await;
    assert_eq!(names(&all.lists), ["Contentful", "Money"]);
    assert_eq!(names(&all.tasks), ["Pay the invoice", "Ship the release"]);
    assert_eq!(all.counts.all, 2);
    assert_eq!(all.counts.planned, 2);
    assert_eq!(all.counts.lists.len(), 2);
    assert_eq!(all.context.as_ref().map(|context| context.lists), Some(2));
    // The default scope is the context's default list.
    let home = seed(&env, None).await;
    let Some(Scope::List { id }) = &home.scope else {
        unreachable!("{:?}", home.scope);
    };
    let contentful = home
        .lists
        .iter()
        .find(|list| list["displayName"] == "Contentful");
    assert_eq!(
        contentful.and_then(|list| list["id"].as_str()),
        Some(id.as_str())
    );
    assert_eq!(names(&home.tasks), ["Ship the release"]);
}

#[tokio::test]
async fn a_context_with_no_lists_shows_an_empty_view_not_tasks() {
    let (env, _graph) = setup().await;
    write_config(&env, &format!("{CONFIG}\n[contexts.empty]\n"));
    let shown = env.json(&["ctx", "empty"]);
    assert_eq!(shown["lists"], json!([]));
    assert!(
        shown["problems"]
            .to_string()
            .contains("names no folders or lists"),
        "{shown}"
    );
    let home = seed(&env, None).await;
    assert_eq!(home.scope, Some(Scope::All));
    assert!(home.tasks.is_empty(), "{:?}", home.tasks);
    assert!(home.lists.is_empty());
    assert_eq!(home.context.as_ref().map(|context| context.lists), Some(0));
}

/// Issue 006: an `--idempotency-key` covers the context a change was
/// resolved in, the active one too, so after a `ctx` switch or an edit of
/// the context's lists the same key is a different request (exit 2), not
/// a replay of a task added elsewhere.
#[tokio::test]
async fn an_idempotency_key_covers_the_active_context_and_its_lists() {
    let (env, _graph) = setup().await;
    env.json(&["ctx", "work"]);
    let add = ["tasks", "add", "Call Ada", "--idempotency-key", "k-ctx"];
    let first = env.json(&add);
    assert_eq!(env.json(&add), first, "the same context replays");

    env.json(&["ctx", "home"]);
    env.failure(&add, 2);

    // Back in work, but config.toml swapped one of its lists.
    env.json(&["ctx", "work"]);
    write_config(
        &env,
        &CONFIG.replace("\"Contentful\", \"Nope\"", "\"Contentful\", \"Books\""),
    );
    env.failure(&add, 2);
}

/// Issue 006: `ctx list --format ids` prints each context's name, and
/// `ctx` with none active still shows what doesn't resolve.
#[tokio::test]
async fn ctx_names_its_contexts_as_ids_and_warns_with_none_active() {
    let (env, _graph) = setup().await;
    let output = env
        .cmd()
        .args(["--format", "ids", "ctx", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let mut ids: Vec<String> = String::from_utf8(output)
        .expect("utf8")
        .lines()
        .map(str::to_owned)
        .collect();
    ids.sort();
    assert_eq!(ids, ["home", "work"]);

    let shown = env.json(&["ctx"]);
    assert_eq!(shown["active"], Value::Null);
    let problems = shown["problems"].to_string();
    assert!(problems.contains("Nope"), "{shown}");
}
