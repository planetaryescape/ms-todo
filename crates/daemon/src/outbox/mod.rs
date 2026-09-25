//! The outbox worker (docs/blueprint/04-sync-cache.md#instant-local-writes).
//! Writes are queued by `task_writes` and `undo`; this sends them to Graph
//! in queue order, each task's operations one after another, and settles
//! each one:
//!
//! - **Success:** Graph's task goes into the cache and the operation is
//!   `done`. A create is `done` only once a `GET` of its list, sent after
//!   the 201, answers 200 (the ghost-write check, S4).
//! - **Temporary failure** (network, throttling, 5xx on a PATCH or DELETE,
//!   sign-in): back to `pending`, and nothing is sent until the backoff
//!   ends.
//! - **Permanent rejection:** `failed`, the local change rolled back, and
//!   `WriteRejected`.
//! - **Ambiguous** (a timeout or 5xx on a create or a recurring
//!   completion): `unknown`, never resent by itself (D-028). After each
//!   sync pass, `unknown` looks for its outcome.
//!
//! After sending anything it asks for a sync pass.

mod attachment_write;
mod child_write;
mod commands;
mod extension_write;
pub(crate) mod move_job;
mod rollback;
mod send;
mod unknown;

use std::sync::Arc;
use std::time::Duration;

use ms_todo_core::message_with_causes;
use tokio::sync::Notify;

use crate::handlers::State;

pub(crate) use commands::{discard, list, retry};
pub(crate) use send::fields_not_holding;

/// The longest the worker sleeps with nothing due, in case a wake-up was
/// missed.
const IDLE_WAKE: Duration = Duration::from_secs(60);
/// How long the worker waits after the store itself failed.
const STORE_RETRY: Duration = Duration::from_secs(5);

/// Wakes the worker when something is queued.
pub(crate) struct Outbox {
    wake: Notify,
}

impl Outbox {
    pub fn new() -> Self {
        Self {
            wake: Notify::new(),
        }
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

/// The operation ID of a command's `index`th operation: the command's own
/// for the first, `<command>.<index>` for the rest.
pub(crate) fn op_id_for(command_id: &str, index: usize) -> String {
    if index == 0 {
        command_id.to_owned()
    } else {
        format!("{command_id}.{index}")
    }
}

/// Settle what an earlier daemon left `inflight`, before anything is sent:
/// creates and recurring completions become `unknown`, the rest `pending`.
pub(crate) async fn recover(state: &State) {
    match state.store.recover_inflight().await {
        Ok(0) => {}
        Ok(unknown) => eprintln!(
            "ms-todo daemon: {unknown} write(s) were being sent when the daemon stopped; their \
             outcome is unknown until attributed (`ms-todo outbox list`)"
        ),
        Err(error) => eprintln!(
            "ms-todo daemon: cannot recover the outbox: {}",
            message_with_causes(&error)
        ),
    }
}

/// Send and settle operations until the daemon stops.
pub(crate) async fn run(state: Arc<State>) {
    let mut syncs = state.syncer.subscribe();
    let mut passes_seen = syncs.borrow().finished;
    loop {
        let sent = send::send_ready(&state).await;
        if sent {
            state.syncer.request();
        }
        let finished = syncs.borrow_and_update().finished;
        if finished > passes_seen {
            passes_seen = finished;
            if unknown::resolve(&state).await {
                // An adoption can free operations that waited for it.
                continue;
            }
        }
        let wait = match state.store.next_attempt_at(now()).await {
            Ok(Some(at)) => Duration::from_secs(u64::try_from(at - now()).unwrap_or(0)),
            Ok(None) => IDLE_WAKE,
            Err(error) => {
                eprintln!(
                    "ms-todo daemon: cannot read the outbox: {}",
                    message_with_causes(&error)
                );
                STORE_RETRY
            }
        };
        tokio::select! {
            () = state.outbox.wake.notified() => {}
            () = tokio::time::sleep(wait.min(IDLE_WAKE)) => {}
            // Only a finished pass matters, not each progress update. The
            // sender lives as long as the syncer.
            Ok(_) = syncs.wait_for(|status| status.finished > passes_seen) => {}
        }
    }
}

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// How long a temporary failure waits: 2 s, 4 s, 8 s, … for each attempt,
/// up to 5 minutes.
fn backoff(attempts: i64) -> i64 {
    let exponent = u32::try_from(attempts.clamp(1, 9)).unwrap_or(9);
    2_i64.pow(exponent).min(300)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_s_first_operation_has_its_id() {
        assert_eq!(op_id_for("op", 0), "op");
        assert_eq!(op_id_for("op", 2), "op.2");
    }

    #[test]
    fn backoff_doubles_up_to_five_minutes() {
        assert_eq!(backoff(1), 2);
        assert_eq!(backoff(3), 8);
        assert_eq!(backoff(40), 300);
    }
}
