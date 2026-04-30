use anima_core::{DataValue, EngineEvent, EventType};
use serde_json::{json, Value};

use crate::events::EventFanout;

pub(super) fn publish_runtime_event(
    event_stream: &EventFanout,
    agent_name: &str,
    event: EngineEvent,
) {
    let Some(agent_id) = event.agent_id.as_deref() else {
        return;
    };

    let payload = match runtime_event_payload(agent_id, agent_name, &event) {
        Some(payload) => payload,
        None => return,
    };

    event_stream.publish(event.event_type.as_str(), payload.to_string());
}

fn runtime_event_payload(agent_id: &str, agent_name: &str, event: &EngineEvent) -> Option<Value> {
    match event.event_type {
        EventType::TaskStarted => Some(json!({
            "agentId": agent_id,
            "agentName": agent_name,
        })),
        EventType::TaskCompleted => Some(json!({
            "agentId": agent_id,
            "agentName": agent_name,
        })),
        EventType::TaskFailed => Some(json!({
            "agentId": agent_id,
            "agentName": agent_name,
            "error": data_value_as_string(&event.data).unwrap_or("task failed"),
        })),
        EventType::ToolBefore => Some(json!({
            "agentId": agent_id,
            "agentName": agent_name,
            "toolName": object_field_string(&event.data, "name").unwrap_or_default(),
            "args": object_field_json(&event.data, "args").unwrap_or_else(|| json!({})),
        })),
        EventType::ToolAfter => Some(json!({
            "agentId": agent_id,
            "agentName": agent_name,
            "toolName": object_field_string(&event.data, "name").unwrap_or_default(),
            "status": object_field_string(&event.data, "status").unwrap_or("error"),
            "durationMs": object_field_u128(&event.data, "durationMs").unwrap_or(0),
            "result": object_field_string(&event.data, "result"),
        })),
        EventType::AgentTokens => Some(json!({
            "agentId": agent_id,
            "agentName": agent_name,
            "usage": data_value_to_json(&event.data),
        })),
        EventType::AgentTerminated => Some(json!({
            "agentId": agent_id,
            "agentName": agent_name,
        })),
        _ => None,
    }
}

fn data_value_as_string(value: &DataValue) -> Option<&str> {
    match value {
        DataValue::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn object_field_string<'a>(value: &'a DataValue, key: &str) -> Option<&'a str> {
    match value {
        DataValue::Object(object) => object.get(key).and_then(data_value_as_string),
        _ => None,
    }
}

fn object_field_u128(value: &DataValue, key: &str) -> Option<u128> {
    match value {
        DataValue::Object(object) => match object.get(key) {
            Some(DataValue::Number(value)) if *value >= 0.0 => Some(*value as u128),
            _ => None,
        },
        _ => None,
    }
}

fn object_field_json(value: &DataValue, key: &str) -> Option<Value> {
    match value {
        DataValue::Object(object) => object.get(key).map(data_value_to_json),
        _ => None,
    }
}

fn data_value_to_json(value: &DataValue) -> Value {
    match value {
        DataValue::Null => Value::Null,
        DataValue::Bool(value) => json!(value),
        DataValue::Number(value) => json!(value),
        DataValue::String(value) => json!(value),
        DataValue::Array(values) => {
            Value::Array(values.iter().map(data_value_to_json).collect::<Vec<_>>())
        }
        DataValue::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
        ),
    }
}
