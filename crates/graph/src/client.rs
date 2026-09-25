//! The Graph HTTP client (docs/blueprint/03-graph-provider.md#http-client):
//! reqwest with rustls, a 60-second timeout per request, `decide_retry`, one
//! refresh on 401, a concurrency cap of 4, and pagination to the end.
//! Only the daemon uses this (D-031, `tests/workspace_boundaries.rs`).

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, IF_MATCH};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::Semaphore;

use crate::api_error::ApiError;
use crate::auth::{AuthError, Authenticator};
use crate::error::GraphError;
use crate::retry::{self, RetryDecision};

/// Every request's deadline (vault: `Deadlines Bound Stalls, Not Work`).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// S9: 4 parallel requests are fine against To Do, 8 get 429s.
const MAX_CONCURRENT_REQUESTS: usize = 4;

/// Page size asked for on every page. P1: Graph doesn't carry
/// `odata.maxpagesize` in the `nextLink`, so it's resent each time, or pages
/// after the first drop back to 50.
const PREFER_PAGE_SIZE: &str = "odata.maxpagesize=200";

/// 1,000 pages is at least 50,000 items at Graph's smallest default page.
/// Hitting it means a paging loop, not a real list, so it's an error.
const MAX_PAGES: usize = 1_000;

/// A JSON object from Graph, every field kept.
pub type Entity = Map<String, Value>;

/// One page of a collection or a delta round. Short or empty pages aren't
/// the end (P1): only a missing `nextLink` is, and the last page of a delta
/// round carries the `deltaLink` instead.
#[derive(Deserialize)]
struct Page {
    value: Vec<Entity>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    delta_link: Option<String>,
}

/// A whole delta round (docs/blueprint/04-sync-cache.md#delta-sync).
#[derive(Debug)]
pub struct Delta {
    /// Every entity that changed, whole, and `@removed` entries for what
    /// was deleted. From a fresh start, every entity in the scope.
    pub items: Vec<Entity>,
    /// Where the next round starts.
    pub delta_link: String,
}

pub struct GraphClient {
    http: reqwest::Client,
    base: Url,
    auth: Arc<Authenticator>,
    permits: Semaphore,
    backoff_unit: Duration,
    max_pages: usize,
}

impl GraphClient {
    /// `graph_base` is the v1.0 root, e.g. `https://graph.microsoft.com/v1.0`.
    pub fn new(auth: Arc<Authenticator>, graph_base: &str) -> Result<Self, GraphError> {
        let base = Url::parse(graph_base.trim_end_matches('/')).map_err(|error| {
            GraphError::InvalidInput(format!("invalid Graph URL {graph_base}: {error}"))
        })?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("ms-todo/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base,
            auth,
            permits: Semaphore::new(MAX_CONCURRENT_REQUESTS),
            backoff_unit: Duration::from_secs(1),
            max_pages: MAX_PAGES,
        })
    }

    /// Scale retry backoff. Only tests change this.
    #[doc(hidden)]
    pub fn with_backoff_unit(mut self, unit: Duration) -> Self {
        self.backoff_unit = unit;
        self
    }

    /// Lower the pagination safety cap. Only tests change this.
    #[doc(hidden)]
    pub fn with_max_pages(mut self, max_pages: usize) -> Self {
        self.max_pages = max_pages;
        self
    }

    /// Every list with our open extension `name` inline, every page: the
    /// filtered `$expand`, since an unfiltered one silently returns nothing
    /// (S2). A list without the extension has none in `extensions`.
    pub async fn list_lists_with_extension(&self, name: &str) -> Result<Vec<Entity>, GraphError> {
        let mut url = self.url(&["me", "todo", "lists"]);
        expand_extension(&mut url, name);
        self.get_collection(url).await
    }

    /// `GET /me/todo/lists/{list}/tasks/{task}` with our open extension
    /// `name` inline, for [`GraphClient::get_each`].
    pub fn task_with_extension_url(&self, list_id: &str, task_id: &str, name: &str) -> Url {
        let mut url = self.task_url(list_id, task_id);
        expand_extension(&mut url, name);
        url
    }

    /// Where a fresh `lists/delta` round starts. No query options: delta
    /// rejects `$select`, `$filter` and `$top` (S3).
    pub fn lists_delta_start(&self) -> String {
        self.url(&["me", "todo", "lists", "delta"]).into()
    }

    /// Where a fresh `tasks/delta` round of list `list_id` starts.
    pub fn tasks_delta_start(&self, list_id: &str) -> String {
        self.url(&["me", "todo", "lists", list_id, "tasks", "delta"])
            .into()
    }

    /// One delta round from `from`, a start URL or a saved `deltaLink`:
    /// follow `nextLink` until a page carries the `deltaLink`, sending
    /// `Prefer` on every page (P1). A rejected link is an error for which
    /// [`GraphError::is_delta_reset`] is true.
    pub async fn delta(&self, from: &str) -> Result<Delta, GraphError> {
        let first = self.next_link(from)?;
        let (items, delta_link) = self.pages(first).await?;
        let delta_link = delta_link
            .ok_or_else(|| GraphError::Decode("a delta round ended without a deltaLink".into()))?;
        Ok(Delta { items, delta_link })
    }

    /// `GET /me/todo/lists/{id}`: after a delete it's 404, even while the
    /// list's tasks still answer (S4).
    pub async fn get_list(&self, list_id: &str) -> Result<Entity, GraphError> {
        entity(
            self.get(self.url(&["me", "todo", "lists", list_id]), false)
                .await?,
        )
    }

    /// `GET /me/todo/lists/{id}` with our open extension `name` inline: the
    /// read before an extension write merges into it (S2).
    pub async fn get_list_with_extension(
        &self,
        list_id: &str,
        name: &str,
    ) -> Result<Entity, GraphError> {
        let mut url = self.url(&["me", "todo", "lists", list_id]);
        expand_extension(&mut url, name);
        entity(self.get(url, false).await?)
    }

    /// `PATCH /me/todo/{owner}/extensions/{name}` with the whole document,
    /// `owner` being a list's path (`["lists", L]`) or a task's
    /// (`["lists", L, "tasks", T]`): Graph replaces the extension with
    /// `data` (S2), so a resend writes the same thing and it's idempotent.
    pub async fn replace_extension(
        &self,
        owner: &[&str],
        name: &str,
        data: &Value,
    ) -> Result<(), GraphError> {
        let url = self.extension_url(owner, Some(name));
        self.send(Call {
            body: Some(data),
            ..Call::new(Method::PATCH, url)
        })
        .await
        .map(drop)
    }

    /// `DELETE /me/todo/{owner}/extensions/{name}`, for a document with no
    /// fields left: Graph refuses a PATCH of an empty one (400
    /// `RequestBroker--ParseUri`). A 404 or a repeat means it's gone,
    /// which counts as success (S2: a delete of one that doesn't exist is
    /// 204 anyway).
    pub async fn delete_extension(&self, owner: &[&str], name: &str) -> Result<(), GraphError> {
        let url = self.extension_url(owner, Some(name));
        match self.send(Call::new(Method::DELETE, url)).await {
            Ok(_) => Ok(()),
            Err(error) if error.status() == Some(404) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// `POST /me/todo/{owner}/extensions`, for a list or task without our
    /// extension yet. POST of a name that exists acts as an upsert (S2), so
    /// unlike other creates a resend is harmless, and it's sent as
    /// idempotent.
    pub async fn create_extension(&self, owner: &[&str], body: &Value) -> Result<(), GraphError> {
        let url = self.extension_url(owner, None);
        self.send(Call {
            body: Some(body),
            ..Call::new(Method::POST, url)
        })
        .await
        .map(drop)
    }

    fn extension_url(&self, owner: &[&str], name: Option<&str>) -> Url {
        let mut segments = vec!["me", "todo"];
        segments.extend_from_slice(owner);
        segments.push("extensions");
        segments.extend(name);
        self.url(&segments)
    }

    /// `GET /me/todo/lists/{id}/tasks`, every page.
    pub async fn list_tasks(&self, list_id: &str) -> Result<Vec<Entity>, GraphError> {
        self.get_collection(self.url(&["me", "todo", "lists", list_id, "tasks"]))
            .await
    }

    /// `GET /me/todo/lists/{list}/tasks/{task}`.
    pub async fn get_task(&self, list_id: &str, task_id: &str) -> Result<Entity, GraphError> {
        let url = self.task_url(list_id, task_id);
        entity(self.get(url, false).await?)
    }

    /// `POST /me/todo/lists/{list}/tasks`. A create is never resent after it
    /// may have reached Graph (D-028), so this can return `OutcomeUnknown`.
    pub async fn create_task(&self, list_id: &str, body: &Value) -> Result<Entity, GraphError> {
        let url = self.url(&["me", "todo", "lists", list_id, "tasks"]);
        entity(
            self.send(Call {
                body: Some(body),
                idempotent: false,
                ..Call::new(Method::POST, url)
            })
            .await?,
        )
    }

    /// `PATCH /me/todo/lists/{list}/tasks/{task}` with `If-Match` when an
    /// etag is known (S6: task PATCH honours it, so a stale one is a 412).
    /// `idempotent` is false for a PATCH that completes a recurring task: a
    /// repeat would complete the next occurrence too.
    pub async fn update_task(
        &self,
        list_id: &str,
        task_id: &str,
        body: &Value,
        etag: Option<&str>,
        idempotent: bool,
    ) -> Result<Entity, GraphError> {
        let url = self.task_url(list_id, task_id);
        entity(
            self.send(Call {
                body: Some(body),
                if_match: etag,
                idempotent,
                ..Call::new(Method::PATCH, url)
            })
            .await?,
        )
    }

    /// `DELETE /me/todo/lists/{list}/tasks/{task}`. A 404 means it's already
    /// gone, which counts as success (S6). No `If-Match`: task DELETE
    /// ignores it (S6).
    pub async fn delete_task(&self, list_id: &str, task_id: &str) -> Result<(), GraphError> {
        let url = self.task_url(list_id, task_id);
        let deleted = self.send(Call::new(Method::DELETE, url)).await;
        match deleted {
            Ok(_) => Ok(()),
            Err(error) if error.status() == Some(404) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// One authenticated POST, PATCH or DELETE of `path`, for `raw`. It's a
    /// debugging passthrough, so it's treated like a create: never resent
    /// after it may have reached Graph.
    pub async fn write_raw(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, GraphError> {
        let url = self.raw_url(path)?;
        self.send(Call {
            body,
            idempotent: false,
            ..Call::new(method, url)
        })
        .await
    }

    fn task_url(&self, list_id: &str, task_id: &str) -> Url {
        self.url(&["me", "todo", "lists", list_id, "tasks", task_id])
    }

    /// One authenticated GET of `path` under the Graph root, for `raw GET`.
    /// Paths only: an absolute URL could send the token to another host.
    pub async fn get_raw(&self, path: &str) -> Result<Value, GraphError> {
        let url = self.raw_url(path)?;
        self.get(url, false).await
    }

    pub(crate) fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        // `path_segments_mut` percent-encodes each segment, so an ID can
        // never add a path level or a query string.
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(segments);
        }
        url
    }

    fn raw_url(&self, path: &str) -> Result<Url, GraphError> {
        let invalid =
            |why: &str| GraphError::InvalidInput(format!("invalid Graph path {path:?}: {why}"));
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(invalid(
                "give a path under the v1.0 root, starting with '/', such as /me",
            ));
        }
        let url = Url::parse(&format!(
            "{}{path}",
            self.base.as_str().trim_end_matches('/')
        ))
        .map_err(|error| invalid(&error.to_string()))?;
        if !self.is_under_base(&url) {
            return Err(invalid("it leaves the Graph v1.0 root"));
        }
        Ok(url)
    }

    /// `url` relative to the v1.0 root, as a `$batch` sub-request names it.
    pub(crate) fn relative(&self, url: &Url) -> String {
        let base_path = self.base.path().trim_end_matches('/');
        let path = url.path().strip_prefix(base_path).unwrap_or(url.path());
        match url.query() {
            Some(query) => format!("{path}?{query}"),
            None => path.to_owned(),
        }
    }

    pub(crate) fn backoff_unit(&self) -> Duration {
        self.backoff_unit
    }

    // Same origin, and still under `/v1.0/` once `..` has been resolved.
    fn is_under_base(&self, url: &Url) -> bool {
        let base_path = self.base.path().trim_end_matches('/');
        url.origin() == self.base.origin()
            && (url.path() == base_path || url.path().starts_with(&format!("{base_path}/")))
    }

    /// Every item of a collection.
    pub(crate) async fn get_collection(&self, first: Url) -> Result<Vec<Entity>, GraphError> {
        Ok(self.pages(first).await?.0)
    }

    /// Follow `@odata.nextLink` until there isn't one (P1: a short or empty
    /// page doesn't mean the last page), sending `Prefer` on every page.
    /// Returns the items and, for a delta round, the last page's
    /// `deltaLink`, checked to stay on Graph.
    async fn pages(&self, first: Url) -> Result<(Vec<Entity>, Option<String>), GraphError> {
        let mut items = Vec::new();
        let mut next = Some(first.clone());
        let mut pages = 0;
        while let Some(url) = next.take() {
            if pages == self.max_pages {
                return Err(GraphError::PageLimit {
                    path: first.path().to_owned(),
                    pages,
                });
            }
            pages += 1;
            let page: Page = serde_json::from_value(self.get(url, true).await?)
                .map_err(|error| GraphError::Decode(format!("a collection page: {error}")))?;
            items.extend(page.value);
            if let Some(delta_link) = page.delta_link {
                // Checked now, so a saved link is always one we'd follow.
                self.next_link(&delta_link)?;
                return Ok((items, Some(delta_link)));
            }
            next = page
                .next_link
                .map(|link| self.next_link(&link))
                .transpose()?;
        }
        Ok((items, None))
    }

    // The bearer token goes wherever a nextLink or deltaLink points, so it
    // must stay on the Graph origin.
    pub(crate) fn next_link(&self, link: &str) -> Result<Url, GraphError> {
        Url::parse(link)
            .ok()
            .filter(|url| self.is_under_base(url))
            .ok_or_else(|| GraphError::Decode(format!("refusing a nextLink outside Graph: {link}")))
    }

    async fn get(&self, url: Url, paged: bool) -> Result<Value, GraphError> {
        self.send(Call {
            paged,
            ..Call::new(Method::GET, url)
        })
        .await
    }

    /// One request with the retry policy (docs/blueprint/03-graph-provider.md#http-client).
    /// A request that isn't idempotent is resent only when the answer proves
    /// it didn't run: a 429, a 401 before the refresh, or a failed connect.
    /// Any other failure after it may have reached Graph is `OutcomeUnknown`
    /// (D-028).
    pub(crate) async fn send(&self, call: Call<'_>) -> Result<Value, GraphError> {
        let idempotent = call.idempotent;
        let body = self.send_bytes(call).await?;
        parse_body(&body).map_err(|error| {
            // It ran, but we can't tell what it made.
            if idempotent {
                error
            } else {
                GraphError::OutcomeUnknown(Box::new(error))
            }
        })
    }

    /// [`GraphClient::send`], answering the body's bytes as they came: a
    /// download, or an answer that isn't JSON.
    pub(crate) async fn send_bytes(&self, call: Call<'_>) -> Result<Vec<u8>, GraphError> {
        let mut attempt = 0;
        let mut refreshed = false;
        let mut token = self.auth.valid_token().await?;
        loop {
            let sent = {
                let _permit = self
                    .permits
                    .acquire()
                    .await
                    .map_err(|_| GraphError::Decode("the request limiter was closed".into()))?;
                let mut request = self
                    .http
                    .request(call.method.clone(), call.url.clone())
                    .bearer_auth(&token.access_token);
                if call.paged {
                    request = request.header("Prefer", PREFER_PAGE_SIZE);
                }
                if let Some(etag) = call.if_match {
                    request = request.header(IF_MATCH, etag);
                }
                if let Some(body) = call.body {
                    request = request.json(body);
                }
                if let Some(chunk) = &call.chunk {
                    request = request
                        .header(CONTENT_TYPE, "application/octet-stream")
                        .header(CONTENT_RANGE, &chunk.range)
                        .body(chunk.bytes.to_vec());
                }
                read(request).await
            };
            let (status, headers, body) = match sent {
                Ok(read) => read,
                Err(error) if retry::decide_network_retry(&error, attempt, call.idempotent) => {
                    tokio::time::sleep(retry::jittered_backoff(attempt, self.backoff_unit)).await;
                    attempt += 1;
                    continue;
                }
                // A failed connect never reached Graph; anything later might have.
                Err(error) if !call.idempotent && !error.is_connect() => {
                    return Err(GraphError::OutcomeUnknown(Box::new(error.into())));
                }
                Err(error) => return Err(error.into()),
            };
            match retry::decide_retry(status, &headers, attempt, call.idempotent) {
                RetryDecision::Success => return Ok(body),
                RetryDecision::RefreshToken if !refreshed => {
                    refreshed = true;
                    token = self.auth.refresh(&token).await?;
                }
                RetryDecision::RefreshToken => return Err(AuthError::Expired.into()),
                RetryDecision::RetryAfter(wait) => {
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                RetryDecision::Backoff => {
                    tokio::time::sleep(retry::jittered_backoff(attempt, self.backoff_unit)).await;
                    attempt += 1;
                }
                decision @ (RetryDecision::GiveUp | RetryDecision::OutcomeUnknown) => {
                    let failure = api_failure(status, &headers, &String::from_utf8_lossy(&body));
                    if decision == RetryDecision::OutcomeUnknown {
                        return Err(GraphError::OutcomeUnknown(Box::new(failure)));
                    }
                    return Err(failure);
                }
            }
        }
    }
}

/// A final error answer as a typed error; a 429 is `RateLimited` with the
/// wait Graph asked for. Shared by single requests and `$batch` steps.
pub(crate) fn api_failure(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> GraphError {
    let error = ApiError::parse(status.as_u16(), crate::api_error::request_id(headers), body);
    if status == StatusCode::TOO_MANY_REQUESTS {
        GraphError::RateLimited {
            retry_after: retry::retry_after(headers),
            source: error,
        }
    } else {
        GraphError::Api(error)
    }
}

/// One request for [`GraphClient::send`].
pub(crate) struct Call<'a> {
    pub method: Method,
    pub url: Url,
    pub body: Option<&'a Value>,
    /// An etag Graph gave us; never `*` or anything made up (S6: those are a 500).
    pub if_match: Option<&'a str>,
    pub paged: bool,
    /// False for every create and for a PATCH that completes a recurring task.
    pub idempotent: bool,
    /// Bytes of an upload session, sent as they are, in place of `body`.
    pub chunk: Option<Chunk<'a>>,
}

/// One range of an attachment's bytes, for an upload session's PUT.
pub(crate) struct Chunk<'a> {
    pub bytes: &'a [u8],
    /// `bytes <first>-<last>/<total>`.
    pub range: String,
}

impl Call<'_> {
    /// An idempotent request with no body, `If-Match` or paging.
    pub fn new(method: Method, url: Url) -> Self {
        Self {
            method,
            url,
            body: None,
            if_match: None,
            paged: false,
            idempotent: true,
            chunk: None,
        }
    }
}

type ReadResponse = (StatusCode, reqwest::header::HeaderMap, Vec<u8>);

// Send and read the whole body while the concurrency permit is held, and
// treat a body cut off mid-read like any other network failure.
async fn read(request: reqwest::RequestBuilder) -> Result<ReadResponse, reqwest::Error> {
    let response = request.send().await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await?.to_vec();
    Ok((status, headers, body))
}

// `$expand` works only with a filter on the extension's ID (S2). Spaces are
// written as %20: `+` isn't a space outside form encoding.
pub(crate) fn expand_extension(url: &mut Url, name: &str) {
    url.set_query(Some(&format!(
        "$expand=extensions($filter=id%20eq%20'{name}')"
    )));
}

pub(crate) fn entity(value: Value) -> Result<Entity, GraphError> {
    match value {
        Value::Object(entity) => Ok(entity),
        other => Err(GraphError::Decode(format!(
            "expected a JSON object, got {other}"
        ))),
    }
}

fn parse_body(body: &[u8]) -> Result<Value, GraphError> {
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body)
        .map_err(|error| GraphError::Decode(format!("the response wasn't JSON: {error}")))
}
