use anima_swarm::{AgentMessage, SwarmConfig, SwarmState, SwarmStrategy};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::agents::AgentConfigRequest;
use super::shared::{ContentResponse, TaskResultResponse, TokenUsageResponse};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwarmStateResponse {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) agent_ids: Vec<String>,
    pub(crate) messages: Vec<SwarmMessageResponse>,
    pub(crate) results: Vec<TaskResultResponse>,
    pub(crate) token_usage: TokenUsageResponse,
    pub(crate) started_at: Option<u64>,
    pub(crate) completed_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwarmMessageResponse {
    pub(crate) id: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) content: ContentResponse,
    pub(crate) timestamp: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SwarmEnvelope {
    pub(crate) swarm: SwarmStateResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SwarmsEnvelope {
    pub(crate) swarms: Vec<SwarmStateResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SwarmRunEnvelope {
    pub(crate) swarm: SwarmStateResponse,
    pub(crate) result: TaskResultResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwarmEventResponse {
    pub(crate) swarm_id: String,
    pub(crate) state: SwarmStateResponse,
    pub(crate) result: Option<TaskResultResponse>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwarmCreateRequest {
    pub(crate) strategy: Option<String>,
    pub(crate) manager: Option<AgentConfigRequest>,
    pub(crate) workers: Option<Vec<AgentConfigRequest>>,
    pub(crate) max_concurrent_agents: Option<u64>,
    pub(crate) max_parallel_delegations: Option<u64>,
    pub(crate) max_turns: Option<u64>,
    pub(crate) token_budget: Option<u64>,
}

impl SwarmCreateRequest {
    pub(crate) fn into_domain(self) -> Result<SwarmConfig, &'static str> {
        let strategy = match self.strategy.as_deref() {
            Some("supervisor") => SwarmStrategy::Supervisor,
            Some("dynamic") => SwarmStrategy::Dynamic,
            Some("round-robin") => SwarmStrategy::RoundRobin,
            Some(_) => return Err("strategy must be supervisor, dynamic, or round-robin"),
            None => return Err("strategy is required"),
        };

        let manager = self.manager.ok_or("manager is required")?.into_domain()?;
        let workers = self
            .workers
            .ok_or("workers are required")?
            .into_iter()
            .map(AgentConfigRequest::into_domain)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(SwarmConfig {
            strategy,
            manager,
            workers,
            max_concurrent_agents: self.max_concurrent_agents.map(|value| value as usize),
            max_parallel_delegations: self.max_parallel_delegations.map(|value| value as usize),
            max_turns: self.max_turns.map(|value| value as usize),
            token_budget: self.token_budget,
        })
    }
}

impl From<&SwarmState> for SwarmStateResponse {
    fn from(value: &SwarmState) -> Self {
        Self {
            id: value.id.clone(),
            status: value.status.as_str().to_string(),
            agent_ids: value.agent_ids.clone(),
            messages: value
                .messages
                .iter()
                .map(SwarmMessageResponse::from)
                .collect(),
            results: value.results.iter().map(TaskResultResponse::from).collect(),
            token_usage: TokenUsageResponse::from(&value.token_usage),
            started_at: value.started_at,
            completed_at: value.completed_at,
        }
    }
}

impl From<&AgentMessage> for SwarmMessageResponse {
    fn from(value: &AgentMessage) -> Self {
        Self {
            id: value.id.clone(),
            from: value.from.clone(),
            to: value.to.clone(),
            content: ContentResponse::from(&value.content),
            timestamp: value.timestamp,
        }
    }
}
