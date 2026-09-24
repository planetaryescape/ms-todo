//! Keeping the cache in step with Graph by delta sync
//! (docs/blueprint/04-sync-cache.md#delta-sync), with a whole read of a
//! scope as the reset path: on start, every 20 seconds while clients are
//! active, every 5 minutes otherwise, and on `ms-todo sync`.

mod hydration;
mod pass;
mod scheduler;

pub(crate) use pass::PassContext;
pub(crate) use scheduler::{PROGRESS, PassOutcome, Syncer};
