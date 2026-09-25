//! A task's steps (`checklistItems`) and link (`linkedResources`): POST,
//! PATCH and DELETE under the task (docs/blueprint/03-graph-provider.md#endpoints-the-whole-surface).
//!
//! A create is never resent after it may have reached Graph (D-028): a
//! step or link carries no marker to find it by, so a lost answer is
//! `OutcomeUnknown`. A PATCH or DELETE sends `If-Match` with the parent
//! task's etag, which both kinds honour with a 412 (S6, S15); each sets or
//! removes absolute values, so it's safe to resend.

use reqwest::{Method, Url};
use serde_json::Value;

use crate::client::{Call, Entity, GraphClient, entity};
use crate::error::GraphError;

impl GraphClient {
    /// `POST …/tasks/{task}/{collection}`: the child as Graph made it.
    pub async fn create_child(
        &self,
        list_id: &str,
        task_id: &str,
        collection: &str,
        body: &Value,
    ) -> Result<Entity, GraphError> {
        let url = self.child_url(list_id, task_id, collection, None);
        entity(
            self.send(Call {
                body: Some(body),
                idempotent: false,
                ..Call::new(Method::POST, url)
            })
            .await?,
        )
    }

    /// `PATCH …/tasks/{task}/{collection}/{id}` with the parent's etag.
    pub async fn update_child(
        &self,
        list_id: &str,
        task_id: &str,
        collection: &str,
        id: &str,
        body: &Value,
        etag: Option<&str>,
    ) -> Result<Entity, GraphError> {
        let url = self.child_url(list_id, task_id, collection, Some(id));
        entity(
            self.send(Call {
                body: Some(body),
                if_match: etag,
                ..Call::new(Method::PATCH, url)
            })
            .await?,
        )
    }

    /// `DELETE …/tasks/{task}/{collection}/{id}` with the parent's etag. A
    /// 404 means it's gone already, which counts as success.
    pub async fn delete_child(
        &self,
        list_id: &str,
        task_id: &str,
        collection: &str,
        id: &str,
        etag: Option<&str>,
    ) -> Result<(), GraphError> {
        let url = self.child_url(list_id, task_id, collection, Some(id));
        let deleted = self
            .send(Call {
                if_match: etag,
                ..Call::new(Method::DELETE, url)
            })
            .await;
        match deleted {
            Ok(_) => Ok(()),
            Err(error) if error.status() == Some(404) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn child_url(&self, list_id: &str, task_id: &str, collection: &str, id: Option<&str>) -> Url {
        let mut segments = vec!["me", "todo", "lists", list_id, "tasks", task_id, collection];
        segments.extend(id);
        self.url(&segments)
    }
}
