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
use crate::tools::ToolExecutionContext;

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
                let config = with_swarm_messaging_tools(context.config.clone());
                let inbox = context.inbox.clone();
                let participants = context.participants.clone();
                let mut initial_runtime = build_swarm_runtime(
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
                        move |input: String| {
                            let runtime = Arc::clone(&runtime);
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
                                    .run_with_tools(
                                        Content {
                                            text: input,
                                            attachments: None,
                                            metadata: None,
                                        },
                                        |agent, user_message, tool_call| {
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
    memory: SharedMemoryStore,
    memory_embeddings: SharedMemoryEmbeddings,
    memory_store: Option<crate::memory_store::MemoryStoreConfig>,
    inbox: Arc<CoordinatorInboxFn>,
    participants: Arc<CoordinatorParticipantsFn>,
    event_listener: Arc<dyn Fn(EngineEvent) + Send + Sync>,
) -> AgentRuntime {
    let mut runtime = AgentRuntime::new(config, model_adapter);
    runtime.set_event_listener(event_listener);
    let mut providers = default_providers(Arc::clone(&memory));
    providers.push(Arc::new(SwarmInboxProvider { inbox }));
    providers.push(Arc::new(SwarmParticipantsProvider { participants }));
    runtime.set_providers(providers);
    runtime.set_evaluators(default_evaluators(memory, memory_embeddings, memory_store));
    runtime.init();
    runtime
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

fn with_swarm_messaging_tools(mut config: AgentConfig) -> AgentConfig {
    let mut tools = config.tools.take().unwrap_or_default();
    push_tool_if_missing(&mut tools, send_message_tool_descriptor());
    push_tool_if_missing(&mut tools, broadcast_message_tool_descriptor());
    config.tools = Some(tools);
    config
}

fn push_tool_if_missing(tools: &mut Vec<ToolDescriptor>, descriptor: ToolDescriptor) {
    if !tools.iter().any(|tool| tool.name == descriptor.name) {
        tools.push(descriptor);
    }
}

fn send_message_tool_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: "send_message".into(),
        description: "Send a message to another live swarm agent by coordinator agent id or configured agent name".into(),
        parameters: send_message_parameters(),
        examples: None,
    }
}

fn send_message_parameters() -> BTreeMap<String, DataValue> {
    let mut properties = BTreeMap::new();
    properties.insert(
        "to_agent_id".into(),
        string_parameter("Coordinator agent id to receive the message"),
    );
    properties.insert(
        "to_agent_name".into(),
        string_parameter("Configured swarm agent name to receive the message"),
    );
    properties.insert(
        "message".into(),
        string_parameter("Message text to deliver"),
    );

    BTreeMap::from([
        ("type".into(), DataValue::String("object".into())),
        ("properties".into(), DataValue::Object(properties)),
        (
            "required".into(),
            DataValue::Array(vec![DataValue::String("message".into())]),
        ),
    ])
}

fn broadcast_message_tool_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: "broadcast_message".into(),
        description: "Broadcast a message to every other live swarm agent".into(),
        parameters: object_parameters(vec![("message", "Message text to broadcast")]),
        examples: None,
    }
}

fn object_parameters(fields: Vec<(&str, &str)>) -> BTreeMap<String, DataValue> {
    let mut properties = BTreeMap::new();
    let mut required = Vec::with_capacity(fields.len());

    for (name, description) in fields {
        properties.insert(name.into(), string_parameter(description));
        required.push(DataValue::String(name.into()));
    }

    BTreeMap::from([
        ("type".into(), DataValue::String("object".into())),
        ("properties".into(), DataValue::Object(properties)),
        ("required".into(), DataValue::Array(required)),
    ])
}

fn string_parameter(description: &str) -> DataValue {
    DataValue::Object(BTreeMap::from([
        ("type".into(), DataValue::String("string".into())),
        ("description".into(), DataValue::String(description.into())),
    ]))
}
