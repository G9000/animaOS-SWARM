use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentSettings, Content, DataValue, Message, MessageRole, ModelAdapter,
    ModelGenerateRequest, ModelStopReason, ToolDescriptor,
};
use axum::{
    extract::Json,
    http::{HeaderMap, Uri},
    routing::post,
    Router,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use crate::{
    provider_definitions, ProviderAdapterConfig, ProviderCredential, ProviderModelAdapter,
};

#[test]
fn provider_definitions_expose_current_non_secret_metadata() {
    let definitions = provider_definitions();
    let ids: Vec<_> = definitions.iter().map(|definition| definition.id).collect();
    assert_eq!(
        ids,
        vec![
            "openai",
            "anthropic",
            "google",
            "ollama",
            "groq",
            "xai",
            "openrouter",
            "mistral",
            "together",
            "deepseek",
            "fireworks",
            "perplexity",
            "moonshot"
        ]
    );
    assert!(definitions
        .iter()
        .all(|definition| !definition.label.is_empty()));
    assert_eq!(
        definitions
            .iter()
            .find(|definition| definition.id == "google")
            .expect("google definition")
            .aliases,
        ["gemini"]
    );
    assert_eq!(
        definitions
            .iter()
            .find(|definition| definition.id == "xai")
            .expect("xai definition")
            .aliases,
        ["grok"]
    );
    assert_eq!(
        definitions
            .iter()
            .find(|definition| definition.id == "moonshot")
            .expect("moonshot definition")
            .aliases,
        ["kimi"]
    );
}

#[test]
fn routing_resolves_the_single_public_provider_catalog() {
    for definition in provider_definitions() {
        let resolved = crate::catalog::resolve_provider(definition.id)
            .expect("every public provider should be routable");
        assert!(std::ptr::eq(resolved.definition, definition));
    }
}

#[test]
fn credential_debug_output_redacts_api_keys() {
    let credential = ProviderCredential {
        api_key: Some("do-not-expose".into()),
        base_url: "https://example.test".into(),
    };
    assert!(!format!("{credential:?}").contains("do-not-expose"));
}

#[tokio::test]
async fn aliases_resolve_to_their_canonical_provider_configuration() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(_body): Json<Value>| async move { openai_response("alias") }),
    );
    let base_url = spawn_server(app).await;
    let adapter = adapter_with(&[("moonshot", Some("moon-key"), &format!("{base_url}/v1"))]);
    let response = adapter
        .generate(&agent_config("kimi", false), &request())
        .await
        .expect("alias should use canonical provider credentials");
    assert_eq!(response.content.text, "alias");
}

#[tokio::test]
async fn injected_reqwest_client_is_used_by_the_adapter() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|headers: HeaderMap, Json(_body): Json<Value>| async move {
            assert_eq!(
                headers
                    .get("x-anima-client")
                    .and_then(|value| value.to_str().ok()),
                Some("injected")
            );
            openai_response("injected client")
        }),
    );
    let base_url = spawn_server(app).await;
    let config = ProviderAdapterConfig {
        providers: BTreeMap::from([(
            "openai".into(),
            ProviderCredential {
                api_key: Some("key".into()),
                base_url: format!("{base_url}/v1"),
            },
        )]),
    };
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-anima-client",
        reqwest::header::HeaderValue::from_static("injected"),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .expect("custom client");
    let adapter = ProviderModelAdapter::with_client(config, client);

    let response = adapter
        .generate(&agent_config("openai", false), &request())
        .await
        .expect("injected client should make the request");
    assert_eq!(response.content.text, "injected client");
}

#[tokio::test]
async fn unknown_provider_is_rejected_clearly() {
    let error = ProviderModelAdapter::new(ProviderAdapterConfig::default())
        .generate(&agent_config("unknown", false), &request())
        .await
        .expect_err("unknown provider must fail");
    assert!(error.contains("unknown model provider: unknown"), "{error}");
}

#[tokio::test]
async fn key_required_provider_without_configured_key_is_rejected() {
    let error = ProviderModelAdapter::new(ProviderAdapterConfig::default())
        .generate(&agent_config("openai", false), &request())
        .await
        .expect_err("openai without key must fail");
    assert!(error.contains("OPENAI_API_KEY"), "{error}");
}

#[tokio::test]
async fn deterministic_and_test_are_host_owned_and_rejected() {
    let adapter = ProviderModelAdapter::new(ProviderAdapterConfig::default());
    for provider in ["deterministic", "test"] {
        let error = adapter
            .generate(&agent_config(provider, false), &request())
            .await
            .expect_err("host test provider must be rejected");
        assert!(error.contains("unknown model provider"), "{error}");
    }
}

#[tokio::test]
async fn settings_cannot_override_host_supplied_credentials_or_base_url() {
    let seen_auth = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().route(
        "/v1/chat/completions",
        post({
            let seen_auth = Arc::clone(&seen_auth);
            move |headers: HeaderMap, Json(_body): Json<Value>| {
                let seen_auth = Arc::clone(&seen_auth);
                async move {
                    seen_auth.lock().expect("mutex").push(
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_owned(),
                    );
                    openai_response("host config")
                }
            }
        }),
    );
    let base_url = spawn_server(app).await;
    let adapter = adapter_with(&[("openai", Some("host-key"), &format!("{base_url}/v1"))]);
    let mut config = agent_config("openai", false);
    let mut settings = AgentSettings::default();
    settings
        .additional
        .insert("apiKey".into(), DataValue::String("agent-key".into()));
    settings.additional.insert(
        "baseUrl".into(),
        DataValue::String("http://127.0.0.1:1".into()),
    );
    config.settings = Some(settings);
    let response = adapter
        .generate(&config, &request())
        .await
        .expect("host config wins");
    assert_eq!(response.content.text, "host config");
    assert_eq!(
        seen_auth.lock().expect("mutex").as_slice(),
        ["Bearer host-key"]
    );
}

#[tokio::test]
async fn routes_openai_compatible_requests_and_tool_calls() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["tools"][0]["function"]["name"], "delegate_task");
            Json(json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"delegate_task","arguments":r#"{"task":"research"}"#}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":8,"completion_tokens":4,"total_tokens":12}}))
        }),
    );
    let base_url = spawn_server(app).await;
    let response = adapter_with(&[("openai", Some("key"), &format!("{base_url}/v1"))])
        .generate(&agent_config("openai", true), &request())
        .await
        .expect("openai response");
    assert_eq!(response.stop_reason, ModelStopReason::ToolCall);
    assert_eq!(response.usage.total_tokens, 12);
    assert_eq!(response.tool_calls.expect("tool")[0].name, "delegate_task");
}

#[tokio::test]
async fn routes_anthropic_google_and_ollama_native_protocols() {
    let anthropic = Router::new().route("/v1/messages", post(|Json(body): Json<Value>| async move {
        assert!(body.get("system").is_some());
        Json(json!({"content":[{"type":"text","text":"anthropic"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":3}}))
    }));
    let base_url = spawn_server(anthropic).await;
    let response = adapter_with(&[("anthropic", Some("key"), &base_url)])
        .generate(&agent_config("anthropic", false), &request())
        .await
        .expect("anthropic response");
    assert_eq!(response.content.text, "anthropic");
    assert_eq!(response.usage.total_tokens, 5);

    let google = Router::new().route("/v1beta/models/gemini-2.0-flash:generateContent", post(|headers: HeaderMap, uri: Uri, Json(body): Json<Value>| async move {
        assert!(body.get("system_instruction").is_some());
        assert_eq!(headers.get("x-goog-api-key").and_then(|value| value.to_str().ok()), Some("key"));
        assert!(uri.query().is_none());
        Json(json!({"candidates":[{"content":{"role":"model","parts":[{"text":"google"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4,"totalTokenCount":7}}))
    }));
    let base_url = spawn_server(google).await;
    let mut config = agent_config("google", false);
    config.model = "gemini-2.0-flash".into();
    let response = adapter_with(&[("google", Some("key"), &base_url)])
        .generate(&config, &request())
        .await
        .expect("google response");
    assert_eq!(response.content.text, "google");
    assert_eq!(response.usage.total_tokens, 7);

    let ollama = Router::new().route("/api/chat", post(|Json(body): Json<Value>| async move {
        assert_eq!(body["stream"], false);
        Json(json!({"message":{"role":"assistant","content":"ollama"},"done":true,"done_reason":"stop","prompt_eval_count":5,"eval_count":7}))
    }));
    let base_url = spawn_server(ollama).await;
    let response = adapter_with(&[("ollama", None, &base_url)])
        .generate(&agent_config("ollama", false), &request())
        .await
        .expect("ollama works without key");
    assert_eq!(response.content.text, "ollama");
    assert_eq!(response.usage.total_tokens, 12);
}

#[tokio::test]
async fn google_transport_errors_do_not_expose_host_credentials_or_query_urls() {
    let conspicuous_key = "google-key-must-never-appear";
    let adapter = adapter_with(&[("google", Some(conspicuous_key), "http://127.0.0.1:9")]);
    let error = adapter
        .generate(&agent_config("google", false), &request())
        .await
        .expect_err("unreachable google endpoint should fail");

    assert!(
        !error.contains(conspicuous_key),
        "credential leaked: {error}"
    );
    assert!(!error.contains("?key="), "query credential leaked: {error}");
}

#[tokio::test]
async fn upstream_error_bodies_are_bounded_and_redacted() {
    let key = "configured-secret-must-not-leak";
    let body = format!("provider echoed {key}: {}", "x".repeat(2_000));
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let body = body.clone();
            async move { (axum::http::StatusCode::BAD_REQUEST, body) }
        }),
    );
    let base_url = spawn_server(app).await;
    let error = adapter_with(&[("openai", Some(key), &format!("{base_url}/v1"))])
        .generate(&agent_config("openai", false), &request())
        .await
        .expect_err("upstream error should surface safely");

    assert!(error.contains("OpenAI API error (400)"), "{error}");
    assert!(error.contains("[REDACTED]"), "{error}");
    assert!(!error.contains(key), "credential leaked: {error}");
    assert!(
        error.chars().count() <= 1_100,
        "error was not bounded: {error}"
    );
}

#[tokio::test]
async fn routes_anthropic_and_google_tool_calls() {
    let anthropic = Router::new().route("/v1/messages", post(|Json(body): Json<Value>| async move {
        assert_eq!(body["tools"][0]["name"], "delegate_task");
        Json(json!({"content":[{"type":"tool_use","id":"toolu-1","name":"delegate_task","input":{"task":"research"}}],"stop_reason":"tool_use","usage":{"input_tokens":2,"output_tokens":3}}))
    }));
    let base_url = spawn_server(anthropic).await;
    let response = adapter_with(&[("anthropic", Some("key"), &base_url)])
        .generate(&agent_config("anthropic", true), &request())
        .await
        .expect("anthropic tool call");
    assert_eq!(response.stop_reason, ModelStopReason::ToolCall);
    assert_eq!(
        response.tool_calls.expect("tool")[0].args.get("task"),
        Some(&DataValue::String("research".into()))
    );

    let google = Router::new().route("/v1beta/models/gemini-2.0-flash:generateContent", post(|Json(body): Json<Value>| async move {
        assert_eq!(body["tools"][0]["function_declarations"][0]["name"], "delegate_task");
        Json(json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"delegate_task","args":{"task":"research"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4,"totalTokenCount":7}}))
    }));
    let base_url = spawn_server(google).await;
    let mut config = agent_config("google", true);
    config.model = "gemini-2.0-flash".into();
    let response = adapter_with(&[("google", Some("key"), &base_url)])
        .generate(&config, &request())
        .await
        .expect("google tool call");
    assert_eq!(response.stop_reason, ModelStopReason::ToolCall);
    assert_eq!(response.tool_calls.expect("tool")[0].name, "delegate_task");
}

#[test]
fn google_native_tool_round_trip_preserves_call_ids_and_correlates_parallel_calls() {
    let parsed = crate::google::parse_google_response(&json!({
        "candidates": [{
            "content": { "parts": [
                {"functionCall": {"id": "google-call-a", "name": "delegate_task", "args": {"task": "research"}}},
                {"functionCall": {"id": "google-call-b", "name": "delegate_task", "args": {"task": "review"}}}
            ]}
        }]
    }))
    .expect("provider function calls should parse");
    let calls = parsed.tool_calls.expect("two tool calls");
    assert_eq!(calls[0].id, "google-call-a");
    assert_eq!(calls[1].id, "google-call-b");

    let mut request = request();
    let tool_calls = calls
        .iter()
        .map(|call| {
            DataValue::Object(BTreeMap::from([
                ("id".into(), DataValue::String(call.id.clone())),
                ("name".into(), DataValue::String(call.name.clone())),
                ("args".into(), DataValue::Object(call.args.clone())),
            ]))
        })
        .collect();
    request.messages.extend([
        Message {
            id: "assistant-call".into(),
            agent_id: "agent".into(),
            room_id: "room".into(),
            content: Content {
                text: String::new(),
                attachments: None,
                metadata: Some(BTreeMap::from([(
                    "toolCalls".into(),
                    DataValue::Array(tool_calls),
                )])),
            },
            role: MessageRole::Assistant,
            created_at_ms: 2,
        },
        tool_message("google-call-a", "first result"),
        tool_message("google-call-b", "second result"),
    ]);

    let body = crate::google::build_google_body(&agent_config("google", true), &request)
        .expect("follow-up request should build");
    let contents = body["contents"].as_array().expect("contents");
    assert_eq!(
        contents[1]["parts"][0]["functionCall"]["id"],
        "google-call-a"
    );
    assert_eq!(
        contents[1]["parts"][1]["functionCall"]["id"],
        "google-call-b"
    );
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["name"],
        "delegate_task"
    );
    assert_eq!(contents[2]["role"], "user");
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["id"],
        "google-call-a"
    );
    assert_eq!(
        contents[3]["parts"][0]["functionResponse"]["name"],
        "delegate_task"
    );
    assert_eq!(
        contents[3]["parts"][0]["functionResponse"]["id"],
        "google-call-b"
    );
}

#[test]
fn google_response_parts_with_thought_signatures_are_preserved_for_replay() {
    let signed_part = json!({
        "functionCall": {
            "id": "google-signed-call",
            "name": "delegate_task",
            "args": {"task": "research"}
        },
        "thoughtSignature": "opaque-provider-signature",
        "futureProviderField": {"kept": true}
    });
    let response = crate::google::parse_google_response(&json!({
        "candidates": [{"content": {"parts": [signed_part.clone()]}}]
    }))
    .expect("signed provider response should parse");
    let parts = response
        .content
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("googleResponseParts"))
        .expect("raw Google parts should be retained in metadata");
    assert_eq!(
        crate::common::data_value_to_json(parts),
        json!([signed_part])
    );

    let tool_call = response.tool_calls.expect("tool call").remove(0);
    let mut metadata = response.content.metadata.expect("raw metadata");
    metadata.insert(
        "toolCalls".into(),
        DataValue::Array(vec![DataValue::Object(BTreeMap::from([
            ("id".into(), DataValue::String(tool_call.id.clone())),
            ("name".into(), DataValue::String(tool_call.name.clone())),
            ("args".into(), DataValue::Object(tool_call.args.clone())),
        ]))]),
    );
    let mut follow_up = request();
    follow_up.messages.push(Message {
        id: "assistant-signed".into(),
        agent_id: "agent".into(),
        room_id: "room".into(),
        content: Content {
            text: String::new(),
            attachments: None,
            metadata: Some(metadata),
        },
        role: MessageRole::Assistant,
        created_at_ms: 2,
    });
    let body = crate::google::build_google_body(&agent_config("google", true), &follow_up)
        .expect("signed history should rebuild");
    assert_eq!(
        body["contents"][1]["parts"][0]["thoughtSignature"],
        "opaque-provider-signature"
    );
    assert_eq!(
        body["contents"][1]["parts"][0]["futureProviderField"],
        json!({"kept": true})
    );
}

#[tokio::test]
async fn google_second_request_replays_signed_calls_then_user_function_results() {
    let signed_part = json!({
        "functionCall": {"id": "call-signed", "name": "delegate_task", "args": {"task": "research"}},
        "thoughtSignature": "signature-from-google"
    });
    let signed_part_for_server = signed_part.clone();
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let app = Router::new().route(
        "/v1beta/models/gpt-4o-mini:generateContent",
        post({
            let seen = Arc::clone(&seen);
            move |Json(body): Json<Value>| {
                let seen = Arc::clone(&seen);
                let signed_part = signed_part_for_server.clone();
                async move {
                    let request_number = {
                        let mut requests = seen.lock().expect("seen requests");
                        requests.push(body);
                        requests.len()
                    };
                    if request_number == 1 {
                        Json(json!({"candidates":[{"content":{"parts":[signed_part]}}]}))
                    } else {
                        Json(json!({"candidates":[{"content":{"parts":[{"text":"done"}]}}]}))
                    }
                }
            }
        }),
    );
    let base_url = spawn_server(app).await;
    let adapter = adapter_with(&[("google", Some("key"), &base_url)]);
    let config = agent_config("google", true);
    let first = adapter
        .generate(&config, &request())
        .await
        .expect("first signed call should parse");
    let call = first.tool_calls.clone().expect("tool call").remove(0);
    let mut metadata = first.content.metadata.clone().expect("raw Google metadata");
    metadata.insert(
        "toolCalls".into(),
        DataValue::Array(vec![DataValue::Object(BTreeMap::from([
            ("id".into(), DataValue::String(call.id.clone())),
            ("name".into(), DataValue::String(call.name.clone())),
            ("args".into(), DataValue::Object(call.args.clone())),
        ]))]),
    );
    let mut second = request();
    second.messages.extend([
        Message {
            id: "assistant-signed".into(),
            agent_id: "agent".into(),
            room_id: "room".into(),
            content: Content {
                text: first.content.text,
                attachments: None,
                metadata: Some(metadata),
            },
            role: MessageRole::Assistant,
            created_at_ms: 2,
        },
        tool_message("call-signed", "completed"),
    ]);
    adapter
        .generate(&config, &second)
        .await
        .expect("second request should succeed");

    let requests = seen.lock().expect("seen requests");
    assert_eq!(requests.len(), 2);
    let contents = requests[1]["contents"].as_array().expect("second contents");
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"], json!([signed_part]));
    assert_eq!(contents[2]["role"], "user");
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["name"],
        "delegate_task"
    );
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["id"],
        "call-signed"
    );
}

#[tokio::test]
async fn ollama_with_tools_uses_its_openai_compatible_endpoint() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer key")
            );
            assert_eq!(body["tools"][0]["function"]["name"], "delegate_task");
            openai_response("ollama compatible")
        }),
    );
    let base_url = spawn_server(app).await;
    let response = adapter_with(&[("ollama", Some("key"), &format!("{base_url}/v1"))])
        .generate(&agent_config("ollama", true), &request())
        .await
        .expect("ollama compatible response");
    assert_eq!(response.content.text, "ollama compatible");
}

fn adapter_with(entries: &[(&str, Option<&str>, &str)]) -> ProviderModelAdapter {
    let providers = entries
        .iter()
        .map(|(id, api_key, base_url)| {
            (
                (*id).into(),
                ProviderCredential {
                    api_key: api_key.map(str::to_owned),
                    base_url: (*base_url).into(),
                },
            )
        })
        .collect();
    ProviderModelAdapter::new(ProviderAdapterConfig { providers })
}

fn agent_config(provider: &str, tools: bool) -> AgentConfig {
    AgentConfig {
        name: "test".into(),
        model: "gpt-4o-mini".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some(provider.into()),
        system: None,
        tools: tools.then(|| {
            vec![ToolDescriptor {
                name: "delegate_task".into(),
                description: "Delegate work".into(),
                parameters_schema: BTreeMap::from([(
                    "type".into(),
                    DataValue::String("object".into()),
                )]),
                examples: None,
            }]
        }),
        plugins: None,
        settings: None,
    }
}

fn request() -> ModelGenerateRequest {
    ModelGenerateRequest {
        system: "System".into(),
        messages: vec![Message {
            id: "message".into(),
            agent_id: "agent".into(),
            room_id: "room".into(),
            content: Content {
                text: "Hello".into(),
                attachments: None,
                metadata: None,
            },
            role: MessageRole::User,
            created_at_ms: 1,
        }],
        temperature: Some(0.2),
        max_tokens: Some(512),
    }
}

fn tool_message(call_id: &str, text: &str) -> Message {
    Message {
        id: format!("tool-{call_id}"),
        agent_id: "agent".into(),
        room_id: "room".into(),
        content: Content {
            text: text.into(),
            attachments: None,
            metadata: Some(BTreeMap::from([(
                "toolCallId".into(),
                DataValue::String(call_id.into()),
            )])),
        },
        role: MessageRole::Tool,
        created_at_ms: 3,
    }
}

fn openai_response(content: &str) -> Json<Value> {
    Json(
        json!({"choices":[{"message":{"content":content},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}),
    )
}

async fn spawn_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{address}")
}
