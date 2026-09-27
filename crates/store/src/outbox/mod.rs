//! The outbox (docs/blueprint/02-data-model.md#outbox-semantics): each task
//! write is applied to `tasks` and queued here in one transaction, and the
//! daemon's worker sends the queue to Graph. What an operation's outcome
//! means is the daemon's business; this applies each outcome atomically:
//! the operation's new state together with what it does to the task row.
//!
//! A task with an operation `pending`, `inflight` or `unknown` is never
//! overwritten or tombstoned by a sync pass ([`NO_UNRESOLVED_OPS`]).

/// How the note of an operation that [`OutboxRow::was_skipped`] starts.
pub const SKIPPED_NOTE: &str = "skipped:";

/// How long an `unknown` operation is looked for before it's flagged for
/// the user, unless `[outbox] unknown_lookup_hours` says otherwise (04).
pub const UNKNOWN_LOOKUP_SECS: i64 = 24 * 60 * 60;

/// The states of an operation not resolved yet, as an SQL list. Its task
/// stays as ms-todo has it until they're all resolved.
macro_rules! unresolved {
    () => {
        "('pending', 'inflight', 'unknown')"
    };
}

/// A condition on `tasks`: no outbox operation of the row is unresolved.
pub(crate) const NO_UNRESOLVED_OPS: &str = concat!(
    "NOT EXISTS (SELECT 1 FROM outbox o \
     WHERE o.entity_local_id = tasks.local_id AND o.state IN ",
    unresolved!(),
    ")"
);

/// The unresolved states as an SQL list, for queries built at run time.
pub(crate) const UNRESOLVED_STATES: &str = unresolved!();

/// The sync state of a row from its operations, as a column of
/// `SELECT … FROM <table>`: `unknown` beats `pending`, which beats `failed`.
macro_rules! sync_state {
    ($table:literal) => {
        concat!(
            "(SELECT CASE \
             WHEN SUM(o.state = 'unknown') > 0 THEN 'unknown' \
             WHEN SUM(o.state IN ('pending', 'inflight')) > 0 THEN 'pending' \
             WHEN SUM(o.state = 'failed') > 0 THEN 'failed' ELSE 'synced' END \
             FROM outbox o WHERE o.entity_local_id = ",
            $table,
            ".local_id AND o.state != 'done') AS sync_state"
        )
    };
}

/// A column of `SELECT … FROM tasks`: the task's sync state.
pub(crate) const SYNC_STATE: &str = sync_state!("tasks");

/// A column of `SELECT … FROM lists`: the list's sync state, from its
/// extension writes (folders).
pub(crate) const LIST_SYNC_STATE: &str = sync_state!("lists");

// Declared after the macros above, which are in scope only below them.
mod enqueue;
mod operation;
mod outcomes;
mod prune;
mod reads;
mod rollback;
mod task_rows;

pub use enqueue::{LocalChange, NewOp, apply_body};
pub use operation::{OpKind, OpState, OutboxRow};
pub use rollback::Restore;

pub(crate) use enqueue::{Queued, insert_op};
pub(crate) use operation::op_in;
pub(crate) use outcomes::{finish, later_ops, merge_duplicate, record_in};
pub(crate) use rollback::fail_ops_in_list;
pub(crate) use task_rows::row_identity;
