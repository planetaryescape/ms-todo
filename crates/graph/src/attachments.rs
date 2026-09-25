//! A task's file attachments (docs/blueprint/03-graph-provider.md#endpoints-the-whole-surface,
//! S14): list their metadata, download their bytes, and add one, directly
//! under 3 MiB or through an upload session up to 25 MiB.
//!
//! Adding an attachment is a create: the POST, and an upload session's
//! final PUT, which commits it, are never resent after they may have
//! reached Graph (D-028). A lost answer to either is `OutcomeUnknown`.

use base64::Engine;
use reqwest::{Method, Url};
use serde_json::{Value, json};

use crate::client::{Call, Chunk, Entity, GraphClient, entity};
use crate::error::GraphError;

/// Graph's limit for one attachment. Anything larger is refused before
/// anything is sent.
pub const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

/// Smaller than this goes in one POST with `contentBytes`.
const DIRECT_UPLOAD_LIMIT: usize = 3 * 1024 * 1024;

/// An upload session's chunk: under Graph's 4 MiB, a multiple of 320 KiB.
const CHUNK_BYTES: usize = 10 * 320 * 1024;

impl GraphClient {
    /// `GET …/tasks/{task}` with our open extension `name` inline (S2).
    pub async fn get_task_with_extension(
        &self,
        list_id: &str,
        task_id: &str,
        name: &str,
    ) -> Result<Entity, GraphError> {
        let url = self.task_with_extension_url(list_id, task_id, name);
        entity(self.send(Call::new(Method::GET, url)).await?)
    }

    /// Every attachment's metadata (`GET …/attachments`), without its bytes.
    pub async fn list_attachments(
        &self,
        list_id: &str,
        task_id: &str,
    ) -> Result<Vec<Entity>, GraphError> {
        self.get_collection(self.attachments_url(list_id, task_id, &[]))
            .await
    }

    /// An attachment's bytes (`GET …/attachments/{id}/$value`).
    pub async fn download_attachment(
        &self,
        list_id: &str,
        task_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, GraphError> {
        let url = self.attachments_url(list_id, task_id, &[attachment_id, "$value"]);
        self.send_bytes(Call::new(Method::GET, url)).await
    }

    /// Add `bytes` as the file `name` to a task. Over 25 MiB is refused
    /// without sending anything.
    pub async fn add_attachment(
        &self,
        list_id: &str,
        task_id: &str,
        name: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<(), GraphError> {
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(GraphError::InvalidInput(format!(
                "{name} is {} bytes; Microsoft To Do takes attachments up to 25 MB",
                bytes.len()
            )));
        }
        if bytes.len() < DIRECT_UPLOAD_LIMIT {
            let body = json!({
                "@odata.type": "#microsoft.graph.taskFileAttachment",
                "name": name,
                "contentType": content_type,
                "contentBytes": base64::engine::general_purpose::STANDARD.encode(bytes),
            });
            return self
                .send(Call {
                    body: Some(&body),
                    idempotent: false,
                    ..Call::new(Method::POST, self.attachments_url(list_id, task_id, &[]))
                })
                .await
                .map(drop);
        }
        let session_url = self.attachments_url(list_id, task_id, &["createUploadSession"]);
        self.upload(session_url, name, content_type, bytes).await
    }

    /// An upload session: create it, then PUT the bytes in order. Only the
    /// final PUT commits the attachment. A session that fails before it
    /// just expires, so creating it and the earlier PUTs are safe to send
    /// again.
    async fn upload(
        &self,
        session_url: Url,
        name: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<(), GraphError> {
        let body = json!({
            "attachmentInfo": {
                "attachmentType": "file",
                "name": name,
                "size": bytes.len(),
                "contentType": content_type,
            }
        });
        let session = self
            .send(Call {
                body: Some(&body),
                ..Call::new(Method::POST, session_url)
            })
            .await?;
        let upload_url = session
            .get("uploadUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| GraphError::Decode("an upload session without uploadUrl".into()))?;
        // The bytes go to `<uploadUrl>/content`; the bare URL answers 404
        // (S14). The token is sent, so it must stay on Graph.
        let url = self.next_link(&format!("{upload_url}/content"))?;
        let total = bytes.len();
        let mut start = 0;
        while start < total {
            let end = (start + CHUNK_BYTES).min(total);
            let last = end == total;
            self.send_bytes(Call {
                chunk: Some(Chunk {
                    bytes: &bytes[start..end],
                    range: format!("bytes {start}-{}/{total}", end - 1),
                }),
                idempotent: !last,
                ..Call::new(Method::PUT, url.clone())
            })
            .await?;
            start = end;
        }
        Ok(())
    }

    /// `…/lists/{list}/tasks/{task}/attachments`, then `more`.
    fn attachments_url(&self, list_id: &str, task_id: &str, more: &[&str]) -> Url {
        let mut segments = vec![
            "me",
            "todo",
            "lists",
            list_id,
            "tasks",
            task_id,
            "attachments",
        ];
        segments.extend_from_slice(more);
        self.url(&segments)
    }
}
