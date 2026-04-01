use std::collections::HashMap;

use anima_core::{AgentConfig, AgentRuntime, AgentRuntimeSnapshot, Content, TaskResult};
use anima_memory::{Memory, MemoryManager, MemoryType, NewMemory, RecentMemoryOptions};
use anima_swarm::SwarmCoordinator;

pub(crate) struct DaemonState {
    pub(crate) memory: MemoryManager,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) _swarm: SwarmCoordinator,
}

impl DaemonState {
    pub(crate) fn new() -> Self {
        Self {
            memory: MemoryManager::new(),
            agents: HashMap::new(),
            _swarm: SwarmCoordinator::new(),
        }
    }

    pub(crate) fn create_agent(&mut self, config: AgentConfig) -> AgentRuntimeSnapshot {
        let mut runtime = AgentRuntime::new(config);
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
            let runtime = self.agents.get_mut(agent_id)?;
            let result = runtime.run(input);
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
