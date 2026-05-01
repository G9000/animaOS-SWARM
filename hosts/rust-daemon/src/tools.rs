mod filesystem;
mod memory;
mod process;
#[cfg(test)]
mod tests;
mod todo;
mod utility;
mod web;
mod workspace;

use std::collections::HashMap;

use anima_core::{AgentState, Content, Message, TaskResult, ToolCall, ToolDescriptor};
use futures::future::BoxFuture;

use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::memory_store::MemoryStoreConfig;
use crate::state::SharedMemoryStore;

pub(crate) use process::{
    background_process_count, new_shared_process_manager_with_limit, SharedProcessManager,
    DEFAULT_MAX_BACKGROUND_PROCESSES,
};
pub(crate) use workspace::{
    canonical_workspace_root, normalized_relative_path, resolve_workspace_write_path,
    workspace_root_path,
};

type ToolHandler = fn(
    ToolExecutionContext,
    AgentState,
    Message,
    ToolCall,
) -> BoxFuture<'static, TaskResult<Content>>;

#[derive(Clone)]
pub(crate) struct ToolRegistry {
    handlers: HashMap<String, ToolHandler>,
}

#[derive(Clone)]
pub(crate) struct ToolExecutionContext {
    pub(super) memory: SharedMemoryStore,
    pub(super) memory_embeddings: SharedMemoryEmbeddings,
    pub(super) memory_store: Option<MemoryStoreConfig>,
    tool_registry: ToolRegistry,
    pub(super) process_manager: SharedProcessManager,
}

impl ToolExecutionContext {
    pub(crate) fn new(
        memory: SharedMemoryStore,
        memory_embeddings: SharedMemoryEmbeddings,
        memory_store: Option<MemoryStoreConfig>,
        tool_registry: ToolRegistry,
        process_manager: SharedProcessManager,
    ) -> Self {
        Self {
            memory,
            memory_embeddings,
            memory_store,
            tool_registry,
            process_manager,
        }
    }

    pub(crate) async fn execute_tool(
        self,
        agent: AgentState,
        user_message: Message,
        tool_call: ToolCall,
    ) -> TaskResult<Content> {
        let handler = self.tool_registry.lookup(&tool_call.name);
        match handler {
            Some(handler) => handler(self, agent, user_message, tool_call).await,
            None => TaskResult::error(format!("Unknown tool: {}", tool_call.name), 0),
        }
    }
}

impl ToolRegistry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
        };
        registry.register("memory_search", memory::execute_memory_search);
        registry.register("memory_add", memory::execute_memory_add);
        registry.register("recent_memories", memory::execute_recent_memories);
        registry.register("web_fetch", web::execute_web_fetch);
        registry.register("exa_search", web::execute_exa_search);
        registry.register("get_current_time", utility::execute_get_current_time);
        registry.register("calculate", utility::execute_calculate);
        registry.register("read_file", filesystem::execute_read_file);
        registry.register("list_dir", filesystem::execute_list_dir);
        registry.register("glob", filesystem::execute_glob);
        registry.register("grep", filesystem::execute_grep);
        registry.register("write_file", filesystem::execute_write_file);
        registry.register("edit_file", filesystem::execute_edit_file);
        registry.register("multi_edit", filesystem::execute_multi_edit);
        registry.register("todo_write", todo::execute_todo_write);
        registry.register("todo_read", todo::execute_todo_read);
        registry.register("bash", process::execute_bash);
        registry.register("bg_start", process::execute_bg_start);
        registry.register("bg_output", process::execute_bg_output);
        registry.register("bg_stop", process::execute_bg_stop);
        registry.register("bg_list", process::execute_bg_list);
        registry
    }

    fn register(&mut self, name: &str, handler: ToolHandler) {
        self.handlers.insert(name.to_string(), handler);
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<ToolHandler> {
        self.handlers.get(name).copied()
    }

    pub(crate) fn tool_names(&self) -> Vec<String> {
        let mut names = self.handlers.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub(crate) fn validate_tools(&self, tools: Option<&[ToolDescriptor]>) -> Result<(), String> {
        let Some(tools) = tools else {
            return Ok(());
        };

        for tool in tools {
            if !self.handlers.contains_key(&tool.name) {
                return Err(format!("unknown tool: {}", tool.name));
            }
        }

        Ok(())
    }
}
