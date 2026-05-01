use anima_core::{DataValue, Message, MessageRole};

pub(super) fn recent_memory_context(system: &str) -> Option<String> {
    context_line(system, "recent_memories", "no recent memories")
}

pub(super) fn swarm_inbox_context(system: &str) -> Option<String> {
    context_line(system, "swarm_inbox", "no swarm messages")
}

fn context_line(system: &str, name: &str, empty_value: &str) -> Option<String> {
    let prefix = format!("[{name}]: ");
    system
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .filter(|value| !value.is_empty() && *value != empty_value)
        .map(ToString::to_string)
}

pub(super) fn trailing_tool_messages(messages: &[Message]) -> Vec<&Message> {
    let mut trailing = messages
        .iter()
        .rev()
        .take_while(|message| matches!(message.role, MessageRole::Tool))
        .collect::<Vec<_>>();
    trailing.reverse();
    trailing
}

pub(super) fn render_tool_result_for_model(message: &Message) -> String {
    let Some(task_result) = message
        .content
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("taskResult"))
    else {
        return message.content.text.clone();
    };

    match task_result {
        DataValue::Object(task_result) => {
            let status = task_result.get("status");
            let data_text = task_result.get("data").and_then(task_result_content_text);

            if matches!(status, Some(DataValue::String(value)) if value == "success") {
                if let Some(text) = data_text {
                    return text.to_string();
                }
            }

            data_value_json(&DataValue::Object(task_result.clone()))
        }
        _ => message.content.text.clone(),
    }
}

fn task_result_content_text(value: &DataValue) -> Option<&str> {
    match value {
        DataValue::Object(content) => match content.get("text") {
            Some(DataValue::String(text)) => Some(text.as_str()),
            _ => None,
        },
        _ => None,
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

fn escape_json(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", u32::from(character)))
            }
            character => escaped.push(character),
        }
    }
    escaped
}
