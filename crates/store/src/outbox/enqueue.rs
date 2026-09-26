//! Queueing a command's operations, each with the local change it makes
//! at once, in one transaction.

use serde_json::Value;
use sqlx::SqliteConnection;

use super::operation::{OpKind, OpState};
use super::outcomes::finish;
use super::task_rows::{
    fetch_task, replace_row, row_identity, task_extension, tombstone_row, write_task_extension,
};
use crate::children::apply_child;
use crate::list_extension::merge_extension;
use crate::moves::set_list;
use crate::pool::next_local_rev;
use crate::tasks::{WriteTask, write_task};
use crate::{Entity, Store, StoreError, TaskRow, now, parse_object, to_json};

/// The entity kind an operation of `kind` is on.
fn entity_kind(kind: OpKind) -> &'static str {
    if kind.is_list() {
        "list"
    } else if kind == OpKind::Remote {
        "remote"
    } else {
        "task"
    }
}

/// An operation to queue, with the local change it makes at once.
#[derive(Clone, Debug)]
pub struct NewOp {
    pub op_id: String,
    pub entity_local_id: String,
    pub list_local_id: String,
    pub op: OpKind,
    pub action: String,
    pub payload: Value,
    pub change: LocalChange,
}

/// What a queued operation does to the task row now, before Graph has it.
#[derive(Clone, Debug)]
pub enum LocalChange {
    /// A new task only ms-todo knows: no Graph ID yet.
    Insert {
        raw: Entity,
        extension: Option<Value>,
    },
    /// The task's JSON becomes `raw`, and it's live.
    Replace(Entity),
    /// The payload's `body` goes over the task's JSON as it is when the
    /// operation is queued, which is also its rollback: a snapshot read
    /// earlier could undo an answer from Graph recorded meanwhile.
    Update,
    /// Tombstone the task; its JSON when queued is the rollback.
    Tombstone,
    /// The task shows in the list `to` at once; its JSON when queued is
    /// the rollback. A move puts it there on Graph.
    Move { to: String },
    /// The payload's `body` goes over the task's extension (a null
    /// removing a field); the extension when queued is the rollback.
    Extension,
    /// The payload's child change goes into the task's JSON
    /// ([`apply_child`]); its JSON when queued is the rollback.
    Child,
}

/// Overlay the fields of a PATCH or POST `body` on a task's JSON, as Graph
/// would apply them. Our extension isn't a field.
pub fn apply_body(raw: &mut Entity, body: &Value) {
    if let Value::Object(fields) = body {
        for (key, value) in fields {
            if key != "extensions" {
                raw.insert(key.clone(), value.clone());
            }
        }
    }
}

impl Store {
    /// Queue `ops`, one command's, and make each one's local change, all in
    /// one transaction. Each waits for the latest unresolved operation on
    /// its task. Returns each task as it is now, tombstoned or not.
    pub async fn enqueue(
        &self,
        command_id: &str,
        undoes: Option<&str>,
        ops: Vec<NewOp>,
    ) -> Result<Vec<TaskRow>, StoreError> {
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let now = now();
        let mut seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM outbox")
            .fetch_one(&mut *tx)
            .await?;
        let mut rows = Vec::with_capacity(ops.len());
        for op in ops {
            seq += 1;
            // What a rejection restores and undo inverts: the task before this.
            let mut rollback = None;
            match &op.change {
                LocalChange::Insert { raw, extension } => {
                    let extension_json = extension.as_ref().map(Value::to_string);
                    write_task(
                        &mut tx,
                        &WriteTask {
                            local_id: &op.entity_local_id,
                            graph_id: None,
                            list_local_id: &op.list_local_id,
                            raw,
                            raw_json: &to_json(raw)?,
                            extension_json: extension_json.as_deref(),
                            hydrated_etag: None,
                            local_rev: rev,
                        },
                    )
                    .await?;
                }
                LocalChange::Replace(raw) => {
                    replace_row(&mut tx, &op.entity_local_id, raw, rev).await?;
                }
                LocalChange::Update => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    let mut raw = current.clone();
                    apply_body(&mut raw, &op.payload["body"]);
                    replace_row(&mut tx, &op.entity_local_id, &raw, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Tombstone => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    tombstone_row(&mut tx, &op.entity_local_id, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Move { to } => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    set_list(&mut tx, &op.entity_local_id, to, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Child => {
                    let current =
                        parse_object(&row_identity(&mut tx, &op.entity_local_id).await?.raw_json)?;
                    let mut raw = current.clone();
                    apply_child(&mut raw, &op.payload);
                    replace_row(&mut tx, &op.entity_local_id, &raw, rev).await?;
                    rollback = Some(current);
                }
                LocalChange::Extension => {
                    let current = task_extension(&mut tx, &op.entity_local_id).await?;
                    let mut merged = current.clone();
                    if let Some(fields) = op.payload["body"].as_object() {
                        merge_extension(&mut merged, fields);
                    }
                    write_task_extension(&mut tx, &op.entity_local_id, &merged, rev).await?;
                    rollback = Some(current);
                }
            }
            insert_op(
                &mut tx,
                &Queued {
                    op_id: &op.op_id,
                    seq,
                    command_id,
                    undoes,
                    created_at: now,
                    entity_local_id: &op.entity_local_id,
                    list_local_id: &op.list_local_id,
                    op: op.op,
                    action: &op.action,
                    payload: &op.payload,
                    rollback: rollback.as_ref(),
                },
            )
            .await?;
            rows.push(
                fetch_task(&mut tx, &op.entity_local_id)
                    .await?
                    .ok_or_else(|| {
                        StoreError::Corrupt(format!("task {} isn't cached", op.entity_local_id))
                    })?,
            );
        }
        tx.commit().await?;
        Ok(rows)
    }

    /// Record a category or extension write Graph has made (D-058): one
    /// operation, `done` already, on the thing `entity`, with `payload`
    /// (what `undo` needs to reverse it) and `rollback`, the thing before.
    pub async fn record_remote(
        &self,
        command_id: &str,
        undoes: Option<&str>,
        entity: &str,
        action: &str,
        payload: &Value,
        rollback: &Entity,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        let seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) + 1 FROM outbox")
            .fetch_one(&mut *tx)
            .await?;
        insert_op(
            &mut tx,
            &Queued {
                op_id: command_id,
                seq,
                command_id,
                undoes,
                created_at: now(),
                entity_local_id: entity,
                list_local_id: "",
                op: OpKind::Remote,
                action,
                payload,
                rollback: Some(rollback),
            },
        )
        .await?;
        finish(&mut tx, command_id, OpState::Done, None).await?;
        tx.commit().await?;
        Ok(())
    }
}

/// An operation as it's queued.
pub(crate) struct Queued<'a> {
    pub op_id: &'a str,
    pub seq: i64,
    pub command_id: &'a str,
    pub undoes: Option<&'a str>,
    pub created_at: i64,
    pub entity_local_id: &'a str,
    pub list_local_id: &'a str,
    pub op: OpKind,
    pub action: &'a str,
    pub payload: &'a Value,
    pub rollback: Option<&'a Entity>,
}

/// Add `op` to the queue, `pending`, waiting for the latest unresolved
/// operation on the same entity.
pub(crate) async fn insert_op(
    tx: &mut SqliteConnection,
    op: &Queued<'_>,
) -> Result<(), StoreError> {
    let mut depends_on: Option<String> = sqlx::query_scalar(concat!(
        "SELECT op_id FROM outbox WHERE entity_local_id = ? AND state IN ",
        unresolved!(),
        " ORDER BY seq DESC LIMIT 1"
    ))
    .bind(op.entity_local_id)
    .fetch_optional(&mut *tx)
    .await?;
    if depends_on.is_none() && !op.op.is_list() {
        // A task's first write in a list not created yet waits for the
        // list: it needs the list's Graph ID.
        depends_on = sqlx::query_scalar(concat!(
            "SELECT op_id FROM outbox WHERE entity_local_id = ? AND op = 'list_create' \
             AND state IN ",
            unresolved!(),
            " ORDER BY seq DESC LIMIT 1"
        ))
        .bind(op.list_local_id)
        .fetch_optional(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO outbox (op_id, seq, command_id, created_at, entity_kind, entity_local_id, \
         list_local_id, op, action, payload_json, depends_on_op_id, undoes_command_id, \
         state, rollback_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
    )
    .bind(op.op_id)
    .bind(op.seq)
    .bind(op.command_id)
    .bind(op.created_at)
    .bind(entity_kind(op.op))
    .bind(op.entity_local_id)
    .bind(op.list_local_id)
    .bind(op.op.as_str())
    .bind(op.action)
    .bind(op.payload.to_string())
    .bind(depends_on)
    .bind(op.undoes)
    .bind(op.rollback.map(to_json).transpose()?)
    .execute(&mut *tx)
    .await?;
    Ok(())
}
