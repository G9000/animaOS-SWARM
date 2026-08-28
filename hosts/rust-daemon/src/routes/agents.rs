use anima_memory::RecentMemoryOptions;

use super::contracts::{
    AgentConfigRequest, AgentEnvelope, AgentRecentMemoriesQuery, AgentRunEnvelope,
    AgentRuntimeSnapshotResponse, AgentUpdateRequest, AgentsEnvelope, DeleteResponse,
    MemoriesEnvelope, MemoryResponse, TaskRequest,
};
use super::ApiError;
use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
use crate::app::SharedDaemonState;
use crate::state::UpdateAgentError;

pub(crate) async fn handle_create_agent(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<AgentEnvelope, ApiError> {
    let request: AgentConfigRequest = super::parse_json_body(body)?;
    let config = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let (snapshot, persist_request) = {
        let mut guard = state.write().await;
        let snapshot = guard
            .create_agent(config)
            .map_err(|message| ApiError::bad_request(message))?;
        (snapshot, guard.control_plane_persist_request())
    };
    persist_request
        .save()
        .await
        .map_err(|error| ApiError::service_unavailable(error.to_string()))?;

    Ok(AgentEnvelope {
        agent: AgentRuntimeSnapshotResponse::from(&snapshot),
    })
}

pub(crate) async fn handle_list_agents(
    state: &SharedDaemonState,
) -> Result<AgentsEnvelope, ApiError> {
    let snapshots = {
        let guard = state.read().await;
        guard.list_agents()
    };

    Ok(AgentsEnvelope {
        agents: snapshots
            .iter()
            .map(AgentRuntimeSnapshotResponse::from)
            .collect(),
    })
}

pub(crate) async fn handle_get_agent(
    agent_id: &str,
    state: &SharedDaemonState,
) -> Result<AgentEnvelope, ApiError> {
    let snapshot = {
        let guard = state.read().await;
        guard.get_agent(agent_id)
    };

    match snapshot {
        Some(snapshot) => Ok(AgentEnvelope {
            agent: AgentRuntimeSnapshotResponse::from(&snapshot),
        }),
        None => Err(ApiError::not_found()),
    }
}

pub(crate) async fn handle_delete_agent(
    agent_id: &str,
    state: &SharedDaemonState,
) -> Result<DeleteResponse, ApiError> {
    let persist_request = {
        let mut guard = state.write().await;
        guard.remove_agent(agent_id);
        guard.control_plane_persist_request()
    };
    persist_request
        .save()
        .await
        .map_err(|error| ApiError::service_unavailable(error.to_string()))?;

    Ok(DeleteResponse { deleted: true })
}

pub(crate) async fn handle_update_agent(
    agent_id: &str,
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<AgentEnvelope, ApiError> {
    let request: AgentUpdateRequest = super::parse_json_body(body)?;
    let patch = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let (snapshot, persist_request) = {
        let mut guard = state.write().await;
        let snapshot = match guard.update_agent(agent_id, patch) {
            Ok(snapshot) => snapshot,
            Err(UpdateAgentError::InvalidTools(message)) => {
                return Err(ApiError::bad_request(message));
            }
            Err(UpdateAgentError::NotFound) => return Err(ApiError::not_found()),
        };
        (snapshot, guard.control_plane_persist_request())
    };
    persist_request
        .save()
        .await
        .map_err(|error| ApiError::service_unavailable(error.to_string()))?;

    Ok(AgentEnvelope {
        agent: AgentRuntimeSnapshotResponse::from(&snapshot),
    })
}

pub(crate) async fn handle_recent_agent_memories(
    agent_id: &str,
    query: AgentRecentMemoriesQuery,
    state: &SharedDaemonState,
) -> Result<MemoriesEnvelope, ApiError> {
    let (memory, runtime_agent_id) = {
        let guard = state.read().await;
        let Some(runtime_agent_id) = guard.agent_runtime_id(agent_id) else {
            return Err(ApiError::not_found());
        };
        (guard.memory_handle(), runtime_agent_id)
    };
    let memories = memory.read().await.get_recent(RecentMemoryOptions {
        agent_id: Some(runtime_agent_id),
        agent_name: None,
        scope: None,
        room_id: None,
        world_id: None,
        session_id: None,
        limit: query.limit,
    });

    Ok(MemoriesEnvelope {
        memories: memories.iter().map(MemoryResponse::from).collect(),
    })
}

pub(crate) async fn handle_run_agent(
    agent_id: &str,
    body: Vec<u8>,
    coordinator: &AgentRunCoordinator,
) -> Result<AgentRunEnvelope, ApiError> {
    let request: TaskRequest = super::parse_json_body(body)?;
    let content = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    coordinator
        .run(AgentRunRequest {
            agent_id: agent_id.to_string(),
            content,
            room: RunRoom::Generated,
            idempotency_key: None,
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::{
        handle_create_agent, handle_delete_agent,
        handle_run_agent as handle_run_agent_with_coordinator, handle_update_agent,
    };
    use crate::agent_runs::AgentRunCoordinator;
    use crate::app::SharedDaemonState;
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
    use crate::state::DaemonState;
    use anima_core::{
        AgentConfig, AgentSettings, AgentStatus, Content, ModelAdapter, ModelGenerateRequest,
        ModelGenerateResponse, ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use futures::executor::block_on;
    use futures::task::noop_waker;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use tokio::sync::{RwLock, Semaphore};

    async fn handle_run_agent(
        agent_id: &str,
        body: Vec<u8>,
        state: &SharedDaemonState,
    ) -> Result<crate::routes::AgentRunEnvelope, crate::routes::ApiError> {
        let coordinator = AgentRunCoordinator::new(Arc::clone(state), Arc::new(Semaphore::new(8)));
        handle_run_agent_with_coordinator(agent_id, body, &coordinator).await
    }

    struct PendingModelAdapter;

    struct CapturingModelAdapter {
        configs: Arc<Mutex<Vec<AgentConfig>>>,
    }

    struct PendingOnce<T> {
        value: Option<T>,
        pending: bool,
    }

    #[async_trait]
    impl ModelAdapter for PendingModelAdapter {
        fn provider(&self) -> &str {
            "pending"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            Ok(PendingOnce::new(ModelGenerateResponse {
                content: Content {
                    text: format!("{} handled task: pending", config.name),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                stop_reason: ModelStopReason::End,
            })
            .await)
        }
    }

    #[async_trait]
    impl ModelAdapter for CapturingModelAdapter {
        fn provider(&self) -> &str {
            "capturing"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            self.configs
                .lock()
                .expect("capture lock should not be poisoned")
                .push(config.clone());
            Ok(ModelGenerateResponse {
                content: Content {
                    text: "captured".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })
        }
    }

    impl<T: Unpin> Future for PendingOnce<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            if self.pending {
                self.pending = false;
                context.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(self.value.take().expect("pending-once value should exist"))
            }
        }
    }

    impl<T> PendingOnce<T> {
        fn new(value: T) -> Self {
            Self {
                value: Some(value),
                pending: true,
            }
        }
    }

    #[test]
    fn handle_run_agent_releases_state_lock_before_runtime_future_completes() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            PendingModelAdapter,
        ))));
        let agent_id = block_on(async {
            let mut guard = state.write().await;
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        });
        let mut future = Box::pin(handle_run_agent(
            &agent_id,
            br#"{"text":"run pending task"}"#.to_vec(),
            &state,
        ));
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        assert!(
            matches!(future.as_mut().poll(&mut context), Poll::Pending),
            "the first poll should suspend on the pending model adapter"
        );
        assert!(
            state.try_write().is_ok(),
            "daemon state lock should be released while the runtime future is pending"
        );

        let response = block_on(future);
        assert!(response.is_ok());
    }

    #[test]
    fn create_then_run_passes_canonical_tool_schemas_to_model_adapter() {
        let configs = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            CapturingModelAdapter {
                configs: Arc::clone(&configs),
            },
        ))));
        let created = block_on(handle_create_agent(
            br#"{"name":"Anima","model":"deterministic","tools":["read_file","write_file","bash"]}"#
                .to_vec(),
            &state,
        ))
        .expect("agent should be created through the request path");
        let agent_id = created.agent.state.id;

        block_on(handle_run_agent(
            &agent_id,
            br#"{"text":"exercise canonical tools"}"#.to_vec(),
            &state,
        ))
        .expect("agent should run through the runtime path");

        let configs = configs.lock().expect("capture lock should not be poisoned");
        assert_eq!(configs.len(), 1);
        let tools = configs[0]
            .tools
            .as_ref()
            .expect("model adapter should receive tools");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["read_file", "write_file", "bash"]
        );
        for tool in tools {
            assert!(!tool.description.is_empty());
            assert!(tool.parameters_schema.contains_key("type"));
            assert!(tool.parameters_schema.contains_key("properties"));
            assert!(tool.parameters_schema.contains_key("required"));
        }
    }

    #[test]
    fn handle_run_agent_keeps_agent_visible_while_runtime_future_is_pending() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            PendingModelAdapter,
        ))));
        let agent_id = block_on(async {
            let mut guard = state.write().await;
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        });
        let mut future = Box::pin(handle_run_agent(
            &agent_id,
            br#"{"text":"run pending task"}"#.to_vec(),
            &state,
        ));
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        assert!(
            matches!(future.as_mut().poll(&mut context), Poll::Pending),
            "the first poll should suspend on the pending model adapter"
        );

        block_on(async {
            let guard = state.read().await;
            let agents = guard.list_agents();
            assert_eq!(agents.len(), 1, "pending runs should remain listable");
            let snapshot = guard
                .get_agent(&agent_id)
                .expect("pending runs should remain readable");
            assert_eq!(snapshot.state.status, AgentStatus::Running);
            assert_eq!(
                guard.agent_runtime_id(&agent_id).as_deref(),
                Some(agent_id.as_str())
            );
        });

        let response = block_on(future);
        assert!(response.is_ok());
    }

    #[test]
    fn update_agent_patch_survives_in_flight_runtime_restoration() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            PendingModelAdapter,
        ))));
        let agent_id = block_on(async {
            let mut guard = state.write().await;
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        });
        let mut future = Box::pin(handle_run_agent(
            &agent_id,
            br#"{"text":"run pending task"}"#.to_vec(),
            &state,
        ));
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));

        block_on(handle_update_agent(
            &agent_id,
            br#"{"name":"updated-operator","tools":["read_file"]}"#.to_vec(),
            &state,
        ))
        .expect("patch should succeed while the run is pending");
        block_on(future).expect("pending run should complete");

        block_on(async {
            let guard = state.read().await;
            let snapshot = guard
                .get_agent(&agent_id)
                .expect("agent should be restored");
            assert_eq!(snapshot.state.config.name, "updated-operator");
            let tools = snapshot
                .state
                .config
                .tools
                .expect("patched tools should survive restoration");
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "read_file");
            assert!(!tools[0].description.is_empty());
            assert!(tools[0].parameters_schema.contains_key("required"));
        });
    }

    #[test]
    fn deleting_agent_during_in_flight_run_stays_deleted_and_persisted() {
        let store_path = std::env::temp_dir().join(format!(
            "anima-delete-race-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        let store_config = ControlPlaneStoreConfig::Json(store_path.clone());
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            PendingModelAdapter,
        ))));
        let agent_id = block_on(async {
            let mut guard = state.write().await;
            guard.set_control_plane_store(Some(store_config.clone()));
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        });
        let mut future = Box::pin(handle_run_agent(
            &agent_id,
            br#"{"text":"run pending task"}"#.to_vec(),
            &state,
        ));
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));

        block_on(handle_delete_agent(&agent_id, &state))
            .expect("deleting the checked-out agent should succeed");
        block_on(future).expect("the already-started request may finish");

        block_on(async {
            let guard = state.read().await;
            assert!(guard.get_agent(&agent_id).is_none());
            assert_eq!(guard.agent_count(), 0);
        });
        let persisted = block_on(load_control_plane_snapshot(&store_config))
            .expect("control-plane snapshot should load")
            .expect("control-plane snapshot should exist");
        assert!(persisted.agents.is_empty());

        let _ = std::fs::remove_file(store_path);
    }

    fn test_config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "gpt-5.4".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: Some("openai".into()),
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }
}
