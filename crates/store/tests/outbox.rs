//! The outbox against a real SQLite file: what each outcome does to the
//! task row, atomically with the operation's state.

use ms_todo_store::{
    ChildVerb, Claim, Cursor, Entity, Hydration, ListsPass, LocalChange, NewOp, OpKind, OpState,
    Restore, STEPS, SeenTask, Store, StoreError, TasksPass, UNKNOWN_LOOKUP_SECS, child_payload,
    tasks_scope,
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

fn child(op_id: &str, local_id: &str, list: &str, verb: ChildVerb, id: &str, body: Value) -> NewOp {
    NewOp {
        op_id: op_id.into(),
        entity_local_id: local_id.into(),
        list_local_id: list.into(),
        op: OpKind::Child,
        action: "step".into(),
        payload: child_payload(STEPS, verb, id, body, &[]),
        change: LocalChange::Child,
    }
}

#[tokio::test]
async fn a_created_step_takes_graphs_id_everywhere_a_queued_change_names_it() {
    let (_dir, store, list) = open().await;
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(TasksPass {
            scope: tasks_scope("L1"),
            list_local_id: list.clone(),
            rev,
            seen: vec![SeenTask {
                raw: task("T1", "Paint", "e1"),
                hydration: Hydration::Kept,
            }],
            gone: Vec::new(),
            failure: None,
            cursor: whole(),
        })
        .await
        .expect("sync");
    let local = store.tasks_in_list(&list).await.expect("tasks")[0]
        .local_id
        .clone();
    let add = child(
        "op-1",
        &local,
        &list,
        ChildVerb::Create,
        "local-a",
        json!({ "displayName": "Buy paint", "isChecked": false }),
    );
    let check = child(
        "op-2",
        &local,
        &list,
        ChildVerb::Update,
        "local-a",
        json!({ "isChecked": true }),
    );
    store.enqueue("op-1", None, vec![add]).await.expect("add");
    let rows = store
        .enqueue("op-2", None, vec![check])
        .await
        .expect("check");
    assert_eq!(
        rows[0].raw["checklistItems"],
        json!([{ "id": "local-a", "displayName": "Buy paint", "isChecked": true }]),
        "both applied at once"
    );

    // Graph created it, and the task read back has it unchecked, since the
    // check hasn't been sent yet.
    let created = entity(json!({ "id": "c1", "displayName": "Buy paint", "isChecked": false }));
    let mut read_back = task("T1", "Paint", "e2");
    read_back.insert("checklistItems".into(), json!([created.clone()]));
    store
        .record_child_created("op-1", &created, Some((read_back, None)))
        .await
        .expect("record");

    let row = store.task(&local).await.expect("read").expect("task");
    assert_eq!(
        row.raw["checklistItems"],
        json!([{ "id": "c1", "displayName": "Buy paint", "isChecked": true }]),
        "Graph's step, with the queued check on top"
    );
    assert_eq!(row.raw["@odata.etag"], "e2");
    for op_id in ["op-1", "op-2"] {
        let op = store.outbox_op(op_id).await.expect("read").expect("op");
        assert_eq!(op.payload["id"], "c1", "{op_id} names Graph's ID");
    }
    let check = store.outbox_op("op-2").await.expect("read").expect("op-2");
    assert_eq!(
        check.rollback.expect("rollback")["checklistItems"][0]["id"],
        "c1",
        "what it rolls back to names Graph's ID too"
    );
    assert_eq!(
        store
            .outbox_op("op-1")
            .await
            .expect("read")
            .expect("op")
            .state,
        OpState::Done
    );

    // A child create still being sent at a restart may have reached Graph.
    store
        .enqueue(
            "op-3",
            None,
            vec![child(
                "op-3",
                &local,
                &list,
                ChildVerb::Create,
                "local-b",
                json!({}),
            )],
        )
        .await
        .expect("add");
    for op_id in ["op-2", "op-3"] {
        assert!(store.mark_inflight(op_id).await.expect("claim"));
    }
    store.recover_inflight().await.expect("recover");
    let state = |op_id: &'static str| {
        let store = &store;
        async move {
            store
                .outbox_op(op_id)
                .await
                .expect("read")
                .expect("op")
                .state
        }
    };
    assert_eq!(state("op-2").await, OpState::Pending, "a PATCH is resent");
    assert_eq!(state("op-3").await, OpState::Unknown, "a POST isn't");
    let (_, flagged) = store
        .outbox_depth(UNKNOWN_LOOKUP_SECS)
        .await
        .expect("depth");
    assert_eq!(flagged, 1, "no marker can find a step, so the user decides");
}

/// T1 synced into the list, with one step `s1` called "A"; its local ID.
async fn with_step(store: &Store, list: &str) -> String {
    let rev = store.local_rev().await.expect("rev");
    let mut raw = task("T1", "Paint", "e1");
    raw.insert(
        "checklistItems".into(),
        json!([{ "id": "s1", "displayName": "A", "isChecked": false }]),
    );
    store
        .apply_tasks(TasksPass {
            scope: tasks_scope("L1"),
            list_local_id: list.to_owned(),
            rev,
            seen: vec![SeenTask {
                raw,
                hydration: Hydration::Kept,
            }],
            gone: Vec::new(),
            failure: None,
            cursor: whole(),
        })
        .await
        .expect("sync");
    store.tasks_in_list(list).await.expect("tasks")[0]
        .local_id
        .clone()
}

fn rename_step(op_id: &str, local: &str, list: &str, name: &str) -> NewOp {
    child(
        op_id,
        local,
        list,
        ChildVerb::Update,
        "s1",
        json!({ "displayName": name }),
    )
}

#[tokio::test]
async fn a_cascade_takes_a_child_written_twice_back_to_where_it_began() {
    let (_dir, store, list) = open().await;
    let local = with_step(&store, &list).await;
    store
        .enqueue("op-0", None, vec![edit("op-0", &local, &list, "Paint it")])
        .await
        .expect("edit");
    store
        .enqueue("op-1", None, vec![rename_step("op-1", &local, &list, "B")])
        .await
        .expect("A to B");
    store
        .enqueue("op-2", None, vec![rename_step("op-2", &local, &list, "C")])
        .await
        .expect("B to C");

    let cascaded = store
        .fail_op("op-0", ("rejected", "no"), &Restore::Nothing)
        .await
        .expect("fail");
    assert_eq!(cascaded, ["op-1", "op-2"]);
    let row = store.task(&local).await.expect("read").expect("task");
    assert_eq!(row.raw["checklistItems"][0]["displayName"], "A");
}

#[tokio::test]
async fn a_task_delete_skips_its_unsent_child_writes_only_once_it_is_done() {
    let (_dir, store, list) = open().await;
    let local = with_step(&store, &list).await;
    let before = store.task(&local).await.expect("read").expect("task").raw;
    let delete = |op_id: &str| NewOp {
        op_id: op_id.into(),
        entity_local_id: local.clone(),
        list_local_id: list.clone(),
        op: OpKind::Delete,
        action: "delete".into(),
        payload: json!({}),
        change: LocalChange::Tombstone,
    };
    store
        .enqueue("op-1", None, vec![rename_step("op-1", &local, &list, "B")])
        .await
        .expect("rename");
    store
        .enqueue("op-2", None, vec![delete("op-2")])
        .await
        .expect("delete");
    let ready: Vec<String> = store
        .ready_ops(i64::MAX)
        .await
        .expect("ready")
        .into_iter()
        .map(|op| op.op_id)
        .collect();
    assert_eq!(
        ready,
        ["op-1", "op-2"],
        "the delete doesn't wait on the step"
    );

    // Rejected, the delete leaves the step to be sent.
    store
        .fail_op("op-2", ("rejected", "no"), &Restore::Replace(before))
        .await
        .expect("fail");
    let step = store.outbox_op("op-1").await.expect("read").expect("op");
    assert_eq!(step.state, OpState::Pending);

    // Done, it takes the step with it.
    store
        .enqueue("op-3", None, vec![delete("op-3")])
        .await
        .expect("delete again");
    store.mark_done("op-3").await.expect("done");
    let step = store.outbox_op("op-1").await.expect("read").expect("op");
    assert!(step.was_skipped(), "{step:?}");
}

/// Set every operation's `finished_at` but `except`'s to 40 days ago.
async fn age(dir: &tempfile::TempDir, except: &[&str]) {
    let url = format!("sqlite://{}", dir.path().join("ms-todo.db").display());
    let pool = sqlx::SqlitePool::connect(&url).await.expect("connect");
    let old = chrono::Utc::now().timestamp() - 40 * 24 * 60 * 60;
    for op in ["a", "e", "u", "b", "f"] {
        if !except.contains(&op) {
            sqlx::query("UPDATE outbox SET finished_at = ?, created_at = ? WHERE op_id = ?")
                .bind(old)
                .bind(old)
                .bind(op)
                .execute(&pool)
                .await
                .expect("age");
        }
    }
    pool.close().await;
}

#[tokio::test]
async fn pruning_takes_old_finished_commands_but_not_an_undo_pair_split_or_a_dependency() {
    let (dir, store, list) = open().await;
    let done = |op: &'static str| {
        let store = &store;
        async move {
            assert!(store.mark_inflight(op).await.expect("claim"));
            store.mark_done(op).await.expect("done");
        }
    };
    store
        .enqueue("a", None, vec![create("a", "local-a", &list, "Buy milk")])
        .await
        .expect("a");
    done("a").await;
    store
        .enqueue("e", None, vec![edit("e", "local-a", &list, "Oat milk")])
        .await
        .expect("e");
    done("e").await;
    store
        .enqueue(
            "u",
            Some("e"),
            vec![edit("u", "local-a", &list, "Buy milk")],
        )
        .await
        .expect("u");
    done("u").await;
    // f waits on b, and fails after b is done: a retry needs b there.
    store
        .enqueue("b", None, vec![create("b", "local-b", &list, "Bread")])
        .await
        .expect("b");
    store
        .enqueue("f", None, vec![edit("f", "local-b", &list, "Rye bread")])
        .await
        .expect("f");
    done("b").await;
    assert!(store.mark_inflight("f").await.expect("claim"));
    store
        .fail_op("f", ("rejected", "no"), &Restore::Nothing)
        .await
        .expect("fail");
    age(&dir, &["u"]).await;
    let cutoff = chrono::Utc::now().timestamp() - 30 * 24 * 60 * 60;

    assert_eq!(store.prune_outbox(cutoff).await.expect("prune"), 1);

    let left: Vec<String> = store
        .outbox(None)
        .await
        .expect("outbox")
        .into_iter()
        .map(|op| op.op_id)
        .collect();
    assert_eq!(left, ["f", "b", "u", "e"], "only a went");
    // With its undo old too, the pair goes together.
    age(&dir, &[]).await;
    assert_eq!(store.prune_outbox(cutoff).await.expect("prune"), 2);
    assert_eq!(store.prune_outbox(cutoff).await.expect("prune"), 0);
}

#[tokio::test]
async fn the_lookup_window_decides_which_unknown_operations_are_flagged() {
    let (_dir, store, list) = open().await;
    store
        .enqueue("c", None, vec![create("c", "local-c", &list, "Buy milk")])
        .await
        .expect("add");
    assert!(store.mark_inflight("c").await.expect("claim"));
    store
        .mark_unknown("c", ("outcome_unknown", "timed out"), None)
        .await
        .expect("unknown");
    let op = store.outbox_op("c").await.expect("read").expect("op");
    let since = op.unknown_since.expect("since");
    assert!(!op.is_flagged(since + 3599, 3600));
    assert!(op.is_flagged(since + 3600, 3600));
    let (_, flagged) = store
        .outbox_depth(UNKNOWN_LOOKUP_SECS)
        .await
        .expect("depth");
    assert_eq!(flagged, 0);
    let (_, flagged) = store.outbox_depth(0).await.expect("depth");
    assert_eq!(flagged, 1);
}

async fn pool(dir: &tempfile::TempDir) -> sqlx::SqlitePool {
    let url = format!("sqlite://{}", dir.path().join("ms-todo.db").display());
    sqlx::SqlitePool::connect(&url).await.expect("connect")
}

#[tokio::test]
async fn a_command_a_live_idempotency_key_answers_for_is_kept_then_pruned_with_its_key() {
    let (dir, store, list) = open().await;
    assert_eq!(
        store.claim_key("key-1", "fp", "k").await.expect("claim"),
        Claim::Fresh
    );
    store
        .enqueue("k", None, vec![create("k", "local-k", &list, "Buy milk")])
        .await
        .expect("k");
    assert!(store.mark_inflight("k").await.expect("claim"));
    store.mark_done("k").await.expect("done");
    store.finish_key("key-1", "{}").await.expect("finish");
    let pool = pool(&dir).await;
    let old = chrono::Utc::now().timestamp() - 40 * 24 * 60 * 60;
    sqlx::query("UPDATE outbox SET finished_at = ?, created_at = ?")
        .bind(old)
        .bind(old)
        .execute(&pool)
        .await
        .expect("age the op");
    let cutoff = chrono::Utc::now().timestamp() - 30 * 24 * 60 * 60;

    assert_eq!(
        store.prune_outbox(cutoff).await.expect("prune"),
        0,
        "the key answered within a day"
    );

    sqlx::query("UPDATE idempotency_keys SET finished_at = ?")
        .bind(chrono::Utc::now().timestamp() - 2 * 24 * 60 * 60)
        .execute(&pool)
        .await
        .expect("age the key");
    assert_eq!(store.prune_outbox(cutoff).await.expect("prune"), 1);
    let keys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM idempotency_keys")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(keys, 0, "the key went with its command");
}

#[tokio::test]
async fn an_overwrite_is_recorded_whole_or_not_at_all() {
    let (dir, store, list) = open().await;
    store
        .enqueue("c", None, vec![create("c", "local-c", &list, "Buy milk")])
        .await
        .expect("add");
    assert!(store.mark_inflight("c").await.expect("claim"));
    store
        .record_sent("c", &task("T1", "Buy milk", "e1"), None, true)
        .await
        .expect("created");
    store
        .enqueue("e", None, vec![edit("e", "local-c", &list, "Oat milk")])
        .await
        .expect("edit");
    assert!(store.mark_inflight("e").await.expect("claim"));
    let ours = task("T1", "Oat milk", "e3");
    let theirs = task("T1", "Almond milk", "e2");
    // A write that fails part-way through changes nothing.
    let pool = pool(&dir).await;
    sqlx::query(
        "CREATE TRIGGER refuse_rollback BEFORE UPDATE OF rollback_json ON outbox \
         BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .execute(&pool)
    .await
    .expect("trigger");

    let failed = store
        .record_overwrite("e", &ours, None, "overwrote title", &theirs)
        .await;

    assert!(failed.is_err());
    let op = store.outbox_op("e").await.expect("read").expect("op");
    assert_eq!(op.state, OpState::Inflight, "not done without its rollback");
    assert_eq!(op.note, None);
    sqlx::query("DROP TRIGGER refuse_rollback")
        .execute(&pool)
        .await
        .expect("drop");
    store
        .record_overwrite("e", &ours, None, "overwrote title", &theirs)
        .await
        .expect("record");
    let op = store.outbox_op("e").await.expect("read").expect("op");
    assert_eq!(op.state, OpState::Done);
    assert_eq!(op.note.as_deref(), Some("overwrote title"));
    assert_eq!(
        op.rollback.and_then(|before| before.get("title").cloned()),
        Some(json!("Almond milk"))
    );
}

#[tokio::test]
async fn a_skipped_write_is_recorded_whole_or_not_at_all_after_reopen() {
    let (dir, store, list) = open().await;
    store
        .enqueue("c", None, vec![create("c", "local-c", &list, "Buy milk")])
        .await
        .expect("add");
    store
        .record_sent("c", &task("T1", "Buy milk", "e1"), None, true)
        .await
        .expect("created");
    store
        .enqueue("e", None, vec![edit("e", "local-c", &list, "Oat milk")])
        .await
        .expect("edit");
    store.mark_inflight("e").await.expect("claim");
    let theirs = task("T1", "Almond milk", "e2");
    let pool = pool(&dir).await;
    sqlx::query("CREATE TRIGGER refuse_skip BEFORE UPDATE OF note ON outbox BEGIN SELECT RAISE(ABORT, 'refused'); END")
        .execute(&pool).await.expect("trigger");
    assert!(
        store
            .record_skipped("e", &theirs, None, "changed elsewhere")
            .await
            .is_err()
    );
    drop(store);
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("reopen");
    let op = store.outbox_op("e").await.expect("read").expect("op");
    assert_eq!(
        op.state,
        OpState::Inflight,
        "done must never appear without its skipped reason"
    );
    assert_eq!(op.note, None);
    assert_eq!(
        store
            .task_any("local-c")
            .await
            .expect("read")
            .expect("task")
            .0
            .raw["title"],
        "Oat milk"
    );
    sqlx::query("DROP TRIGGER refuse_skip")
        .execute(&pool)
        .await
        .expect("drop");
    store
        .record_skipped("e", &theirs, None, "changed elsewhere")
        .await
        .expect("record");
    drop(store);
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("reopen");
    let op = store.outbox_op("e").await.expect("read").expect("op");
    assert_eq!(op.state, OpState::Done);
    assert_eq!(op.note.as_deref(), Some("skipped: changed elsewhere"));
    assert!(op.was_skipped());
    assert_eq!(
        store
            .task_any("local-c")
            .await
            .expect("read")
            .expect("task")
            .0
            .raw["title"],
        "Almond milk"
    );
}

#[tokio::test]
async fn concurrent_completions_queue_one_operation_per_recurring_occurrence() {
    let (_dir, store, list) = open().await;
    store
        .enqueue(
            "c",
            None,
            vec![create("c", "local-c", &list, "Water plants")],
        )
        .await
        .expect("add");
    let mut recurring = task("T1", "Water plants", "e1");
    recurring.insert(
        "recurrence".into(),
        json!({ "pattern": { "type": "daily", "interval": 1 } }),
    );
    recurring.insert(
        "dueDateTime".into(),
        json!({ "dateTime": "2026-09-24T00:00:00", "timeZone": "UTC" }),
    );
    store
        .record_sent("c", &recurring, None, true)
        .await
        .expect("created");
    let complete = |id: &str, due: &str| NewOp {
        op_id: id.into(),
        entity_local_id: "local-c".into(),
        list_local_id: list.clone(),
        op: OpKind::Update,
        action: "complete".into(),
        payload: json!({ "body": { "status": "completed" }, "recurring": true, "due_before": due }),
        change: LocalChange::Update,
    };
    let (first, second) = tokio::join!(
        store.enqueue("first", None, vec![complete("first", "2026-09-24")]),
        store.enqueue("second", None, vec![complete("second", "2026-09-24")]),
    );
    first.expect("first");
    second.expect("second");
    let first = store.outbox_op("first").await.expect("first");
    let second = store.outbox_op("second").await.expect("second");
    assert_ne!(
        first.is_some(),
        second.is_some(),
        "one pending completion per occurrence"
    );
    let queued = first.or(second).expect("completion");
    recurring.insert(
        "dueDateTime".into(),
        json!({ "dateTime": "2026-09-25T00:00:00", "timeZone": "UTC" }),
    );
    store
        .record_sent(&queued.op_id, &recurring, None, true)
        .await
        .expect("rolled");
    store
        .enqueue("next", None, vec![complete("next", "2026-09-25")])
        .await
        .expect("next occurrence");
    assert!(store.outbox_op("next").await.expect("read").is_some());
}

#[tokio::test]
async fn an_ordinary_completion_still_applies_its_reminder_change() {
    let (_dir, store, list) = open().await;
    store
        .enqueue("c", None, vec![create("c", "local-c", &list, "Buy milk")])
        .await
        .expect("add");
    let mut completed = task("T1", "Buy milk", "e1");
    completed.insert("status".into(), json!("completed"));
    completed.insert("isReminderOn".into(), json!(true));
    store
        .record_sent("c", &completed, None, true)
        .await
        .expect("created");
    store
        .enqueue(
            "complete",
            None,
            vec![NewOp {
                op_id: "complete".into(),
                entity_local_id: "local-c".into(),
                list_local_id: list,
                op: OpKind::Update,
                action: "complete".into(),
                payload: json!({ "body": { "status": "completed", "isReminderOn": false } }),
                change: LocalChange::Update,
            }],
        )
        .await
        .expect("complete");
    assert!(
        store
            .outbox_op("complete")
            .await
            .expect("operation")
            .is_some()
    );
    assert_eq!(
        store
            .task_any("local-c")
            .await
            .expect("read")
            .expect("task")
            .0
            .raw["isReminderOn"],
        false
    );
}
