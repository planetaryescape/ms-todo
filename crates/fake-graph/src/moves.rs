//! What a move needs from the Graph double (rung 5e, S14), mounted by
//! [`FakeGraph::accept_moves`]: task creates that take their checklist
//! items, their one linked resource and our extension inline, task
//! deletes, and file attachments (`crate::attachments`). Every child
//! change moves the task's etag, as S1 found.
//!
//! Each mock sits below the default priority, so a test's own mock for the
//! same request wins: that's how a test makes one step fail.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, Request, ResponseTemplate};

pub use crate::attachments::{Attachment, post_attachment, put_chunk};
use crate::attachments::{find_task, list_and_task};
use crate::graph::{Data, FakeGraph, answer_get, lock, next_write, not_found};

/// Below the default (5), so a test's own mock wins.
const PRIORITY: u8 = 10;

impl FakeGraph {
    /// Answer every request a move sends, as Graph does.
    pub async fn accept_moves(&self) {
        let shared = Arc::clone(&self.data);
        Mock::given(method("POST"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/tasks$"))
            .respond_with(move |request: &Request| create_task(&mut lock(&shared), request))
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;

        let shared = Arc::clone(&self.data);
        Mock::given(method("DELETE"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+$"))
            .respond_with(move |request: &Request| delete_task(&mut lock(&shared), request))
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;

        self.accept_attachments().await;
    }

    /// Make the next `verb` request to a path matching `path` take `delay`
    /// to answer, having first done what `effect` does to what Graph holds
    /// (`None`: nothing, as a request that never arrived). Stopping the
    /// daemon meanwhile is a crash at that step.
    pub async fn stall(&self, verb: &str, path: &str, effect: Option<Handler>, delay: Duration) {
        let shared = Arc::clone(&self.data);
        Mock::given(method(verb))
            .and(path_regex(path))
            .respond_with(move |request: &Request| {
                let answer = match effect {
                    Some(effect) => effect(&mut lock(&shared), request),
                    None => ResponseTemplate::new(503),
                };
                answer.set_delay(delay)
            })
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&self.server)
            .await;
    }

    /// Answer GETs of `path` from what Graph holds, and right after the
    /// `nth` one (from 1), change what it holds with `edit`: an edit on
    /// another device landing between two reads.
    pub async fn edit_after_gets(&self, path: &str, nth: usize, edit: fn(&mut Data)) {
        let shared = Arc::clone(&self.data);
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path_regex(path))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let relative = request.url.path().trim_start_matches("/v1.0");
                let (status, body) = answer_get(&data, relative);
                let count = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if count == nth {
                    edit(&mut data);
                }
                ResponseTemplate::new(status).set_body_json(body)
            })
            .with_priority(1)
            .mount(&self.server)
            .await;
    }

    /// Refuse a task PATCH whose `If-Match` isn't the task's etag, with
    /// Graph's 412 (S6); others go on to `accept_task_patches`. Opt-in,
    /// since most tests change Graph without the daemon re-reading it.
    pub async fn refuse_stale_task_patches(&self) {
        Mock::given(method("PATCH"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+$"))
            .and(StaleEtag(Arc::clone(&self.data)))
            .respond_with(|request: &Request| {
                // The matcher saw it stale; answer as Graph does.
                crate::children::precondition(&Value::Null, request)
                    .unwrap_or_else(|| ResponseTemplate::new(412))
            })
            .with_priority(1)
            .mount(&self.server)
            .await;
    }

    /// Make the next task create keep less than it was sent: Graph stores
    /// the task, then `lose` changes what it holds, as a lossy copy would.
    pub async fn lossy_create(&self, lose: fn(&mut Value)) {
        let shared = Arc::clone(&self.data);
        Mock::given(method("POST"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/tasks$"))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let answer = create_task(&mut data, request);
                let (list, _) = list_and_task(request);
                if let Some(created) = data.tasks.get_mut(&list).and_then(|tasks| tasks.last_mut())
                {
                    lose(created);
                }
                answer
            })
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&self.server)
            .await;
    }

    /// The task `task` in `list` as Graph holds it, without its extension.
    pub fn task(&self, list: &str, task: &str) -> Option<Value> {
        find_task(&lock(&self.data), list, task).cloned()
    }

    /// The tasks in `list`, as Graph holds them.
    pub fn tasks_in(&self, list: &str) -> Vec<Value> {
        lock(&self.data)
            .tasks
            .get(list)
            .cloned()
            .unwrap_or_default()
    }
}

/// What one request does to what Graph holds, and its answer.
pub type Handler = fn(&mut Data, &Request) -> ResponseTemplate;

/// A task's DELETE; a task already gone is a 404.
pub fn delete_task(data: &mut Data, request: &Request) -> ResponseTemplate {
    let (list, task) = list_and_task(request);
    let Some(tasks) = data.tasks.get_mut(&list) else {
        return not_found();
    };
    let before = tasks.len();
    tasks.retain(|found| found["id"] != task.as_str());
    if tasks.len() == before {
        return not_found();
    }
    data.attachments.remove(&task);
    data.extensions.remove(&task);
    ResponseTemplate::new(204)
}

/// A POST of a task: Graph's defaults, then what was sent, children with
/// IDs, our extension kept apart and given back inline (S13).
pub fn create_task(data: &mut Data, request: &Request) -> ResponseTemplate {
    let (list, _) = list_and_task(request);
    if !data.tasks.contains_key(&list) {
        return not_found();
    }
    let mut sent: Value = serde_json::from_slice(&request.body).unwrap_or_default();
    if sent["linkedResources"]
        .as_array()
        .is_some_and(|links| links.len() > 1)
    {
        return ResponseTemplate::new(400).set_body_json(json!({ "error": {
            "code": "invalidRequest",
            "message": "Only one item in linkedResources is allowed."
        } }));
    }
    let id = format!("C{}", next_write());
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string();
    let mut task = json!({
        "id": id,
        "@odata.etag": format!("W/\"{id}-1\""),
        "status": "notStarted",
        "importance": "normal",
        "isReminderOn": false,
        "hasAttachments": false,
        "categories": [],
        "createdDateTime": now,
        "lastModifiedDateTime": now,
    });
    let extension = sent
        .as_object_mut()
        .and_then(|fields| fields.remove("extensions"))
        .and_then(|all| all.as_array().and_then(|all| all.first().cloned()));
    if let (Some(task), Some(sent)) = (task.as_object_mut(), sent.as_object()) {
        for (key, value) in sent {
            // Graph stamps its own; a copy's createdDateTime is the move's.
            if key != "createdDateTime" {
                task.insert(key.clone(), value.clone());
            }
        }
    }
    for key in ["checklistItems", "linkedResources"] {
        if let Some(items) = task[key].as_array_mut() {
            for item in items {
                item["id"] = json!(format!("{key}-{}", next_write()));
            }
        }
    }
    let mut stored = None;
    if let Some(mut extension) = extension {
        if let Some(fields) = extension.as_object_mut() {
            fields.remove("@odata.type");
            fields.insert(
                "id".into(),
                json!("microsoft.graph.openTypeExtension.com.planetaryescape.mstodo"),
            );
        }
        data.extensions.insert(id.clone(), extension.clone());
        stored = Some(extension);
    }
    data.tasks.entry(list).or_default().push(task.clone());
    if let Some(extension) = stored {
        task["extensions"] = json!([extension]);
    }
    ResponseTemplate::new(201).set_body_json(task)
}

/// A task PATCH whose `If-Match` isn't the task's current etag.
struct StaleEtag(Arc<std::sync::Mutex<Data>>);

impl wiremock::Match for StaleEtag {
    fn matches(&self, request: &Request) -> bool {
        let data = lock(&self.0);
        let (list, task) = list_and_task(request);
        find_task(&data, &list, &task)
            .is_some_and(|task| crate::children::precondition(task, request).is_some())
    }
}
