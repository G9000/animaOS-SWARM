use std::fs;
use std::path::{Path, PathBuf};

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use super::workspace::{canonical_workspace_root, workspace_root_path};
use super::ToolExecutionContext;

const TODO_DIRECTORY_NAME: &str = ".animaos-swarm";
const TODO_FILE_NAME: &str = "todos.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TodoItem {
    pub(super) content: String,
    pub(super) status: String,
    #[serde(rename = "activeForm")]
    pub(super) active_form: String,
}

pub(super) fn execute_todo_write(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let todos = match tool_call.args.get("todos") {
            Some(DataValue::Array(values)) => {
                let mut todos = Vec::with_capacity(values.len());
                for (index, value) in values.iter().enumerate() {
                    match parse_todo_item(value, index) {
                        Ok(todo) => todos.push(todo),
                        Err(error) => return TaskResult::error(error, 0),
                    }
                }
                todos
            }
            Some(_) => return TaskResult::error("todo_write todos must be an array", 0),
            None => return TaskResult::error("todo_write todos is required", 0),
        };

        match write_todo_list(&todos) {
            Ok(message) => TaskResult::success(
                Content {
                    text: message,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

pub(super) fn execute_todo_read(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    _tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        match read_todo_list() {
            Ok(message) => TaskResult::success(
                Content {
                    text: message,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

fn write_todo_list(todos: &[TodoItem]) -> Result<String, String> {
    let workspace_root = workspace_root_path("todo_write")?;
    write_todo_list_from_root(&workspace_root, todos)
}

pub(super) fn write_todo_list_from_root(
    workspace_root: &Path,
    todos: &[TodoItem],
) -> Result<String, String> {
    let warnings = validate_todo_items(todos)?;
    let todo_file = todo_file_path_from_root(workspace_root, "todo_write")?;
    if let Some(parent) = todo_file.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "todo_write failed to create todo directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let serialized = serde_json::to_string_pretty(todos)
        .map_err(|error| format!("todo_write failed to serialize todos: {error}"))?;
    fs::write(&todo_file, serialized)
        .map_err(|error| format!("todo_write failed to persist todo list: {error}"))?;

    let completed = todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .count();
    let in_progress = todos
        .iter()
        .filter(|todo| todo.status == "in_progress")
        .count();
    let pending = todos.iter().filter(|todo| todo.status == "pending").count();
    let mut message = format!(
        "Todos updated ({} completed, {} in progress, {} pending).",
        completed, in_progress, pending
    );
    if !warnings.is_empty() {
        message.push(' ');
        message.push_str(&warnings.join(" "));
    }
    message.push_str(" Proceed with current tasks.");

    Ok(message)
}

fn read_todo_list() -> Result<String, String> {
    let workspace_root = workspace_root_path("todo_read")?;
    read_todo_list_from_root(&workspace_root)
}

pub(super) fn read_todo_list_from_root(workspace_root: &Path) -> Result<String, String> {
    let todos = load_todo_items_from_root(workspace_root, "todo_read")?;
    if todos.is_empty() {
        return Ok("No todos set.".to_string());
    }

    Ok(todos
        .iter()
        .enumerate()
        .map(|(index, todo)| {
            let icon = match todo.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[>]",
                _ => "[ ]",
            };
            format!("{} {}. [{}] {}", icon, index + 1, todo.status, todo.content)
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn load_todo_items_from_root(
    workspace_root: &Path,
    tool_name: &str,
) -> Result<Vec<TodoItem>, String> {
    let todo_file = todo_file_path_from_root(workspace_root, tool_name)?;
    if !todo_file.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&todo_file)
        .map_err(|error| format!("{tool_name} failed to read todo list: {error}"))?;
    match serde_json::from_str::<Vec<TodoItem>>(&content) {
        Ok(todos) => {
            validate_todo_items(&todos)?;
            Ok(todos)
        }
        Err(_) => Ok(Vec::new()),
    }
}

fn validate_todo_items(todos: &[TodoItem]) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    let mut in_progress = 0usize;

    for (index, todo) in todos.iter().enumerate() {
        if todo.content.trim().is_empty() {
            return Err(format!(
                "todos[{index}]: content must be a non-empty string"
            ));
        }
        if !matches!(
            todo.status.as_str(),
            "pending" | "in_progress" | "completed"
        ) {
            return Err(format!(
                "todos[{index}]: status must be pending | in_progress | completed"
            ));
        }
        if todo.active_form.trim().is_empty() {
            return Err(format!(
                "todos[{index}]: activeForm must be a non-empty string"
            ));
        }
        if todo.status == "in_progress" {
            in_progress += 1;
        }
    }

    if in_progress > 1 {
        warnings.push(format!(
            "Warning: {in_progress} todos are in_progress -- ideally only one at a time."
        ));
    }

    Ok(warnings)
}

fn parse_todo_item(value: &DataValue, index: usize) -> Result<TodoItem, String> {
    let DataValue::Object(fields) = value else {
        return Err(format!("todos[{index}] must be an object"));
    };

    let content = match fields.get("content") {
        Some(DataValue::String(value)) if !value.trim().is_empty() => value.clone(),
        Some(DataValue::String(_)) | Some(_) | None => {
            return Err(format!(
                "todos[{index}]: content must be a non-empty string"
            ));
        }
    };
    let status = match fields.get("status") {
        Some(DataValue::String(value)) if !value.trim().is_empty() => value.clone(),
        Some(DataValue::String(_)) | Some(_) | None => {
            return Err(format!(
                "todos[{index}]: status must be pending | in_progress | completed"
            ));
        }
    };
    let active_form = match fields.get("activeForm") {
        Some(DataValue::String(value)) if !value.trim().is_empty() => value.clone(),
        Some(DataValue::String(_)) | Some(_) | None => {
            return Err(format!(
                "todos[{index}]: activeForm must be a non-empty string"
            ));
        }
    };

    Ok(TodoItem {
        content,
        status,
        active_form,
    })
}

pub(super) fn todo_file_path_from_root(
    workspace_root: &Path,
    tool_name: &str,
) -> Result<PathBuf, String> {
    let canonical_root = canonical_workspace_root(workspace_root, tool_name)?;
    Ok(canonical_root
        .join(TODO_DIRECTORY_NAME)
        .join(TODO_FILE_NAME))
}
