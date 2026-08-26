mod runtime_events;
mod swarm_relationships;
mod swarm_runtime;
mod swarm_tools;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anima_core::{
    AgentConfig, AgentConfigUpdate, AgentRuntime, AgentRuntimeSnapshot, AgentStatus,
    DatabaseAdapter, ModelAdapter, ToolDescriptor,
};
use anima_memory::{locomo_query_expander, MemoryManager, QueryExpander, TextAnalyzer};
use anima_swarm::coordinator::CoordinatorMessageEventFn;
use anima_swarm::strategies::resolve_strategy;
use anima_swarm::{SwarmConfig, SwarmCoordinator, SwarmState};
use tokio::sync::RwLock as AsyncRwLock;
use tracing::warn;

use crate::components::{default_evaluators, default_providers};
use crate::control_plane_store::{
    save_control_plane_snapshot, ControlPlaneSnapshot, ControlPlaneStoreConfig, StoredSwarmSnapshot,
};
use crate::events::{EventFanout, EventSubscriber, DEFAULT_EVENT_BUFFER};
use crate::memory_embeddings::{MemoryEmbeddingRuntime, SharedMemoryEmbeddings};
use crate::memory_store::MemoryStoreConfig;
use crate::model::DeterministicModelAdapter;
use crate::tools::{
    background_process_count, new_shared_process_manager_with_limit, SharedProcessManager,
    ToolExecutionContext, ToolRegistry, DEFAULT_MAX_BACKGROUND_PROCESSES,
};

use self::swarm_relationships::{persist_swarm_message_relationship, swarm_agent_names};

pub(crate) type SharedMemoryStore = Arc<AsyncRwLock<MemoryManager>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateAgentError {
    InvalidTools(String),
    NotFound,
}

const MEMORY_QUERY_EXPANDER_ENV: &str = "ANIMAOS_RS_MEMORY_QUERY_EXPANDER";
const MEMORY_TEXT_ANALYZER_ENV: &str = "ANIMAOS_RS_MEMORY_TEXT_ANALYZER";

pub(crate) fn memory_manager_from_env() -> MemoryManager {
    let text_analyzer = memory_text_analyzer_from_env();
    match memory_query_expander_from_env() {
        Some(query_expander) => {
            MemoryManager::with_text_analyzer_and_query_expander(text_analyzer, query_expander)
        }
        None => MemoryManager::with_text_analyzer(text_analyzer),
    }
}

pub(crate) fn memory_text_analyzer_from_env() -> TextAnalyzer {
    let Ok(value) = std::env::var(MEMORY_TEXT_ANALYZER_ENV) else {
        return TextAnalyzer::default();
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "default" | "multilingual" | "unicode" => TextAnalyzer::multilingual(),
        unknown => {
            warn!(
                env = MEMORY_TEXT_ANALYZER_ENV,
                value = unknown,
                "unknown memory text analyzer profile; using multilingual search"
            );
            TextAnalyzer::default()
        }
    }
}

pub(crate) fn memory_query_expander_from_env() -> Option<QueryExpander> {
    let Ok(value) = std::env::var(MEMORY_QUERY_EXPANDER_ENV) else {
        return None;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" | "off" | "disabled" => None,
        "locomo" | "locomo-benchmark" => Some(locomo_query_expander()),
        unknown => {
            warn!(
                env = MEMORY_QUERY_EXPANDER_ENV,
                value = unknown,
                "unknown memory query expander profile; using default BM25 search"
            );
            None
        }
    }
}

pub(crate) struct DaemonState {
    pub(crate) memory: SharedMemoryStore,
    pub(crate) memory_embeddings: SharedMemoryEmbeddings,
    pub(crate) memory_store: Option<MemoryStoreConfig>,
    pub(crate) control_plane_store: Option<ControlPlaneStoreConfig>,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) agent_snapshots: HashMap<String, AgentRuntimeSnapshot>,
    pub(crate) swarms: HashMap<String, SwarmCoordinator>,
    pub(crate) swarm_configs: HashMap<String, SwarmConfig>,
    pub(crate) swarm_events: HashMap<String, EventFanout>,
    pub(crate) swarm_snapshots: HashMap<String, SwarmState>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) process_manager: SharedProcessManager,
    pub(crate) event_fanout: EventFanout,
    pub(crate) db: Option<Arc<dyn DatabaseAdapter>>,
}

pub(crate) struct ControlPlanePersistRequest {
    config: Option<ControlPlaneStoreConfig>,
    snapshot: ControlPlaneSnapshot,
}

impl ControlPlanePersistRequest {
    pub(crate) async fn save(self) -> std::io::Result<()> {
        save_control_plane_snapshot(self.config.as_ref(), &self.snapshot).await
    }
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
        let memory = Arc::new(AsyncRwLock::new(memory_manager_from_env()));
        let memory_embeddings = Arc::new(AsyncRwLock::new(MemoryEmbeddingRuntime::local_default()));
        Self {
            memory,
            memory_embeddings,
            memory_store: None,
            control_plane_store: None,
            agents: HashMap::new(),
            agent_snapshots: HashMap::new(),
            swarms: HashMap::new(),
            swarm_configs: HashMap::new(),
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

    pub(crate) fn memory_embeddings_handle(&self) -> SharedMemoryEmbeddings {
        Arc::clone(&self.memory_embeddings)
    }

    pub(crate) fn memory_store_config(&self) -> Option<MemoryStoreConfig> {
        self.memory_store.clone()
    }

    pub(crate) fn replace_memory(&mut self, memory: MemoryManager) {
        self.memory = Arc::new(AsyncRwLock::new(memory));
    }

    pub(crate) fn set_memory_store(&mut self, memory_store: Option<MemoryStoreConfig>) {
        self.memory_store = memory_store;
    }

    pub(crate) fn set_control_plane_store(
        &mut self,
        control_plane_store: Option<ControlPlaneStoreConfig>,
    ) {
        self.control_plane_store = control_plane_store;
    }

    pub(crate) fn control_plane_persist_request(&self) -> ControlPlanePersistRequest {
        ControlPlanePersistRequest {
            config: self.control_plane_store.clone(),
            snapshot: self.control_plane_snapshot(),
        }
    }

    pub(crate) fn control_plane_snapshot(&self) -> ControlPlaneSnapshot {
        let agents = self.list_agents();
        let mut swarms = self
            .swarm_configs
            .iter()
            .filter_map(|(swarm_id, config)| {
                self.get_swarm(swarm_id).map(|state| StoredSwarmSnapshot {
                    config: config.clone(),
                    state,
                })
            })
            .collect::<Vec<_>>();
        swarms.sort_by(|left, right| left.state.id.cmp(&right.state.id));

        ControlPlaneSnapshot::new(agents, swarms)
    }

    pub(crate) fn restore_control_plane_snapshot(
        &mut self,
        snapshot: ControlPlaneSnapshot,
    ) -> Result<(usize, usize), String> {
        let mut restored_agents = 0;
        let mut restored_swarms = 0;

        for agent_snapshot in snapshot.agents {
            self.restore_agent_snapshot(agent_snapshot)?;
            restored_agents += 1;
        }

        for stored_swarm in snapshot.swarms {
            let (coordinator, event_stream) =
                self.build_recovered_swarm(stored_swarm.config, stored_swarm.state)?;
            self.register_recovered_swarm(coordinator, event_stream);
            restored_swarms += 1;
        }

        Ok((restored_agents, restored_swarms))
    }

    pub(crate) fn replace_memory_embeddings(&mut self, embeddings: MemoryEmbeddingRuntime) {
        self.memory_embeddings = Arc::new(AsyncRwLock::new(embeddings));
    }

    pub(crate) fn agent_count(&self) -> usize {
        let mut count = self.agent_snapshots.len();
        for agent_id in self.agents.keys() {
            if !self.agent_snapshots.contains_key(agent_id) {
                count += 1;
            }
        }
        count
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

    pub(crate) fn control_plane_durability(&self) -> String {
        self.control_plane_store
            .as_ref()
            .map(|config| config.storage_label().to_string())
            .unwrap_or_else(|| "ephemeral".to_string())
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

        let event_stream = EventFanout::new(self.event_fanout.capacity());
        let swarm_message_events = event_stream.clone();
        let global_message_events = self.event_fanout();
        let memory = Arc::clone(&self.memory);
        let memory_embeddings = Arc::clone(&self.memory_embeddings);
        let memory_store = self.memory_store.clone();
        let agent_names = Arc::new(swarm_agent_names(&config));
        let message_events: Arc<CoordinatorMessageEventFn> = Arc::new(move |swarm_id, message| {
            let global_message_events = global_message_events.clone();
            let swarm_message_events = swarm_message_events.clone();
            let memory = Arc::clone(&memory);
            let memory_embeddings = Arc::clone(&memory_embeddings);
            let memory_store = memory_store.clone();
            let agent_names = Arc::clone(&agent_names);
            Box::pin(async move {
                runtime_events::publish_swarm_message_event(
                    &global_message_events,
                    &swarm_message_events,
                    &swarm_id,
                    &message,
                );
                persist_swarm_message_relationship(
                    memory,
                    memory_embeddings,
                    memory_store,
                    agent_names,
                    swarm_id,
                    message,
                )
                .await;
            })
        });
        let strategy = resolve_strategy(config.strategy);
        let factory = self.swarm_agent_factory(event_stream.clone());

        Ok((
            SwarmCoordinator::with_hooks_and_message_events(
                config,
                strategy,
                factory,
                Some(message_events),
            ),
            event_stream,
        ))
    }

    pub(crate) fn build_recovered_swarm(
        &self,
        config: SwarmConfig,
        mut snapshot: SwarmState,
    ) -> Result<(SwarmCoordinator, EventFanout), String> {
        self.validate_swarm_tools(&config)?;
        snapshot.agent_ids.clear();
        if snapshot.status == anima_swarm::SwarmStatus::Running {
            snapshot.status = anima_swarm::SwarmStatus::Failed;
            snapshot
                .completed_at
                .get_or_insert_with(anima_core::primitives::now_millis);
        }

        let event_stream = EventFanout::new(self.event_fanout.capacity());
        let swarm_message_events = event_stream.clone();
        let global_message_events = self.event_fanout();
        let memory = Arc::clone(&self.memory);
        let memory_embeddings = Arc::clone(&self.memory_embeddings);
        let memory_store = self.memory_store.clone();
        let agent_names = Arc::new(swarm_agent_names(&config));
        let message_events: Arc<CoordinatorMessageEventFn> = Arc::new(move |swarm_id, message| {
            let global_message_events = global_message_events.clone();
            let swarm_message_events = swarm_message_events.clone();
            let memory = Arc::clone(&memory);
            let memory_embeddings = Arc::clone(&memory_embeddings);
            let memory_store = memory_store.clone();
            let agent_names = Arc::clone(&agent_names);
            Box::pin(async move {
                runtime_events::publish_swarm_message_event(
                    &global_message_events,
                    &swarm_message_events,
                    &swarm_id,
                    &message,
                );
                persist_swarm_message_relationship(
                    memory,
                    memory_embeddings,
                    memory_store,
                    agent_names,
                    swarm_id,
                    message,
                )
                .await;
            })
        });
        let strategy = resolve_strategy(config.strategy);
        let factory = self.swarm_agent_factory(event_stream.clone());

        Ok((
            SwarmCoordinator::with_recovered_state_and_hooks(
                config,
                snapshot,
                strategy,
                factory,
                Some(message_events),
            ),
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
        self.swarm_configs
            .insert(swarm_id.clone(), coordinator.config());
        self.swarms.insert(swarm_id.clone(), coordinator);
        self.swarm_events.insert(swarm_id.clone(), event_stream);
        self.swarm_snapshots.insert(swarm_id, snapshot.clone());
        snapshot
    }

    pub(crate) fn register_recovered_swarm(
        &mut self,
        coordinator: SwarmCoordinator,
        event_stream: EventFanout,
    ) -> SwarmState {
        self.register_swarm(coordinator, event_stream)
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
        mut config: AgentConfig,
    ) -> Result<AgentRuntimeSnapshot, String> {
        config.tools = self.resolve_agent_tools(config.tools)?;
        let mut runtime = AgentRuntime::new(config, Arc::clone(&self.model_adapter));
        runtime.set_providers(default_providers(Arc::clone(&self.memory)));
        runtime.set_evaluators(default_evaluators(
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
        ));
        if let Some(db) = &self.db {
            runtime.set_database(Arc::clone(db));
        }
        runtime.init();
        let agent_id = runtime.id().to_string();
        let snapshot = runtime.snapshot();
        self.agent_snapshots
            .insert(agent_id.clone(), snapshot.clone());
        self.agents.insert(agent_id, runtime);
        Ok(snapshot)
    }

    /// Apply a fully validated partial config update to an existing agent and
    /// refresh its snapshot.
    pub(crate) fn update_agent(
        &mut self,
        agent_id: &str,
        mut patch: AgentConfigUpdate,
    ) -> Result<AgentRuntimeSnapshot, UpdateAgentError> {
        if !self.agents.contains_key(agent_id) && !self.agent_snapshots.contains_key(agent_id) {
            return Err(UpdateAgentError::NotFound);
        }
        patch.tools = self
            .resolve_agent_tools(patch.tools)
            .map_err(UpdateAgentError::InvalidTools)?;

        if let Some(runtime) = self.agents.get_mut(agent_id) {
            runtime.update_config(patch);
            let snapshot = runtime.snapshot();
            self.agent_snapshots
                .insert(agent_id.to_string(), snapshot.clone());
            return Ok(snapshot);
        }

        // The runtime can be checked out for an in-flight run; fall back to
        // patching the snapshot so the change still persists. The run itself
        // keeps the config it started with.
        let snapshot = self
            .agent_snapshots
            .get_mut(agent_id)
            .expect("agent existence was checked before validation");
        if let Some(name) = patch.name {
            snapshot.state.name = name.clone();
            snapshot.state.config.name = name;
        }
        if let Some(model) = patch.model {
            snapshot.state.config.model = model;
        }
        if let Some(provider) = patch.provider {
            snapshot.state.config.provider = if provider.is_empty() {
                None
            } else {
                Some(provider)
            };
        }
        if let Some(system) = patch.system {
            snapshot.state.config.system = if system.is_empty() {
                None
            } else {
                Some(system)
            };
        }
        if let Some(tools) = patch.tools {
            snapshot.state.config.tools = Some(tools);
        }
        Ok(snapshot.clone())
    }

    fn restore_agent_snapshot(&mut self, mut snapshot: AgentRuntimeSnapshot) -> Result<(), String> {
        snapshot.state.config.tools = self.resolve_agent_tools(snapshot.state.config.tools)?;
        let agent_id = snapshot.state.id.clone();
        let mut runtime = AgentRuntime::from_snapshot(snapshot, Arc::clone(&self.model_adapter));
        runtime.set_providers(default_providers(Arc::clone(&self.memory)));
        runtime.set_evaluators(default_evaluators(
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
        ));
        if let Some(db) = &self.db {
            runtime.set_database(Arc::clone(db));
        }
        if runtime.state().status == AgentStatus::Running {
            runtime.mark_failed("daemon restarted before task completed", 0);
        }

        let restored_snapshot = runtime.snapshot();
        self.agent_snapshots
            .insert(agent_id.clone(), restored_snapshot);
        self.agents.insert(agent_id, runtime);
        Ok(())
    }

    pub(crate) fn list_agents(&self) -> Vec<AgentRuntimeSnapshot> {
        let mut snapshots = self.agent_snapshots.clone();
        for (agent_id, runtime) in &self.agents {
            snapshots.insert(agent_id.clone(), runtime.snapshot());
        }

        let mut snapshots: Vec<_> = snapshots.into_values().collect();
        snapshots.sort_by(|left, right| {
            left.state
                .created_at_ms
                .cmp(&right.state.created_at_ms)
                .then_with(|| left.state.id.cmp(&right.state.id))
        });
        snapshots
    }

    pub(crate) fn get_agent(&self, agent_id: &str) -> Option<AgentRuntimeSnapshot> {
        self.agents
            .get(agent_id)
            .map(AgentRuntime::snapshot)
            .or_else(|| self.agent_snapshots.get(agent_id).cloned())
    }

    pub(crate) fn remove_agent(&mut self, agent_id: &str) {
        self.agent_snapshots.remove(agent_id);
        if let Some(mut runtime) = self.agents.remove(agent_id) {
            runtime.stop();
        }
    }

    pub(crate) fn agent_runtime_id(&self, agent_id: &str) -> Option<String> {
        self.agents
            .get(agent_id)
            .map(|runtime| runtime.id().to_string())
            .or_else(|| {
                self.agent_snapshots
                    .get(agent_id)
                    .map(|snapshot| snapshot.state.id.clone())
            })
    }

    pub(crate) fn take_agent_runtime(
        &mut self,
        agent_id: &str,
    ) -> Option<(AgentRuntime, ToolExecutionContext)> {
        let runtime = self.agents.remove(agent_id)?;
        let mut snapshot = runtime.snapshot();
        snapshot.state.status = AgentStatus::Running;
        self.agent_snapshots.insert(agent_id.to_string(), snapshot);
        let tool_context = ToolExecutionContext::new(
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
            self.tool_registry.clone(),
            Arc::clone(&self.process_manager),
        );
        Some((runtime, tool_context))
    }

    pub(crate) fn restore_agent_runtime(
        &mut self,
        mut runtime: AgentRuntime,
    ) -> (
        AgentRuntimeSnapshot,
        String,
        String,
        SharedMemoryStore,
        SharedMemoryEmbeddings,
        Option<MemoryStoreConfig>,
    ) {
        if let Some(latest_config) = self
            .agent_snapshots
            .get(runtime.id())
            .map(|snapshot| snapshot.state.config.clone())
        {
            runtime.update_config(AgentConfigUpdate {
                name: Some(latest_config.name),
                model: Some(latest_config.model),
                provider: Some(latest_config.provider.unwrap_or_default()),
                system: Some(latest_config.system.unwrap_or_default()),
                tools: latest_config.tools,
            });
        }
        let snapshot = runtime.snapshot();
        let agent_id = runtime.id().to_string();
        let agent_name = runtime.state().name;
        self.agent_snapshots
            .insert(agent_id.clone(), snapshot.clone());
        self.agents.insert(agent_id.clone(), runtime);

        (
            snapshot,
            agent_id,
            agent_name,
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
        )
    }

    fn validate_swarm_tools(&self, config: &SwarmConfig) -> Result<(), String> {
        self.tool_registry
            .validate_tools(config.manager.tools.as_deref())?;

        for worker in &config.workers {
            self.tool_registry.validate_tools(worker.tools.as_deref())?;
        }

        Ok(())
    }

    fn resolve_agent_tools(
        &self,
        tools: Option<Vec<ToolDescriptor>>,
    ) -> Result<Option<Vec<ToolDescriptor>>, String> {
        let Some(tools) = tools else {
            return Ok(None);
        };
        let mut names = Vec::with_capacity(tools.len());
        let mut seen = HashSet::with_capacity(tools.len());
        for tool in tools {
            if !seen.insert(tool.name.clone()) {
                return Err(format!("duplicate tool '{}'", tool.name));
            }
            if self.tool_registry.descriptor(&tool.name).is_none() {
                return Err(format!("unknown tool: {}", tool.name));
            }
            names.push(tool.name);
        }

        self.tool_registry.resolve_descriptors(names).map(Some)
    }
}
