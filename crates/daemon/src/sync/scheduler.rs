//! When passes run, and who's waiting for them. Passes are numbered; a
//! request for a sync gets the number of the next pass to start, which by
//! construction fetches after the request arrived, and waits until that
//! many have finished (vault: `Zero Change Can Be Success`: a pass that
//! finds nothing still counts).
//!
//! Cadence (docs/blueprint/04-sync-cache.md#delta-sync): a pass every 20
//! seconds while a client is connected or for 10 minutes after any client
//! request, otherwise every 5 minutes, and at once on `ms-todo sync`. A
//! delta pass that finds nothing costs one request per scope.

use std::future::Future;
use std::time::Duration;

use ms_todo_protocol::{ErrorPayload, SyncProgress};
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;

use super::pass::{PassContext, run_pass};

/// Between passes while someone is using ms-todo (04: 15–30 seconds).
const ACTIVE_INTERVAL: Duration = Duration::from_secs(20);
/// Between passes otherwise.
const IDLE_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How long after a client's last request it still counts as active.
const ACTIVE_FOR: Duration = Duration::from_secs(10 * 60);

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

/// What decides the cadence.
#[derive(Clone, Copy, Debug, Default)]
struct Activity {
    connected: usize,
    last_request: Option<Instant>,
}

impl Activity {
    fn interval(self, now: Instant) -> Duration {
        let recent = self
            .last_request
            .is_some_and(|at| now.saturating_duration_since(at) < ACTIVE_FOR);
        if self.connected > 0 || recent {
            ACTIVE_INTERVAL
        } else {
            IDLE_INTERVAL
        }
    }
}

pub(crate) struct Syncer {
    status: watch::Sender<SyncStatus>,
    wake: Notify,
    activity: watch::Sender<Activity>,
}

/// A connected client. Dropping it disconnects.
pub(crate) struct Client<'a> {
    syncer: &'a Syncer,
}

impl Drop for Client<'_> {
    fn drop(&mut self) {
        self.syncer
            .activity
            .send_modify(|activity| activity.connected = activity.connected.saturating_sub(1));
    }
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
            activity: watch::Sender::new(Activity::default()),
        }
    }

    /// A client connected; it counts as active until the guard drops.
    pub fn client_connected(&self) -> Client<'_> {
        self.activity
            .send_modify(|activity| activity.connected += 1);
        Client { syncer: self }
    }

    /// A client asked for something: poll at the active cadence for the
    /// next [`ACTIVE_FOR`].
    pub fn client_request(&self) {
        self.activity
            .send_modify(|activity| activity.last_request = Some(Instant::now()));
    }

    pub fn status(&self) -> SyncStatus {
        self.status.borrow().clone()
    }

    /// Watch the status, e.g. for each pass finishing.
    pub fn subscribe(&self) -> watch::Receiver<SyncStatus> {
        self.status.subscribe()
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

    /// Run passes until the daemon stops: the first now, then at the
    /// cadence, or on request.
    pub async fn run(&self, context: PassContext) {
        let context = &context;
        self.run_with(move || {
            run_pass(context, move |progress| {
                self.status.send_modify(|status| status.progress = progress);
            })
        })
        .await;
    }

    async fn run_with<F, Fut>(&self, mut pass: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = PassOutcome>,
    {
        let mut activity = self.activity.subscribe();
        loop {
            let outcome = pass().await;
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
            let finished = Instant::now();
            loop {
                // Recomputed whenever activity changes, so the first request
                // after an idle spell brings the next pass forward.
                let interval = activity.borrow_and_update().interval(Instant::now());
                tokio::select! {
                    () = tokio::time::sleep_until(finished + interval) => break,
                    () = self.wake.notified() => break,
                    // The sender lives as long as `self`.
                    Ok(()) = activity.changed() => {}
                }
            }
            self.status.send_modify(|status| status.started += 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A syncer whose passes only count, running on paused time.
    fn counting() -> (Arc<Syncer>, Arc<AtomicU32>) {
        let syncer = Arc::new(Syncer::new());
        let passes = Arc::new(AtomicU32::new(0));
        let (running, counter) = (Arc::clone(&syncer), Arc::clone(&passes));
        tokio::spawn(async move {
            running
                .run_with(|| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    async { PassOutcome::default() }
                })
                .await;
        });
        (syncer, passes)
    }

    /// Let paused time run on to `at` after the start.
    async fn at(start: Instant, at: Duration) {
        tokio::time::sleep_until(start + at).await;
    }

    const fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[tokio::test(start_paused = true)]
    async fn idle_polls_every_5_minutes_and_a_request_brings_20_second_polling_for_10_minutes() {
        let start = Instant::now();
        let (syncer, passes) = counting();
        let count = || passes.load(Ordering::SeqCst);

        at(start, secs(1)).await;
        assert_eq!(count(), 1, "the first pass runs at once");
        at(start, secs(299)).await;
        assert_eq!(count(), 1, "idle: nothing for 5 minutes");
        at(start, secs(301)).await;
        assert_eq!(count(), 2);

        // A request at 5:01. The last pass finished at 5:00, so the next is
        // due at 5:20, then every 20 seconds.
        syncer.client_request();
        at(start, secs(319)).await;
        assert_eq!(count(), 2);
        at(start, secs(321)).await;
        assert_eq!(count(), 3);
        // Requests don't cause passes, they only set the cadence.
        for second in (325..360).step_by(5) {
            at(start, secs(second)).await;
            syncer.client_request();
        }
        at(start, secs(361)).await;
        assert_eq!(count(), 5, "passes at 5:40 and 6:00");

        // The last request was at 5:55, so polling stays at 20 seconds
        // until 15:55: the pass at 15:40 is the last to see it active, the
        // one at 16:00 finds it idle, and the next is due at 21:00.
        at(start, secs(961)).await;
        let settled = count();
        assert_eq!(settled, 5 + 30, "every 20 seconds from 6:20 to 16:00");
        at(start, secs(1259)).await;
        assert_eq!(count(), settled);
        at(start, secs(1261)).await;
        assert_eq!(count(), settled + 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_connected_client_keeps_20_second_polling_and_sync_runs_a_pass_at_once() {
        let start = Instant::now();
        let (syncer, passes) = counting();
        let count = || passes.load(Ordering::SeqCst);
        at(start, secs(1)).await;

        let client = syncer.client_connected();
        at(start, secs(21)).await;
        assert_eq!(
            count(),
            2,
            "connected: the next pass is 20 seconds after the last"
        );
        at(start, secs(20 * 60 + 1)).await;
        assert_eq!(count(), 61, "still every 20 seconds after 20 minutes");

        drop(client);
        at(start, secs(20 * 60 + 21)).await;
        assert_eq!(count(), 61, "no client and no request: idle");

        let wanted = syncer.request();
        let outcome = tokio::time::timeout(secs(1), syncer.wait_for(wanted)).await;
        assert!(outcome.is_ok(), "`sync` doesn't wait for the cadence");
        assert_eq!(count(), 62);
    }
}
