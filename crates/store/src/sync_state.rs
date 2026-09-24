//! Per-scope sync state (docs/blueprint/04-sync-cache.md#freshness-guarantee-for-the-cli):
//! a generation that only goes up, one per finished pass even when nothing
//! changed, so `sync --wait` waits for a counter rather than a change or a
//! clock tick.

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
}

impl ScopeRow {
    /// A scope is `initial` until its first pass finishes.
    pub fn is_ready(&self) -> bool {
        self.generation > 0
    }

    pub fn generation(&self) -> u64 {
        u64::try_from(self.generation).unwrap_or(0)
    }
}

impl Store {
    pub async fn scope(&self, scope: &str) -> Result<Option<ScopeRow>, StoreError> {
        Ok(sqlx::query_as(
            "SELECT scope, generation, in_progress, last_success_at, last_changed_count, \
             last_error, last_error_kind, last_error_at FROM sync_state WHERE scope = ?",
        )
        .bind(scope)
        .fetch_optional(self.reader())
        .await?)
    }

    pub async fn scopes(&self) -> Result<Vec<ScopeRow>, StoreError> {
        Ok(sqlx::query_as(
            "SELECT scope, generation, in_progress, last_success_at, last_changed_count, \
             last_error, last_error_kind, last_error_at FROM sync_state ORDER BY scope = 'lists' DESC, scope",
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
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO sync_state (scope, generation, in_progress, last_success_at, last_changed_count) \
         VALUES (?1, 1, 0, ?2, ?3) \
         ON CONFLICT(scope) DO UPDATE SET generation = generation + 1, in_progress = 0, \
         last_success_at = ?2, last_changed_count = ?3, last_error = NULL, \
         last_error_kind = NULL, last_error_at = NULL",
    )
    .bind(scope)
    .bind(now())
    .bind(changed)
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
