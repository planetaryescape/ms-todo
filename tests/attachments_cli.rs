//! Attachments (rung 8b, D-056) through the real binary and daemon,
//! against a fake Graph that behaves as S1, S14 and S16 found: a small file
//! in one POST, a big one through an upload session in chunks (resumed
//! after a lost answer, and `unknown` after a lost last one), the 25 MB
//! limit, safe downloads, delete and its undo from the kept copy, and a
//! phone's attachments arriving by sync.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::Env;
use support::fake_graph::{FakeGraph, list, task};
use support::fake_moves::put_chunk;
use wiremock::Request;

const CHUNKS: &str =
    r"^/v1\.0/users/[^/]+/todo/lists/L-tasks/tasks/T1/attachmentSessions/[^/]+/content$";
/// An upload session's chunk (S16: 10 × 320 KiB).
const CHUNK: usize = 3_276_800;

/// Graph with the default list holding "File taxes", which has no
/// attachments, and the fake answering attachment requests.
async fn graph(env: &mut Env) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks
            .insert("L-tasks".into(), vec![task("T1", "File taxes", "W/\"e1\"")]);
    });
    graph.accept_attachments().await;
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

/// Bytes that don't repeat within a chunk, so a chunk sent twice or out
/// of place changes the hash.
fn random_bytes(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn file(dir: &Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write");
    path.to_string_lossy().into_owned()
}

fn listed(env: &Env, task: &str) -> Vec<Value> {
    env.json(&["attachments", "list", task])["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn names(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

async fn requests(graph: &FakeGraph, verb: &str, path: &str) -> Vec<Request> {
    graph
        .requests(verb)
        .await
        .into_iter()
        .filter(|request| request.url.path().ends_with(path))
        .collect()
}

async fn session_puts(graph: &FakeGraph) -> Vec<Request> {
    requests(graph, "PUT", "/content").await
}

#[tokio::test]
async fn a_small_file_is_one_post_and_shows_at_once() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let dir = tempfile::tempdir().expect("tempdir");
    let invoice = file(dir.path(), "invoice.pdf", b"%PDF-1.7 invoice");

    let added = env.json(&["attachments", "add", &task, &invoice]);
    assert_eq!(added["action"], "attachment_add");
    let shown = &added["items"][0];
    assert_eq!(shown["sync_state"], "pending");
    assert_eq!(shown["hasAttachments"], true);
    assert_eq!(
        names(shown["attachments"].as_array().expect("attachments")),
        ["invoice.pdf"],
        "in the cache before Graph has it"
    );

    env.settled();
    let posts = requests(&graph, "POST", "/tasks/T1/attachments").await;
    assert_eq!(posts.len(), 1);
    let sent: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(sent["name"], "invoice.pdf");
    assert_eq!(sent["contentType"], "application/pdf");
    assert!(session_puts(&graph).await.is_empty(), "no upload session");
    assert_eq!(
        graph.attachments_of("T1"),
        [("invoice.pdf".to_owned(), b"%PDF-1.7 invoice".to_vec())]
    );
    let items = listed(&env, &task);
    assert_eq!(items[0]["index"], 1);
    assert!(
        items[0]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with('A')),
        "Graph's ID replaced the placeholder: {items:?}"
    );
    assert!(items[0].get("contentBytes").is_none(), "never the bytes");
    assert_eq!(
        env.json(&["tasks", "list"])["items"][0]["sync_state"],
        "synced"
    );
}

#[tokio::test]
async fn a_big_file_goes_through_an_upload_session_and_comes_back_whole() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = random_bytes(10 * 1024 * 1024 + 123);
    let big = file(dir.path(), "scan.bin", &bytes);

    env.json(&["attachments", "add", &task, &big]);
    env.settled();

    let puts = session_puts(&graph).await;
    assert_eq!(puts.len(), bytes.len().div_ceil(CHUNK), "one PUT per chunk");
    let ranges: Vec<String> = puts
        .iter()
        .map(|put| {
            put.headers
                .get("content-range")
                .and_then(|range| range.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(
        ranges[0],
        format!("bytes 0-{}/{}", CHUNK - 1, bytes.len()),
        "in order, from 0"
    );
    assert!(
        puts.iter()
            .all(|put| put.headers.get("authorization").is_some()),
        "the bytes go with the token (S14)"
    );
    let held = graph.attachments_of("T1");
    assert_eq!(sha256(&held[0].1), sha256(&bytes));

    let out = tempfile::tempdir().expect("tempdir");
    let downloaded = env.json(&[
        "attachments",
        "download",
        &task,
        "1",
        "--out",
        &out.path().to_string_lossy(),
    ]);
    let saved = &downloaded["files"][0];
    assert_eq!(saved["bytes"], bytes.len());
    assert_eq!(saved["sha256"], sha256(&bytes));
    let written = std::fs::read(saved["path"].as_str().expect("path")).expect("read");
    assert_eq!(sha256(&written), sha256(&bytes), "byte for byte");
}

#[tokio::test]
async fn a_chunk_whose_answer_was_lost_is_resent_and_the_upload_goes_on() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    // The first chunk lands, but its answer is a 503.
    graph.lose_answer("PUT", CHUNKS, put_chunk, 503).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = random_bytes(7 * 1024 * 1024);
    let big = file(dir.path(), "scan.bin", &bytes);

    let added = env.json(&["attachments", "add", &task, &big]);
    let op = env.op_in_state(added["op_id"].as_str().expect("op_id"), "done");
    assert_eq!(op["attempts"], 1, "within the one attempt");

    let puts = session_puts(&graph).await;
    // Three chunks, and the first sent again: Graph says it has it
    // (InvalidStart), so the upload went on from the next.
    assert_eq!(puts.len(), 4);
    assert_eq!(
        requests(&graph, "POST", "/createUploadSession").await.len(),
        1,
        "one session"
    );
    let held = graph.attachments_of("T1");
    assert_eq!(held.len(), 1);
    assert_eq!(sha256(&held[0].1), sha256(&bytes));
}

#[tokio::test]
async fn a_lost_answer_to_the_last_chunk_is_unknown_and_never_resent() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = random_bytes(5 * 1024 * 1024);
    let big = file(dir.path(), "scan.bin", &bytes);
    // The last chunk commits the file, and its answer is lost.
    let last = format!("bytes {CHUNK}-{}/{}", bytes.len() - 1, bytes.len());
    graph.lose_chunk_answer(&last, 504).await;

    let added = env.json(&["attachments", "add", &task, &big]);
    let op_id = added["op_id"].as_str().expect("op_id");
    let op = env.op_in_state(op_id, "unknown");
    assert_eq!(
        op["flagged"], true,
        "nothing can find an attachment but the user"
    );
    assert!(
        op["note"]
            .as_str()
            .unwrap_or_default()
            .contains("outbox retry"),
        "{op}"
    );
    env.synced();
    env.synced();
    assert_eq!(session_puts(&graph).await.len(), 2, "never sent again");
    assert_eq!(
        requests(&graph, "POST", "/createUploadSession").await.len(),
        1,
        "never uploaded again"
    );
    assert_eq!(graph.attachments_of("T1").len(), 1, "it landed");
    env.failure(&["undo"], 2);
}

#[tokio::test]
async fn over_25_mb_is_refused_before_any_request() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let dir = tempfile::tempdir().expect("tempdir");
    let big = dir.path().join("huge.bin");
    std::fs::File::create(&big)
        .expect("create")
        .set_len(25 * 1024 * 1024 + 1)
        .expect("size");

    let error = env.failure(&["attachments", "add", &task, &big.to_string_lossy()], 2);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("25 MB"),
        "{error}"
    );
    let missing = env.failure(
        &[
            "attachments",
            "add",
            &task,
            &dir.path().join("nope.pdf").to_string_lossy(),
        ],
        2,
    );
    assert_eq!(missing["error"]["kind"], "invalid_input");
    assert!(env.outbox().is_empty(), "nothing queued");
    assert!(graph.writes().await.is_empty(), "nothing sent");
}

#[tokio::test]
async fn a_relative_path_is_resolved_by_the_cli_and_a_dry_run_queues_nothing() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let dir = tempfile::tempdir().expect("tempdir");
    file(dir.path(), "notes.txt", b"hello");

    let output = env
        .cmd()
        .current_dir(dir.path())
        .args([
            "--format",
            "json",
            "attachments",
            "add",
            &task,
            "notes.txt",
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plan: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(plan["dry_run"], true);
    let path = plan["changes"][0]["file"]["path"].as_str().expect("path");
    assert!(Path::new(path).is_absolute(), "{path}");
    assert!(path.ends_with("notes.txt"));
    assert_eq!(plan["changes"][0]["body"]["contentType"], "text/plain");
    assert!(env.outbox().is_empty());
    assert!(graph.writes().await.is_empty());
}

#[tokio::test]
async fn downloads_are_safe_numbered_private_and_never_follow_a_link() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    // As a phone might name them: Graph keeps any name (S16).
    graph.attach("L-tasks", "T1", "../../evil.txt", b"evil");
    graph.attach("L-tasks", "T1", "receipt.pdf", b"receipt");
    env.synced();
    let task = taxes(&env);
    let out = tempfile::tempdir().expect("tempdir");
    let out_dir = out.path().to_string_lossy().into_owned();

    let first = env.json(&["attachments", "download", &task, "--out", &out_dir]);
    let paths: Vec<PathBuf> = first["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| PathBuf::from(file["path"].as_str().expect("path")))
        .collect();
    let real = out.path().canonicalize().expect("real");
    assert_eq!(
        paths,
        [real.join("_._evil.txt"), real.join("receipt.pdf")],
        "inside the directory, `..` neutralised"
    );
    for path in &paths {
        let mode = std::fs::metadata(path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{}", path.display());
    }
    assert_eq!(std::fs::read(&paths[1]).expect("read"), b"receipt");

    let again = env.json(&[
        "attachments",
        "download",
        &task,
        "receipt.pdf",
        "--out",
        &out_dir,
    ]);
    assert_eq!(
        again["files"][0]["path"],
        json!(real.join("receipt (1).pdf"))
    );
    let forced = env.json(&[
        "attachments",
        "download",
        &task,
        "2",
        "--out",
        &out_dir,
        "--force",
    ]);
    assert_eq!(forced["files"][0]["path"], json!(real.join("receipt.pdf")));
    let parts = std::fs::read_dir(&real)
        .expect("list")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
        .count();
    assert_eq!(parts, 0, "no .part left");

    let link = tempfile::tempdir().expect("tempdir");
    let linked = link.path().join("out");
    std::os::unix::fs::symlink(&real, &linked).expect("link");
    let refused = env.failure(
        &[
            "attachments",
            "download",
            &task,
            "--out",
            &linked.to_string_lossy(),
        ],
        2,
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("symlink"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_delete_is_undone_from_the_copy_ms_todo_kept() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let bytes = random_bytes(4096);
    graph.attach("L-tasks", "T1", "contract.pdf", &bytes);
    env.synced();
    let task = taxes(&env);

    env.failure(&["attachments", "delete", &task, "contract.pdf"], 2);
    let deleted = env.json(&["attachments", "delete", &task, "1", "--yes"]);
    assert_eq!(deleted["action"], "attachment_delete");
    assert_eq!(deleted["items"][0]["attachments"], json!([]));
    assert_eq!(deleted["items"][0]["hasAttachments"], false);
    env.settled();
    assert!(graph.attachments_of("T1").is_empty());
    let deletes = graph.requests("DELETE").await;
    assert_eq!(deletes.len(), 1);
    assert!(
        deletes[0].headers.get("if-match").is_none(),
        "Graph ignores it on an attachment (S16)"
    );

    let undone = env.json(&["undo"]);
    assert_eq!(undone["action"], "undo");
    env.settled();
    let held = graph.attachments_of("T1");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0, "contract.pdf");
    assert_eq!(sha256(&held[0].1), sha256(&bytes), "the same bytes");
    assert_eq!(names(&listed(&env, &task)), ["contract.pdf"]);

    // Undoing the undo deletes it again, keeping it again.
    env.json(&["undo", undone["op_id"].as_str().expect("op_id")]);
    env.settled();
    assert!(graph.attachments_of("T1").is_empty());
}

#[tokio::test]
async fn an_add_is_undone_by_deleting_it() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    let dir = tempfile::tempdir().expect("tempdir");
    let receipt = file(dir.path(), "receipt.txt", b"paid");

    env.json(&["attachments", "add", &task, &receipt]);
    env.settled();
    assert_eq!(graph.attachments_of("T1").len(), 1);
    env.json(&["undo"]);
    env.settled();
    assert!(graph.attachments_of("T1").is_empty());
    assert!(listed(&env, &task).is_empty());
}

#[tokio::test]
async fn a_phones_attachments_arrive_by_sync_and_leave_by_it() {
    let mut env = Env::new();
    let graph = graph(&mut env).await;
    let task = taxes(&env);
    assert!(listed(&env, &task).is_empty());

    graph.attach("L-tasks", "T1", "photo.jpg", b"jpeg");
    env.synced();
    let items = listed(&env, &task);
    assert_eq!(names(&items), ["photo.jpg"]);
    assert_eq!(items[0]["size"], 4 + 258, "Graph's size, not the bytes");

    graph.detach("L-tasks", "T1", "photo.jpg");
    env.synced();
    assert!(listed(&env, &task).is_empty());
    assert_eq!(
        env.json(&["tasks", "list"])["items"][0]["hasAttachments"],
        false
    );
}
