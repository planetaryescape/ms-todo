//! Folders in the store: a list's folder and order read from its extension,
//! and folder writes through the outbox, against a real SQLite file.

use ms_todo_store::{Cursor, Entity, ListExtensionOp, ListsPass, OpState, Restore, Store};
use serde_json::{Map, Value, json};

fn entity(value: Value) -> Entity {
    value.as_object().cloned().expect("object")
}

fn whole() -> Cursor {
    Cursor {
        delta_link: "link".into(),
        replayed: false,
    }
}

/// A store with one list, "Finances", whose extension is `extension`.
async fn open(extension: Option<Value>) -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("open");
    sync_list(&store, extension, "e1").await;
    let list = store.lists().await.expect("lists")[0].local_id.clone();
    (dir, store, list)
}

/// A lists pass in which Graph has the list with `extension`.
async fn sync_list(store: &Store, extension: Option<Value>, etag: &str) {
    let rev = store.local_rev().await.expect("rev");
    let list = entity(json!({ "id": "L1", "displayName": "Finances", "@odata.etag": etag }));
    store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(list, extension)],
            cursor: whole(),
        })
        .await
        .expect("lists");
}

fn move_to(op_id: &str, list: &str, folder: Value) -> ListExtensionOp {
    let fields: Map<String, Value> = [("folder".to_owned(), folder)].into_iter().collect();
    ListExtensionOp {
        op_id: op_id.into(),
        list_local_id: list.into(),
        action: "move_list".into(),
        fields,
    }
}

#[tokio::test]
async fn a_lists_folder_and_order_come_from_its_extension() {
    let (_dir, store, _) = open(Some(json!({
        "extensionName": "com.planetaryescape.mstodo",
        "folder": " Areas ",
        "order@odata.type": "#Int64",
        "order": 3,
        "folderOrder": 2
    })))
    .await;
    let list = &store.lists().await.expect("lists")[0];
    assert_eq!(list.folder(), Some("Areas"));
    assert_eq!(list.order(), Some(3));
    assert_eq!(list.folder_order(), Some(2));
    assert_eq!(list.sync_state, "synced");

    sync_list(&store, Some(json!({ "folder": "  " })), "e2").await;
    let list = &store.lists().await.expect("lists")[0];
    assert_eq!(list.folder(), None, "a blank folder is none");
    assert_eq!(list.order(), None);
}

#[tokio::test]
async fn a_folder_write_merges_at_once_and_a_pass_leaves_it_until_graph_answers() {
    let (_dir, store, list) = open(Some(json!({ "keep": "me", "folder": "Work" }))).await;
    let rows = store
        .enqueue_list_extension("op-1", None, vec![move_to("op-1", &list, json!("Areas"))])
        .await
        .expect("queue");
    assert_eq!(rows[0].folder(), Some("Areas"));
    assert_eq!(rows[0].sync_state, "pending");
    assert_eq!(rows[0].extension.as_ref().expect("ext")["keep"], "me");

    // A pass that read Graph before the write reached it changes nothing.
    sync_list(
        &store,
        Some(json!({ "keep": "me", "folder": "Work" })),
        "e2",
    )
    .await;
    assert_eq!(
        store.lists().await.expect("lists")[0].folder(),
        Some("Areas")
    );

    let op = store.outbox_op("op-1").await.expect("read").expect("op");
    assert_eq!(
        op.rollback,
        Some(entity(json!({ "keep": "me", "folder": "Work" })))
    );
    assert_eq!(op.title.as_deref(), Some("Finances"));
    assert!(store.mark_inflight("op-1").await.expect("claim"));
    let graph_has = entity(json!({ "keep": "me", "folder": "Areas", "extensionName": "x" }));
    store
        .record_list_extension("op-1", &graph_has)
        .await
        .expect("record");
    let list_row = &store.lists().await.expect("lists")[0];
    assert_eq!(list_row.sync_state, "synced");
    assert_eq!(list_row.extension, Some(Value::Object(graph_has)));
    let op = store.outbox_op("op-1").await.expect("read").expect("op");
    assert_eq!(op.state, OpState::Done);
}

#[tokio::test]
async fn graphs_answer_keeps_a_later_queued_folder_write_on_top() {
    let (_dir, store, list) = open(None).await;
    store
        .enqueue_list_extension("op-1", None, vec![move_to("op-1", &list, json!("Areas"))])
        .await
        .expect("first");
    store
        .enqueue_list_extension("op-2", None, vec![move_to("op-2", &list, json!("Someday"))])
        .await
        .expect("second");
    let second = store.outbox_op("op-2").await.expect("read").expect("op");
    assert_eq!(second.depends_on.as_deref(), Some("op-1"));
    store
        .record_list_extension("op-1", &entity(json!({ "folder": "Areas" })))
        .await
        .expect("record");
    assert_eq!(
        store.lists().await.expect("lists")[0].folder(),
        Some("Someday")
    );
}

#[tokio::test]
async fn a_rejected_folder_write_restores_the_extension_before_it() {
    let (_dir, store, list) = open(Some(json!({ "folder": "Work" }))).await;
    store
        .enqueue_list_extension("op-1", None, vec![move_to("op-1", &list, Value::Null)])
        .await
        .expect("queue");
    assert_eq!(store.lists().await.expect("lists")[0].folder(), None);
    let before = entity(json!({ "folder": "Work" }));
    store
        .fail_op("op-1", ("rejected", "no"), &Restore::Replace(before))
        .await
        .expect("fail");
    let row = &store.lists().await.expect("lists")[0];
    assert_eq!(row.folder(), Some("Work"));
    assert_eq!(row.sync_state, "failed");
    // An empty extension is no extension.
    store
        .discard_op("op-1", OpState::Failed, &Restore::Replace(Entity::new()))
        .await
        .expect("discard")
        .expect("discarded");
    let row = &store.lists().await.expect("lists")[0];
    assert_eq!(row.extension, None);
    assert_eq!(row.sync_state, "synced");
}
