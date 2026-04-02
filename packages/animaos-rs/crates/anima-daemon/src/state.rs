use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentRuntime, AgentRuntimeSnapshot, Content, ModelAdapter, TaskResult,
};
use anima_memory::{Memory, MemoryManager, MemoryType, NewMemory, RecentMemoryOptions};
use anima_swarm::SwarmCoordinator;

use crate::components::{default_evaluators, default_providers};
use crate::events::{EventFanout, DEFAULT_EVENT_BUFFER};
use crate::model::DeterministicModelAdapter;
use crate::tools::{ToolExecutionContext, ToolRegistry};

pub(crate) struct DaemonState {
    pub(crate) memory: Arc<Mutex<MemoryManager>>,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) tool_registry: ToolRegistry,
    #[allow(dead_code)]
    pub(crate) event_fanout: EventFanout,
    pub(crate) _swarm: SwarmCoordinator,
}

impl DaemonState {
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self::with_model_adapter_and_events(
            Arc::new(DeterministicModelAdapter),
            EventFanout::new(DEFAULT_EVENT_BUFFER),
        )
    }

    pub(crate) fn with_events(event_fanout: EventFanout) -> Self {
        Self::with_model_adapter_and_events(Arc::new(DeterministicModelAdapter), event_fanout)
    }

    #[allow(dead_code)]
    pub(crate) fn with_model_adapter(model_adapter: Arc<dyn ModelAdapter>) -> Self {
        Self::with_model_adapter_and_events(model_adapter, EventFanout::new(DEFAULT_EVENT_BUFFER))
    }

    pub(crate) fn with_model_adapter_and_events(
        model_adapter: Arc<dyn ModelAdapter>,
        event_fanout: EventFanout,
    ) -> Self {
        let memory = Arc::new(Mutex::new(MemoryManager::new()));
        Self {
            memory,
            agents: HashMap::new(),
            model_adapter,
            tool_registry: ToolRegistry::new(),
            event_fanout,
            _swarm: SwarmCoordinator::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn event_fanout(&self) -> EventFanout {
        self.event_fanout.clone()
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

    pub(crate) fn take_agent_runtime(
        &mut self,
        agent_id: &str,
    ) -> Option<(AgentRuntime, ToolExecutionContext)> {
        let runtime = self.agents.remove(agent_id)?;
        let tool_context =
            ToolExecutionContext::new(Arc::clone(&self.memory), self.tool_registry.clone());
        Some((runtime, tool_context))
    }

    pub(crate) fn complete_agent_run(
        &mut self,
        runtime: AgentRuntime,
        result: &TaskResult<Content>,
    ) -> AgentRuntimeSnapshot {
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

        snapshot
    }
}
