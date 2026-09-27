//! Pruning finished operations (D-065), so the outbox doesn't grow for as
//! long as ms-todo is used. A command goes once every operation of it is
//! `done` and the last finished before the cutoff. A discarded operation
//! is deleted when it's discarded, and a `failed` or unresolved one waits
//! for the user, so neither is pruned.
//!
//! Two rules keep what's left consistent:
//!
//! - An operation some unfinished one depends on stays: `ready_ops` sends
//!   an operation only once its dependency is `done`, and one that's gone
//!   would hold it back for ever (a `failed` one that's retried, say).
//! - A command and the undo of it go together or not at all. With the
//!   undo gone and the command kept, `undo` would offer it again; with the
//!   command gone, the undo would refer to nothing.

use std::collections::{HashMap, HashSet};

use crate::{Store, StoreError};

impl Store {
    /// Delete the commands whose operations are all `done` and finished at
    /// or before `cutoff` (Unix seconds), within the rules above. Returns
    /// how many operations went.
    pub async fn prune_outbox(&self, cutoff: i64) -> Result<u64, StoreError> {
        let mut tx = self.writer().begin().await?;
        // An operation recorded `done` from the start (a category write)
        // may have no `finished_at`; it finished when it was made.
        let old: Vec<String> = sqlx::query_scalar(
            "SELECT o.command_id FROM outbox o GROUP BY o.command_id \
             HAVING SUM(o.state != 'done') = 0 \
             AND MAX(COALESCE(o.finished_at, o.created_at)) <= ? \
             AND NOT EXISTS (SELECT 1 FROM outbox w JOIN outbox d ON d.op_id = w.depends_on_op_id \
             WHERE d.command_id = o.command_id AND w.state != 'done')",
        )
        .bind(cutoff)
        .fetch_all(&mut *tx)
        .await?;
        if old.is_empty() {
            return Ok(0);
        }
        // (the undo, the command it undoes), for every undo still here.
        let undos: Vec<(String, String)> = sqlx::query_as(
            "SELECT DISTINCT command_id, undoes_command_id FROM outbox \
             WHERE undoes_command_id IS NOT NULL",
        )
        .fetch_all(&mut *tx)
        .await?;
        let present: HashSet<String> = sqlx::query_scalar("SELECT DISTINCT command_id FROM outbox")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
        let prunable = together(old.into_iter().collect(), &undos, &present);
        let mut removed = 0;
        for command in &prunable {
            removed += sqlx::query("DELETE FROM outbox WHERE command_id = ?")
                .bind(command)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        }
        tx.commit().await?;
        Ok(removed)
    }
}

/// `old` without any command linked by an undo to one that stays: one in
/// the outbox (`present`) but not in `old`. A chain of undos (an undo
/// undone, and so on) stays whole, so this repeats until nothing changes.
fn together(
    mut old: HashSet<String>,
    undos: &[(String, String)],
    present: &HashSet<String>,
) -> HashSet<String> {
    let mut linked: HashMap<&str, Vec<&str>> = HashMap::new();
    for (undo, target) in undos {
        linked.entry(undo).or_default().push(target);
        linked.entry(target).or_default().push(undo);
    }
    loop {
        let staying: Vec<String> = old
            .iter()
            .filter(|command| {
                linked.get(command.as_str()).is_some_and(|others| {
                    others
                        .iter()
                        .any(|other| present.contains(*other) && !old.contains(*other))
                })
            })
            .cloned()
            .collect();
        if staying.is_empty() {
            return old;
        }
        for command in staying {
            old.remove(&command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn undo(undo: &str, target: &str) -> (String, String) {
        (undo.to_owned(), target.to_owned())
    }

    #[test]
    fn a_command_and_its_undo_go_together() {
        let present = set(&["a", "u", "b"]);
        // Both old: both go.
        assert_eq!(
            together(set(&["a", "u"]), &[undo("u", "a")], &present),
            set(&["a", "u"])
        );
        // The undo is recent: the command stays with it.
        assert_eq!(
            together(set(&["a", "b"]), &[undo("u", "a")], &present),
            set(&["b"])
        );
        // An undo whose command stays (a failed op, say) stays too.
        assert_eq!(together(set(&["u"]), &[undo("u", "a")], &present), set(&[]));
    }

    #[test]
    fn a_chain_of_undos_stays_whole() {
        // a, undone by u1, which u2 undid: u2 is recent.
        let present = set(&["a", "u1", "u2"]);
        let undos = [undo("u1", "a"), undo("u2", "u1")];
        assert_eq!(together(set(&["a", "u1"]), &undos, &present), set(&[]));
    }

    #[test]
    fn an_undo_of_a_command_pruned_before_can_go() {
        let present = set(&["u"]);
        assert_eq!(
            together(set(&["u"]), &[undo("u", "a")], &present),
            set(&["u"])
        );
    }
}
