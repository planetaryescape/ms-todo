//! A task DELETE that Graph checks against an etag. Task DELETE ignores
//! `If-Match` (S6), but a PATCH honours it, so one `$batch` sends an empty
//! PATCH with `If-Match` and the DELETE depending on it: a task changed
//! since `etag` makes the PATCH a 412 and the DELETE a 424 that never ran
//! (S6, follow-up of 2026-09-27; D-070). What's left is the gap between
//! the two steps inside Graph, not a round trip.

use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::batch::Responses;
use crate::client::{Call, GraphClient, api_failure};
use crate::error::GraphError;

/// How a conditional delete ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionalDelete {
    Deleted,
    /// The task's etag isn't the one given any more: nothing was deleted.
    Changed,
    /// Already gone.
    Gone,
}

impl GraphClient {
    /// Delete the task only if its etag is still `etag`.
    pub async fn delete_task_if_unchanged(
        &self,
        list_id: &str,
        task_id: &str,
        etag: &str,
    ) -> Result<ConditionalDelete, GraphError> {
        let task = self.relative(&self.task_url(list_id, task_id));
        let body = json!({ "requests": [
            {
                "id": "check",
                "method": "PATCH",
                "url": task,
                "headers": { "Content-Type": "application/json", "If-Match": etag },
                "body": {}
            },
            { "id": "delete", "dependsOn": ["check"], "method": "DELETE", "url": task }
        ] });
        // Safe to resend: after a batch that landed, the check is a 404
        // (gone) or, if only the PATCH landed, a 412, which keeps the task.
        let answer = self
            .send(Call {
                body: Some(&body),
                ..Call::new(Method::POST, self.url(&["$batch"]))
            })
            .await?;
        let answer: Responses = serde_json::from_value(answer)
            .map_err(|error| GraphError::Decode(format!("a $batch response: {error}")))?;
        let step = |id: &str| {
            answer
                .responses
                .iter()
                .find(|response| response.id == id)
                .ok_or_else(|| GraphError::Decode(format!("the $batch has no answer for {id}")))
        };
        let failure = |response: &crate::batch::SubResponse| {
            let status =
                StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            api_failure(status, &response.header_map(), &response.body.to_string())
        };
        let check = step("check")?;
        match check.status {
            412 => return Ok(ConditionalDelete::Changed),
            404 => return Ok(ConditionalDelete::Gone),
            200..=299 => {}
            _ => return Err(failure(check)),
        }
        let delete = step("delete")?;
        match delete.status {
            200..=299 | 404 => Ok(ConditionalDelete::Deleted),
            _ => Err(failure(delete)),
        }
    }
}
