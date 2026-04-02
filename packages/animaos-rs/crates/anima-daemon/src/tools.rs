use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall, ToolDescriptor};
use anima_memory::{MemoryManager, MemorySearchOptions, MemoryType, NewMemory};

type ToolHandler =
    fn(&ToolExecutionContext, &AgentState, &Message, &ToolCall) -> TaskResult<Content>;

#[derive(Clone)]
pub(crate) struct ToolRegistry {
    handlers: HashMap<String, ToolHandler>,
}

#[derive(Clone)]
pub(crate) struct ToolExecutionContext {
    memory: Arc<Mutex<MemoryManager>>,
    tool_registry: ToolRegistry,
}

impl ToolExecutionContext {
    pub(crate) fn new(memory: Arc<Mutex<MemoryManager>>, tool_registry: ToolRegistry) -> Self {
        Self {
            memory,
            tool_registry,
        }
    }

    pub(crate) fn execute_tool(
        &self,
        agent: AgentState,
        user_message: Message,
        tool_call: ToolCall,
    ) -> TaskResult<Content> {
        let handler = self.tool_registry.lookup(&tool_call.name);
        match handler {
            Some(handler) => handler(self, &agent, &user_message, &tool_call),
            None => TaskResult::error(format!("Unknown tool: {}", tool_call.name), 0),
        }
    }
}

impl ToolRegistry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
        };
        registry.register("memory_search", execute_memory_search);
        registry.register("memory_add", execute_memory_add);
        registry.register("recent_memories", execute_recent_memories);
        registry
    }

    fn register(&mut self, name: &str, handler: ToolHandler) {
        self.handlers.insert(name.to_string(), handler);
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<ToolHandler> {
        self.handlers.get(name).copied()
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

fn execute_memory_search(
    context: &ToolExecutionContext,
    agent: &AgentState,
    _user_message: &Message,
    tool_call: &ToolCall,
) -> TaskResult<Content> {
    let query = match tool_call.args.get("query") {
        Some(DataValue::String(value)) if !value.is_empty() => value.clone(),
        _ => {
            return TaskResult::error("memory_search query must be a non-empty string", 0);
        }
    };

    let limit = match tool_call.args.get("limit") {
        Some(DataValue::Number(value))
            if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
        {
            *value as usize
        }
        Some(DataValue::Number(_)) | Some(_) => {
            return TaskResult::error("memory_search limit must be a positive integer", 0);
        }
        None => 3,
    };

    let results = context
        .memory
        .lock()
        .expect("memory mutex should not be poisoned")
        .search(
            &query,
            MemorySearchOptions {
                agent_id: Some(agent.id.clone()),
                limit: Some(limit),
                ..MemorySearchOptions::default()
            },
        );

    let mut metadata = BTreeMap::new();
    metadata.insert("query".into(), DataValue::String(query));
    metadata.insert("matchCount".into(), DataValue::Number(results.len() as f64));

    let text = if results.is_empty() {
        "no memory matches".to_string()
    } else {
        results
            .into_iter()
            .map(|result| result.content)
            .collect::<Vec<_>>()
            .join("\n")
    };

    TaskResult::success(
        Content {
            text,
            attachments: None,
            metadata: Some(metadata),
        },
        0,
    )
}

fn execute_memory_add(
    context: &ToolExecutionContext,
    agent: &AgentState,
    _user_message: &Message,
    tool_call: &ToolCall,
) -> TaskResult<Content> {
    let content = match tool_call.args.get("content") {
        Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
        _ => return TaskResult::error("memory_add content must be a non-empty string", 0),
    };

    let memory_type = match tool_call.args.get("type") {
        None => MemoryType::Fact,
        Some(DataValue::String(value)) => match MemoryType::parse(value) {
            Ok(memory_type) => memory_type,
            Err(()) => {
                return TaskResult::error(
                    "memory_add type must be one of fact, observation, task_result, reflection",
                    0,
                )
            }
        },
        Some(_) => return TaskResult::error("memory_add type must be a string", 0),
    };

    let importance = match tool_call.args.get("importance") {
        None => 0.8,
        Some(DataValue::Number(value)) if value.is_finite() && (0.0..=1.0).contains(value) => {
            *value
        }
        Some(DataValue::Number(_)) | Some(_) => {
            return TaskResult::error("memory_add importance must be between 0 and 1", 0);
        }
    };

    let memory = match context
        .memory
        .lock()
        .expect("memory mutex should not be poisoned")
        .add(NewMemory {
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            memory_type,
            content: content.clone(),
            importance,
            tags: Some(vec!["runtime".into(), "tool-memory-add".into()]),
        }) {
        Ok(memory) => memory,
        Err(error) => return TaskResult::error(error.message(), 0),
    };

    let mut metadata = BTreeMap::new();
    metadata.insert("memoryId".into(), DataValue::String(memory.id));
    metadata.insert(
        "memoryType".into(),
        DataValue::String(memory.memory_type.as_str().to_string()),
    );

    TaskResult::success(
        Content {
            text: format!("stored memory: {content}"),
            attachments: None,
            metadata: Some(metadata),
        },
        0,
    )
}

fn execute_recent_memories(
    context: &ToolExecutionContext,
    agent: &AgentState,
    _user_message: &Message,
    tool_call: &ToolCall,
) -> TaskResult<Content> {
    let limit = match tool_call.args.get("limit") {
        Some(DataValue::Number(value))
            if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
        {
            *value as usize
        }
        Some(DataValue::Number(_)) | Some(_) => {
            return TaskResult::error("recent_memories limit must be a positive integer", 0);
        }
        None => 3,
    };

    let memories = context
        .memory
        .lock()
        .expect("memory mutex should not be poisoned")
        .get_recent(anima_memory::RecentMemoryOptions {
            agent_id: Some(agent.id.clone()),
            agent_name: None,
            limit: Some(limit),
        });

    let mut metadata = BTreeMap::new();
    metadata.insert("limit".into(), DataValue::Number(limit as f64));
    metadata.insert(
        "matchCount".into(),
        DataValue::Number(memories.len() as f64),
    );

    let text = if memories.is_empty() {
        "no recent memories".to_string()
    } else {
        memories
            .into_iter()
            .map(|memory| memory.content)
            .collect::<Vec<_>>()
            .join("\n")
    };

    TaskResult::success(
        Content {
            text,
            attachments: None,
            metadata: Some(metadata),
        },
        0,
    )
}
