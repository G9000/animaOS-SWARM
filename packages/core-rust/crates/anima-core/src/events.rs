use crate::primitives::{DataValue, UuidString};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    AgentSpawned,
    AgentStarted,
    AgentCompleted,
    AgentFailed,
    AgentTerminated,
    AgentMessage,
    TaskStarted,
    TaskCompleted,
    TaskFailed,
    ToolBefore,
    ToolAfter,
    AgentTokens,
    SwarmCreated,
    SwarmCompleted,
    SwarmStopped,
}

impl EventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentSpawned => "agent:spawned",
            Self::AgentStarted => "agent:started",
            Self::AgentCompleted => "agent:completed",
            Self::AgentFailed => "agent:failed",
            Self::AgentTerminated => "agent:terminated",
            Self::AgentMessage => "agent:message",
            Self::TaskStarted => "task:started",
            Self::TaskCompleted => "task:completed",
            Self::TaskFailed => "task:failed",
            Self::ToolBefore => "tool:before",
            Self::ToolAfter => "tool:after",
            Self::AgentTokens => "agent:tokens",
            Self::SwarmCreated => "swarm:created",
            Self::SwarmCompleted => "swarm:completed",
            Self::SwarmStopped => "swarm:stopped",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineEvent {
    pub id: UuidString,
    pub event_type: EventType,
    pub agent_id: Option<String>,
    pub timestamp_ms: u64,
    pub data: DataValue,
}

#[cfg(test)]
mod tests {
    use super::{EngineEvent, EventType};
    use crate::primitives::DataValue;

    #[test]
    fn event_type_strings_match_ts_contract() {
        assert_eq!(EventType::AgentSpawned.as_str(), "agent:spawned");
        assert_eq!(EventType::ToolAfter.as_str(), "tool:after");
        assert_eq!(EventType::SwarmStopped.as_str(), "swarm:stopped");
    }

    #[test]
    fn engine_event_holds_agent_id_and_payload() {
        let event = EngineEvent {
            id: "event-1".into(),
            event_type: EventType::TaskCompleted,
            agent_id: Some("agent-1".into()),
            timestamp_ms: 123,
            data: DataValue::String("done".into()),
        };

        assert_eq!(event.id, "event-1");
        assert_eq!(event.agent_id.as_deref(), Some("agent-1"));
        assert_eq!(event.timestamp_ms, 123);
    }
}
