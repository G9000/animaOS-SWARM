use anima_core::{
    AgentConfig, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStreamFrame,
    ModelStreamSink,
};
use async_trait::async_trait;
use reqwest::Client;

use crate::anthropic::{build_anthropic_body, parse_anthropic_response};
use crate::catalog::{resolve_provider, ProviderKind};
use crate::google::{build_google_body, parse_google_response};
use crate::ollama::{build_ollama_body, parse_ollama_response};
use crate::openai_compatible::{build_openai_compatible_body, parse_openai_compatible_response};
use crate::stream::{
    consume_anthropic_sse, consume_google_sse, consume_ollama_ndjson, consume_openai_sse,
};
use crate::ProviderDefinition;
use crate::{ProviderAdapterConfig, ProviderCredential};

const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// How one OpenAI-compatible endpoint wants its request shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpenAiRequestShape {
    /// Send `stream_options.include_usage` on streamed requests.
    pub(crate) stream_usage: bool,
    /// Send `max_completion_tokens` instead of `max_tokens`.
    pub(crate) max_completion_tokens: bool,
}

/// `stream_options.include_usage` goes only to endpoints that document it:
/// any vLLM server, and OpenAI and DeepSeek at their own default endpoints (a
/// custom base URL may be Azure or a strict proxy that rejects the field).
/// OpenAI's own endpoint gets `max_completion_tokens`, which its reasoning
/// models require instead of `max_tokens` (spec §12.4).
pub(crate) fn openai_request_shape(
    definition: &ProviderDefinition,
    base_url: &str,
) -> OpenAiRequestShape {
    let default_endpoint = base_url
        .trim_end_matches('/')
        .eq_ignore_ascii_case(definition.default_base_url.trim_end_matches('/'));
    OpenAiRequestShape {
        stream_usage: match definition.id {
            "vllm" => true,
            "openai" | "deepseek" => default_endpoint,
            _ => false,
        },
        max_completion_tokens: definition.id == "openai" && default_endpoint,
    }
}

fn shape_openai_body(body: &mut serde_json::Value, shape: OpenAiRequestShape) {
    if !shape.max_completion_tokens {
        return;
    }
    if let Some(object) = body.as_object_mut() {
        if let Some(max_tokens) = object.remove("max_tokens") {
            object.insert("max_completion_tokens".into(), max_tokens);
        }
    }
}

/// Whether a success answer is plain JSON rather than a stream.
fn is_json_response(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("application/json")
        })
}

async fn emit_final(
    sink: &dyn ModelStreamSink,
    response: ModelGenerateResponse,
) -> Result<(), String> {
    sink.emit(ModelStreamFrame::Final(response))
        .await
        .map_err(|_| "provider stream consumer failed".to_owned())
}

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
        shape: OpenAiRequestShape,
    ) -> Result<ModelGenerateResponse, String> {
        let mut body = build_openai_compatible_body(config, request)?;
        shape_openai_body(&mut body, shape);
        let mut builder = self
            .client
            .post(endpoint)
            .header("content-type", "application/json")
            .json(&body);
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

    async fn stream_anthropic(
        &self,
        credential: ProviderCredential,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        let api_key = self.key_required(&credential, "ANTHROPIC_API_KEY", "anthropic")?;
        for attempt in 0..2 {
            let mut body = build_anthropic_body(config, request)?;
            body["stream"] = serde_json::Value::Bool(true);
            let response = self
                .client
                .post(join_base_url(&credential.base_url, "/v1/messages"))
                .header("content-type", "application/json")
                .header("x-api-key", &api_key)
                .header("anthropic-version", ANTHROPIC_API_VERSION)
                .json(&body)
                .send()
                .await
                .map_err(|error| transport_error("Anthropic", "stream request", error))?;
            if response.status().is_success() {
                if is_json_response(&response) {
                    let payload = response_payload(response, "Anthropic", Some(&api_key)).await?;
                    return emit_final(sink, parse_anthropic_response(&payload)?).await;
                }
                return consume_anthropic_sse(response, sink).await;
            }
            let retry = retryable(response.status()) && attempt == 0;
            let error = response_payload(response, "Anthropic", Some(&api_key))
                .await
                .unwrap_err();
            if !retry {
                return Err(error);
            }
        }
        Err("Anthropic stream retry exhausted".into())
    }

    async fn stream_openai_compatible(
        &self,
        provider_name: &str,
        endpoint: String,
        api_key: Option<&str>,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
        shape: OpenAiRequestShape,
    ) -> Result<(), String> {
        for attempt in 0..2 {
            let mut body = build_openai_compatible_body(config, request)?;
            shape_openai_body(&mut body, shape);
            body["stream"] = serde_json::Value::Bool(true);
            if shape.stream_usage {
                body["stream_options"] = serde_json::json!({ "include_usage": true });
            }
            let mut builder = self
                .client
                .post(&endpoint)
                .header("content-type", "application/json")
                .json(&body);
            if let Some(api_key) = api_key {
                builder = builder.bearer_auth(api_key);
            }
            let response = builder
                .send()
                .await
                .map_err(|error| transport_error(provider_name, "stream request", error))?;
            if response.status().is_success() {
                if is_json_response(&response) {
                    let payload = response_payload(response, provider_name, api_key).await?;
                    return emit_final(
                        sink,
                        parse_openai_compatible_response(&payload, provider_name)?,
                    )
                    .await;
                }
                return consume_openai_sse(response, sink).await;
            }
            let retry = retryable(response.status()) && attempt == 0;
            let error = response_payload(response, provider_name, api_key)
                .await
                .unwrap_err();
            if !retry {
                return Err(error);
            }
        }
        Err(format!("{provider_name} stream retry exhausted"))
    }

    async fn stream_google(
        &self,
        credential: ProviderCredential,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        let api_key = self.key_required(&credential, "GOOGLE_API_KEY", "google")?;
        let endpoint = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            credential.base_url.trim_end_matches('/'),
            config.model
        );
        for attempt in 0..2 {
            let response = self
                .client
                .post(&endpoint)
                .header("content-type", "application/json")
                .header("x-goog-api-key", &api_key)
                .json(&build_google_body(config, request)?)
                .send()
                .await
                .map_err(|error| transport_error("Google", "stream request", error))?;
            if response.status().is_success() {
                if is_json_response(&response) {
                    let payload = response_payload(response, "Google", Some(&api_key)).await?;
                    return emit_final(sink, parse_google_response(&payload)?).await;
                }
                return consume_google_sse(response, sink).await;
            }
            let status = response.status();
            let error = response_payload(response, "Google", Some(&api_key))
                .await
                .unwrap_err();
            // A non-retryable failure surfaces immediately, at either attempt. A
            // retryable failure on attempt 0 retries once; if attempt 1 *also*
            // fails retryably, that is reported as exhaustion rather than the raw
            // second error (unlike attempt == 0's gate alone, which would make the
            // trailing "retry exhausted" below unreachable).
            if !retryable(status) {
                return Err(error);
            }
            if attempt == 1 {
                return Err("Google stream retry exhausted".into());
            }
        }
        Err("Google stream retry exhausted".into())
    }

    async fn stream_ollama_native(
        &self,
        credential: &ProviderCredential,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        let mut body = build_ollama_body(config, request)?;
        body["stream"] = serde_json::Value::Bool(true);
        let api_key = credential
            .api_key
            .as_deref()
            .filter(|key| !key.trim().is_empty());
        let mut builder = self
            .client
            .post(ollama_native_endpoint(&credential.base_url))
            .header("content-type", "application/json")
            .json(&body);
        if let Some(api_key) = api_key {
            builder = builder.bearer_auth(api_key);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| transport_error("Ollama", "stream request", error))?;
        if !response.status().is_success() {
            return Err(response_payload(response, "Ollama", api_key)
                .await
                .unwrap_err());
        }
        if is_json_response(&response) {
            let payload = response_payload(response, "Ollama", api_key).await?;
            return emit_final(sink, parse_ollama_response(&payload)?).await;
        }
        consume_ollama_ndjson(response, sink).await
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
                    openai_request_shape(definition, &credential.base_url),
                )
                .await
            }
        }
    }

    async fn stream(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
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
            ProviderKind::Anthropic => {
                self.stream_anthropic(credential, config, request, sink)
                    .await
            }
            ProviderKind::Google => self.stream_google(credential, config, request, sink).await,
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
                        .stream_ollama_native(&credential, config, request, sink)
                        .await;
                }
                self.stream_openai_compatible(
                    definition.label,
                    join_base_url(&credential.base_url, "/chat/completions"),
                    credential.api_key.as_deref(),
                    config,
                    request,
                    sink,
                    openai_request_shape(definition, &credential.base_url),
                )
                .await
            }
        }
    }
}

fn retryable(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
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
