use std::collections::BTreeMap;

use anima_core::{DataValue, Message, TokenUsage, ToolDescriptor};
use serde_json::{json, Value};

pub(super) fn tool_parameters_schema_json(tool: &ToolDescriptor) -> Value {
    if tool_parameters_are_json_schema(&tool.parameters) {
        data_value_to_json(&DataValue::Object(tool.parameters.clone()))
    } else {
        json!({
            "type": "object",
            "properties": data_value_to_json(&DataValue::Object(tool.parameters.clone())),
        })
    }
}

fn tool_parameters_are_json_schema(parameters: &BTreeMap<String, DataValue>) -> bool {
    matches!(parameters.get("type"), Some(DataValue::String(_)))
        || parameters.contains_key("properties")
        || parameters.contains_key("required")
        || parameters.contains_key("items")
        || parameters.contains_key("oneOf")
        || parameters.contains_key("anyOf")
        || parameters.contains_key("allOf")
        || parameters.contains_key("additionalProperties")
}

pub(super) fn assistant_tool_calls_json(message: &Message) -> Result<Option<Vec<Value>>, String> {
    let Some(metadata) = message.content.metadata.as_ref() else {
        return Ok(None);
    };
    let Some(value) = metadata.get("toolCalls") else {
        return Ok(None);
    };
    let DataValue::Array(tool_calls) = value else {
        return Err("assistant toolCalls metadata must be an array".to_string());
    };

    tool_calls
        .iter()
        .map(|tool_call| {
            let DataValue::Object(tool_call) = tool_call else {
                return Err("assistant toolCall metadata entries must be objects".to_string());
            };

            let id = required_data_string(tool_call, "id")?;
            let name = required_data_string(tool_call, "name")?;
            let args = match tool_call.get("args") {
                Some(DataValue::Object(args)) => args.clone(),
                Some(_) => return Err("assistant toolCall args must be an object".to_string()),
                None => BTreeMap::new(),
            };

            Ok(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": data_value_to_json_string(&DataValue::Object(args))?,
                }
            }))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

pub(super) fn required_data_string(
    object: &BTreeMap<String, DataValue>,
    key: &str,
) -> Result<String, String> {
    match object.get(key) {
        Some(DataValue::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(format!("missing required string field: {key}")),
    }
}

pub(super) fn tool_call_id(message: &Message) -> String {
    message
        .content
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("toolCallId"))
        .and_then(|value| match value {
            DataValue::String(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_else(|| message.id.clone())
}

pub(super) fn tool_call_args(value: &Value) -> Result<BTreeMap<String, DataValue>, String> {
    match value {
        Value::String(arguments) => {
            let parsed: Value = serde_json::from_str(arguments)
                .map_err(|error| format!("failed to parse tool call arguments: {error}"))?;
            json_value_to_data_map(&parsed)
        }
        Value::Object(_) => json_value_to_data_map(value),
        _ => Err("tool call arguments must be a JSON object or stringified JSON object".into()),
    }
}

pub(super) fn json_value_to_data_map(value: &Value) -> Result<BTreeMap<String, DataValue>, String> {
    match value {
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), json_value_to_data_value(value)?)))
            .collect(),
        _ => Err("expected a JSON object".to_string()),
    }
}

fn json_value_to_data_value(value: &Value) -> Result<DataValue, String> {
    Ok(match value {
        Value::Null => DataValue::Null,
        Value::Bool(value) => DataValue::Bool(*value),
        Value::Number(value) => DataValue::Number(
            value
                .as_f64()
                .ok_or_else(|| "expected a finite JSON number".to_string())?,
        ),
        Value::String(value) => DataValue::String(value.clone()),
        Value::Array(values) => DataValue::Array(
            values
                .iter()
                .map(json_value_to_data_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Object(values) => DataValue::Object(
            values
                .iter()
                .map(|(key, value)| -> Result<(String, DataValue), String> {
                    Ok((key.clone(), json_value_to_data_value(value)?))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?,
        ),
    })
}

pub(super) fn data_value_to_json(value: &DataValue) -> Value {
    match value {
        DataValue::Null => Value::Null,
        DataValue::Bool(value) => Value::Bool(*value),
        DataValue::Number(value) => json!(value),
        DataValue::String(value) => Value::String(value.clone()),
        DataValue::Array(values) => Value::Array(values.iter().map(data_value_to_json).collect()),
        DataValue::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
        ),
    }
}

pub(super) fn data_value_to_json_string(value: &DataValue) -> Result<String, String> {
    serde_json::to_string(&data_value_to_json(value))
        .map_err(|error| format!("failed to serialize tool call arguments: {error}"))
}

pub(super) fn response_content_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                Value::Object(value) => value
                    .get("text")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

pub(super) fn response_usage(value: Option<&Value>) -> TokenUsage {
    let Some(Value::Object(usage)) = value else {
        return TokenUsage::default();
    };

    TokenUsage {
        prompt_tokens: value_to_u64(usage.get("prompt_tokens")),
        completion_tokens: value_to_u64(usage.get("completion_tokens")),
        total_tokens: value_to_u64(usage.get("total_tokens")),
    }
}

pub(super) fn value_to_u64(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(value)) => value
            .as_u64()
            .unwrap_or_else(|| value.as_f64().unwrap_or(0.0) as u64),
        _ => 0,
    }
}
