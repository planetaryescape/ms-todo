//! What the daemon remembers between runs, as text by key (migration
//! `0006`): the day the last My Day rollover ran for, and what it left.

use crate::{Store, StoreError};

impl Store {
    pub async fn setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(
            sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
                .bind(key)
                .fetch_optional(self.reader())
                .await?,
        )
    }

    /// Set each `(key, value)` in one transaction.
    pub async fn set_settings(&self, values: &[(&str, &str)]) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        for (key, value) in values {
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES (?, ?) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
