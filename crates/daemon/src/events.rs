//! Events the daemon pushes to clients (docs/blueprint/04-sync-cache.md#instant-local-writes).
//! Rung 4 has one, `WriteRejected`, and no client subscribes yet: the TUI
//! does from rung 5. Until then each event also goes to the daemon's log,
//! and a rejected operation stays `failed` in `ms-todo outbox list`, so
//! nothing is lost for want of a listener.

use ms_todo_protocol::{Event, OpError, WriteRejected};
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

    #[cfg(test)]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
