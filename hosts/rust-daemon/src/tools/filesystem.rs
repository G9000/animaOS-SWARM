pub(super) mod edit;
pub(super) mod search;

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;

use super::ToolExecutionContext;
use self::{
    edit::{edit_workspace_file, multi_edit_workspace_file, write_workspace_file},
    search::{glob_workspace_paths, grep_workspace_files, list_workspace_dir, read_workspace_file},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileEditOperation {
    pub(super) old_string: String,
    pub(super) new_string: String,
}

pub(super) fn execute_read_file(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let file_path = match tool_call.args.get("file_path") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("read_file file_path must be a non-empty string", 0),
        };

        let offset = match tool_call.args.get("offset") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 =>
            {
                *value as usize
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("read_file offset must be a non-negative integer", 0);
            }
            None => 0,
        };

        let limit = match tool_call.args.get("limit") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 =>
            {
                *value as usize
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("read_file limit must be a non-negative integer", 0);
            }
            None => 2_000,
        };

        match read_workspace_file(&file_path, offset, limit) {
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

pub(super) fn execute_list_dir(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let path = match tool_call.args.get("path") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("list_dir path must be a non-empty string", 0),
        };

        match list_workspace_dir(&path) {
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

pub(super) fn execute_glob(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let pattern = match tool_call.args.get("pattern") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("glob pattern must be a non-empty string", 0),
        };

        let path = match tool_call.args.get("path") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            Some(DataValue::String(_)) => ".".to_string(),
            Some(_) => return TaskResult::error("glob path must be a string", 0),
            None => ".".to_string(),
        };

        match glob_workspace_paths(&pattern, &path) {
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

pub(super) fn execute_grep(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let pattern = match tool_call.args.get("pattern") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("grep pattern must be a non-empty string", 0),
        };

        let path = match tool_call.args.get("path") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            Some(DataValue::String(_)) => ".".to_string(),
            Some(_) => return TaskResult::error("grep path must be a string", 0),
            None => ".".to_string(),
        };

        let include = match tool_call.args.get("include") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            Some(DataValue::String(_)) => None,
            Some(_) => return TaskResult::error("grep include must be a string", 0),
            None => None,
        };

        match grep_workspace_files(&pattern, &path, include.as_deref()) {
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

pub(super) fn execute_write_file(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let file_path = match tool_call.args.get("file_path") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("write_file file_path must be a non-empty string", 0),
        };

        let content = match tool_call.args.get("content") {
            Some(DataValue::String(value)) => value.clone(),
            _ => return TaskResult::error("write_file content must be a string", 0),
        };

        match write_workspace_file(&file_path, &content) {
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

pub(super) fn execute_edit_file(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let file_path = match tool_call.args.get("file_path") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("edit_file file_path must be a non-empty string", 0),
        };

        let old_string = match tool_call.args.get("old_string") {
            Some(DataValue::String(value)) => value.clone(),
            _ => return TaskResult::error("edit_file old_string must be a string", 0),
        };

        let new_string = match tool_call.args.get("new_string") {
            Some(DataValue::String(value)) => value.clone(),
            _ => return TaskResult::error("edit_file new_string must be a string", 0),
        };

        match edit_workspace_file(&file_path, &old_string, &new_string) {
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

pub(super) fn execute_multi_edit(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let file_path = match tool_call.args.get("file_path") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("multi_edit file_path must be a non-empty string", 0),
        };

        let edits = match tool_call.args.get("edits") {
            Some(DataValue::Array(values)) if values.is_empty() => {
                return TaskResult::error("No edits provided", 0);
            }
            Some(DataValue::Array(values)) => {
                let mut edits = Vec::with_capacity(values.len());
                for value in values {
                    let DataValue::Object(object) = value else {
                        return TaskResult::error(
                            "multi_edit edits must be objects with old_string and new_string",
                            0,
                        );
                    };

                    let old_string = match object.get("old_string") {
                        Some(DataValue::String(value)) => value.clone(),
                        _ => {
                            return TaskResult::error(
                                "multi_edit edits must include string old_string values",
                                0,
                            )
                        }
                    };
                    let new_string = match object.get("new_string") {
                        Some(DataValue::String(value)) => value.clone(),
                        _ => {
                            return TaskResult::error(
                                "multi_edit edits must include string new_string values",
                                0,
                            )
                        }
                    };

                    edits.push(FileEditOperation {
                        old_string,
                        new_string,
                    });
                }
                edits
            }
            _ => return TaskResult::error("multi_edit edits must be a non-empty array", 0),
        };

        match multi_edit_workspace_file(&file_path, &edits) {
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
