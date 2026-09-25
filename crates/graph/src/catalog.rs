//! Lists, Outlook categories and open extensions by name (rung 8e,
//! docs/blueprint/03-graph-provider.md#endpoints-the-whole-surface).
//!
//! A list or category POST is a create, never resent after it may have
//! reached Graph (D-028), so it can return `OutcomeUnknown`. Every PATCH
//! sets absolute values and every DELETE counts a 404 as done, so both
//! are resent as usual.

use reqwest::Method;
use serde_json::Value;

use crate::client::{Call, Entity, GraphClient, entity};
use crate::error::GraphError;

const CATEGORIES: [&str; 3] = ["me", "outlook", "masterCategories"];

impl GraphClient {
    /// `POST /me/todo/lists`: the list as Graph made it.
    pub async fn create_list(&self, body: &Value) -> Result<Entity, GraphError> {
        let url = self.url(&["me", "todo", "lists"]);
        entity(
            self.send(Call {
                body: Some(body),
                idempotent: false,
                ..Call::new(Method::POST, url)
            })
            .await?,
        )
    }

    /// `PATCH /me/todo/lists/{id}`. List PATCH ignores `If-Match` (S6), so
    /// none is sent.
    pub async fn update_list(&self, list_id: &str, body: &Value) -> Result<Entity, GraphError> {
        let url = self.url(&["me", "todo", "lists", list_id]);
        entity(
            self.send(Call {
                body: Some(body),
                ..Call::new(Method::PATCH, url)
            })
            .await?,
        )
    }

    /// `DELETE /me/todo/lists/{id}`, its tasks with it. A 404 means it's
    /// gone already, which counts as success.
    pub async fn delete_list(&self, list_id: &str) -> Result<(), GraphError> {
        let url = self.url(&["me", "todo", "lists", list_id]);
        match self.send(Call::new(Method::DELETE, url)).await {
            Ok(_) => Ok(()),
            Err(error) if error.status() == Some(404) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// `GET /me/outlook/masterCategories`, every page.
    pub async fn list_categories(&self) -> Result<Vec<Entity>, GraphError> {
        self.get_collection(self.url(&CATEGORIES)).await
    }

    /// `POST /me/outlook/masterCategories`. Names are unique ignoring
    /// case: a second one is 409 `CategoryNameExists` (S7).
    pub async fn create_category(&self, body: &Value) -> Result<Entity, GraphError> {
        entity(
            self.send(Call {
                body: Some(body),
                idempotent: false,
                ..Call::new(Method::POST, self.url(&CATEGORIES))
            })
            .await?,
        )
    }

    /// `PATCH /me/outlook/masterCategories/{id}`: only `color` takes; a
    /// new name is ignored (S7).
    pub async fn update_category(&self, id: &str, body: &Value) -> Result<Entity, GraphError> {
        let url = self.url(&["me", "outlook", "masterCategories", id]);
        entity(
            self.send(Call {
                body: Some(body),
                ..Call::new(Method::PATCH, url)
            })
            .await?,
        )
    }

    /// `DELETE /me/outlook/masterCategories/{id}`; a 404 counts as done.
    pub async fn delete_category(&self, id: &str) -> Result<(), GraphError> {
        let url = self.url(&["me", "outlook", "masterCategories", id]);
        match self.send(Call::new(Method::DELETE, url)).await {
            Ok(_) => Ok(()),
            Err(error) if error.status() == Some(404) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// `GET /me/todo/{owner}/extensions/{name}`.
    pub async fn get_extension(&self, owner: &[&str], name: &str) -> Result<Entity, GraphError> {
        entity(
            self.send(Call::new(
                Method::GET,
                self.extension_url(owner, Some(name)),
            ))
            .await?,
        )
    }
}
