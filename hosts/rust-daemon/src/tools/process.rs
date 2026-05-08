pub(super) mod background;
pub(super) mod shell;

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;

use self::{
    background::{
        list_background_processes, read_background_process_output, start_background_process,
        stop_background_process,
    },
    shell::execute_bash_command,
};
use super::ToolExecutionContext;

pub(crate) use self::background::{
    background_process_count, new_shared_process_manager_with_limit, SharedProcessManager,
    DEFAULT_MAX_BACKGROUND_PROCESSES,
};

pub(super) fn execute_bash(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let command = match tool_call.args.get("command") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("bash command must be a non-empty string", 0),
        };

        let timeout_ms = match tool_call.args.get("timeout") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
            {
                *value as u64
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("bash timeout must be a positive integer", 0);
            }
            None => 120_000,
        };

        let cwd = match tool_call.args.get("cwd") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            Some(DataValue::String(_)) => ".".to_string(),
            Some(_) => return TaskResult::error("bash cwd must be a string", 0),
            None => ".".to_string(),
        };

        // execute_bash_command spawns a child + polls in a busy-loop on a
        // worker thread; running it directly on a tokio worker would block the
        // entire runtime. spawn_blocking moves it onto the blocking pool.
        let result = tokio::task::spawn_blocking(move || {
            execute_bash_command(&command, timeout_ms, &cwd)
        })
        .await;
        match result {
            Ok(Ok(result)) if result.status == "success" => TaskResult::success(
                Content {
                    text: result.output,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Ok(Ok(result)) => TaskResult::error(result.output, 0),
            Ok(Err(error)) => TaskResult::error(error, 0),
            Err(error) => TaskResult::error(format!("bash worker panicked: {error}"), 0),
        }
    })
}

pub(super) fn execute_bg_start(
    context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let command = match tool_call.args.get("command") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("bg_start command must be a non-empty string", 0),
        };

        let cwd = match tool_call.args.get("cwd") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            Some(DataValue::String(_)) => ".".to_string(),
            Some(_) => return TaskResult::error("bg_start cwd must be a string", 0),
            None => ".".to_string(),
        };

        match start_background_process(&context.process_manager, &command, &cwd) {
            Ok(text) => TaskResult::success(
                Content {
                    text,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

pub(super) fn execute_bg_output(
    context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let id = match tool_call.args.get("id") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("bg_output id must be a non-empty string", 0),
        };

        let all = match tool_call.args.get("all") {
            Some(DataValue::Bool(value)) => *value,
            Some(_) => return TaskResult::error("bg_output all must be a boolean", 0),
            None => false,
        };

        match read_background_process_output(&context.process_manager, &id, all) {
            Ok(text) => TaskResult::success(
                Content {
                    text,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

pub(super) fn execute_bg_stop(
    context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let id = match tool_call.args.get("id") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("bg_stop id must be a non-empty string", 0),
        };

        match stop_background_process(&context.process_manager, &id) {
            Ok(text) => TaskResult::success(
                Content {
                    text,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

pub(super) fn execute_bg_list(
    context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    _tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        match list_background_processes(&context.process_manager) {
            Ok(text) => TaskResult::success(
                Content {
                    text,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}
