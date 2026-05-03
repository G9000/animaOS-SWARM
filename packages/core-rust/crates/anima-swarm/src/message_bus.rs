use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anima_core::Content;

use crate::types::{AgentMessage, SwarmMessageBus};

static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct MessageBus {
    inboxes: HashMap<String, Vec<AgentMessage>>,
    all_messages: Vec<AgentMessage>,
}

impl MessageBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_agent(&mut self, agent_id: &str) {
        self.inboxes.entry(agent_id.to_string()).or_default();
    }

    pub fn unregister_agent(&mut self, agent_id: &str) {
        self.inboxes.remove(agent_id);
    }

    pub fn send(&mut self, from: &str, to: &str, content: Content) {
        self.send_message(from, to, content);
    }

    pub fn broadcast(&mut self, from: &str, content: Content) {
        self.broadcast_message(from, content);
    }

    pub fn send_message(&mut self, from: &str, to: &str, content: Content) -> AgentMessage {
        let message = self.next_message(from, to, content);
        self.all_messages.push(message.clone());

        if let Some(inbox) = self.inboxes.get_mut(to) {
            inbox.push(message.clone());
        }

        message
    }

    pub fn broadcast_message(&mut self, from: &str, content: Content) -> AgentMessage {
        let message = self.next_message(from, "broadcast", content);
        self.all_messages.push(message.clone());

        for (agent_id, inbox) in &mut self.inboxes {
            if agent_id != from {
                inbox.push(message.clone());
            }
        }

        message
    }

    pub fn get_messages(&self, agent_id: &str) -> Vec<AgentMessage> {
        <Self as SwarmMessageBus>::get_messages(self, agent_id)
    }

    pub fn get_all_messages(&self) -> Vec<AgentMessage> {
        <Self as SwarmMessageBus>::get_all_messages(self)
    }

    pub fn clear(&mut self) {
        <Self as SwarmMessageBus>::clear(self);
    }

    pub fn clear_inboxes(&mut self) {
        <Self as SwarmMessageBus>::clear_inboxes(self);
    }

    fn next_message(&self, from: &str, to: &str, content: Content) -> AgentMessage {
        AgentMessage {
            id: format!(
                "swarm-msg-{}",
                NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed) + 1
            ),
            from: from.to_string(),
            to: to.to_string(),
            content,
            timestamp: now_millis(),
        }
    }
}

impl SwarmMessageBus for MessageBus {
    fn send(&mut self, from: &str, to: &str, content: Content) {
        self.send_message(from, to, content);
    }

    fn broadcast(&mut self, from: &str, content: Content) {
        self.broadcast_message(from, content);
    }

    fn get_messages(&self, agent_id: &str) -> Vec<AgentMessage> {
        self.inboxes.get(agent_id).cloned().unwrap_or_default()
    }

    fn get_all_messages(&self) -> Vec<AgentMessage> {
        self.all_messages.clone()
    }

    fn clear(&mut self) {
        self.inboxes.clear();
        self.all_messages.clear();
    }

    fn clear_inboxes(&mut self) {
        for inbox in self.inboxes.values_mut() {
            inbox.clear();
        }
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_millis()
}
