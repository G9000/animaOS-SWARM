use std::collections::BTreeMap;

use anima_core::{
    AgentConfig, Content, DataValue, Message, MessageRole, ModelAdapter, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, TokenUsage, ToolCall, ToolDescriptor,
};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Map, Value};

use crate::model::DeterministicModelAdapter;

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434/v1";
const DEFAULT_GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
const DEFAULT_XAI_BASE_URL: &str = "https://api.x.ai/v1";
const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";
const DEFAULT_TOGETHER_BASE_URL: &str = "https://api.together.xyz/v1";
const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const DEFAULT_FIREWORKS_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";
const DEFAULT_PERPLEXITY_BASE_URL: &str = "https://api.perplexity.ai";
const DEFAULT_MOONSHOT_BASE_URL: &str = "https://api.moonshot.ai/v1";

const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Per-provider configuration: env-var key name, default base URL.
struct ProviderDef {
    api_key_envs: &'static [&'static str],
    base_url_envs: &'static [&'static str],
    default_base_url: &'static str,
}

const PROVIDER_DEFS: &[(&str, ProviderDef)] = &[
    (
        "openai",
        ProviderDef {
            api_key_envs: &["OPENAI_API_KEY", "OPENAI_KEY", "OPENAI_TOKEN"],
            base_url_envs: &["OPENAI_BASE_URL"],
            default_base_url: DEFAULT_OPENAI_BASE_URL,
        },
    ),
    (
        "anthropic",
        ProviderDef {
            api_key_envs: &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_KEY",
                "ANTHROPIC_TOKEN",
                "CLAUDE_API_KEY",
            ],
            base_url_envs: &["ANTHROPIC_BASE_URL"],
            default_base_url: DEFAULT_ANTHROPIC_BASE_URL,
        },
    ),
    (
        "google",
        ProviderDef {
            api_key_envs: &[
                "GOOGLE_API_KEY",
                "GOOGLE_KEY",
                "GOOGLE_AI_KEY",
                "GEMINI_API_KEY",
                "GOOGLE_GENERATIVE_AI_API_KEY",
            ],
            base_url_envs: &["GOOGLE_BASE_URL"],
            default_base_url: DEFAULT_GOOGLE_BASE_URL,
        },
    ),
    (
        "ollama",
        ProviderDef {
            api_key_envs: &["OLLAMA_API_KEY"],
            base_url_envs: &["OLLAMA_BASE_URL"],
            default_base_url: DEFAULT_OLLAMA_BASE_URL,
        },
    ),
    (
        "groq",
        ProviderDef {
            api_key_envs: &["GROQ_API_KEY", "GROQ_KEY", "GROQ_TOKEN"],
            base_url_envs: &["GROQ_BASE_URL"],
            default_base_url: DEFAULT_GROQ_BASE_URL,
        },
    ),
    (
        "xai",
        ProviderDef {
            api_key_envs: &["XAI_API_KEY", "XAI_KEY", "GROK_API_KEY"],
            base_url_envs: &["XAI_BASE_URL"],
            default_base_url: DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "openrouter",
        ProviderDef {
            api_key_envs: &["OPENROUTER_API_KEY", "OPENROUTER_KEY", "OPENROUTER_TOKEN"],
            base_url_envs: &["OPENROUTER_BASE_URL"],
            default_base_url: DEFAULT_OPENROUTER_BASE_URL,
        },
    ),
    (
        "mistral",
        ProviderDef {
            api_key_envs: &["MISTRAL_API_KEY", "MISTRAL_KEY", "MISTRAL_TOKEN"],
            base_url_envs: &["MISTRAL_BASE_URL"],
            default_base_url: DEFAULT_MISTRAL_BASE_URL,
        },
    ),
    (
        "together",
        ProviderDef {
            api_key_envs: &["TOGETHER_API_KEY", "TOGETHER_KEY", "TOGETHER_TOKEN"],
            base_url_envs: &["TOGETHER_BASE_URL"],
            default_base_url: DEFAULT_TOGETHER_BASE_URL,
        },
    ),
    (
        "deepseek",
        ProviderDef {
            api_key_envs: &["DEEPSEEK_API_KEY"],
            base_url_envs: &["DEEPSEEK_BASE_URL"],
            default_base_url: DEFAULT_DEEPSEEK_BASE_URL,
        },
    ),
    (
        "fireworks",
        ProviderDef {
            api_key_envs: &["FIREWORKS_API_KEY"],
            base_url_envs: &["FIREWORKS_BASE_URL"],
            default_base_url: DEFAULT_FIREWORKS_BASE_URL,
        },
    ),
    (
        "perplexity",
        ProviderDef {
            api_key_envs: &["PERPLEXITY_API_KEY"],
            base_url_envs: &["PERPLEXITY_BASE_URL"],
            default_base_url: DEFAULT_PERPLEXITY_BASE_URL,
        },
    ),
    (
        "moonshot",
        ProviderDef {
            api_key_envs: &[
                "MOONSHOT_API_KEY",
                "MOONSHOT_KEY",
                "MOONSHOT_TOKEN",
                "KIMI_API_KEY",
            ],
            base_url_envs: &["MOONSHOT_BASE_URL", "KIMI_BASE_URL"],
            default_base_url: DEFAULT_MOONSHOT_BASE_URL,
        },
    ),
];

fn provider_def(name: &str) -> Option<&'static ProviderDef> {
    PROVIDER_DEFS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, def)| def)
}

fn first_env_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|value| (!value.trim().is_empty()).then_some(value))
    })
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeModelAdapterConfig {
    /// Cached env vars per provider: (api_key, base_url).
    providers: Vec<(String, Option<String>, String)>,
}

impl RuntimeModelAdapterConfig {
    fn from_env() -> Self {
        let providers = PROVIDER_DEFS
            .iter()
            .map(|(name, def)| {
                (
                    (*name).to_string(),
                    first_env_value(def.api_key_envs),
                    first_env_value(def.base_url_envs)
                        .unwrap_or_else(|| def.default_base_url.to_string()),
                )
            })
            .collect();
        Self { providers }
    }

    fn api_key(&self, provider: &str) -> Option<&str> {
        self.providers
            .iter()
            .find(|(n, _, _)| n == provider)
            .and_then(|(_, k, _)| k.as_deref())
    }

    fn base_url(&self, provider: &str) -> &str {
        self.providers
            .iter()
            .find(|(n, _, _)| n == provider)
            .map(|(_, _, u)| u.as_str())
            .unwrap_or_else(|| {
                provider_def(provider)
                    .map(|d| d.default_base_url)
                    .unwrap_or(DEFAULT_OPENAI_BASE_URL)
            })
    }
}

#[derive(Clone)]
pub(crate) struct RuntimeModelAdapter {
    client: Client,
    config: RuntimeModelAdapterConfig,
}

impl RuntimeModelAdapter {
    pub(crate) fn from_env() -> Self {
        Self::with_config(RuntimeModelAdapterConfig::from_env())
    }

    fn with_config(config: RuntimeModelAdapterConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Resolve credentials for a named provider, preferring per-request
    /// settings over the daemon-level config.
    fn resolve_provider_creds(
        &self,
        provider_name: &str,
        agent_config: &AgentConfig,
    ) -> (Option<String>, String) {
        let fallback_key = self.config.api_key(provider_name);
        let fallback_url = self.config.base_url(provider_name);

        let api_key = resolved_api_key(agent_config, "apiKey", fallback_key);
        let base_url = resolved_base_url(agent_config, "baseUrl", fallback_url);
        (api_key, base_url)
    }

    /// Generate via an OpenAI-compatible provider (OpenAI, Ollama, Groq, xAI,
    /// OpenRouter, Mistral, Together, DeepSeek, Fireworks, Perplexity,
    /// Moonshot/Kimi, …).
    async fn generate_openai_compat_provider(
        &self,
        provider_name: &str,
        require_key: bool,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let (api_key, base_url) = self.resolve_provider_creds(provider_name, config);

        if require_key && api_key.is_none() {
            let def = provider_def(provider_name);
            let env_name = def
                .and_then(|d| d.api_key_envs.first().copied())
                .unwrap_or("API_KEY");
            return Err(format!(
                "{env_name} is not configured for daemon-backed {provider_name} models"
            ));
        }

        let display = capitalise(provider_name);
        self.generate_openai_compatible(
            &display,
            &join_base_url(&base_url, "/chat/completions"),
            api_key.as_deref(),
            config,
            request,
        )
        .await
    }

    /// Generate via Anthropic's native Messages API.
    async fn generate_anthropic(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let (api_key, base_url) = self.resolve_provider_creds("anthropic", config);
        let api_key = api_key.ok_or_else(|| {
            "ANTHROPIC_API_KEY is not configured for daemon-backed anthropic models".to_string()
        })?;

        let body = build_anthropic_body(config, request)?;
        let response = self
            .client
            .post(join_base_url(&base_url, "/v1/messages"))
            .header("content-type", "application/json")
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Anthropic request failed: {e}"))?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .map_err(|e| format!("Anthropic response read failed: {e}"))?;

        if !status.is_success() {
            return Err(format!(
                "Anthropic API error ({}): {}",
                status.as_u16(),
                payload_text
            ));
        }

        let payload: Value = serde_json::from_str(&payload_text)
            .map_err(|e| format!("Anthropic response parse failed: {e}"))?;

        parse_anthropic_response(&payload)
    }

    /// Generate via Google Gemini's native generateContent API.
    async fn generate_google(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let (api_key, base_url) = self.resolve_provider_creds("google", config);
        let api_key = api_key.ok_or_else(|| {
            "GOOGLE_API_KEY is not configured for daemon-backed google models".to_string()
        })?;

        let body = build_google_body(config, request)?;
        let endpoint = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            base_url.trim_end_matches('/'),
            config.model,
            api_key
        );

        let response = self
            .client
            .post(&endpoint)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Google request failed: {e}"))?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .map_err(|e| format!("Google response read failed: {e}"))?;

        if !status.is_success() {
            return Err(format!(
                "Google API error ({}): {}",
                status.as_u16(),
                payload_text
            ));
        }

        let payload: Value = serde_json::from_str(&payload_text)
            .map_err(|e| format!("Google response parse failed: {e}"))?;

        parse_google_response(&payload)
    }

    async fn generate_openai_compatible(
        &self,
        provider_name: &str,
        endpoint: &str,
        api_key: Option<&str>,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let body = build_openai_compatible_body(config, request)?;
        let mut request_builder = self
            .client
            .post(endpoint)
            .header("content-type", "application/json")
            .json(&body);

        if let Some(api_key) = api_key {
            request_builder = request_builder.bearer_auth(api_key);
        }

        let response = request_builder
            .send()
            .await
            .map_err(|error| format!("{provider_name} request failed: {error}"))?;
        let status = response.status();
        let payload_text = response
            .text()
            .await
            .map_err(|error| format!("{provider_name} response read failed: {error}"))?;

        if !status.is_success() {
            return Err(format!(
                "{provider_name} API error ({}): {}",
                status.as_u16(),
                payload_text
            ));
        }

        let payload: Value = serde_json::from_str(&payload_text)
            .map_err(|error| format!("{provider_name} response parse failed: {error}"))?;

        parse_openai_compatible_response(&payload, provider_name)
    }
}

#[async_trait]
impl ModelAdapter for RuntimeModelAdapter {
    fn provider(&self) -> &str {
        "runtime"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let provider = config
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("deterministic")
            .to_ascii_lowercase();

        match provider.as_str() {
            "deterministic" | "test" => DeterministicModelAdapter.generate(config, request).await,
            "openai" => {
                self.generate_openai_compat_provider("openai", true, config, request)
                    .await
            }
            "ollama" => {
                self.generate_openai_compat_provider("ollama", false, config, request)
                    .await
            }
            "anthropic" => self.generate_anthropic(config, request).await,
            "google" | "gemini" => self.generate_google(config, request).await,
            "groq" => {
                self.generate_openai_compat_provider("groq", true, config, request)
                    .await
            }
            "xai" | "grok" => {
                self.generate_openai_compat_provider("xai", true, config, request)
                    .await
            }
            "openrouter" => {
                self.generate_openai_compat_provider("openrouter", true, config, request)
                    .await
            }
            "mistral" => {
                self.generate_openai_compat_provider("mistral", true, config, request)
                    .await
            }
            "together" => {
                self.generate_openai_compat_provider("together", true, config, request)
                    .await
            }
            "deepseek" => {
                self.generate_openai_compat_provider("deepseek", true, config, request)
                    .await
            }
            "fireworks" => {
                self.generate_openai_compat_provider("fireworks", true, config, request)
                    .await
            }
            "perplexity" => {
                self.generate_openai_compat_provider("perplexity", true, config, request)
                    .await
            }
            "moonshot" | "kimi" => {
                self.generate_openai_compat_provider("moonshot", true, config, request)
                    .await
            }
            other => Err(format!(
                "unsupported model provider for daemon-backed runtime: {other}"
            )),
        }
    }
}

fn build_openai_compatible_body(
    config: &AgentConfig,
    request: &ModelGenerateRequest,
) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(config.model.clone()));
    body.insert(
        "messages".into(),
        Value::Array(build_openai_compatible_messages(request)?),
    );

    if let Some(tools) = build_openai_compatible_tools(config)? {
        body.insert("tools".into(), Value::Array(tools));
    }

    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
    }

    if let Some(max_tokens) = request.max_tokens {
        body.insert("max_tokens".into(), json!(max_tokens));
    }

    Ok(Value::Object(body))
}

fn resolved_api_key(config: &AgentConfig, key: &str, fallback: Option<&str>) -> Option<String> {
    agent_setting_string(config, key).or_else(|| fallback.map(ToString::to_string))
}

fn resolved_base_url(config: &AgentConfig, key: &str, fallback: &str) -> String {
    agent_setting_string(config, key).unwrap_or_else(|| fallback.to_string())
}

fn agent_setting_string(config: &AgentConfig, key: &str) -> Option<String> {
    config
        .settings
        .as_ref()
        .and_then(|settings| settings.additional.get(key))
        .and_then(|value| match value {
            DataValue::String(value) if !value.trim().is_empty() => Some(value.clone()),
            _ => None,
        })
}

fn build_openai_compatible_messages(request: &ModelGenerateRequest) -> Result<Vec<Value>, String> {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system,
    })];

    for message in &request.messages {
        match message.role {
            MessageRole::System => messages.push(json!({
                "role": "system",
                "content": message.content.text,
            })),
            MessageRole::User => messages.push(json!({
                "role": "user",
                "content": message.content.text,
            })),
            MessageRole::Assistant => {
                let tool_calls = assistant_tool_calls_json(message)?;
                let content = if message.content.text.is_empty() {
                    Value::Null
                } else {
                    Value::String(message.content.text.clone())
                };

                let mut assistant_message = Map::new();
                assistant_message.insert("role".into(), Value::String("assistant".into()));
                assistant_message.insert("content".into(), content);
                if let Some(tool_calls) = tool_calls {
                    assistant_message.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                messages.push(Value::Object(assistant_message));
            }
            MessageRole::Tool => messages.push(json!({
                "role": "tool",
                "tool_call_id": tool_call_id(message),
                "content": message.content.text,
            })),
        }
    }

    Ok(messages)
}

fn build_openai_compatible_tools(config: &AgentConfig) -> Result<Option<Vec<Value>>, String> {
    let Some(tools) = config.tools.as_ref().filter(|tools| !tools.is_empty()) else {
        return Ok(None);
    };

    tools
        .iter()
        .map(tool_descriptor_json)
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn tool_descriptor_json(tool: &ToolDescriptor) -> Result<Value, String> {
    Ok(json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool_parameters_schema_json(tool),
        }
    }))
}

fn tool_parameters_schema_json(tool: &ToolDescriptor) -> Value {
    if tool_parameters_are_json_schema(&tool.parameters) {
        data_value_to_json(&DataValue::Object(tool.parameters.clone()))
    } else {
        json!({
            "type": "object",
            "properties": data_value_to_json(&DataValue::Object(tool.parameters.clone())),
        })
    }
}

fn tool_parameters_are_json_schema(parameters: &BTreeMap<String, DataValue>) -> bool {
    matches!(parameters.get("type"), Some(DataValue::String(_)))
        || parameters.contains_key("properties")
        || parameters.contains_key("required")
        || parameters.contains_key("items")
        || parameters.contains_key("oneOf")
        || parameters.contains_key("anyOf")
        || parameters.contains_key("allOf")
        || parameters.contains_key("additionalProperties")
}

fn assistant_tool_calls_json(message: &Message) -> Result<Option<Vec<Value>>, String> {
    let Some(metadata) = message.content.metadata.as_ref() else {
        return Ok(None);
    };
    let Some(value) = metadata.get("toolCalls") else {
        return Ok(None);
    };
    let DataValue::Array(tool_calls) = value else {
        return Err("assistant toolCalls metadata must be an array".to_string());
    };

    tool_calls
        .iter()
        .map(|tool_call| {
            let DataValue::Object(tool_call) = tool_call else {
                return Err("assistant toolCall metadata entries must be objects".to_string());
            };

            let id = required_data_string(tool_call, "id")?;
            let name = required_data_string(tool_call, "name")?;
            let args = match tool_call.get("args") {
                Some(DataValue::Object(args)) => args.clone(),
                Some(_) => return Err("assistant toolCall args must be an object".to_string()),
                None => BTreeMap::new(),
            };

            Ok(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": data_value_to_json_string(&DataValue::Object(args))?,
                }
            }))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn required_data_string(object: &BTreeMap<String, DataValue>, key: &str) -> Result<String, String> {
    match object.get(key) {
        Some(DataValue::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(format!("missing required string field: {key}")),
    }
}

fn tool_call_id(message: &Message) -> String {
    message
        .content
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("toolCallId"))
        .and_then(|value| match value {
            DataValue::String(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_else(|| message.id.clone())
}

fn parse_openai_compatible_response(
    payload: &Value,
    provider_name: &str,
) -> Result<ModelGenerateResponse, String> {
    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| format!("{provider_name} response did not include a choice"))?;

    let message = choice
        .get("message")
        .ok_or_else(|| format!("{provider_name} response did not include a message"))?;

    let tool_calls = parse_openai_compatible_tool_calls(message.get("tool_calls"))?;
    let stop_reason = if tool_calls.as_ref().is_some_and(|calls| !calls.is_empty()) {
        ModelStopReason::ToolCall
    } else if choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .is_some_and(|reason| reason == "length")
    {
        ModelStopReason::MaxTokens
    } else {
        ModelStopReason::End
    };

    Ok(ModelGenerateResponse {
        content: Content {
            text: response_content_text(message.get("content")),
            attachments: None,
            metadata: None,
        },
        tool_calls,
        usage: response_usage(payload.get("usage")),
        stop_reason,
    })
}

fn parse_openai_compatible_tool_calls(
    value: Option<&Value>,
) -> Result<Option<Vec<ToolCall>>, String> {
    let Some(Value::Array(tool_calls)) = value else {
        return Ok(None);
    };

    tool_calls
        .iter()
        .map(|tool_call| {
            let id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "provider tool call is missing an id".to_string())?;
            let function = tool_call
                .get("function")
                .ok_or_else(|| "provider tool call is missing a function payload".to_string())?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "provider tool call is missing a function name".to_string())?;
            let args =
                tool_call_args(function.get("arguments").ok_or_else(|| {
                    "provider tool call is missing function arguments".to_string()
                })?)?;

            Ok(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                args,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|tool_calls| {
            if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            }
        })
}

fn tool_call_args(value: &Value) -> Result<BTreeMap<String, DataValue>, String> {
    match value {
        Value::String(arguments) => {
            let parsed: Value = serde_json::from_str(arguments)
                .map_err(|error| format!("failed to parse tool call arguments: {error}"))?;
            json_value_to_data_map(&parsed)
        }
        Value::Object(_) => json_value_to_data_map(value),
        _ => Err("tool call arguments must be a JSON object or stringified JSON object".into()),
    }
}

fn json_value_to_data_map(value: &Value) -> Result<BTreeMap<String, DataValue>, String> {
    match value {
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), json_value_to_data_value(value)?)))
            .collect(),
        _ => Err("expected a JSON object".to_string()),
    }
}

fn json_value_to_data_value(value: &Value) -> Result<DataValue, String> {
    Ok(match value {
        Value::Null => DataValue::Null,
        Value::Bool(value) => DataValue::Bool(*value),
        Value::Number(value) => DataValue::Number(
            value
                .as_f64()
                .ok_or_else(|| "expected a finite JSON number".to_string())?,
        ),
        Value::String(value) => DataValue::String(value.clone()),
        Value::Array(values) => DataValue::Array(
            values
                .iter()
                .map(json_value_to_data_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Object(values) => DataValue::Object(
            values
                .iter()
                .map(|(key, value)| -> Result<(String, DataValue), String> {
                    Ok((key.clone(), json_value_to_data_value(value)?))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?,
        ),
    })
}

fn data_value_to_json(value: &DataValue) -> Value {
    match value {
        DataValue::Null => Value::Null,
        DataValue::Bool(value) => Value::Bool(*value),
        DataValue::Number(value) => json!(value),
        DataValue::String(value) => Value::String(value.clone()),
        DataValue::Array(values) => Value::Array(values.iter().map(data_value_to_json).collect()),
        DataValue::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
        ),
    }
}

fn data_value_to_json_string(value: &DataValue) -> Result<String, String> {
    serde_json::to_string(&data_value_to_json(value))
        .map_err(|error| format!("failed to serialize tool call arguments: {error}"))
}

fn response_content_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                Value::Object(value) => value
                    .get("text")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn response_usage(value: Option<&Value>) -> TokenUsage {
    let Some(Value::Object(usage)) = value else {
        return TokenUsage::default();
    };

    TokenUsage {
        prompt_tokens: value_to_u64(usage.get("prompt_tokens")),
        completion_tokens: value_to_u64(usage.get("completion_tokens")),
        total_tokens: value_to_u64(usage.get("total_tokens")),
    }
}

fn value_to_u64(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(value)) => value
            .as_u64()
            .unwrap_or_else(|| value.as_f64().unwrap_or(0.0) as u64),
        _ => 0,
    }
}

fn join_base_url(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

// ── Anthropic body / response helpers ──────────────────────────────────────────

fn build_anthropic_body(
    config: &AgentConfig,
    request: &ModelGenerateRequest,
) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(config.model.clone()));

    // system is a top-level string, not a message
    body.insert("system".into(), Value::String(request.system.clone()));

    // messages
    let messages = build_anthropic_messages(request)?;
    body.insert("messages".into(), Value::Array(messages));

    // tools
    if let Some(tools) = build_anthropic_tools(config)? {
        body.insert("tools".into(), Value::Array(tools));
    }

    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    body.insert(
        "max_tokens".into(),
        json!(request.max_tokens.unwrap_or(4096)),
    );

    Ok(Value::Object(body))
}

fn build_anthropic_messages(request: &ModelGenerateRequest) -> Result<Vec<Value>, String> {
    let mut messages: Vec<Value> = Vec::new();

    for message in &request.messages {
        match message.role {
            MessageRole::System => {
                // Anthropic doesn't support system role in messages — merge into
                // the previous user turn or create one.
                if let Some(Value::Object(last)) = messages.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("user") {
                        if let Some(Value::String(content)) = last.get_mut("content") {
                            content.push('\n');
                            content.push_str(&message.content.text);
                            continue;
                        }
                    }
                }
                messages.push(json!({
                    "role": "user",
                    "content": message.content.text,
                }));
            }
            MessageRole::User => messages.push(json!({
                "role": "user",
                "content": message.content.text,
            })),
            MessageRole::Assistant => {
                let tool_use = anthropic_assistant_tool_use(message)?;
                let mut content_blocks: Vec<Value> = Vec::new();

                if !message.content.text.is_empty() {
                    content_blocks.push(json!({
                        "type": "text",
                        "text": message.content.text,
                    }));
                }
                content_blocks.extend(tool_use.into_iter());

                messages.push(json!({
                    "role": "assistant",
                    "content": content_blocks,
                }));
            }
            MessageRole::Tool => {
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id(message),
                        "content": message.content.text,
                    }],
                }));
            }
        }
    }
    Ok(messages)
}

fn anthropic_assistant_tool_use(message: &Message) -> Result<Vec<Value>, String> {
    let Some(metadata) = message.content.metadata.as_ref() else {
        return Ok(vec![]);
    };
    let Some(DataValue::Array(tool_calls)) = metadata.get("toolCalls") else {
        return Ok(vec![]);
    };

    tool_calls
        .iter()
        .map(|tc| {
            let DataValue::Object(tc) = tc else {
                return Err("assistant toolCall entries must be objects".to_string());
            };
            let id = required_data_string(tc, "id")?;
            let name = required_data_string(tc, "name")?;
            let input = match tc.get("args") {
                Some(DataValue::Object(args)) => {
                    data_value_to_json(&DataValue::Object(args.clone()))
                }
                Some(_) => return Err("assistant toolCall args must be an object".to_string()),
                None => json!({}),
            };
            Ok(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }))
        })
        .collect()
}

fn build_anthropic_tools(config: &AgentConfig) -> Result<Option<Vec<Value>>, String> {
    let Some(tools) = config.tools.as_ref().filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    tools
        .iter()
        .map(|tool| {
            Ok(json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool_parameters_schema_json(tool),
            }))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn parse_anthropic_response(payload: &Value) -> Result<ModelGenerateResponse, String> {
    let content_blocks = payload
        .get("content")
        .and_then(Value::as_array)
        .ok_or("Anthropic response missing content array")?;

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(t.to_string());
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let empty_obj = Value::Object(Map::new());
                let input = block.get("input").unwrap_or(&empty_obj);
                let args = json_value_to_data_map(input)?;
                tool_calls.push(ToolCall { id, name, args });
            }
            _ => {}
        }
    }

    let stop_reason = if !tool_calls.is_empty() {
        ModelStopReason::ToolCall
    } else if payload.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
        ModelStopReason::MaxTokens
    } else {
        ModelStopReason::End
    };

    let usage = if let Some(u) = payload.get("usage") {
        TokenUsage {
            prompt_tokens: value_to_u64(u.get("input_tokens")),
            completion_tokens: value_to_u64(u.get("output_tokens")),
            total_tokens: value_to_u64(u.get("input_tokens"))
                + value_to_u64(u.get("output_tokens")),
        }
    } else {
        TokenUsage::default()
    };

    Ok(ModelGenerateResponse {
        content: Content {
            text: text_parts.join(""),
            attachments: None,
            metadata: None,
        },
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        usage,
        stop_reason,
    })
}

// ── Google Gemini body / response helpers ──────────────────────────────────────

fn build_google_body(
    config: &AgentConfig,
    request: &ModelGenerateRequest,
) -> Result<Value, String> {
    let mut body = Map::new();

    // system_instruction
    body.insert(
        "system_instruction".into(),
        json!({ "parts": [{ "text": request.system }] }),
    );

    // contents
    let contents = build_google_contents(request)?;
    body.insert("contents".into(), Value::Array(contents));

    // tools
    if let Some(tools) = build_google_tools(config)? {
        body.insert("tools".into(), json!([{ "function_declarations": tools }]));
    }

    // generationConfig
    let mut gen_config = Map::new();
    if let Some(temperature) = request.temperature {
        gen_config.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = request.max_tokens {
        gen_config.insert("maxOutputTokens".into(), json!(max_tokens));
    }
    if !gen_config.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gen_config));
    }

    Ok(Value::Object(body))
}

fn build_google_contents(request: &ModelGenerateRequest) -> Result<Vec<Value>, String> {
    let mut contents: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message.role {
            MessageRole::System => {
                // Merge system into user or skip (already in system_instruction).
            }
            MessageRole::User => contents.push(json!({
                "role": "user",
                "parts": [{ "text": message.content.text }],
            })),
            MessageRole::Assistant => {
                let mut parts: Vec<Value> = Vec::new();
                if !message.content.text.is_empty() {
                    parts.push(json!({ "text": message.content.text }));
                }
                parts.extend(google_function_call_parts(message)?);
                contents.push(json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
            MessageRole::Tool => {
                let call_id = tool_call_id(message);
                // Parse tool result text as JSON if possible, else wrap in a string.
                let response_value: Value = serde_json::from_str(&message.content.text)
                    .unwrap_or_else(|_| json!({ "result": message.content.text }));
                contents.push(json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "name": call_id,
                            "response": response_value,
                        }
                    }],
                }));
            }
        }
    }
    Ok(contents)
}

fn google_function_call_parts(message: &Message) -> Result<Vec<Value>, String> {
    let Some(metadata) = message.content.metadata.as_ref() else {
        return Ok(vec![]);
    };
    let Some(DataValue::Array(tool_calls)) = metadata.get("toolCalls") else {
        return Ok(vec![]);
    };
    tool_calls
        .iter()
        .map(|tc| {
            let DataValue::Object(tc) = tc else {
                return Err("toolCall entries must be objects".to_string());
            };
            let name = required_data_string(tc, "name")?;
            let args = match tc.get("args") {
                Some(DataValue::Object(a)) => data_value_to_json(&DataValue::Object(a.clone())),
                _ => json!({}),
            };
            Ok(json!({
                "functionCall": {
                    "name": name,
                    "args": args,
                }
            }))
        })
        .collect()
}

fn build_google_tools(config: &AgentConfig) -> Result<Option<Vec<Value>>, String> {
    let Some(tools) = config.tools.as_ref().filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    tools
        .iter()
        .map(|tool| {
            Ok(json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool_parameters_schema_json(tool),
            }))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn parse_google_response(payload: &Value) -> Result<ModelGenerateResponse, String> {
    let candidate = payload
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or("Google response missing candidates")?;

    let parts = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array);

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(parts) = parts {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                text_parts.push(text.to_string());
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let empty_obj = Value::Object(Map::new());
                let args_value = fc.get("args").unwrap_or(&empty_obj);
                let args = json_value_to_data_map(args_value)?;
                // Gemini doesn't use call IDs; synthesize one.
                let id = format!("call_{name}");
                tool_calls.push(ToolCall { id, name, args });
            }
        }
    }

    let stop_reason = if !tool_calls.is_empty() {
        ModelStopReason::ToolCall
    } else if candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .is_some_and(|r| r == "MAX_TOKENS")
    {
        ModelStopReason::MaxTokens
    } else {
        ModelStopReason::End
    };

    let usage = if let Some(u) = payload.get("usageMetadata") {
        TokenUsage {
            prompt_tokens: value_to_u64(u.get("promptTokenCount")),
            completion_tokens: value_to_u64(u.get("candidatesTokenCount")),
            total_tokens: value_to_u64(u.get("totalTokenCount")),
        }
    } else {
        TokenUsage::default()
    };

    Ok(ModelGenerateResponse {
        content: Content {
            text: text_parts.join(""),
            attachments: None,
            metadata: None,
        },
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        usage,
        stop_reason,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use anima_core::{
        AgentConfig, Content, DataValue, Message, MessageRole, ModelAdapter, ModelGenerateRequest,
        ModelStopReason, ToolDescriptor,
    };
    use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
    use serde_json::{json, Value};
    use tokio::net::TcpListener;

    use super::{
        data_value_to_json, tool_parameters_schema_json, RuntimeModelAdapter,
        RuntimeModelAdapterConfig, PROVIDER_DEFS,
    };

    /// Build a test config with overrides for a specific provider.
    fn test_config(overrides: &[(&str, Option<&str>, &str)]) -> RuntimeModelAdapterConfig {
        let providers = PROVIDER_DEFS
            .iter()
            .map(|(name, def)| {
                if let Some((_, key, url)) = overrides.iter().find(|(n, _, _)| n == name) {
                    (
                        name.to_string(),
                        key.map(|k| k.to_string()),
                        url.to_string(),
                    )
                } else {
                    (name.to_string(), None, def.default_base_url.to_string())
                }
            })
            .collect();
        RuntimeModelAdapterConfig { providers }
    }

    #[tokio::test]
    async fn runtime_adapter_routes_openai_requests_through_real_http_adapter() {
        let seen_auth = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().route(
            "/v1/chat/completions",
            post({
                let seen_auth = Arc::clone(&seen_auth);
                move |headers: HeaderMap, State(()): State<()>, Json(body): Json<Value>| {
                    let seen_auth = Arc::clone(&seen_auth);
                    async move {
                        seen_auth
                            .lock()
                            .expect("auth mutex should not be poisoned")
                            .push(
                                headers
                                    .get("authorization")
                                    .and_then(|value| value.to_str().ok())
                                    .unwrap_or_default()
                                    .to_string(),
                            );

                        assert!(
                            body.get("tools").is_some(),
                            "openai body should include tools: {body}"
                        );

                        Json(json!({
                            "choices": [
                                {
                                    "message": {
                                        "content": null,
                                        "tool_calls": [
                                            {
                                                "id": "call-1",
                                                "type": "function",
                                                "function": {
                                                    "name": "delegate_task",
                                                    "arguments": "{\"worker_name\":\"research_agent\",\"task\":\"research three angles\"}"
                                                }
                                            }
                                        ]
                                    },
                                    "finish_reason": "tool_calls"
                                }
                            ],
                            "usage": {
                                "prompt_tokens": 8,
                                "completion_tokens": 4,
                                "total_tokens": 12
                            }
                        }))
                    }
                }
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[(
            "openai",
            Some("test-key"),
            &format!("{base_url}/v1"),
        )]));

        let response = adapter
            .generate(&agent_config("openai"), &request())
            .await
            .expect("openai adapter should generate");

        let tool_calls = response.tool_calls.expect("tool calls should be present");
        assert_eq!(response.stop_reason, ModelStopReason::ToolCall);
        assert_eq!(response.usage.total_tokens, 12);
        assert_eq!(tool_calls[0].name, "delegate_task");
        assert_eq!(
            tool_calls[0].args.get("worker_name"),
            Some(&DataValue::String("research_agent".into()))
        );
        assert_eq!(
            seen_auth
                .lock()
                .expect("auth mutex should not be poisoned")
                .as_slice(),
            ["Bearer test-key"]
        );
    }

    #[tokio::test]
    async fn runtime_adapter_routes_ollama_requests_through_real_http_adapter() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|Json(_body): Json<Value>| async move {
                Json(json!({
                    "choices": [
                        {
                            "message": {
                                "content": "researched and drafted the campaign"
                            },
                            "finish_reason": "stop"
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 5,
                        "completion_tokens": 7,
                        "total_tokens": 12
                    }
                }))
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[(
            "ollama",
            None,
            &format!("{base_url}/v1"),
        )]));

        let response = adapter
            .generate(&agent_config("ollama"), &request())
            .await
            .expect("ollama adapter should generate");

        assert_eq!(response.stop_reason, ModelStopReason::End);
        assert_eq!(response.content.text, "researched and drafted the campaign");
        assert_eq!(response.usage.total_tokens, 12);
    }

    #[tokio::test]
    async fn runtime_adapter_errors_when_openai_key_is_missing() {
        let adapter = RuntimeModelAdapter::with_config(test_config(&[]));

        let error = adapter
            .generate(&agent_config("openai"), &request())
            .await
            .expect_err("missing key should fail");

        assert!(
            error.contains("OPENAI_API_KEY"),
            "error should explain missing openai key: {error}"
        );
    }

    #[tokio::test]
    async fn runtime_adapter_prefers_request_settings_for_openai_credentials() {
        let seen_auth = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().route(
            "/v1/chat/completions",
            post({
                let seen_auth = Arc::clone(&seen_auth);
                move |headers: HeaderMap, Json(_body): Json<Value>| {
                    let seen_auth = Arc::clone(&seen_auth);
                    async move {
                        seen_auth
                            .lock()
                            .expect("auth mutex should not be poisoned")
                            .push(
                                headers
                                    .get("authorization")
                                    .and_then(|value| value.to_str().ok())
                                    .unwrap_or_default()
                                    .to_string(),
                            );

                        Json(json!({
                            "choices": [
                                {
                                    "message": {
                                        "content": "campaign plan ready"
                                    },
                                    "finish_reason": "stop"
                                }
                            ],
                            "usage": {
                                "prompt_tokens": 6,
                                "completion_tokens": 6,
                                "total_tokens": 12
                            }
                        }))
                    }
                }
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[]));

        let mut config = agent_config("openai");
        let mut settings = anima_core::AgentSettings::default();
        settings
            .additional
            .insert("apiKey".into(), DataValue::String("request-key".into()));
        settings.additional.insert(
            "baseUrl".into(),
            DataValue::String(format!("{base_url}/v1")),
        );
        config.settings = Some(settings);

        let response = adapter
            .generate(&config, &request())
            .await
            .expect("request-scoped credentials should generate");

        assert_eq!(response.content.text, "campaign plan ready");
        assert_eq!(
            seen_auth
                .lock()
                .expect("auth mutex should not be poisoned")
                .as_slice(),
            ["Bearer request-key"]
        );
    }

    fn agent_config(provider: &str) -> AgentConfig {
        let mut delegate_parameters = BTreeMap::new();
        delegate_parameters.insert("type".into(), DataValue::String("object".into()));

        AgentConfig {
            name: "orchestrator_agent".into(),
            model: "gpt-4o-mini".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: Some(provider.into()),
            system: Some("You coordinate a team.".into()),
            tools: Some(vec![ToolDescriptor {
                name: "delegate_task".into(),
                description: "Delegate a subtask to a worker agent".into(),
                parameters: delegate_parameters,
                examples: None,
            }]),
            plugins: None,
            settings: None,
        }
    }

    fn request() -> ModelGenerateRequest {
        ModelGenerateRequest {
            system: "You are a helpful assistant".into(),
            messages: vec![Message {
                id: "msg-1".into(),
                agent_id: "agent-1".into(),
                room_id: "room-1".into(),
                content: Content {
                    text: "prepare a campaign".into(),
                    attachments: None,
                    metadata: None,
                },
                role: MessageRole::User,
                created_at: 1,
            }],
            temperature: Some(0.2),
            max_tokens: Some(512),
        }
    }

    async fn spawn_server(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let addr = listener
            .local_addr()
            .expect("test listener should have an address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should serve successfully");
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn runtime_adapter_routes_anthropic_through_native_messages_api() {
        let seen_headers = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let app = Router::new().route(
            "/v1/messages",
            post({
                let seen_headers = Arc::clone(&seen_headers);
                move |headers: HeaderMap, Json(body): Json<Value>| {
                    let seen_headers = Arc::clone(&seen_headers);
                    async move {
                        seen_headers.lock().unwrap().push((
                            headers
                                .get("x-api-key")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                            headers
                                .get("anthropic-version")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        ));

                        assert!(
                            body.get("system").is_some(),
                            "anthropic body should have top-level system"
                        );
                        assert!(
                            body.get("tools").is_some(),
                            "anthropic body should include tools"
                        );

                        Json(json!({
                            "content": [
                                {
                                    "type": "tool_use",
                                    "id": "toolu_1",
                                    "name": "delegate_task",
                                    "input": {
                                        "worker_name": "research_agent",
                                        "task": "research three angles"
                                    }
                                }
                            ],
                            "stop_reason": "tool_use",
                            "usage": {
                                "input_tokens": 10,
                                "output_tokens": 5
                            }
                        }))
                    }
                }
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[(
            "anthropic",
            Some("anth-key"),
            &base_url,
        )]));

        let response = adapter
            .generate(&agent_config("anthropic"), &request())
            .await
            .expect("anthropic adapter should generate");

        let tool_calls = response.tool_calls.expect("tool calls should be present");
        assert_eq!(response.stop_reason, ModelStopReason::ToolCall);
        assert_eq!(tool_calls[0].name, "delegate_task");
        assert_eq!(response.usage.prompt_tokens, 10);
        assert_eq!(response.usage.completion_tokens, 5);

        let headers = seen_headers.lock().unwrap();
        assert_eq!(headers[0].0, "anth-key");
        assert_eq!(headers[0].1, "2023-06-01");
    }

    #[tokio::test]
    async fn runtime_adapter_routes_google_through_native_gemini_api() {
        let app = Router::new().route(
            "/v1beta/models/gemini-2.0-flash:generateContent",
            post(|Json(body): Json<Value>| async move {
                assert!(
                    body.get("system_instruction").is_some(),
                    "google body should have system_instruction"
                );

                Json(json!({
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [{
                                "functionCall": {
                                    "name": "delegate_task",
                                    "args": {
                                        "worker_name": "research_agent",
                                        "task": "research three angles"
                                    }
                                }
                            }]
                        },
                        "finishReason": "STOP"
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 12,
                        "candidatesTokenCount": 8,
                        "totalTokenCount": 20
                    }
                }))
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[(
            "google",
            Some("goog-key"),
            &base_url,
        )]));

        let mut config = agent_config("google");
        config.model = "gemini-2.0-flash".into();

        let response = adapter
            .generate(&config, &request())
            .await
            .expect("google adapter should generate");

        let tool_calls = response.tool_calls.expect("tool calls should be present");
        assert_eq!(response.stop_reason, ModelStopReason::ToolCall);
        assert_eq!(tool_calls[0].name, "delegate_task");
        assert_eq!(response.usage.total_tokens, 20);
    }

    #[tokio::test]
    async fn runtime_adapter_routes_groq_through_openai_compatible() {
        let app = Router::new().route(
            "/openai/v1/chat/completions",
            post(|headers: HeaderMap, Json(_body): Json<Value>| async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(auth, "Bearer groq-key");
                Json(json!({
                    "choices": [{
                        "message": { "content": "groq response" },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8 }
                }))
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[(
            "groq",
            Some("groq-key"),
            &format!("{base_url}/openai/v1"),
        )]));

        let response = adapter
            .generate(&agent_config("groq"), &request())
            .await
            .expect("groq adapter should generate");

        assert_eq!(response.content.text, "groq response");
    }

    #[tokio::test]
    async fn runtime_adapter_routes_moonshot_through_openai_compatible() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(auth, "Bearer moonshot-key");
                assert_eq!(body.get("model"), Some(&json!("kimi-k2.5")));
                Json(json!({
                    "choices": [{
                        "message": { "content": "moonshot response" },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 6, "completion_tokens": 4, "total_tokens": 10 }
                }))
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[(
            "moonshot",
            Some("moonshot-key"),
            &format!("{base_url}/v1"),
        )]));

        let mut config = agent_config("moonshot");
        config.model = "kimi-k2.5".into();
        let response = adapter
            .generate(&config, &request())
            .await
            .expect("moonshot adapter should generate");

        assert_eq!(response.content.text, "moonshot response");
        assert_eq!(response.usage.total_tokens, 10);
    }

    #[tokio::test]
    async fn runtime_adapter_accepts_kimi_as_moonshot_alias() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|Json(_body): Json<Value>| async move {
                Json(json!({
                    "choices": [{
                        "message": { "content": "kimi alias response" },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5 }
                }))
            }),
        );

        let base_url = spawn_server(app).await;
        let adapter = RuntimeModelAdapter::with_config(test_config(&[(
            "moonshot",
            Some("moonshot-key"),
            &format!("{base_url}/v1"),
        )]));

        let mut config = agent_config("kimi");
        config.model = "kimi-k2.5".into();
        let response = adapter
            .generate(&config, &request())
            .await
            .expect("kimi alias should generate through moonshot");

        assert_eq!(response.content.text, "kimi alias response");
    }

    #[test]
    fn tool_parameters_schema_wraps_property_maps_into_json_schema_objects() {
        let mut properties = BTreeMap::new();
        properties.insert(
            "query".into(),
            DataValue::Object(BTreeMap::from([(
                "type".into(),
                DataValue::String("string".into()),
            )])),
        );
        properties.insert(
            "type".into(),
            DataValue::Object(BTreeMap::from([(
                "type".into(),
                DataValue::String("string".into()),
            )])),
        );

        let schema = tool_parameters_schema_json(&ToolDescriptor {
            name: "memory_search".into(),
            description: "Search memories".into(),
            parameters: properties,
            examples: None,
        });

        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "type": { "type": "string" }
                }
            })
        );
    }

    #[test]
    fn tool_parameters_schema_preserves_full_json_schema_objects() {
        let mut schema = BTreeMap::new();
        schema.insert("type".into(), DataValue::String("object".into()));
        schema.insert(
            "properties".into(),
            DataValue::Object(BTreeMap::from([(
                "query".into(),
                DataValue::Object(BTreeMap::from([(
                    "type".into(),
                    DataValue::String("string".into()),
                )])),
            )])),
        );

        let normalized = tool_parameters_schema_json(&ToolDescriptor {
            name: "delegate_task".into(),
            description: "Delegate work".into(),
            parameters: schema.clone(),
            examples: None,
        });

        assert_eq!(normalized, data_value_to_json(&DataValue::Object(schema)));
    }
}
