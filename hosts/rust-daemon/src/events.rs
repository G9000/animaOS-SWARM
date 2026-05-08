use tokio::sync::broadcast;

pub(crate) const DEFAULT_EVENT_BUFFER: usize = 64;

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EventEnvelope {
    pub(crate) event: String,
    pub(crate) data: String,
}

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct EventFanout {
    sender: broadcast::Sender<EventEnvelope>,
    capacity: usize,
}

#[allow(dead_code)]
pub(crate) struct EventSubscriber {
    receiver: broadcast::Receiver<EventEnvelope>,
}

#[allow(dead_code)]
impl EventFanout {
    pub(crate) fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender, capacity }
    }

    /// Channel buffer this fanout was constructed with. Used by per-swarm
    /// fanouts so they inherit the daemon's configured event buffer rather
    /// than the compile-time default.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    pub(crate) fn publish(&self, event: impl Into<String>, data: String) {
        let _ = self.sender.send(EventEnvelope {
            event: event.into(),
            data,
        });
    }

    pub(crate) fn subscribe(&self) -> EventSubscriber {
        EventSubscriber {
            receiver: self.sender.subscribe(),
        }
    }
}

#[allow(dead_code)]
impl EventSubscriber {
    pub(crate) async fn recv(&mut self) -> Result<EventEnvelope, broadcast::error::RecvError> {
        self.receiver.recv().await
    }
}

#[cfg(test)]
mod tests {
    use crate::state::DaemonState;

    use super::EventFanout;

    #[tokio::test]
    async fn daemon_state_reuses_the_same_event_fanout() {
        let fanout = EventFanout::new(8);
        let mut subscriber = fanout.subscribe();
        let state = DaemonState::with_events(fanout.clone());

        state
            .event_fanout()
            .publish("health", "{\"status\":\"ok\"}".to_string());

        let event = subscriber.recv().await.expect("event received");
        assert_eq!(event.event, "health");
        assert_eq!(event.data, "{\"status\":\"ok\"}");
    }
}
