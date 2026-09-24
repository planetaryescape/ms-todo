//! `--idempotency-key` (docs/blueprint/04-sync-cache.md#instant-local-writes):
//! each key is kept with a fingerprint of the operation and its payload,
//! while the operation runs and for 24 hours after it finishes. The daemon
//! decides what a fingerprint covers and what a result is; this stores
//! both as opaque strings.

use crate::{Store, StoreError, now};

/// How long a finished operation's key is kept.
pub const IDEMPOTENCY_WINDOW_SECS: i64 = 24 * 60 * 60;

/// What a key means for a new request carrying it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// Unused: it's now held for this request, which runs.
    Fresh,
    /// Used before for the same request, which finished with this result.
    Replay(String),
    /// Used before for the same request, which hasn't finished.
    Running { op_id: String },
    /// Used before for a different request.
    Mismatch,
}

impl Store {
    /// Look `key` up and, if it's unused, hold it for `op_id`.
    pub async fn claim_key(
        &self,
        key: &str,
        fingerprint: &str,
        op_id: &str,
    ) -> Result<Claim, StoreError> {
        let now = now();
        let mut tx = self.writer().begin().await?;
        sqlx::query(
            "DELETE FROM idempotency_keys WHERE finished_at IS NOT NULL AND finished_at <= ?",
        )
        .bind(now - IDEMPOTENCY_WINDOW_SECS)
        .execute(&mut *tx)
        .await?;
        let existing: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT fingerprint, op_id, result_json FROM idempotency_keys WHERE key = ?",
        )
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        let claim = match existing {
            Some((stored, ..)) if stored != fingerprint => Claim::Mismatch,
            Some((_, _, Some(result))) => Claim::Replay(result),
            Some((_, op_id, None)) => Claim::Running { op_id },
            None => {
                sqlx::query(
                    "INSERT INTO idempotency_keys (key, fingerprint, op_id, created_at) \
                     VALUES (?, ?, ?, ?)",
                )
                .bind(key)
                .bind(fingerprint)
                .bind(op_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                Claim::Fresh
            }
        };
        tx.commit().await?;
        Ok(claim)
    }

    /// The operation holding `key` finished with `result`, which a repeat
    /// gets for the next 24 hours.
    pub async fn finish_key(&self, key: &str, result: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE idempotency_keys SET result_json = ?, finished_at = ? WHERE key = ?")
            .bind(result)
            .bind(now())
            .bind(key)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    /// Forget `key`: its operation changed nothing, so a repeat may run.
    pub async fn release_key(&self, key: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM idempotency_keys WHERE key = ?")
            .bind(key)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    /// Keys whose operation never finished, as `(key, op_id)`: after a
    /// restart, their outcome is unknown.
    pub async fn unfinished_keys(&self) -> Result<Vec<(String, String)>, StoreError> {
        Ok(
            sqlx::query_as("SELECT key, op_id FROM idempotency_keys WHERE result_json IS NULL")
                .fetch_all(self.reader())
                .await?,
        )
    }
}
