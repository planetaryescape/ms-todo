//! Per-scope sync state (docs/blueprint/04-sync-cache.md#freshness-guarantee-for-the-cli):
//! a generation that only goes up, one per finished pass even when nothing
//! changed, so `sync --wait` waits for a counter rather than a change or a
//! clock tick. From rung 3b each scope also keeps its delta cursor.

use sqlx::{FromRow, SqliteConnection};

use crate::{Store, StoreError, now};

pub const LISTS_SCOPE: &str = "lists";

const TASKS_PREFIX: &str = "tasks:";

/// The scope of one list's tasks.
pub fn tasks_scope(list_graph_id: &str) -> String {
    format!("{TASKS_PREFIX}{list_graph_id}")
}

/// The list Graph ID of a scope from [`tasks_scope`].
pub fn scope_list(scope: &str) -> Option<&str> {
    scope.strip_prefix(TASKS_PREFIX)
}

#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ScopeRow {
    pub scope: String,
    pub generation: i64,
    pub in_progress: bool,
    pub last_success_at: Option<i64>,
    pub last_changed_count: i64,
    pub last_error: Option<String>,
    pub last_error_kind: Option<String>,
    pub last_error_at: Option<i64>,
    /// The `@odata.deltaLink` the last checkpoint saved. Without one the
    /// scope is in enumeration mode: its next pass reads it whole.
    pub delta_link: Option<String>,
    /// Unix seconds: when a pass last checkpointed by replaying a link.
    pub last_delta_at: Option<i64>,
}

/// How a pass read its scope, and the cursor it leaves behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// The `@odata.deltaLink` to save when the pass checkpoints.
    pub delta_link: String,
    /// True when the pass replayed a saved link and so saw only changes;
    /// false when it read the whole scope (the first pass, or a reset),
    /// which lets it tombstone what it didn't see.
    pub replayed: bool,
}

impl ScopeRow {
    /// A scope is `initial` until its first pass finishes.
    pub fn is_ready(&self) -> bool {
        self.generation > 0
    }

    pub fn generation(&self) -> u64 {
        u64::try_from(self.generation).unwrap_or(0)
    }

    /// Whether the next pass replays a delta link.
    pub fn is_delta(&self) -> bool {
        self.delta_link.is_some()
    }
}

impl Store {
    pub async fn scope(&self, scope: &str) -> Result<Option<ScopeRow>, StoreError> {
        Ok(sqlx::query_as(
            "SELECT scope, generation, in_progress, last_success_at, last_changed_count, \
             last_error, last_error_kind, last_error_at, delta_link, last_delta_at \
             FROM sync_state WHERE scope = ?",
        )
        .bind(scope)
        .fetch_optional(self.reader())
        .await?)
    }

    pub async fn scopes(&self) -> Result<Vec<ScopeRow>, StoreError> {
        Ok(sqlx::query_as(
            "SELECT scope, generation, in_progress, last_success_at, last_changed_count, \
             last_error, last_error_kind, last_error_at, delta_link, last_delta_at \
             FROM sync_state ORDER BY scope = 'lists' DESC, scope",
        )
        .fetch_all(self.reader())
        .await?)
    }

    /// A pass over `scope` has started. Returns [`Store::local_rev`], to
    /// give the pass's `apply_*` call, read here so that it's always read
    /// before the pass fetches anything.
    pub async fn begin_scope(&self, scope: &str) -> Result<i64, StoreError> {
        sqlx::query(
            "INSERT INTO sync_state (scope, in_progress) VALUES (?, 1) \
             ON CONFLICT(scope) DO UPDATE SET in_progress = 1",
        )
        .bind(scope)
        .execute(self.writer())
        .await?;
        self.local_rev().await
    }

    /// A pass over `scope` failed before it could apply anything. The
    /// cached rows stay as they were, and so does the generation.
    pub async fn fail_scope(
        &self,
        scope: &str,
        kind: &str,
        message: &str,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        record_failure(&mut tx, scope, kind, message).await?;
        tx.commit().await?;
        Ok(())
    }

    /// A pass over `scope` that changed nothing: checkpoint it with
    /// `cursor`.
    pub async fn advance_scope(&self, scope: &str, cursor: &Cursor) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        checkpoint(&mut tx, scope, 0, cursor).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Graph rejected `scope`'s delta link (410, or 400 "Badly formed
    /// token."): drop it, so the scope is in enumeration mode until a pass
    /// reads it whole and checkpoints a new one (04, reconciliation).
    pub async fn reset_scope(&self, scope: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE sync_state SET delta_link = NULL WHERE scope = ?")
            .bind(scope)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    /// After a restart nothing is running, whatever the table says.
    pub async fn clear_in_progress(&self) -> Result<(), StoreError> {
        sqlx::query("UPDATE sync_state SET in_progress = 0")
            .execute(self.writer())
            .await?;
        Ok(())
    }
}

pub(crate) async fn checkpoint(
    tx: &mut SqliteConnection,
    scope: &str,
    changed: i64,
    cursor: &Cursor,
) -> Result<(), StoreError> {
    let now = now();
    sqlx::query(
        "INSERT INTO sync_state (scope, generation, in_progress, last_success_at, last_changed_count, \
         delta_link, last_delta_at) \
         VALUES (?1, 1, 0, ?2, ?3, ?4, ?5) \
         ON CONFLICT(scope) DO UPDATE SET generation = generation + 1, in_progress = 0, \
         last_success_at = ?2, last_changed_count = ?3, last_error = NULL, \
         last_error_kind = NULL, last_error_at = NULL, delta_link = ?4, \
         last_delta_at = COALESCE(?5, last_delta_at)",
    )
    .bind(scope)
    .bind(now)
    .bind(changed)
    .bind(&cursor.delta_link)
    .bind(cursor.replayed.then_some(now))
    .execute(tx)
    .await?;
    Ok(())
}

pub(crate) async fn record_failure(
    tx: &mut SqliteConnection,
    scope: &str,
    kind: &str,
    message: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO sync_state (scope, in_progress, last_error, last_error_kind, last_error_at) \
         VALUES (?1, 0, ?2, ?3, ?4) \
         ON CONFLICT(scope) DO UPDATE SET in_progress = 0, last_error = ?2, \
         last_error_kind = ?3, last_error_at = ?4",
    )
    .bind(scope)
    .bind(message)
    .bind(kind)
    .bind(now())
    .execute(tx)
    .await?;
    Ok(())
}
