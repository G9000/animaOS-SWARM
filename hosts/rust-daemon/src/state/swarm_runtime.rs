use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anima_core::{AgentConfig, AgentRuntime, Content, EngineEvent, ModelAdapter, TokenUsage};
use anima_swarm::coordinator::{
    CoordinatorAgentFactoryContext, CoordinatorAgentFactoryFn, CoordinatorAgentShell,
};
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
        let model_adapter = Arc::clone(&self.model_adapter);
        let tool_registry = self.tool_registry.clone();
        let process_manager = Arc::clone(&self.process_manager);
        let db = self.db.clone();

        Arc::new(move |context: CoordinatorAgentFactoryContext| {
            let memory = Arc::clone(&memory);
            let memory_embeddings = Arc::clone(&memory_embeddings);
            let model_adapter = Arc::clone(&model_adapter);
            let tool_registry = tool_registry.clone();
            let process_manager = Arc::clone(&process_manager);
            let event_stream = event_stream.clone();
            let db = db.clone();

            Box::pin(async move {
                let tool_context = ToolExecutionContext::new(
                    Arc::clone(&memory),
                    Arc::clone(&memory_embeddings),
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
                let mut initial_runtime = build_swarm_runtime(
                    context.config.clone(),
                    Arc::clone(&model_adapter),
                    Arc::clone(&memory),
                    Arc::clone(&memory_embeddings),
                    Arc::clone(&runtime_events),
                );
                if let Some(db) = &db {
                    initial_runtime.set_database(Arc::clone(db));
                }
                let runtime = Arc::new(AsyncMutex::new(initial_runtime));
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
                        let delegate_tasks = context.delegate_tasks.clone();
                        let tool_context = tool_context.clone();
                        let runtime_events = Arc::clone(&runtime_events);
                        let db = db.clone();
                        move |input: String| {
                            let runtime = Arc::clone(&runtime);
                            let config = config.clone();
                            let memory = Arc::clone(&memory);
                            let memory_embeddings = Arc::clone(&memory_embeddings);
                            let model_adapter = Arc::clone(&model_adapter);
                            let token_usage = Arc::clone(&token_usage);
                            let needs_reset = Arc::clone(&needs_reset);
                            let delegate_task = delegate_task.clone();
                            let delegate_tasks = delegate_tasks.clone();
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
                                            let tool_context = tool_context.clone();
                                            async move {
                                                execute_swarm_tool(
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
    event_listener: Arc<dyn Fn(EngineEvent) + Send + Sync>,
) -> AgentRuntime {
    let mut runtime = AgentRuntime::new(config, model_adapter);
    runtime.set_event_listener(event_listener);
    runtime.set_providers(default_providers(Arc::clone(&memory)));
    runtime.set_evaluators(default_evaluators(memory, memory_embeddings));
    runtime.init();
    runtime
}
