use anima_core::{AgentConfig, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse};
use async_trait::async_trait;
use reqwest::Client;

use crate::anthropic::{build_anthropic_body, parse_anthropic_response};
use crate::catalog::{resolve_provider, ProviderKind};
use crate::google::{build_google_body, parse_google_response};
use crate::ollama::{build_ollama_body, parse_ollama_response};
use crate::openai_compatible::{build_openai_compatible_body, parse_openai_compatible_response};
use crate::{ProviderAdapterConfig, ProviderCredential};

const ANTHROPIC_API_VERSION: &str = "2023-06-01";

#[derive(Clone)]
pub struct ProviderModelAdapter {
    client: Client,
    config: ProviderAdapterConfig,
}

impl ProviderModelAdapter {
    pub fn new(config: ProviderAdapterConfig) -> Self {
        Self::with_client(config, Client::new())
    }

    pub fn with_client(config: ProviderAdapterConfig, client: Client) -> Self {
        Self { client, config }
    }

    fn credential_for(&self, provider: &str, default_base_url: &str) -> ProviderCredential {
        self.config
            .providers
            .get(provider)
            .cloned()
            .unwrap_or_else(|| ProviderCredential {
                api_key: None,
                base_url: default_base_url.to_owned(),
            })
    }

    fn key_required(
        &self,
        credential: &ProviderCredential,
        env_name: &str,
        label: &str,
    ) -> Result<String, String> {
        credential
            .api_key
            .clone()
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| format!("{env_name} is not configured for host-supplied {label} models"))
    }

    async fn generate_anthropic(
        &self,
        credential: ProviderCredential,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let api_key = self.key_required(&credential, "ANTHROPIC_API_KEY", "anthropic")?;
        let response = self
            .client
            .post(join_base_url(&credential.base_url, "/v1/messages"))
            .header("content-type", "application/json")
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .json(&build_anthropic_body(config, request)?)
            .send()
            .await
            .map_err(|error| transport_error("Anthropic", "request", error))?;
        let payload =
            response_payload(response, "Anthropic", credential.api_key.as_deref()).await?;
        parse_anthropic_response(&payload)
    }

    async fn generate_google(
        &self,
        credential: ProviderCredential,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let api_key = self.key_required(&credential, "GOOGLE_API_KEY", "google")?;
        let endpoint = format!(
            "{}/v1beta/models/{}:generateContent",
            credential.base_url.trim_end_matches('/'),
            config.model
        );
        let response = self
            .client
            .post(endpoint)
            .header("content-type", "application/json")
            .header("x-goog-api-key", api_key)
            .json(&build_google_body(config, request)?)
            .send()
            .await
            .map_err(|error| transport_error("Google", "request", error))?;
        let payload = response_payload(response, "Google", credential.api_key.as_deref()).await?;
        parse_google_response(&payload)
    }

    async fn generate_openai_compatible(
        &self,
        provider_name: &str,
        endpoint: String,
        api_key: Option<&str>,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let mut builder = self
            .client
            .post(endpoint)
            .header("content-type", "application/json")
            .json(&build_openai_compatible_body(config, request)?);
        if let Some(api_key) = api_key {
            builder = builder.bearer_auth(api_key);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| transport_error(provider_name, "request", error))?;
        let payload = response_payload(response, provider_name, api_key).await?;
        parse_openai_compatible_response(&payload, provider_name)
    }

    async fn generate_ollama_native(
        &self,
        credential: &ProviderCredential,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let mut builder = self
            .client
            .post(ollama_native_endpoint(&credential.base_url))
            .header("content-type", "application/json")
            .json(&build_ollama_body(config, request)?);
        if let Some(api_key) = credential
            .api_key
            .as_deref()
            .filter(|key| !key.trim().is_empty())
        {
            builder = builder.bearer_auth(api_key);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| transport_error("Ollama", "request", error))?;
        let payload = response_payload(response, "Ollama", credential.api_key.as_deref()).await?;
        parse_ollama_response(&payload)
    }
}

#[async_trait]
impl ModelAdapter for ProviderModelAdapter {
    fn provider(&self) -> &str {
        "providers"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let requested = config
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or("deterministic")
            .to_ascii_lowercase();
        let entry = resolve_provider(&requested)
            .ok_or_else(|| format!("unknown model provider: {requested}"))?;
        let definition = &entry.definition;
        let credential = self.credential_for(definition.id, definition.default_base_url);

        match entry.kind {
            ProviderKind::Anthropic => self.generate_anthropic(credential, config, request).await,
            ProviderKind::Google => self.generate_google(credential, config, request).await,
            ProviderKind::OpenAiCompatible => {
                if definition.requires_key {
                    self.key_required(
                        &credential,
                        definition
                            .api_key_envs
                            .first()
                            .copied()
                            .unwrap_or("API_KEY"),
                        definition.label,
                    )?;
                }
                if definition.id == "ollama" && config.tools.as_ref().is_none_or(Vec::is_empty) {
                    return self
                        .generate_ollama_native(&credential, config, request)
                        .await;
                }
                self.generate_openai_compatible(
                    definition.label,
                    join_base_url(&credential.base_url, "/chat/completions"),
                    credential.api_key.as_deref(),
                    config,
                    request,
                )
                .await
            }
        }
    }
}

async fn response_payload(
    response: reqwest::Response,
    provider: &str,
    api_key: Option<&str>,
) -> Result<serde_json::Value, String> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| transport_error(provider, "response read", error))?;
    if !status.is_success() {
        return Err(format!(
            "{provider} API error ({}): {}",
            status.as_u16(),
            sanitize_upstream_body(&text, api_key)
        ));
    }
    serde_json::from_str(&text)
        .map_err(|error| format!("{provider} response parse failed: {error}"))
}

fn sanitize_upstream_body(body: &str, api_key: Option<&str>) -> String {
    let redacted = api_key
        .filter(|key| !key.is_empty())
        .map_or_else(|| body.to_owned(), |key| body.replace(key, "[REDACTED]"));
    redacted.chars().take(1_024).collect()
}

fn transport_error(provider: &str, operation: &str, error: reqwest::Error) -> String {
    format!("{provider} {operation} failed: {}", error.without_url())
}

fn join_base_url(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn ollama_native_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    join_base_url(trimmed.strip_suffix("/v1").unwrap_or(trimmed), "/api/chat")
}
