//! `--idempotency-key K` (docs/blueprint/04-sync-cache.md#instant-local-writes):
//! the key is stored with a fingerprint of the operation and its payload.
//! A repeat with the same key and fingerprint gets the first result; the
//! same key with another request is invalid input (exit 2).
//!
//! Since rung 4 a mutation only queues its writes, in one local
//! transaction: whatever reaches Graph later is the outbox's business. So
//! a failed request changed nothing, and frees its key; a successful one
//! keeps it, with its result, while its operations are unresolved and for
//! 24 hours after (the store's side of this).

use std::future::Future;

use ms_todo_core::{ErrorKind, message_with_causes};
use ms_todo_protocol::{ErrorPayload, Request, Response, ResponseData};
use ms_todo_store::Claim;
use sha2::{Digest, Sha256};

use crate::handlers::{State, error_payload, store_error};

/// What a repeat under the same key must match: the SHA-256 of the
/// request's JSON (serde_json writes object keys in sorted order), without
/// the fields that differ between repeats. The request's own tag names the
/// operation, and every field of its payload is in, so a new field can't
/// be left out by accident.
pub(crate) fn fingerprint(request: &Request) -> String {
    let mut request = request.clone();
    if let Request::AddTask {
        op_id,
        idempotency_key,
        ..
    }
    | Request::ChangeTasks {
        op_id,
        idempotency_key,
        ..
    }
    | Request::ChangeLists {
        op_id,
        idempotency_key,
        ..
    }
    | Request::Undo {
        op_id,
        idempotency_key,
        ..
    } = &mut request
    {
        *op_id = None;
        *idempotency_key = None;
    }
    let json = serde_json::to_string(&request).unwrap_or_default();
    Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Run `operation` under `key`, or answer from the key's first run.
pub(crate) async fn run_once(
    state: &State,
    key: Option<&str>,
    fingerprint: &str,
    op_id: &str,
    operation: impl Future<Output = Result<ResponseData, ErrorPayload>>,
) -> Result<ResponseData, ErrorPayload> {
    let Some(key) = key else {
        return operation.await;
    };
    match state
        .store
        .claim_key(key, fingerprint, op_id)
        .await
        .map_err(store_error)?
    {
        Claim::Fresh => {}
        Claim::Replay(result) => return replay(&result),
        Claim::Running { op_id } => {
            return Err(ErrorPayload {
                op_id: Some(op_id.clone()),
                ..error_payload(
                    ErrorKind::OutcomeUnknown,
                    format!(
                        "a request with idempotency key {key:?} (op_id {op_id}) is still running, \
                         or the daemon stopped while it ran, so its outcome is unknown. Check \
                         `ms-todo outbox list` before trying again"
                    ),
                )
            });
        }
        Claim::Mismatch => {
            return Err(error_payload(
                ErrorKind::InvalidInput,
                format!(
                    "idempotency key {key:?} was already used for a different request; use a \
                     new key for a new request"
                ),
            ));
        }
    }
    let result = operation.await;
    let stored = match &result {
        Ok(_) => state.store.finish_key(key, &encode(&result)).await,
        Err(_) => state.store.release_key(key).await,
    };
    if let Err(error) = stored {
        eprintln!(
            "ms-todo daemon: cannot record idempotency key {key:?}: {}",
            message_with_causes(&error)
        );
    }
    result
}

/// After a restart, keys whose operation never finished get an
/// `outcome_unknown` result, so they expire 24 hours from now.
pub(crate) async fn settle_unfinished(state: &State) {
    let unfinished = match state.store.unfinished_keys().await {
        Ok(unfinished) => unfinished,
        Err(error) => {
            eprintln!(
                "ms-todo daemon: cannot read idempotency keys: {}",
                message_with_causes(&error)
            );
            return;
        }
    };
    for (key, op_id) in unfinished {
        let result: Result<ResponseData, ErrorPayload> = Err(ErrorPayload {
            op_id: Some(op_id.clone()),
            ..error_payload(
                ErrorKind::OutcomeUnknown,
                format!(
                    "the daemon stopped while the request with idempotency key {key:?} (op_id \
                     {op_id}) ran, so its outcome is unknown. Check `ms-todo outbox list` before \
                     trying again"
                ),
            )
        });
        if let Err(error) = state.store.finish_key(&key, &encode(&result)).await {
            eprintln!(
                "ms-todo daemon: cannot settle idempotency key {key:?}: {}",
                message_with_causes(&error)
            );
        }
    }
}

fn encode(result: &Result<ResponseData, ErrorPayload>) -> String {
    serde_json::to_string(&Response::from(result.clone())).unwrap_or_default()
}

fn replay(stored: &str) -> Result<ResponseData, ErrorPayload> {
    match serde_json::from_str(stored) {
        Ok(Response::Ok { data }) => Ok(data),
        Ok(Response::Error { error }) => Err(error),
        Ok(Response::Unknown) | Err(_) => Err(error_payload(
            ErrorKind::Internal,
            "the stored result for this idempotency key can't be read; use a new key after \
             checking `ms-todo tasks list`"
                .into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ms_todo_protocol::NewTask;

    #[test]
    fn a_fingerprint_ignores_the_op_id_and_key_but_not_the_payload() {
        let add = |title: &str, op_id: &str, key: &str| Request::AddTask {
            task: NewTask {
                title: title.into(),
                list: None,
                due: None,
                reminder: None,
                importance: None,
                body: None,
            },
            dry_run: false,
            op_id: Some(op_id.into()),
            idempotency_key: Some(key.into()),
        };
        let a = fingerprint(&add("Buy milk", "op-1", "k"));
        assert_eq!(a, fingerprint(&add("Buy milk", "op-2", "k2")));
        assert_ne!(a, fingerprint(&add("Buy oat milk", "op-1", "k")));
        assert_eq!(a.len(), 64);
    }
}
