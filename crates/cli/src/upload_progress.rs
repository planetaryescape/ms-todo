//! `attachments add`'s progress line (D-067): in a terminal, an upload
//! big enough for an upload session is followed through the daemon's
//! `UploadProgress` events on stderr, one line rewritten in place, until
//! it's done or stops. The add itself was answered already: stopping
//! (Ctrl-C, or the upload not starting soon) only stops watching, and the
//! daemon carries on.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use ms_todo_core::Paths;
use ms_todo_protocol::{Applied, Event, UploadProgress};
use tokio::time::Instant;

use crate::daemon_client::{self, DaemonClient};

/// Smaller than this is one POST, with no progress to show (Graph's limit
/// for a direct upload).
const SESSION_BYTES: u64 = 3 * 1024 * 1024;
/// How long to wait for an upload to start: the outbox may be busy, or
/// offline.
const START_WITHIN: Duration = Duration::from_secs(15);
/// How long a started upload may go without a word.
const STALL_WITHIN: Duration = Duration::from_secs(120);

/// A subscription opened before the add is sent, so no event is missed.
pub struct Watch {
    client: DaemonClient,
    id: u64,
}

/// Subscribe, when there's a terminal to show progress in. A failure
/// here only means no progress line.
pub async fn subscribe(paths: &Paths, dry_run: bool) -> Option<Watch> {
    if dry_run || !std::io::stderr().is_terminal() || crate::terminal::quiet() {
        return None;
    }
    let (mut client, _) = daemon_client::connect(paths).await.ok()?;
    let id = client.subscribe().await.ok()?;
    Some(Watch { client, id })
}

impl Watch {
    /// Follow the uploads `applied` queued that go through a session.
    pub async fn follow(mut self, applied: &Applied) {
        let mut waiting = big_uploads(applied);
        if waiting == 0 {
            return;
        }
        let ours = |op_id: &str| {
            op_id == applied.op_id || op_id.starts_with(&format!("{}.", applied.op_id))
        };
        let mut deadline = Instant::now() + START_WITHIN;
        let mut shown = false;
        while waiting > 0 {
            let Some(event) = self.client.next_event(self.id, deadline).await else {
                if shown {
                    eprintln!();
                }
                eprintln!(
                    "The upload carries on in the background; `ms-todo attachments list` shows \
                     it once it's done."
                );
                return;
            };
            match event {
                Event::UploadProgress(progress) if ours(&progress.op_id) => {
                    deadline = Instant::now() + STALL_WITHIN;
                    show(&progress);
                    shown = true;
                    if progress.done {
                        waiting -= 1;
                        eprintln!();
                        shown = false;
                        if progress.sent < progress.total {
                            eprintln!(
                                "{} stopped part-way; `ms-todo outbox list` says what happens next.",
                                ms_todo_core::display_safe(&progress.name)
                            );
                        }
                    }
                }
                Event::WriteRejected(rejected) if ours(&rejected.op_id) => {
                    if shown {
                        eprintln!();
                    }
                    return;
                }
                _ => {}
            }
        }
    }
}

/// How many of the attachments `applied` added are big enough for an
/// upload session: those it answered still uploading, by their size.
fn big_uploads(applied: &Applied) -> usize {
    applied
        .items
        .iter()
        .flat_map(|task| task.get("attachments").and_then(|all| all.as_array()))
        .flatten()
        .filter(|attachment| {
            attachment["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(ms_todo_core::LOCAL_CHILD_PREFIX))
                && attachment["size"].as_u64().unwrap_or(0) >= SESSION_BYTES
        })
        .count()
}

// One line, rewritten in place.
fn show(progress: &UploadProgress) {
    let percent = progress.percent();
    let mut stderr = std::io::stderr().lock();
    let _ = write!(
        stderr,
        "\r\x1b[2KUploading {}: {} of {} ({percent}%)",
        ms_todo_core::display_safe(&progress.name),
        megabytes(progress.sent),
        megabytes(progress.total),
    );
    let _ = stderr.flush();
}

// 1024-based, as the TUI's detail pane shows a file's size.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ms_todo_protocol::TaskAction;
    use serde_json::json;

    #[test]
    fn only_uploads_still_going_through_a_session_are_followed() {
        let task = json!({ "attachments": [
            { "id": "local-1", "size": SESSION_BYTES },
            { "id": "local-2", "size": 10 },
            { "id": "AAMk", "size": SESSION_BYTES * 2 }
        ] });
        let applied = Applied {
            op_id: "op".into(),
            action: TaskAction::AttachmentAdd,
            items: vec![task.as_object().cloned().expect("object")],
            list_ids: Vec::new(),
            rolled: Vec::new(),
            undoes: None,
            refused: Vec::new(),
        };
        assert_eq!(big_uploads(&applied), 1);
    }
}
