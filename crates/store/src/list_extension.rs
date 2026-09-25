//! Folder writes (docs/blueprint/05-custom-features.md#folders-list-groups):
//! a change to some fields of a list's extension, applied to `lists` and
//! queued in the outbox in one transaction, like a task write. The worker
//! sends it as GET, merge and a write of the whole document, since Graph
//! replaces an extension on PATCH (S2).
//!
//! The operation's `body` is the fields changed, a null removing one; its
//! rollback is the whole extension before, `{}` for none.

use serde_json::{Map, Value};
use sqlx::SqliteConnection;

use crate::lists::list_in;
use crate::outbox::{OpKind, OpState, Queued, Restore, finish, insert_op, later_ops, op_in};
use crate::pool::next_local_rev;
use crate::{Entity, ListRow, Store, StoreError, now};

/// A change to one list's extension, to queue.
#[derive(Clone, Debug)]
pub struct ListExtensionOp {
    pub op_id: String,
    pub list_local_id: String,
    /// What the user asked for, such as `move_list`.
    pub action: String,
    /// The fields to set; a null removes the field.
    pub fields: Map<String, Value>,
}

/// Put `fields` over `extension`, a null removing a field: every other
/// field, ours or Graph's (`id`, `extensionName`), is kept.
pub fn merge_extension(extension: &mut Map<String, Value>, fields: &Map<String, Value>) {
    for (key, value) in fields {
        if value.is_null() {
            extension.remove(key);
        } else {
            extension.insert(key.clone(), value.clone());
        }
    }
}

impl Store {
    /// Queue `ops`, one command's, and change each list's cached extension
    /// at once, all in one transaction. Each waits for the latest
    /// unresolved operation on its list. Returns the lists as they are now.
    pub async fn enqueue_list_extension(
        &self,
        command_id: &str,
        undoes: Option<&str>,
        ops: Vec<ListExtensionOp>,
    ) -> Result<Vec<ListRow>, StoreError> {
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let now = now();
        let mut seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM outbox")
            .fetch_one(&mut *tx)
            .await?;
        let mut rows = Vec::with_capacity(ops.len());
        for op in ops {
            seq += 1;
            let list = list_in(&mut tx, &op.list_local_id).await?.ok_or_else(|| {
                StoreError::Invalid(format!("list {} isn't cached", op.list_local_id))
            })?;
            let before: Entity = list
                .extension
                .as_ref()
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut merged = before.clone();
            merge_extension(&mut merged, &op.fields);
            write_extension(&mut tx, &op.list_local_id, &merged, rev).await?;
            let payload = serde_json::json!({ "body": Value::Object(op.fields) });
            insert_op(
                &mut tx,
                &Queued {
                    op_id: &op.op_id,
                    seq,
                    command_id,
                    undoes,
                    created_at: now,
                    entity_local_id: &op.list_local_id,
                    list_local_id: &op.list_local_id,
                    op: OpKind::Extension,
                    action: &op.action,
                    payload: &payload,
                    rollback: Some(&before),
                },
            )
            .await?;
            rows.push(list_in(&mut tx, &op.list_local_id).await?.ok_or_else(|| {
                StoreError::Corrupt(format!("list {} vanished", op.list_local_id))
            })?);
        }
        tx.commit().await?;
        Ok(rows)
    }

    /// Graph now holds `extension` for the list of operation `op_id`: cache
    /// it, with the fields of the list's later unresolved operations on
    /// top, and mark the operation `done`, in one transaction.
    pub async fn record_list_extension(
        &self,
        op_id: &str,
        extension: &Map<String, Value>,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer().begin().await?;
        let rev = next_local_rev(&mut tx).await?;
        let op = op_in(&mut tx, op_id).await?;
        let mut cached = extension.clone();
        for (kind, payload) in later_ops(&mut tx, &op).await? {
            if let (OpKind::Extension, Some(fields)) = (kind, payload["body"].as_object()) {
                merge_extension(&mut cached, fields);
            }
        }
        write_extension(&mut tx, &op.entity_local_id, &cached, rev).await?;
        finish(&mut tx, op_id, OpState::Done, None).await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Undo a folder write's local change: the list's extension becomes the
/// one `restore` holds. A list is never tombstoned by its extension.
pub(crate) async fn restore_list_extension(
    tx: &mut SqliteConnection,
    local_id: &str,
    restore: &Restore,
    rev: i64,
) -> Result<(), StoreError> {
    match restore {
        Restore::Nothing | Restore::Tombstone | Restore::MoveBack { .. } => Ok(()),
        Restore::Replace(extension) => write_extension(tx, local_id, extension, rev).await,
    }
}

/// Make `extension` the list's cached extension (none when it's empty),
/// written by the daemon at `rev`.
async fn write_extension(
    tx: &mut SqliteConnection,
    local_id: &str,
    extension: &Map<String, Value>,
    rev: i64,
) -> Result<(), StoreError> {
    let json = (!extension.is_empty()).then(|| Value::Object(extension.clone()).to_string());
    sqlx::query("UPDATE lists SET extension_json = ?, local_rev = ? WHERE local_id = ?")
        .bind(json)
        .bind(rev)
        .bind(local_id)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn a_merge_sets_and_removes_only_the_fields_given() {
        let current = json!({
            "extensionName": "com.planetaryescape.mstodo",
            "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
            "folder": "Work",
            "order": 3,
            "somethingElse": true
        });
        let mut merged = fields(current);
        merge_extension(
            &mut merged,
            &fields(json!({ "folder": "Areas", "order": null })),
        );
        assert_eq!(
            Value::Object(merged),
            json!({
                "extensionName": "com.planetaryescape.mstodo",
                "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
                "folder": "Areas",
                "somethingElse": true
            })
        );
        let mut none = Map::new();
        merge_extension(&mut none, &fields(json!({ "folder": "Areas" })));
        assert_eq!(Value::Object(none), json!({ "folder": "Areas" }));
    }
}
