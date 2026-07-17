use anima_core::{
    AgentConfig, Content, MessageRole, ModelGenerateRequest, ModelGenerateResponse,
    ModelStopReason, TokenUsage,
};
use serde_json::{json, Map, Value};

pub(super) fn build_ollama_body(
    config: &AgentConfig,
    request: &ModelGenerateRequest,
) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(config.model.clone()));
    body.insert("stream".into(), Value::Bool(false));
    body.insert("think".into(), Value::Bool(false));
    body.insert(
        "messages".into(),
        Value::Array(build_ollama_messages(request)),
    );

    let mut options = Map::new();
    if let Some(temperature) = request.temperature {
        options.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = request.max_tokens {
        options.insert("num_predict".into(), json!(max_tokens));
    }
    if !options.is_empty() {
        body.insert("options".into(), Value::Object(options));
    }

    Ok(Value::Object(body))
}

fn build_ollama_messages(request: &ModelGenerateRequest) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system,
    })];

    for message in &request.messages {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        messages.push(json!({
            "role": role,
            "content": message.content.text,
        }));
    }

    messages
}

pub(super) fn parse_ollama_response(payload: &Value) -> Result<ModelGenerateResponse, String> {
    let message = payload
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| "Ollama response did not include a message".to_string())?;

    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let prompt_tokens = payload
        .get("prompt_eval_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let completion_tokens = payload
        .get("eval_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let stop_reason = match payload.get("done_reason").and_then(Value::as_str) {
        Some("length") => ModelStopReason::MaxTokens,
        _ => ModelStopReason::End,
    };

    Ok(ModelGenerateResponse {
        content: Content {
            text,
            attachments: None,
            metadata: None,
        },
        tool_calls: None,
        usage: TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
        stop_reason,
    })
}
