use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, Content, Message, MessageRole, ModelAdapter, ModelGenerateRequest, ModelStopReason,
};
use anima_model_adapters::{provider_definitions, ProviderAdapterConfig, ProviderCredential};
use axum::{http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::{provider_summaries_for_config, RuntimeModelAdapter};

#[tokio::test]
async fn runtime_adapter_delegates_deterministic_requests_to_daemon_adapter() {
    let adapter = RuntimeModelAdapter::with_config(ProviderAdapterConfig::default());

    let response = adapter
        .generate(&agent_config("deterministic"), &request())
        .await
        .expect("deterministic adapter should generate");

    assert_eq!(response.stop_reason, ModelStopReason::End);
    assert_eq!(
        response.content.text,
        "operator handled task: prepare a campaign"
    );
}

#[tokio::test]
async fn runtime_adapter_routes_openai_requests_through_provider_adapter() {
    let seen_auth = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().route(
        "/v1/chat/completions",
        post({
            let seen_auth = Arc::clone(&seen_auth);
            move |headers: HeaderMap, Json(body): Json<Value>| {
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
                    assert!(body.get("messages").is_some());

                    Json(json!({
                        "choices": [{
                            "message": { "content": "provider response" },
                            "finish_reason": "stop"
                        }],
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
    let adapter = RuntimeModelAdapter::with_config(ProviderAdapterConfig {
        providers: BTreeMap::from([(
            "openai".into(),
            ProviderCredential {
                api_key: Some("test-key".into()),
                base_url: format!("{base_url}/v1"),
            },
        )]),
    });

    let response = adapter
        .generate(&agent_config("openai"), &request())
        .await
        .expect("openai provider should generate");

    assert_eq!(response.content.text, "provider response");
    assert_eq!(response.usage.total_tokens, 12);
    assert_eq!(
        seen_auth
            .lock()
            .expect("auth mutex should not be poisoned")
            .as_slice(),
        ["Bearer test-key"]
    );
}

#[test]
fn provider_summaries_are_canonical_and_never_include_credentials() {
    let secret = "not-for-provider-output";
    let config = ProviderAdapterConfig {
        providers: BTreeMap::from([(
            "openai".into(),
            ProviderCredential {
                api_key: Some(secret.into()),
                base_url: "https://private.example/v1".into(),
            },
        )]),
    };

    let summaries = provider_summaries_for_config(&config);
    assert_eq!(summaries[0].id, "deterministic");
    assert_eq!(summaries[0].label, "Deterministic (mock)");
    assert!(!summaries[0].requires_key);
    assert!(summaries[0].configured);

    assert_eq!(summaries.len(), provider_definitions().len() + 1);
    for (summary, definition) in summaries.iter().skip(1).zip(provider_definitions()) {
        assert_eq!(summary.id, definition.id);
        assert_eq!(summary.label, definition.label);
        assert_eq!(summary.requires_key, definition.requires_key);
        assert_eq!(summary.api_key_envs, definition.api_key_envs);
        assert_eq!(
            summary.configured,
            !definition.requires_key || definition.id == "openai"
        );
    }

    let rendered = format!("{summaries:#?}");
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("private.example"));
}

fn agent_config(provider: &str) -> AgentConfig {
    AgentConfig {
        name: "operator".into(),
        model: "gpt-4o-mini".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some(provider.into()),
        system: None,
        tools: None,
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
            created_at_ms: 1,
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
