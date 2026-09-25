//! A task's file attachments, mounted by [`FakeGraph::accept_attachments`],
//! as S1, S14 and S16 found them: listing their metadata (never the
//! bytes), downloading `$value`, a direct POST, an upload session whose
//! bytes go to `<uploadUrl>/content` in order, and DELETE. Adding or
//! deleting one moves the task's etag and sets `hasAttachments`.
//!
//! An upload session answers each PUT but the last with
//! `nextExpectedRanges` as To Do sends it (`["3276800"]`, S16), a range it
//! already has or one past what it has with 400 `InvalidStart`, and the
//! last with 201 and the attachment's URL in `Location`.
//!
//! Each mock sits below the default priority, so a test's own mock for the
//! same request wins: that's how a test makes one step fail.

use std::sync::Arc;

use base64::Engine;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path_regex};
use wiremock::{Mock, Request, ResponseTemplate};

use crate::graph::{Data, FakeGraph, lock, next_write, not_found};
use crate::moves::Handler;

/// Below the default (5), so a test's own mock wins.
const PRIORITY: u8 = 10;

/// An upload session's PUTs.
pub const SESSION_PUTS: &str =
    r"^/v1\.0/users/[^/]+/todo/lists/[^/]+/tasks/[^/]+/attachmentSessions/[^/]+/content$";

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
    /// Answer attachment requests as Graph does.
    pub async fn accept_attachments(&self) {
        let base = self.server.uri();
        let shared = Arc::clone(&self.data);
        Mock::given(method("GET"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/attachments$",
            ))
            .respond_with(move |request: &Request| {
                let data = lock(&shared);
                let (list, task) = list_and_task(request);
                match listing(&data, &list, &task) {
                    Some(listed) => ResponseTemplate::new(200).set_body_json(listed),
                    None => not_found(),
                }
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
                let id = attachment_id(request);
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
        Mock::given(method("DELETE"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/attachments/[^/]+$",
            ))
            .respond_with(move |request: &Request| delete_attachment(&mut lock(&shared), request))
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
            .and(path_regex(SESSION_PUTS))
            .respond_with(move |request: &Request| put_chunk(&mut lock(&shared), request))
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;
    }

    /// Make the next `verb` request to a path matching `path` do what
    /// `effect` does to what Graph holds, and then answer `status`: an
    /// answer lost after the request landed.
    pub async fn lose_answer(&self, verb: &str, path: &str, effect: Handler, status: u16) {
        let shared = Arc::clone(&self.data);
        Mock::given(method(verb))
            .and(path_regex(path))
            .respond_with(move |request: &Request| {
                effect(&mut lock(&shared), request);
                ResponseTemplate::new(status)
            })
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&self.server)
            .await;
    }

    /// Make the upload-session PUT of `range` (its `Content-Range`, as
    /// `bytes 0-9/10`) land, and then answer `status`: a lost answer to
    /// that one chunk.
    pub async fn lose_chunk_answer(&self, range: &str, status: u16) {
        let shared = Arc::clone(&self.data);
        Mock::given(method("PUT"))
            .and(path_regex(SESSION_PUTS))
            .and(header("content-range", range))
            .respond_with(move |request: &Request| {
                put_chunk(&mut lock(&shared), request);
                ResponseTemplate::new(status)
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

    /// Take the attachment `name` off task `task` in `list`, as a phone
    /// would.
    pub fn detach(&self, list: &str, task: &str, name: &str) {
        let mut data = lock(&self.data);
        let id = data
            .attachments
            .get(task)
            .and_then(|all| all.iter().find(|found| found.name == name))
            .map(|found| found.id.clone());
        if let Some(id) = id {
            remove(&mut data, list, task, &id);
        }
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

/// An attachment's direct POST, with its bytes as `contentBytes`. Graph
/// echoes the bytes in its answer (S16).
pub fn post_attachment(data: &mut Data, request: &Request) -> ResponseTemplate {
    let (list, task) = list_and_task(request);
    let sent: Value = serde_json::from_slice(&request.body).unwrap_or_default();
    let encoded = sent["contentBytes"].as_str().unwrap_or_default().to_owned();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .unwrap_or_default();
    let name = sent["name"].as_str().unwrap_or_default().to_owned();
    let content_type = sent["contentType"].as_str().unwrap_or_default().to_owned();
    match attach(data, &list, &task, name, content_type, bytes) {
        Some(created) => {
            let mut answer = metadata(&created);
            answer["contentBytes"] = json!(encoded);
            answer["@odata.context"] = json!("https://graph.microsoft.com/v1.0/$metadata#…");
            ResponseTemplate::new(201).set_body_json(answer)
        }
        None => not_found(),
    }
}

/// One PUT of an upload session's bytes, which must start where the
/// session has got to; the last one attaches the file.
pub fn put_chunk(data: &mut Data, request: &Request) -> ResponseTemplate {
    let path = request.url.path().to_owned();
    let id = path.split('/').nth(10).unwrap_or_default().to_owned();
    let Some(session) = data.sessions.get_mut(&id) else {
        return not_found();
    };
    let start = request
        .headers
        .get("content-range")
        .and_then(|range| range.to_str().ok())
        .and_then(|range| range.strip_prefix("bytes "))
        .and_then(|range| range.split('-').next())
        .and_then(|start| start.parse::<usize>().ok());
    if start != Some(session.received.len()) {
        return ResponseTemplate::new(400).set_body_json(json!({ "error": {
            "code": "invalidRequest",
            "message": "Invalid request",
            "innerError": { "code": "InvalidStart" }
        } }));
    }
    session.received.extend_from_slice(&request.body);
    if session.received.len() < session.size {
        let next = session.received.len();
        return ResponseTemplate::new(200).set_body_json(json!({
            "expirationDateTime": "2099-01-01T00:00:00Z",
            "nextExpectedRanges": [next.to_string()]
        }));
    }
    let Some(session) = data.sessions.remove(&id) else {
        return not_found();
    };
    let (list, task) = (session.list.clone(), session.task.clone());
    match attach(
        data,
        &list,
        &task,
        session.name,
        session.content_type,
        session.received,
    ) {
        Some(created) => {
            let origin = request.url.origin().ascii_serialization();
            let location = format!(
                "{origin}/v1.0/users/someone@example.com/todo/lists/{list}/tasks/{task}/attachments/{}",
                created.id
            );
            ResponseTemplate::new(201).insert_header("Location", location)
        }
        None => not_found(),
    }
}

/// An attachment's DELETE. Graph ignores `If-Match` on it (S16).
pub fn delete_attachment(data: &mut Data, request: &Request) -> ResponseTemplate {
    let (list, task) = list_and_task(request);
    let id = attachment_id(request);
    if remove(data, &list, &task, &id) {
        ResponseTemplate::new(204)
    } else {
        not_found()
    }
}

/// `GET …/attachments` of a task: its attachments' metadata, or `None`
/// if the task isn't there. `$batch` answers from it too.
pub(crate) fn listing(data: &Data, list: &str, task: &str) -> Option<Value> {
    find_task(data, list, task)?;
    let listed: Vec<Value> = data
        .attachments
        .get(task)
        .map(|all| all.iter().map(metadata).collect())
        .unwrap_or_default();
    Some(json!({ "value": listed }))
}

pub(crate) fn attach(
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

/// Take attachment `id` off the task; false if it isn't there.
fn remove(data: &mut Data, list: &str, task: &str, id: &str) -> bool {
    let Some(all) = data.attachments.get_mut(task) else {
        return false;
    };
    let before = all.len();
    all.retain(|found| found.id != id);
    if all.len() == before {
        return false;
    }
    let left = !all.is_empty();
    if let Some(found) = data
        .tasks
        .get_mut(list)
        .and_then(|tasks| tasks.iter_mut().find(|found| found["id"] == task))
    {
        found["hasAttachments"] = json!(left);
        found["@odata.etag"] = json!(format!("W/\"{task}-{}\"", next_write()));
    }
    true
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

pub(crate) fn find_task<'a>(data: &'a Data, list: &str, task: &str) -> Option<&'a Value> {
    data.tasks
        .get(list)?
        .iter()
        .find(|found| found["id"] == task)
}

// `/v1.0/me/todo/lists/{list}/tasks/{task}/…`
pub(crate) fn list_and_task(request: &Request) -> (String, String) {
    let mut parts = request.url.path().split('/').skip(5);
    let list = parts.next().unwrap_or_default().to_owned();
    let task = parts.nth(1).unwrap_or_default().to_owned();
    (list, task)
}

// `/v1.0/me/todo/lists/{list}/tasks/{task}/attachments/{id}…`
fn attachment_id(request: &Request) -> String {
    request
        .url
        .path()
        .split('/')
        .nth(9)
        .unwrap_or_default()
        .to_owned()
}
