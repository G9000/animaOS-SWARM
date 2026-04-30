use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, Content, DataValue, Message, MessageRole, ModelAdapter, ModelGenerateRequest,
    ModelStopReason, ToolDescriptor,
};
use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::common::{data_value_to_json, tool_parameters_schema_json};
use super::{RuntimeModelAdapter, RuntimeModelAdapterConfig, PROVIDER_DEFS};

fn test_config(overrides: &[(&str, Option<&str>, &str)]) -> RuntimeModelAdapterConfig {
    let providers = PROVIDER_DEFS
        .iter()
        .map(|(name, def)| {
            if let Some((_, key, url)) = overrides.iter().find(|(provider_name, _, _)| provider_name == name)
            {
                (name.to_string(), key.map(|value| value.to_string()), url.to_string())
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
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        headers
                            .get("anthropic-version")
                            .and_then(|value| value.to_str().ok())
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
                .and_then(|value| value.to_str().ok())
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
                .and_then(|value| value.to_str().ok())
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
