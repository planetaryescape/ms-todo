//! Search against a real SQLite file: migration 0004's backfill, the
//! triggers that keep the index current, ranking, query syntax, and the
//! filters.

use ms_todo_store::{
    Cursor, Entity, Hydration, ListsPass, SearchHit, SeenTask, StatusFilter, Store, StoreError,
    TaskSearch, TasksPass, tasks_scope,
};
use serde_json::{Value, json};

fn entity(value: Value) -> Entity {
    value.as_object().cloned().expect("object")
}

fn task(id: &str, title: &str) -> Entity {
    entity(json!({
        "id": id, "title": title, "status": "notStarted", "importance": "normal",
        "@odata.etag": format!("W/\"{id}\""), "createdDateTime": "2026-09-24T10:00:00.0000000Z"
    }))
}

fn with_body(mut task: Entity, content: &str, content_type: &str) -> Entity {
    task.insert(
        "body".into(),
        json!({ "content": content, "contentType": content_type }),
    );
    task
}

fn completed(mut task: Entity) -> Entity {
    task.insert("status".into(), json!("completed"));
    task
}

async fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("open");
    (dir, store)
}

/// Lists L1 "Home" and L2 "Work", returning their local IDs.
async fn with_lists(store: &Store) -> (String, String) {
    let rev = store.local_rev().await.expect("rev");
    let list = |id: &str, name: &str| {
        entity(json!({ "id": id, "displayName": name, "wellknownListName": "none" }))
    };
    store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(list("L1", "Home"), None), (list("L2", "Work"), None)],
            cursor: whole(),
        })
        .await
        .expect("lists");
    let lists = store.lists().await.expect("lists");
    let id = |graph_id: &str| {
        lists
            .iter()
            .find(|list| list.graph_id.as_deref() == Some(graph_id))
            .expect("list")
            .local_id
            .clone()
    };
    (id("L1"), id("L2"))
}

fn whole() -> Cursor {
    Cursor {
        delta_link: "link".into(),
        replayed: false,
    }
}

/// Sync `tasks` into a list as Graph's whole list: any other task there is
/// tombstoned.
async fn sync(store: &Store, list_graph_id: &str, list_local_id: &str, tasks: Vec<Entity>) {
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(TasksPass {
            scope: tasks_scope(list_graph_id),
            list_local_id: list_local_id.to_owned(),
            rev,
            seen: tasks
                .into_iter()
                .map(|raw| SeenTask {
                    raw,
                    hydration: Hydration::Kept,
                })
                .collect(),
            gone: Vec::new(),
            failure: None,
            cursor: whole(),
        })
        .await
        .expect("apply");
}

async fn search(store: &Store, query: &str) -> Vec<SearchHit> {
    search_with(store, query, None, StatusFilter::All).await
}

async fn search_with(
    store: &Store,
    query: &str,
    list: Option<&str>,
    status: StatusFilter,
) -> Vec<SearchHit> {
    store
        .search_tasks(&TaskSearch {
            query,
            list_local_id: list,
            status,
            view: None,
            limit: None,
        })
        .await
        .expect("search")
}

async fn found(store: &Store, query: &str) -> Vec<String> {
    search(store, query)
        .await
        .into_iter()
        .map(|hit| hit.task.graph_id.expect("graph id"))
        .collect()
}

#[tokio::test]
async fn the_index_follows_inserts_edits_tombstones_and_their_reversal() {
    let (_dir, store) = open().await;
    let (home, _) = with_lists(&store).await;
    sync(&store, "L1", &home, vec![task("T1", "Renew car insurance")]).await;
    assert_eq!(found(&store, "insurance").await, ["T1"]);

    // The title changes on the phone.
    sync(&store, "L1", &home, vec![task("T1", "Renew car tax")]).await;
    assert!(found(&store, "insurance").await.is_empty());
    assert_eq!(found(&store, "tax").await, ["T1"]);

    // So does the body.
    let noted = with_body(
        task("T1", "Renew car tax"),
        "Ask about the discount",
        "text",
    );
    sync(&store, "L1", &home, vec![noted.clone()]).await;
    assert_eq!(found(&store, "discount").await, ["T1"]);
    let renoted = with_body(task("T1", "Renew car tax"), "Paid online", "text");
    sync(&store, "L1", &home, vec![renoted.clone()]).await;
    assert!(found(&store, "discount").await.is_empty());
    assert_eq!(found(&store, "online").await, ["T1"]);

    // Deleted on the phone: tombstoned, and gone from search.
    sync(&store, "L1", &home, Vec::new()).await;
    assert!(found(&store, "tax").await.is_empty());
    assert!(found(&store, "online").await.is_empty());

    // Seen again (an undo, or a whole read that finds it): back.
    sync(&store, "L1", &home, vec![renoted]).await;
    assert_eq!(found(&store, "tax").await, ["T1"]);

    // The daemon's own writes too.
    store
        .upsert_task_local(&home, &task("T2", "Book the dentist"), None)
        .await
        .expect("write");
    assert_eq!(found(&store, "dentist").await, ["T2"]);
    let t2 = store.task("T2").await.expect("read").expect("live");
    store
        .tombstone_task_local(&t2.local_id)
        .await
        .expect("delete");
    assert!(found(&store, "dentist").await.is_empty());
}

#[tokio::test]
async fn a_title_match_ranks_above_a_body_match() {
    let (_dir, store) = open().await;
    let (home, _) = with_lists(&store).await;
    sync(
        &store,
        "L1",
        &home,
        vec![
            with_body(
                task("T-body", "Call the broker"),
                "About the insurance renewal and nothing else at all",
                "text",
            ),
            task("T-title", "Insurance renewal"),
        ],
    )
    .await;
    let hits = search(&store, "insurance").await;
    let order: Vec<_> = hits
        .iter()
        .map(|hit| hit.task.graph_id.as_deref().expect("id"))
        .collect();
    assert_eq!(order, ["T-title", "T-body"]);
    assert_eq!(hits[0].snippet, "**Insurance** renewal");
    assert!(
        hits[1].snippet.contains("**insurance**"),
        "{}",
        hits[1].snippet
    );
    assert_eq!(hits[0].list_name, "Home");
}

#[tokio::test]
async fn prefix_phrase_and_boolean_queries_work() {
    let (_dir, store) = open().await;
    let (home, _) = with_lists(&store).await;
    sync(
        &store,
        "L1",
        &home,
        vec![
            task("T1", "Renew car insurance"),
            task("T2", "Insure the insured car"),
            task("T3", "Car wash"),
            task("T4", "Café visit"),
        ],
    )
    .await;
    let mut prefix = found(&store, "insur*").await;
    prefix.sort();
    assert_eq!(prefix, ["T1", "T2"]);
    assert_eq!(found(&store, "\"car insurance\"").await, ["T1"]);
    assert_eq!(found(&store, "car NOT insur*").await, ["T3"]);
    let mut either = found(&store, "wash OR renew").await;
    either.sort();
    assert_eq!(either, ["T1", "T3"]);
    // All words, in any order.
    assert_eq!(found(&store, "insurance car").await, ["T1"]);
    // Diacritics are folded both ways.
    assert_eq!(found(&store, "cafe").await, ["T4"]);
    assert_eq!(found(&store, "café").await, ["T4"]);
}

#[tokio::test]
async fn a_malformed_query_is_invalid_query_not_a_database_failure() {
    let (_dir, store) = open().await;
    with_lists(&store).await;
    for query in ["OR milk", "milk AND", "(milk", "\"milk", "*", "NOT"] {
        let result = store
            .search_tasks(&TaskSearch {
                query,
                list_local_id: None,
                status: StatusFilter::All,
                view: None,
                limit: None,
            })
            .await;
        assert!(
            matches!(result, Err(StoreError::InvalidQuery(_))),
            "{query:?}: {result:?}"
        );
    }
    // Punctuation inside a word is text, not syntax.
    for query in [
        "e-mail",
        "don't",
        "v1.2",
        "a:b",
        "c++",
        "^x",
        "NEAR(a b)",
        "milk (eggs OR bread)",
    ] {
        search(&store, query).await;
    }
}

#[tokio::test]
async fn html_bodies_are_searched_as_text() {
    let (_dir, store) = open().await;
    let (home, _) = with_lists(&store).await;
    sync(
        &store,
        "L1",
        &home,
        vec![with_body(
            task("T1", "Flagged email"),
            "<html><body><p>Your <b>policy</b> renews on <i>Friday</i></p></body></html>",
            "html",
        )],
    )
    .await;
    assert_eq!(found(&store, "policy renews").await, ["T1"]);
    assert!(found(&store, "html").await.is_empty(), "markup isn't text");
    let hit = &search(&store, "policy").await[0];
    assert!(!hit.snippet.contains('<'), "{}", hit.snippet);
}

#[tokio::test]
async fn list_status_and_limit_filters() {
    let (_dir, store) = open().await;
    let (home, work) = with_lists(&store).await;
    sync(
        &store,
        "L1",
        &home,
        vec![task("H1", "Pay rent"), completed(task("H2", "Pay gas"))],
    )
    .await;
    sync(&store, "L2", &work, vec![task("W1", "Pay invoice")]).await;

    let ids = |hits: Vec<SearchHit>| -> Vec<String> {
        let mut ids: Vec<String> = hits
            .into_iter()
            .map(|hit| hit.task.graph_id.expect("id"))
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        ids(search_with(&store, "pay", None, StatusFilter::Open).await),
        ["H1", "W1"]
    );
    assert_eq!(
        ids(search_with(&store, "pay", None, StatusFilter::Completed).await),
        ["H2"]
    );
    assert_eq!(
        ids(search_with(&store, "pay", Some(&home), StatusFilter::All).await),
        ["H1", "H2"]
    );
    let limited = store
        .search_tasks(&TaskSearch {
            query: "pay",
            list_local_id: None,
            status: StatusFilter::All,
            view: None,
            limit: Some(2),
        })
        .await
        .expect("search");
    assert_eq!(limited.len(), 2);
}

/// A database as rung 4 left it (migrations 0001–0003), with tasks in it.
async fn rung_4_database(path: &std::path::Path) {
    let migrations = tempfile::tempdir().expect("tempdir");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for name in [
        "0001_lists_tasks_sync_state.sql",
        "0002_delta_links.sql",
        "0003_outbox.sql",
    ] {
        std::fs::copy(source.join(name), migrations.path().join(name)).expect("copy");
    }
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options)
        .await
        .expect("connect");
    sqlx::migrate::Migrator::new(migrations.path())
        .await
        .expect("migrator")
        .run(&pool)
        .await
        .expect("migrate to 0003");
    sqlx::query(
        "INSERT INTO lists (local_id, graph_id, display_name, raw_json) \
         VALUES ('l1', 'L1', 'Home', '{}')",
    )
    .execute(&pool)
    .await
    .expect("list");
    let rows = [
        ("t1", "Renew insurance", None, None, None),
        (
            "t2",
            "Call Sam",
            Some("about the boiler"),
            Some("text"),
            None,
        ),
        (
            "t3",
            "Flagged email",
            Some("<p>the <b>mortgage</b> offer</p>"),
            Some("html"),
            None,
        ),
        ("t4", "Old insurance task", None, None, Some(1_i64)),
    ];
    for (id, title, body, body_type, deleted_at) in rows {
        sqlx::query(
            "INSERT INTO tasks (local_id, graph_id, list_local_id, title, body_content, \
             body_content_type, status, importance, raw_json, deleted_at) \
             VALUES (?, ?, 'l1', ?, ?, ?, 'notStarted', 'normal', ?, ?)",
        )
        .bind(id)
        .bind(id.to_uppercase())
        .bind(title)
        .bind(body)
        .bind(body_type)
        .bind(json!({ "id": id.to_uppercase(), "title": title }).to_string())
        .bind(deleted_at)
        .execute(&pool)
        .await
        .expect("task");
    }
    pool.close().await;
}

#[tokio::test]
async fn upgrading_indexes_the_tasks_already_cached() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ms-todo.db");
    rung_4_database(&path).await;

    let store = Store::open(&path).await.expect("upgrade");
    assert_eq!(
        found(&store, "insurance").await,
        ["T1"],
        "not the tombstone"
    );
    assert_eq!(found(&store, "boiler").await, ["T2"]);
    // An html body is rendered by the store on open, not by the migration.
    assert_eq!(found(&store, "mortgage").await, ["T3"]);
    drop(store);
    let store = Store::open(&path).await.expect("reopen");
    assert_eq!(found(&store, "mortgage offer").await, ["T3"]);
}
