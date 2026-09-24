//! Keeping the cache in step with Graph by full enumeration
//! (docs/blueprint/04-sync-cache.md): on start, every 5 minutes, and on
//! `ms-todo sync`. In rung 3b delta takes over, and this enumeration
//! becomes its reset path.

mod hydration;
mod pass;
mod scheduler;

pub(crate) use pass::PassContext;
pub(crate) use scheduler::{PROGRESS, PassOutcome, Syncer};
