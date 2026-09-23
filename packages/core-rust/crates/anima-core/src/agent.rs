use std::collections::BTreeMap;
use std::str::FromStr;

use crate::primitives::{DataValue, UuidString};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolExample {
    pub input: String,
    pub args: BTreeMap<String, DataValue>,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters_schema: BTreeMap<String, DataValue>,
    pub examples: Option<Vec<ToolExample>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSettings {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    /// Cap on tool-calling turns inside a single `AgentRuntime::run` invocation.
    /// Defaults to `runtime::MAX_TOOL_ITERATIONS` (8) when not set.
    pub max_tool_iterations: Option<usize>,
    pub additional: BTreeMap<String, DataValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    pub tools: Option<Vec<ToolDescriptor>>,
    pub plugins: Option<Vec<PluginDescriptor>>,
    pub settings: Option<AgentSettings>,
}

impl AgentConfig {
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        self.tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool.name == tool_name))
    }
}

pub fn tool_not_configured_error(tool_name: &str) -> String {
    format!("tool '{tool_name}' is not configured for this agent")
}

/// Partial update for an existing agent's config. Only the fields that are
/// `Some` are applied; everything else is left untouched.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentConfigUpdate {
    pub name: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub system: Option<String>,
    pub tools: Option<Vec<ToolDescriptor>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Terminated,
}

impl AgentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Terminated => "terminated",
        }
    }
}

impl FromStr for AgentStatus {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "idle" => Ok(Self::Idle),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "terminated" => Ok(Self::Terminated),
            _ => Err("unknown agent status"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    /// Prompt tokens served from a provider cache; included in `prompt_tokens`.
    #[serde(default)]
    pub cached_prompt_tokens: u64,
    /// Reasoning or thinking tokens; included in `completion_tokens`.
    #[serde(default)]
    pub reasoning_tokens: u64,
}

impl TokenUsage {
    /// Adds `other` into `self` field by field, saturating instead of overflowing.
    pub fn saturating_add(&mut self, other: &TokenUsage) {
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.completion_tokens = self.completion_tokens.saturating_add(other.completion_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.cached_prompt_tokens = self
            .cached_prompt_tokens
            .saturating_add(other.cached_prompt_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentState {
    pub id: UuidString,
    /// Denormalized display name mirrored from config.name for snapshots and transport ergonomics.
    pub name: String,
    pub status: AgentStatus,
    pub config: AgentConfig,
    pub created_at_ms: u64,
    pub token_usage: TokenUsage,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentConfig, AgentSettings, AgentState, AgentStatus, PluginDescriptor, TokenUsage,
        ToolDescriptor,
    };
    use std::collections::BTreeMap;
    use std::str::FromStr;

    #[test]
    fn token_usage_saturating_add_sums_all_five_fields_and_saturates_on_overflow() {
        let mut total = TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 2,
            total_tokens: 3,
            cached_prompt_tokens: 4,
            reasoning_tokens: 5,
        };
        total.saturating_add(&TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
            cached_prompt_tokens: 40,
            reasoning_tokens: 50,
        });
        assert_eq!(
            total,
            TokenUsage {
                prompt_tokens: 11,
                completion_tokens: 22,
                total_tokens: 33,
                cached_prompt_tokens: 44,
                reasoning_tokens: 55,
            }
        );

        let mut saturating = TokenUsage {
            prompt_tokens: u64::MAX,
            completion_tokens: u64::MAX,
            total_tokens: u64::MAX,
            cached_prompt_tokens: u64::MAX,
            reasoning_tokens: u64::MAX,
        };
        saturating.saturating_add(&TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 1,
            cached_prompt_tokens: 1,
            reasoning_tokens: 1,
        });
        assert_eq!(
            saturating,
            TokenUsage {
                prompt_tokens: u64::MAX,
                completion_tokens: u64::MAX,
                total_tokens: u64::MAX,
                cached_prompt_tokens: u64::MAX,
                reasoning_tokens: u64::MAX,
            }
        );
    }

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
            tools: Some(vec![ToolDescriptor {
                name: "search".into(),
                description: "Search the web".into(),
                parameters_schema: BTreeMap::new(),
                examples: None,
            }]),
            plugins: Some(vec![PluginDescriptor {
                name: "notes".into(),
                description: "Workspace notes".into(),
            }]),
            settings: Some(AgentSettings {
                temperature: Some(0.2),
                timeout_ms: Some(5_000),
                ..AgentSettings::default()
            }),
        };

        let state = AgentState {
            id: "agent-1".into(),
            name: "researcher".into(),
            status: AgentStatus::Idle,
            config,
            created_at_ms: 123,
            token_usage: TokenUsage::default(),
        };

        assert_eq!(state.name, "researcher");
        assert_eq!(state.name, state.config.name);
        assert_eq!(state.status, AgentStatus::Idle);
        assert_eq!(state.created_at_ms, 123);
        assert_eq!(state.config.model, "gpt-5.4");
        assert_eq!(
            state
                .config
                .settings
                .as_ref()
                .and_then(|settings| settings.timeout_ms),
            Some(5_000)
        );
        assert_eq!(
            state
                .config
                .tools
                .as_ref()
                .map(|tools| tools[0].name.as_str()),
            Some("search")
        );
    }

    #[test]
    fn agent_status_round_trips_string_values() {
        let statuses = [
            (AgentStatus::Idle, "idle"),
            (AgentStatus::Running, "running"),
            (AgentStatus::Completed, "completed"),
            (AgentStatus::Failed, "failed"),
            (AgentStatus::Terminated, "terminated"),
        ];

        for (status, text) in statuses {
            assert_eq!(status.as_str(), text);
            assert_eq!(AgentStatus::from_str(text), Ok(status));
        }

        assert_eq!(
            AgentStatus::from_str("unknown"),
            Err("unknown agent status")
        );
    }

    #[test]
    fn agent_settings_default_leaves_optional_fields_empty() {
        let settings = AgentSettings::default();

        assert_eq!(settings.temperature, None);
        assert_eq!(settings.max_tokens, None);
        assert_eq!(settings.timeout_ms, None);
        assert_eq!(settings.max_retries, None);
        assert!(settings.additional.is_empty());
    }
}

#[cfg(test)]
mod token_usage_tests {
    use super::TokenUsage;

    #[test]
    fn usage_saved_before_detail_fields_loads_with_zero_details() {
        let usage: TokenUsage =
            serde_json::from_str(r#"{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}"#)
                .expect("legacy usage deserializes");

        assert_eq!(
            usage,
            TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 4,
                total_tokens: 7,
                cached_prompt_tokens: 0,
                reasoning_tokens: 0,
            }
        );
    }
}
