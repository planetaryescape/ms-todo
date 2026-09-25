//! A Microsoft Graph double for syncs: lists (with the filtered `$expand`),
//! each list's tasks in pages, `lists/delta` and each list's `tasks/delta`,
//! a single list's or task's GET, and `$batch` GETs of single tasks with our
//! extension, chained the way Graph chains them (a failed step makes the
//! rest 424), and writes of our extension on a list (folders) or a task
//! (My Day): PATCH replaces the document and POST upserts it, as S2 found,
//! and each moves the list's or task's etag. Tests change `data` between syncs, as a phone would,
//! and mount their own mocks for other writes.
//!
//! Delta works like Graph's as far as ms-todo can tell: a delta token
//! remembers the scope as it was when issued, a replay returns every item
//! that differs since, whole, plus `@removed` for what's gone, and a round
//! always ends with an empty page carrying the `deltaLink` (P1). A token
//! that isn't a number is "Badly formed", and one the fake doesn't know,
//! or issued for another scope, is 410 `SyncStateNotFound` (S4).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::GraphUser;

#[derive(Default)]
pub struct Data {
    pub lists: Vec<Value>,
    /// Tasks by list ID.
    pub tasks: HashMap<String, Vec<Value>>,
    /// Our extension by task or list ID.
    pub extensions: HashMap<String, Value>,
    /// Tasks whose single GET answers 404, as if deleted after the page.
    pub gone_on_fetch: HashSet<String>,
    /// Lists whose single GET answers 404, as if deleted since lists delta
    /// last looked (S4).
    pub gone_lists: HashSet<String>,
    /// Tasks whose single GET answers 403.
    pub forbidden_on_fetch: HashSet<String>,
    /// Tasks per page; Graph's is up to 200 with the `Prefer` ms-todo sends.
    pub page_size: usize,
    /// How long each page of tasks takes, to catch a sync in the middle.
    pub tasks_delay: Option<Duration>,
    /// Statuses the next delta requests of a scope (`lists`, or a list
    /// ID) fail with, one per request, with Graph's body for each.
    pub delta_errors: HashMap<String, VecDeque<u16>>,
    /// The scope each delta token was issued for, and what it held then.
    delta_tokens: HashMap<u64, (String, HashMap<String, Value>)>,
    /// Delta rounds being paged: the items, then the deltaLink.
    delta_rounds: HashMap<u64, (Vec<Value>, String)>,
    next_token: u64,
    /// Extension writes so far, to make each one's new etag.
    extension_writes: u64,
    /// File attachments by task ID (see `moves`).
    pub attachments: HashMap<String, Vec<crate::attachments::Attachment>>,
    /// Upload sessions by ID.
    pub(crate) sessions: HashMap<String, crate::attachments::Session>,
}

impl Data {
    /// Forget every delta token, as Graph sometimes does (S4): a replay of
    /// any of them is a 410.
    pub fn forget_delta_tokens(&mut self) {
        self.delta_tokens.clear();
    }

    /// Fail the next delta request of `scope` with `status`.
    pub fn fail_delta(&mut self, scope: &str, status: u16) {
        self.delta_errors
            .entry(scope.to_owned())
            .or_default()
            .push_back(status);
    }
}

pub struct FakeGraph {
    pub server: MockServer,
    pub(crate) data: Arc<Mutex<Data>>,
}

impl FakeGraph {
    /// Graph with `lists`, signed in, and `env` pointed at it.
    pub async fn start(env: &mut impl GraphUser, lists: Vec<Value>) -> Self {
        let graph = Self::serve(MockServer::start().await, lists).await;
        env.use_graph(format!("{}/v1.0", graph.server.uri()));
        env.sign_in();
        graph
    }

    /// Graph with `lists`, answering on `server`.
    pub async fn serve(server: MockServer, lists: Vec<Value>) -> Self {
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
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path_regex(r"^/v1\.0/me/todo/lists(/[^/]+/tasks)?/delta$"))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let answer = answer_delta(&mut data, &base, request);
                match (
                    data.tasks_delay,
                    request.url.path().ends_with("/tasks/delta"),
                ) {
                    (Some(delay), true) => answer.set_delay(delay),
                    _ => answer,
                }
            })
            .with_priority(1)
            .mount(&server)
            .await;

        let shared = Arc::clone(&data);
        Mock::given(method("GET"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+$"))
            .respond_with(move |request: &Request| {
                let data = lock(&shared);
                let id = request.url.path().rsplit('/').next().unwrap_or_default();
                let expanded = query(request, "$expand").is_some();
                match data.lists.iter().find(|list| list["id"] == id) {
                    Some(list) if !data.gone_lists.contains(id) => {
                        let list = if expanded {
                            with_extension(list, &data.extensions)
                        } else {
                            list.clone()
                        };
                        ResponseTemplate::new(200).set_body_json(list)
                    }
                    _ => not_found(),
                }
            })
            .mount(&server)
            .await;

        // A single task's GET, as the outbox sends while it looks for an
        // outcome. Below the default priority, so a test's own mock wins.
        let shared = Arc::clone(&data);
        Mock::given(method("GET"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+$"))
            .respond_with(move |request: &Request| {
                let relative = request.url.path().trim_start_matches("/v1.0");
                let (status, body) = answer_get(&lock(&shared), relative);
                ResponseTemplate::new(status).set_body_json(body)
            })
            .with_priority(10)
            .mount(&server)
            .await;

        // Our extension on a list: PATCH replaces the whole document (S2).
        let shared = Arc::clone(&data);
        Mock::given(method("PATCH"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/extensions/[^/]+$"))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let list = request
                    .url
                    .path()
                    .split('/')
                    .nth(5)
                    .unwrap_or_default()
                    .to_owned();
                if !data.extensions.contains_key(&list) {
                    return not_found();
                }
                let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                if body.as_object().is_none_or(serde_json::Map::is_empty) {
                    // What Graph answers an empty document.
                    return ResponseTemplate::new(400).set_body_json(json!({ "error": {
                        "code": "RequestBroker--ParseUri",
                        "message": "Resource not found for the segment 'todo'."
                    } }));
                }
                let stored = write_list_extension(&mut data, &list, body);
                ResponseTemplate::new(200).set_body_json(stored)
            })
            .mount(&server)
            .await;

        // DELETE of our extension: 204, whether or not it existed (S2).
        let shared = Arc::clone(&data);
        Mock::given(method("DELETE"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/extensions/[^/]+$"))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let list = request
                    .url
                    .path()
                    .split('/')
                    .nth(5)
                    .unwrap_or_default()
                    .to_owned();
                if data.extensions.remove(&list).is_some() {
                    bump_list_etag(&mut data, &list);
                }
                ResponseTemplate::new(204)
            })
            .mount(&server)
            .await;

        // POST of our extension upserts it (S2), with an empty 201.
        let shared = Arc::clone(&data);
        Mock::given(method("POST"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/extensions$"))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let list = request
                    .url
                    .path()
                    .split('/')
                    .nth(5)
                    .unwrap_or_default()
                    .to_owned();
                let mut body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("@odata.type");
                    fields.remove("extensionName");
                }
                write_list_extension(&mut data, &list, body);
                ResponseTemplate::new(201)
            })
            .mount(&server)
            .await;

        // Our extension on a task (My Day), as on a list: PATCH replaces
        // the document, DELETE removes it, POST upserts it, and each moves
        // the task's etag (S2).
        let shared = Arc::clone(&data);
        Mock::given(method("PATCH"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/extensions/[^/]+$",
            ))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let (list, task) = task_of(request);
                if !data.extensions.contains_key(&task) {
                    return not_found();
                }
                let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                if body.as_object().is_none_or(serde_json::Map::is_empty) {
                    return ResponseTemplate::new(400).set_body_json(json!({ "error": {
                        "code": "RequestBroker--ParseUri",
                        "message": "Resource not found for the segment 'todo'."
                    } }));
                }
                let stored = write_task_extension(&mut data, &list, &task, body);
                ResponseTemplate::new(200).set_body_json(stored)
            })
            .mount(&server)
            .await;

        let shared = Arc::clone(&data);
        Mock::given(method("DELETE"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/extensions/[^/]+$",
            ))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let (list, task) = task_of(request);
                if data.extensions.remove(&task).is_some() {
                    bump_task_etag(&mut data, &list, &task);
                }
                ResponseTemplate::new(204)
            })
            .mount(&server)
            .await;

        let shared = Arc::clone(&data);
        Mock::given(method("POST"))
            .and(path_regex(
                r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/extensions$",
            ))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let (list, task) = task_of(request);
                let mut body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("@odata.type");
                    fields.remove("extensionName");
                }
                write_task_extension(&mut data, &list, &task, body);
                ResponseTemplate::new(201)
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

        Self { server, data }
    }

    /// Answer a task's PATCH as Graph does: the fields sent, merged into
    /// the task, with a new etag; a completion gets today's
    /// `completedDateTime` at midnight UTC, the day and no time (S12).
    pub async fn accept_task_patches(&self) {
        let shared = Arc::clone(&self.data);
        Mock::given(method("PATCH"))
            .and(path_regex(r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+$"))
            .respond_with(move |request: &Request| {
                let mut data = lock(&shared);
                let mut parts = request.url.path().split('/').skip(5);
                let (Some(list), Some(_), Some(id)) = (parts.next(), parts.next(), parts.next())
                else {
                    return not_found();
                };
                let sent: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                let Some(task) = data
                    .tasks
                    .get_mut(list)
                    .and_then(|tasks| tasks.iter_mut().find(|task| task["id"] == id))
                else {
                    return not_found();
                };
                if let (Some(task), Some(sent)) = (task.as_object_mut(), sent.as_object()) {
                    task.extend(sent.clone());
                }
                if sent["status"] == "completed" {
                    let today = chrono::Utc::now().format("%Y-%m-%dT00:00:00.0000000");
                    task["completedDateTime"] =
                        json!({ "dateTime": today.to_string(), "timeZone": "UTC" });
                }
                let etag = format!("W/\"{id}-{}\"", next_write());
                task["@odata.etag"] = json!(etag);
                ResponseTemplate::new(200).set_body_json(task.clone())
            })
            .mount(&self.server)
            .await;
    }

    /// Change what Graph holds.
    pub fn edit(&self, change: impl FnOnce(&mut Data)) {
        change(&mut lock(&self.data));
    }

    /// Our extension on the list or task `id`, as Graph holds it.
    pub fn extension(&self, id: &str) -> Option<Value> {
        lock(&self.data).extensions.get(id).cloned()
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

    /// The delta requests of `scope` (`lists`, or a list ID) that started a
    /// round, in order: each one's `$deltatoken`, or `None` for a fresh
    /// start.
    pub async fn delta_starts(&self, scope: &str) -> Vec<Option<String>> {
        let wanted = match scope {
            "lists" => "/v1.0/me/todo/lists/delta".to_owned(),
            list => format!("/v1.0/me/todo/lists/{list}/tasks/delta"),
        };
        self.requests("GET")
            .await
            .into_iter()
            .filter(|request| request.url.path() == wanted)
            .filter(|request| query(request, "$skiptoken").is_none())
            .map(|request| query(&request, "$deltatoken"))
            .collect()
    }

    /// Every delta page request of any scope.
    pub async fn delta_pages(&self) -> Vec<Request> {
        self.requests("GET")
            .await
            .into_iter()
            .filter(|request| request.url.path().ends_with("/delta"))
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

/// A number for each task write, for its new etag.
pub(crate) fn next_write() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static WRITES: AtomicUsize = AtomicUsize::new(0);
    WRITES.fetch_add(1, Ordering::Relaxed) + 1
}

pub(crate) fn lock(data: &Mutex<Data>) -> std::sync::MutexGuard<'_, Data> {
    data.lock().unwrap_or_else(PoisonError::into_inner)
}

fn query(request: &Request, key: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn delta_error(status: u16) -> ResponseTemplate {
    let (code, message) = match status {
        410 => (
            "SyncStateNotFound",
            "The delta token is no longer valid, and the app must reset the sync state.",
        ),
        400 => ("BadRequest", "Badly formed token."),
        404 => (
            "ErrorItemNotFound",
            "The specified object was not found in the store.",
        ),
        _ => ("InternalServerError", "Something went wrong."),
    };
    ResponseTemplate::new(status)
        .set_body_json(json!({ "error": { "code": code, "message": message } }))
}

// `/me/todo/lists/delta` or `/me/todo/lists/{l}/tasks/delta`.
fn answer_delta(data: &mut Data, base: &str, request: &Request) -> ResponseTemplate {
    let path = request.url.path().to_owned();
    let scope = match path.split('/').nth(5) {
        Some("delta") | None => "lists".to_owned(),
        Some(list) => list.to_owned(),
    };
    if let Some(skip) = query(request, "$skiptoken") {
        return delta_page(data, base, &path, &skip);
    }
    if let Some(status) = data
        .delta_errors
        .get_mut(&scope)
        .and_then(VecDeque::pop_front)
    {
        return delta_error(status);
    }
    let since = match query(request, "$deltatoken") {
        None => HashMap::new(),
        Some(token) => {
            let Ok(token) = token.parse::<u64>() else {
                return delta_error(400);
            };
            match data.delta_tokens.get(&token) {
                Some((issued_for, then)) if *issued_for == scope => then.clone(),
                _ => return delta_error(410),
            }
        }
    };
    // A deleted list's tasks delta answers 200 with nothing (S4).
    let now: HashMap<String, Value> = match scope.as_str() {
        "lists" => data.lists.clone(),
        list => data.tasks.get(list).cloned().unwrap_or_default(),
    }
    .into_iter()
    .map(|item| (item["id"].as_str().unwrap_or_default().to_owned(), item))
    .collect();
    let mut items: Vec<Value> = now
        .iter()
        .filter(|(id, item)| since.get(*id) != Some(*item))
        .map(|(_, item)| item.clone())
        .collect();
    items.extend(
        since
            .keys()
            .filter(|id| !now.contains_key(*id))
            .map(|id| json!({ "@removed": { "reason": "deleted" }, "id": id })),
    );
    items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let token = data.next_token;
    data.next_token += 1;
    data.delta_tokens.insert(token, (scope, now));
    let round = token;
    data.delta_rounds
        .insert(round, (items, format!("{base}{path}?$deltatoken={token}")));
    delta_page(data, base, &path, &format!("{round}-0"))
}

// One page of a round, `skip` being `<round>-<offset>`. After the items
// comes an empty page with the deltaLink, as Graph often sends (P1).
fn delta_page(data: &Data, base: &str, path: &str, skip: &str) -> ResponseTemplate {
    let parsed = skip.split_once('-').and_then(|(round, offset)| {
        let offset = offset.parse::<usize>().ok()?;
        Some((
            round,
            data.delta_rounds.get(&round.parse::<u64>().ok()?)?,
            offset,
        ))
    });
    let Some((round, (items, delta_link), offset)) = parsed else {
        return ResponseTemplate::new(400).set_body_json(
            json!({ "error": { "code": "BadRequest", "message": "bad skip token" } }),
        );
    };
    if offset >= items.len() {
        return ResponseTemplate::new(200)
            .set_body_json(json!({ "value": [], "@odata.deltaLink": delta_link }));
    }
    let end = (offset + data.page_size).min(items.len());
    ResponseTemplate::new(200).set_body_json(json!({
        "value": items[offset..end].to_vec(),
        "@odata.nextLink": format!("{base}{path}?$skiptoken={round}-{end}")
    }))
}

pub(crate) fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404)
        .set_body_json(json!({ "error": { "code": "ErrorItemNotFound", "message": "not found" } }))
}

/// Store `fields` as the whole of list `list`'s extension, in Graph's
/// shape, and move the list's etag, as any extension write does (S2).
fn write_list_extension(data: &mut Data, list: &str, fields: Value) -> Value {
    let mut stored = json!({
        "extensionName": "com.planetaryescape.mstodo",
        "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo"
    });
    if let (Some(stored), Some(fields)) = (stored.as_object_mut(), fields.as_object()) {
        stored.extend(fields.clone());
    }
    data.extensions.insert(list.to_owned(), stored.clone());
    bump_list_etag(data, list);
    stored
}

fn bump_list_etag(data: &mut Data, list: &str) {
    data.extension_writes += 1;
    let etag = format!("W/\"{list}-x{}\"", data.extension_writes);
    if let Some(found) = data.lists.iter_mut().find(|found| found["id"] == list) {
        found["@odata.etag"] = json!(etag);
    }
}

/// The list and task IDs of `/v1.0/me/todo/lists/{list}/tasks/{task}/…`.
fn task_of(request: &Request) -> (String, String) {
    let mut parts = request.url.path().split('/').skip(5);
    let list = parts.next().unwrap_or_default().to_owned();
    let task = parts.nth(1).unwrap_or_default().to_owned();
    (list, task)
}

/// Store `fields` as the whole of task `task`'s extension, and move the
/// task's etag, as any extension write does (S2).
fn write_task_extension(data: &mut Data, list: &str, task: &str, fields: Value) -> Value {
    let mut stored = json!({
        "extensionName": "com.planetaryescape.mstodo",
        "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo"
    });
    if let (Some(stored), Some(fields)) = (stored.as_object_mut(), fields.as_object()) {
        stored.extend(fields.clone());
    }
    data.extensions.insert(task.to_owned(), stored.clone());
    bump_task_etag(data, list, task);
    stored
}

fn bump_task_etag(data: &mut Data, list: &str, task: &str) {
    let etag = format!("W/\"{task}-{}\"", next_write());
    if let Some(found) = data
        .tasks
        .get_mut(list)
        .and_then(|tasks| tasks.iter_mut().find(|found| found["id"] == task))
    {
        found["@odata.etag"] = json!(etag);
    }
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
pub(crate) fn answer_get(data: &Data, url: &str) -> (u16, Value) {
    let path = url.split('?').next().unwrap_or_default();
    let parts: Vec<&str> = path.split('/').collect();
    let (Some(list), Some(task)) = (parts.get(4), parts.get(6)) else {
        return (
            400,
            json!({ "error": { "code": "BadRequest", "message": url } }),
        );
    };
    if parts.get(7) == Some(&"attachments") && parts.len() == 8 {
        return match crate::attachments::listing(data, list, task) {
            Some(listed) => (200, listed),
            None => (
                404,
                json!({ "error": { "code": "ErrorItemNotFound", "message": "gone" } }),
            ),
        };
    }
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

/// Graph with one list, the default "Tasks" (`L-tasks`), holding `tasks`.
pub async fn graph_with_tasks(env: &mut impl GraphUser, tasks: Vec<Value>) -> FakeGraph {
    let graph = FakeGraph::start(env, vec![list("L-tasks", "Tasks", "defaultList")]).await;
    graph.edit(|data| {
        data.tasks.insert("L-tasks".into(), tasks);
    });
    graph
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
