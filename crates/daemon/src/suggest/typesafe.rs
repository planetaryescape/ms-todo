//! TypeSafe's System One endpoint (https://docs.typesafe.ai/api), asked
//! one Choice question: which of the user's lists a task belongs in. A
//! request has one overall deadline, retries included, so a slow provider
//! costs a suggestion, never a wait.

use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::api_key::ApiKey;

pub(crate) const DEFAULT_URL: &str = "https://api.typesafe.ai/v1/systemone";

/// The whole request, retries included.
pub(crate) const DEADLINE: Duration = Duration::from_secs(3);

/// Waits before each resend after a 429 or 529, as the docs ask:
/// exponential, and short enough to fit the deadline.
const BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(500)];

/// TypeSafe's "overloaded".
const OVERLOADED: u16 = 529;

const MODEL: &str = "jev-latest";

/// The question key; the model never sees it.
const QUESTION: &str = "list";

/// Calibrated with forced choice (no "none" option) and the criteria
/// `criteria::build` makes (D-053).
const INSTRUCTIONS: &str = "The user captured the task in `task_title` into their inbox. Which \
     of their existing lists should it be filed into? Each option is one of their lists, with \
     the folder it's in and some of its open tasks.";

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(crate) struct Answer {
    /// The option chosen: a key of the criteria.
    pub choice: String,
    pub confidence: f64,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub(crate) enum TypeSafeError {
    #[error("TypeSafe answered {0}")]
    Status(u16),
    #[error("TypeSafe couldn't be reached: {0}")]
    Network(String),
    #[error("TypeSafe didn't answer within {} seconds", DEADLINE.as_secs())]
    Timeout,
    #[error("TypeSafe's answer wasn't readable: {0}")]
    Decode(String),
}

pub(crate) struct TypeSafe {
    http: reqwest::Client,
    url: String,
    deadline: Duration,
}

#[derive(Deserialize)]
struct Response {
    answers: Map<String, Value>,
}

impl TypeSafe {
    pub fn new(url: String) -> Self {
        Self::with_deadline(url, DEADLINE)
    }

    pub fn with_deadline(url: String, deadline: Duration) -> Self {
        Self {
            http: reqwest::Client::new(),
            url,
            deadline,
        }
    }

    /// Which of `criteria`'s options (list key to description) the task
    /// titled `title` belongs in.
    pub async fn choose(
        &self,
        key: &ApiKey,
        title: &str,
        criteria: &Map<String, Value>,
    ) -> Result<Answer, TypeSafeError> {
        let body = json!({
            "model": MODEL,
            "state": { "task_title": title },
            "questions": {
                QUESTION: {
                    "type": "choice",
                    "instructions": INSTRUCTIONS,
                    "criteria": criteria,
                }
            }
        });
        tokio::time::timeout(self.deadline, self.send(key, &body))
            .await
            .map_err(|_| TypeSafeError::Timeout)?
    }

    async fn send(&self, key: &ApiKey, body: &Value) -> Result<Answer, TypeSafeError> {
        let mut waits = BACKOFF.iter();
        loop {
            let response = self
                .http
                .post(&self.url)
                .bearer_auth(key.expose())
                .json(body)
                .send()
                .await
                // Without the URL: reqwest's error names it, and nothing
                // about the request belongs in the log.
                .map_err(|error| TypeSafeError::Network(error.without_url().to_string()))?;
            let status = response.status();
            let busy = status == StatusCode::TOO_MANY_REQUESTS || status.as_u16() == OVERLOADED;
            match waits.next() {
                Some(wait) if busy => tokio::time::sleep(*wait).await,
                _ if !status.is_success() => return Err(TypeSafeError::Status(status.as_u16())),
                _ => {
                    let response: Response = response
                        .json()
                        .await
                        .map_err(|error| TypeSafeError::Decode(error.without_url().to_string()))?;
                    return answer(response);
                }
            }
        }
    }
}

fn answer(mut response: Response) -> Result<Answer, TypeSafeError> {
    let value = response
        .answers
        .remove(QUESTION)
        .ok_or_else(|| TypeSafeError::Decode("no answer to the question".into()))?;
    serde_json::from_value(value).map_err(|error| TypeSafeError::Decode(error.to_string()))
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_partial_json, header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn key() -> ApiKey {
        ApiKey::for_tests("sk-test")
    }

    fn criteria() -> Map<String, Value> {
        let Value::Object(criteria) = json!({
            "Groceries": "List \"Groceries\". Example tasks: Milk; Eggs",
            "Garden": "Home list \"Garden\"."
        }) else {
            unreachable!("literal object")
        };
        criteria
    }

    fn answered(choice: &str, confidence: f64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {
                "list": {
                    "type": "choice",
                    "choice": choice,
                    "probabilities": { choice: confidence },
                    "confidence": confidence
                }
            },
            "usage": { "input_tokens": 300, "output_tokens": 20 }
        }))
    }

    async fn server() -> (MockServer, TypeSafe) {
        let server = MockServer::start().await;
        let client = TypeSafe::with_deadline(server.uri(), Duration::from_millis(1500));
        (server, client)
    }

    #[tokio::test]
    async fn asks_one_forced_choice_over_the_lists() {
        let (server, client) = server().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Bearer sk-test"))
            .and(body_partial_json(json!({
                "model": "jev-latest",
                "state": { "task_title": "buy oat milk" },
                "questions": { "list": { "type": "choice", "criteria": criteria() } }
            })))
            .respond_with(answered("Groceries", 0.93))
            .expect(1)
            .mount(&server)
            .await;
        let answer = client
            .choose(&key(), "buy oat milk", &criteria())
            .await
            .expect("answer");
        assert_eq!(
            answer,
            Answer {
                choice: "Groceries".into(),
                confidence: 0.93
            }
        );
    }

    #[tokio::test]
    async fn a_busy_answer_is_retried_with_backoff() {
        for busy in [429, 529] {
            let (server, client) = server().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(busy))
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(answered("Garden", 0.81))
                .expect(1)
                .mount(&server)
                .await;
            let answer = client.choose(&key(), "mow", &criteria()).await;
            assert_eq!(answer.map(|answer| answer.choice), Ok("Garden".into()));
        }
    }

    #[tokio::test]
    async fn busy_every_time_gives_up_after_two_retries() {
        let (server, client) = server().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .expect(3)
            .mount(&server)
            .await;
        let answer = client.choose(&key(), "mow", &criteria()).await;
        assert_eq!(answer, Err(TypeSafeError::Status(429)));
    }

    #[tokio::test]
    async fn a_server_error_is_not_retried() {
        let (server, client) = server().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let answer = client.choose(&key(), "mow", &criteria()).await;
        assert_eq!(answer, Err(TypeSafeError::Status(500)));
    }

    #[tokio::test]
    async fn a_slow_answer_times_out() {
        let server = MockServer::start().await;
        let client = TypeSafe::with_deadline(server.uri(), Duration::from_millis(200));
        Mock::given(method("POST"))
            .respond_with(answered("Garden", 0.9).set_delay(Duration::from_secs(2)))
            .mount(&server)
            .await;
        let answer = client.choose(&key(), "mow", &criteria()).await;
        assert_eq!(answer, Err(TypeSafeError::Timeout));
    }

    #[tokio::test]
    async fn an_answer_without_the_question_is_a_decode_error() {
        let (server, client) = server().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "answers": {} })))
            .mount(&server)
            .await;
        let answer = client.choose(&key(), "mow", &criteria()).await;
        assert!(
            matches!(answer, Err(TypeSafeError::Decode(_))),
            "{answer:?}"
        );
    }
}
