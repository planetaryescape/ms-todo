//! Pruning finished outbox operations (D-065): when the daemon starts, and
//! once a day after, commands whose operations all finished longer ago
//! than `[outbox] retention_days` are deleted, within the store's rules
//! (`Store::prune_outbox`). That window is also how far back `undo`
//! reaches.

use std::sync::Arc;
use std::time::Duration;

use ms_todo_core::message_with_causes;

use super::now;
use crate::handlers::State;

const DAILY: Duration = Duration::from_secs(24 * 60 * 60);

/// Prune daily until the daemon stops; [`prune`] runs once at the start,
/// before the daemon listens.
pub(crate) async fn run(state: Arc<State>) {
    loop {
        tokio::time::sleep(DAILY).await;
        prune(&state).await;
    }
}

pub(crate) async fn prune(state: &State) {
    let config = &state.outbox.config;
    let at = now();
    match state.store.prune_outbox(at - config.retention_secs()).await {
        Ok(removed) => {
            if removed > 0 {
                eprintln!(
                    "ms-todo daemon: outbox: pruned {removed} operation(s) finished over {} \
                     day(s) ago",
                    config.retention_days
                );
            }
            state.outbox.pruned(at, removed);
        }
        Err(error) => eprintln!(
            "ms-todo daemon: outbox: cannot prune finished operations: {}",
            message_with_causes(&error)
        ),
    }
}
