use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentRuntimeSnapshot, AgentSettings, Attachment, AttachmentType, Content,
    DataValue, PluginDescriptor, TaskResult, TokenUsage, ToolDescriptor, ToolExample,
};
use anima_memory::Memory;

use crate::http::{Request, Response};
use crate::json::{escape_json, JsonParser, JsonValue};
use crate::state::DaemonState;

pub(crate) fn route_agent_request(
    request: Request,
    state: &Arc<Mutex<DaemonState>>,
) -> Option<Response> {
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/api/agents") => Some(handle_create_agent(request.body, state)),
        ("GET", "/api/agents") => Some(handle_list_agents(state)),
        _ => route_agent_path(request, state),
    }
}

fn route_agent_path(request: Request, state: &Arc<Mutex<DaemonState>>) -> Option<Response> {
    let path = request.path.strip_prefix("/api/agents/")?;
    let segments: Vec<_> = path.split('/').collect();

    match (request.method.as_str(), segments.as_slice()) {
        ("GET", [agent_id]) => Some(handle_get_agent(agent_id, state)),
        ("POST", [agent_id, "run"]) => Some(handle_run_agent(agent_id, request.body, state)),
        ("GET", [agent_id, "memories", "recent"]) => {
            Some(handle_recent_agent_memories(agent_id, request.query, state))
        }
        _ => None,
    }
}

pub(crate) fn handle_create_agent(body: Vec<u8>, state: &Arc<Mutex<DaemonState>>) -> Response {
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

    let config = match parse_agent_config(&object) {
        Ok(config) => config,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let snapshot = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        match guard.create_agent(config) {
            Ok(snapshot) => snapshot,
            Err(message) => {
                return Response::json(
                    "HTTP/1.1 400 Bad Request",
                    format!("{{\"error\":\"{}\"}}", escape_json(&message)),
                )
            }
        }
    };

    Response::json(
        "HTTP/1.1 201 Created",
        format!("{{\"agent\":{}}}", runtime_snapshot_json(&snapshot)),
    )
}

pub(crate) fn handle_list_agents(state: &Arc<Mutex<DaemonState>>) -> Response {
    let snapshots = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.list_agents()
    };

    Response::json(
        "HTTP/1.1 200 OK",
        format!(
            "{{\"agents\":[{}]}}",
            snapshots
                .iter()
                .map(runtime_snapshot_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
    )
}

pub(crate) fn handle_get_agent(agent_id: &str, state: &Arc<Mutex<DaemonState>>) -> Response {
    let snapshot = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_agent(agent_id)
    };

    match snapshot {
        Some(snapshot) => Response::json(
            "HTTP/1.1 200 OK",
            format!("{{\"agent\":{}}}", runtime_snapshot_json(&snapshot)),
        ),
        None => Response::error("HTTP/1.1 404 Not Found", "not found"),
    }
}

pub(crate) fn handle_recent_agent_memories(
    agent_id: &str,
    query: std::collections::HashMap<String, String>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let limit = match parse_optional_usize(query.get("limit").map(String::as_str)) {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let memories = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.recent_memories_for_agent(agent_id, limit)
    };

    match memories {
        Some(memories) => Response::json(
            "HTTP/1.1 200 OK",
            format!("{{\"memories\":[{}]}}", join_memories(&memories)),
        ),
        None => Response::error("HTTP/1.1 404 Not Found", "not found"),
    }
}

pub(crate) fn handle_run_agent(
    agent_id: &str,
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

    let content = match parse_content(&object) {
        Ok(content) => content,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let run_result = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.run_agent(agent_id, content)
    };

    match run_result {
        Some((snapshot, result)) => Response::json(
            "HTTP/1.1 200 OK",
            format!(
                "{{\"agent\":{},\"result\":{}}}",
                runtime_snapshot_json(&snapshot),
                task_result_json(Some(&result))
            ),
        ),
        None => Response::error("HTTP/1.1 404 Not Found", "not found"),
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

fn parse_content(object: &BTreeMap<String, JsonValue>) -> Result<Content, &'static str> {
    Ok(Content {
        text: required_string(object, "text")?,
        attachments: optional_attachments(object.get("attachments"))?,
        metadata: optional_metadata(object.get("metadata"))?,
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

fn optional_attachments(
    value: Option<&JsonValue>,
) -> Result<Option<Vec<Attachment>>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Array(values) => values
            .iter()
            .map(parse_attachment)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err("attachments must be an array"),
    }
}

fn parse_attachment(value: &JsonValue) -> Result<Attachment, &'static str> {
    let JsonValue::Object(object) = value else {
        return Err("attachments must contain objects");
    };

    let attachment_type = match object.get("type") {
        Some(JsonValue::String(value)) => match value.as_str() {
            "file" => AttachmentType::File,
            "image" => AttachmentType::Image,
            "url" => AttachmentType::Url,
            _ => return Err("attachment type must be file, image, or url"),
        },
        _ => return Err("attachment type is required"),
    };
    let name = match object.get("name") {
        Some(JsonValue::String(value)) if !value.is_empty() => value.clone(),
        _ => return Err("attachment name is required"),
    };
    let data = match object.get("data") {
        Some(JsonValue::String(value)) => value.clone(),
        _ => return Err("attachment data is required"),
    };

    Ok(Attachment {
        attachment_type,
        name,
        data,
    })
}

fn optional_metadata(
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
        _ => Err("metadata must be an object"),
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

fn parse_optional_usize(value: Option<&str>) -> Result<Option<usize>, &'static str> {
    value
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer")
        })
        .transpose()
}

fn runtime_snapshot_json(snapshot: &AgentRuntimeSnapshot) -> String {
    format!(
        "{{\"state\":{},\"messageCount\":{},\"eventCount\":{},\"lastTask\":{}}}",
        agent_state_json(&snapshot.state),
        snapshot.message_count,
        snapshot.event_count,
        task_result_json(snapshot.last_task.as_ref())
    )
}

fn agent_state_json(state: &anima_core::AgentState) -> String {
    format!(
        "{{\"id\":\"{}\",\"name\":\"{}\",\"status\":\"{}\",\"config\":{},\"createdAt\":{},\"tokenUsage\":{}}}",
        escape_json(&state.id),
        escape_json(&state.name),
        state.status.as_str(),
        agent_config_json(&state.config),
        state.created_at,
        token_usage_json(&state.token_usage)
    )
}

fn agent_config_json(config: &AgentConfig) -> String {
    format!(
        "{{\"name\":\"{}\",\"model\":\"{}\",\"bio\":{},\"lore\":{},\"knowledge\":{},\"topics\":{},\"adjectives\":{},\"style\":{},\"provider\":{},\"system\":{},\"tools\":{},\"plugins\":{},\"settings\":{}}}",
        escape_json(&config.name),
        escape_json(&config.model),
        optional_string_json(config.bio.as_deref()),
        optional_string_json(config.lore.as_deref()),
        optional_string_array_json(config.knowledge.as_deref()),
        optional_string_array_json(config.topics.as_deref()),
        optional_string_array_json(config.adjectives.as_deref()),
        optional_string_json(config.style.as_deref()),
        optional_string_json(config.provider.as_deref()),
        optional_string_json(config.system.as_deref()),
        tools_json(config.tools.as_deref()),
        plugins_json(config.plugins.as_deref()),
        settings_json(config.settings.as_ref())
    )
}

fn settings_json(settings: Option<&AgentSettings>) -> String {
    let Some(settings) = settings else {
        return "null".to_string();
    };

    let mut fields = Vec::new();
    if let Some(value) = settings.temperature {
        fields.push(format!("\"temperature\":{value}"));
    }
    if let Some(value) = settings.max_tokens {
        fields.push(format!("\"maxTokens\":{value}"));
    }
    if let Some(value) = settings.timeout {
        fields.push(format!("\"timeout\":{value}"));
    }
    if let Some(value) = settings.max_retries {
        fields.push(format!("\"maxRetries\":{value}"));
    }
    for (key, value) in &settings.additional {
        fields.push(format!(
            "\"{}\":{}",
            escape_json(key),
            data_value_json(value)
        ));
    }

    format!("{{{}}}", fields.join(","))
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
            "{{\"text\":\"{}\",\"attachments\":{},\"metadata\":{}}}",
            escape_json(&content.text),
            attachments_json(content.attachments.as_deref()),
            metadata_json(content.metadata.as_ref())
        ),
    }
}

fn attachments_json(attachments: Option<&[Attachment]>) -> String {
    match attachments {
        None => "null".to_string(),
        Some(attachments) => format!(
            "[{}]",
            attachments
                .iter()
                .map(|attachment| {
                    format!(
                        "{{\"type\":\"{}\",\"name\":\"{}\",\"data\":\"{}\"}}",
                        attachment_type_str(attachment.attachment_type),
                        escape_json(&attachment.name),
                        escape_json(&attachment.data)
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
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

fn attachment_type_str(value: AttachmentType) -> &'static str {
    match value {
        AttachmentType::File => "file",
        AttachmentType::Image => "image",
        AttachmentType::Url => "url",
    }
}

fn optional_string_json(value: Option<&str>) -> String {
    match value {
        None => "null".to_string(),
        Some(value) => format!("\"{}\"", escape_json(value)),
    }
}

fn optional_string_array_json(values: Option<&[String]>) -> String {
    match values {
        None => "null".to_string(),
        Some(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| format!("\"{}\"", escape_json(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn tools_json(values: Option<&[ToolDescriptor]>) -> String {
    match values {
        None => "null".to_string(),
        Some(values) => format!(
            "[{}]",
            values.iter().map(tool_json).collect::<Vec<_>>().join(",")
        ),
    }
}

fn tool_json(tool: &ToolDescriptor) -> String {
    format!(
        "{{\"name\":\"{}\",\"description\":{},\"parameters\":{},\"examples\":{}}}",
        escape_json(&tool.name),
        optional_string_json(Some(&tool.description)),
        data_value_json(&DataValue::Object(tool.parameters.clone())),
        tool_examples_json(tool.examples.as_deref())
    )
}

fn tool_examples_json(examples: Option<&[ToolExample]>) -> String {
    match examples {
        None => "null".to_string(),
        Some(examples) => format!(
            "[{}]",
            examples
                .iter()
                .map(|example| {
                    format!(
                        "{{\"input\":\"{}\",\"args\":{},\"output\":\"{}\"}}",
                        escape_json(&example.input),
                        data_value_json(&DataValue::Object(example.args.clone())),
                        escape_json(&example.output)
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn plugins_json(values: Option<&[PluginDescriptor]>) -> String {
    match values {
        None => "null".to_string(),
        Some(values) => format!(
            "[{}]",
            values.iter().map(plugin_json).collect::<Vec<_>>().join(",")
        ),
    }
}

fn plugin_json(plugin: &PluginDescriptor) -> String {
    format!(
        "{{\"name\":\"{}\",\"description\":{}}}",
        escape_json(&plugin.name),
        optional_string_json(Some(&plugin.description))
    )
}

fn join_memories(memories: &[Memory]) -> String {
    memories
        .iter()
        .map(memory_json)
        .collect::<Vec<_>>()
        .join(",")
}

fn memory_json(memory: &Memory) -> String {
    format!(
        "{{\"id\":\"{}\",\"agentId\":\"{}\",\"agentName\":\"{}\",\"type\":\"{}\",\"content\":\"{}\",\"importance\":{},\"createdAt\":{},\"tags\":{}}}",
        escape_json(&memory.id),
        escape_json(&memory.agent_id),
        escape_json(&memory.agent_name),
        memory.memory_type.as_str(),
        escape_json(&memory.content),
        memory.importance,
        memory.created_at,
        match memory.tags.as_deref() {
            None => "null".to_string(),
            Some(tags) => format!(
                "[{}]",
                tags.iter()
                    .map(|tag| format!("\"{}\"", escape_json(tag)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    )
}
