use std::fs;
use std::path::{Path, PathBuf};

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use super::workspace::{canonical_workspace_root, workspace_root_path};
use super::{ctx_workspace_root, ToolExecutionContext};

const TODO_DIRECTORY_NAME: &str = ".animaos-swarm";
const TODO_FILE_NAME: &str = "todos.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub(crate) struct TodoItem {
    pub(crate) content: String,
    pub(crate) status: String,
    #[serde(rename = "activeForm")]
    pub(crate) active_form: String,
}

pub(super) fn execute_todo_write(
    context: ToolExecutionContext,
    agent: AgentState,
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

        match write_agent_todos(ctx_workspace_root(&context), &agent.id, &todos, None)
            .map(|_| format!("Todos updated ({} completed, {} in progress, {} pending). Proceed with current tasks.",
                todos.iter().filter(|task| task.status == "completed").count(),
                todos.iter().filter(|task| task.status == "in_progress").count(),
                todos.iter().filter(|task| task.status == "pending").count()))
        {
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
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    _tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        match read_agent_todos(ctx_workspace_root(&context), &agent.id).map(|snapshot| {
            if snapshot.tasks.is_empty() {
                "No todos set.".to_string()
            } else {
                snapshot
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(index, task)| {
                        format!(
                            "{} {}. [{}] {}",
                            match task.status.as_str() {
                                "completed" => "[x]",
                                "in_progress" => "[>]",
                                _ => "[ ]",
                            },
                            index + 1,
                            task.status,
                            task.content
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }) {
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

fn write_todo_list(configured_root: Option<&Path>, todos: &[TodoItem]) -> Result<String, String> {
    let workspace_root = workspace_root_path("todo_write", configured_root)?;
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

fn read_todo_list(configured_root: Option<&Path>) -> Result<String, String> {
    let workspace_root = workspace_root_path("todo_read", configured_root)?;
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

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub(crate) struct AgentTodos {
    pub(crate) tasks: Vec<TodoItem>,
    pub(crate) revision: String,
}

static AGENT_TODO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn agent_todo_path(root: Option<&Path>, id: &str) -> Result<PathBuf, String> {
    let root = workspace_root_path("agent tasks", root)?;
    let canonical = canonical_workspace_root(&root, "agent tasks")?;
    let filename: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
    Ok(canonical
        .join(TODO_DIRECTORY_NAME)
        .join("agent-tasks")
        .join(format!("{filename}.json")))
}

fn agent_todos_at(path: &Path) -> Result<AgentTodos, String> {
    use std::hash::{Hash, Hasher};
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => b"[]".to_vec(),
        Err(error) => return Err(format!("Could not read agent tasks: {error}")),
    };
    let tasks: Vec<TodoItem> =
        serde_json::from_slice(&bytes).map_err(|e| format!("Could not decode agent tasks: {e}"))?;
    validate_todo_items(&tasks)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    Ok(AgentTodos {
        tasks,
        revision: format!("{:016x}", hash.finish()),
    })
}

pub(crate) fn read_agent_todos(root: Option<&Path>, id: &str) -> Result<AgentTodos, String> {
    let _lock = AGENT_TODO_LOCK
        .lock()
        .map_err(|_| "Task store unavailable")?;
    agent_todos_at(&agent_todo_path(root, id)?)
}

pub(crate) fn write_agent_todos(
    root: Option<&Path>,
    id: &str,
    tasks: &[TodoItem],
    expected_revision: Option<&str>,
) -> Result<AgentTodos, String> {
    use std::io::Write;
    validate_todo_items(tasks)?;
    let _lock = AGENT_TODO_LOCK
        .lock()
        .map_err(|_| "Task store unavailable")?;
    let path = agent_todo_path(root, id)?;
    if let Some(expected) = expected_revision {
        if agent_todos_at(&path)?.revision != expected {
            return Err("Tasks changed. Refresh before saving again.".into());
        }
    }
    fs::create_dir_all(path.parent().expect("task parent")).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(tasks).map_err(|e| e.to_string())?;
    atomicwrites::AtomicFile::new(&path, atomicwrites::AllowOverwrite)
        .write(|file| file.write_all(&bytes))
        .map_err(|e| e.to_string())?;
    agent_todos_at(&path)
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    #[test]
    fn per_agent_tasks_are_isolated_and_stale_edits_do_not_overwrite_tool_updates() {
        let root = std::env::temp_dir().join(format!("agent-tasks-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let empty = read_agent_todos(Some(&root), "one").unwrap();
        let task = TodoItem {
            content: "Research".into(),
            status: "pending".into(),
            active_form: "Researching".into(),
        };
        let saved =
            write_agent_todos(Some(&root), "one", &[task.clone()], Some(&empty.revision)).unwrap();
        assert_eq!(
            read_agent_todos(Some(&root), "one").unwrap().tasks,
            vec![task.clone()]
        );
        assert!(read_agent_todos(Some(&root), "two")
            .unwrap()
            .tasks
            .is_empty());
        let completed = TodoItem {
            status: "completed".into(),
            ..task
        };
        write_agent_todos(Some(&root), "one", &[completed.clone()], None).unwrap();
        assert!(write_agent_todos(Some(&root), "one", &[], Some(&saved.revision)).is_err());
        assert_eq!(
            read_agent_todos(Some(&root), "one").unwrap().tasks,
            vec![completed]
        );
        assert!(agent_todo_path(Some(&root), "../escape")
            .unwrap()
            .starts_with(root.canonicalize().unwrap()));
        fs::remove_dir_all(root).unwrap();
    }
}
