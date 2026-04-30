use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use chrono::{SecondsFormat, Utc};
use futures::future::BoxFuture;

use super::ToolExecutionContext;

pub(super) fn execute_get_current_time(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    _tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        TaskResult::success(
            Content {
                text: current_time_iso_utc(),
                attachments: None,
                metadata: None,
            },
            0,
        )
    })
}

pub(super) fn execute_calculate(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let expression = match tool_call.args.get("expression") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("calculate expression must be a non-empty string", 0),
        };

        match evaluate_expression(&expression) {
            Ok(result) => TaskResult::success(
                Content {
                    text: result,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

pub(super) fn current_time_iso_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(super) fn evaluate_expression(expression: &str) -> Result<String, String> {
    meval::eval_str(expression)
        .map(|value| value.to_string())
        .map_err(|error| format!("calculate expression could not be evaluated: {error}"))
}
