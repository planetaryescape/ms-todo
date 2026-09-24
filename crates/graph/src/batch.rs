//! Fan-out GETs through `$batch` (docs/blueprint/03-graph-provider.md#http-client,
//! S10): at most 20 sub-requests, chained with `dependsOn` so a batch runs
//! them one after another and adds no concurrency. Every sub-response is
//! checked on its own; the outer 200 means nothing.
//!
//! In a chain, a failed step makes every later one 424 `FailedDependency`.
//! Those never ran, so they go back on the queue. Any other step follows
//! the same `decide_retry` rules as a single GET. The first step of a batch
//! can't be a 424, so every batch settles at least one request.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::{Call, Entity, GraphClient, api_failure, entity};
use crate::error::GraphError;
use crate::retry::{self, RetryDecision};

/// S10: 21 sub-requests is a 400.
const BATCH_LIMIT: usize = 20;

#[derive(Deserialize)]
struct Responses {
    responses: Vec<SubResponse>,
}

#[derive(Deserialize)]
struct SubResponse {
    id: String,
    status: u16,
    #[serde(default)]
    headers: Map<String, Value>,
    #[serde(default)]
    body: Value,
}

impl SubResponse {
    /// The step's own headers (`Retry-After`, `request-id`), as HTTP headers.
    fn header_map(&self) -> HeaderMap {
        self.headers
            .iter()
            .filter_map(|(name, value)| {
                Some((
                    HeaderName::from_bytes(name.as_bytes()).ok()?,
                    HeaderValue::from_str(value.as_str()?).ok()?,
                ))
            })
            .collect()
    }
}

impl GraphClient {
    /// GET each of `urls` (under the Graph root), in sequential batches of
    /// up to 20. The outer error is a batch request that failed as a whole
    /// (network, sign-in); otherwise each URL gets its own result, in order.
    pub async fn get_each(
        &self,
        urls: &[Url],
    ) -> Result<Vec<Result<Entity, GraphError>>, GraphError> {
        let mut results: Vec<Option<Result<Entity, GraphError>>> =
            urls.iter().map(|_| None).collect();
        let mut retries = vec![0_u32; urls.len()];
        let mut queue: VecDeque<usize> = (0..urls.len()).collect();
        let batch_url = self.url(&["$batch"]);
        while !queue.is_empty() {
            let chunk: Vec<usize> = std::iter::from_fn(|| queue.pop_front())
                .take(BATCH_LIMIT)
                .collect();
            let requests: Vec<Value> = chunk
                .iter()
                .enumerate()
                .map(|(position, &index)| {
                    let mut request = json!({
                        "id": index.to_string(),
                        "method": "GET",
                        "url": self.relative(&urls[index]),
                    });
                    if let Some(previous) = position.checked_sub(1) {
                        request["dependsOn"] = json!([chunk[previous].to_string()]);
                    }
                    request
                })
                .collect();
            let body = json!({ "requests": requests });
            // Only GETs inside, so the batch itself is safe to resend.
            let answer = self
                .send(Call {
                    body: Some(&body),
                    ..Call::new(Method::POST, batch_url.clone())
                })
                .await?;
            let answer: Responses = serde_json::from_value(answer)
                .map_err(|error| GraphError::Decode(format!("a $batch response: {error}")))?;

            let mut again = Vec::new();
            let mut wait = Duration::ZERO;
            let mut answered = HashSet::new();
            for response in answer.responses {
                let Some(index) = response
                    .id
                    .parse::<usize>()
                    .ok()
                    .filter(|index| chunk.contains(index))
                else {
                    continue;
                };
                answered.insert(index);
                let Ok(status) = StatusCode::from_u16(response.status) else {
                    results[index] = Some(Err(GraphError::Decode(format!(
                        "a $batch step answered status {}",
                        response.status
                    ))));
                    continue;
                };
                if status == StatusCode::FAILED_DEPENDENCY {
                    again.push(index);
                    continue;
                }
                let headers = response.header_map();
                let pause = match retry::decide_retry(status, &headers, retries[index], true) {
                    RetryDecision::Success => {
                        results[index] = Some(entity(response.body));
                        continue;
                    }
                    RetryDecision::RetryAfter(pause) => pause,
                    RetryDecision::Backoff => {
                        retry::jittered_backoff(retries[index], self.backoff_unit())
                    }
                    // A step's 401 can't be fixed by refreshing: the batch
                    // itself was authorised.
                    RetryDecision::RefreshToken
                    | RetryDecision::GiveUp
                    | RetryDecision::OutcomeUnknown => {
                        results[index] = Some(Err(api_failure(
                            status,
                            &headers,
                            &response.body.to_string(),
                        )));
                        continue;
                    }
                };
                retries[index] += 1;
                again.push(index);
                wait = wait.max(pause);
            }
            for &index in &chunk {
                if !answered.contains(&index) {
                    results[index] = Some(Err(GraphError::Decode(format!(
                        "the $batch response has no answer for request {index}"
                    ))));
                }
            }
            again.sort_unstable();
            for index in again.into_iter().rev() {
                queue.push_front(index);
            }
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        Ok(results
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| {
                    Err(GraphError::Decode("a $batch request got no answer".into()))
                })
            })
            .collect())
    }
}
