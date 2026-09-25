//! A task's steps (`checklistItems`) and link (`linkedResources`), mounted
//! by [`FakeGraph::accept_children`], as S1, S6, S14 and S15 found them:
//!
//! - every write moves the task's etag, and a PATCH or DELETE whose
//!   `If-Match` isn't the task's etag is 412;
//! - a step PATCH without `isChecked` unchecks it, and unchecking clears
//!   `checkedDateTime`;
//! - a link needs `applicationName`, a task takes one link, and a link
//!   PATCH ignores a null or empty value;
//! - an empty collection is left out of the task.
//!
//! Each mock sits below the default priority, so a test's own mock for
//! the same request wins: that's how a test loses an answer.

use std::sync::Arc;

use serde_json::{Map, Value, json};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, Request, ResponseTemplate};

use crate::graph::{Data, FakeGraph, lock, next_write, not_found};

const PRIORITY: u8 = 10;
const STEPS: &str = "checklistItems";
const LINKS: &str = "linkedResources";

impl FakeGraph {
    /// Answer step and link writes as Graph does.
    pub async fn accept_children(&self) {
        for collection in [STEPS, LINKS] {
            let shared = Arc::clone(&self.data);
            Mock::given(method("POST"))
                .and(path_regex(format!(
                    r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/{collection}$"
                )))
                .respond_with(move |request: &Request| {
                    create(&mut lock(&shared), request, collection)
                })
                .with_priority(PRIORITY)
                .mount(&self.server)
                .await;
            let shared = Arc::clone(&self.data);
            Mock::given(method("PATCH"))
                .and(path_regex(format!(
                    r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/{collection}/[^/]+$"
                )))
                .respond_with(move |request: &Request| {
                    update(&mut lock(&shared), request, collection)
                })
                .with_priority(PRIORITY)
                .mount(&self.server)
                .await;
            let shared = Arc::clone(&self.data);
            Mock::given(method("DELETE"))
                .and(path_regex(format!(
                    r"^/v1\.0/me/todo/lists/[^/]+/tasks/[^/]+/{collection}/[^/]+$"
                )))
                .respond_with(move |request: &Request| {
                    delete(&mut lock(&shared), request, collection)
                })
                .with_priority(PRIORITY)
                .mount(&self.server)
                .await;
        }
    }

    /// The steps or link of task `id` in list `list`, as Graph holds them.
    pub fn children(&self, list: &str, id: &str, collection: &str) -> Vec<Value> {
        let data = lock(&self.data);
        data.tasks
            .get(list)
            .and_then(|tasks| tasks.iter().find(|task| task["id"] == id))
            .and_then(|task| task.get(collection))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

fn create(data: &mut Data, request: &Request, collection: &str) -> ResponseTemplate {
    let (list, task, _) = parts(request);
    let body: Map<String, Value> = serde_json::from_slice(&request.body).unwrap_or_default();
    let Some(found) = task_mut(data, &list, &task) else {
        return not_found();
    };
    let mut item = body;
    if collection == LINKS {
        if !item.contains_key("applicationName") {
            return bad_request(
                "invalidRequest",
                "The property 'applicationName' is required when creating the linked resource",
            );
        }
        if found
            .get(LINKS)
            .and_then(Value::as_array)
            .is_some_and(|links| !links.is_empty())
        {
            return bad_request("invalidRequest", "Linked Resource already exists.");
        }
    } else {
        item.insert("createdDateTime".into(), json!(now()));
        let checked = item.get("isChecked") == Some(&Value::Bool(true));
        item.insert("isChecked".into(), json!(checked));
        if checked && !item.contains_key("checkedDateTime") {
            item.insert("checkedDateTime".into(), json!(now()));
        }
    }
    item.insert("id".into(), json!(format!("{collection}-{}", next_write())));
    let mut items = found
        .get(collection)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items.push(Value::Object(item.clone()));
    found[collection] = Value::Array(items);
    bump(found, &task);
    ResponseTemplate::new(201).set_body_json(item)
}

fn update(data: &mut Data, request: &Request, collection: &str) -> ResponseTemplate {
    let (list, task, id) = parts(request);
    let sent: Map<String, Value> = serde_json::from_slice(&request.body).unwrap_or_default();
    let Some(found) = task_mut(data, &list, &task) else {
        return not_found();
    };
    if let Some(refused) = precondition(found, request) {
        return refused;
    }
    let Some(item) = child_mut(found, collection, &id) else {
        return not_found();
    };
    if collection == STEPS {
        // S1: a step PATCH without isChecked unchecks it.
        let checked = sent.get("isChecked") == Some(&Value::Bool(true));
        let was = item.get("isChecked") == Some(&Value::Bool(true));
        item.insert("isChecked".into(), json!(checked));
        if !checked {
            item.remove("checkedDateTime");
        } else if !was {
            item.insert("checkedDateTime".into(), json!(now()));
        }
        if let Some(name) = sent.get("displayName") {
            item.insert("displayName".into(), name.clone());
        }
    } else {
        for (key, value) in &sent {
            // S15: a null or empty value is ignored, not a clear.
            if !value.is_null() && value != "" {
                item.insert(key.clone(), value.clone());
            }
        }
    }
    let answer = item.clone();
    bump(found, &task);
    ResponseTemplate::new(200).set_body_json(answer)
}

fn delete(data: &mut Data, request: &Request, collection: &str) -> ResponseTemplate {
    let (list, task, id) = parts(request);
    let Some(found) = task_mut(data, &list, &task) else {
        return not_found();
    };
    if let Some(refused) = precondition(found, request) {
        return refused;
    }
    let mut items = found
        .get(collection)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let before = items.len();
    items.retain(|item| item["id"] != id.as_str());
    if items.len() == before {
        return not_found();
    }
    if items.is_empty() {
        if let Some(found) = found.as_object_mut() {
            found.remove(collection);
        }
    } else {
        found[collection] = Value::Array(items);
    }
    bump(found, &task);
    ResponseTemplate::new(204)
}

/// A 412 when `If-Match` isn't the task's etag (S6).
pub(crate) fn precondition(task: &Value, request: &Request) -> Option<ResponseTemplate> {
    let sent = request.headers.get("if-match")?.to_str().ok()?;
    (task["@odata.etag"] != sent).then(|| {
        ResponseTemplate::new(412).set_body_json(json!({ "error": {
            "code": "ErrorIrresolvableConflict",
            "message": "A precondition provided in the request (such as an if-match header) does \
                        not match the resource's current state."
        } }))
    })
}

fn task_mut<'a>(data: &'a mut Data, list: &str, task: &str) -> Option<&'a mut Value> {
    data.tasks
        .get_mut(list)?
        .iter_mut()
        .find(|found| found["id"] == task)
}

fn child_mut<'a>(
    task: &'a mut Value,
    collection: &str,
    id: &str,
) -> Option<&'a mut Map<String, Value>> {
    task.get_mut(collection)?
        .as_array_mut()?
        .iter_mut()
        .find(|item| item["id"] == id)?
        .as_object_mut()
}

fn bump(task: &mut Value, id: &str) {
    task["@odata.etag"] = json!(format!("W/\"{id}-{}\"", next_write()));
    task["lastModifiedDateTime"] = json!(now());
}

// `/v1.0/me/todo/lists/{list}/tasks/{task}/{collection}[/{id}]`
fn parts(request: &Request) -> (String, String, String) {
    let parts: Vec<&str> = request.url.path().split('/').collect();
    let at = |index: usize| parts.get(index).copied().unwrap_or_default().to_owned();
    (at(5), at(7), at(9))
}

fn bad_request(code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(400)
        .set_body_json(json!({ "error": { "code": code, "message": message } }))
}

fn now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}
