//! Events the daemon pushes to clients that sent `Subscribe`
//! (docs/blueprint/04-sync-cache.md#instant-local-writes): what changed in
//! the cache, and rejected writes. `SyncState` comes from the syncer's own
//! status instead (`server`). A rejection also goes to the daemon's log,
//! and a rejected operation stays `failed` in `ms-todo outbox list`, so
//! nothing is lost for want of a listener.

use ms_todo_protocol::{EntityChanged, Event, MAX_CHANGED_IDS, OpError, WriteRejected};
use tokio::sync::broadcast;

/// Events kept for a subscriber that falls behind.
const BACKLOG: usize = 256;

#[derive(Clone)]
pub(crate) struct Events {
    sender: broadcast::Sender<Event>,
}

impl Events {
    pub fn new() -> Self {
        Self {
            sender: broadcast::Sender::new(BACKLOG),
        }
    }

    /// Graph rejected `op_id`, a change to task `task_id`, for good.
    pub fn write_rejected(&self, op_id: &str, task_id: &str, kind: &str, message: &str) {
        eprintln!("ms-todo daemon: WriteRejected: operation {op_id} ({kind}): {message}");
        // With no subscriber there's nobody to tell; the outbox keeps it.
        let _ = self.sender.send(Event::WriteRejected(WriteRejected {
            op_id: op_id.to_owned(),
            task_id: task_id.to_owned(),
            error: OpError {
                kind: kind.to_owned(),
                message: message.to_owned(),
            },
        }));
    }

    /// These tasks changed in the cache.
    pub fn tasks_changed(&self, tasks: Vec<String>) {
        self.changed(Vec::new(), tasks);
    }

    /// These lists and tasks (local IDs) changed in the cache. Over
    /// [`MAX_CHANGED_IDS`] is `ResyncNeeded`; nothing is no event.
    pub fn changed(&self, lists: Vec<String>, tasks: Vec<String>) {
        let event = match lists.len() + tasks.len() {
            0 => return,
            count if count > MAX_CHANGED_IDS => Event::ResyncNeeded,
            _ => Event::EntityChanged(EntityChanged { lists, tasks }),
        };
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_carry_their_ids_up_to_the_cap_and_a_resync_past_it() {
        let events = Events::new();
        let mut received = events.subscribe();
        events.changed(vec!["l1".into()], vec!["t1".into()]);
        assert_eq!(
            received.try_recv().expect("an event"),
            Event::EntityChanged(EntityChanged {
                lists: vec!["l1".into()],
                tasks: vec!["t1".into()],
            })
        );
        events.tasks_changed(Vec::new());
        assert!(received.try_recv().is_err(), "nothing changed, no event");

        let ids = |count: usize| (0..count).map(|n| format!("t{n}")).collect::<Vec<_>>();
        events.tasks_changed(ids(MAX_CHANGED_IDS));
        assert!(matches!(
            received.try_recv().expect("an event"),
            Event::EntityChanged(changed) if changed.tasks.len() == MAX_CHANGED_IDS
        ));
        events.changed(vec!["l1".into()], ids(MAX_CHANGED_IDS));
        assert_eq!(received.try_recv().expect("an event"), Event::ResyncNeeded);
    }

    #[test]
    fn a_rejection_reaches_a_subscriber() {
        let events = Events::new();
        let mut received = events.subscribe();
        events.write_rejected("op-1", "t1", "rejected", "no");
        assert_eq!(
            received.try_recv().expect("an event"),
            Event::WriteRejected(WriteRejected {
                op_id: "op-1".into(),
                task_id: "t1".into(),
                error: OpError {
                    kind: "rejected".into(),
                    message: "no".into(),
                },
            })
        );
    }
}
