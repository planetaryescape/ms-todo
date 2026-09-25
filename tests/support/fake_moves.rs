//! What a move needs from the Graph double (rung 5e, S14), mounted by
//! [`FakeGraph::accept_moves`]: task creates that take their checklist
//! items, their one linked resource and our extension inline, task
//! deletes, and file attachments: listing, downloading, a direct POST and
//! an upload session whose bytes go to `<uploadUrl>/content`. Every child
//! change moves the task's etag, as S1 found.
//!
//! Each mock sits below the default priority, so a test's own mock for the
//! same request wins: that's how a test makes one step fail.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, Request, ResponseTemplate};

use super::fake_graph::{Data, FakeGraph, answer_get, lock, next_write, not_found};

/// Below the default (5), so a test's own mock wins.
const PRIORITY: u8 = 10;

#[derive(Clone, Debug)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct Session {
    list: String,
    task: String,
    name: String,
    content_type: String,
    size: usize,
    received: Vec<u8>,
}

impl FakeGraph {
    /// Answer every request a move sends, as Graph does.
    pub async fn accept_moves(&self) {
        let base = self.server.uri();
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

        let shared = Arc::clone(&self.data);
        Mock::given(method("GET"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/attachments$",
            ))
            .respond_with(move |request: &Request| {
                let data = lock(&shared);
                let (list, task) = list_and_task(request);
                if find_task(&data, &list, &task).is_none() {
                    return not_found();
                }
                let listed: Vec<Value> = data
                    .attachments
                    .get(&task)
                    .map(|all| all.iter().map(metadata).collect())
                    .unwrap_or_default();
                ResponseTemplate::new(200).set_body_json(json!({ "value": listed }))
            })
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;

        let shared = Arc::clone(&self.data);
        Mock::given(method("GET"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/attachments/[^/]+/\$value$",
            ))
            .respond_with(move |request: &Request| {
                let data = lock(&shared);
                let (_, task) = list_and_task(request);
                let id = request.url.path().split('/').nth(9).unwrap_or_default();
                match data
                    .attachments
                    .get(&task)
                    .and_then(|all| all.iter().find(|found| found.id == id))
                {
                    Some(found) => ResponseTemplate::new(200).set_body_bytes(found.bytes.clone()),
                    None => not_found(),
                }
            })
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;

        let shared = Arc::clone(&self.data);
        Mock::given(method("POST"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/attachments$",
            ))
            .respond_with(move |request: &Request| post_attachment(&mut lock(&shared), request))
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;

        let shared = Arc::clone(&self.data);
        Mock::given(method("POST"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/attachments/createUploadSession$",
            ))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let (list, task) = list_and_task(request);
                if find_task(&data, &list, &task).is_none() {
                    return not_found();
                }
                let sent: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                let info = &sent["attachmentInfo"];
                let id = format!("S{}", next_write());
                data.sessions.insert(
                    id.clone(),
                    Session {
                        list: list.clone(),
                        task: task.clone(),
                        name: info["name"].as_str().unwrap_or_default().to_owned(),
                        content_type: info["contentType"].as_str().unwrap_or_default().to_owned(),
                        size: usize::try_from(info["size"].as_u64().unwrap_or(0)).unwrap_or(0),
                        received: Vec::new(),
                    },
                );
                // As S14 saw it: under users/<address>, not /me.
                let url = format!(
                    "{base}/v1.0/users/someone@example.com/todo/lists/{list}/tasks/{task}/attachmentSessions/{id}"
                );
                ResponseTemplate::new(201).set_body_json(json!({
                    "uploadUrl": url,
                    "expirationDateTime": "2099-01-01T00:00:00Z",
                    "nextExpectedRanges": ["0-"]
                }))
            })
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;

        let shared = Arc::clone(&self.data);
        Mock::given(method("PUT"))
            .and(path_regex(
                r"^/v1\.0/users/[^/]+/todo/lists/[^/]+/tasks/[^/]+/attachmentSessions/[^/]+/content$",
            ))
            .respond_with(move |request: &Request| put_chunk(&mut lock(&shared), request))
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;
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

    /// Put `bytes` on the task `task` in `list` as the file `name`, as a
    /// phone would.
    pub fn attach(&self, list: &str, task: &str, name: &str, bytes: &[u8]) {
        let mut data = lock(&self.data);
        attach(
            &mut data,
            list,
            task,
            name.to_owned(),
            "application/octet-stream".into(),
            bytes.to_vec(),
        );
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

    /// The attachments of task `task`, name and bytes.
    pub fn attachments_of(&self, task: &str) -> Vec<(String, Vec<u8>)> {
        lock(&self.data)
            .attachments
            .get(task)
            .map(|all| {
                all.iter()
                    .map(|found| (found.name.clone(), found.bytes.clone()))
                    .collect()
            })
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

/// An attachment's direct POST, with its bytes as `contentBytes`.
pub fn post_attachment(data: &mut Data, request: &Request) -> ResponseTemplate {
    let (list, task) = list_and_task(request);
    let sent: Value = serde_json::from_slice(&request.body).unwrap_or_default();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(sent["contentBytes"].as_str().unwrap_or_default())
        .unwrap_or_default();
    let name = sent["name"].as_str().unwrap_or_default().to_owned();
    let content_type = sent["contentType"].as_str().unwrap_or_default().to_owned();
    match attach(data, &list, &task, name, content_type, bytes) {
        Some(created) => ResponseTemplate::new(201).set_body_json(metadata(&created)),
        None => not_found(),
    }
}

/// One PUT of an upload session's bytes; the last one attaches the file.
pub fn put_chunk(data: &mut Data, request: &Request) -> ResponseTemplate {
    let id = request
        .url
        .path()
        .split('/')
        .nth(10)
        .unwrap_or_default()
        .to_owned();
    let Some(session) = data.sessions.get_mut(&id) else {
        return not_found();
    };
    session.received.extend_from_slice(&request.body);
    if session.received.len() < session.size {
        let next = session.received.len();
        return ResponseTemplate::new(200)
            .set_body_json(json!({ "nextExpectedRanges": [format!("{next}-")] }));
    }
    let Some(session) = data.sessions.remove(&id) else {
        return not_found();
    };
    match attach(
        data,
        &session.list,
        &session.task,
        session.name,
        session.content_type,
        session.received,
    ) {
        Some(created) => ResponseTemplate::new(201)
            .insert_header("Location", format!("attachments/{}", created.id)),
        None => not_found(),
    }
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

fn attach(
    data: &mut Data,
    list: &str,
    task: &str,
    name: String,
    content_type: String,
    bytes: Vec<u8>,
) -> Option<Attachment> {
    let found = data
        .tasks
        .get_mut(list)?
        .iter_mut()
        .find(|found| found["id"] == task)?;
    found["hasAttachments"] = json!(true);
    found["@odata.etag"] = json!(format!("W/\"{task}-{}\"", next_write()));
    let created = Attachment {
        id: format!("A{}", next_write()),
        name,
        content_type,
        bytes,
    };
    data.attachments
        .entry(task.to_owned())
        .or_default()
        .push(created.clone());
    Some(created)
}

fn metadata(attachment: &Attachment) -> Value {
    json!({
        "@odata.type": "#microsoft.graph.taskFileAttachment",
        "id": attachment.id,
        "name": attachment.name,
        "contentType": attachment.content_type,
        // Graph's size counts more than the bytes (S14: 321 for 63).
        "size": attachment.bytes.len() + 258,
        "lastModifiedDateTime": "2026-09-25T02:53:23Z"
    })
}

fn find_task<'a>(data: &'a Data, list: &str, task: &str) -> Option<&'a Value> {
    data.tasks
        .get(list)?
        .iter()
        .find(|found| found["id"] == task)
}

// `/v1.0/me/todo/lists/{list}/tasks/{task}/…`
fn list_and_task(request: &Request) -> (String, String) {
    let mut parts = request.url.path().split('/').skip(5);
    let list = parts.next().unwrap_or_default().to_owned();
    let task = parts.nth(1).unwrap_or_default().to_owned();
    (list, task)
}
