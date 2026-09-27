//! A search by meaning: the query's vector against every candidate's, by
//! cosine similarity (a dot product, since both are unit length). Brute
//! force: a few thousand 256-float vectors take well under a millisecond
//! (S19), so there's no vector index.

use ms_todo_core::ErrorKind;
use ms_todo_protocol::ErrorPayload;
use ms_todo_store::{Candidate, Candidates, TaskRow, TaskScope};

use super::model::MODEL_ID;
use crate::handlers::{State, error_payload, store_error};

/// Tasks less similar than this aren't results, so a search returns a
/// shorter list rather than every task in order. From S19's labelled
/// set: most unrelated tasks scored below it, and it loses one expected
/// task of the 31 the model ranked in its top five.
const MIN_SCORE: f32 = 0.15;

/// A task that matched, with its similarity to the query.
pub(crate) struct Hit {
    pub candidate: Candidate,
    pub score: f32,
}

/// Tasks in `scope` closest in meaning to `query`, best first, and how
/// many in scope aren't embedded for their current text yet. `wait`:
/// wait for the model while it's loading (a CLI search), rather than
/// answer that it isn't ready (the TUI's filter, typed a key at a time).
pub(crate) async fn search(
    state: &State,
    query: &str,
    scope: &TaskScope<'_>,
    limit: Option<u32>,
    wait: bool,
    context: Option<&crate::contexts::Resolved>,
) -> Result<(Vec<Hit>, u32), ErrorPayload> {
    let query = query.trim();
    if query.is_empty() {
        return Err(error_payload(
            ErrorKind::InvalidInput,
            "give some words to search for".into(),
        ));
    }
    let embedder = if wait {
        state.semantic.embedder(&state.store, &state.events).await?
    } else {
        state.semantic.ready_embedder()?
    };
    let wanted = embedder.embed_one(query);
    let candidates = state
        .store
        .semantic_candidates(MODEL_ID, scope)
        .await
        .map_err(store_error)?;
    let pending = pending_within(&candidates, context);
    let mut hits: Vec<Hit> = candidates
        .tasks
        .into_iter()
        .filter_map(|candidate| {
            let score = dot(&wanted, &candidate.vector);
            (score >= MIN_SCORE).then_some(Hit { candidate, score })
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    if let Some(limit) = limit {
        hits.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    Ok((hits, pending))
}

/// How many tasks in scope aren't indexed yet, counting only the
/// context's: the others can't be results.
fn pending_within(candidates: &Candidates, context: Option<&crate::contexts::Resolved>) -> u32 {
    match context {
        None => candidates.pending,
        Some(_) => {
            let within = candidates
                .pending_lists
                .iter()
                .filter(|list| crate::contexts::within(context, list))
                .count();
            u32::try_from(within).unwrap_or(u32::MAX)
        }
    }
}

/// The tasks of `search` for a TUI seed: never waits for the model, and
/// the pending count isn't shown.
pub(crate) async fn rows(
    state: &State,
    query: &str,
    scope: &TaskScope<'_>,
) -> Result<Vec<TaskRow>, ErrorPayload> {
    let (hits, _) = search(state, query, scope, None, false, None).await?;
    Ok(hits.into_iter().map(|hit| hit.candidate.task).collect())
}

/// A vector of another length (a damaged row) scores nothing.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_vectors_score_their_cosine_and_a_mismatch_nothing() {
        assert!((dot(&[0.6, 0.8], &[0.6, 0.8]) - 1.0).abs() < 1e-6);
        assert!(dot(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(dot(&[1.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn a_context_counts_only_its_own_unindexed_tasks() {
        let candidates = Candidates {
            pending: 3,
            pending_lists: vec!["L-work".into(), "L-home".into(), "L-home".into()],
            ..Candidates::default()
        };
        assert_eq!(pending_within(&candidates, None), 3);
        let work = crate::contexts::Resolved {
            name: "work".into(),
            lists: Vec::new(),
            ids: ["L-work".to_owned()].into(),
            default_list: None,
            problems: Vec::new(),
        };
        assert_eq!(pending_within(&candidates, Some(&work)), 1);
    }
}
