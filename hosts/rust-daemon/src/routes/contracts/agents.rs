use std::collections::{BTreeMap, HashMap};

use anima_core::{
    AgentConfig, AgentRuntimeSnapshot, AgentSettings, AgentState, DataValue, Message, MessageRole,
    PluginDescriptor, ToolDescriptor, ToolExample,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use super::shared::{
    data_value_to_json, json_to_data_value, number_value, parse_usize, required_string, u32_value,
    u64_value, usize_value, ContentResponse, TaskResultResponse, TokenUsageResponse,
};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolExampleResponse {
    pub(crate) input: String,
    pub(crate) args: BTreeMap<String, Value>,
    pub(crate) output: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDescriptorResponse {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: BTreeMap<String, Value>,
    pub(crate) examples: Option<Vec<ToolExampleResponse>>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginDescriptorResponse {
    pub(crate) name: String,
    pub(crate) description: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentSettingsResponse {
    pub(crate) temperature: Option<f64>,
    #[serde(rename = "maxTokens")]
    pub(crate) max_tokens: Option<u32>,
    pub(crate) timeout_ms: Option<u64>,
    #[serde(rename = "maxRetries")]
    pub(crate) max_retries: Option<u32>,
    #[serde(rename = "maxToolIterations")]
    pub(crate) max_tool_iterations: Option<usize>,
    pub(crate) additional: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentConfigResponse {
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) bio: Option<String>,
    pub(crate) lore: Option<String>,
    pub(crate) knowledge: Option<Vec<String>>,
    pub(crate) topics: Option<Vec<String>>,
    pub(crate) adjectives: Option<Vec<String>>,
    pub(crate) style: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) system: Option<String>,
    pub(crate) tools: Option<Vec<ToolDescriptorResponse>>,
    pub(crate) plugins: Option<Vec<PluginDescriptorResponse>>,
    pub(crate) settings: Option<AgentSettingsResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentStateResponse {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) config: AgentConfigResponse,
    pub(crate) created_at_ms: u64,
    pub(crate) token_usage: TokenUsageResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRuntimeSnapshotResponse {
    pub(crate) state: AgentStateResponse,
    pub(crate) message_count: usize,
    pub(crate) messages: Vec<AgentMessageResponse>,
    pub(crate) event_count: usize,
    pub(crate) last_task: Option<TaskResultResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentMessageResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) room_id: String,
    pub(crate) content: ContentResponse,
    pub(crate) role: String,
    pub(crate) created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentEnvelope {
    pub(crate) agent: AgentRuntimeSnapshotResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentsEnvelope {
    pub(crate) agents: Vec<AgentRuntimeSnapshotResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentRunEnvelope {
    pub(crate) agent: AgentRuntimeSnapshotResponse,
    pub(crate) result: TaskResultResponse,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolExampleRequestObject {
    pub(crate) input: Option<String>,
    #[serde(default)]
    pub(crate) args: BTreeMap<String, Value>,
    pub(crate) output: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDescriptorRequestObject {
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) parameters: BTreeMap<String, Value>,
    pub(crate) examples: Option<Vec<ToolExampleRequestObject>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum ToolDescriptorRequest {
    Name(String),
    Detailed(ToolDescriptorRequestObject),
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginDescriptorRequestObject {
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum PluginDescriptorRequest {
    Name(String),
    Detailed(PluginDescriptorRequestObject),
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentSettingsRequest {
    pub(crate) temperature: Option<Value>,
    #[serde(rename = "maxTokens")]
    pub(crate) max_tokens: Option<Value>,
    #[serde(alias = "timeout")]
    pub(crate) timeout_ms: Option<Value>,
    #[serde(rename = "maxRetries")]
    pub(crate) max_retries: Option<Value>,
    #[serde(rename = "maxToolIterations")]
    pub(crate) max_tool_iterations: Option<Value>,
    #[serde(flatten)]
    pub(crate) additional: HashMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentConfigRequest {
    pub(crate) name: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) bio: Option<String>,
    pub(crate) lore: Option<String>,
    pub(crate) knowledge: Option<Vec<String>>,
    pub(crate) topics: Option<Vec<String>>,
    pub(crate) adjectives: Option<Vec<String>>,
    pub(crate) style: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) system: Option<String>,
    pub(crate) tools: Option<Vec<ToolDescriptorRequest>>,
    pub(crate) plugins: Option<Vec<PluginDescriptorRequest>>,
    pub(crate) settings: Option<AgentSettingsRequest>,
}

#[derive(Clone, Debug, Deserialize, IntoParams, ToSchema, Default)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRecentMemoriesQuery {
    pub(crate) limit: Option<usize>,
}

impl AgentConfigRequest {
    pub(crate) fn into_domain(self) -> Result<AgentConfig, &'static str> {
        Ok(AgentConfig {
            name: required_string(self.name, "name is required")?,
            model: required_string(self.model, "model is required")?,
            bio: self.bio,
            lore: self.lore,
            knowledge: self.knowledge,
            topics: self.topics,
            adjectives: self.adjectives,
            style: self.style,
            provider: self.provider,
            system: self.system,
            tools: self
                .tools
                .map(|tools| {
                    tools
                        .into_iter()
                        .map(ToolDescriptorRequest::into_domain)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            plugins: self
                .plugins
                .map(|plugins| {
                    plugins
                        .into_iter()
                        .map(PluginDescriptorRequest::into_domain)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            settings: self
                .settings
                .map(AgentSettingsRequest::into_domain)
                .transpose()?,
        })
    }
}

impl ToolDescriptorRequest {
    fn into_domain(self) -> Result<ToolDescriptor, &'static str> {
        match self {
            Self::Name(name) if !name.is_empty() => Ok(ToolDescriptor {
                name,
                description: String::new(),
                parameters_schema: BTreeMap::new(),
                examples: None,
            }),
            Self::Name(_) => Err("tools must contain strings or objects"),
            Self::Detailed(value) => Ok(ToolDescriptor {
                name: required_string(value.name, "tool name is required")?,
                description: value.description,
                parameters_schema: value
                    .parameters
                    .into_iter()
                    .map(|(key, value)| {
                        Ok::<(String, DataValue), &'static str>((key, json_to_data_value(value)?))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?,
                examples: value
                    .examples
                    .map(|examples| {
                        examples
                            .into_iter()
                            .map(ToolExampleRequestObject::into_domain)
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()?,
            }),
        }
    }
}

impl ToolExampleRequestObject {
    fn into_domain(self) -> Result<ToolExample, &'static str> {
        Ok(ToolExample {
            input: required_string(self.input, "tool example input is required")?,
            args: self
                .args
                .into_iter()
                .map(|(key, value)| {
                    Ok::<(String, DataValue), &'static str>((key, json_to_data_value(value)?))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?,
            output: required_string(self.output, "tool example output is required")?,
        })
    }
}

impl PluginDescriptorRequest {
    fn into_domain(self) -> Result<PluginDescriptor, &'static str> {
        match self {
            Self::Name(name) if !name.is_empty() => Ok(PluginDescriptor {
                name,
                description: String::new(),
            }),
            Self::Name(_) => Err("plugins must contain strings or objects"),
            Self::Detailed(value) => Ok(PluginDescriptor {
                name: required_string(value.name, "plugin name is required")?,
                description: value.description,
            }),
        }
    }
}

impl AgentSettingsRequest {
    fn into_domain(mut self) -> Result<AgentSettings, &'static str> {
        let mut settings = AgentSettings::default();

        settings.temperature = self
            .temperature
            .take()
            .map(|value| number_value(value, "temperature"))
            .transpose()?;
        settings.max_tokens = self
            .max_tokens
            .take()
            .map(|value| u32_value(value, "maxTokens"))
            .transpose()?;
        settings.timeout_ms = self
            .timeout_ms
            .take()
            .map(|value| u64_value(value, "timeoutMs"))
            .transpose()?;
        settings.max_retries = self
            .max_retries
            .take()
            .map(|value| u32_value(value, "maxRetries"))
            .transpose()?;
        settings.max_tool_iterations = self
            .max_tool_iterations
            .take()
            .map(|value| usize_value(value, "maxToolIterations"))
            .transpose()?;

        settings.additional = self
            .additional
            .into_iter()
            .map(|(key, value)| {
                Ok::<(String, DataValue), &'static str>((key, json_to_data_value(value)?))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(settings)
    }
}

impl AgentRecentMemoriesQuery {
    pub(crate) fn from_query_map(query: &HashMap<String, String>) -> Result<Self, &'static str> {
        Ok(Self {
            limit: query
                .get("limit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
        })
    }
}

impl From<&ToolExample> for ToolExampleResponse {
    fn from(value: &ToolExample) -> Self {
        Self {
            input: value.input.clone(),
            args: value
                .args
                .iter()
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
            output: value.output.clone(),
        }
    }
}

impl From<&ToolDescriptor> for ToolDescriptorResponse {
    fn from(value: &ToolDescriptor) -> Self {
        Self {
            name: value.name.clone(),
            description: value.description.clone(),
            parameters: value
                .parameters_schema
                .iter()
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
            examples: value
                .examples
                .as_ref()
                .map(|examples| examples.iter().map(ToolExampleResponse::from).collect()),
        }
    }
}

impl From<&PluginDescriptor> for PluginDescriptorResponse {
    fn from(value: &PluginDescriptor) -> Self {
        Self {
            name: value.name.clone(),
            description: value.description.clone(),
        }
    }
}

impl From<&AgentSettings> for AgentSettingsResponse {
    fn from(value: &AgentSettings) -> Self {
        Self {
            temperature: value.temperature,
            max_tokens: value.max_tokens,
            timeout_ms: value.timeout_ms,
            max_retries: value.max_retries,
            max_tool_iterations: value.max_tool_iterations,
            additional: value
                .additional
                .iter()
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
        }
    }
}

impl From<&AgentConfig> for AgentConfigResponse {
    fn from(value: &AgentConfig) -> Self {
        Self {
            name: value.name.clone(),
            model: value.model.clone(),
            bio: value.bio.clone(),
            lore: value.lore.clone(),
            knowledge: value.knowledge.clone(),
            topics: value.topics.clone(),
            adjectives: value.adjectives.clone(),
            style: value.style.clone(),
            provider: value.provider.clone(),
            system: value.system.clone(),
            tools: value
                .tools
                .as_ref()
                .map(|tools| tools.iter().map(ToolDescriptorResponse::from).collect()),
            plugins: value
                .plugins
                .as_ref()
                .map(|plugins| plugins.iter().map(PluginDescriptorResponse::from).collect()),
            settings: value.settings.as_ref().map(AgentSettingsResponse::from),
        }
    }
}

impl From<&AgentState> for AgentStateResponse {
    fn from(value: &AgentState) -> Self {
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            status: value.status.as_str().to_string(),
            config: AgentConfigResponse::from(&value.config),
            created_at_ms: value.created_at_ms,
            token_usage: TokenUsageResponse::from(&value.token_usage),
        }
    }
}

impl From<&AgentRuntimeSnapshot> for AgentRuntimeSnapshotResponse {
    fn from(value: &AgentRuntimeSnapshot) -> Self {
        Self {
            state: AgentStateResponse::from(&value.state),
            message_count: value.message_count,
            messages: value
                .messages
                .iter()
                .map(AgentMessageResponse::from)
                .collect(),
            event_count: value.event_count,
            last_task: value.last_task.as_ref().map(TaskResultResponse::from),
        }
    }
}

impl From<&Message> for AgentMessageResponse {
    fn from(value: &Message) -> Self {
        Self {
            id: value.id.clone(),
            agent_id: value.agent_id.clone(),
            room_id: value.room_id.clone(),
            content: ContentResponse::from(&value.content),
            role: message_role_to_str(value.role).to_string(),
            created_at_ms: value.created_at_ms,
        }
    }
}

fn message_role_to_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}
