//! Sending an attachment's `child` operation (D-056).
//!
//! - **Add:** the file is read now, from the path queued with it, and
//!   refused if its size or modified time differ from when it was queued
//!   (it changed, and the user should say whether to send it as it is
//!   now). Then a POST, or an upload session over 3 MiB. Neither carries a
//!   marker to find the attachment by, so a lost answer to the POST or to
//!   the session's last PUT is `unknown`, flagged at once, and never sent
//!   again by itself (04).
//!   An upload session saves where it stands in the operation's
//!   `progress` before each step (D-067), so after a daemon restart it
//!   goes on in the same session, or a new one when that one can't be
//!   taken up; only a final PUT that may have gone is `unknown`. Each step
//!   is told to subscribers as `UploadProgress`.
//! - **Delete:** the attachment's bytes are kept first (`attachments::kept`),
//!   so `undo` can attach it again, and if they can't be, nothing is
//!   deleted (unless the user said `--no-undo`); then the DELETE. Graph ignores
//!   `If-Match` on it (S16), so none is sent. One gone already is done.

use ms_todo_core::ErrorKind;
use ms_todo_graph::{GraphError, NewAttachment, UploadState, UploadWatcher};
use ms_todo_protocol::{Event, UploadProgress};
use ms_todo_store::{ATTACHMENTS, ChildVerb, OutboxRow};
use serde_json::json;

use super::child_write::{done, fetch};
use super::send::{Attempt, Failure, classify};
use crate::attachments::{NO_UNDO, Source, kept};
use crate::handlers::{State, error_payload};

pub(super) async fn send(
    state: &State,
    list: &str,
    task: &str,
    op: &OutboxRow,
) -> Result<Attempt, Failure> {
    match ChildVerb::of(&op.payload) {
        Some(ChildVerb::Create) => create(state, list, task, op).await,
        Some(ChildVerb::Delete) => delete(state, list, task, op).await,
        _ => Err(Failure::Rejected(error_payload(
            ErrorKind::Internal,
            format!("operation {} doesn't say what it does", op.op_id),
        ))),
    }
}

async fn create(state: &State, list: &str, task: &str, op: &OutboxRow) -> Result<Attempt, Failure> {
    let body = op.body();
    let name = body["name"].as_str().unwrap_or("attachment");
    let content_type = body["contentType"]
        .as_str()
        .unwrap_or("application/octet-stream");
    let Some(source) = Source::of(&op.payload) else {
        return Err(rejected(format!(
            "operation {} doesn't say which file to attach",
            op.op_id
        )));
    };
    // The daemon's own copy (D-067) is done with once Graph has it.
    let staged = (op.payload["file"]["staged"] == true).then(|| source.path.clone());
    let bytes = read(source).await.map_err(rejected)?;
    let file = NewAttachment {
        name,
        content_type,
        bytes: &bytes,
    };
    let mut watcher = Progress {
        state,
        op,
        name,
        started: false,
    };
    let resume = saved_upload(op);
    let result = state
        .graph
        .add_attachment_watched(list, task, &file, resume, &mut watcher)
        .await;
    if watcher.started {
        let sent = if result.is_ok() { bytes.len() } else { 0 };
        watcher.tell(sent, bytes.len(), true);
    }
    if let Err(error) = &result
        && !matches!(error, GraphError::OutcomeUnknown(_))
    {
        // Nothing committed, and the session was left: the next attempt
        // starts a new one.
        forget_upload(state, op).await;
    }
    let created = result.map_err(classify)?;
    if let Some(path) = staged {
        // What's left, `staged::sweep` takes later.
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(Attempt::ChildCreated {
        created,
        task: fetch(state, list, task).await.ok(),
    })
}

/// The upload session an earlier attempt saved, if any.
fn saved_upload(op: &OutboxRow) -> Option<UploadState> {
    let saved = op.progress.as_ref()?.get(UPLOAD)?;
    serde_json::from_value(saved.clone()).ok()
}

/// Where an upload's session stands, in its operation's `progress`.
const UPLOAD: &str = "upload";

async fn forget_upload(state: &State, op: &OutboxRow) {
    if let Err(error) = state.store.set_progress(&op.op_id, &json!({})).await {
        eprintln!(
            "ms-todo daemon: cannot clear operation {}'s upload: {error}",
            op.op_id
        );
    }
}

/// Saves an upload session's state as it goes, and tells subscribers.
struct Progress<'a> {
    state: &'a State,
    op: &'a OutboxRow,
    name: &'a str,
    /// A session was used, so subscribers heard of it.
    started: bool,
}

impl Progress<'_> {
    fn tell(&self, sent: usize, total: usize, done: bool) {
        self.state
            .events
            .send(Event::UploadProgress(UploadProgress {
                op_id: self.op.op_id.clone(),
                task_id: self.op.entity_local_id.clone(),
                name: self.name.to_owned(),
                sent: sent as u64,
                total: total as u64,
                done,
            }));
    }
}

impl UploadWatcher for Progress<'_> {
    async fn save(&mut self, upload: &UploadState) -> Result<(), String> {
        let progress = json!({ UPLOAD: upload });
        self.state
            .store
            .set_progress(&self.op.op_id, &progress)
            .await
            .map_err(|error| error.to_string())?;
        self.started = true;
        self.tell(upload.next, upload.total, false);
        Ok(())
    }
}

/// The file's bytes, if it's as it was when queued.
async fn read(source: Source) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        let changed = || {
            format!(
                "{} changed after it was queued, so it wasn't attached; add it again to send it \
                 as it is now",
                source.path.display()
            )
        };
        let unchanged = |now: &Source| now.bytes == source.bytes && now.modified == source.modified;
        let before =
            Source::check(&source.path).map_err(|why| format!("{why}, so nothing was attached"))?;
        if !unchanged(&before) {
            return Err(changed());
        }
        let bytes = std::fs::read(&source.path)
            .map_err(|error| format!("cannot read {}: {error}", source.path.display()))?;
        let after = Source::check(&source.path).map_err(|_| changed())?;
        if !unchanged(&after) || bytes.len() as u64 != source.bytes {
            return Err(changed());
        }
        Ok(bytes)
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn delete(state: &State, list: &str, task: &str, op: &OutboxRow) -> Result<Attempt, Failure> {
    let id = op.payload["id"].as_str().unwrap_or_default();
    let kept = kept::path(&state.kept_dir, &op.op_id);
    // Kept already by an attempt that failed after keeping it.
    if op.payload[NO_UNDO] != true && !kept.is_file() {
        match state.graph.download_attachment(list, task, id).await {
            Ok(bytes) => {
                // Without its copy the delete couldn't be undone, which the
                // user didn't agree to: nothing is deleted.
                if let Err(error) = kept::keep(&state.kept_dir, &op.op_id, bytes).await {
                    return Err(rejected(format!(
                        "couldn't keep a copy for undo: {error}; nothing was deleted. \
                         `ms-todo attachments delete --no-undo` deletes it without one"
                    )));
                }
            }
            // Gone already: deleted, as asked.
            Err(error) if error.status() == Some(404) => return Ok(done(state, list, task).await),
            Err(error) => return Err(classify(error)),
        }
    }
    state
        .graph
        .delete_child(list, task, ATTACHMENTS, id, None)
        .await
        .map_err(classify)?;
    Ok(done(state, list, task).await)
}

fn rejected(message: String) -> Failure {
    Failure::Rejected(error_payload(ErrorKind::Rejected, message))
}

/// Whether a child operation is an attachment's.
pub(super) fn is_attachment(op: &OutboxRow) -> bool {
    op.payload["collection"] == ATTACHMENTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_file_changed_after_it_was_queued_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("invoice.pdf");
        std::fs::write(&path, b"first").expect("write");
        let queued = Source::check(&path).expect("checked");
        assert_eq!(read(queued.clone()).await.expect("unchanged"), b"first");

        std::fs::write(&path, b"second, longer").expect("write");
        let refused = read(queued.clone()).await.expect_err("changed");
        assert!(refused.contains("changed after it was queued"), "{refused}");

        std::fs::remove_file(&path).expect("remove");
        let gone = read(queued).await.expect_err("gone");
        assert!(gone.contains("no file"), "{gone}");
    }
}
