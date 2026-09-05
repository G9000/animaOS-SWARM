use anima_memory::RecentMemoryOptions;

use super::contracts::{
    AgentConfigRequest, AgentEnvelope, AgentRecentMemoriesQuery, AgentRunEnvelope,
    AgentRuntimeSnapshotResponse, AgentUpdateRequest, AgentsEnvelope, DeleteResponse,
    MemoriesEnvelope, MemoryResponse, TaskRequest,
};
use super::ApiError;
use crate::agent_runs::{AgentRunCoordinator, AgentRunPermit, AgentRunRequest, RunRoom};
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

#[allow(dead_code)] // HTTP deletion is coordinated by ConnectorManager; retained for unit coverage.
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
    permit: AgentRunPermit,
) -> Result<AgentRunEnvelope, ApiError> {
    let request: TaskRequest = super::parse_json_body(body)?;
    let room = match request.room_id.as_deref() {
        Some(id) if id.trim().is_empty() || id.len() > 256 || id.starts_with("peer:") => return Err(ApiError::bad_request_static("roomId must be non-empty, at most 256 bytes, and outside the reserved peer namespace")),
        Some(id) => RunRoom::Stable(id.to_string()),
        None => RunRoom::Generated,
    };
    let content = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    coordinator
        .run_admitted(
            AgentRunRequest {
                agent_id: agent_id.to_string(),
                content,
                room,
                idempotency_key: None,
            },
            permit,
        )
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
    use std::sync::{Arc, Mutex};
    use tokio::sync::{RwLock, Semaphore};

    async fn handle_run_agent(
        agent_id: &str,
        body: Vec<u8>,
        state: &SharedDaemonState,
    ) -> Result<crate::routes::AgentRunEnvelope, crate::routes::ApiError> {
        let coordinator = AgentRunCoordinator::new(Arc::clone(state), Arc::new(Semaphore::new(8)));
        let permit = coordinator
            .try_admit()
            .expect("test coordinator should admit the run");
        handle_run_agent_with_coordinator(agent_id, body, &coordinator, permit).await
    }

    struct PendingModelAdapter {
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    struct CapturingModelAdapter {
        configs: Arc<Mutex<Vec<AgentConfig>>>,
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
            self.entered.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("release semaphore should remain open")
                .forget();
            Ok(ModelGenerateResponse {
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

    #[tokio::test]
    async fn handle_run_agent_releases_state_lock_before_runtime_future_completes() {
        let (adapter, entered, release) = pending_adapter();
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let agent_id = {
            let mut guard = state.write().await;
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        };
        let run_state = Arc::clone(&state);
        let run_agent_id = agent_id.clone();
        let run = tokio::spawn(async move {
            handle_run_agent(
                &run_agent_id,
                br#"{"text":"run pending task"}"#.to_vec(),
                &run_state,
            )
            .await
        });
        entered
            .acquire()
            .await
            .expect("run should enter model")
            .forget();
        assert!(
            state.try_write().is_ok(),
            "daemon state lock should be released while the runtime future is pending"
        );

        release.add_permits(1);
        let response = run.await.expect("run task should join");
        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn create_then_run_passes_canonical_tool_schemas_to_model_adapter() {
        let configs = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            CapturingModelAdapter {
                configs: Arc::clone(&configs),
            },
        ))));
        let created = handle_create_agent(
            br#"{"name":"Anima","model":"deterministic","tools":["read_file","write_file","bash"]}"#
                .to_vec(),
            &state,
        )
        .await
        .expect("agent should be created through the request path");
        let agent_id = created.agent.state.id;

        handle_run_agent(
            &agent_id,
            br#"{"text":"exercise canonical tools"}"#.to_vec(),
            &state,
        )
        .await
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
            [
                "read_file",
                "write_file",
                "bash",
                "list_workspace_agents",
                "send_message",
                "broadcast_message"
            ]
        );
        for tool in tools {
            assert!(!tool.description.is_empty());
            assert!(tool.parameters_schema.contains_key("type"));
            assert!(tool.parameters_schema.contains_key("properties"));
            assert!(tool.parameters_schema.contains_key("required"));
        }
    }

    #[tokio::test]
    async fn handle_run_agent_keeps_agent_visible_while_runtime_future_is_pending() {
        let (adapter, entered, release) = pending_adapter();
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let agent_id = {
            let mut guard = state.write().await;
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        };
        let run_state = Arc::clone(&state);
        let run_agent_id = agent_id.clone();
        let run = tokio::spawn(async move {
            handle_run_agent(
                &run_agent_id,
                br#"{"text":"run pending task"}"#.to_vec(),
                &run_state,
            )
            .await
        });
        entered
            .acquire()
            .await
            .expect("run should enter model")
            .forget();

        {
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
        }

        release.add_permits(1);
        let response = run.await.expect("run task should join");
        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn update_agent_patch_survives_in_flight_runtime_restoration() {
        let (adapter, entered, release) = pending_adapter();
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let agent_id = {
            let mut guard = state.write().await;
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        };
        let run_state = Arc::clone(&state);
        let run_agent_id = agent_id.clone();
        let run = tokio::spawn(async move {
            handle_run_agent(
                &run_agent_id,
                br#"{"text":"run pending task"}"#.to_vec(),
                &run_state,
            )
            .await
        });
        entered
            .acquire()
            .await
            .expect("run should enter model")
            .forget();

        handle_update_agent(
            &agent_id,
            br#"{"name":"updated-operator","tools":["read_file"]}"#.to_vec(),
            &state,
        )
        .await
        .expect("patch should succeed while the run is pending");
        release.add_permits(1);
        run.await
            .expect("run task should join")
            .expect("pending run should complete");

        {
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
        }
    }

    #[tokio::test]
    async fn deleting_agent_during_in_flight_run_stays_deleted_and_persisted() {
        let store_path = std::env::temp_dir().join(format!(
            "anima-delete-race-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        let store_config = ControlPlaneStoreConfig::Json(store_path.clone());
        let (adapter, entered, release) = pending_adapter();
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let agent_id = {
            let mut guard = state.write().await;
            guard.set_control_plane_store(Some(store_config.clone()));
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        };
        let run_state = Arc::clone(&state);
        let run_agent_id = agent_id.clone();
        let run = tokio::spawn(async move {
            handle_run_agent(
                &run_agent_id,
                br#"{"text":"run pending task"}"#.to_vec(),
                &run_state,
            )
            .await
        });
        entered
            .acquire()
            .await
            .expect("run should enter model")
            .forget();

        handle_delete_agent(&agent_id, &state)
            .await
            .expect("deleting the checked-out agent should succeed");
        release.add_permits(1);
        run.await
            .expect("run task should join")
            .expect("the already-started request may finish");

        {
            let guard = state.read().await;
            assert!(guard.get_agent(&agent_id).is_none());
            assert_eq!(guard.agent_count(), 0);
        }
        let persisted = load_control_plane_snapshot(&store_config)
            .await
            .expect("control-plane snapshot should load")
            .expect("control-plane snapshot should exist");
        assert!(persisted.agents.is_empty());

        let _ = std::fs::remove_file(store_path);
    }

    fn pending_adapter() -> (Arc<dyn ModelAdapter>, Arc<Semaphore>, Arc<Semaphore>) {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        (
            Arc::new(PendingModelAdapter {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
            entered,
            release,
        )
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
