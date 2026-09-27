//! D-067's attachments through the real binary and daemon, against a fake
//! Graph: a file from stdin or a URL (kept by the daemon until it's
//! uploaded, and https only), an upload session taken up again after the
//! daemon stops mid-way, or started afresh when it can't be, and the
//! `UploadProgress` events a subscriber hears.

mod support;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ms_todo_protocol::{Codec, Event, Message, Payload, Request};
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Lets the daemon of a debug build download from this machine over http,
/// as the tests' server is.
const LOOPBACK_HTTP: &str = "MS_TODO_DOWNLOAD_LOOPBACK_HTTP";
const CHUNKS: &str =
    r"^/v1\.0/users/[^/]+/todo/lists/L-tasks/tasks/T1/attachmentSessions/[^/]+/content$";
const SLOW: Duration = Duration::from_secs(30);

/// Graph with "File taxes" in the default list, and a daemon that may
/// download from this machine.
async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks
            .insert("L-tasks".into(), vec![task("T1", "File taxes", "W/\"e1\"")]);
    });
    graph.accept_attachments().await;
    env.cmd()
        .env(LOOPBACK_HTTP, "1")
        .args(["daemon", "start"])
        .assert()
        .success();
    env.synced();
    graph
}

fn taxes(env: &Env) -> String {
    env.local_id(&["tasks", "list"], "T1")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bytes(len: usize) -> Vec<u8> {
    (0..len)
        .map(|at| u8::try_from(at * 31 % 251).unwrap_or(0))
        .collect()
}

fn op_id(answer: &Value) -> String {
    answer["op_id"].as_str().expect("op_id").to_owned()
}

/// Requests to Graph whose path ends with `end`.
async fn count(graph: &FakeGraph, verb: &str, end: &str) -> usize {
    graph
        .requests(verb)
        .await
        .iter()
        .filter(|request| request.url.path().ends_with(end))
        .count()
}

/// The daemon's staged copies.
fn staged(env: &Env) -> Vec<std::path::PathBuf> {
    let data = env.home.path().join("data");
    let found: Vec<_> = walk(&data)
        .into_iter()
        .filter(|path| {
            path.parent()
                .is_some_and(|parent| parent.ends_with("attachments-staged"))
        })
        .collect();
    found
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

#[tokio::test]
async fn stdin_is_attached_under_the_name_given() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);

    let refused = env
        .cmd()
        .args(["--format", "json", "attachments", "add", &task, "-"])
        .write_stdin("no name")
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(
        String::from_utf8_lossy(&refused).contains("--name"),
        "{}",
        String::from_utf8_lossy(&refused)
    );

    let output = env
        .cmd()
        .args([
            "--format",
            "json",
            "attachments",
            "add",
            &task,
            "-",
            "--name",
            "notes.txt",
        ])
        .write_stdin("from a pipe\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let added: Value = serde_json::from_slice(&output).expect("json");
    env.op_in_state(&op_id(&added), "done");
    assert_eq!(
        graph.attachments_of("T1"),
        [("notes.txt".to_owned(), b"from a pipe\n".to_vec())]
    );
    let posted = graph.requests("POST").await;
    let sent: Value = serde_json::from_slice(&posted.last().expect("a POST").body).expect("json");
    assert_eq!(sent["contentType"], "text/plain");
    assert!(
        staged(&env).is_empty(),
        "the daemon's copy goes once it's sent"
    );
}

#[tokio::test]
async fn a_url_is_downloaded_by_the_daemon_and_attached() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let files = MockServer::start().await;
    let report = bytes(5000);
    Mock::given(method("GET"))
        .and(path("/files/Q3%20report.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(report.clone()))
        .mount(&files)
        .await;
    Mock::given(method("GET"))
        .and(path("/away"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://example.com/file.pdf"),
        )
        .mount(&files)
        .await;
    let url = format!("{}/files/Q3%20report.pdf", files.uri());

    let plan = env.json(&["attachments", "add", &task, &url, "--dry-run"]);
    assert_eq!(plan["changes"][0]["body"]["name"], "Q3 report.pdf");
    assert_eq!(plan["changes"][0]["url"], url.as_str());
    assert!(
        files
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a dry run downloads nothing"
    );

    let added = env.json(&["attachments", "add", &task, &url]);
    env.op_in_state(&op_id(&added), "done");
    assert_eq!(
        graph.attachments_of("T1"),
        [("Q3 report.pdf".to_owned(), report.clone())]
    );

    // --name wins over the URL's.
    let named = env.json(&["attachments", "add", &task, &url, "--name", "q3.pdf"]);
    env.op_in_state(&op_id(&named), "done");
    assert_eq!(graph.attachments_of("T1")[1].0, "q3.pdf");

    // Not https, or a redirect away from it, is refused and nothing queued.
    let plain = env.failure(
        &["attachments", "add", &task, "http://example.com/a.pdf"],
        2,
    );
    assert!(
        plain["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("only https")),
        "{plain}"
    );
    let away = env.failure(
        &[
            "attachments",
            "add",
            &task,
            &format!("{}/away", files.uri()),
        ],
        2,
    );
    assert!(
        away["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("isn't https")),
        "{away}"
    );
    // Nor to this machine by another name: the typed host was 127.0.0.1,
    // and `localhost` resolves to it.
    let port = files.address().port();
    Mock::given(method("GET"))
        .and(path("/inward"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            format!("http://localhost:{port}/files/Q3%20report.pdf").as_str(),
        ))
        .mount(&files)
        .await;
    let inward = env.failure(
        &[
            "attachments",
            "add",
            &task,
            &format!("{}/inward", files.uri()),
        ],
        2,
    );
    assert!(
        inward["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("this machine or its network")),
        "{inward}"
    );
    // A URL goes on its own.
    env.cmd()
        .args(["attachments", "add", &task, &url, "/tmp/other.pdf"])
        .assert()
        .code(2);
    assert_eq!(graph.attachments_of("T1").len(), 2);
}

#[tokio::test]
async fn a_download_over_25_mb_is_refused() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let files = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0_u8; 25 * 1024 * 1024 + 1]))
        .mount(&files)
        .await;
    let refused = env.failure(
        &[
            "attachments",
            "add",
            &task,
            &format!("{}/big.bin", files.uri()),
        ],
        2,
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("25 MB")),
        "{refused}"
    );
    assert!(graph.attachments_of("T1").is_empty());
    assert!(staged(&env).is_empty(), "nothing kept");
}

/// Add a 7 MB file (three chunks) in `dir` whose second chunk hangs, and
/// stop the daemon while it does.
async fn stopped_mid_upload(
    env: &Env,
    graph: &FakeGraph,
    dir: &std::path::Path,
    content: &[u8],
) -> String {
    // The first chunk is taken as usual; the second never answers.
    graph
        .stall(
            "PUT",
            CHUNKS,
            Some(support::fake_moves::put_chunk),
            Duration::ZERO,
        )
        .await;
    graph.stall("PUT", CHUNKS, None, SLOW).await;
    let path = dir.join("scan.bin");
    std::fs::write(&path, content).expect("write");
    let added = env.json(&["attachments", "add", &taxes(env), &path.to_string_lossy()]);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while count(graph, "PUT", "/content").await < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the upload never got going"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    env.json(&["daemon", "stop"]);
    op_id(&added)
}

#[tokio::test]
async fn an_upload_goes_on_in_its_session_after_the_daemon_stops() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let content = bytes(7 * 1024 * 1024);
    let dir = tempfile::tempdir().expect("tempdir");
    let op = stopped_mid_upload(&env, &graph, dir.path(), &content).await;

    // Started again, it isn't unknown: it takes up the same session.
    let done = env.op_in_state(&op, "done");
    assert_eq!(done["attempts"], 2);
    assert_eq!(count(&graph, "POST", "/createUploadSession").await, 1);
    let held = graph.attachments_of("T1");
    assert_eq!(held.len(), 1, "attached once");
    assert_eq!(sha256(&held[0].1), sha256(&content));
}

#[tokio::test]
async fn a_session_that_cannot_be_taken_up_starts_afresh_and_attaches_once() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let content = bytes(7 * 1024 * 1024);
    let dir = tempfile::tempdir().expect("tempdir");
    let op = stopped_mid_upload(&env, &graph, dir.path(), &content).await;
    // The session expired meanwhile.
    graph.expire_upload_sessions();

    env.op_in_state(&op, "done");
    assert_eq!(count(&graph, "POST", "/createUploadSession").await, 2);
    let held = graph.attachments_of("T1");
    assert_eq!(held.len(), 1, "attached once");
    assert_eq!(sha256(&held[0].1), sha256(&content));
}

#[tokio::test]
async fn a_stop_while_the_final_part_is_on_its_way_is_unknown_and_never_resent() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let content = bytes(7 * 1024 * 1024);
    let dir = tempfile::tempdir().expect("tempdir");
    // Two chunks go through; the third, which commits it, lands and hangs.
    for _ in 0..2 {
        graph
            .stall(
                "PUT",
                CHUNKS,
                Some(support::fake_moves::put_chunk),
                Duration::ZERO,
            )
            .await;
    }
    graph
        .stall("PUT", CHUNKS, Some(support::fake_moves::put_chunk), SLOW)
        .await;
    let path = dir.path().join("scan.bin");
    std::fs::write(&path, &content).expect("write");
    let added = env.json(&["attachments", "add", &taxes(&env), &path.to_string_lossy()]);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while graph.attachments_of("T1").is_empty() {
        assert!(std::time::Instant::now() < deadline, "never committed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    env.json(&["daemon", "stop"]);
    let op = env.op_in_state(&op_id(&added), "unknown");
    assert_eq!(op["flagged"], true);
    assert_eq!(
        count(&graph, "PUT", "/content").await,
        3,
        "never sent again"
    );
    assert_eq!(graph.attachments_of("T1").len(), 1);
}

#[tokio::test]
async fn a_subscriber_hears_an_upload_s_progress() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let stream = UnixStream::connect(env.socket()).await.expect("connect");
    let mut framed = Framed::new(stream, Codec::new());
    framed
        .send(Message {
            id: 1,
            payload: Payload::Request(Request::Subscribe),
        })
        .await
        .expect("subscribe");

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("scan.bin");
    let content = bytes(7 * 1024 * 1024);
    std::fs::write(&path, &content).expect("write");
    let added = env.json(&["attachments", "add", &task, &path.to_string_lossy()]);

    let mut heard = Vec::new();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(20), framed.next())
            .await
            .expect("an event in time")
            .expect("open")
            .expect("readable");
        if let Payload::Event(Event::UploadProgress(progress)) = message.payload {
            let done = progress.done;
            heard.push(progress);
            if done {
                break;
            }
        }
    }
    assert!(heard.iter().all(|progress| progress.op_id == op_id(&added)));
    assert!(heard.iter().all(|progress| progress.name == "scan.bin"));
    let sent: Vec<u64> = heard.iter().map(|progress| progress.sent).collect();
    let total = content.len() as u64;
    assert_eq!(sent.first(), Some(&0));
    assert!(sent.windows(2).all(|pair| pair[0] <= pair[1]), "{sent:?}");
    assert_eq!(sent.last(), Some(&total));
    assert!(sent.contains(&3_276_800), "{sent:?}");
    assert_eq!(graph.attachments_of("T1").len(), 1);
}
