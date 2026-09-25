//! Moving a task to another list (docs/blueprint/05-custom-features.md#move-between-lists,
//! docs/blueprint/04-sync-cache.md#instant-local-writes, D-019, D-051).
//! Graph has no move, so a move is a copy that checks its work before it
//! deletes anything, run as one outbox operation with saved steps
//! ([`Progress`]):
//!
//! 1. **Prepare:** read the source, its extension and its attachments'
//!    bytes. Nothing is written.
//! 2. **Create** the copy in one POST with every field, the checklist
//!    items, the linked resource and our extension carrying this
//!    operation's `opId` and `originalCreatedAt` (S13, S14). The task keeps
//!    its local ID and takes the copy's Graph ID.
//! 3. **Attachments:** add each to the copy, one after another.
//! 4. **Verify:** the target list answers 200 (the ghost-write check), the
//!    copy read back matches the source field by field with the same
//!    attachments byte for byte, and the source hasn't changed meanwhile.
//! 5. **Delete** the source. Only now.
//!
//! A step that fails for good before the delete rolls the move back: the
//! partial copy is deleted and the source was never touched. That's only
//! done when every step's outcome is known; a create with no answer pauses
//! the move as `unknown` instead, deleting nothing. Once the delete may
//! have started, the copy is never deleted: a resumed move reads both and
//! decides (04's four cases).

mod copy;
pub(crate) mod progress;
mod spool;

use ms_todo_core::ErrorKind;
use ms_todo_graph::{GraphError, MAX_ATTACHMENT_BYTES};
use ms_todo_protocol::ErrorPayload;
use ms_todo_store::{Entity, OutboxRow, Restore, StoreError};
use serde_json::Value;

use copy::{ORIGINAL_CREATED_AT, copy_body, original_created_at};
pub(crate) use copy::{comparable, differences};
use progress::{AttachmentStep, Source};
pub(crate) use progress::{Progress, Stage, StepState, from_list};
pub(crate) use spool::remove as remove_spool;

use super::send::{Failure, classify};
use crate::entities::{EXTENSION_NAME, split_extension};
use crate::handlers::{State, error_payload, store_error};

/// How one attempt at a move ended. The steps taken are saved either way.
pub(super) enum Settled {
    /// The copy checks out and the source is gone; recorded.
    Done,
    /// Nothing is sent again until the outcome is found or the user
    /// decides; `note` says why, for `outbox list`.
    Paused { error: ErrorPayload, note: String },
    /// Rolled back: the partial copy is gone and the source untouched.
    Failed(ErrorPayload),
    /// Try again after a backoff, from the step it stopped at.
    Temporary(ErrorPayload),
}

/// One move, while an attempt runs.
struct Job<'a> {
    state: &'a State,
    op: &'a OutboxRow,
    target: String,
}

/// Run the move `op` from the step it had reached until it's settled. A
/// step that ends the attempt returns `Err`, `Done` included.
pub(super) async fn run(state: &State, op: &OutboxRow) -> Settled {
    let mut progress = Progress::of(op);
    let target = match list_graph_id(state, &op.list_local_id).await {
        Ok(target) => target,
        // The source may be gone already: the task is kept here for the
        // user, as when both are lost.
        Err(Settled::Failed(error)) if progress.delete_started => {
            return Settled::Paused {
                error,
                note: "the list it was moving to was deleted on another device after the \
                       original was deleted; ms-todo kept the whole task. `ms-todo outbox \
                       discard` lets it go"
                    .into(),
            };
        }
        Err(settled) => return settled,
    };
    let job = Job { state, op, target };
    loop {
        let step = match progress.stage {
            Stage::Prepare => job.prepare(&mut progress).await,
            Stage::Create => job.create(&mut progress).await,
            Stage::Attachments => job.attachments(&mut progress).await,
            Stage::Verify => job.verify(&mut progress).await,
            Stage::Delete => job.delete(&mut progress).await,
            Stage::RollBack => job.roll_back(&mut progress).await,
        };
        if let Err(settled) = step {
            return settled;
        }
    }
}

/// A list's Graph ID, if it's cached and live: a move never starts into or
/// out of a list known to be gone.
async fn list_graph_id(state: &State, local_id: &str) -> Result<String, Settled> {
    match state.store.list_state(local_id).await {
        Ok(Some((Some(graph_id), false))) => Ok(graph_id),
        Ok(_) => Err(Settled::Failed(error_payload(
            ErrorKind::Rejected,
            "the list was deleted on another device, so the task wasn't moved; it's kept where \
             it was"
                .into(),
        ))),
        Err(error) => Err(Settled::Temporary(store_error(error))),
    }
}

/// What putting the task back where the move found it does to its row:
/// back in its list, as the source was, with the source's Graph ID. A task
/// being re-created from its local copy (both were lost) has no source to
/// go back to: like a failed create, it's tombstoned, and its content
/// stays in the failed move.
pub(crate) async fn move_back(state: &State, op: &OutboxRow) -> Result<Restore, StoreError> {
    let progress = Progress::of(op);
    let from_list = from_list(op);
    Ok(match progress.source {
        Some(Source { graph_id: None, .. }) => Restore::Tombstone,
        Some(source) => Restore::MoveBack {
            graph_id: source.graph_id,
            list_local_id: from_list,
            raw: source.raw,
        },
        None => match state.store.task_any(&op.entity_local_id).await? {
            Some((row, _)) => Restore::MoveBack {
                graph_id: row.graph_id,
                list_local_id: from_list,
                raw: row.raw,
            },
            None => Restore::Nothing,
        },
    })
}

/// `outbox retry` of a move: a resend the user chose. A paused move goes
/// on from where it stopped, sending again what had no answer; one whose
/// source and copy were both lost is re-created in the target list from
/// its local copy; a failed one starts over, and shows in the target list
/// again. Returns the local change to make and the steps to save.
pub(crate) async fn retry(state: &State, op: &OutboxRow) -> Result<(Restore, Value), ErrorPayload> {
    let mut progress = Progress::of(op);
    if op.state == ms_todo_store::OpState::Failed {
        let Some((row, _)) = state
            .store
            .task_any(&op.entity_local_id)
            .await
            .map_err(store_error)?
        else {
            return Err(error_payload(
                ErrorKind::NotFound,
                format!("{} isn't cached any more", op.entity_local_id),
            ));
        };
        let restore = Restore::MoveBack {
            graph_id: row.graph_id,
            list_local_id: op.list_local_id.clone(),
            raw: row.raw,
        };
        return Ok((restore, Progress::default().to_value()));
    }
    if progress.both_missing {
        // Everything but the local copy starts again, into the target list.
        let source = progress.source.map(|source| Source {
            graph_id: None,
            ..source
        });
        let attachments = progress
            .attachments
            .into_iter()
            .map(|step| AttachmentStep {
                state: StepState::Pending,
                ..step
            })
            .collect();
        let recreate = Progress {
            stage: Stage::Create,
            source,
            attachments,
            ..Progress::default()
        };
        return Ok((Restore::Nothing, recreate.to_value()));
    }
    progress.in_doubt = false;
    progress.needs_user = false;
    for step in &mut progress.attachments {
        if step.state == StepState::Sent {
            step.state = StepState::Pending;
        }
    }
    Ok((Restore::Nothing, progress.to_value()))
}

/// `outbox discard` of a move: what happens to the task, and whether both
/// lists must be read whole again. A move not started yet, or paused, puts
/// the task back where it was (a paused one's copy, if Graph made it, then
/// arrives by sync as a task of its own: nothing is deleted); one whose
/// source and copy were both lost lets its local copy go. A move that has
/// begun copying and isn't paused can't be discarded: it carries on.
pub(crate) async fn discard(
    state: &State,
    op: &OutboxRow,
) -> Result<(Restore, bool), ErrorPayload> {
    let progress = Progress::of(op);
    match op.state {
        ms_todo_store::OpState::Failed => Ok((Restore::Nothing, false)),
        ms_todo_store::OpState::Pending if progress.wrote_anything() => Err(error_payload(
            ErrorKind::InvalidInput,
            format!(
                "move {} has started copying the task, so it can't be dropped now; it carries on \
                 by itself (see `ms-todo outbox list`)",
                op.op_id
            ),
        )),
        _ if progress.both_missing => Ok((Restore::Tombstone, true)),
        _ => Ok((move_back(state, op).await.map_err(store_error)?, true)),
    }
}

type Step = Result<(), Settled>;

impl Job<'_> {
    async fn save(&self, progress: &Progress) -> Step {
        self.state
            .store
            .set_progress(&self.op.op_id, &progress.to_value())
            .await
            .map_err(|error| Settled::Temporary(store_error(error)))
    }

    fn spool_root(&self) -> &std::path::Path {
        &self.state.moves_dir
    }

    /// Read the source whole: its fields, children, extension and every
    /// attachment's bytes. Only reads, so it's simply done again after a
    /// temporary failure.
    async fn prepare(&self, progress: &mut Progress) -> Step {
        let list_graph_id = list_graph_id(self.state, &from_list(self.op)).await?;
        let row = self
            .state
            .store
            .task_any(&self.op.entity_local_id)
            .await
            .map_err(|error| Settled::Temporary(store_error(error)))?
            .map(|(row, _)| row);
        let Some(graph_id) = row.and_then(|row| row.graph_id) else {
            return Err(Settled::Failed(error_payload(
                ErrorKind::Rejected,
                "the task was never created in Microsoft To Do, so there's nothing to move".into(),
            )));
        };
        let fetched = self
            .state
            .graph
            .get_task_with_extension(&list_graph_id, &graph_id, EXTENSION_NAME)
            .await;
        let (raw, extension) = match fetched {
            Ok(task) => split_extension(task),
            Err(error) if error.status() == Some(404) => {
                return Err(Settled::Failed(error_payload(
                    ErrorKind::NotFound,
                    "the task was deleted on another device, so it wasn't moved".into(),
                )));
            }
            Err(error) => return Err(read_failure(error)),
        };
        let mut attachments = Vec::new();
        if raw.get("hasAttachments") == Some(&Value::Bool(true)) {
            let listed = self
                .state
                .graph
                .list_attachments(&list_graph_id, &graph_id)
                .await
                .map_err(read_failure)?;
            for (index, attachment) in listed.iter().enumerate() {
                attachments.push(
                    self.fetch_attachment(&list_graph_id, &graph_id, index, attachment)
                        .await?,
                );
            }
        }
        progress.source = Some(Source {
            list_graph_id,
            graph_id: Some(graph_id),
            raw,
            extension: extension.flatten(),
        });
        progress.attachments = attachments;
        progress.stage = Stage::Create;
        self.save(progress).await
    }

    /// Download one of the source's attachments into the spool. One over
    /// Graph's 25 MB is refused before anything is written.
    async fn fetch_attachment(
        &self,
        list_graph_id: &str,
        task_graph_id: &str,
        index: usize,
        attachment: &Entity,
    ) -> Result<AttachmentStep, Settled> {
        let name = text(attachment, "name").unwrap_or_else(|| format!("attachment-{index}"));
        let Some(id) = text(attachment, "id") else {
            return Err(Settled::Failed(error_payload(
                ErrorKind::Decode,
                format!("Graph listed the attachment {name:?} without an ID"),
            )));
        };
        let bytes = self
            .state
            .graph
            .download_attachment(list_graph_id, task_graph_id, &id)
            .await
            .map_err(read_failure)?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(Settled::Failed(error_payload(
                ErrorKind::InvalidInput,
                format!(
                    "the attachment {name:?} is over 25 MB, which Microsoft To Do can't take, so \
                     the task wasn't moved"
                ),
            )));
        }
        let spool = index.to_string();
        let size = bytes.len();
        let sha256 = spool::write(self.spool_root(), &self.op.op_id, &spool, bytes)
            .await
            .map_err(|error| {
                Settled::Temporary(error_payload(
                    ErrorKind::Internal,
                    format!("cannot keep the attachment {name:?} for the move: {error}"),
                ))
            })?;
        Ok(AttachmentStep {
            content_type: text(attachment, "contentType")
                .unwrap_or_else(|| "application/octet-stream".into()),
            name,
            bytes: size,
            sha256,
            spool,
            state: StepState::Pending,
        })
    }

    /// Create the copy, with its children and our extension, in one POST.
    async fn create(&self, progress: &mut Progress) -> Step {
        if progress.copy_id.is_some() {
            progress.stage = Stage::Attachments;
            return self.save(progress).await;
        }
        let source = source(progress)?;
        let body = copy_body(&source.raw, source.extension.as_ref(), &self.op.op_id);
        progress.in_doubt = true;
        self.save(progress).await?;
        match self.state.graph.create_task(&self.target, &body).await {
            Ok(created) => {
                let (raw, extension) = split_extension(created);
                progress.copy_id = text(&raw, "id");
                progress.in_doubt = false;
                progress.stage = Stage::Attachments;
                // If the cache can't take it, the move stays in doubt, and
                // the copy is found by its opId, as after a lost answer.
                self.state
                    .store
                    .record_move(&self.op.op_id, &raw, extension, &progress.to_value(), false)
                    .await
                    .map_err(|error| Settled::Paused {
                        error: store_error(error),
                        note: "Graph created the copy, but the cache couldn't record it; it's \
                               found by its opId after the next sync"
                            .into(),
                    })?;
                self.state
                    .events
                    .tasks_changed(vec![self.op.entity_local_id.clone()]);
                Ok(())
            }
            Err(error) => match classify(error) {
                Failure::Unknown(error) => Err(Settled::Paused {
                    error,
                    note: "creating the copy got no answer; nothing is sent again, and the copy \
                           is looked for by its opId after each sync"
                        .into(),
                }),
                Failure::Rejected(error) => {
                    progress.in_doubt = false;
                    self.start_roll_back(progress, &error).await
                }
                Failure::Temporary(error) => {
                    progress.in_doubt = false;
                    self.save(progress).await?;
                    Err(Settled::Temporary(error))
                }
            },
        }
    }

    /// Add each attachment to the copy, one after another. An upload with
    /// no answer can't be attributed, so it pauses the move for the user.
    async fn attachments(&self, progress: &mut Progress) -> Step {
        let copy_id = copy_id(progress)?;
        for index in 0..progress.attachments.len() {
            if progress.attachments[index].state == StepState::Done {
                continue;
            }
            let step = progress.attachments[index].clone();
            let bytes = match spool::read(self.spool_root(), &self.op.op_id, &step.spool).await {
                Ok((bytes, hash)) if hash == step.sha256 => bytes,
                Ok(_) | Err(_) => {
                    let error = error_payload(
                        ErrorKind::Internal,
                        format!(
                            "the attachment {:?} read from the original is gone from ms-todo's \
                             spool, so the move was undone; move it again",
                            step.name
                        ),
                    );
                    return self.start_roll_back(progress, &error).await;
                }
            };
            progress.attachments[index].state = StepState::Sent;
            progress.in_doubt = true;
            progress.needs_user = true;
            self.save(progress).await?;
            let sent = self
                .state
                .graph
                .add_attachment(
                    &self.target,
                    &copy_id,
                    &step.name,
                    &step.content_type,
                    &bytes,
                )
                .await;
            let failure = match sent {
                Ok(()) => {
                    progress.attachments[index].state = StepState::Done;
                    progress.in_doubt = false;
                    progress.needs_user = false;
                    self.save(progress).await?;
                    continue;
                }
                Err(error) => classify(error),
            };
            if let Failure::Unknown(error) = failure {
                return Err(Settled::Paused {
                    error,
                    note: format!(
                        "the move is paused: adding the attachment {:?} to the copy got no \
                         answer, and ms-todo never sends it again by itself. Both tasks are \
                         kept. `ms-todo outbox retry` sends it again (it may then be attached \
                         twice); `ms-todo outbox discard` leaves both",
                        step.name
                    ),
                });
            }
            // Refused, or never sent: its outcome is known.
            progress.attachments[index].state = StepState::Pending;
            progress.in_doubt = false;
            progress.needs_user = false;
            if let Failure::Rejected(error) = failure {
                return self.start_roll_back(progress, &error).await;
            }
            self.save(progress).await?;
            if let Failure::Temporary(error) = failure {
                return Err(Settled::Temporary(error));
            }
        }
        progress.stage = Stage::Verify;
        self.save(progress).await
    }

    /// Check the copy before anything is deleted.
    async fn verify(&self, progress: &mut Progress) -> Step {
        let copy_id = copy_id(progress)?;
        let source = source(progress)?.clone();
        // The ghost-write check (04, S4): a POST into a list deleted
        // meanwhile still answers 201.
        match self.state.graph.get_list(&self.target).await {
            Ok(_) => {}
            Err(error) if error.status() == Some(404) => {
                let error = error_payload(
                    ErrorKind::Rejected,
                    "the list it was moving to was deleted on another device, so the move was \
                     undone; the task is where it was"
                        .into(),
                );
                return self.start_roll_back(progress, &error).await;
            }
            Err(error) => return self.read_failed(progress, error).await,
        }
        let fetched = self
            .state
            .graph
            .get_task_with_extension(&self.target, &copy_id, EXTENSION_NAME)
            .await;
        let (copy, copy_extension) = match fetched {
            Ok(task) => split_extension(task),
            Err(error) if error.status() == Some(404) => {
                let error = error_payload(
                    ErrorKind::Conflict,
                    "the copy was deleted on another device before it was checked, so the move \
                     was undone; the task is where it was"
                        .into(),
                );
                return self.start_roll_back(progress, &error).await;
            }
            Err(error) => return self.read_failed(progress, error).await,
        };
        let copy_extension = copy_extension.flatten();
        let copied = match self.copied_attachments(&copy_id).await {
            Ok(copied) => copied,
            Err(error) => return self.read_failed(progress, error).await,
        };
        let mut wrong = differences(
            &comparable(&source.raw, source.extension.as_ref()),
            &comparable(&copy, copy_extension.as_ref()),
        );
        let original = original_created_at(&source.raw, source.extension.as_ref());
        if original.is_some()
            && copy_extension
                .as_ref()
                .and_then(|extension| extension.get(ORIGINAL_CREATED_AT))
                .and_then(Value::as_str)
                != original.as_deref()
        {
            wrong.push(ORIGINAL_CREATED_AT.into());
        }
        let mut expected: Vec<(String, usize, String)> = progress
            .attachments
            .iter()
            .map(|step| (step.name.clone(), step.bytes, step.sha256.clone()))
            .collect();
        expected.sort();
        if copied != expected {
            wrong.push("attachments".into());
        }
        if !wrong.is_empty() {
            let error = error_payload(
                ErrorKind::Rejected,
                format!(
                    "the copy didn't match the original ({}), so it was deleted and the original \
                     kept",
                    wrong.join(", ")
                ),
            );
            return self.start_roll_back(progress, &error).await;
        }
        progress.copy = Some(copy);
        progress.copy_extension = copy_extension;
        let Some(source_id) = source.graph_id.clone() else {
            // Re-created from the local copy: there's no source to delete.
            return self.finish(progress).await;
        };
        let now = self
            .state
            .graph
            .get_task(&source.list_graph_id, &source_id)
            .await;
        match now {
            Ok(now) if now.get("@odata.etag") != source.raw.get("@odata.etag") => {
                let error = error_payload(
                    ErrorKind::Conflict,
                    "the task changed on another device while it was being moved, so the copy \
                     was deleted and the original kept with that change; move it again"
                        .into(),
                );
                self.start_roll_back(progress, &error).await
            }
            Ok(_) => {
                progress.stage = Stage::Delete;
                self.save(progress).await
            }
            // Deleted on another device meanwhile: the checked copy is all
            // that's left, so it stays.
            Err(error) if error.status() == Some(404) => self.finish(progress).await,
            Err(error) => self.read_failed(progress, error).await,
        }
    }

    /// The copy's attachments as `(name, bytes, sha256)`, sorted.
    async fn copied_attachments(
        &self,
        copy_id: &str,
    ) -> Result<Vec<(String, usize, String)>, GraphError> {
        let listed = self
            .state
            .graph
            .list_attachments(&self.target, copy_id)
            .await?;
        let mut copied = Vec::new();
        for attachment in &listed {
            let id = text(attachment, "id").unwrap_or_default();
            let bytes = self
                .state
                .graph
                .download_attachment(&self.target, copy_id, &id)
                .await?;
            let size = bytes.len();
            let (_, hash) = spool::hash(bytes)
                .await
                .map_err(|error| GraphError::Decode(error.to_string()))?;
            copied.push((text(attachment, "name").unwrap_or_default(), size, hash));
        }
        copied.sort();
        Ok(copied)
    }

    /// Delete the source. From the moment its DELETE may be sent, the copy
    /// is never deleted: a resumed move reads both and decides (04).
    async fn delete(&self, progress: &mut Progress) -> Step {
        let source = source(progress)?.clone();
        let copy_id = copy_id(progress)?;
        let Some(source_id) = source.graph_id.clone() else {
            return self.finish(progress).await;
        };
        if progress.delete_started {
            return self
                .recover_delete(progress, &source, &source_id, &copy_id)
                .await;
        }
        progress.delete_started = true;
        self.save(progress).await?;
        self.delete_source(progress, &source, &source_id).await
    }

    async fn delete_source(
        &self,
        progress: &mut Progress,
        source: &Source,
        source_id: &str,
    ) -> Step {
        // A 404 counts as deleted.
        match self
            .state
            .graph
            .delete_task(&source.list_graph_id, source_id)
            .await
        {
            Ok(()) => self.finish(progress).await,
            Err(error) => {
                self.refused(progress, error, |why| {
                    format!(
                        "the copy is complete and checked, but Microsoft To Do refused to \
                         delete the original ({why}); both are kept. `ms-todo outbox retry` \
                         tries the delete again"
                    )
                })
                .await
            }
        }
    }

    /// The source's DELETE may have been sent before: read both tasks and
    /// act on what's there, never deleting the copy (04).
    async fn recover_delete(
        &self,
        progress: &mut Progress,
        source: &Source,
        source_id: &str,
        copy_id: &str,
    ) -> Step {
        let source_there = match self
            .state
            .graph
            .get_task(&source.list_graph_id, source_id)
            .await
        {
            Ok(_) => true,
            Err(error) if error.status() == Some(404) => false,
            Err(error) => return Err(read_failure(error)),
        };
        let copy = match self
            .state
            .graph
            .get_task_with_extension(&self.target, copy_id, EXTENSION_NAME)
            .await
        {
            Ok(copy) => Some(split_extension(copy)),
            Err(error) if error.status() == Some(404) => None,
            Err(error) => return Err(read_failure(error)),
        };
        match (source_there, copy) {
            (false, Some((copy, extension))) => {
                progress.copy = Some(copy);
                if let Some(extension) = extension.flatten() {
                    progress.copy_extension = Some(extension);
                }
                self.finish(progress).await
            }
            (true, Some(_)) => self.delete_source(progress, source, source_id).await,
            (true, None) => {
                // Nothing is lost: the original is there. The copy is
                // already gone, so rolling back deletes nothing.
                progress.copy_id = None;
                let error = error_payload(
                    ErrorKind::Conflict,
                    "the copy was deleted on another device before the move finished; the \
                     original is kept where it was"
                        .into(),
                );
                self.start_roll_back(progress, &error).await
            }
            (false, None) => {
                progress.both_missing = true;
                progress.needs_user = true;
                self.save(progress).await?;
                Err(Settled::Paused {
                    error: error_payload(
                        ErrorKind::OutcomeUnknown,
                        "both the original and the copy are gone from Microsoft To Do".into(),
                    ),
                    note: "both the original and the copy are gone from Microsoft To Do (deleted \
                           on another device?). ms-todo kept the whole task, attachments \
                           included, and re-creates or deletes nothing by itself: `ms-todo \
                           outbox retry` re-creates it in the list it was moving to, `ms-todo \
                           outbox discard` lets it go"
                        .into(),
                })
            }
        }
    }

    /// The move is done: the task is the checked copy, `progress.copy`.
    async fn finish(&self, progress: &mut Progress) -> Step {
        let Some(copy) = progress.copy.clone() else {
            return Err(corrupt("the checked copy"));
        };
        progress.needs_user = false;
        progress.both_missing = false;
        self.state
            .store
            .record_move(
                &self.op.op_id,
                &copy,
                Some(progress.copy_extension.clone()),
                &progress.to_value(),
                true,
            )
            .await
            .map_err(|error| Settled::Temporary(store_error(error)))?;
        spool::remove(self.spool_root(), &self.op.op_id).await;
        Err(Settled::Done)
    }

    /// A write failed: refused for good, the move pauses for the user with
    /// `note` (given Graph's reason); otherwise it's tried again later.
    /// Deletes are idempotent, so no outcome here is unknown.
    async fn refused(
        &self,
        progress: &mut Progress,
        error: GraphError,
        note: impl FnOnce(&str) -> String,
    ) -> Step {
        match classify(error) {
            Failure::Rejected(error) => {
                progress.needs_user = true;
                self.save(progress).await?;
                Err(Settled::Paused {
                    note: note(&error.message),
                    error,
                })
            }
            Failure::Temporary(error) | Failure::Unknown(error) => Err(Settled::Temporary(error)),
        }
    }

    /// Roll the move back from here, for `error`.
    async fn start_roll_back(&self, progress: &mut Progress, error: &ErrorPayload) -> Step {
        progress.rollback = Some((error.kind.clone(), error.message.clone()));
        progress.stage = Stage::RollBack;
        self.save(progress).await
    }

    /// Delete the partial copy, then fail the move. Never once the source's
    /// DELETE may have started.
    async fn roll_back(&self, progress: &mut Progress) -> Step {
        if let Some(copy_id) = progress.copy_id.clone()
            && !progress.delete_started
        {
            if let Err(error) = self.state.graph.delete_task(&self.target, &copy_id).await {
                return self
                    .refused(progress, error, |why| {
                        format!(
                            "the move was being undone, but Microsoft To Do refused to delete \
                             the partial copy ({why}); the original is untouched"
                        )
                    })
                    .await;
            }
            progress.copy_id = None;
            self.save(progress).await?;
        }
        let (kind, message) = progress
            .rollback
            .clone()
            .unwrap_or_else(|| ("rejected".into(), "the move was undone".into()));
        Err(Settled::Failed(ErrorPayload {
            kind,
            message,
            ..ErrorPayload::default()
        }))
    }

    /// A read during the check failed: a temporary failure waits, and a
    /// refusal rolls the move back.
    async fn read_failed(&self, progress: &mut Progress, error: GraphError) -> Step {
        match classify(error) {
            Failure::Rejected(error) => self.start_roll_back(progress, &error).await,
            Failure::Temporary(error) | Failure::Unknown(error) => Err(Settled::Temporary(error)),
        }
    }
}

fn read_failure(error: GraphError) -> Settled {
    match classify(error) {
        Failure::Rejected(error) => Settled::Failed(error),
        Failure::Temporary(error) | Failure::Unknown(error) => Settled::Temporary(error),
    }
}

fn source(progress: &Progress) -> Result<&Source, Settled> {
    progress
        .source
        .as_ref()
        .ok_or_else(|| corrupt("its source"))
}

fn copy_id(progress: &Progress) -> Result<String, Settled> {
    progress.copy_id.clone().ok_or_else(|| corrupt("its copy"))
}

fn corrupt(what: &str) -> Settled {
    Settled::Paused {
        error: error_payload(
            ErrorKind::Internal,
            format!("the move lost track of {what}"),
        ),
        note: format!(
            "the move's saved steps don't record {what}; nothing more is sent. `ms-todo outbox \
             discard` puts the task back"
        ),
    }
}

fn text(entity: &Entity, key: &str) -> Option<String> {
    entity.get(key).and_then(Value::as_str).map(str::to_owned)
}
