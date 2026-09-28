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
    /// Leave `temperature` out.
    pub(crate) omit_temperature: bool,
}

/// `stream_options.include_usage` goes to OpenAI and DeepSeek only at their
/// default base URL (the M0/M1 rule): a strict proxy at a custom
/// `OPENAI_BASE_URL` may 400 on an unrecognized field, and with streaming as
/// the only path that would fail every run. vLLM and Ollama document the
/// field at any base URL (with tools, Ollama streams through its
/// OpenAI-compatible endpoint, which reports usage only when asked), so they
/// always get it. Other providers never get it; Groq and Moonshot report
/// stream usage in their own fields without it. OpenAI's own endpoint gets
/// `max_completion_tokens`, which its reasoning models require instead of
/// `max_tokens` (spec §12.4), and no `temperature` for those reasoning models,
/// which reject any but the default (so a 0.2 compaction or title call still
/// works for them).
pub(crate) fn openai_request_shape(
    definition: &ProviderDefinition,
    base_url: &str,
    model: &str,
) -> OpenAiRequestShape {
    let default_endpoint = base_url
        .trim_end_matches('/')
        .eq_ignore_ascii_case(definition.default_base_url.trim_end_matches('/'));
    OpenAiRequestShape {
        stream_usage: matches!(definition.id, "vllm" | "ollama")
            || (matches!(definition.id, "openai" | "deepseek") && default_endpoint),
        max_completion_tokens: definition.id == "openai" && default_endpoint,
        omit_temperature: definition.id == "openai"
            && default_endpoint
            && is_openai_reasoning_model(model),
    }
}

/// OpenAI's reasoning model ids: `o1*`, `o3*`, `o4*`, and `gpt-5*` except the
/// `-chat` variants, which are ordinary chat models.
fn is_openai_reasoning_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    ["o1", "o3", "o4"]
        .iter()
        .any(|prefix| model.starts_with(prefix))
        || (model.starts_with("gpt-5") && !model.contains("-chat"))
}

pub(crate) fn shape_openai_body(body: &mut serde_json::Value, shape: OpenAiRequestShape) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if shape.omit_temperature {
        object.remove("temperature");
    }
    if shape.max_completion_tokens {
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
        let mut retried = false;
        loop {
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
                .map_err(|error| {
                    after_retry(
                        retried,
                        transport_error("Anthropic", "stream request", error),
                    )
                })?;
            if response.status().is_success() {
                if is_json_response(&response) {
                    let payload = response_payload(response, "Anthropic", Some(&api_key)).await?;
                    return emit_final(sink, parse_anthropic_response(&payload)?).await;
                }
                return consume_anthropic_sse(response, sink).await;
            }
            let retry = !retried && retryable(response.status());
            let error = response_payload(response, "Anthropic", Some(&api_key))
                .await
                .unwrap_err();
            if !retry {
                return Err(after_retry(retried, error));
            }
            retried = true;
        }
    }

    async fn stream_openai_compatible(
        &self,
        provider: &ProviderDefinition,
        endpoint: String,
        api_key: Option<&str>,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
        shape: OpenAiRequestShape,
    ) -> Result<(), String> {
        let provider_name = provider.label;
        let mut retried = false;
        loop {
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
            let response = builder.send().await.map_err(|error| {
                after_retry(
                    retried,
                    transport_error(provider_name, "stream request", error),
                )
            })?;
            if response.status().is_success() {
                if is_json_response(&response) {
                    let payload = response_payload(response, provider_name, api_key).await?;
                    return emit_final(
                        sink,
                        parse_openai_compatible_response(&payload, provider_name)?,
                    )
                    .await;
                }
                return consume_openai_sse(response, sink, provider).await;
            }
            let retry = !retried && retryable(response.status());
            let error = response_payload(response, provider_name, api_key)
                .await
                .unwrap_err();
            if !retry {
                return Err(after_retry(retried, error));
            }
            retried = true;
        }
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
        let mut retried = false;
        loop {
            let response = self
                .client
                .post(&endpoint)
                .header("content-type", "application/json")
                .header("x-goog-api-key", &api_key)
                .json(&build_google_body(config, request)?)
                .send()
                .await
                .map_err(|error| {
                    after_retry(retried, transport_error("Google", "stream request", error))
                })?;
            if response.status().is_success() {
                if is_json_response(&response) {
                    let payload = response_payload(response, "Google", Some(&api_key)).await?;
                    return emit_final(sink, parse_google_response(&payload)?).await;
                }
                return consume_google_sse(response, sink, &api_key).await;
            }
            let retry = !retried && retryable(response.status());
            let error = response_payload(response, "Google", Some(&api_key))
                .await
                .unwrap_err();
            if !retry {
                return Err(after_retry(retried, error));
            }
            retried = true;
        }
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
                    openai_request_shape(definition, &credential.base_url, &config.model),
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
                    definition,
                    join_base_url(&credential.base_url, "/chat/completions"),
                    credential.api_key.as_deref(),
                    config,
                    request,
                    sink,
                    openai_request_shape(definition, &credential.base_url, &config.model),
                )
                .await
            }
        }
    }
}

fn retryable(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
}

/// A stream request retries a 429 or 5xx answer once. When that one retry also
/// fails, the call returns the second attempt's own error, marked as such.
fn after_retry(retried: bool, error: String) -> String {
    if retried {
        format!("after one retry: {error}")
    } else {
        error
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

pub(crate) fn sanitize_upstream_body(body: &str, api_key: Option<&str>) -> String {
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
