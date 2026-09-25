//! One broadcast channel per watched agent, capped at 16 subscribers
//! (spec §6). An agent nobody watches has no channel, so publishing to it
//! costs nothing. Closing the hub at shutdown ends every stream.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::{broadcast, watch};

use super::events::LiveEvent;
use super::registry::LiveRuns;
use super::MAX_EVENT_SUBSCRIBERS_PER_AGENT;

struct AgentChannel {
    sender: broadcast::Sender<Arc<LiveEvent>>,
    subscribers: usize,
}

struct HubInner {
    capacity: usize,
    channels: Mutex<HashMap<String, AgentChannel>>,
    runs: LiveRuns,
    lagged: AtomicU64,
    /// Set once, at shutdown; every subscription watches it.
    closed: watch::Sender<bool>,
}

impl HubInner {
    fn channels(&self) -> MutexGuard<'_, HashMap<String, AgentChannel>> {
        self.channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The daemon's live hub: agent fanouts and the runs in flight.
#[derive(Clone)]
pub(crate) struct LiveHub {
    inner: Arc<HubInner>,
}

/// The agent already has `MAX_EVENT_SUBSCRIBERS_PER_AGENT` streams open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubscriberLimit;

/// What a subscription yields next.
#[derive(Debug)]
pub(crate) enum LiveDelivery {
    Event(Arc<LiveEvent>),
    /// The subscription fell behind and this many events were dropped.
    Lagged(u64),
}

impl LiveHub {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(HubInner {
                capacity: capacity.max(1),
                channels: Mutex::new(HashMap::new()),
                runs: LiveRuns::default(),
                lagged: AtomicU64::new(0),
                closed: watch::channel(false).0,
            }),
        }
    }

    pub(crate) fn runs(&self) -> &LiveRuns {
        &self.inner.runs
    }

    /// Ends every open stream, and every stream opened later right after its
    /// snapshot. An SSE response never ends on its own, so graceful shutdown
    /// calls this first; otherwise one open console tab would block the exit.
    pub(crate) fn close(&self) {
        self.inner.closed.send_replace(true);
    }

    /// Sends `event` to its agent's stream and, when it differs, to
    /// `parent_agent_id`'s (a helper's or delegated run's companion).
    pub(crate) fn publish(&self, event: LiveEvent, parent_agent_id: Option<&str>) {
        let event = Arc::new(event);
        let channels = self.inner.channels();
        let parent = parent_agent_id.filter(|parent| *parent != event.agent_id);
        for agent_id in std::iter::once(event.agent_id.as_str()).chain(parent) {
            if let Some(channel) = channels.get(agent_id) {
                // No receivers left is not an error: the stream is closing.
                let _ = channel.sender.send(Arc::clone(&event));
            }
        }
    }

    pub(crate) fn subscribe(&self, agent_id: &str) -> Result<LiveSubscription, SubscriberLimit> {
        let mut channels = self.inner.channels();
        let channel = channels
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentChannel {
                sender: broadcast::channel(self.inner.capacity).0,
                subscribers: 0,
            });
        if channel.subscribers >= MAX_EVENT_SUBSCRIBERS_PER_AGENT {
            return Err(SubscriberLimit);
        }
        channel.subscribers += 1;
        Ok(LiveSubscription {
            receiver: channel.sender.subscribe(),
            closed: self.inner.closed.subscribe(),
            guard: SubscriberGuard {
                hub: Arc::clone(&self.inner),
                agent_id: agent_id.to_string(),
            },
        })
    }

    /// Open streams of `agent_id`; M8's status and metrics read it (spec §11.3).
    #[allow(dead_code)]
    pub(crate) fn subscribers(&self, agent_id: &str) -> usize {
        self.inner
            .channels()
            .get(agent_id)
            .map_or(0, |channel| channel.subscribers)
    }

    /// Events dropped for lagging subscribers since start; M8's metrics read
    /// it (spec §11.3).
    #[allow(dead_code)]
    pub(crate) fn lagged_events(&self) -> u64 {
        self.inner.lagged.load(Ordering::Relaxed)
    }
}

struct SubscriberGuard {
    hub: Arc<HubInner>,
    agent_id: String,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        let mut channels = self.hub.channels();
        if let Some(channel) = channels.get_mut(&self.agent_id) {
            channel.subscribers = channel.subscribers.saturating_sub(1);
            if channel.subscribers == 0 {
                channels.remove(&self.agent_id);
            }
        }
    }
}

/// One open stream of an agent's events.
pub(crate) struct LiveSubscription {
    receiver: broadcast::Receiver<Arc<LiveEvent>>,
    closed: watch::Receiver<bool>,
    guard: SubscriberGuard,
}

impl LiveSubscription {
    /// The next event, a lag marker, or `None` once the hub closed.
    pub(crate) async fn next(&mut self) -> Option<LiveDelivery> {
        let received = tokio::select! {
            // Checked first: once the daemon stops, buffered events are moot.
            biased;
            _ = self.closed.wait_for(|closed| *closed) => return None,
            received = self.receiver.recv() => received,
        };
        match received {
            Ok(event) => Some(LiveDelivery::Event(event)),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                self.guard.hub.lagged.fetch_add(missed, Ordering::Relaxed);
                Some(LiveDelivery::Lagged(missed))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}
