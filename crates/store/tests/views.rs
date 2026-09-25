//! The smart views and the sidebar's counts, against a real SQLite file.

use ms_todo_store::{
    Cursor, Entity, Hydration, ListsPass, SeenTask, Store, TasksPass, View, tasks_scope,
};
use serde_json::{Value, json};

fn entity(value: Value) -> Entity {
    value.as_object().cloned().expect("object")
}

fn task(id: &str, extra: Value) -> SeenTask {
    let mut raw = entity(json!({
        "id": id, "title": id, "status": "notStarted", "importance": "normal",
        "@odata.etag": "e1", "createdDateTime": format!("2026-09-24T10:00:0{}.0000000Z", &id[1..])
    }));
    raw.extend(entity(extra));
    SeenTask {
        raw,
        hydration: Hydration::Kept,
    }
}

/// `seen` with ms-todo's extension naming who it waits on.
fn assigned(mut seen: SeenTask, who: &str) -> SeenTask {
    seen.hydration = Hydration::Fetched(Some(json!({ "assignee": who })));
    seen
}

fn due(date: &str) -> Value {
    json!({ "dateTime": format!("{date}T00:00:00.0000000"), "timeZone": "Europe/London" })
}

/// Two lists: "Home" with every kind of task, "Work" with one open task.
async fn filled() -> (tempfile::TempDir, Store, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("open");
    let list = |id: &str, name: &str| {
        (
            entity(json!({ "id": id, "displayName": name, "wellknownListName": "none" })),
            None,
        )
    };
    let whole = || Cursor {
        delta_link: "link".into(),
        replayed: false,
    };
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_lists(ListsPass {
            rev,
            lists: vec![list("L1", "Home"), list("L2", "Work")],
            cursor: whole(),
        })
        .await
        .expect("lists");
    let lists = store.lists().await.expect("lists");
    let (home, work) = (lists[0].local_id.clone(), lists[1].local_id.clone());
    for (graph_id, local_id, seen) in [
        (
            "L1",
            &home,
            vec![
                assigned(task("T1", json!({ "importance": "high" })), "sam"),
                // Blank is nobody.
                assigned(
                    task("T2", json!({ "dueDateTime": due("2026-10-02") })),
                    "  ",
                ),
                task(
                    "T3",
                    json!({ "dueDateTime": due("2026-09-30"), "importance": "high" }),
                ),
                assigned(
                    task(
                        "T4",
                        json!({ "status": "completed",
                                "completedDateTime": { "dateTime": "2026-09-20T09:00:00.0000000", "timeZone": "UTC" } }),
                    ),
                    "Sam",
                ),
                task(
                    "T5",
                    json!({ "status": "completed", "dueDateTime": due("2026-09-01"),
                            "completedDateTime": { "dateTime": "2026-09-22T09:00:00.0000000", "timeZone": "UTC" } }),
                ),
            ],
        ),
        (
            "L2",
            &work,
            vec![
                assigned(task("T6", json!({})), "Ada"),
                // Completed here, not yet answered by Graph: no date.
                task("T7", json!({ "status": "completed" })),
            ],
        ),
    ] {
        store
            .apply_tasks(TasksPass {
                scope: tasks_scope(graph_id),
                list_local_id: local_id.clone(),
                rev,
                seen,
                gone: Vec::new(),
                failure: None,
                cursor: whole(),
            })
            .await
            .expect("tasks");
    }
    (dir, store, home, work)
}

async fn titles(store: &Store, view: View) -> Vec<String> {
    store
        .tasks_in_view(view)
        .await
        .expect("view")
        .into_iter()
        .map(|task| task.title)
        .collect()
}

#[tokio::test]
async fn each_view_holds_its_tasks_in_its_order() {
    let (_dir, store, _, _) = filled().await;
    // Due soonest first, then the undated.
    assert_eq!(titles(&store, View::Important).await, ["T3", "T1"]);
    // A completed task with a due date isn't planned.
    assert_eq!(titles(&store, View::Planned).await, ["T3", "T2"]);
    assert_eq!(titles(&store, View::All).await, ["T1", "T2", "T3", "T6"]);
    // Most recently completed first; one with no date yet is newest.
    assert_eq!(titles(&store, View::Completed).await, ["T7", "T5", "T4"]);
    // Open and assigned, by person whatever the case; blank is nobody.
    assert_eq!(titles(&store, View::Assigned).await, ["T6", "T1"]);
}

#[tokio::test]
async fn counts_cover_every_view_and_each_lists_open_tasks() {
    let (_dir, store, home, work) = filled().await;
    let counts = store.task_counts().await.expect("counts");
    assert_eq!(
        (
            counts.important,
            counts.planned,
            counts.all,
            counts.completed,
            counts.assigned
        ),
        (2, 2, 4, 3, 2)
    );
    assert_eq!(counts.open_by_list.get(&home), Some(&3));
    assert_eq!(counts.open_by_list.get(&work), Some(&1));

    // A tombstoned list's tasks count nowhere.
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(
                entity(json!({ "id": "L1", "displayName": "Home", "wellknownListName": "none" })),
                None,
            )],
            cursor: Cursor {
                delta_link: "link".into(),
                replayed: false,
            },
        })
        .await
        .expect("lists");
    let counts = store.task_counts().await.expect("counts");
    assert_eq!(counts.all, 3);
    assert_eq!(counts.open_by_list.get(&work), None);
    assert_eq!(titles(&store, View::All).await, ["T1", "T2", "T3"]);
}
