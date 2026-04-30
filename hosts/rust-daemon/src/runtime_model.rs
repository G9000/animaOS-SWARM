mod anthropic;
mod common;
mod google;
mod openai_compatible;

#[cfg(test)]
mod tests;

use anima_core::{
    AgentConfig, DataValue, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse,
};
use async_trait::async_trait;
use reqwest::Client;

use crate::model::DeterministicModelAdapter;
use self::{
    anthropic::{build_anthropic_body, parse_anthropic_response},
    google::{build_google_body, parse_google_response},
    openai_compatible::{build_openai_compatible_body, parse_openai_compatible_response},
};

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
        .find(|(provider_name, _)| *provider_name == name)
        .map(|(_, def)| def)
}

fn openai_compatible_provider(provider: &str) -> Option<(&'static str, bool)> {
    match provider {
        "openai" => Some(("openai", true)),
        "ollama" => Some(("ollama", false)),
        "groq" => Some(("groq", true)),
        "xai" | "grok" => Some(("xai", true)),
        "openrouter" => Some(("openrouter", true)),
        "mistral" => Some(("mistral", true)),
        "together" => Some(("together", true)),
        "deepseek" => Some(("deepseek", true)),
        "fireworks" => Some(("fireworks", true)),
        "perplexity" => Some(("perplexity", true)),
        "moonshot" | "kimi" => Some(("moonshot", true)),
        _ => None,
    }
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
            .find(|(name, _, _)| name == provider)
            .and_then(|(_, key, _)| key.as_deref())
    }

    fn base_url(&self, provider: &str) -> &str {
        self.providers
            .iter()
            .find(|(name, _, _)| name == provider)
            .map(|(_, _, url)| url.as_str())
            .unwrap_or_else(|| {
                provider_def(provider)
                    .map(|def| def.default_base_url)
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

    async fn generate_openai_compat_provider(
        &self,
        provider_name: &str,
        require_key: bool,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let (api_key, base_url) = self.resolve_provider_creds(provider_name, config);

        if require_key && api_key.is_none() {
            let env_name = provider_def(provider_name)
                .and_then(|def| def.api_key_envs.first().copied())
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
            .map_err(|error| format!("Anthropic request failed: {error}"))?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .map_err(|error| format!("Anthropic response read failed: {error}"))?;

        if !status.is_success() {
            return Err(format!(
                "Anthropic API error ({}): {}",
                status.as_u16(),
                payload_text
            ));
        }

        let payload: serde_json::Value = serde_json::from_str(&payload_text)
            .map_err(|error| format!("Anthropic response parse failed: {error}"))?;

        parse_anthropic_response(&payload)
    }

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
            .map_err(|error| format!("Google request failed: {error}"))?;

        let status = response.status();
        let payload_text = response
            .text()
            .await
            .map_err(|error| format!("Google response read failed: {error}"))?;

        if !status.is_success() {
            return Err(format!(
                "Google API error ({}): {}",
                status.as_u16(),
                payload_text
            ));
        }

        let payload: serde_json::Value = serde_json::from_str(&payload_text)
            .map_err(|error| format!("Google response parse failed: {error}"))?;

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

        let payload: serde_json::Value = serde_json::from_str(&payload_text)
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
            "anthropic" => self.generate_anthropic(config, request).await,
            "google" | "gemini" => self.generate_google(config, request).await,
            other => {
                if let Some((provider_name, require_key)) = openai_compatible_provider(other) {
                    self.generate_openai_compat_provider(provider_name, require_key, config, request)
                        .await
                } else {
                    Err(format!(
                        "unsupported model provider for daemon-backed runtime: {other}"
                    ))
                }
            }
        }
    }
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

fn join_base_url(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn capitalise(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}
