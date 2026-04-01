use std::collections::BTreeMap;

use crate::primitives::{DataValue, UuidString};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSettings {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub timeout: Option<u64>,
    pub max_retries: Option<u32>,
    pub additional: BTreeMap<String, DataValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentConfig {
    pub name: String,
    pub model: String,
    pub bio: Option<String>,
    pub lore: Option<String>,
    pub knowledge: Option<Vec<String>>,
    pub topics: Option<Vec<String>>,
    pub adjectives: Option<Vec<String>>,
    pub style: Option<String>,
    pub provider: Option<String>,
    pub system: Option<String>,
    pub tools: Option<Vec<String>>,
    pub plugins: Option<Vec<String>>,
    pub settings: Option<AgentSettings>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Terminated,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentState {
    pub id: UuidString,
    pub name: String,
    pub status: AgentStatus,
    pub config: AgentConfig,
    pub created_at: u128,
    pub token_usage: TokenUsage,
}

#[cfg(test)]
mod tests {
    use super::{AgentConfig, AgentSettings, AgentState, AgentStatus, TokenUsage};

    #[test]
    fn agent_state_keeps_ts_shape_fields() {
        let config = AgentConfig {
            name: "researcher".into(),
            model: "gpt-5.4".into(),
            bio: Some("Finds answers".into()),
            lore: None,
            knowledge: Some(vec!["Rust".into(), "TypeScript".into()]),
            topics: None,
            adjectives: None,
            style: None,
            provider: Some("openai".into()),
            system: None,
            tools: Some(vec!["search".into()]),
            plugins: Some(vec!["notes".into()]),
            settings: Some(AgentSettings {
                temperature: Some(0.2),
                ..AgentSettings::default()
            }),
        };

        let state = AgentState {
            id: "agent-1".into(),
            name: "researcher".into(),
            status: AgentStatus::Idle,
            config,
            created_at: 123,
            token_usage: TokenUsage::default(),
        };

        assert_eq!(state.name, "researcher");
        assert_eq!(state.status, AgentStatus::Idle);
        assert_eq!(state.config.model, "gpt-5.4");
    }
}
