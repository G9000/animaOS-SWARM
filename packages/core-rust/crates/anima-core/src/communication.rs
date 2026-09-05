/// Host-neutral lineage for synchronous agent-to-agent requests.
/// Hosts retain responsibility for identity, permissions, scheduling and storage.
#[derive(Clone, Debug)]
pub struct AgentCommunicationRoute {
    participants: Vec<String>,
    remaining: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

pub const MAX_AGENT_COMMUNICATION_HOPS: usize = 3;
pub const MAX_AGENT_COMMUNICATION_REQUESTS: usize = 12;

impl AgentCommunicationRoute {
    pub fn start(sender: impl Into<String>) -> Self {
        Self {
            participants: vec![sender.into()],
            remaining: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(
                MAX_AGENT_COMMUNICATION_REQUESTS,
            )),
        }
    }

    pub fn participants(&self) -> &[String] {
        &self.participants
    }

    pub fn forward(&self, sender: &str, recipient: &str) -> Result<Self, &'static str> {
        if self.participants.last().map(String::as_str) != Some(sender)
            || recipient.trim().is_empty()
        {
            return Err("Invalid communication sender or recipient");
        }
        if self.participants.iter().any(|id| id == recipient) {
            return Err(
                "Agent communication cycle rejected; return your answer to the caller instead",
            );
        }
        if self.participants.len() > MAX_AGENT_COMMUNICATION_HOPS {
            return Err("Agent communication hop limit reached");
        }
        self.remaining
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .map_err(|_| "Agent communication request budget exhausted")?;
        let mut next = self.clone();
        next.participants.push(recipient.into());
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peers_can_forward_but_cannot_loop_or_forge_sender() {
        let a = AgentCommunicationRoute::start("a");
        assert!(a.forward("other", "b").is_err());
        assert!(a.forward("a", "a").is_err());
        let b = a.forward("a", "b").unwrap();
        assert!(b.forward("b", "a").is_err());
        let c = b.forward("b", "c").unwrap();
        let d = c.forward("c", "d").unwrap();
        assert!(d.forward("d", "e").is_err());
        assert_eq!(a.participants(), &["a"]);
    }
    #[test]
    fn broadcasts_share_a_finite_request_budget() {
        let route = AgentCommunicationRoute::start("a");
        for index in 0..MAX_AGENT_COMMUNICATION_REQUESTS {
            route
                .clone()
                .forward("a", &format!("peer-{index}"))
                .unwrap();
        }
        assert!(route.forward("a", "extra").is_err());
    }
}
