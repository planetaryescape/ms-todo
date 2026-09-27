//! Task vectors against a real SQLite file (migration 0008): which tasks
//! need embedding, what a save and a prune do, and the candidates a
//! semantic search ranks, with how many are pending.

use ms_todo_store::{
    Cursor, Embedded, Entity, Hydration, ListsPass, SeenTask, StatusFilter, Store, TaskScope,
    TasksPass, tasks_scope, text_hash,
};
use serde_json::{Value, json};

const MODEL: &str = "model@1";

fn entity(value: Value) -> Entity {
    value.as_object().cloned().expect("object")
}

fn task(id: &str, title: &str) -> Entity {
    entity(json!({
        "id": id, "title": title, "status": "notStarted", "importance": "normal",
        "@odata.etag": format!("W/\"{id}\""), "createdDateTime": "2026-09-24T10:00:00.0000000Z"
    }))
}

async fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("ms-todo.db"))
        .await
        .expect("open");
    (dir, store)
}

/// List L1 "Home", returning its local ID.
async fn with_list(store: &Store) -> String {
    let rev = store.local_rev().await.expect("rev");
    let list = entity(json!({ "id": "L1", "displayName": "Home", "wellknownListName": "none" }));
    store
        .apply_lists(ListsPass {
            rev,
            lists: vec![(list, None)],
            cursor: whole(),
        })
        .await
        .expect("lists");
    store.lists().await.expect("lists")[0].local_id.clone()
}

fn whole() -> Cursor {
    Cursor {
        delta_link: "link".into(),
        replayed: false,
    }
}

/// Sync `tasks` into L1 as Graph's whole list: any other task there is
/// tombstoned.
async fn sync(store: &Store, list_local_id: &str, tasks: Vec<Entity>) {
    let rev = store.local_rev().await.expect("rev");
    store
        .apply_tasks(TasksPass {
            scope: tasks_scope("L1"),
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

/// Embed every job with a vector made from its title's length, so a test
/// can tell vectors apart.
async fn embed_all(store: &Store, model: &str) -> usize {
    let jobs = store.embedding_jobs(model).await.expect("jobs");
    let embedded: Vec<Embedded> = jobs
        .iter()
        .map(|job| Embedded {
            local_id: job.local_id.clone(),
            text_hash: job.text_hash.clone(),
            vector: vec![job.text.len() as f32, 1.0],
        })
        .collect();
    store.save_embeddings(model, &embedded).await.expect("save");
    jobs.len()
}

fn scope(list: &str) -> TaskScope<'_> {
    TaskScope {
        list_local_id: Some(list),
        status: StatusFilter::Open,
        view: None,
    }
}

#[tokio::test]
async fn only_new_and_changed_tasks_are_embedded() {
    let (_dir, store) = open().await;
    let list = with_list(&store).await;
    let mut noted = task("T2", "Call the broker");
    noted.insert(
        "body".into(),
        json!({ "content": "about the insurance", "contentType": "text" }),
    );
    sync(&store, &list, vec![task("T1", "Buy milk"), noted.clone()]).await;

    let jobs = store.embedding_jobs(MODEL).await.expect("jobs");
    let mut texts: Vec<&str> = jobs.iter().map(|job| job.text.as_str()).collect();
    texts.sort_unstable();
    // The text FTS indexes: the title, then the notes.
    assert_eq!(texts, ["Buy milk", "Call the broker\nabout the insurance"]);
    assert_eq!(jobs[0].text_hash, text_hash(&jobs[0].text));
    assert_eq!(embed_all(&store, MODEL).await, 2);
    assert!(store.embedding_jobs(MODEL).await.expect("jobs").is_empty());
    assert_eq!(store.embedding_counts(MODEL).await.expect("counts"), (2, 0));

    // A change that isn't to the text needs nothing.
    let mut important = task("T1", "Buy milk");
    important.insert("importance".into(), json!("high"));
    sync(&store, &list, vec![important, noted.clone()]).await;
    assert!(store.embedding_jobs(MODEL).await.expect("jobs").is_empty());

    // A new title does.
    sync(&store, &list, vec![task("T1", "Buy oat milk"), noted]).await;
    let jobs = store.embedding_jobs(MODEL).await.expect("jobs");
    assert_eq!(
        jobs.iter().map(|job| job.text.as_str()).collect::<Vec<_>>(),
        ["Buy oat milk"]
    );
    assert_eq!(store.embedding_counts(MODEL).await.expect("counts"), (1, 1));

    // Another model's vectors are all out of date, and pruned.
    assert_eq!(
        store.embedding_jobs("model@2").await.expect("jobs").len(),
        2
    );
    assert_eq!(store.prune_embeddings("model@2").await.expect("prune"), 2);
}

#[tokio::test]
async fn candidates_carry_their_vectors_and_count_what_is_pending() {
    let (_dir, store) = open().await;
    let list = with_list(&store).await;
    let mut done = task("T3", "Old claim");
    done.insert("status".into(), json!("completed"));
    sync(
        &store,
        &list,
        vec![task("T1", "Buy milk"), task("T2", "Renew insurance"), done],
    )
    .await;
    let candidates = store
        .semantic_candidates(MODEL, &scope(&list))
        .await
        .expect("candidates");
    assert!(candidates.tasks.is_empty());
    assert_eq!(candidates.pending, 2, "open tasks only");

    embed_all(&store, MODEL).await;
    // T1 renamed: its old vector still ranks it until it's embedded again.
    sync(
        &store,
        &list,
        vec![task("T1", "Buy oat milk"), task("T2", "Renew insurance")],
    )
    .await;
    let candidates = store
        .semantic_candidates(MODEL, &scope(&list))
        .await
        .expect("candidates");
    assert_eq!(candidates.pending, 1);
    let mut found: Vec<(&str, &str, Vec<f32>)> = candidates
        .tasks
        .iter()
        .map(|candidate| {
            (
                candidate.task.title.as_str(),
                candidate.list_name.as_str(),
                candidate.vector.clone(),
            )
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(
        found,
        [
            ("Buy oat milk", "Home", vec![8.0, 1.0]),
            ("Renew insurance", "Home", vec![15.0, 1.0])
        ]
    );

    // The tombstoned T3's vector is pruned.
    assert_eq!(store.prune_embeddings(MODEL).await.expect("prune"), 1);
    // Another model's search has no vectors yet.
    let other = store
        .semantic_candidates("model@2", &scope(&list))
        .await
        .expect("candidates");
    assert!(other.tasks.is_empty());
    assert_eq!(other.pending, 2);
}

#[tokio::test]
async fn a_vector_for_a_task_tombstoned_meanwhile_is_not_saved() {
    let (_dir, store) = open().await;
    let list = with_list(&store).await;
    sync(&store, &list, vec![task("T1", "Buy milk")]).await;
    let jobs = store.embedding_jobs(MODEL).await.expect("jobs");
    // The task goes before its vector is saved.
    sync(&store, &list, Vec::new()).await;
    let late: Vec<Embedded> = jobs
        .into_iter()
        .map(|job| Embedded {
            local_id: job.local_id,
            text_hash: job.text_hash,
            vector: vec![1.0, 0.0],
        })
        .collect();
    store.save_embeddings(MODEL, &late).await.expect("save");
    assert_eq!(store.prune_embeddings(MODEL).await.expect("prune"), 0);
}
