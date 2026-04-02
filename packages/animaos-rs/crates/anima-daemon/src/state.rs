use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentRuntime, AgentRuntimeSnapshot, AgentState, Content, DataValue, Message,
    ModelAdapter, TaskResult, TokenUsage, ToolCall,
};
use anima_memory::{Memory, MemoryManager, MemoryType, NewMemory, RecentMemoryOptions};
use anima_swarm::coordinator::{
    CoordinatorAgentFactoryContext, CoordinatorAgentFactoryFn, CoordinatorAgentShell,
};
use anima_swarm::strategies::resolve_strategy;
use anima_swarm::{SwarmConfig, SwarmCoordinator, SwarmState};
use tokio::sync::Mutex as AsyncMutex;

use crate::components::{default_evaluators, default_providers};
use crate::events::{EventFanout, EventSubscriber, DEFAULT_EVENT_BUFFER};
use crate::model::DeterministicModelAdapter;
use crate::tools::{ToolExecutionContext, ToolRegistry};

pub(crate) struct DaemonState {
    pub(crate) memory: Arc<Mutex<MemoryManager>>,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) swarms: HashMap<String, SwarmCoordinator>,
    pub(crate) swarm_events: HashMap<String, EventFanout>,
    pub(crate) swarm_snapshots: HashMap<String, SwarmState>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) event_fanout: EventFanout,
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
            swarms: HashMap::new(),
            swarm_events: HashMap::new(),
            swarm_snapshots: HashMap::new(),
            model_adapter,
            tool_registry: ToolRegistry::new(),
            event_fanout,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn event_fanout(&self) -> EventFanout {
        self.event_fanout.clone()
    }

    pub(crate) fn build_swarm(
        &self,
        config: SwarmConfig,
    ) -> Result<(SwarmCoordinator, EventFanout), String> {
        self.validate_swarm_tools(&config)?;

        let event_stream = EventFanout::new(DEFAULT_EVENT_BUFFER);
        let strategy = resolve_strategy(config.strategy);
        let factory = self.swarm_agent_factory();

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

    pub(crate) fn get_swarm_coordinator(&self, swarm_id: &str) -> Option<SwarmCoordinator> {
        self.swarms.get(swarm_id).cloned()
    }

    pub(crate) fn subscribe_to_swarm_events(&self, swarm_id: &str) -> Option<EventSubscriber> {
        self.swarm_events.get(swarm_id).map(EventFanout::subscribe)
    }

    pub(crate) fn publish_swarm_event(
        &self,
        swarm_id: &str,
        event: impl Into<String>,
        data: String,
    ) {
        let event = event.into();
        self.event_fanout.publish(event.clone(), data.clone());
        if let Some(fanout) = self.swarm_events.get(swarm_id) {
            fanout.publish(event, data);
        }
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

    fn validate_swarm_tools(&self, config: &SwarmConfig) -> Result<(), String> {
        self.tool_registry
            .validate_tools(config.manager.tools.as_deref())?;

        for worker in &config.workers {
            self.tool_registry.validate_tools(worker.tools.as_deref())?;
        }

        Ok(())
    }

    fn swarm_agent_factory(&self) -> Arc<CoordinatorAgentFactoryFn> {
        let memory = Arc::clone(&self.memory);
        let model_adapter = Arc::clone(&self.model_adapter);
        let tool_registry = self.tool_registry.clone();

        Arc::new(move |context: CoordinatorAgentFactoryContext| {
            let memory = Arc::clone(&memory);
            let model_adapter = Arc::clone(&model_adapter);
            let tool_registry = tool_registry.clone();

            Box::pin(async move {
                let tool_context = ToolExecutionContext::new(Arc::clone(&memory), tool_registry);
                let runtime = Arc::new(AsyncMutex::new(build_swarm_runtime(
                    context.config.clone(),
                    Arc::clone(&model_adapter),
                    Arc::clone(&memory),
                )));
                let config = context.config.clone();
                let token_usage = Arc::new(Mutex::new(TokenUsage::default()));
                let needs_reset = Arc::new(AtomicBool::new(false));

                Ok(CoordinatorAgentShell {
                    run: Arc::new({
                        let runtime = Arc::clone(&runtime);
                        let config = config.clone();
                        let memory = Arc::clone(&memory);
                        let model_adapter = Arc::clone(&model_adapter);
                        let token_usage = Arc::clone(&token_usage);
                        let needs_reset = Arc::clone(&needs_reset);
                        let delegate_task = context.delegate_task.clone();
                        let tool_context = tool_context.clone();
                        move |input: String| {
                            let runtime = Arc::clone(&runtime);
                            let config = config.clone();
                            let memory = Arc::clone(&memory);
                            let model_adapter = Arc::clone(&model_adapter);
                            let token_usage = Arc::clone(&token_usage);
                            let needs_reset = Arc::clone(&needs_reset);
                            let delegate_task = delegate_task.clone();
                            let tool_context = tool_context.clone();
                            Box::pin(async move {
                                let mut runtime = runtime.lock().await;
                                if needs_reset.swap(false, Ordering::AcqRel) {
                                    *runtime = build_swarm_runtime(
                                        config,
                                        Arc::clone(&model_adapter),
                                        Arc::clone(&memory),
                                    );
                                }
                                let result = runtime
                                    .run_with_tools(
                                        Content {
                                            text: input,
                                            attachments: None,
                                            metadata: None,
                                        },
                                        |agent, user_message, tool_call| {
                                            let delegate_task = delegate_task.clone();
                                            let tool_context = tool_context.clone();
                                            async move {
                                                execute_swarm_tool(
                                                    delegate_task,
                                                    tool_context,
                                                    agent,
                                                    user_message,
                                                    tool_call,
                                                )
                                                .await
                                            }
                                        },
                                    )
                                    .await;

                                *token_usage
                                    .lock()
                                    .expect("swarm token mutex should not be poisoned") =
                                    runtime.snapshot().state.token_usage.clone();

                                result
                            })
                        }
                    }),
                    token_usage: Arc::new({
                        let token_usage = Arc::clone(&token_usage);
                        move || {
                            token_usage
                                .lock()
                                .expect("swarm token mutex should not be poisoned")
                                .clone()
                        }
                    }),
                    clear_task_state: Arc::new({
                        let needs_reset = Arc::clone(&needs_reset);
                        let token_usage = Arc::clone(&token_usage);
                        move || {
                            needs_reset.store(true, Ordering::Release);
                            *token_usage
                                .lock()
                                .expect("swarm token mutex should not be poisoned") =
                                TokenUsage::default();
                        }
                    }),
                    stop: Arc::new({
                        let runtime = Arc::clone(&runtime);
                        let token_usage = Arc::clone(&token_usage);
                        move || {
                            let runtime = Arc::clone(&runtime);
                            let token_usage = Arc::clone(&token_usage);
                            Box::pin(async move {
                                let mut runtime = runtime.lock().await;
                                runtime.stop();
                                *token_usage
                                    .lock()
                                    .expect("swarm token mutex should not be poisoned") =
                                    runtime.snapshot().state.token_usage.clone();
                            })
                        }
                    }),
                })
            })
        })
    }
}

fn build_swarm_runtime(
    config: AgentConfig,
    model_adapter: Arc<dyn ModelAdapter>,
    memory: Arc<Mutex<MemoryManager>>,
) -> AgentRuntime {
    let mut runtime = AgentRuntime::new(config, model_adapter);
    runtime.set_providers(default_providers(Arc::clone(&memory)));
    runtime.set_evaluators(default_evaluators(memory));
    runtime.init();
    runtime
}

async fn execute_swarm_tool(
    delegate_task: Option<Arc<anima_swarm::coordinator::CoordinatorDelegateFn>>,
    tool_context: ToolExecutionContext,
    agent: AgentState,
    user_message: Message,
    tool_call: ToolCall,
) -> TaskResult<Content> {
    match tool_call.name.as_str() {
        "delegate_task" => {
            let Some(delegate_task) = delegate_task else {
                return TaskResult::error("delegate_task is unavailable", 0);
            };

            let Some(worker_name) = string_arg(&tool_call, "worker_name") else {
                return TaskResult::error("delegate_task worker_name must be a string", 0);
            };
            let Some(task) = string_arg(&tool_call, "task") else {
                return TaskResult::error("delegate_task task must be a string", 0);
            };

            delegate_task(worker_name, task).await
        }
        "choose_speaker" => {
            let Some(delegate_task) = delegate_task else {
                return TaskResult::error("choose_speaker is unavailable", 0);
            };

            let Some(agent_name) = string_arg(&tool_call, "agent_name") else {
                return TaskResult::error("choose_speaker agent_name must be a string", 0);
            };
            let instruction = string_arg(&tool_call, "instruction").unwrap_or_default();

            delegate_task(agent_name, instruction).await
        }
        _ => tool_context.execute_tool(agent, user_message, tool_call),
    }
}

fn string_arg(tool_call: &ToolCall, key: &str) -> Option<String> {
    match tool_call.args.get(key) {
        Some(DataValue::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}
