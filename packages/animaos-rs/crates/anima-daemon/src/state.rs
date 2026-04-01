use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentRuntime, AgentRuntimeSnapshot, AgentState, Content, Message, ModelAdapter,
    TaskResult, ToolCall,
};
use anima_memory::{Memory, MemoryManager, MemoryType, NewMemory, RecentMemoryOptions};
use anima_swarm::SwarmCoordinator;

use crate::components::{default_evaluators, default_providers};
use crate::model::DeterministicModelAdapter;
use crate::tools::ToolRegistry;

pub(crate) struct DaemonState {
    pub(crate) memory: Arc<Mutex<MemoryManager>>,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) _swarm: SwarmCoordinator,
}

impl DaemonState {
    pub(crate) fn new() -> Self {
        Self::with_model_adapter(Arc::new(DeterministicModelAdapter))
    }

    pub(crate) fn with_model_adapter(model_adapter: Arc<dyn ModelAdapter>) -> Self {
        let memory = Arc::new(Mutex::new(MemoryManager::new()));
        Self {
            memory,
            agents: HashMap::new(),
            model_adapter,
            tool_registry: ToolRegistry::new(),
            _swarm: SwarmCoordinator::new(),
        }
    }

    pub(crate) fn create_agent(
        &mut self,
        config: AgentConfig,
    ) -> Result<AgentRuntimeSnapshot, String> {
        self.tool_registry.validate_tools(config.tools.as_deref())?;
        let mut runtime = AgentRuntime::new(config, Arc::clone(&self.model_adapter));
        runtime.set_providers(default_providers(Arc::clone(&self.memory)));
        runtime.set_evaluators(default_evaluators(Arc::clone(&self.memory)));
        runtime.init();
        let agent_id = runtime.id().to_string();
        let snapshot = runtime.snapshot();
        self.agents.insert(agent_id, runtime);
        Ok(snapshot)
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
        Some(
            self.memory
                .lock()
                .expect("memory mutex should not be poisoned")
                .get_recent(RecentMemoryOptions {
                    agent_id: Some(runtime.id().to_string()),
                    agent_name: None,
                    limit,
                }),
        )
    }

    pub(crate) fn run_agent(
        &mut self,
        agent_id: &str,
        input: Content,
    ) -> Option<(AgentRuntimeSnapshot, TaskResult<Content>)> {
        let mut runtime = self.agents.remove(agent_id)?;
        let result = runtime.run_with_tools(input, |agent, user_message, tool_call| {
            self.execute_tool(agent, user_message, tool_call)
        });
        let snapshot = runtime.snapshot();
        let agent_id = runtime.id().to_string();
        let agent_name = runtime.state().name;
        self.agents.insert(agent_id.clone(), runtime);

        if let Some(content) = result.data.as_ref() {
            self.memory
                .lock()
                .expect("memory mutex should not be poisoned")
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

    fn execute_tool(
        &mut self,
        agent: &AgentState,
        user_message: &Message,
        tool_call: &ToolCall,
    ) -> TaskResult<Content> {
        let handler = self.tool_registry.lookup(&tool_call.name);
        match handler {
            Some(handler) => handler(self, agent, user_message, tool_call),
            None => TaskResult::error(format!("Unknown tool: {}", tool_call.name), 0),
        }
    }
}
