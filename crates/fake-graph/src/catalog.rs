//! What rung 8e needs from the Graph double, mounted by
//! [`FakeGraph::accept_catalog`]:
//!
//! - list creates, renames and deletes (a delete takes the list's tasks;
//!   list PATCH ignores `If-Match`, S6);
//! - Outlook categories: names unique ignoring case (409
//!   `CategoryNameExists`), and a PATCH that ignores a new name (S7);
//! - open extensions by name other than ms-todo's own: GET, POST (an
//!   upsert), PATCH (a replace) and DELETE on a list or a task. Graph
//!   can't list them (S2, and a list's is a 404 too), and ms-todo never
//!   asks.
//!
//! The extension mocks sit above the default priority but match only a
//! name that isn't ms-todo's, which the mocks in `graph` keep answering.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Match, Mock, Request, ResponseTemplate};

use crate::graph::{Data, FakeGraph, lock, next_write, not_found};

const OURS: &str = "com.planetaryescape.mstodo";

/// Below the default, so a test's own mock wins.
const PRIORITY: u8 = 10;

/// Open extensions other than ms-todo's, by owner (a list or task ID)
/// then name.
pub type NamedExtensions = HashMap<String, HashMap<String, Value>>;

impl FakeGraph {
    /// Answer list, category and named-extension requests as Graph does.
    pub async fn accept_catalog(&self) {
        self.mount("POST", r"^/v1\.0/me/todo/lists$", create_list)
            .await;
        self.mount("PATCH", r"^/v1\.0/me/todo/lists/[^/]+$", update_list)
            .await;
        self.mount("DELETE", r"^/v1\.0/me/todo/lists/[^/]+$", delete_list)
            .await;

        let shared = Arc::clone(&self.data);
        Mock::given(method("GET"))
            .and(path("/v1.0/me/outlook/masterCategories"))
            .respond_with(move |_: &Request| {
                let data = lock(&shared);
                ResponseTemplate::new(200).set_body_json(json!({ "value": data.categories }))
            })
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;
        self.mount(
            "POST",
            r"^/v1\.0/me/outlook/masterCategories$",
            create_category,
        )
        .await;
        self.mount(
            "PATCH",
            r"^/v1\.0/me/outlook/masterCategories/[^/]+$",
            update_category,
        )
        .await;
        self.mount(
            "DELETE",
            r"^/v1\.0/me/outlook/masterCategories/[^/]+$",
            delete_category,
        )
        .await;

        for verb in ["GET", "PATCH", "DELETE"] {
            let shared = Arc::clone(&self.data);
            Mock::given(method(verb))
                .and(path_regex(r"/extensions/[^/]+$"))
                .and(NotOurs)
                .respond_with(move |request: &Request| named(&mut lock(&shared), request))
                .with_priority(1)
                .mount(&self.server)
                .await;
        }
        let shared = Arc::clone(&self.data);
        Mock::given(method("POST"))
            .and(path_regex(r"/extensions$"))
            .and(NotOurs)
            .respond_with(move |request: &Request| post_named(&mut lock(&shared), request))
            .with_priority(1)
            .mount(&self.server)
            .await;
    }

    async fn mount(
        &self,
        verb: &str,
        pattern: &str,
        answer: fn(&mut Data, &Request) -> ResponseTemplate,
    ) {
        let shared = Arc::clone(&self.data);
        Mock::given(method(verb))
            .and(path_regex(pattern))
            .respond_with(move |request: &Request| answer(&mut lock(&shared), request))
            .with_priority(PRIORITY)
            .mount(&self.server)
            .await;
    }

    /// The category called `name`, ignoring case, as Graph holds it.
    pub fn category(&self, name: &str) -> Option<Value> {
        lock(&self.data)
            .categories
            .iter()
            .find(|category| {
                category["displayName"]
                    .as_str()
                    .is_some_and(|found| found.eq_ignore_ascii_case(name))
            })
            .cloned()
    }

    /// The open extension `name` on the list or task `owner`.
    pub fn named_extension(&self, owner: &str, name: &str) -> Option<Value> {
        lock(&self.data)
            .named_extensions
            .get(owner)
            .and_then(|named| named.get(name))
            .cloned()
    }

    /// The list called `name`, as Graph holds it.
    pub fn list_named(&self, name: &str) -> Option<Value> {
        lock(&self.data)
            .lists
            .iter()
            .find(|list| list["displayName"] == name)
            .cloned()
    }
}

pub fn category(id: &str, name: &str, color: &str) -> Value {
    json!({ "id": id, "displayName": name, "color": color })
}

/// Any request whose extension name, the last segment after
/// `extensions`, isn't ms-todo's; a POST's name is in its body.
struct NotOurs;

impl Match for NotOurs {
    fn matches(&self, request: &Request) -> bool {
        let segments: Vec<&str> = request.url.path().split('/').collect();
        let name = match segments.iter().position(|segment| *segment == "extensions") {
            Some(at) if at + 1 < segments.len() => segments[at + 1].to_owned(),
            Some(_) => {
                let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
                body["extensionName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            }
            None => return false,
        };
        !name.eq_ignore_ascii_case(OURS)
            && !name.eq_ignore_ascii_case(&format!("microsoft.graph.openTypeExtension.{OURS}"))
    }
}

fn body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap_or_default()
}

fn last(request: &Request) -> String {
    request
        .url
        .path()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

pub fn create_list(data: &mut Data, request: &Request) -> ResponseTemplate {
    let sent = body(request);
    let id = format!("L-new-{}", next_write());
    let list = crate::graph::list(
        &id,
        sent["displayName"].as_str().unwrap_or_default(),
        "none",
    );
    data.lists.push(list.clone());
    data.tasks.insert(id, Vec::new());
    ResponseTemplate::new(201).set_body_json(list)
}

fn update_list(data: &mut Data, request: &Request) -> ResponseTemplate {
    let id = last(request);
    let sent = body(request);
    let Some(list) = data.lists.iter_mut().find(|list| list["id"] == id) else {
        return not_found();
    };
    if let Some(name) = sent["displayName"].as_str() {
        list["displayName"] = json!(name);
    }
    list["@odata.etag"] = json!(format!("W/\"{id}-{}\"", next_write()));
    ResponseTemplate::new(200).set_body_json(list.clone())
}

fn delete_list(data: &mut Data, request: &Request) -> ResponseTemplate {
    let id = last(request);
    let before = data.lists.len();
    data.lists.retain(|list| list["id"] != id);
    if data.lists.len() == before {
        return not_found();
    }
    data.tasks.remove(&id);
    data.extensions.remove(&id);
    data.named_extensions.remove(&id);
    ResponseTemplate::new(204)
}

fn create_category(data: &mut Data, request: &Request) -> ResponseTemplate {
    let sent = body(request);
    let name = sent["displayName"].as_str().unwrap_or_default().to_owned();
    let taken = data.categories.iter().any(|category| {
        category["displayName"]
            .as_str()
            .is_some_and(|found| found.eq_ignore_ascii_case(&name))
    });
    if taken {
        return ResponseTemplate::new(409).set_body_json(json!({ "error": {
            "code": "CategoryNameExists",
            "message": "A category with that name already exists."
        } }));
    }
    let made = json!({
        "id": format!("cat-{}", next_write()),
        "displayName": name,
        "color": sent.get("color").cloned().unwrap_or(json!("preset0"))
    });
    data.categories.push(made.clone());
    ResponseTemplate::new(201).set_body_json(made)
}

fn update_category(data: &mut Data, request: &Request) -> ResponseTemplate {
    let id = last(request);
    let sent = body(request);
    let Some(category) = data.categories.iter_mut().find(|found| found["id"] == id) else {
        return not_found();
    };
    // Graph ignores a new name (S7).
    if let Some(color) = sent.get("color") {
        category["color"] = color.clone();
    }
    ResponseTemplate::new(200).set_body_json(category.clone())
}

fn delete_category(data: &mut Data, request: &Request) -> ResponseTemplate {
    let id = last(request);
    let before = data.categories.len();
    data.categories.retain(|found| found["id"] != id);
    if data.categories.len() == before {
        return not_found();
    }
    ResponseTemplate::new(204)
}

/// The owner (a list's or task's ID) of an `…/extensions…` path.
fn owner_of(request: &Request) -> String {
    let segments: Vec<&str> = request.url.path().split('/').collect();
    segments
        .iter()
        .position(|segment| *segment == "extensions")
        .and_then(|at| segments.get(at.checked_sub(1)?))
        .copied()
        .unwrap_or_default()
        .to_owned()
}

fn stored(name: &str, fields: &Map<String, Value>) -> Value {
    let mut document = json!({
        "@odata.type": "#microsoft.graph.openTypeExtension",
        "id": name,
        "extensionName": name,
    });
    if let Some(document) = document.as_object_mut() {
        for (key, value) in fields {
            if key != "@odata.type" && key != "extensionName" && key != "id" {
                document.insert(key.clone(), value.clone());
            }
        }
    }
    document
}

fn named(data: &mut Data, request: &Request) -> ResponseTemplate {
    let owner = owner_of(request);
    let name = last(request);
    let entry = data.named_extensions.entry(owner).or_default();
    match request.method.as_str() {
        "GET" => match entry.get(&name) {
            Some(found) => ResponseTemplate::new(200).set_body_json(found),
            None => not_found(),
        },
        "PATCH" => {
            if !entry.contains_key(&name) {
                return not_found();
            }
            // PATCH replaces the whole document (S2).
            let fields = body(request).as_object().cloned().unwrap_or_default();
            let document = stored(&name, &fields);
            entry.insert(name, document.clone());
            ResponseTemplate::new(200).set_body_json(document)
        }
        _ => {
            entry.remove(&name);
            ResponseTemplate::new(204)
        }
    }
}

fn post_named(data: &mut Data, request: &Request) -> ResponseTemplate {
    let owner = owner_of(request);
    let fields = body(request).as_object().cloned().unwrap_or_default();
    let name = fields["extensionName"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let document = stored(&name, &fields);
    data.named_extensions
        .entry(owner)
        .or_default()
        .insert(name, document.clone());
    ResponseTemplate::new(201).set_body_json(document)
}
