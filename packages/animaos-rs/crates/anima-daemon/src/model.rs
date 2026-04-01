use anima_core::{
    AgentConfig, Content, DataValue, MessageRole, ModelAdapter, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, TokenUsage, ToolCall,
};
use std::collections::BTreeMap;

pub(crate) struct DeterministicModelAdapter;

impl ModelAdapter for DeterministicModelAdapter {
    fn provider(&self) -> &str {
        "deterministic"
    }

    fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let tool_results = trailing_tool_messages(&request.messages)
            .into_iter()
            .map(render_tool_result_for_model)
            .collect::<Vec<_>>();
        let input = request
            .messages
            .iter()
            .rev()
            .find(|message| matches!(message.role, MessageRole::User))
            .map(|message| message.content.text.as_str())
            .unwrap_or("");
        let recent_context = recent_memory_context(&request.system);

        if !tool_results.is_empty() {
            let output_text = format!("{} handled task: {}", config.name, tool_results.join("\n"));
            let prompt_tokens = count_tokens(&request.system)
                + request
                    .messages
                    .iter()
                    .map(|message| count_tokens(&message.content.text))
                    .sum::<u64>();
            let completion_tokens = count_tokens(&output_text);

            return Ok(ModelGenerateResponse {
                content: Content {
                    text: output_text,
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens: prompt_tokens + completion_tokens,
                },
                stop_reason: ModelStopReason::End,
            });
        }

        if input.eq_ignore_ascii_case("recall context") {
            if let Some(context) = recent_context {
                let output_text = format!("{} recalled context: {}", config.name, context);
                let prompt_tokens = count_tokens(&request.system)
                    + request
                        .messages
                        .iter()
                        .map(|message| count_tokens(&message.content.text))
                        .sum::<u64>();
                let completion_tokens = count_tokens(&output_text);

                return Ok(ModelGenerateResponse {
                    content: Content {
                        text: output_text,
                        attachments: None,
                        metadata: None,
                    },
                    tool_calls: None,
                    usage: TokenUsage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                    stop_reason: ModelStopReason::End,
                });
            }
        }

        if has_tool(config, "memory_add") {
            if let Some(content) = input
                .strip_prefix("remember ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let mut args = BTreeMap::new();
                args.insert("content".into(), DataValue::String(content.to_string()));
                args.insert("type".into(), DataValue::String("fact".into()));
                args.insert("importance".into(), DataValue::Number(0.8));

                let prompt_tokens = count_tokens(&request.system)
                    + request
                        .messages
                        .iter()
                        .map(|message| count_tokens(&message.content.text))
                        .sum::<u64>();

                return Ok(ModelGenerateResponse {
                    content: Content {
                        text: "storing memory".into(),
                        attachments: None,
                        metadata: None,
                    },
                    tool_calls: Some(vec![ToolCall {
                        id: format!("tool-call-add-{}", count_tokens(input)),
                        name: "memory_add".into(),
                        args,
                    }]),
                    usage: TokenUsage {
                        prompt_tokens,
                        completion_tokens: 1,
                        total_tokens: prompt_tokens + 1,
                    },
                    stop_reason: ModelStopReason::ToolCall,
                });
            }
        }

        if has_tool(config, "recent_memories") {
            if let Some(limit) = parse_recent_limit(input) {
                let mut args = BTreeMap::new();
                args.insert("limit".into(), DataValue::Number(limit as f64));

                let prompt_tokens = count_tokens(&request.system)
                    + request
                        .messages
                        .iter()
                        .map(|message| count_tokens(&message.content.text))
                        .sum::<u64>();

                return Ok(ModelGenerateResponse {
                    content: Content {
                        text: "loading recent memories".into(),
                        attachments: None,
                        metadata: None,
                    },
                    tool_calls: Some(vec![ToolCall {
                        id: format!("tool-call-recent-{}", count_tokens(input)),
                        name: "recent_memories".into(),
                        args,
                    }]),
                    usage: TokenUsage {
                        prompt_tokens,
                        completion_tokens: 1,
                        total_tokens: prompt_tokens + 1,
                    },
                    stop_reason: ModelStopReason::ToolCall,
                });
            }
        }

        if has_tool(config, "memory_search") {
            let mut args = BTreeMap::new();
            args.insert("query".into(), DataValue::String(input.to_string()));

            let prompt_tokens = count_tokens(&request.system)
                + request
                    .messages
                    .iter()
                    .map(|message| count_tokens(&message.content.text))
                    .sum::<u64>();

            return Ok(ModelGenerateResponse {
                content: Content {
                    text: "searching memories".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: Some(vec![ToolCall {
                    id: format!("tool-call-{}", count_tokens(input)),
                    name: "memory_search".into(),
                    args,
                }]),
                usage: TokenUsage {
                    prompt_tokens,
                    completion_tokens: 1,
                    total_tokens: prompt_tokens + 1,
                },
                stop_reason: ModelStopReason::ToolCall,
            });
        }

        let output_text = format!("{} handled task: {}", config.name, input);
        let prompt_tokens = count_tokens(&request.system)
            + request
                .messages
                .iter()
                .map(|message| count_tokens(&message.content.text))
                .sum::<u64>();
        let completion_tokens = count_tokens(&output_text);

        Ok(ModelGenerateResponse {
            content: Content {
                text: output_text,
                attachments: None,
                metadata: None,
            },
            tool_calls: None,
            usage: TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            },
            stop_reason: ModelStopReason::End,
        })
    }
}

fn count_tokens(value: &str) -> u64 {
    let count = value.split_whitespace().count() as u64;
    count.max(1)
}

fn has_tool(config: &AgentConfig, tool_name: &str) -> bool {
    config
        .tools
        .as_ref()
        .is_some_and(|tools| tools.iter().any(|tool| tool.name == tool_name))
}

fn parse_recent_limit(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if !trimmed.starts_with("recent") {
        return None;
    }

    let suffix = trimmed["recent".len()..].trim();
    if suffix.is_empty() {
        return Some(3);
    }

    suffix.parse::<u64>().ok().filter(|value| *value > 0)
}

fn recent_memory_context(system: &str) -> Option<String> {
    system
        .lines()
        .find_map(|line| line.strip_prefix("[recent_memories]: "))
        .filter(|value| !value.is_empty() && *value != "no recent memories")
        .map(ToString::to_string)
}

fn trailing_tool_messages(messages: &[anima_core::Message]) -> Vec<&anima_core::Message> {
    let mut trailing = messages
        .iter()
        .rev()
        .take_while(|message| matches!(message.role, MessageRole::Tool))
        .collect::<Vec<_>>();
    trailing.reverse();
    trailing
}

fn render_tool_result_for_model(message: &anima_core::Message) -> String {
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

#[cfg(test)]
mod tests {
    use super::DeterministicModelAdapter;
    use anima_core::{
        AgentConfig, Content, DataValue, Message, MessageRole, ModelAdapter, ModelGenerateRequest,
        TaskResult,
    };
    use std::collections::BTreeMap;

    #[test]
    fn deterministic_adapter_aggregates_trailing_tool_messages() {
        let adapter = DeterministicModelAdapter;
        let request = ModelGenerateRequest {
            system: "You are helpful".into(),
            messages: vec![
                message("msg-1", "room-1", MessageRole::User, "search both"),
                message(
                    "msg-2",
                    "room-1",
                    MessageRole::Assistant,
                    "searching memories",
                ),
                tool_message(
                    "msg-3",
                    "room-1",
                    "alpha result",
                    TaskResult::success(content("alpha result"), 1),
                ),
                tool_message(
                    "msg-4",
                    "room-1",
                    "beta result",
                    TaskResult::success(content("beta result"), 2),
                ),
            ],
            temperature: None,
            max_tokens: None,
        };

        let response = adapter
            .generate(&config_with_memory_search(), &request)
            .expect("adapter should generate");

        assert!(
            response.content.text.contains("alpha result"),
            "response should include the first tool result: {}",
            response.content.text
        );
        assert!(
            response.content.text.contains("beta result"),
            "response should include the second tool result: {}",
            response.content.text
        );
    }

    fn config_with_memory_search() -> AgentConfig {
        AgentConfig {
            name: "reviewer".into(),
            model: "gpt-5.4".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: Some("openai".into()),
            system: None,
            tools: Some(vec![anima_core::ToolDescriptor {
                name: "memory_search".into(),
                description: "Search memories".into(),
                parameters: BTreeMap::new(),
                examples: None,
            }]),
            plugins: None,
            settings: None,
        }
    }

    fn message(id: &str, room_id: &str, role: MessageRole, text: &str) -> Message {
        Message {
            id: id.into(),
            agent_id: "agent-1".into(),
            room_id: room_id.into(),
            content: content(text),
            role,
            created_at: 1,
        }
    }

    fn tool_message(id: &str, room_id: &str, text: &str, result: TaskResult<Content>) -> Message {
        let mut metadata = BTreeMap::new();
        metadata.insert("toolCallId".into(), DataValue::String(id.into()));
        metadata.insert("taskResult".into(), task_result_data_value(&result));

        Message {
            id: id.into(),
            agent_id: "agent-1".into(),
            room_id: room_id.into(),
            content: Content {
                text: text.into(),
                attachments: None,
                metadata: Some(metadata),
            },
            role: MessageRole::Tool,
            created_at: 1,
        }
    }

    fn content(text: &str) -> Content {
        Content {
            text: text.into(),
            attachments: None,
            metadata: None,
        }
    }

    fn task_result_data_value(result: &TaskResult<Content>) -> DataValue {
        let mut value = BTreeMap::new();
        value.insert(
            "status".into(),
            DataValue::String(result.status.as_str().to_string()),
        );
        value.insert(
            "data".into(),
            match result.data.as_ref() {
                Some(content) => {
                    let mut content_value = BTreeMap::new();
                    content_value.insert("text".into(), DataValue::String(content.text.clone()));
                    content_value.insert("attachments".into(), DataValue::Null);
                    content_value.insert("metadata".into(), DataValue::Null);
                    DataValue::Object(content_value)
                }
                None => DataValue::Null,
            },
        );
        value.insert(
            "error".into(),
            result
                .error
                .as_ref()
                .map(|error| DataValue::String(error.clone()))
                .unwrap_or(DataValue::Null),
        );
        value.insert(
            "durationMs".into(),
            DataValue::Number(result.duration_ms as f64),
        );
        DataValue::Object(value)
    }
}
