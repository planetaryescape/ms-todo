//! The store against a real SQLite file: migrations, upserts, tombstones,
//! the checkpoint, and idempotency keys.

use ms_todo_store::{
    Claim, Cursor, Entity, Hydration, LISTS_SCOPE, ListsPass, SeenTask, Store, TasksPass,
    tasks_scope,
};
use serde_json::{Value, json};

fn entity(value: Value) -> Entity {
    value.as_object().cloned().expect("object")
}

fn list(id: &str, name: &str) -> Entity {
    entity(
        json!({ "id": id, "displayName": name, "wellknownListName": "none", "@odata.etag": "W/\"l\"" }),
    )
}

fn task(id: &str, title: &str, etag: &str) -> Entity {
    entity(json!({
        "id": id, "title": title, "status": "notStarted", "importance": "normal",
        "@odata.etag": etag, "createdDateTime": "2026-09-24T10:00:00.0000000Z"
    }))
}

async fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("open");
    (dir, store)
}

/// A store holding list "L1" (Groceries), returning its local ID.
async fn with_list(store: &Store) -> String {
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(list("L1", "Groceries"), None)],
            cursor: whole(),
        })
        .await
        .expect("apply lists");
    store.lists().await.expect("lists")[0].local_id.clone()
}

fn pass(list_local_id: &str, rev: i64, seen: Vec<SeenTask>) -> TasksPass {
    TasksPass {
        scope: tasks_scope("L1"),
        list_local_id: list_local_id.to_owned(),
        rev,
        seen,
        gone: Vec::new(),
        failure: None,
        cursor: whole(),
    }
}

/// The cursor of a whole read.
fn whole() -> Cursor {
    Cursor {
        delta_link: "link-0".into(),
        replayed: false,
    }
}

fn seen(raw: Entity) -> SeenTask {
    SeenTask {
        raw,
        hydration: Hydration::Kept,
    }
}

#[tokio::test]
async fn migrations_run_once_and_the_database_is_in_wal_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("ms-todo.db");
    let store = Store::open(&path).await.expect("first open");
    drop(store);
    let store = Store::open(&path).await.expect("reopen");
    assert!(store.size_bytes() > 0);
    assert!(
        dir.path().join("nested").join("ms-todo.db-wal").exists(),
        "WAL mode keeps a -wal file"
    );
    assert!(store.scopes().await.expect("scopes").is_empty());
}

fn mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

#[tokio::test]
async fn the_database_and_its_wal_files_are_private_and_repaired_on_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fresh").join("ms-todo.db");
    let store = Store::open(&path).await.expect("open");
    store.lists().await.expect("read");
    assert_eq!(mode(path.parent().expect("dir")), 0o700, "a new directory");
    for suffix in ["", "-wal", "-shm"] {
        let file = dir.path().join("fresh").join(format!("ms-todo.db{suffix}"));
        assert_eq!(mode(&file), 0o600, "{}", file.display());
    }
    drop(store);

    // A database an older build left world-readable is repaired.
    use std::os::unix::fs::PermissionsExt;
    for suffix in ["", "-wal", "-shm"] {
        let file = dir.path().join("fresh").join(format!("ms-todo.db{suffix}"));
        if file.exists() {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }
    }
    let _store = Store::open(&path).await.expect("reopen");
    for suffix in ["", "-wal", "-shm"] {
        let file = dir.path().join("fresh").join(format!("ms-todo.db{suffix}"));
        if file.exists() {
            assert_eq!(mode(&file), 0o600, "{}", file.display());
        }
    }
}

#[tokio::test]
async fn lists_are_upserted_with_stable_local_ids_and_unseen_ones_are_tombstoned() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let first = store.scope(LISTS_SCOPE).await.expect("scope").expect("row");
    assert_eq!(first.generation, 1);
    assert!(first.is_ready());

    // Renamed on the phone, and a second list appears.
    let rev = store.local_rev().await.expect("rev");
    let applied = store
        .apply_lists(ListsPass {
            rev,
            lists: vec![
                (list("L1", "Food"), Some(json!({ "folder": "Home" }))),
                (list("L2", "Work"), None),
            ],
            cursor: whole(),
        })
        .await
        .expect("apply");
    assert_eq!(applied.changed, 2);
    let lists = store.lists().await.expect("lists");
    assert_eq!(lists[0].local_id, groceries, "the local ID is stable");
    assert_eq!(lists[0].display_name, "Food");
    assert_eq!(lists[0].extension, Some(json!({ "folder": "Home" })));

    // Tasks in L1, then L1 disappears from Graph.
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(pass(&groceries, rev, vec![seen(task("T1", "Milk", "e1"))]))
        .await
        .expect("tasks");
    let applied = store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(list("L2", "Work"), None)],
            cursor: whole(),
        })
        .await
        .expect("apply");
    assert_eq!(applied.removed, ["L1"]);
    assert_eq!(store.lists().await.expect("lists").len(), 1);
    assert!(
        store
            .tasks_in_list(&groceries)
            .await
            .expect("tasks")
            .is_empty()
    );
    assert!(store.task("T1").await.expect("task").is_none());
    assert!(
        store
            .scope(&tasks_scope("L1"))
            .await
            .expect("scope")
            .is_none(),
        "a deleted list's tasks scope is dropped"
    );

    // A pass that changes nothing still moves the generation.
    let rev = store.local_rev().await.expect("rev");
    let applied = store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(list("L2", "Work"), None)],
            cursor: whole(),
        })
        .await
        .expect("apply");
    assert_eq!(applied.changed, 0);
    let scope = store.scope(LISTS_SCOPE).await.expect("scope").expect("row");
    assert_eq!(scope.generation, 4);
    assert_eq!(scope.last_changed_count, 0);
}

#[tokio::test]
async fn tasks_are_upserted_and_what_wasnt_seen_or_is_gone_is_tombstoned() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    let changed = store
        .apply_tasks(pass(
            &groceries,
            rev,
            vec![
                seen(task("T1", "Milk", "e1")),
                seen(task("T2", "Eggs", "e1")),
                seen(task("T3", "Bread", "e1")),
            ],
        ))
        .await
        .expect("apply");
    assert_eq!(changed, 3);
    let t1 = store.task("T1").await.expect("read").expect("T1");
    assert_eq!(
        store.task(&t1.local_id).await.expect("read"),
        Some(t1.clone())
    );

    // T2 wasn't seen; T3's extension fetch said 404; T1 changed.
    let mut pass2 = pass(
        &groceries,
        rev,
        vec![
            seen(task("T1", "Oat milk", "e2")),
            seen(task("T3", "Bread", "e1")),
        ],
    );
    pass2.gone = vec!["T3".into()];
    let changed = store.apply_tasks(pass2).await.expect("apply");
    assert_eq!(changed, 3);
    let live = store.tasks_in_list(&groceries).await.expect("tasks");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].local_id, t1.local_id, "the local ID is stable");
    assert_eq!(live[0].title, "Oat milk");
    let scope = store
        .scope(&tasks_scope("L1"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(scope.generation, 2);
}

#[tokio::test]
async fn a_row_written_locally_during_a_pass_is_neither_overwritten_nor_tombstoned() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(pass(&groceries, rev, vec![seen(task("T1", "Milk", "e1"))]))
        .await
        .expect("apply");

    // A pass starts and fetches, then the daemon adds T2 and edits T1
    // before the pass applies what it fetched.
    let pass_rev = store.local_rev().await.expect("rev");
    let added = store
        .upsert_task_local(
            &groceries,
            &task("T2", "Eggs", "e1"),
            Some(Some(json!({ "opId": "op-1" }))),
        )
        .await
        .expect("add");
    store
        .upsert_task_local(&groceries, &task("T1", "Oat milk", "e2"), None)
        .await
        .expect("edit");
    store
        .apply_tasks(pass(
            &groceries,
            pass_rev,
            vec![seen(task("T1", "Milk", "e1"))],
        ))
        .await
        .expect("stale pass");

    let t2 = store.task("T2").await.expect("read").expect("T2 is kept");
    assert_eq!(t2.local_id, added.local_id);
    assert_eq!(t2.extension, Some(json!({ "opId": "op-1" })));
    let t1 = store.task("T1").await.expect("read").expect("T1");
    assert_eq!(t1.title, "Oat milk", "the stale page didn't undo the edit");

    // The next pass, which starts after the writes, applies normally.
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(pass(
            &groceries,
            rev,
            vec![seen(task("T1", "Oat milk", "e2"))],
        ))
        .await
        .expect("apply");
    assert!(store.task("T2").await.expect("read").is_none());
}

#[tokio::test]
async fn a_local_delete_during_a_pass_is_not_undone() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(pass(&groceries, rev, vec![seen(task("T1", "Milk", "e1"))]))
        .await
        .expect("apply");
    let pass_rev = store.local_rev().await.expect("rev");
    let t1 = store.task("T1").await.expect("read").expect("T1");
    store
        .tombstone_task_local(&t1.local_id)
        .await
        .expect("delete");
    store
        .apply_tasks(pass(
            &groceries,
            pass_rev,
            vec![seen(task("T1", "Milk", "e1"))],
        ))
        .await
        .expect("stale pass");
    assert!(store.task("T1").await.expect("read").is_none());
}

#[tokio::test]
async fn a_failed_fetch_applies_what_came_back_but_does_not_checkpoint() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    let mut failed = pass(&groceries, rev, vec![seen(task("T1", "Milk", "e1"))]);
    failed.failure = Some(("network".into(), "timed out".into()));
    store.apply_tasks(failed).await.expect("apply");

    let scope = store
        .scope(&tasks_scope("L1"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(scope.generation, 0, "no checkpoint");
    assert!(!scope.is_ready());
    assert_eq!(scope.last_error_kind.as_deref(), Some("network"));
    assert!(store.task("T1").await.expect("read").is_some());

    store
        .apply_tasks(pass(&groceries, rev, Vec::new()))
        .await
        .expect("apply");
    let scope = store
        .scope(&tasks_scope("L1"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(scope.generation, 1);
    assert_eq!(scope.last_error, None, "a success clears the error");
}

#[tokio::test]
async fn a_task_needs_its_extension_fetched_until_it_is_fetched_at_its_etag() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let wanted = [("T1".to_owned(), Some("e1".to_owned()))];
    assert!(
        store
            .needing_hydration(&wanted)
            .await
            .expect("read")
            .contains("T1")
    );

    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(pass(
            &groceries,
            rev,
            vec![SeenTask {
                raw: task("T1", "Milk", "e1"),
                hydration: Hydration::Fetched(None),
            }],
        ))
        .await
        .expect("apply");
    assert!(
        store
            .needing_hydration(&wanted)
            .await
            .expect("read")
            .is_empty()
    );

    // Kept at a new etag: the extension may have changed.
    store
        .apply_tasks(pass(&groceries, rev, vec![seen(task("T1", "Milk", "e2"))]))
        .await
        .expect("apply");
    let moved = [("T1".to_owned(), Some("e2".to_owned()))];
    assert!(
        store
            .needing_hydration(&moved)
            .await
            .expect("read")
            .contains("T1")
    );
}

#[tokio::test]
async fn idempotency_keys_replay_refuse_a_different_request_and_can_be_released() {
    let (_dir, store) = open().await;
    assert_eq!(
        store.claim_key("k", "fp-a", "op-1").await.expect("claim"),
        Claim::Fresh
    );
    assert_eq!(
        store.claim_key("k", "fp-a", "op-2").await.expect("claim"),
        Claim::Running {
            op_id: "op-1".into()
        }
    );
    assert_eq!(
        store.claim_key("k", "fp-b", "op-2").await.expect("claim"),
        Claim::Mismatch
    );
    assert_eq!(
        store.unfinished_keys().await.expect("read"),
        [("k".into(), "op-1".into())]
    );

    store
        .finish_key("k", r#"{"done":true}"#)
        .await
        .expect("finish");
    assert_eq!(
        store.claim_key("k", "fp-a", "op-3").await.expect("claim"),
        Claim::Replay(r#"{"done":true}"#.into())
    );
    assert_eq!(
        store.claim_key("k", "fp-b", "op-3").await.expect("claim"),
        Claim::Mismatch
    );

    store.release_key("k").await.expect("release");
    assert_eq!(
        store.claim_key("k", "fp-b", "op-4").await.expect("claim"),
        Claim::Fresh
    );
}

fn delta(link: &str) -> Cursor {
    Cursor {
        delta_link: link.into(),
        replayed: true,
    }
}

#[tokio::test]
async fn a_delta_round_applies_only_what_it_names_and_saves_its_link() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    let mut whole = pass(
        &groceries,
        rev,
        vec![
            seen(task("T1", "Milk", "e1")),
            seen(task("T2", "Eggs", "e1")),
        ],
    );
    whole.cursor = Cursor {
        delta_link: "link-1".into(),
        replayed: false,
    };
    store.apply_tasks(whole).await.expect("whole read");
    let scope = store
        .scope(&tasks_scope("L1"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(scope.delta_link.as_deref(), Some("link-1"));
    assert!(scope.is_delta());
    assert_eq!(
        scope.last_delta_at, None,
        "a whole read isn't a delta round"
    );

    // Delta names T2 as removed and T3 as new; T1 isn't mentioned.
    let rev = store.local_rev().await.expect("rev");
    let mut round = pass(&groceries, rev, vec![seen(task("T3", "Bread", "e1"))]);
    round.gone = vec!["T2".into()];
    round.cursor = delta("link-2");
    let changed = store.apply_tasks(round).await.expect("delta round");
    assert_eq!(changed, 2);
    let mut titles: Vec<String> = store
        .tasks_in_list(&groceries)
        .await
        .expect("tasks")
        .into_iter()
        .map(|task| task.title)
        .collect();
    titles.sort();
    assert_eq!(
        titles,
        ["Bread", "Milk"],
        "T1 isn't tombstoned for not being named"
    );
    let scope = store
        .scope(&tasks_scope("L1"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(scope.delta_link.as_deref(), Some("link-2"));
    assert!(scope.last_delta_at.is_some());
}

#[tokio::test]
async fn delta_never_tombstones_a_row_written_locally_since_it_fetched() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(pass(&groceries, rev, vec![seen(task("T1", "Milk", "e1"))]))
        .await
        .expect("apply");

    let pass_rev = store.local_rev().await.expect("rev");
    store
        .upsert_task_local(&groceries, &task("T1", "Oat milk", "e2"), None)
        .await
        .expect("edit");
    let mut round = pass(&groceries, pass_rev, Vec::new());
    round.gone = vec!["T1".into()];
    round.cursor = delta("link");
    store.apply_tasks(round).await.expect("stale delta");

    let t1 = store.task("T1").await.expect("read").expect("T1 is kept");
    assert_eq!(t1.title, "Oat milk");
}

#[tokio::test]
async fn a_failed_delta_round_keeps_the_old_link() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    let mut whole = pass(&groceries, rev, Vec::new());
    whole.cursor = delta("link-1");
    store.apply_tasks(whole).await.expect("apply");

    let mut failed = pass(&groceries, rev, vec![seen(task("T1", "Milk", "e1"))]);
    failed.cursor = delta("link-2");
    failed.failure = Some(("rejected".into(), "403".into()));
    store.apply_tasks(failed).await.expect("apply");

    let scope = store
        .scope(&tasks_scope("L1"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(scope.delta_link.as_deref(), Some("link-1"));
    assert_eq!(scope.generation, 1);
}

#[tokio::test]
async fn a_reset_drops_the_link_and_advancing_saves_one() {
    let (_dir, store) = open().await;
    store
        .advance_scope(LISTS_SCOPE, &delta("link-1"))
        .await
        .expect("advance");
    let scope = store.scope(LISTS_SCOPE).await.expect("read").expect("row");
    assert_eq!(scope.delta_link.as_deref(), Some("link-1"));
    assert_eq!(scope.generation, 1);
    assert_eq!(scope.last_changed_count, 0);

    store.reset_scope(LISTS_SCOPE).await.expect("reset");
    let scope = store.scope(LISTS_SCOPE).await.expect("read").expect("row");
    assert!(!scope.is_delta());
    assert_eq!(scope.generation, 1, "the cached data and generation stay");
}

#[tokio::test]
async fn a_list_found_deleted_takes_its_tasks_and_scope_with_it() {
    let (_dir, store) = open().await;
    let groceries = with_list(&store).await;
    let rev = store.local_rev().await.expect("rev");
    let mut whole = pass(&groceries, rev, vec![seen(task("T1", "Milk", "e1"))]);
    whole.cursor = delta("link");
    store.apply_tasks(whole).await.expect("apply");

    assert!(store.remove_list("L1", rev).await.expect("remove"));
    assert!(store.lists().await.expect("lists").is_empty());
    assert!(store.task("T1").await.expect("read").is_none());
    assert!(
        store
            .scope(&tasks_scope("L1"))
            .await
            .expect("read")
            .is_none()
    );
    assert!(!store.remove_list("L1", rev).await.expect("again"));
}

#[tokio::test]
async fn migration_0002_applies_on_top_of_a_rung_3a_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ms-todo.db");
    let only_0001 = dir.path().join("migrations");
    std::fs::create_dir(&only_0001).expect("mkdir");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/migrations/0001_lists_tasks_sync_state.sql"
        ),
        only_0001.join("0001_lists_tasks_sync_state.sql"),
    )
    .expect("copy 0001");
    {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(options)
            .await
            .expect("connect");
        sqlx::migrate::Migrator::new(only_0001.as_path())
            .await
            .expect("migrator")
            .run(&pool)
            .await
            .expect("0001");
        sqlx::query(
            "INSERT INTO sync_state (scope, generation, last_success_at) VALUES ('lists', 7, 1)",
        )
        .execute(&pool)
        .await
        .expect("a rung 3a scope");
        pool.close().await;
    }

    let store = Store::open(&path).await.expect("open applies 0002");
    let scope = store.scope(LISTS_SCOPE).await.expect("read").expect("row");
    assert_eq!(scope.generation, 7);
    assert!(
        !scope.is_delta(),
        "a rung 3a scope starts in enumeration mode"
    );
}
