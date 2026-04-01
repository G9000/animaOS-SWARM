use std::collections::HashMap;
use std::sync::Arc;

use anima_core::{
    AgentConfig, AgentRuntime, AgentRuntimeSnapshot, AgentState, Content, DataValue, Message,
    ModelAdapter, TaskResult, ToolCall,
};
use anima_memory::{
    Memory, MemoryManager, MemorySearchOptions, MemoryType, NewMemory, RecentMemoryOptions,
};
use anima_swarm::SwarmCoordinator;

use crate::model::DeterministicModelAdapter;

pub(crate) struct DaemonState {
    pub(crate) memory: MemoryManager,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) _swarm: SwarmCoordinator,
}

impl DaemonState {
    pub(crate) fn new() -> Self {
        Self::with_model_adapter(Arc::new(DeterministicModelAdapter))
    }

    pub(crate) fn with_model_adapter(model_adapter: Arc<dyn ModelAdapter>) -> Self {
        Self {
            memory: MemoryManager::new(),
            agents: HashMap::new(),
            model_adapter,
            _swarm: SwarmCoordinator::new(),
        }
    }

    pub(crate) fn create_agent(&mut self, config: AgentConfig) -> AgentRuntimeSnapshot {
        let mut runtime = AgentRuntime::new(config, Arc::clone(&self.model_adapter));
        runtime.init();
        let agent_id = runtime.id().to_string();
        let snapshot = runtime.snapshot();
        self.agents.insert(agent_id, runtime);
        snapshot
    }

    pub(crate) fn list_agents(&self) -> Vec<AgentRuntimeSnapshot> {
        let mut snapshots: Vec<_> = self.agents.values().map(AgentRuntime::snapshot).collect();
        snapshots.sort_by(|left, right| {
            left.state
                .created_at
                .cmp(&right.state.created_at)
                .then_with(|| left.state.id.cmp(&right.state.id))
        });
        snapshots
    }

    pub(crate) fn get_agent(&self, agent_id: &str) -> Option<AgentRuntimeSnapshot> {
        self.agents.get(agent_id).map(AgentRuntime::snapshot)
    }

    pub(crate) fn recent_memories_for_agent(
        &self,
        agent_id: &str,
        limit: Option<usize>,
    ) -> Option<Vec<Memory>> {
        let runtime = self.agents.get(agent_id)?;
        Some(self.memory.get_recent(RecentMemoryOptions {
            agent_id: Some(runtime.id().to_string()),
            agent_name: None,
            limit,
        }))
    }

    pub(crate) fn run_agent(
        &mut self,
        agent_id: &str,
        input: Content,
    ) -> Option<(AgentRuntimeSnapshot, TaskResult<Content>)> {
        let (agent_id, agent_name, snapshot, result) = {
            let memory = &self.memory;
            let runtime = self.agents.get_mut(agent_id)?;
            let result = runtime.run_with_tools(input, |agent, user_message, tool_call| {
                execute_tool(memory, agent, user_message, tool_call)
            });
            let snapshot = runtime.snapshot();
            let agent_id = runtime.id().to_string();
            let agent_name = runtime.state().name;
            (agent_id, agent_name, snapshot, result)
        };

        if let Some(content) = result.data.as_ref() {
            self.memory
                .add(NewMemory {
                    agent_id,
                    agent_name,
                    memory_type: MemoryType::TaskResult,
                    content: content.text.clone(),
                    importance: 0.8,
                    tags: Some(vec!["runtime".into(), "task-result".into()]),
                })
                .expect("runtime task_result memory should be valid");
        }

        Some((snapshot, result))
    }
}

fn execute_tool(
    memory: &MemoryManager,
    agent: &AgentState,
    _user_message: &Message,
    tool_call: &ToolCall,
) -> TaskResult<Content> {
    match tool_call.name.as_str() {
        "memory_search" => execute_memory_search(memory, agent, tool_call),
        _ => TaskResult::error(format!("Unknown tool: {}", tool_call.name), 0),
    }
}

fn execute_memory_search(
    memory: &MemoryManager,
    agent: &AgentState,
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

    let results = memory.search(
        &query,
        MemorySearchOptions {
            agent_id: Some(agent.id.clone()),
            limit: Some(limit),
            ..MemorySearchOptions::default()
        },
    );

    let mut metadata = std::collections::BTreeMap::new();
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
