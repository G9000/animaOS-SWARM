mod dispatch;
mod render;

#[cfg(test)]
mod tests;

use anima_core::{
    AgentConfig, Content, MessageRole, ModelAdapter, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, TokenUsage, ToolCall,
};
use async_trait::async_trait;

use self::dispatch::{deterministic_tool_dispatch, DeterministicToolDispatch};
use self::render::{recent_memory_context, render_tool_result_for_model, trailing_tool_messages};

pub(crate) struct DeterministicModelAdapter;

#[async_trait]
impl ModelAdapter for DeterministicModelAdapter {
    fn provider(&self) -> &str {
        "deterministic"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let prompt_tokens = prompt_token_count(request);
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
            return Ok(text_response(output_text, prompt_tokens));
        }

        if input.eq_ignore_ascii_case("recall context") {
            if let Some(context) = recent_context {
                let output_text = format!("{} recalled context: {}", config.name, context);
                return Ok(text_response(output_text, prompt_tokens));
            }
        }

        if let Some(dispatch) = deterministic_tool_dispatch(config, input) {
            return Ok(tool_call_response(dispatch, prompt_tokens, input));
        }

        let output_text = format!("{} handled task: {}", config.name, input);
        Ok(text_response(output_text, prompt_tokens))
    }
}

fn count_tokens(value: &str) -> u64 {
    let count = value.split_whitespace().count() as u64;
    count.max(1)
}

fn prompt_token_count(request: &ModelGenerateRequest) -> u64 {
    count_tokens(&request.system)
        + request
            .messages
            .iter()
            .map(|message| count_tokens(&message.content.text))
            .sum::<u64>()
}

fn text_response(output_text: String, prompt_tokens: u64) -> ModelGenerateResponse {
    let completion_tokens = count_tokens(&output_text);

    ModelGenerateResponse {
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
    }
}

fn tool_call_response(
    dispatch: DeterministicToolDispatch,
    prompt_tokens: u64,
    input: &str,
) -> ModelGenerateResponse {
    ModelGenerateResponse {
        content: Content {
            text: dispatch.response_text.into(),
            attachments: None,
            metadata: None,
        },
        tool_calls: Some(vec![ToolCall {
            id: format!("{}{}", dispatch.id_prefix, count_tokens(input)),
            name: dispatch.name.into(),
            args: dispatch.args,
        }]),
        usage: TokenUsage {
            prompt_tokens,
            completion_tokens: 1,
            total_tokens: prompt_tokens + 1,
        },
        stop_reason: ModelStopReason::ToolCall,
    }
}
