use anima_core::{AgentConfig, Content, TaskResult, TokenUsage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwarmStrategy {
    Supervisor,
    Dynamic,
    RoundRobin,
}

impl SwarmStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Dynamic => "dynamic",
            Self::RoundRobin => "round-robin",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwarmConfig {
    pub strategy: SwarmStrategy,
    pub manager: AgentConfig,
    pub workers: Vec<AgentConfig>,
    pub max_concurrent_agents: Option<usize>,
    pub max_turns: Option<usize>,
    pub token_budget: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwarmStatus {
    Idle,
    Running,
    Completed,
    Failed,
}

impl SwarmStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwarmState {
    pub id: String,
    pub status: SwarmStatus,
    pub agent_ids: Vec<String>,
    pub results: Vec<TaskResult<Content>>,
    pub token_usage: TokenUsage,
    pub started_at: Option<u128>,
    pub completed_at: Option<u128>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub content: Content,
    pub timestamp: u128,
}

pub trait SwarmMessageBus {
    fn send(&mut self, from: &str, to: &str, content: Content);
    fn broadcast(&mut self, from: &str, content: Content);
    fn get_messages(&self, agent_id: &str) -> Vec<AgentMessage>;
    fn get_all_messages(&self) -> Vec<AgentMessage>;
    fn clear(&mut self);
    fn clear_inboxes(&mut self);
}

pub struct StrategyContext<'a> {
    pub task: String,
    pub manager_config: AgentConfig,
    pub worker_configs: Vec<AgentConfig>,
    pub spawn_agent: &'a mut dyn FnMut(AgentConfig) -> TaskResult<Content>,
    pub message_bus: &'a mut dyn SwarmMessageBus,
    pub max_turns: usize,
}
