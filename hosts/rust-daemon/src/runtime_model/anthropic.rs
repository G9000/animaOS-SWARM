use anima_core::{
    AgentConfig, Content, DataValue, Message, MessageRole, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, TokenUsage, ToolCall,
};
use serde_json::{json, Map, Value};

use super::common::{
    data_value_to_json, json_value_to_data_map, required_data_string, tool_call_id,
    tool_parameters_schema_json, value_to_u64,
};

pub(super) fn build_anthropic_body(
    config: &AgentConfig,
    request: &ModelGenerateRequest,
) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(config.model.clone()));
    body.insert("system".into(), Value::String(request.system.clone()));
    body.insert(
        "messages".into(),
        Value::Array(build_anthropic_messages(request)?),
    );

    if let Some(tools) = build_anthropic_tools(config)? {
        body.insert("tools".into(), Value::Array(tools));
    }

    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    body.insert(
        "max_tokens".into(),
        json!(request.max_tokens.unwrap_or(4096)),
    );

    Ok(Value::Object(body))
}

fn build_anthropic_messages(request: &ModelGenerateRequest) -> Result<Vec<Value>, String> {
    let mut messages: Vec<Value> = Vec::new();

    for message in &request.messages {
        match message.role {
            MessageRole::System => {
                if let Some(Value::Object(last)) = messages.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("user") {
                        if let Some(Value::String(content)) = last.get_mut("content") {
                            content.push('\n');
                            content.push_str(&message.content.text);
                            continue;
                        }
                    }
                }
                messages.push(json!({
                    "role": "user",
                    "content": message.content.text,
                }));
            }
            MessageRole::User => messages.push(json!({
                "role": "user",
                "content": message.content.text,
            })),
            MessageRole::Assistant => {
                let tool_use = anthropic_assistant_tool_use(message)?;
                let mut content_blocks: Vec<Value> = Vec::new();

                if !message.content.text.is_empty() {
                    content_blocks.push(json!({
                        "type": "text",
                        "text": message.content.text,
                    }));
                }
                content_blocks.extend(tool_use);

                messages.push(json!({
                    "role": "assistant",
                    "content": content_blocks,
                }));
            }
            MessageRole::Tool => {
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id(message),
                        "content": message.content.text,
                    }],
                }));
            }
        }
    }

    Ok(messages)
}

fn anthropic_assistant_tool_use(message: &Message) -> Result<Vec<Value>, String> {
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
                return Err("assistant toolCall entries must be objects".to_string());
            };
            let id = required_data_string(tool_call, "id")?;
            let name = required_data_string(tool_call, "name")?;
            let input = match tool_call.get("args") {
                Some(DataValue::Object(args)) => {
                    data_value_to_json(&DataValue::Object(args.clone()))
                }
                Some(_) => return Err("assistant toolCall args must be an object".to_string()),
                None => json!({}),
            };
            Ok(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }))
        })
        .collect()
}

fn build_anthropic_tools(config: &AgentConfig) -> Result<Option<Vec<Value>>, String> {
    let Some(tools) = config.tools.as_ref().filter(|tools| !tools.is_empty()) else {
        return Ok(None);
    };

    tools
        .iter()
        .map(|tool| {
            Ok(json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool_parameters_schema_json(tool),
            }))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

pub(super) fn parse_anthropic_response(payload: &Value) -> Result<ModelGenerateResponse, String> {
    let content_blocks = payload
        .get("content")
        .and_then(Value::as_array)
        .ok_or("Anthropic response missing content array")?;

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_string());
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let empty_obj = Value::Object(Map::new());
                let input = block.get("input").unwrap_or(&empty_obj);
                let args = json_value_to_data_map(input)?;
                tool_calls.push(ToolCall { id, name, args });
            }
            _ => {}
        }
    }

    let stop_reason = if !tool_calls.is_empty() {
        ModelStopReason::ToolCall
    } else if payload.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
        ModelStopReason::MaxTokens
    } else {
        ModelStopReason::End
    };

    let usage = if let Some(usage) = payload.get("usage") {
        TokenUsage {
            prompt_tokens: value_to_u64(usage.get("input_tokens")),
            completion_tokens: value_to_u64(usage.get("output_tokens")),
            total_tokens: value_to_u64(usage.get("input_tokens"))
                + value_to_u64(usage.get("output_tokens")),
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
