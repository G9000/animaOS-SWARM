use std::collections::BTreeMap;

use anima_core::{
    AgentConfig, AgentSettings, Attachment, AttachmentType, Content, DataValue, PluginDescriptor,
    TaskResult, TokenUsage, ToolDescriptor, ToolExample,
};

use crate::json::{escape_json, JsonValue};

pub(crate) fn parse_agent_config(
    object: &BTreeMap<String, JsonValue>,
) -> Result<AgentConfig, &'static str> {
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

pub(crate) fn parse_content(object: &BTreeMap<String, JsonValue>) -> Result<Content, &'static str> {
    Ok(Content {
        text: required_text_or_task(object)?,
        attachments: optional_attachments(object.get("attachments"))?,
        metadata: optional_metadata(object.get("metadata"))?,
    })
}

pub(crate) fn required_text_or_task(
    object: &BTreeMap<String, JsonValue>,
) -> Result<String, &'static str> {
    if let Some(JsonValue::String(value)) = object.get("text").filter(|value| match value {
        JsonValue::String(value) => !value.is_empty(),
        _ => false,
    }) {
        return Ok(value.clone());
    }

    if let Some(JsonValue::String(value)) = object.get("task").filter(|value| match value {
        JsonValue::String(value) => !value.is_empty(),
        _ => false,
    }) {
        return Ok(value.clone());
    }

    Err("text is required")
}

pub(crate) fn required_string(
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

pub(crate) fn optional_string(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
) -> Result<Option<String>, &'static str> {
    match object.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        _ => Err("optional field must be a string"),
    }
}

pub(crate) fn optional_string_array(
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

pub(crate) fn optional_tools(
    value: Option<&JsonValue>,
) -> Result<Option<Vec<ToolDescriptor>>, &'static str> {
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

pub(crate) fn optional_plugins(
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

pub(crate) fn optional_settings(
    value: Option<&JsonValue>,
) -> Result<Option<AgentSettings>, &'static str> {
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

pub(crate) fn optional_attachments(
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

pub(crate) fn optional_metadata(
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

pub(crate) fn optional_usize(value: Option<&JsonValue>) -> Result<Option<usize>, &'static str> {
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

pub(crate) fn optional_u64(value: Option<&JsonValue>) -> Result<Option<u64>, &'static str> {
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

pub(crate) fn json_value_to_data_value(value: &JsonValue) -> Result<DataValue, &'static str> {
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

pub(crate) fn token_usage_json(usage: &TokenUsage) -> String {
    format!(
        "{{\"promptTokens\":{},\"completionTokens\":{},\"totalTokens\":{}}}",
        usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
    )
}

pub(crate) fn task_result_json(task: Option<&TaskResult<Content>>) -> String {
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

pub(crate) fn content_json(content: Option<&Content>) -> String {
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

pub(crate) fn data_value_json(value: &DataValue) -> String {
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

pub(crate) fn optional_string_json(value: Option<&str>) -> String {
    match value {
        None => "null".to_string(),
        Some(value) => format!("\"{}\"", escape_json(value)),
    }
}

pub(crate) fn optional_string_array_json(values: Option<&[String]>) -> String {
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

pub(crate) fn string_array_json(values: &[String]) -> String {
    optional_string_array_json(Some(values))
}

pub(crate) fn tools_json(values: Option<&[ToolDescriptor]>) -> String {
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

pub(crate) fn plugins_json(values: Option<&[PluginDescriptor]>) -> String {
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
