//! `ms-todo outbox list|retry|discard` (docs/blueprint/07-cli.md): the
//! user's view of the queue, and their way to resolve what the worker
//! won't decide alone.
//!
//! - `retry` sends an `unknown` or `failed` operation again (a resend the
//!   user chose: an `unknown` create may then exist twice). A `failed`
//!   one's local change is made again first.
//! - `discard` drops an operation that isn't `done` or being sent. One that
//!   never reached Graph (`pending`) has its local change undone; an
//!   `unknown` create's task goes (Graph's copy, if it exists, arrives by
//!   sync), and for any other `unknown` one the list is read whole on the
//!   next pass. A `failed` one was rolled back already. Whatever waited on
//!   it fails with it.
//!
//! Both act only if the operation is still in the state they read: one
//! conditional update in the store, so neither races the worker. If it
//! moved, they change nothing and answer `conflict`.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::{ErrorPayload, OpError, OutboxOp, OutboxState, ResponseData};
use ms_todo_store::{OpKind, OpState, OutboxRow, Restore, apply_body, merge_extension};

use super::rollback::{announce, current_of, reconcile, reconcile_list, undo_local};
use super::{move_job, now};
use crate::handlers::{State, error_payload, store_error};

pub(crate) async fn list(
    state: &State,
    wanted: Option<OutboxState>,
) -> Result<ResponseData, ErrorPayload> {
    let wanted = match wanted {
        None => None,
        Some(wanted) => Some(store_state(wanted)?),
    };
    let ops = state.store.outbox(wanted).await.map_err(store_error)?;
    let now = now();
    Ok(ResponseData::Outbox {
        items: ops.iter().map(|op| outbox_op(op, now)).collect(),
    })
}

pub(crate) async fn retry(state: &State, op_id: &str) -> Result<ResponseData, ErrorPayload> {
    let op = find(state, op_id).await?;
    if let Some(dependency) = &op.depends_on {
        let found = state
            .store
            .outbox_op(dependency)
            .await
            .map_err(store_error)?;
        let blocked = match &found {
            Some(found) if found.state == OpState::Done => None,
            Some(found) => Some(format!("is {}", found.state.as_str())),
            None => Some("was discarded".to_owned()),
        };
        if let Some(why) = blocked {
            // Resent, it would build on a change that didn't happen: an
            // undo's re-create of a task whose delete failed would make a
            // second copy.
            return Err(error_payload(
                ErrorKind::Conflict,
                format!(
                    "operation {} was queued on {dependency}, which {why}, so it can't be sent \
                     on its own; retry {dependency} first, or discard {}",
                    op.op_id, op.op_id
                ),
            ));
        }
    }
    let mut progress = None;
    let restore = match op.state {
        OpState::Unknown | OpState::Failed if op.op == OpKind::Move => {
            let (restore, steps) = move_job::retry(state, &op).await?;
            progress = Some(steps);
            restore
        }
        OpState::Unknown => Restore::Nothing,
        OpState::Failed => redo_local(state, &op).await?,
        OpState::Pending => {
            return Err(error_payload(
                ErrorKind::InvalidInput,
                format!(
                    "operation {} is queued already; it's sent as soon as it can be",
                    op.op_id
                ),
            ));
        }
        OpState::Inflight => return Err(being_sent(&op)),
        OpState::Done => return Err(already_done(&op)),
    };
    let requeued = state
        .store
        .requeue(&op.op_id, op.state, &restore, progress.as_ref())
        .await
        .map_err(store_error)?;
    if !requeued {
        return Err(moved(&op));
    }
    announce(state, &op);
    state.outbox.wake();
    current(state, &op.op_id).await
}

pub(crate) async fn discard(state: &State, op_id: &str) -> Result<ResponseData, ErrorPayload> {
    let op = find(state, op_id).await?;
    match op.state {
        OpState::Inflight => return Err(being_sent(&op)),
        OpState::Done => return Err(already_done(&op)),
        OpState::Pending | OpState::Unknown | OpState::Failed => {}
    }
    let current = current_of(state, &op).await.map_err(store_error)?;
    let (restore, read_again) = match (op.state, op.op) {
        (_, OpKind::Move) => move_job::discard(state, &op).await?,
        (OpState::Pending, _) => (undo_local(&op, current.as_ref()), false),
        (OpState::Unknown, OpKind::Create) => (Restore::Tombstone, false),
        (OpState::Unknown, _) => (Restore::Nothing, true),
        _ => (Restore::Nothing, false),
    };
    let cascaded = state
        .store
        .discard_op(&op.op_id, op.state, &restore)
        .await
        .map_err(store_error)?
        .ok_or_else(|| moved(&op))?;
    if read_again || !cascaded.is_empty() {
        reconcile(state, &op).await;
    }
    if op.op == OpKind::Move {
        reconcile_list(state, &move_job::from_list(&op)).await;
        move_job::remove_spool(&state.moves_dir, &op.op_id).await;
    }
    announce(state, &op);
    let cause = format!("not sent: it waited on {}, which was discarded", op.op_id);
    for waiter in &cascaded {
        state
            .events
            .write_rejected(waiter, &op.entity_local_id, "rejected", &cause);
    }
    Ok(ResponseData::OutboxOp(outbox_op(&op, now())))
}

/// A `failed` operation's local change, made again for a retry.
async fn redo_local(state: &State, op: &OutboxRow) -> Result<Restore, ErrorPayload> {
    let Some(current) = current_of(state, op).await.map_err(store_error)? else {
        return Err(error_payload(
            ErrorKind::NotFound,
            format!("{} isn't cached any more", op.entity_local_id),
        ));
    };
    Ok(match op.op {
        // The task's content is still in its row, tombstoned.
        OpKind::Create => Restore::Replace(current),
        OpKind::Update => {
            let mut raw = current;
            apply_body(&mut raw, op.body());
            Restore::Replace(raw)
        }
        OpKind::Delete => Restore::Tombstone,
        OpKind::Extension | OpKind::TaskExtension => {
            let fields = op.body().as_object().cloned().unwrap_or_default();
            let mut extension = current;
            merge_extension(&mut extension, &fields);
            Restore::Replace(extension)
        }
        // `retry` asks `move_job` before it gets here.
        OpKind::Move => Restore::Nothing,
    })
}

async fn find(state: &State, op_id: &str) -> Result<OutboxRow, ErrorPayload> {
    state
        .store
        .outbox_op(op_id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            error_payload(
                ErrorKind::NotFound,
                format!("no outbox operation {op_id:?}; see `ms-todo outbox list`"),
            )
        })
}

async fn current(state: &State, op_id: &str) -> Result<ResponseData, ErrorPayload> {
    let op = find(state, op_id).await?;
    Ok(ResponseData::OutboxOp(outbox_op(&op, now())))
}

fn being_sent(op: &OutboxRow) -> ErrorPayload {
    error_payload(
        ErrorKind::InvalidInput,
        format!(
            "operation {} is being sent right now; check again in a moment",
            op.op_id
        ),
    )
}

/// Its state changed between reading it and acting on it: the worker
/// claimed it, or it was resolved.
fn moved(op: &OutboxRow) -> ErrorPayload {
    error_payload(
        ErrorKind::Conflict,
        format!(
            "operation {} changed while this ran (it may be being sent now), so nothing was \
             done; check `ms-todo outbox list` and try again",
            op.op_id
        ),
    )
}

fn already_done(op: &OutboxRow) -> ErrorPayload {
    error_payload(
        ErrorKind::InvalidInput,
        format!(
            "operation {} is done already; to reverse it, use `ms-todo undo {}`",
            op.op_id, op.command_id
        ),
    )
}

fn store_state(state: OutboxState) -> Result<OpState, ErrorPayload> {
    Ok(match state {
        OutboxState::Pending => OpState::Pending,
        OutboxState::Inflight => OpState::Inflight,
        OutboxState::Unknown => OpState::Unknown,
        OutboxState::Failed => OpState::Failed,
        OutboxState::Done => OpState::Done,
        OutboxState::Other => {
            return Err(error_payload(
                ErrorKind::Unsupported,
                "this daemon doesn't know that state; restart it with `ms-todo daemon stop`".into(),
            ));
        }
    })
}

/// An operation as clients see it.
pub(super) fn outbox_op(op: &OutboxRow, now: i64) -> OutboxOp {
    OutboxOp {
        op_id: op.op_id.clone(),
        command_id: op.command_id.clone(),
        action: op.action.clone(),
        task_id: op.entity_local_id.clone(),
        list_id: op.list_local_id.clone(),
        title: op.title.clone(),
        state: match op.state {
            OpState::Pending => OutboxState::Pending,
            OpState::Inflight => OutboxState::Inflight,
            OpState::Unknown => OutboxState::Unknown,
            OpState::Failed => OutboxState::Failed,
            OpState::Done => OutboxState::Done,
        },
        attempts: u32::try_from(op.attempts).unwrap_or(u32::MAX),
        created_at: op.created_at,
        next_attempt_at: (op.state == OpState::Pending && op.next_attempt_at > now)
            .then_some(op.next_attempt_at),
        sent_at: op.sent_at,
        unknown_since: op.unknown_since,
        depends_on: op.depends_on.clone(),
        undoes: op.undoes.clone(),
        last_error: op
            .last_error
            .clone()
            .map(|(kind, message)| OpError { kind, message }),
        note: op.note.clone(),
        flagged: op.is_flagged(now),
        changes: op.body().clone(),
    }
}
