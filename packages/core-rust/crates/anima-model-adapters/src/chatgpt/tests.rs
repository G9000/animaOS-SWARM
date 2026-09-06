use super::*;
use anima_core::{Content, DataValue, Message, MessageRole, ModelStopReason, ToolDescriptor};
use serde_json::json;
use std::collections::BTreeMap;

fn config() -> AgentConfig {
    AgentConfig {
        name: "subscription test".into(),
        model: "gpt-5.5".into(),
        provider: Some("chatgpt".into()),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        system: None,
        plugins: None,
        settings: None,
        tools: Some(vec![ToolDescriptor {
            name: "lookup".into(),
            description: "Look up a record".into(),
            parameters_schema: BTreeMap::from([(
                "type".into(),
                DataValue::String("object".into()),
            )]),
            examples: None,
        }]),
    }
}

fn message(
    role: MessageRole,
    text: &str,
    metadata: Option<BTreeMap<String, DataValue>>,
) -> Message {
    Message {
        id: "message-1".into(),
        agent_id: "agent-1".into(),
        room_id: "room-1".into(),
        role,
        content: Content {
            text: text.into(),
            attachments: None,
            metadata,
        },
        created_at_ms: 0,
    }
}

#[test]
fn subscription_request_keeps_anima_tool_history_and_disables_storage() {
    let call = DataValue::Object(BTreeMap::from([
        ("id".into(), DataValue::String("call_1".into())),
        ("name".into(), DataValue::String("lookup".into())),
        ("args".into(), DataValue::Object(BTreeMap::new())),
    ]));
    let request = ModelGenerateRequest {
        system: "Use Anima permissions".into(),
        temperature: Some(0.7),
        max_tokens: Some(1000),
        messages: vec![
            message(MessageRole::User, "hello", None),
            message(
                MessageRole::Assistant,
                "",
                Some(BTreeMap::from([(
                    "toolCalls".into(),
                    DataValue::Array(vec![call]),
                )])),
            ),
            message(
                MessageRole::Tool,
                "record",
                Some(BTreeMap::from([(
                    "toolCallId".into(),
                    DataValue::String("call_1".into()),
                )])),
            ),
        ],
    };
    let body = request_body(&config(), &request).unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["instructions"], request.system);
    assert_eq!(body["input"][1]["type"], "function_call");
    assert_eq!(body["input"][1]["call_id"], "call_1");
    assert_eq!(body["input"][2]["type"], "function_call_output");
    assert_eq!(body["input"][2]["call_id"], "call_1");
    assert_eq!(body["tools"][0]["name"], "lookup");
    assert!(body.get("temperature").is_none());
    assert!(body.get("max_tokens").is_none());
}

#[test]
fn subscription_response_normalizes_tool_calls_and_usage() {
    let response = completed_response(&json!({"status":"completed", "output":[
        {"type":"message","content":[{"type":"output_text","text":"Checking"}]},
        {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"id\":7}"}
    ],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}))
    .unwrap();
    assert_eq!(response.content.text, "Checking");
    assert_eq!(response.stop_reason, ModelStopReason::ToolCall);
    assert_eq!(response.usage.prompt_tokens, 10);
    assert_eq!(response.usage.completion_tokens, 5);
    assert_eq!(response.tool_calls.unwrap()[0].name, "lookup");
}

#[test]
fn subscription_rejects_failed_or_malformed_output() {
    for payload in [
        json!({"status":"completed","output":[]}),
        json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"  "}]}]}),
        json!({"status":"incomplete","output":[{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"}]}),
        json!({"status":"failed","output":[]}),
        json!({"status":"completed","output":[{"type":"function_call","name":"lookup","arguments":"{}"}]}),
        json!({"status":"completed"}),
    ] {
        assert!(completed_response(&payload).is_err());
    }
}

#[tokio::test]
async fn subscription_retains_completed_items_when_terminal_output_is_empty() {
    use axum::routing::post;
    for terminal_output in [
        json!([]),
        Value::Null,
        json!([
            {"type":"message","content":[{"type":"output_text","text":"Checking"}]},
            {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"id\":7}"}
        ]),
    ] {
        let events = [
            json!({"type":"response.output_text.delta","delta":"Checking"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{
                "type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"id\":7}"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":{
                "type":"message","content":[{"type":"output_text","text":"Checking"}]}}),
            json!({"type":"response.completed","response":{"status":"completed",
                "output":terminal_output,"usage":{"input_tokens":2,"output_tokens":3}}}),
        ];
        let body = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        let (endpoint, task) = server(axum::Router::new().route(
            "/responses",
            post(move || {
                let body = body.clone();
                async move { body }
            }),
        ))
        .await;
        let mut adapter = ChatGptResponsesAdapter::new("fake".into(), "account".into()).unwrap();
        adapter.endpoint = endpoint;
        let response = adapter.generate(&config(), &request()).await.unwrap();
        assert_eq!(response.content.text, "Checking");
        assert_eq!(response.tool_calls.unwrap()[0].id, "call_1");
        assert_eq!(response.usage.total_tokens, 5);
        task.abort();
    }
}

async fn server(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/responses"), task)
}

fn request() -> ModelGenerateRequest {
    ModelGenerateRequest {
        system: "System".into(),
        messages: vec![message(MessageRole::User, "Hi", None)],
        temperature: None,
        max_tokens: None,
    }
}

#[tokio::test]
async fn subscription_http_uses_only_subscription_auth_and_normalizes_stream() {
    use axum::{http::HeaderMap, routing::post, Json};
    let (endpoint, task) = server(axum::Router::new().route("/responses", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
        assert_eq!(headers["authorization"], "Bearer fake-subscription");
        assert_eq!(headers["chatgpt-account-id"], "test-account");
        assert_eq!(body["model"], "gpt-5.5");
        assert_eq!(body["store"], false);
        let delta = json!({"type":"response.output_text.delta", "delta":"Hello 世界"});
        let done = json!({"type":"response.completed", "response":{
            "status":"completed", "output":[{"type":"message","content":[{"type":"output_text","text":"Hello 世界"}]}],
            "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}});
        // Split within UTF-8 sequences and SSE delimiters, as real network chunks may do.
        let bytes = format!("data: {delta}\r\n\r\ndata: {done}\n\n").into_bytes();
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = bytes.chunks(3).map(|s| Ok(s.to_vec())).collect();
        axum::body::Body::from_stream(futures::stream::iter(chunks))
    }))).await;
    let mut adapter =
        ChatGptResponsesAdapter::new("fake-subscription".into(), "test-account".into()).unwrap();
    adapter.endpoint = endpoint;
    let response = adapter.generate(&config(), &request()).await.unwrap();
    assert_eq!(response.content.text, "Hello 世界");
    assert_eq!(response.usage.total_tokens, 5);
    task.abort();
}

#[tokio::test]
async fn subscription_rejects_truncated_stream_and_redacts_upstream_errors() {
    use axum::{http::StatusCode, routing::post};
    for (status, body, expected) in [
        (
            StatusCode::OK,
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "without a completed response",
        ),
        (
            StatusCode::OK,
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"lookup\",\"arguments\":\"{}\"}}\n\n",
            "without a completed response",
        ),
        (
            StatusCode::UNAUTHORIZED,
            "fake-subscription secret account details",
            "authorization expired",
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "fake-subscription",
            "usage limit reached",
        ),
        (
            StatusCode::OK,
            "data: {\"type\":\"response.failed\",\"error\":\"fake-subscription\"}\n\n",
            "generation failed",
        ),
    ] {
        let (endpoint, task) = server(
            axum::Router::new().route("/responses", post(move || async move { (status, body) })),
        )
        .await;
        let mut adapter =
            ChatGptResponsesAdapter::new("fake-subscription".into(), "test-account".into())
                .unwrap();
        adapter.endpoint = endpoint;
        let error = adapter.generate(&config(), &request()).await.unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("fake-subscription"));
        task.abort();
    }
}

#[tokio::test]
async fn subscription_does_not_follow_redirects() {
    use axum::{http::StatusCode, routing::post};
    let (endpoint, task) = server(axum::Router::new().route(
        "/responses",
        post(|| async {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", "https://example.com/collect")],
            )
        }),
    ))
    .await;
    let mut adapter =
        ChatGptResponsesAdapter::new("fake-subscription".into(), "test-account".into()).unwrap();
    adapter.endpoint = endpoint;
    assert!(adapter
        .generate(&config(), &request())
        .await
        .unwrap_err()
        .contains("HTTP 307"));
    task.abort();
}
