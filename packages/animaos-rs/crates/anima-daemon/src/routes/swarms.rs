use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentSettings, Content, DataValue, PluginDescriptor, TaskResult, TokenUsage,
    ToolDescriptor, ToolExample,
};
use anima_swarm::{SwarmConfig, SwarmState, SwarmStatus, SwarmStrategy};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use futures::stream;

use super::Response;
use crate::json::{escape_json, JsonParser, JsonValue};
use crate::state::DaemonState;

pub(crate) async fn handle_create_swarm(
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let body = match std::str::from_utf8(&body) {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid UTF-8",
            )
        }
    };

    let object = match JsonParser::new(body).parse_object() {
        Ok(object) => object,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid JSON",
            )
        }
    };

    let config = match parse_swarm_config(&object) {
        Ok(config) => config,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let (coordinator, event_stream) = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        match guard.build_swarm(config) {
            Ok(built) => built,
            Err(message) => {
                return Response::json(
                    "HTTP/1.1 400 Bad Request",
                    format!("{{\"error\":\"{}\"}}", escape_json(&message)),
                )
            }
        }
    };

    if let Err(message) = coordinator.start().await {
        return Response::json(
            "HTTP/1.1 400 Bad Request",
            format!("{{\"error\":\"{}\"}}", escape_json(&message)),
        );
    }

    let snapshot = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.register_swarm(coordinator, event_stream)
    };
    publish_swarm_event(state, &snapshot.id, "swarm:created", &snapshot, None);

    Response::json(
        "HTTP/1.1 201 Created",
        format!("{{\"swarm\":{}}}", swarm_state_json(&snapshot)),
    )
}

pub(crate) fn handle_get_swarm(swarm_id: &str, state: &Arc<Mutex<DaemonState>>) -> Response {
    let snapshot = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_swarm(swarm_id)
    };

    match snapshot {
        Some(snapshot) => Response::json(
            "HTTP/1.1 200 OK",
            format!("{{\"swarm\":{}}}", swarm_state_json(&snapshot)),
        ),
        None => Response::error("HTTP/1.1 404 Not Found", "not found"),
    }
}

pub(crate) async fn handle_run_swarm(
    swarm_id: &str,
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let body = match std::str::from_utf8(&body) {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid UTF-8",
            )
        }
    };

    let object = match JsonParser::new(body).parse_object() {
        Ok(object) => object,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid JSON",
            )
        }
    };

    let task = match required_string(&object, "text") {
        Ok(task) => task,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let coordinator = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_swarm_coordinator(swarm_id)
    };

    let Some(coordinator) = coordinator else {
        return Response::error("HTTP/1.1 404 Not Found", "not found");
    };

    publish_swarm_event(
        state,
        swarm_id,
        "swarm:running",
        &coordinator.get_state(),
        Some(&TaskResult::success(
            Content {
                text: task.clone(),
                attachments: None,
                metadata: None,
            },
            0,
        )),
    );

    let result = coordinator.dispatch(task).await;
    let snapshot = coordinator.get_state();
    {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.store_swarm_snapshot(snapshot.clone());
    }

    publish_swarm_event(state, swarm_id, "swarm:completed", &snapshot, Some(&result));

    Response::json(
        "HTTP/1.1 200 OK",
        format!(
            "{{\"swarm\":{},\"result\":{}}}",
            swarm_state_json(&snapshot),
            task_result_json(Some(&result))
        ),
    )
}

pub(crate) fn handle_subscribe_swarm_events(
    swarm_id: &str,
    state: &Arc<Mutex<DaemonState>>,
) -> axum::response::Response {
    let subscriber = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.subscribe_to_swarm_events(swarm_id)
    };

    let Some(subscriber) = subscriber else {
        return (
            StatusCode::NOT_FOUND,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            "{\"error\":\"not found\"}",
        )
            .into_response();
    };

    let stream = stream::unfold(subscriber, |mut subscriber| async move {
        loop {
            match subscriber.recv().await {
                Ok(message) => {
                    let event = Event::default().event(message.event).data(message.data);
                    return Some((Ok::<Event, Infallible>(event), subscriber));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn publish_swarm_event(
    state: &Arc<Mutex<DaemonState>>,
    swarm_id: &str,
    event: &str,
    snapshot: &SwarmState,
    result: Option<&TaskResult<Content>>,
) {
    let payload = format!(
        "{{\"swarmId\":\"{}\",\"state\":{},\"result\":{}}}",
        escape_json(swarm_id),
        swarm_state_json(snapshot),
        task_result_json(result)
    );

    state
        .lock()
        .expect("daemon state mutex should not be poisoned")
        .publish_swarm_event(swarm_id, event, payload);
}

fn parse_swarm_config(object: &BTreeMap<String, JsonValue>) -> Result<SwarmConfig, &'static str> {
    let strategy = match object.get("strategy") {
        Some(JsonValue::String(value)) => parse_strategy(value)?,
        _ => return Err("strategy is required"),
    };

    let manager = match object.get("manager") {
        Some(JsonValue::Object(value)) => parse_agent_config(value)?,
        _ => return Err("manager is required"),
    };

    let workers = match object.get("workers") {
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                JsonValue::Object(object) => parse_agent_config(object),
                _ => Err("workers must contain objects"),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("workers are required"),
    };

    Ok(SwarmConfig {
        strategy,
        manager,
        workers,
        max_concurrent_agents: optional_usize(object.get("maxConcurrentAgents"))?,
        max_turns: optional_usize(object.get("maxTurns"))?,
        token_budget: optional_u64(object.get("tokenBudget"))?,
    })
}

fn parse_strategy(value: &str) -> Result<SwarmStrategy, &'static str> {
    match value {
        "supervisor" => Ok(SwarmStrategy::Supervisor),
        "dynamic" => Ok(SwarmStrategy::Dynamic),
        "round-robin" => Ok(SwarmStrategy::RoundRobin),
        _ => Err("strategy must be supervisor, dynamic, or round-robin"),
    }
}

fn parse_agent_config(object: &BTreeMap<String, JsonValue>) -> Result<AgentConfig, &'static str> {
    Ok(AgentConfig {
        name: required_string(object, "name")?,
        model: required_string(object, "model")?,
        bio: optional_string(object, "bio")?,
        lore: optional_string(object, "lore")?,
        knowledge: optional_string_array(object, "knowledge")?,
        topics: optional_string_array(object, "topics")?,
        adjectives: optional_string_array(object, "adjectives")?,
        style: optional_string(object, "style")?,
        provider: optional_string(object, "provider")?,
        system: optional_string(object, "system")?,
        tools: optional_tools(object.get("tools"))?,
        plugins: optional_plugins(object.get("plugins"))?,
        settings: optional_settings(object.get("settings"))?,
    })
}

fn required_string(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
) -> Result<String, &'static str> {
    match object.get(key) {
        Some(JsonValue::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(match key {
            "name" => "name is required",
            "model" => "model is required",
            "text" => "text is required",
            _ => "required string field is missing",
        }),
    }
}

fn optional_string(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
) -> Result<Option<String>, &'static str> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        _ => Err("optional field must be a string"),
    }
}

fn optional_string_array(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
) -> Result<Option<Vec<String>>, &'static str> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Array(values) => values
            .iter()
            .map(|value| match value {
                JsonValue::String(value) => Ok(value.clone()),
                _ => Err("string array fields must only contain strings"),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err("string array fields must be arrays of strings"),
    }
}

fn optional_tools(value: Option<&JsonValue>) -> Result<Option<Vec<ToolDescriptor>>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Array(values) => values
            .iter()
            .map(parse_tool_descriptor)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err("tools must be an array"),
    }
}

fn parse_tool_descriptor(value: &JsonValue) -> Result<ToolDescriptor, &'static str> {
    match value {
        JsonValue::String(name) if !name.is_empty() => Ok(ToolDescriptor {
            name: name.clone(),
            description: String::new(),
            parameters: BTreeMap::new(),
            examples: None,
        }),
        JsonValue::Object(object) => Ok(ToolDescriptor {
            name: required_object_string(object, "name", "tool name is required")?,
            description: optional_object_string(object, "description")?.unwrap_or_default(),
            parameters: optional_object_data_map(object.get("parameters"))?.unwrap_or_default(),
            examples: optional_tool_examples(object.get("examples"))?,
        }),
        _ => Err("tools must contain strings or objects"),
    }
}

fn optional_tool_examples(
    value: Option<&JsonValue>,
) -> Result<Option<Vec<ToolExample>>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Array(values) => values
            .iter()
            .map(parse_tool_example)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err("tool examples must be an array"),
    }
}

fn parse_tool_example(value: &JsonValue) -> Result<ToolExample, &'static str> {
    let JsonValue::Object(object) = value else {
        return Err("tool examples must contain objects");
    };

    Ok(ToolExample {
        input: required_object_string(object, "input", "tool example input is required")?,
        args: optional_object_data_map(object.get("args"))?.unwrap_or_default(),
        output: required_object_string(object, "output", "tool example output is required")?,
    })
}

fn optional_plugins(
    value: Option<&JsonValue>,
) -> Result<Option<Vec<PluginDescriptor>>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Array(values) => values
            .iter()
            .map(parse_plugin_descriptor)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err("plugins must be an array"),
    }
}

fn parse_plugin_descriptor(value: &JsonValue) -> Result<PluginDescriptor, &'static str> {
    match value {
        JsonValue::String(name) if !name.is_empty() => Ok(PluginDescriptor {
            name: name.clone(),
            description: String::new(),
        }),
        JsonValue::Object(object) => Ok(PluginDescriptor {
            name: required_object_string(object, "name", "plugin name is required")?,
            description: optional_object_string(object, "description")?.unwrap_or_default(),
        }),
        _ => Err("plugins must contain strings or objects"),
    }
}

fn required_object_string(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
    message: &'static str,
) -> Result<String, &'static str> {
    match object.get(key) {
        Some(JsonValue::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(message),
    }
}

fn optional_object_string(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
) -> Result<Option<String>, &'static str> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        _ => Err("descriptor fields must be strings"),
    }
}

fn optional_object_data_map(
    value: Option<&JsonValue>,
) -> Result<Option<BTreeMap<String, DataValue>>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Object(object) => object
            .iter()
            .map(|(key, value)| Ok((key.clone(), json_value_to_data_value(value)?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(Some),
        _ => Err("descriptor maps must be objects"),
    }
}

fn optional_settings(value: Option<&JsonValue>) -> Result<Option<AgentSettings>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    let JsonValue::Object(object) = value else {
        return Err("settings must be an object");
    };

    let mut settings = AgentSettings::default();
    for (key, value) in object {
        match key.as_str() {
            "temperature" => settings.temperature = Some(number_value(value, "temperature")?),
            "maxTokens" => settings.max_tokens = Some(u32_value(value, "maxTokens")?),
            "timeout" => settings.timeout = Some(u64_value(value, "timeout")?),
            "maxRetries" => settings.max_retries = Some(u32_value(value, "maxRetries")?),
            _ => {
                settings
                    .additional
                    .insert(key.clone(), json_value_to_data_value(value)?);
            }
        }
    }

    Ok(Some(settings))
}

fn optional_usize(value: Option<&JsonValue>) -> Result<Option<usize>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Number(number)
            if number.is_finite() && *number >= 0.0 && number.fract() == 0.0 =>
        {
            Ok(Some(*number as usize))
        }
        _ => Err("numeric fields must be positive integers"),
    }
}

fn optional_u64(value: Option<&JsonValue>) -> Result<Option<u64>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Number(number)
            if number.is_finite() && *number >= 0.0 && number.fract() == 0.0 =>
        {
            Ok(Some(*number as u64))
        }
        _ => Err("numeric fields must be positive integers"),
    }
}

fn number_value(value: &JsonValue, field: &'static str) -> Result<f64, &'static str> {
    match value {
        JsonValue::Number(number) if number.is_finite() => Ok(*number),
        _ => Err(match field {
            "temperature" => "temperature must be a number",
            _ => "field must be a number",
        }),
    }
}

fn u32_value(value: &JsonValue, field: &'static str) -> Result<u32, &'static str> {
    match value {
        JsonValue::Number(number)
            if number.is_finite() && *number >= 0.0 && number.fract() == 0.0 =>
        {
            u32::try_from(*number as u64).map_err(|_| match field {
                "maxTokens" => "maxTokens must be a positive integer",
                "maxRetries" => "maxRetries must be a positive integer",
                _ => "field must be a positive integer",
            })
        }
        _ => Err(match field {
            "maxTokens" => "maxTokens must be a positive integer",
            "maxRetries" => "maxRetries must be a positive integer",
            _ => "field must be a positive integer",
        }),
    }
}

fn u64_value(value: &JsonValue, field: &'static str) -> Result<u64, &'static str> {
    match value {
        JsonValue::Number(number)
            if number.is_finite() && *number >= 0.0 && number.fract() == 0.0 =>
        {
            Ok(*number as u64)
        }
        _ => Err(match field {
            "timeout" => "timeout must be a positive integer",
            _ => "field must be a positive integer",
        }),
    }
}

fn json_value_to_data_value(value: &JsonValue) -> Result<DataValue, &'static str> {
    match value {
        JsonValue::Null => Ok(DataValue::Null),
        JsonValue::Bool(value) => Ok(DataValue::Bool(*value)),
        JsonValue::Number(value) if value.is_finite() => Ok(DataValue::Number(*value)),
        JsonValue::Number(_) => Err("settings values must be finite"),
        JsonValue::String(value) => Ok(DataValue::String(value.clone())),
        JsonValue::Array(values) => values
            .iter()
            .map(json_value_to_data_value)
            .collect::<Result<Vec<_>, _>>()
            .map(DataValue::Array),
        JsonValue::Object(object) => object
            .iter()
            .map(|(key, value)| Ok((key.clone(), json_value_to_data_value(value)?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(DataValue::Object),
    }
}

fn swarm_state_json(state: &SwarmState) -> String {
    format!(
        "{{\"id\":\"{}\",\"status\":\"{}\",\"agentIds\":{},\"results\":{},\"tokenUsage\":{},\"startedAt\":{},\"completedAt\":{}}}",
        escape_json(&state.id),
        swarm_status_str(state.status),
        string_array_json(&state.agent_ids),
        task_results_json(&state.results),
        token_usage_json(&state.token_usage),
        optional_u128_json(state.started_at),
        optional_u128_json(state.completed_at)
    )
}

fn swarm_status_str(status: SwarmStatus) -> &'static str {
    status.as_str()
}

fn task_results_json(results: &[TaskResult<Content>]) -> String {
    format!(
        "[{}]",
        results
            .iter()
            .map(|result| task_result_json(Some(result)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn token_usage_json(usage: &TokenUsage) -> String {
    format!(
        "{{\"promptTokens\":{},\"completionTokens\":{},\"totalTokens\":{}}}",
        usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
    )
}

fn task_result_json(task: Option<&TaskResult<Content>>) -> String {
    match task {
        None => "null".to_string(),
        Some(task) => format!(
            "{{\"status\":\"{}\",\"data\":{},\"error\":{},\"durationMs\":{}}}",
            task.status.as_str(),
            content_json(task.data.as_ref()),
            optional_string_json(task.error.as_deref()),
            task.duration_ms
        ),
    }
}

fn content_json(content: Option<&Content>) -> String {
    match content {
        None => "null".to_string(),
        Some(content) => format!(
            "{{\"text\":\"{}\",\"attachments\":null,\"metadata\":{}}}",
            escape_json(&content.text),
            metadata_json(content.metadata.as_ref())
        ),
    }
}

fn metadata_json(metadata: Option<&BTreeMap<String, DataValue>>) -> String {
    match metadata {
        None => "null".to_string(),
        Some(metadata) => format!(
            "{{{}}}",
            metadata
                .iter()
                .map(|(key, value)| format!("\"{}\":{}", escape_json(key), data_value_json(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn data_value_json(value: &DataValue) -> String {
    match value {
        DataValue::Null => "null".to_string(),
        DataValue::Bool(value) => value.to_string(),
        DataValue::Number(value) => value.to_string(),
        DataValue::String(value) => format!("\"{}\"", escape_json(value)),
        DataValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(data_value_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        DataValue::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("\"{}\":{}", escape_json(key), data_value_json(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn optional_string_json(value: Option<&str>) -> String {
    match value {
        None => "null".to_string(),
        Some(value) => format!("\"{}\"", escape_json(value)),
    }
}

fn string_array_json(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("\"{}\"", escape_json(value)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn optional_u128_json(value: Option<u128>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}
