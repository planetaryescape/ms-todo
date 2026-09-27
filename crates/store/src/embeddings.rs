//! Task vectors for semantic search (D-062, migration 0008): what still
//! needs embedding, saving the vectors, and the vectors a search ranks.
//! The model itself is the daemon's; the store only keeps its output,
//! tagged with the model's name so a new model re-embeds everything.
//!
//! The text embedded is the text the FTS index holds (D-041): the title,
//! then the plain-text body on the next line when there is one.

use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, FromRow, Row};

use crate::search::StatusFilter;
use crate::tasks::{TaskRecord, TaskRow, task_record_columns};
use crate::views::View;
use crate::{Store, StoreError};

/// A live task whose vector is missing, out of date or another model's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddingJob {
    pub local_id: String,
    /// What to embed.
    pub text: String,
    /// [`text_hash`] of `text`, saved with the vector.
    pub text_hash: Vec<u8>,
}

/// A vector made from a job's text.
pub struct Embedded {
    pub local_id: String,
    pub text_hash: Vec<u8>,
    pub vector: Vec<f32>,
}

/// Which tasks a semantic search looks through.
#[derive(Clone, Copy, Debug, Default)]
pub struct TaskScope<'a> {
    /// Only this list's tasks.
    pub list_local_id: Option<&'a str>,
    pub status: StatusFilter,
    /// Only tasks in this smart view.
    pub view: Option<View>,
}

/// A task in scope with a vector.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub task: TaskRow,
    /// The display name of the task's list.
    pub list_name: String,
    pub vector: Vec<f32>,
}

/// The tasks in scope that have a vector, and how many don't have one for
/// their current text yet. A task whose text changed keeps its old vector
/// until it's embedded again: it's a candidate and counted as pending.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidates {
    pub tasks: Vec<Candidate>,
    pub pending: u32,
}

/// The text of a task that is embedded: its title, and its plain-text
/// body on the next line when it has one.
pub(crate) fn embedding_text(title: &str, body_text: Option<&str>) -> String {
    match body_text.map(str::trim).filter(|body| !body.is_empty()) {
        Some(body) => format!("{title}\n{body}"),
        None => title.to_owned(),
    }
}

/// SHA-256 of `text`: whether a saved vector is still the text's.
pub fn text_hash(text: &str) -> Vec<u8> {
    Sha256::digest(text.as_bytes()).to_vec()
}

/// A live task's text, and the hash its vector was made from, if any.
#[derive(FromRow)]
struct JobRow {
    local_id: String,
    title: String,
    body_text: Option<String>,
    text_hash: Option<Vec<u8>>,
}

impl Store {
    /// Live tasks to embed for `model`: those with no vector, a vector of
    /// another text, or another model's.
    pub async fn embedding_jobs(&self, model: &str) -> Result<Vec<EmbeddingJob>, StoreError> {
        let rows: Vec<JobRow> = sqlx::query_as(
            "SELECT tasks.local_id, tasks.title, tasks.body_text, e.text_hash FROM tasks \
             LEFT JOIN task_embeddings e ON e.task_local_id = tasks.local_id AND e.model = ?1 \
             WHERE tasks.deleted_at IS NULL",
        )
        .bind(model)
        .fetch_all(self.reader())
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(
                |JobRow {
                     local_id,
                     title,
                     body_text: body,
                     text_hash: saved_hash,
                 }| {
                    let text = embedding_text(&title, body.as_deref());
                    let hash = text_hash(&text);
                    (saved_hash.as_ref() != Some(&hash)).then_some(EmbeddingJob {
                        local_id,
                        text,
                        text_hash: hash,
                    })
                },
            )
            .collect())
    }

    /// Save `vectors`, made by `model`, in one transaction. A task deleted
    /// since its job was read is skipped.
    pub async fn save_embeddings(
        &self,
        model: &str,
        vectors: &[Embedded],
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        for embedded in vectors {
            sqlx::query(
                "INSERT INTO task_embeddings (task_local_id, model, text_hash, vector) \
                 SELECT local_id, ?2, ?3, ?4 FROM tasks WHERE local_id = ?1 \
                 ON CONFLICT (task_local_id) DO UPDATE SET \
                 model = excluded.model, text_hash = excluded.text_hash, vector = excluded.vector",
            )
            .bind(&embedded.local_id)
            .bind(model)
            .bind(&embedded.text_hash)
            .bind(vector_bytes(&embedded.vector))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Remove vectors no search can use: those of tombstoned tasks and of
    /// another model. Returns how many went.
    pub async fn prune_embeddings(&self, model: &str) -> Result<u64, StoreError> {
        let done = sqlx::query(
            "DELETE FROM task_embeddings WHERE model <> ?1 OR task_local_id IN \
             (SELECT local_id FROM tasks WHERE deleted_at IS NOT NULL)",
        )
        .bind(model)
        .execute(self.writer())
        .await?;
        Ok(done.rows_affected())
    }

    /// How many live tasks have a vector from `model` for their current
    /// text, and how many don't, for `doctor`.
    pub async fn embedding_counts(&self, model: &str) -> Result<(u32, u32), StoreError> {
        let live: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE deleted_at IS NULL")
            .fetch_one(self.reader())
            .await?;
        let pending = self.embedding_jobs(model).await?.len();
        let live = u32::try_from(live).unwrap_or(u32::MAX);
        let pending = u32::try_from(pending).unwrap_or(u32::MAX);
        Ok((live.saturating_sub(pending), pending))
    }

    /// The live tasks in `scope`, in live lists, with their vectors from
    /// `model`.
    pub async fn semantic_candidates(
        &self,
        model: &str,
        scope: &TaskScope<'_>,
    ) -> Result<Candidates, StoreError> {
        let status = scope.status.and_condition();
        let view = scope
            .view
            .map(|view| format!("AND {}", view.condition()))
            .unwrap_or_default();
        let rows: Vec<SqliteRow> = sqlx::query(AssertSqlSafe(format!(
            "SELECT {}, tasks.body_text, lists.display_name AS list_name, \
             e.text_hash, e.vector \
             FROM tasks \
             JOIN lists ON lists.local_id = tasks.list_local_id \
             LEFT JOIN task_embeddings e ON e.task_local_id = tasks.local_id AND e.model = ?1 \
             WHERE tasks.deleted_at IS NULL AND lists.deleted_at IS NULL \
             AND (?2 IS NULL OR tasks.list_local_id = ?2) {status} {view}",
            task_record_columns()
        )))
        .bind(model)
        .bind(scope.list_local_id)
        .fetch_all(self.reader())
        .await?;
        let mut candidates = Candidates::default();
        for row in &rows {
            let task = TaskRow::try_from(TaskRecord::from_row(row)?)?;
            let saved_hash: Option<Vec<u8>> = row.try_get("text_hash")?;
            let body: Option<String> = row.try_get("body_text")?;
            let current = saved_hash.is_some_and(|hash| {
                hash == text_hash(&embedding_text(&task.title, body.as_deref()))
            });
            if !current {
                candidates.pending += 1;
            }
            let Some(bytes) = row.try_get::<Option<Vec<u8>>, _>("vector")? else {
                continue;
            };
            candidates.tasks.push(Candidate {
                list_name: row.try_get("list_name")?,
                vector: vector_from_bytes(&bytes)?,
                task,
            });
        }
        Ok(candidates)
    }
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn vector_from_bytes(bytes: &[u8]) -> Result<Vec<f32>, StoreError> {
    let (chunks, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() {
        return Err(StoreError::Corrupt(format!(
            "a task vector is {} bytes, not a whole number of f32s",
            bytes.len()
        )));
    }
    Ok(chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_the_title_then_the_body() {
        assert_eq!(embedding_text("Buy milk", None), "Buy milk");
        assert_eq!(embedding_text("Buy milk", Some("  \n")), "Buy milk");
        assert_eq!(
            embedding_text("Buy milk", Some("semi-skimmed\n")),
            "Buy milk\nsemi-skimmed"
        );
    }

    #[test]
    fn vectors_round_trip_through_bytes() {
        let vector = vec![0.25_f32, -1.0, 3.5e-8];
        assert_eq!(
            vector_from_bytes(&vector_bytes(&vector)).expect("whole"),
            vector
        );
        assert!(vector_from_bytes(&[0, 1, 2]).is_err());
    }
}
