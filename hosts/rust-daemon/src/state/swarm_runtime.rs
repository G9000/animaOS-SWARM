use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentRuntime, Content, DataValue, EngineEvent, Message, ModelAdapter, Provider,
    ProviderResult, TokenUsage, ToolDescriptor,
};
use anima_swarm::coordinator::{
    CoordinatorAgentFactoryContext, CoordinatorAgentFactoryFn, CoordinatorAgentShell,
    CoordinatorInboxFn, CoordinatorParticipantsFn,
};
use async_trait::async_trait;
use tokio::sync::Mutex as AsyncMutex;

use crate::components::{default_evaluators, default_providers};
use crate::events::EventFanout;
use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::tools::{ToolExecutionContext, ToolRegistry};

use super::runtime_events::publish_runtime_event;
use super::swarm_tools::execute_swarm_tool;
use super::{DaemonState, SharedMemoryStore};

impl DaemonState {
    pub(super) fn swarm_agent_factory(
        &self,
        event_stream: EventFanout,
    ) -> Arc<CoordinatorAgentFactoryFn> {
        let memory = Arc::clone(&self.memory);
        let memory_embeddings = Arc::clone(&self.memory_embeddings);
        let memory_store = self.memory_store.clone();
        let model_adapter = Arc::clone(&self.model_adapter);
        let tool_registry = self.tool_registry.clone();
        let process_manager = Arc::clone(&self.process_manager);
        let db = self.db.clone();

        Arc::new(move |context: CoordinatorAgentFactoryContext| {
            let memory = Arc::clone(&memory);
            let memory_embeddings = Arc::clone(&memory_embeddings);
            let memory_store = memory_store.clone();
            let model_adapter = Arc::clone(&model_adapter);
            let tool_registry = tool_registry.clone();
            let process_manager = Arc::clone(&process_manager);
            let event_stream = event_stream.clone();
            let db = db.clone();

            Box::pin(async move {
                let config = with_swarm_messaging_tools(context.config.clone(), &tool_registry);
                let tool_context = ToolExecutionContext::new(
                    Arc::clone(&memory),
                    Arc::clone(&memory_embeddings),
                    memory_store.clone(),
                    tool_registry,
                    process_manager,
                );
                let runtime_events: Arc<dyn Fn(EngineEvent) + Send + Sync> = Arc::new({
                    let event_stream = event_stream.clone();
                    let agent_name = context.config.name.clone();
                    move |event: EngineEvent| {
                        publish_runtime_event(&event_stream, &agent_name, event);
                    }
                });
                let persistence_agent_id =
                    stable_swarm_persistence_agent_id(&context.swarm_id, &config.name);
                let inbox = context.inbox.clone();
                let participants = context.participants.clone();
                let mut initial_runtime = build_swarm_runtime(
                    persistence_agent_id.clone(),
                    config.clone(),
                    Arc::clone(&model_adapter),
                    Arc::clone(&memory),
                    Arc::clone(&memory_embeddings),
                    memory_store.clone(),
                    inbox.clone(),
                    participants.clone(),
                    Arc::clone(&runtime_events),
                );
                if let Some(db) = &db {
                    initial_runtime.set_database(Arc::clone(db));
                }
                let runtime = Arc::new(AsyncMutex::new(initial_runtime));
                let config = config.clone();
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
                        let delegate_tasks = context.delegate_tasks.clone();
                        let send = context.send.clone();
                        let broadcast = context.broadcast.clone();
                        let inbox = inbox.clone();
                        let participants = participants.clone();
                        let tool_context = tool_context.clone();
                        let runtime_events = Arc::clone(&runtime_events);
                        let db = db.clone();
                        move |input: Content| {
                            let runtime = Arc::clone(&runtime);
                            let persistence_agent_id = persistence_agent_id.clone();
                            let config = config.clone();
                            let memory = Arc::clone(&memory);
                            let memory_embeddings = Arc::clone(&memory_embeddings);
                            let memory_store = memory_store.clone();
                            let model_adapter = Arc::clone(&model_adapter);
                            let token_usage = Arc::clone(&token_usage);
                            let needs_reset = Arc::clone(&needs_reset);
                            let delegate_task = delegate_task.clone();
                            let delegate_tasks = delegate_tasks.clone();
                            let send = send.clone();
                            let broadcast = broadcast.clone();
                            let inbox = inbox.clone();
                            let participants = participants.clone();
                            let tool_context = tool_context.clone();
                            let runtime_events = Arc::clone(&runtime_events);
                            let db = db.clone();
                            Box::pin(async move {
                                let mut runtime = runtime.lock().await;
                                if needs_reset.swap(false, Ordering::AcqRel) {
                                    let mut new_runtime = build_swarm_runtime(
                                        persistence_agent_id,
                                        config,
                                        Arc::clone(&model_adapter),
                                        Arc::clone(&memory),
                                        Arc::clone(&memory_embeddings),
                                        memory_store,
                                        inbox,
                                        participants.clone(),
                                        Arc::clone(&runtime_events),
                                    );
                                    if let Some(db) = &db {
                                        new_runtime.set_database(Arc::clone(db));
                                    }
                                    *runtime = new_runtime;
                                }
                                let result = runtime
                                    .run_with_tools(input, |agent, user_message, tool_call| {
                                        let delegate_task = delegate_task.clone();
                                        let delegate_tasks = delegate_tasks.clone();
                                        let send = send.clone();
                                        let broadcast = broadcast.clone();
                                        let participants = participants.clone();
                                        let tool_context = tool_context.clone();
                                        async move {
                                            execute_swarm_tool(
                                                send,
                                                broadcast,
                                                participants,
                                                delegate_task,
                                                delegate_tasks,
                                                tool_context,
                                                agent,
                                                user_message,
                                                tool_call,
                                            )
                                            .await
                                        }
                                    })
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
    persistence_agent_id: String,
    config: AgentConfig,
    model_adapter: Arc<dyn ModelAdapter>,
    memory: SharedMemoryStore,
    memory_embeddings: SharedMemoryEmbeddings,
    memory_store: Option<crate::memory_store::MemoryStoreConfig>,
    inbox: Arc<CoordinatorInboxFn>,
    participants: Arc<CoordinatorParticipantsFn>,
    event_listener: Arc<dyn Fn(EngineEvent) + Send + Sync>,
) -> AgentRuntime {
    let mut runtime = AgentRuntime::new(config, model_adapter);
    runtime.set_persistence_agent_id(persistence_agent_id);
    runtime.set_event_listener(event_listener);
    let mut providers = default_providers(Arc::clone(&memory));
    providers.push(Arc::new(SwarmInboxProvider { inbox }));
    providers.push(Arc::new(SwarmParticipantsProvider { participants }));
    runtime.set_providers(providers);
    runtime.set_evaluators(default_evaluators(memory, memory_embeddings, memory_store));
    runtime.init();
    runtime
}

fn stable_swarm_persistence_agent_id(swarm_id: &str, agent_name: &str) -> String {
    format!("{swarm_id}:agent:{agent_name}")
}

struct SwarmParticipantsProvider {
    participants: Arc<CoordinatorParticipantsFn>,
}

#[async_trait]
impl Provider for SwarmParticipantsProvider {
    fn name(&self) -> &str {
        "swarm_participants"
    }

    fn description(&self) -> &str {
        "Provides live swarm participant names and coordinator ids"
    }

    async fn get(
        &self,
        _runtime: &AgentRuntime,
        _message: &Message,
    ) -> Result<ProviderResult, String> {
        let participants = (self.participants)().await?;
        let text = if participants.is_empty() {
            "no live swarm participants".to_string()
        } else {
            participants
                .iter()
                .map(|participant| format!("{} ({})", participant.agent_name, participant.agent_id))
                .collect::<Vec<_>>()
                .join(" | ")
        };

        let mut metadata = BTreeMap::new();
        metadata.insert(
            "kind".into(),
            DataValue::String("swarm_participants".into()),
        );
        metadata.insert(
            "participantCount".into(),
            DataValue::Number(participants.len() as f64),
        );

        Ok(ProviderResult {
            text,
            metadata: Some(metadata),
        })
    }
}

struct SwarmInboxProvider {
    inbox: Arc<CoordinatorInboxFn>,
}

#[async_trait]
impl Provider for SwarmInboxProvider {
    fn name(&self) -> &str {
        "swarm_inbox"
    }

    fn description(&self) -> &str {
        "Provides messages delivered to this swarm agent"
    }

    async fn get(
        &self,
        _runtime: &AgentRuntime,
        _message: &Message,
    ) -> Result<ProviderResult, String> {
        let messages = (self.inbox)().await?;
        let text = if messages.is_empty() {
            "no swarm messages".to_string()
        } else {
            messages
                .iter()
                .map(|message| {
                    format!(
                        "from {} to {}: {}",
                        message.from, message.to, message.content.text
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };

        let mut metadata = BTreeMap::new();
        metadata.insert("kind".into(), DataValue::String("swarm_inbox".into()));
        metadata.insert(
            "messageCount".into(),
            DataValue::Number(messages.len() as f64),
        );

        Ok(ProviderResult {
            text,
            metadata: Some(metadata),
        })
    }
}

fn with_swarm_messaging_tools(
    mut config: AgentConfig,
    tool_registry: &ToolRegistry,
) -> AgentConfig {
    let mut tools = config.tools.take().unwrap_or_default();
    let messaging_tools = tool_registry
        .resolve_descriptors(["send_message", "broadcast_message"])
        .expect("swarm messaging tools must be registered");
    for descriptor in messaging_tools {
        replace_or_push_tool(&mut tools, descriptor);
    }
    config.tools = Some(tools);
    config
}

fn replace_or_push_tool(tools: &mut Vec<ToolDescriptor>, descriptor: ToolDescriptor) {
    let mut replaced = false;
    for existing in tools.iter_mut().filter(|tool| tool.name == descriptor.name) {
        *existing = descriptor.clone();
        replaced = true;
    }
    if !replaced {
        tools.push(descriptor);
    }
}

#[cfg(test)]
mod tests {
    use super::with_swarm_messaging_tools;
    use crate::tools::ToolRegistry;
    use anima_core::{AgentConfig, ToolDescriptor};

    #[test]
    fn swarm_messaging_tools_use_registry_descriptors() {
        let registry = ToolRegistry::new();
        let config = AgentConfig {
            name: "worker".into(),
            model: "deterministic".into(),
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
        };

        let config = with_swarm_messaging_tools(config, &registry);
        let tools = config.tools.expect("swarm tools");

        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["send_message", "broadcast_message"]
        );
        for name in ["send_message", "broadcast_message"] {
            assert_eq!(
                tools.iter().find(|tool| tool.name == name),
                registry.descriptor(name).as_ref(),
                "swarm runtime descriptor for {name} drifted from the registry"
            );
        }
    }

    #[test]
    fn swarm_messaging_tools_replace_existing_entries_in_place() {
        let registry = ToolRegistry::new();
        let mut config = agent_config();
        config.tools = Some(vec![
            untrusted_descriptor("broadcast_message"),
            untrusted_descriptor("custom_tool"),
            untrusted_descriptor("send_message"),
            untrusted_descriptor("send_message"),
        ]);

        let config = with_swarm_messaging_tools(config, &registry);
        let tools = config.tools.expect("swarm tools");

        assert_eq!(tools.len(), 4);
        assert_eq!(
            tools[0],
            registry
                .descriptor("broadcast_message")
                .expect("broadcast descriptor")
        );
        assert_eq!(tools[1], untrusted_descriptor("custom_tool"));
        assert_eq!(
            tools[2],
            registry
                .descriptor("send_message")
                .expect("send descriptor")
        );
        assert_eq!(
            tools[3],
            registry
                .descriptor("send_message")
                .expect("duplicate send descriptor")
        );
    }

    fn untrusted_descriptor(name: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: "Caller-provided descriptor".into(),
            parameters_schema: Default::default(),
            examples: None,
        }
    }

    fn agent_config() -> AgentConfig {
        AgentConfig {
            name: "worker".into(),
            model: "deterministic".into(),
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
        }
    }
}
