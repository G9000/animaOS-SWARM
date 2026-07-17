use anima_core::{
    AgentConfig, Content, MessageRole, ModelGenerateRequest, ModelGenerateResponse,
    ModelStopReason, ToolCall, ToolDescriptor,
};
use serde_json::{json, Map, Value};

use super::common::{
    assistant_tool_calls_json, response_content_text, response_usage, tool_call_args, tool_call_id,
    tool_parameters_schema_json,
};

pub(super) fn build_openai_compatible_body(
    config: &AgentConfig,
    request: &ModelGenerateRequest,
) -> Result<Value, String> {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(config.model.clone()));
    body.insert(
        "messages".into(),
        Value::Array(build_openai_compatible_messages(request)?),
    );

    if let Some(tools) = build_openai_compatible_tools(config)? {
        body.insert("tools".into(), Value::Array(tools));
    }

    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
    }

    if let Some(max_tokens) = request.max_tokens {
        body.insert("max_tokens".into(), json!(max_tokens));
    }

    Ok(Value::Object(body))
}

fn build_openai_compatible_messages(request: &ModelGenerateRequest) -> Result<Vec<Value>, String> {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system,
    })];

    for message in &request.messages {
        match message.role {
            MessageRole::System => messages.push(json!({
                "role": "system",
                "content": message.content.text,
            })),
            MessageRole::User => messages.push(json!({
                "role": "user",
                "content": message.content.text,
            })),
            MessageRole::Assistant => {
                let tool_calls = assistant_tool_calls_json(message)?;
                let content = if message.content.text.is_empty() {
                    Value::Null
                } else {
                    Value::String(message.content.text.clone())
                };

                let mut assistant_message = Map::new();
                assistant_message.insert("role".into(), Value::String("assistant".into()));
                assistant_message.insert("content".into(), content);
                if let Some(tool_calls) = tool_calls {
                    assistant_message.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                messages.push(Value::Object(assistant_message));
            }
            MessageRole::Tool => messages.push(json!({
                "role": "tool",
                "tool_call_id": tool_call_id(message),
                "content": message.content.text,
            })),
        }
    }

    Ok(messages)
}

fn build_openai_compatible_tools(config: &AgentConfig) -> Result<Option<Vec<Value>>, String> {
    let Some(tools) = config.tools.as_ref().filter(|tools| !tools.is_empty()) else {
        return Ok(None);
    };

    tools
        .iter()
        .map(tool_descriptor_json)
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn tool_descriptor_json(tool: &ToolDescriptor) -> Result<Value, String> {
    Ok(json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool_parameters_schema_json(tool),
        }
    }))
}

pub(super) fn parse_openai_compatible_response(
    payload: &Value,
    provider_name: &str,
) -> Result<ModelGenerateResponse, String> {
    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| format!("{provider_name} response did not include a choice"))?;

    let message = choice
        .get("message")
        .ok_or_else(|| format!("{provider_name} response did not include a message"))?;

    let tool_calls = parse_openai_compatible_tool_calls(message.get("tool_calls"))?;
    let stop_reason = if tool_calls.as_ref().is_some_and(|calls| !calls.is_empty()) {
        ModelStopReason::ToolCall
    } else if choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .is_some_and(|reason| reason == "length")
    {
        ModelStopReason::MaxTokens
    } else {
        ModelStopReason::End
    };

    Ok(ModelGenerateResponse {
        content: Content {
            text: response_content_text(message.get("content")),
            attachments: None,
            metadata: None,
        },
        tool_calls,
        usage: response_usage(payload.get("usage")),
        stop_reason,
    })
}

fn parse_openai_compatible_tool_calls(
    value: Option<&Value>,
) -> Result<Option<Vec<ToolCall>>, String> {
    let Some(Value::Array(tool_calls)) = value else {
        return Ok(None);
    };

    tool_calls
        .iter()
        .map(|tool_call| {
            let id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "provider tool call is missing an id".to_string())?;
            let function = tool_call
                .get("function")
                .ok_or_else(|| "provider tool call is missing a function payload".to_string())?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "provider tool call is missing a function name".to_string())?;
            let args =
                tool_call_args(function.get("arguments").ok_or_else(|| {
                    "provider tool call is missing function arguments".to_string()
                })?)?;

            Ok(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                args,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|tool_calls| {
            if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            }
        })
}
