//! A Microsoft Graph double for full-enumeration syncs: lists (with the
//! filtered `$expand`), each list's tasks in pages, and `$batch` GETs of
//! single tasks with our extension, chained the way Graph chains them
//! (a failed step makes the rest 424). Tests change `data` between syncs,
//! as a phone would, and mount their own mocks for writes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::Env;

#[derive(Default)]
pub struct Data {
    pub lists: Vec<Value>,
    /// Tasks by list ID.
    pub tasks: HashMap<String, Vec<Value>>,
    /// Our extension by task ID.
    pub extensions: HashMap<String, Value>,
    /// Tasks whose single GET answers 404, as if deleted after the page.
    pub gone_on_fetch: HashSet<String>,
    /// Tasks whose single GET answers 403.
    pub forbidden_on_fetch: HashSet<String>,
    /// Tasks per page; Graph's is up to 200 with the `Prefer` ms-todo sends.
    pub page_size: usize,
    /// How long each page of tasks takes, to catch a sync in the middle.
    pub tasks_delay: Option<Duration>,
}

pub struct FakeGraph {
    pub server: MockServer,
    data: Arc<Mutex<Data>>,
}

impl FakeGraph {
    /// Graph with `lists`, signed in, and `env` pointed at it.
    pub async fn start(env: &mut Env, lists: Vec<Value>) -> Self {
        let server = MockServer::start().await;
        let data = Arc::new(Mutex::new(Data {
            lists,
            page_size: 200,
            ..Data::default()
        }));
        let uri = server.uri();

        let shared = Arc::clone(&data);
        Mock::given(method("GET"))
            .and(path("/v1.0/me/todo/lists"))
            .respond_with(move |_: &Request| {
                let data = lock(&shared);
                let lists: Vec<Value> = data
                    .lists
                    .iter()
                    .map(|list| with_extension(list, &data.extensions))
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({ "value": lists }))
            })
            .mount(&server)
            .await;

        let shared = Arc::clone(&data);
        Mock::given(method("GET"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/tasks$"))
            .respond_with(move |request: &Request| {
                let data = lock(&shared);
                let list = request.url.path().split('/').nth(5).unwrap_or_default();
                let Some(tasks) = data.tasks.get(list) else {
                    return not_found();
                };
                let skip: usize = request
                    .url
                    .query_pairs()
                    .find(|(key, _)| key == "$skip")
                    .and_then(|(_, value)| value.parse().ok())
                    .unwrap_or(0);
                let end = (skip + data.page_size).min(tasks.len());
                let mut page = json!({ "value": tasks[skip.min(end)..end].to_vec() });
                if end < tasks.len() {
                    page["@odata.nextLink"] =
                        json!(format!("{uri}/v1.0/me/todo/lists/{list}/tasks?$skip={end}"));
                }
                let answer = ResponseTemplate::new(200).set_body_json(page);
                match data.tasks_delay {
                    Some(delay) => answer.set_delay(delay),
                    None => answer,
                }
            })
            .mount(&server)
            .await;

        let shared = Arc::clone(&data);
        Mock::given(method("POST"))
            .and(path("/v1.0/$batch"))
            .respond_with(move |request: &Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                let data = lock(&shared);
                let mut failed = false;
                let responses: Vec<Value> = body["requests"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|sub| {
                        let id = sub["id"].clone();
                        let (status, body) = if failed {
                            (
                                424,
                                json!({ "error": { "code": "FailedDependency", "message": "" } }),
                            )
                        } else {
                            answer_get(&data, sub["url"].as_str().unwrap_or_default())
                        };
                        failed |= status >= 400;
                        json!({ "id": id, "status": status, "headers": {}, "body": body })
                    })
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({ "responses": responses }))
            })
            .mount(&server)
            .await;

        env.graph_url = Some(format!("{}/v1.0", server.uri()));
        env.sign_in();
        Self { server, data }
    }

    /// Change what Graph holds.
    pub fn edit(&self, change: impl FnOnce(&mut Data)) {
        change(&mut lock(&self.data));
    }

    /// Every request that reached Graph with this method, whatever path.
    pub async fn requests(&self, verb: &str) -> Vec<Request> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.method.as_str() == verb)
            .collect()
    }

    /// What reached Graph as a write: every request but GETs and the
    /// `$batch` calls that carry sync's GETs.
    pub async fn writes(&self) -> Vec<Request> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| {
                request.method.as_str() != "GET" && request.url.path() != "/v1.0/$batch"
            })
            .collect()
    }

    /// The single-task GETs sent inside `$batch` calls, as their URLs.
    pub async fn batched_gets(&self) -> Vec<String> {
        self.requests("POST")
            .await
            .into_iter()
            .filter(|request| request.url.path() == "/v1.0/$batch")
            .flat_map(|request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                body["requests"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|sub| sub["url"].as_str().map(str::to_owned))
            })
            .collect()
    }
}

fn lock(data: &Mutex<Data>) -> std::sync::MutexGuard<'_, Data> {
    data.lock().unwrap_or_else(PoisonError::into_inner)
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404)
        .set_body_json(json!({ "error": { "code": "ErrorItemNotFound", "message": "not found" } }))
}

fn with_extension(entity: &Value, extensions: &HashMap<String, Value>) -> Value {
    let mut entity = entity.clone();
    let id = entity["id"].as_str().unwrap_or_default().to_owned();
    entity["extensions"] = match extensions.get(&id) {
        Some(extension) => json!([extension]),
        None => json!([]),
    };
    entity
}

// `/me/todo/lists/{l}/tasks/{t}?$expand=…`
fn answer_get(data: &Data, url: &str) -> (u16, Value) {
    let path = url.split('?').next().unwrap_or_default();
    let parts: Vec<&str> = path.split('/').collect();
    let (Some(list), Some(task)) = (parts.get(4), parts.get(6)) else {
        return (
            400,
            json!({ "error": { "code": "BadRequest", "message": url } }),
        );
    };
    if data.forbidden_on_fetch.contains(*task) {
        return (
            403,
            json!({ "error": { "code": "Forbidden", "message": "no" } }),
        );
    }
    let found = data
        .tasks
        .get(*list)
        .and_then(|tasks| tasks.iter().find(|candidate| candidate["id"] == *task));
    match found {
        Some(found) if !data.gone_on_fetch.contains(*task) => {
            (200, with_extension(found, &data.extensions))
        }
        _ => (
            404,
            json!({ "error": { "code": "ErrorItemNotFound", "message": "gone" } }),
        ),
    }
}

pub fn list(id: &str, name: &str, wellknown: &str) -> Value {
    json!({
        "@odata.etag": format!("W/\"{id}\""),
        "id": id,
        "displayName": name,
        "wellknownListName": wellknown,
        "isOwner": true,
        "isShared": false
    })
}

pub fn task(id: &str, title: &str, etag: &str) -> Value {
    json!({
        "@odata.etag": etag,
        "id": id,
        "title": title,
        "status": "notStarted",
        "importance": "normal",
        "isReminderOn": false,
        "categories": [],
        "createdDateTime": "2026-09-24T10:00:00.1234567Z",
        "lastModifiedDateTime": "2026-09-24T10:00:00.1234567Z"
    })
}
