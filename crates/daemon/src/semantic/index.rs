//! Keeping the vectors in step with the cache: a pass embeds every live
//! task whose text changed since its vector was made, in batches off the
//! async threads, and drops the vectors no search can use.

use std::sync::Arc;

use ms_todo_store::{Embedded, Store, StoreError};

use super::model::{Embedder, MODEL_ID};

/// Tasks per batch: one blocking call and one transaction each. A few
/// milliseconds of embedding (S19), so a pass never holds the writer long.
const BATCH: usize = 256;

/// Embed what changed. Returns how many tasks were embedded.
pub(crate) async fn catch_up(store: &Store, embedder: &Arc<Embedder>) -> Result<usize, String> {
    let describe = |error: StoreError| ms_todo_core::message_with_causes(&error);
    store.prune_embeddings(MODEL_ID).await.map_err(describe)?;
    let jobs = store.embedding_jobs(MODEL_ID).await.map_err(describe)?;
    for batch in jobs.chunks(BATCH) {
        let texts: Vec<String> = batch.iter().map(|job| job.text.clone()).collect();
        let embedder = Arc::clone(embedder);
        let vectors = tokio::task::spawn_blocking(move || embedder.embed(&texts))
            .await
            .map_err(|error| format!("embedding tasks failed: {error}"))?;
        let embedded: Vec<Embedded> = batch
            .iter()
            .zip(vectors)
            .map(|(job, vector)| Embedded {
                local_id: job.local_id.clone(),
                text_hash: job.text_hash.clone(),
                vector,
            })
            .collect();
        store
            .save_embeddings(MODEL_ID, &embedded)
            .await
            .map_err(describe)?;
    }
    Ok(jobs.len())
}
