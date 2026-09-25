//! ms-todo's local cache (docs/blueprint/02-data-model.md): SQLite in WAL
//! mode through sqlx, with the migrations in `crates/store/migrations/`.
//! Only the daemon depends on this crate (D-031,
//! `tests/workspace_boundaries.rs`); clients read it over the protocol.
//!
//! The store knows Graph's JSON shape only as far as it needs to fill the
//! columns reads filter on. What a sync pass or a write means is the
//! daemon's business; this crate applies it atomically.

mod graph_columns;
mod idempotency;
mod list_extension;
mod lists;
mod moves;
mod outbox;
mod pool;
mod search;
mod sync_state;
mod tasks;
mod views;

use serde_json::{Map, Value};

pub use idempotency::{Claim, IDEMPOTENCY_WINDOW_SECS};
pub use list_extension::{ListExtensionOp, merge_extension};
pub use lists::{FOLDER_FIELD, FOLDER_ORDER_FIELD, ListRow, ListsApplied, ListsPass, ORDER_FIELD};
pub use outbox::{
    LocalChange, NewOp, OpKind, OpState, OutboxRow, Restore, UNKNOWN_LOOKUP_SECS, apply_body,
};
pub use pool::Store;
pub use search::{SearchHit, StatusFilter, TaskSearch};
pub use sync_state::{Cursor, LISTS_SCOPE, ScopeRow, scope_list, tasks_scope};
pub use tasks::{Hydration, SeenTask, TaskRow, TasksPass};
pub use views::{TaskCounts, View};

/// A JSON object from Graph.
pub type Entity = Map<String, Value>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the local database failed")]
    Sqlx(#[from] sqlx::Error),
    #[error("the local database's migrations failed")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// The database has a migration this build doesn't know: a newer
    /// ms-todo upgraded it.
    #[error("this database was upgraded by a newer ms-todo; install the latest version")]
    NewerDatabase,
    #[error("a row in the local database isn't valid: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Invalid(String),
    /// A search query FTS5 can't read: the user's input, not the store.
    #[error("{0}")]
    InvalidQuery(String),
}

/// Unix seconds, the store's clock for tombstones and sync times. Ordering
/// never depends on it: generations and `local_rev` are counters.
pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(crate) fn new_local_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn parse_object(json: &str) -> Result<Entity, StoreError> {
    match serde_json::from_str(json) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) | Err(_) => Err(StoreError::Corrupt(format!(
            "expected a JSON object, got {}",
            json.chars().take(80).collect::<String>()
        ))),
    }
}

pub(crate) fn to_json(entity: &Entity) -> Result<String, StoreError> {
    serde_json::to_string(entity).map_err(|error| StoreError::Invalid(error.to_string()))
}

pub(crate) fn parse_optional(json: Option<String>) -> Result<Option<Value>, StoreError> {
    json.map(|json| {
        serde_json::from_str(&json).map_err(|error| StoreError::Corrupt(error.to_string()))
    })
    .transpose()
}
