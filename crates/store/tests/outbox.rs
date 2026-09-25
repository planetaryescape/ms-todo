//! The outbox against a real SQLite file: what each outcome does to the
//! task row, atomically with the operation's state.

use ms_todo_store::{
    Cursor, Entity, Hydration, ListsPass, LocalChange, NewOp, OpKind, OpState, Restore, SeenTask,
    Store, StoreError, TasksPass, tasks_scope,
};
use serde_json::{Value, json};

fn entity(value: Value) -> Entity {
    value.as_object().cloned().expect("object")
}

fn task(id: &str, title: &str, etag: &str) -> Entity {
    entity(json!({
        "id": id, "title": title, "status": "notStarted", "importance": "normal",
        "@odata.etag": etag, "createdDateTime": "2026-09-24T10:00:00.0000000Z"
    }))
}

async fn open() -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("open");
    let rev = store.local_rev().await.expect("rev");
    let list = entity(json!({ "id": "L1", "displayName": "Groceries", "@odata.etag": "l" }));
    store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(list, None)],
            cursor: whole(),
        })
        .await
        .expect("lists");
    let list = store.lists().await.expect("lists")[0].local_id.clone();
    (dir, store, list)
}

fn whole() -> Cursor {
    Cursor {
        delta_link: "link".into(),
        replayed: false,
    }
}

fn create(op_id: &str, local_id: &str, list: &str, title: &str) -> NewOp {
    let body = json!({ "title": title, "extensions": [{ "opId": op_id }] });
    NewOp {
        op_id: op_id.into(),
        entity_local_id: local_id.into(),
        list_local_id: list.into(),
        op: OpKind::Create,
        action: "add".into(),
        payload: json!({ "body": body }),
        change: LocalChange::Insert {
            raw: entity(json!({ "title": title, "status": "notStarted" })),
            extension: Some(
                json!({ "extensionName": "com.planetaryescape.mstodo", "opId": op_id }),
            ),
        },
    }
}

fn edit(op_id: &str, local_id: &str, list: &str, title: &str) -> NewOp {
    NewOp {
        op_id: op_id.into(),
        entity_local_id: local_id.into(),
        list_local_id: list.into(),
        op: OpKind::Update,
        action: "edit".into(),
        payload: json!({ "body": { "title": title } }),
        change: LocalChange::Update,
    }
}

#[tokio::test]
async fn graphs_answer_keeps_later_queued_edits_on_top_and_merges_a_row_sync_made() {
    let (_dir, store, list) = open().await;
    store
        .enqueue("op-1", None, vec![create("op-1", "t1", &list, "Milk")])
        .await
        .expect("create");
    let rows = store
        .enqueue("op-2", None, vec![edit("op-2", "t1", &list, "Oat milk")])
        .await
        .expect("edit");
    assert_eq!(rows[0].sync_state, "pending");
    assert_eq!(
        store
            .outbox_op("op-2")
            .await
            .expect("read")
            .expect("op-2")
            .depends_on
            .as_deref(),
        Some("op-1")
    );
    // Sync got there first: Graph's task is cached under another local ID.
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(TasksPass {
            scope: tasks_scope("L1"),
            list_local_id: list.clone(),
            rev,
            seen: vec![SeenTask {
                raw: task("T1", "Milk", "e1"),
                hydration: Hydration::Kept,
            }],
            gone: Vec::new(),
            failure: None,
            cursor: whole(),
        })
        .await
        .expect("sync");

    store
        .record_sent("op-1", &task("T1", "Milk", "e1"), None, true)
        .await
        .expect("record");

    let live = store.tasks_in_list(&list).await.expect("tasks");
    assert_eq!(live.len(), 1, "merged into one row: {live:?}");
    assert_eq!(live[0].local_id, "t1");
    assert_eq!(live[0].graph_id.as_deref(), Some("T1"));
    assert_eq!(live[0].title, "Oat milk", "the queued edit stays on top");
    assert_eq!(live[0].sync_state, "pending");
    let done = store.outbox_op("op-1").await.expect("read").expect("op-1");
    assert_eq!(done.state, OpState::Done);
}

#[tokio::test]
async fn a_rejection_rolls_back_only_what_it_changed_and_restart_settles_inflight_ops() {
    let (_dir, store, list) = open().await;
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(TasksPass {
            scope: tasks_scope("L1"),
            list_local_id: list.clone(),
            rev,
            seen: vec![SeenTask {
                raw: task("T1", "Milk", "e1"),
                hydration: Hydration::Kept,
            }],
            gone: Vec::new(),
            failure: None,
            cursor: whole(),
        })
        .await
        .expect("sync");
    let t1 = store.task("T1").await.expect("read").expect("T1");
    store
        .enqueue(
            "op-1",
            None,
            vec![edit("op-1", &t1.local_id, &list, "Oat milk")],
        )
        .await
        .expect("edit");
    store
        .enqueue("op-2", None, vec![create("op-2", "t2", &list, "Eggs")])
        .await
        .expect("create");
    assert!(store.mark_inflight("op-1").await.expect("inflight"));
    assert!(store.mark_inflight("op-2").await.expect("inflight"));

    assert_eq!(store.recover_inflight().await.expect("recover"), 1);
    let op1 = store.outbox_op("op-1").await.expect("read").expect("op-1");
    let op2 = store.outbox_op("op-2").await.expect("read").expect("op-2");
    assert_eq!(op1.state, OpState::Pending, "a PATCH is safe to resend");
    assert_eq!(op2.state, OpState::Unknown, "a create may have happened");

    store
        .fail_op(
            "op-1",
            ("rejected", "no"),
            &Restore::Replace(t1.raw.clone()),
        )
        .await
        .expect("fail");
    let t1 = store.task("T1").await.expect("read").expect("T1");
    assert_eq!(t1.title, "Milk");
    assert_eq!(t1.sync_state, "failed");
    let discarded = store
        .discard_op("op-2", OpState::Unknown, &Restore::Tombstone)
        .await
        .expect("discard");
    assert_eq!(discarded, Some(Vec::new()));
    assert!(store.task("t2").await.expect("read").is_none());
    assert!(store.outbox_op("op-2").await.expect("read").is_none());
}

#[tokio::test]
async fn a_database_a_newer_ms_todo_migrated_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ms-todo.db");
    drop(Store::open(&path).await.expect("open"));
    let mut connection = <sqlx::SqliteConnection as sqlx::Connection>::connect(&format!(
        "sqlite:{}",
        path.display()
    ))
    .await
    .expect("connect");
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (9999, 'newer', 1, x'00', 0)",
    )
    .execute(&mut connection)
    .await
    .expect("insert");

    let refused = Store::open(&path).await.err().expect("refused");
    assert!(matches!(refused, StoreError::NewerDatabase), "{refused:?}");
}

#[tokio::test]
async fn a_failed_create_fails_the_writes_queued_behind_it_and_the_task_stays_gone() {
    let (_dir, store, list) = open().await;
    store
        .enqueue("op-1", None, vec![create("op-1", "t1", &list, "Milk")])
        .await
        .expect("create");
    store
        .enqueue("op-2", None, vec![edit("op-2", "t1", &list, "Oat milk")])
        .await
        .expect("edit");

    let cascaded = store
        .fail_op("op-1", ("rejected", "no"), &Restore::Tombstone)
        .await
        .expect("fail");

    assert_eq!(cascaded, ["op-2"]);
    let edit = store.outbox_op("op-2").await.expect("read").expect("op-2");
    assert_eq!(edit.state, OpState::Failed);
    assert!(store.task("t1").await.expect("read").is_none());
    assert!(store.ready_ops(i64::MAX).await.expect("ready").is_empty());
}

/// A create, then two edits queued on it, each waiting on the one before.
async fn chain(store: &Store, list: &str) {
    store
        .enqueue("c", None, vec![create("c", "t1", list, "Milk")])
        .await
        .expect("create");
    store
        .enqueue("e1", None, vec![edit("e1", "t1", list, "Oat milk")])
        .await
        .expect("edit");
    store
        .enqueue("e2", None, vec![edit("e2", "t1", list, "Soy milk")])
        .await
        .expect("edit");
}

#[tokio::test]
async fn discarding_an_operation_fails_everything_that_waits_on_it() {
    let (_dir, store, list) = open().await;
    chain(&store, &list).await;

    let failed = store
        .discard_op("c", OpState::Pending, &Restore::Tombstone)
        .await
        .expect("discard")
        .expect("it was pending");

    assert_eq!(failed, ["e1", "e2"], "through e1 to e2");
    for op in ["e1", "e2"] {
        let op = store.outbox_op(op).await.expect("read").expect("op");
        assert_eq!(op.state, OpState::Failed);
        assert!(
            op.note
                .as_deref()
                .is_some_and(|note| note.contains("c, which was discarded"))
        );
    }
    assert!(store.ready_ops(i64::MAX).await.expect("ready").is_empty());
}

#[tokio::test]
async fn a_discard_that_loses_the_race_to_the_worker_changes_nothing() {
    let (_dir, store, list) = open().await;
    chain(&store, &list).await;
    // Read as pending; then the worker claims it before the discard runs.
    assert!(store.mark_inflight("c").await.expect("claim"));

    let discarded = store
        .discard_op("c", OpState::Pending, &Restore::Tombstone)
        .await
        .expect("discard");

    assert_eq!(discarded, None);
    let op = store
        .outbox_op("c")
        .await
        .expect("read")
        .expect("still there");
    assert_eq!(op.state, OpState::Inflight);
    assert!(
        store.task("t1").await.expect("read").is_some(),
        "not rolled back"
    );
    let waiting = store.outbox_op("e1").await.expect("read").expect("e1");
    assert_eq!(waiting.state, OpState::Pending);
}

#[tokio::test]
async fn the_worker_cannot_claim_an_operation_discarded_meanwhile() {
    let (_dir, store, list) = open().await;
    chain(&store, &list).await;
    // Read by the worker as ready; then discarded before its claim.
    assert!(
        store
            .ready_ops(i64::MAX)
            .await
            .expect("ready")
            .iter()
            .any(|op| op.op_id == "c")
    );
    store
        .discard_op("c", OpState::Pending, &Restore::Tombstone)
        .await
        .expect("discard")
        .expect("it was pending");

    assert!(!store.mark_inflight("c").await.expect("claim"));
}

#[tokio::test]
async fn a_retry_that_loses_the_race_changes_nothing() {
    let (_dir, store, list) = open().await;
    chain(&store, &list).await;
    assert!(store.mark_inflight("c").await.expect("claim"));
    store
        .mark_unknown("c", ("outcome_unknown", "503"), None)
        .await
        .expect("unknown");

    // Read as unknown by two retries; the first wins.
    assert!(
        store
            .requeue("c", OpState::Unknown, &Restore::Nothing, None)
            .await
            .expect("retry")
    );
    assert!(
        !store
            .requeue("c", OpState::Unknown, &Restore::Nothing, None)
            .await
            .expect("retry")
    );
    // And one that read it as unknown can't touch it once it's inflight.
    assert!(store.mark_inflight("c").await.expect("claim"));
    assert!(
        !store
            .requeue("c", OpState::Unknown, &Restore::Nothing, None)
            .await
            .expect("retry")
    );
    let op = store.outbox_op("c").await.expect("read").expect("c");
    assert_eq!(op.state, OpState::Inflight);
}

#[tokio::test]
async fn only_a_done_dependency_unblocks_an_operation() {
    let (_dir, store, list) = open().await;
    chain(&store, &list).await;
    assert!(store.mark_inflight("c").await.expect("claim"));
    store
        .fail_op("c", ("rejected", "no"), &Restore::Tombstone)
        .await
        .expect("fail");
    // Put e1 back to pending, as a retry that skipped the check would.
    assert!(
        store
            .requeue("e1", OpState::Failed, &Restore::Nothing, None)
            .await
            .expect("requeue")
    );

    let ready = store.ready_ops(i64::MAX).await.expect("ready");

    assert!(ready.iter().all(|op| op.op_id != "e1"), "{ready:?}");
}

fn my_day(op_id: &str, local_id: &str, list: &str, fields: Value) -> NewOp {
    NewOp {
        op_id: op_id.into(),
        entity_local_id: local_id.into(),
        list_local_id: list.into(),
        op: OpKind::TaskExtension,
        action: "my_day_add".into(),
        payload: json!({ "body": fields }),
        change: LocalChange::Extension,
    }
}

#[tokio::test]
async fn a_task_extension_write_merges_at_once_rolls_back_and_stays_over_graphs_answer() {
    let (_dir, store, list) = open().await;
    store
        .enqueue(
            "op-1",
            None,
            vec![create("op-1", "t1", &list, "Call the bank")],
        )
        .await
        .expect("create");
    store
        .record_sent("op-1", &task("T1", "Call the bank", "e1"), None, true)
        .await
        .expect("record");

    // Queued: the extension changes at once, other fields kept; the
    // extension before is the rollback.
    let rows = store
        .enqueue(
            "op-2",
            None,
            vec![my_day(
                "op-2",
                "t1",
                &list,
                json!({ "myDay": "2026-09-25", "myDayDueSet": true }),
            )],
        )
        .await
        .expect("my day");
    let extension = rows[0].extension.clone().expect("extension");
    assert_eq!(extension["myDay"], "2026-09-25");
    assert_eq!(extension["opId"], "op-1");
    let op = store.outbox_op("op-2").await.expect("read").expect("op-2");
    assert_eq!(op.op, OpKind::TaskExtension);
    assert!(op.rollback.expect("rollback").get("myDay").is_none());

    // Graph answers an earlier write while this one waits: it stays on top.
    store
        .record_sent(
            "op-1",
            &task("T1", "Call the bank", "e2"),
            Some(Some(json!({ "opId": "op-1" }))),
            true,
        )
        .await
        .expect("record again");
    let row = store.task("t1").await.expect("read").expect("t1");
    assert_eq!(row.extension.expect("extension")["myDay"], "2026-09-25");

    // Rejected: the extension goes back.
    let before = entity(json!({ "opId": "op-1" }));
    store
        .fail_op("op-2", ("rejected", "no"), &Restore::Replace(before))
        .await
        .expect("fail");
    let row = store.task("t1").await.expect("read").expect("t1");
    assert_eq!(row.extension, Some(json!({ "opId": "op-1" })));
    assert_eq!(
        row.raw["title"], "Call the bank",
        "the task itself untouched"
    );
}

#[tokio::test]
async fn the_default_undo_passes_over_an_automatic_command() {
    let (_dir, store, list) = open().await;
    store
        .enqueue("op-1", None, vec![create("op-1", "t1", &list, "Milk")])
        .await
        .expect("create");
    store
        .enqueue("op-2", None, vec![edit("op-2", "t1", &list, "Oat milk")])
        .await
        .expect("edit");
    let mut automatic = my_day("op-3", "t1", &list, json!({ "myDay": null }));
    automatic.payload["origin"] = json!("auto");
    store
        .enqueue("op-3", None, vec![automatic])
        .await
        .expect("automatic");
    assert_eq!(
        store
            .last_undoable_command()
            .await
            .expect("read")
            .as_deref(),
        Some("op-2")
    );
}
