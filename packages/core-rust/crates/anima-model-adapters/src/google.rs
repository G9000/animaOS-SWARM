use std::collections::BTreeMap;

use anima_core::{
    AgentConfig, Content, DataValue, Message, MessageRole, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, TokenUsage, ToolCall,
};
use serde_json::{json, Map, Value};

use super::common::{
    data_value_to_json, json_value_to_data_map, required_data_string, tool_call_id,
    tool_parameters_schema_json, value_to_u64,
};

const GOOGLE_RESPONSE_PARTS_JSON: &str = "googleResponsePartsJson";

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

    for (index, message) in request.messages.iter().enumerate() {
        match message.role {
            MessageRole::System => {}
            MessageRole::User => contents.push(json!({
                "role": "user",
                "parts": [{ "text": message.content.text }],
            })),
            MessageRole::Assistant => {
                let parts = match preserved_google_response_parts(message)? {
                    Some(parts) => parts,
                    None => {
                        let mut parts: Vec<Value> = Vec::new();
                        if !message.content.text.is_empty() {
                            parts.push(json!({ "text": message.content.text }));
                        }
                        parts.extend(google_function_call_parts(message)?);
                        parts
                    }
                };
                contents.push(json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
            MessageRole::Tool => {
                contents.push(google_tool_response(message, &request.messages[..index])?);
            }
        }
    }

    Ok(contents)
}

fn preserved_google_response_parts(message: &Message) -> Result<Option<Vec<Value>>, String> {
    let Some(metadata) = message.content.metadata.as_ref() else {
        return Ok(None);
    };
    let Some(parts_json) = metadata.get(GOOGLE_RESPONSE_PARTS_JSON) else {
        return Ok(None);
    };
    let DataValue::String(parts_json) = parts_json else {
        return Err("googleResponsePartsJson metadata must be a JSON string".to_string());
    };
    serde_json::from_str(parts_json)
        .map(Some)
        .map_err(|error| format!("invalid googleResponsePartsJson metadata: {error}"))
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
            let id = required_data_string(tool_call, "id")?;
            let name = required_data_string(tool_call, "name")?;
            let args = match tool_call.get("args") {
                Some(DataValue::Object(args)) => {
                    data_value_to_json(&DataValue::Object(args.clone()))
                }
                _ => json!({}),
            };
            Ok(json!({
                "functionCall": {
                    "id": id,
                    "name": name,
                    "args": args,
                }
            }))
        })
        .collect()
}

fn google_tool_response(message: &Message, prior_messages: &[Message]) -> Result<Value, String> {
    let call_id = tool_call_id(message);
    let name = prior_messages
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::Assistant)
        .filter_map(|message| message.content.metadata.as_ref())
        .filter_map(|metadata| metadata.get("toolCalls"))
        .filter_map(|calls| match calls {
            DataValue::Array(calls) => Some(calls),
            _ => None,
        })
        .flatten()
        .find_map(|call| match call {
            DataValue::Object(call)
                if call.get("id") == Some(&DataValue::String(call_id.clone())) =>
            {
                required_data_string(call, "name").ok()
            }
            _ => None,
        })
        .ok_or_else(|| {
            format!("Google tool result does not match a prior assistant call: {call_id}")
        })?;
    let response_value: Value = serde_json::from_str(&message.content.text)
        .unwrap_or_else(|_| json!({ "result": message.content.text }));

    Ok(json!({
        "role": "user",
        "parts": [{
            "functionResponse": {
                "name": name,
                "id": call_id,
                "response": response_value,
            }
        }],
    }))
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
    let raw_parts = parts.cloned().unwrap_or_default();
    let raw_parts_json = serde_json::to_string(&raw_parts)
        .map_err(|error| format!("failed to serialize Google response parts: {error}"))?;

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(parts) = parts {
        for part in parts {
            // Spec §12.4: thinking parts are never shown as response text; the raw
            // part is still kept in `raw_parts`/`raw_parts_json` above for replay.
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                continue;
            }
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
                let id = function_call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("call_{name}"));
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
        let prompt = value_to_u64(usage.get("promptTokenCount"))
            + value_to_u64(usage.get("toolUsePromptTokenCount"));
        let thoughts = value_to_u64(usage.get("thoughtsTokenCount"));
        let completion = value_to_u64(usage.get("candidatesTokenCount")) + thoughts;
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_prompt_tokens: value_to_u64(usage.get("cachedContentTokenCount")),
            reasoning_tokens: thoughts,
        }
    } else {
        TokenUsage::default()
    };

    Ok(ModelGenerateResponse {
        content: Content {
            text: text_parts.join(""),
            attachments: None,
            metadata: Some(BTreeMap::from([(
                GOOGLE_RESPONSE_PARTS_JSON.into(),
                DataValue::String(raw_parts_json),
            )])),
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

/// The parts of one streamed Google response (spec §12.4). Every raw part
/// is kept, so the final response built from them carries the same replay
/// metadata as a non-streamed one.
#[derive(Default)]
pub(crate) struct GoogleStreamAccumulator {
    parts: Vec<Value>,
    finish_reason: Option<String>,
    usage: Option<Value>,
}

/// More parts than any sane response; a stream past this is refused.
const MAX_GOOGLE_STREAM_PARTS: usize = 4_096;

impl GoogleStreamAccumulator {
    /// Takes one SSE payload; returns the text it adds. A `thought: true` part
    /// (spec §12.4) never contributes to the visible delta, though it is still
    /// kept in the raw parts below for replay.
    pub(crate) fn push(&mut self, payload: &Value) -> Result<Option<String>, String> {
        if let Some(usage) = payload.get("usageMetadata") {
            self.usage = Some(usage.clone());
        }
        let Some(candidate) = payload
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return Ok(None);
        };
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }
        let mut delta = String::new();
        for part in candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if self.parts.len() >= MAX_GOOGLE_STREAM_PARTS {
                return Err("provider stream parse failed".to_string());
            }
            let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
            if !is_thought {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    delta.push_str(text);
                }
            }
            self.parts.push(part.clone());
        }
        Ok((!delta.is_empty()).then_some(delta))
    }

    /// The whole response, parsed like a non-streamed one.
    pub(crate) fn finish(self) -> Result<ModelGenerateResponse, String> {
        let mut candidate = json!({ "content": { "role": "model", "parts": self.parts } });
        if let Some(reason) = self.finish_reason {
            candidate["finishReason"] = Value::String(reason);
        }
        let mut payload = json!({ "candidates": [candidate] });
        if let Some(usage) = self.usage {
            payload["usageMetadata"] = usage;
        }
        parse_google_response(&payload)
    }
}
