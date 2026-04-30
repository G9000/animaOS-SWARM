mod runtime_events;
mod swarm_runtime;
mod swarm_tools;

use std::collections::HashMap;
use std::sync::Arc;

use anima_core::{
    AgentConfig, AgentRuntime, AgentRuntimeSnapshot, DatabaseAdapter, ModelAdapter,
};
use anima_memory::MemoryManager;
use anima_swarm::strategies::resolve_strategy;
use anima_swarm::{SwarmConfig, SwarmCoordinator, SwarmState};
use tokio::sync::RwLock as AsyncRwLock;

use crate::components::{default_evaluators, default_providers};
use crate::events::{EventFanout, EventSubscriber, DEFAULT_EVENT_BUFFER};
use crate::model::DeterministicModelAdapter;
use crate::tools::{
    background_process_count, new_shared_process_manager_with_limit, SharedProcessManager,
    ToolExecutionContext, ToolRegistry, DEFAULT_MAX_BACKGROUND_PROCESSES,
};

pub(crate) type SharedMemoryStore = Arc<AsyncRwLock<MemoryManager>>;

pub(crate) struct DaemonState {
    pub(crate) memory: SharedMemoryStore,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) swarms: HashMap<String, SwarmCoordinator>,
    pub(crate) swarm_events: HashMap<String, EventFanout>,
    pub(crate) swarm_snapshots: HashMap<String, SwarmState>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) process_manager: SharedProcessManager,
    pub(crate) event_fanout: EventFanout,
    pub(crate) db: Option<Arc<dyn DatabaseAdapter>>,
}

impl DaemonState {
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self::with_model_adapter_and_events_and_limits(
            Arc::new(DeterministicModelAdapter),
            EventFanout::new(DEFAULT_EVENT_BUFFER),
            DEFAULT_MAX_BACKGROUND_PROCESSES,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn with_events(event_fanout: EventFanout) -> Self {
        Self::with_events_and_limits(event_fanout, DEFAULT_MAX_BACKGROUND_PROCESSES)
    }

    pub(crate) fn with_events_and_limits(
        event_fanout: EventFanout,
        max_background_processes: usize,
    ) -> Self {
        Self::with_model_adapter_and_events_and_limits(
            Arc::new(DeterministicModelAdapter),
            event_fanout,
            max_background_processes,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn with_model_adapter(model_adapter: Arc<dyn ModelAdapter>) -> Self {
        Self::with_model_adapter_and_events_and_limits(
            model_adapter,
            EventFanout::new(DEFAULT_EVENT_BUFFER),
            DEFAULT_MAX_BACKGROUND_PROCESSES,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn with_model_adapter_and_events(
        model_adapter: Arc<dyn ModelAdapter>,
        event_fanout: EventFanout,
    ) -> Self {
        Self::with_model_adapter_and_events_and_limits(
            model_adapter,
            event_fanout,
            DEFAULT_MAX_BACKGROUND_PROCESSES,
        )
    }

    pub(crate) fn with_model_adapter_and_events_and_limits(
        model_adapter: Arc<dyn ModelAdapter>,
        event_fanout: EventFanout,
        max_background_processes: usize,
    ) -> Self {
        let memory = Arc::new(AsyncRwLock::new(MemoryManager::new()));
        Self {
            memory,
            agents: HashMap::new(),
            swarms: HashMap::new(),
            swarm_events: HashMap::new(),
            swarm_snapshots: HashMap::new(),
            model_adapter,
            tool_registry: ToolRegistry::new(),
            process_manager: new_shared_process_manager_with_limit(max_background_processes),
            event_fanout,
            db: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn event_fanout(&self) -> EventFanout {
        self.event_fanout.clone()
    }

    pub(crate) fn memory_handle(&self) -> SharedMemoryStore {
        Arc::clone(&self.memory)
    }

    pub(crate) fn agent_count(&self) -> usize {
        self.agents.len()
    }

    pub(crate) fn swarm_count(&self) -> usize {
        self.swarms.len()
    }

    pub(crate) fn swarm_snapshot_count(&self) -> usize {
        self.swarm_snapshots.len()
    }

    pub(crate) fn database_configured(&self) -> bool {
        self.db.is_some()
    }

    pub(crate) fn background_process_count(&self) -> Result<usize, String> {
        background_process_count(&self.process_manager)
    }

    pub(crate) fn set_database(&mut self, db: Arc<dyn DatabaseAdapter>) {
        for runtime in self.agents.values_mut() {
            runtime.set_database(Arc::clone(&db));
        }
        self.db = Some(db);
    }

    pub(crate) fn build_swarm(
        &self,
        config: SwarmConfig,
    ) -> Result<(SwarmCoordinator, EventFanout), String> {
        self.validate_swarm_tools(&config)?;

        let event_stream = EventFanout::new(DEFAULT_EVENT_BUFFER);
        let strategy = resolve_strategy(config.strategy);
        let factory = self.swarm_agent_factory(event_stream.clone());

        Ok((
            SwarmCoordinator::with_hooks(config, strategy, factory),
            event_stream,
        ))
    }

    pub(crate) fn register_swarm(
        &mut self,
        coordinator: SwarmCoordinator,
        event_stream: EventFanout,
    ) -> SwarmState {
        let snapshot = coordinator.get_state();
        let swarm_id = snapshot.id.clone();
        self.swarms.insert(swarm_id.clone(), coordinator);
        self.swarm_events.insert(swarm_id.clone(), event_stream);
        self.swarm_snapshots.insert(swarm_id, snapshot.clone());
        snapshot
    }

    pub(crate) fn get_swarm(&self, swarm_id: &str) -> Option<SwarmState> {
        self.swarms
            .get(swarm_id)
            .map(SwarmCoordinator::get_state)
            .or_else(|| self.swarm_snapshots.get(swarm_id).cloned())
    }

    pub(crate) fn list_swarms(&self) -> Vec<SwarmState> {
        let mut snapshots = self.swarm_snapshots.clone();
        for (swarm_id, coordinator) in &self.swarms {
            snapshots.insert(swarm_id.clone(), coordinator.get_state());
        }

        let mut snapshots: Vec<_> = snapshots.into_values().collect();
        snapshots.sort_by(|left, right| left.id.cmp(&right.id));
        snapshots
    }

    pub(crate) fn get_swarm_coordinator(&self, swarm_id: &str) -> Option<SwarmCoordinator> {
        self.swarms.get(swarm_id).cloned()
    }

    pub(crate) fn subscribe_to_swarm_events(&self, swarm_id: &str) -> Option<EventSubscriber> {
        self.swarm_events.get(swarm_id).map(EventFanout::subscribe)
    }

    pub(crate) fn swarm_event_fanout(&self, swarm_id: &str) -> Option<EventFanout> {
        self.swarm_events.get(swarm_id).cloned()
    }

    pub(crate) fn store_swarm_snapshot(&mut self, snapshot: SwarmState) {
        self.swarm_snapshots.insert(snapshot.id.clone(), snapshot);
    }

    pub(crate) fn create_agent(
        &mut self,
        config: AgentConfig,
    ) -> Result<AgentRuntimeSnapshot, String> {
        self.tool_registry.validate_tools(config.tools.as_deref())?;
        let mut runtime = AgentRuntime::new(config, Arc::clone(&self.model_adapter));
        runtime.set_providers(default_providers(Arc::clone(&self.memory)));
        runtime.set_evaluators(default_evaluators(Arc::clone(&self.memory)));
        if let Some(db) = &self.db {
            runtime.set_database(Arc::clone(db));
        }
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

    pub(crate) fn remove_agent(&mut self, agent_id: &str) {
        if let Some(mut runtime) = self.agents.remove(agent_id) {
            runtime.stop();
        }
    }

    pub(crate) fn agent_runtime_id(&self, agent_id: &str) -> Option<String> {
        self.agents.get(agent_id).map(|runtime| runtime.id().to_string())
    }

    pub(crate) fn take_agent_runtime(
        &mut self,
        agent_id: &str,
    ) -> Option<(AgentRuntime, ToolExecutionContext)> {
        let runtime = self.agents.remove(agent_id)?;
        let tool_context = ToolExecutionContext::new(
            Arc::clone(&self.memory),
            self.tool_registry.clone(),
            Arc::clone(&self.process_manager),
        );
        Some((runtime, tool_context))
    }

    pub(crate) fn restore_agent_runtime(
        &mut self,
        runtime: AgentRuntime,
    ) -> (AgentRuntimeSnapshot, String, String, SharedMemoryStore) {
        let snapshot = runtime.snapshot();
        let agent_id = runtime.id().to_string();
        let agent_name = runtime.state().name;
        self.agents.insert(agent_id.clone(), runtime);

        (snapshot, agent_id, agent_name, Arc::clone(&self.memory))
    }

    fn validate_swarm_tools(&self, config: &SwarmConfig) -> Result<(), String> {
        self.tool_registry
            .validate_tools(config.manager.tools.as_deref())?;

        for worker in &config.workers {
            self.tool_registry.validate_tools(worker.tools.as_deref())?;
        }

        Ok(())
    }

}
