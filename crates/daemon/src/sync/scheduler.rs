//! When passes run, and who's waiting for them. Passes are numbered; a
//! request for a sync gets the number of the next pass to start, which by
//! construction fetches after the request arrived, and waits until that
//! many have finished (vault: `Zero Change Can Be Success`: a pass that
//! finds nothing still counts).

use std::time::Duration;

use ms_todo_protocol::{ErrorPayload, SyncProgress};
use tokio::sync::{Notify, mpsc, watch};

use super::pass::{PassContext, run_pass};

/// How often a pass runs with nobody asking (04: every 5 minutes until
/// rung 3b's delta polling).
const INTERVAL: Duration = Duration::from_secs(5 * 60);

tokio::task_local! {
    /// Where the request being handled wants the progress of a pass it
    /// waits for. The connection loop sets it for every request and sends
    /// what arrives as events, so any wait on a sync (`sync --wait`, or a
    /// read or write waiting for the first sync) resets the client's stall
    /// deadline (issue 002).
    pub(crate) static PROGRESS: mpsc::UnboundedSender<SyncProgress>;
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SyncStatus {
    /// Passes started, the running one included.
    pub started: u64,
    pub finished: u64,
    /// The running pass's progress.
    pub progress: SyncProgress,
    /// How the last finished pass went.
    pub last: Option<PassOutcome>,
}

impl SyncStatus {
    pub fn running(&self) -> bool {
        self.started > self.finished
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PassOutcome {
    /// Scopes finished and checkpointed.
    pub scopes: u32,
    pub changed: u64,
    /// The first scope failure, if any.
    pub failure: Option<ErrorPayload>,
}

pub(crate) struct Syncer {
    status: watch::Sender<SyncStatus>,
    wake: Notify,
}

impl Syncer {
    /// The first pass counts as started from here, so a read that arrives
    /// before the loop gets going sees a sync in progress.
    pub fn new() -> Self {
        let (status, _) = watch::channel(SyncStatus {
            started: 1,
            ..SyncStatus::default()
        });
        Self {
            status,
            wake: Notify::new(),
        }
    }

    pub fn status(&self) -> SyncStatus {
        self.status.borrow().clone()
    }

    /// Ask for a pass that starts after now, and return its number.
    pub fn request(&self) -> u64 {
        let next = self.status.borrow().started + 1;
        self.wake.notify_one();
        next
    }

    /// Wait until pass `number` has finished, and say how the latest went.
    /// Progress meanwhile goes to [`PROGRESS`], when the caller set it.
    pub async fn wait_for(&self, number: u64) -> PassOutcome {
        let mut status = self.status.subscribe();
        loop {
            {
                let now = status.borrow_and_update();
                if now.finished >= number {
                    break;
                }
                let _ = PROGRESS.try_with(|progress| progress.send(now.progress.clone()));
            }
            // The sender lives as long as `self`, so this only fails if the
            // daemon is going away.
            if status.changed().await.is_err() {
                break;
            }
        }
        self.status().last.unwrap_or_default()
    }

    /// Wait for the running pass, or run one if none is running. For a
    /// caller that just needs the cache filled once.
    pub async fn settle(&self) -> PassOutcome {
        let status = self.status();
        let number = if status.running() {
            status.started
        } else {
            self.request()
        };
        self.wait_for(number).await
    }

    /// Run passes until the daemon stops: the first now, then every
    /// [`INTERVAL`] or on request.
    pub async fn run(&self, context: PassContext) {
        loop {
            let outcome = run_pass(&context, |progress| {
                self.status.send_modify(|status| status.progress = progress);
            })
            .await;
            if let Some(failure) = &outcome.failure {
                eprintln!(
                    "ms-todo daemon: sync failed ({}): {}",
                    failure.kind, failure.message
                );
            }
            self.status.send_modify(|status| {
                status.finished = status.started;
                status.last = Some(outcome);
                status.progress = SyncProgress::default();
            });
            tokio::select! {
                () = tokio::time::sleep(INTERVAL) => {}
                () = self.wake.notified() => {}
            }
            self.status.send_modify(|status| status.started += 1);
        }
    }
}
