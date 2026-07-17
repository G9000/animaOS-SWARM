use anima_core::{
    AgentConfig, Content, DataValue, Message, MessageRole, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, TokenUsage, ToolCall,
};
use serde_json::{json, Map, Value};

use super::common::{
    data_value_to_json, json_value_to_data_map, required_data_string, tool_call_id,
    tool_parameters_schema_json, value_to_u64,
};

pub(super) fn build_google_body(
    config: &AgentConfig,
    request: &ModelGenerateRequest,
) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert(
        "system_instruction".into(),
        json!({ "parts": [{ "text": request.system }] }),
    );
    body.insert(
        "contents".into(),
        Value::Array(build_google_contents(request)?),
    );

    if let Some(tools) = build_google_tools(config)? {
        body.insert("tools".into(), json!([{ "function_declarations": tools }]));
    }

    let mut generation_config = Map::new();
    if let Some(temperature) = request.temperature {
        generation_config.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = request.max_tokens {
        generation_config.insert("maxOutputTokens".into(), json!(max_tokens));
    }
    if !generation_config.is_empty() {
        body.insert("generationConfig".into(), Value::Object(generation_config));
    }

    Ok(Value::Object(body))
}

fn build_google_contents(request: &ModelGenerateRequest) -> Result<Vec<Value>, String> {
    let mut contents: Vec<Value> = Vec::new();

    for message in &request.messages {
        match message.role {
            MessageRole::System => {}
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
        .map(|tool_call| {
            let DataValue::Object(tool_call) = tool_call else {
                return Err("toolCall entries must be objects".to_string());
            };
            let name = required_data_string(tool_call, "name")?;
            let args = match tool_call.get("args") {
                Some(DataValue::Object(args)) => {
                    data_value_to_json(&DataValue::Object(args.clone()))
                }
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
    let Some(tools) = config.tools.as_ref().filter(|tools| !tools.is_empty()) else {
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

pub(super) fn parse_google_response(payload: &Value) -> Result<ModelGenerateResponse, String> {
    let candidate = payload
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or("Google response missing candidates")?;

    let parts = candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array);

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(parts) = parts {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                text_parts.push(text.to_string());
            }
            if let Some(function_call) = part.get("functionCall") {
                let name = function_call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let empty_obj = Value::Object(Map::new());
                let args_value = function_call.get("args").unwrap_or(&empty_obj);
                let args = json_value_to_data_map(args_value)?;
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
        .is_some_and(|reason| reason == "MAX_TOKENS")
    {
        ModelStopReason::MaxTokens
    } else {
        ModelStopReason::End
    };

    let usage = if let Some(usage) = payload.get("usageMetadata") {
        TokenUsage {
            prompt_tokens: value_to_u64(usage.get("promptTokenCount")),
            completion_tokens: value_to_u64(usage.get("candidatesTokenCount")),
            total_tokens: value_to_u64(usage.get("totalTokenCount")),
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
