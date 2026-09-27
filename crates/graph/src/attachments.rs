//! A task's file attachments (docs/blueprint/03-graph-provider.md#endpoints-the-whole-surface,
//! S14, S16): list their metadata, download their bytes, and add one,
//! directly under 3 MiB or through an upload session up to 25 MiB.
//!
//! Adding an attachment is a create: the POST, and an upload session's
//! final PUT, which commits it, are never resent after they may have
//! reached Graph (D-028). A lost answer to either is `OutcomeUnknown`.
//!
//! An upload session's earlier PUTs commit nothing, so one whose answer
//! was lost is sent again: Graph answers a range it already has with 400
//! `InvalidStart` (S16), which says the lost one landed, and the upload
//! goes on from the next range. Graph has no way to ask a session where
//! it stands (a GET of it is 404, S16), so that's how it resumes.
//!
//! An [`UploadWatcher`] hears where a session stands before each step is
//! sent (D-067), so a daemon that stopped half-way can go on from the
//! last range Graph took, in the same session: the range it may have sent
//! since is sent again, and `InvalidStart` says it landed. A session that
//! can't be taken up again (expired, or refused) is left and a new one
//! started; that's safe, since only the final PUT commits anything, and
//! the watcher hears before it's sent, so a final PUT that may have gone
//! is never sent again.

use base64::Engine;
use reqwest::header::{HeaderMap, LOCATION};
use reqwest::{Method, Url};
use serde_json::{Value, json};

use serde::{Deserialize, Serialize};

use crate::client::{Call, Chunk, Entity, GraphClient, entity};
use crate::error::GraphError;
use crate::retry;

/// Graph's limit for one attachment. Anything larger is refused before
/// anything is sent.
pub use ms_todo_core::MAX_ATTACHMENT_BYTES;

/// Smaller than this goes in one POST with `contentBytes`.
const DIRECT_UPLOAD_LIMIT: usize = 3 * 1024 * 1024;

/// An upload session's chunk: under Graph's 4 MiB, a multiple of 320 KiB.
const CHUNK_BYTES: usize = 10 * 320 * 1024;

/// How many times one upload sends a chunk again after its answer was
/// lost, in all, before it gives up and the operation tries later.
const MAX_RESENDS: u32 = 3;

/// An attachment's own fields, as a listing gives them: what the cache
/// keeps. A POST's answer also echoes `contentBytes`.
const METADATA: &[&str] = &[
    "@odata.type",
    "id",
    "name",
    "contentType",
    "size",
    "lastModifiedDateTime",
];

/// Where an upload session stands, as an [`UploadWatcher`] saves it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadState {
    /// The session's URL, once it's made. It carries its own
    /// authorisation, for two hours (S14).
    #[serde(default)]
    pub url: Option<String>,
    /// The first byte Graph hasn't taken.
    #[serde(default)]
    pub next: usize,
    pub total: usize,
    /// The final PUT, which commits the attachment, may have been sent.
    #[serde(default)]
    pub committing: bool,
}

/// Told where an upload stands before each step that could change it is
/// sent; an error stops the upload there, having sent nothing more.
pub trait UploadWatcher: Send {
    fn save(
        &mut self,
        state: &UploadState,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

/// A watcher that keeps nothing: an upload that can't be taken up again.
struct Unwatched;

impl UploadWatcher for Unwatched {
    async fn save(&mut self, _: &UploadState) -> Result<(), String> {
        Ok(())
    }
}

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

    /// `GET …/tasks/{task}/attachments`, for [`GraphClient::get_each`].
    pub fn attachments_list_url(&self, list_id: &str, task_id: &str) -> Url {
        self.attachments_url(list_id, task_id, &[])
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

    /// Add `bytes` as the file `name` to a task, and answer what Graph
    /// made: its `id`, and the metadata it said or was sent. Over 25 MiB
    /// is refused without sending anything.
    pub async fn add_attachment(
        &self,
        list_id: &str,
        task_id: &str,
        name: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<Entity, GraphError> {
        let file = NewAttachment {
            name,
            content_type,
            bytes,
        };
        self.add_attachment_watched(list_id, task_id, &file, None, &mut Unwatched)
            .await
    }

    /// [`GraphClient::add_attachment`], telling `watcher` where an upload
    /// session stands as it goes, and going on from `resume`, a session
    /// an earlier attempt saved, when there is one.
    pub async fn add_attachment_watched(
        &self,
        list_id: &str,
        task_id: &str,
        file: &NewAttachment<'_>,
        resume: Option<UploadState>,
        watcher: &mut impl UploadWatcher,
    ) -> Result<Entity, GraphError> {
        let NewAttachment {
            name,
            content_type,
            bytes,
        } = *file;
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
            let created = self
                .send(Call {
                    body: Some(&body),
                    idempotent: false,
                    ..Call::new(Method::POST, self.attachments_url(list_id, task_id, &[]))
                })
                .await?;
            return entity(created).map(metadata);
        }
        let session_url = self.attachments_url(list_id, task_id, &["createUploadSession"]);
        let total = bytes.len();
        let resume = match resume {
            Some(state) if state.committing => {
                // Sent again, it could attach the file twice.
                return Err(GraphError::OutcomeUnknown(Box::new(GraphError::Decode(
                    "the upload's final part may have been sent before it stopped".into(),
                ))));
            }
            Some(UploadState {
                url: Some(url),
                next,
                total: saved,
                ..
            }) if saved == total => Some((url, next)),
            _ => None,
        };
        if let Some((upload_url, next)) = resume {
            match self
                .put_chunks(&upload_url, file, next, true, watcher)
                .await
            {
                Err(Upload::Stale) => {
                    // Expired or refused before anything could commit: a
                    // new session, from the start.
                    self.cancel(&upload_url).await;
                }
                done => return done.map_err(Upload::into_error),
            }
        }
        self.upload(session_url, file, watcher).await
    }

    /// An upload session: create it, then PUT the bytes in order, each
    /// from where Graph's `nextExpectedRanges` says. Only the final PUT
    /// commits the attachment. A session that fails before it just
    /// expires, so creating it and the earlier PUTs are safe to send again.
    async fn upload(
        &self,
        session_url: Url,
        file: &NewAttachment<'_>,
        watcher: &mut impl UploadWatcher,
    ) -> Result<Entity, GraphError> {
        let total = file.bytes.len();
        let body = json!({
            "attachmentInfo": {
                "attachmentType": "file",
                "name": file.name,
                "size": total,
                "contentType": file.content_type,
            }
        });
        let starting = UploadState {
            total,
            ..UploadState::default()
        };
        save(watcher, &starting).await?;
        let session = self
            .send(Call {
                body: Some(&body),
                ..Call::new(Method::POST, session_url)
            })
            .await?;
        let upload_url = session
            .get("uploadUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| GraphError::Decode("an upload session without uploadUrl".into()))?
            .to_owned();
        self.put_chunks(&upload_url, file, 0, false, watcher)
            .await
            .map_err(Upload::into_error)
    }

    /// PUT `file`'s bytes into the session at `upload_url` from `start`,
    /// in order, telling `watcher` before each. `in_doubt`: the PUT of
    /// `start` may have landed already (an upload taken up again).
    async fn put_chunks(
        &self,
        upload_url: &str,
        file: &NewAttachment<'_>,
        mut start: usize,
        mut in_doubt: bool,
        watcher: &mut impl UploadWatcher,
    ) -> Result<Entity, Upload> {
        let (name, content_type, bytes) = (file.name, file.content_type, file.bytes);
        // The bytes go to `<uploadUrl>/content`; the bare URL answers 404
        // (S14). The token is sent, so it must stay on Graph.
        let url = self.next_link(&format!("{upload_url}/content"))?;
        let total = bytes.len();
        // Taken up again, a session may be gone: until Graph takes a PUT,
        // a refusal means start again rather than fail.
        let mut resumed = in_doubt;
        let mut resends = 0;
        let mut state = UploadState {
            url: Some(upload_url.to_owned()),
            next: start,
            total,
            committing: false,
        };
        loop {
            let end = (start + CHUNK_BYTES).min(total);
            let last = end == total;
            state.next = start;
            state.committing = last;
            save(watcher, &state).await?;
            let sent = self
                .send_full(Call {
                    chunk: Some(Chunk {
                        bytes: &bytes[start..end],
                        range: format!("bytes {start}-{}/{total}", end - 1),
                    }),
                    // Every PUT is sent once per call, so this loop knows
                    // which ones may have landed.
                    idempotent: false,
                    ..Call::new(Method::PUT, url.clone())
                })
                .await;
            match sent {
                Ok((headers, _)) if last => {
                    return Ok(created(&headers, name, content_type, total)?);
                }
                Ok((_, answer)) => {
                    start = next_start(&answer, start, total)?.unwrap_or(end);
                    in_doubt = false;
                    resumed = false;
                }
                // The final PUT is the create: never sent again.
                Err(error @ GraphError::OutcomeUnknown(_)) if last => {
                    return Err(Upload::Failed(error));
                }
                Err(error) if in_doubt && !last && is_invalid_start(&error) => {
                    // Graph has this range: the PUT whose answer was lost
                    // landed.
                    start = end;
                    in_doubt = false;
                    resumed = false;
                }
                Err(_) if resumed => return Err(Upload::Stale),
                Err(error) if last => return Err(Upload::Failed(error)),
                Err(GraphError::OutcomeUnknown(cause)) => {
                    if resends == MAX_RESENDS {
                        self.cancel(upload_url).await;
                        return Err(Upload::Failed(*cause));
                    }
                    tokio::time::sleep(retry::jittered_backoff(resends, self.backoff_unit())).await;
                    resends += 1;
                    in_doubt = true;
                }
                Err(error) => {
                    self.cancel(upload_url).await;
                    return Err(Upload::Failed(error));
                }
            }
        }
    }

    /// Cancel an upload session given up on (03): a DELETE of its URL.
    /// Best effort: one left alone expires in two hours (S14).
    async fn cancel(&self, upload_url: &str) {
        if let Ok(url) = self.next_link(upload_url) {
            let _ = self.send_bytes(Call::new(Method::DELETE, url)).await;
        }
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

/// A file to attach: what it's called, its type and its bytes.
#[derive(Clone, Copy, Debug)]
pub struct NewAttachment<'a> {
    pub name: &'a str,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

/// How putting a session's chunks stopped short.
enum Upload {
    Failed(GraphError),
    /// A session taken up again refused its first PUT: nothing it holds
    /// can commit, so a new one starts.
    Stale,
}

impl Upload {
    fn into_error(self) -> GraphError {
        match self {
            Self::Failed(error) => error,
            Self::Stale => {
                GraphError::Decode("the upload session couldn't be taken up again".into())
            }
        }
    }
}

impl From<GraphError> for Upload {
    fn from(error: GraphError) -> Self {
        Self::Failed(error)
    }
}

async fn save(watcher: &mut impl UploadWatcher, state: &UploadState) -> Result<(), GraphError> {
    watcher.save(state).await.map_err(|why| {
        GraphError::Decode(format!(
            "the upload stopped: its progress couldn't be saved ({why})"
        ))
    })
}

/// An attachment's own fields, without its bytes or the answer's
/// metadata.
pub fn metadata(mut attachment: Entity) -> Entity {
    attachment.retain(|key, _| METADATA.contains(&key.as_str()));
    attachment
}

/// What an upload session made, from its last PUT's `Location`, which is
/// the new attachment's URL (S16): `…/attachments/{id}`.
fn created(
    headers: &HeaderMap,
    name: &str,
    content_type: &str,
    total: usize,
) -> Result<Entity, GraphError> {
    let id = headers
        .get(LOCATION)
        .and_then(|location| location.to_str().ok())
        .and_then(attachment_id)
        .ok_or_else(|| {
            // It's attached, but nothing names it: as good as unknown.
            GraphError::OutcomeUnknown(Box::new(GraphError::Decode(
                "the upload finished without saying which attachment it made".into(),
            )))
        })?;
    let made = json!({
        "@odata.type": "#microsoft.graph.taskFileAttachment",
        "id": id,
        "name": name,
        "contentType": content_type,
        "size": total,
    });
    entity(made)
}

/// The ID in an attachment's URL: everything after its last
/// `/attachments/`, percent-decoded, or Graph's `attachments('…')` form.
fn attachment_id(location: &str) -> Option<String> {
    let path = location.split(['?', '#']).next()?;
    let rest = match path.rfind("/attachments/") {
        Some(at) => &path[at + "/attachments/".len()..],
        None => {
            let at = path.rfind("attachments('")?;
            path[at + "attachments('".len()..].strip_suffix("')")?
        }
    };
    let id = percent_encoding::percent_decode_str(rest)
        .decode_utf8()
        .ok()?
        .into_owned();
    (!id.is_empty()).then_some(id)
}

/// Where the next PUT starts, from a PUT's answer: the first of
/// `nextExpectedRanges`, `"<start>-"` or, as To Do sends it, `"<start>"`
/// (S16). `None` when the answer doesn't say. A range that doesn't move
/// the upload on is an error, never a loop.
fn next_start(answer: &[u8], was: usize, total: usize) -> Result<Option<usize>, GraphError> {
    let Ok(answer) = serde_json::from_slice::<Value>(answer) else {
        return Ok(None);
    };
    let Some(first) = answer["nextExpectedRanges"]
        .as_array()
        .and_then(|ranges| ranges.first())
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let start = first
        .split('-')
        .next()
        .and_then(|start| start.trim().parse::<usize>().ok())
        .ok_or_else(|| GraphError::Decode(format!("an upload range Graph sent: {first:?}")))?;
    if start <= was || start >= total {
        return Err(GraphError::Decode(format!(
            "Graph asked for bytes from {start} of {total} after sending from {was}"
        )));
    }
    Ok(Some(start))
}

/// Graph's 400 `InvalidStart`: a PUT of a range it already has (S16).
fn is_invalid_start(error: &GraphError) -> bool {
    match error {
        GraphError::Api(api) => {
            api.status == 400 && api.inner_code.as_deref() == Some("InvalidStart")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_comes_from_the_location_as_graph_sends_it() {
        assert_eq!(
            attachment_id(
                "https://graph.microsoft.com/v1.0/users/a@b.com/todo/lists/L==/tasks/T=/attachments/AQMk_x-y="
            )
            .as_deref(),
            Some("AQMk_x-y=")
        );
        assert_eq!(
            attachment_id("https://g/v1.0/x/attachments('A%2B1')").as_deref(),
            Some("A+1")
        );
        assert_eq!(attachment_id("https://graph.microsoft.com"), None);
        assert_eq!(attachment_id("https://g/v1.0/x/attachments/"), None);
    }

    #[test]
    fn the_next_range_is_read_in_both_shapes_and_must_move_on() {
        let answer = |range: &str| json!({ "nextExpectedRanges": [range] }).to_string();
        assert_eq!(
            next_start(answer("3276800").as_bytes(), 0, 9_000_000).expect("read"),
            Some(3_276_800)
        );
        assert_eq!(
            next_start(answer("3276800-").as_bytes(), 0, 9_000_000).expect("read"),
            Some(3_276_800)
        );
        assert_eq!(next_start(b"{}", 0, 10).expect("read"), None);
        assert!(
            next_start(answer("0-").as_bytes(), 0, 10).is_err(),
            "no progress"
        );
        assert!(next_start(answer("x").as_bytes(), 0, 10).is_err());
    }

    #[test]
    fn metadata_drops_the_bytes_and_the_context() {
        let answer = json!({
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#…",
            "@odata.type": "#microsoft.graph.taskFileAttachment",
            "id": "A1", "name": "a.txt", "contentType": "text/plain", "size": 300,
            "lastModifiedDateTime": "2026-09-25T08:00:00Z", "contentBytes": "aGk="
        });
        let kept = metadata(answer.as_object().cloned().expect("object"));
        assert!(!kept.contains_key("contentBytes"));
        assert!(!kept.contains_key("@odata.context"));
        assert_eq!(kept["id"], "A1");
        assert_eq!(kept.len(), 6);
    }
}
