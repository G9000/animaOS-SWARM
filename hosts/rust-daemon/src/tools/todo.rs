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

const TASKS_CHANGED_PREFIX: &str = "Tasks changed.";

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

        let root = ctx_workspace_root(&context);
        let expected = context
            .todo_revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match write_agent_todos(root, &agent.id, &todos, expected.as_deref()) {
            Ok(saved) => {
                *context
                    .todo_revision
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(saved.revision);
                TaskResult::success(
                    Content {
                        text: format!(
                            "Todos updated ({} completed, {} in progress, {} pending). Proceed with current tasks.",
                            todos.iter().filter(|task| task.status == "completed").count(),
                            todos.iter().filter(|task| task.status == "in_progress").count(),
                            todos.iter().filter(|task| task.status == "pending").count()
                        ),
                        attachments: None,
                        metadata: None,
                    },
                    0,
                )
            }
            // Another room updated the list since this run saw it: save nothing,
            // show the latest list, and let a merged retry succeed.
            Err(error) if error.starts_with(TASKS_CHANGED_PREFIX) => {
                match read_agent_todos(root, &agent.id) {
                    Ok(latest) => {
                        let message = format!(
                            "Tasks changed since this run last saw them, so nothing was saved. The latest tasks are:\n{}\nMerge your changes into this list and call todo_write again with the complete list.",
                            render_agent_todos(&latest.tasks)
                        );
                        *context
                            .todo_revision
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(latest.revision);
                        TaskResult::error(message, 0)
                    }
                    Err(read_error) => TaskResult::error(read_error, 0),
                }
            }
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
        match read_agent_todos(ctx_workspace_root(&context), &agent.id) {
            Ok(snapshot) => {
                *context
                    .todo_revision
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(snapshot.revision.clone());
                TaskResult::success(
                    Content {
                        text: render_agent_todos(&snapshot.tasks),
                        attachments: None,
                        metadata: None,
                    },
                    0,
                )
            }
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

fn render_agent_todos(tasks: &[TodoItem]) -> String {
    if tasks.is_empty() {
        return "No todos set.".to_string();
    }
    tasks
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

    use anima_core::TaskStatus;
    use std::sync::Arc;

    fn todo_agent(id: &str) -> AgentState {
        AgentState {
            id: id.into(),
            name: "todo-agent".into(),
            status: anima_core::AgentStatus::Idle,
            config: anima_core::AgentConfig {
                name: "todo-agent".into(),
                model: "test".into(),
                bio: None,
                lore: None,
                knowledge: None,
                topics: None,
                adjectives: None,
                style: None,
                provider: None,
                system: None,
                tools: None,
                plugins: None,
                settings: None,
            },
            created_at_ms: 1,
            token_usage: anima_core::TokenUsage::default(),
        }
    }

    fn todo_context(root: &Path) -> ToolExecutionContext {
        ToolExecutionContext::new(
            Arc::new(tokio::sync::RwLock::new(anima_memory::MemoryManager::new())),
            Arc::new(tokio::sync::RwLock::new(
                crate::memory_embeddings::MemoryEmbeddingRuntime::disabled(),
            )),
            None,
            crate::tools::ToolRegistry::new(),
            crate::tools::new_shared_process_manager_with_limit(1),
            Some(root.to_path_buf()),
            None,
        )
    }

    fn user_message() -> Message {
        Message {
            id: "todo-message".into(),
            agent_id: "agent-cas".into(),
            room_id: "room-a".into(),
            content: Content::default(),
            role: anima_core::MessageRole::User,
            created_at_ms: 1,
        }
    }

    fn write_call(items: &[&str]) -> ToolCall {
        ToolCall {
            id: "todo-write".into(),
            name: "todo_write".into(),
            args: std::collections::BTreeMap::from([(
                "todos".to_string(),
                DataValue::Array(
                    items
                        .iter()
                        .map(|content| {
                            DataValue::Object(std::collections::BTreeMap::from([
                                ("content".to_string(), DataValue::String((*content).into())),
                                ("status".to_string(), DataValue::String("pending".into())),
                                (
                                    "activeForm".to_string(),
                                    DataValue::String(format!("Doing {content}")),
                                ),
                            ]))
                        })
                        .collect(),
                ),
            )]),
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test]
    async fn todo_write_is_compare_and_swap_across_concurrent_runs() {
        let root = temp_root("agent-todo-cas");
        let baseline = read_agent_todos(Some(&root), "agent-cas").unwrap().revision;
        let first = todo_context(&root).with_todo_baseline(Some(baseline.clone()));
        let second = todo_context(&root).with_todo_baseline(Some(baseline));

        let saved = execute_todo_write(
            second,
            todo_agent("agent-cas"),
            user_message(),
            write_call(&["From room B"]),
        )
        .await;
        assert_eq!(saved.status, TaskStatus::Success);

        let conflict = execute_todo_write(
            first.clone(),
            todo_agent("agent-cas"),
            user_message(),
            write_call(&["From room A"]),
        )
        .await;
        assert_eq!(conflict.status, TaskStatus::Error);
        let message = conflict.error.unwrap();
        assert!(
            message.starts_with("Tasks changed since this run last saw them, so nothing was saved."),
            "{message}"
        );
        assert!(message.contains("[ ] 1. [pending] From room B"), "{message}");
        assert!(message.ends_with("call todo_write again with the complete list."), "{message}");
        assert_eq!(
            read_agent_todos(Some(&root), "agent-cas").unwrap().tasks[0].content,
            "From room B"
        );

        let merged = execute_todo_write(
            first,
            todo_agent("agent-cas"),
            user_message(),
            write_call(&["From room B", "From room A"]),
        )
        .await;
        assert_eq!(merged.status, TaskStatus::Success);
        assert_eq!(
            merged.data.unwrap().text,
            "Todos updated (0 completed, 0 in progress, 2 pending). Proceed with current tasks."
        );
        assert_eq!(read_agent_todos(Some(&root), "agent-cas").unwrap().tasks.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn todo_read_refreshes_the_revision_a_run_writes_against() {
        let root = temp_root("agent-todo-read");
        let context = todo_context(&root).with_todo_baseline(Some("stale-revision".into()));

        let read = execute_todo_read(
            context.clone(),
            todo_agent("agent-read"),
            user_message(),
            ToolCall {
                id: "todo-read".into(),
                name: "todo_read".into(),
                args: std::collections::BTreeMap::new(),
            },
        )
        .await;
        assert_eq!(read.data.unwrap().text, "No todos set.");

        let written = execute_todo_write(
            context,
            todo_agent("agent-read"),
            user_message(),
            write_call(&["Plan"]),
        )
        .await;
        assert_eq!(written.status, TaskStatus::Success);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn todo_write_without_a_baseline_replaces_the_list() {
        let root = temp_root("agent-todo-blind");

        let written = execute_todo_write(
            todo_context(&root),
            todo_agent("agent-blind"),
            user_message(),
            write_call(&["Plan"]),
        )
        .await;

        assert_eq!(written.status, TaskStatus::Success);
        fs::remove_dir_all(root).unwrap();
    }
}
