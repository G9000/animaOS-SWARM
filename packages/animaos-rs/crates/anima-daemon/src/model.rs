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
        let last_tool_message = request
            .messages
            .iter()
            .rev()
            .find(|message| matches!(message.role, MessageRole::Tool))
            .map(|message| message.content.text.as_str());
        let input = request
            .messages
            .iter()
            .rev()
            .find(|message| matches!(message.role, MessageRole::User))
            .map(|message| message.content.text.as_str())
            .unwrap_or("");

        if let Some(tool_result) = last_tool_message {
            let output_text = format!("{} handled task: {tool_result}", config.name);
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

        if config
            .tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool.name == "memory_search"))
        {
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
