use anima_memory::RecentMemoryOptions;

use super::contracts::{
    AgentConfigRequest, AgentEnvelope, AgentRecentMemoriesQuery, AgentRunEnvelope,
    AgentRuntimeSnapshotResponse, AgentSummariesEnvelope, AgentSummaryResponse, AgentUpdateRequest,
    AgentsEnvelope, DeleteResponse, MemoriesEnvelope, MemoryResponse, TaskRequest,
};
use super::ApiError;
use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom, RUN_ADMISSION_SATURATED};
use crate::app::SharedDaemonState;
use crate::runs::RunSource;
use crate::state::UpdateAgentError;

pub(crate) const AGENT_BUSY_MESSAGE: &str =
    "Agent has a run in progress; wait for it to finish before deleting it";

/// Rooms owned by connectors, automations, jobs, agent-to-agent requests, and
/// mapped legacy sessions; the generic run route may not write into them, so
/// a client `roomId` can never alias a mapped `legacy-room:<hash>` session id
/// (spec §3.1).
const RESERVED_ROOM_PREFIXES: [&str; 5] = [
    "telegram:",
    "schedule:",
    "job:",
    "peer:",
    crate::sessions::LEGACY_ROOM_SESSION_PREFIX,
];

/// `RESERVED_ROOM_PREFIXES` spelled out for the rejection message below, so
/// the wording can't drift out of sync with the list it describes.
fn reserved_room_prefix_list() -> String {
    let (last, rest) = RESERVED_ROOM_PREFIXES
        .split_last()
        .expect("RESERVED_ROOM_PREFIXES is non-empty");
    format!("{}, and {last}", rest.join(", "))
}

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

pub(crate) async fn handle_list_agent_summaries(
    state: &SharedDaemonState,
) -> AgentSummariesEnvelope {
    let summaries = state.read().await.agent_summaries();
    AgentSummariesEnvelope {
        agents: summaries.iter().map(AgentSummaryResponse::from).collect(),
    }
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
        if guard.in_flight_runs(agent_id) > 0 {
            return Err(ApiError::conflict(AGENT_BUSY_MESSAGE));
        }
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
    let mut patch = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    // The route owns the control-plane transaction. Keep runtime readers and
    // publishers out until both durable representations agree. Saving a prepared
    // persistence request does not acquire the daemon state lock.
    let mut guard = state.write().await;
    let previous = guard.get_agent(agent_id).ok_or_else(ApiError::not_found)?;
    let yaml_update = guard
        .workspace
        .as_ref()
        .map(|workspace| {
            super::workspace_agent_yaml::WorkspaceAgentYamlUpdate::prepare(
                &workspace.root_path,
                &previous.state.name,
                &mut patch,
            )
        })
        .transpose()?
        .flatten();
    let snapshot = match guard.update_agent(agent_id, patch) {
        Ok(snapshot) => snapshot,
        Err(UpdateAgentError::InvalidTools(message)) => return Err(ApiError::bad_request(message)),
        Err(UpdateAgentError::NotFound) => return Err(ApiError::not_found()),
    };
    if let Some(update) = &yaml_update {
        if let Err(error) = update.apply() {
            guard.restore_agent_config(agent_id, previous.state.config);
            return Err(error);
        }
    }
    if let Err(error) = guard.control_plane_persist_request().save().await {
        guard.restore_agent_config(agent_id, previous.state.config);
        if let Some(update) = &yaml_update {
            update.rollback().map_err(|rollback| {
                ApiError::service_unavailable(format!(
                    "Agent persistence failed ({error}); anima.yaml rollback also failed: {}",
                    rollback.message
                ))
            })?;
        }
        return Err(ApiError::service_unavailable(error.to_string()));
    }

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
    let room = match request.room_id.as_deref() {
        Some(id)
            if id.trim().is_empty()
                || id.len() > 256
                || RESERVED_ROOM_PREFIXES
                    .iter()
                    .any(|prefix| id.starts_with(prefix)) =>
        {
            return Err(ApiError::bad_request(format!(
                "roomId must be non-empty, at most 256 bytes, and outside the reserved {} namespaces",
                reserved_room_prefix_list()
            )));
        }
        Some(id) => RunRoom::Stable(id.to_string()),
        None => RunRoom::Generated,
    };
    let content = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;
    // Waiting for a busy room or agent slot is bounded per agent (spec §16);
    // beyond that the route keeps its fail-fast saturation (spec §4.9).
    let waiting = coordinator
        .try_take_waiting_unit(agent_id)
        .ok_or_else(|| ApiError::service_unavailable(RUN_ADMISSION_SATURATED))?;

    coordinator
        .run_budgeted(
            AgentRunRequest {
                agent_id: agent_id.to_string(),
                content,
                room,
                idempotency_key: None,
                source: RunSource::Api,
                source_ref: None,
                parent: None,
            },
            waiting,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::{
        handle_create_agent, handle_delete_agent,
        handle_run_agent as handle_run_agent_with_coordinator, handle_update_agent,
    };
    use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
    use crate::app::SharedDaemonState;
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
    use crate::runs::RunSource;
    use crate::state::DaemonState;
    use anima_core::{
        AgentConfig, AgentSettings, AgentStatus, Content, DataValue, ModelAdapter,
        ModelGenerateRequest, ModelGenerateResponse, ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use axum::http::StatusCode;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::{RwLock, Semaphore};

    async fn handle_run_agent(
        agent_id: &str,
        body: Vec<u8>,
        state: &SharedDaemonState,
    ) -> Result<crate::routes::AgentRunEnvelope, crate::routes::ApiError> {
        let coordinator = AgentRunCoordinator::new(Arc::clone(state), Arc::new(Semaphore::new(8)));
        handle_run_agent_with_coordinator(agent_id, body, &coordinator).await
    }

    struct PendingModelAdapter {
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    struct CapturingModelAdapter {
        configs: Arc<Mutex<Vec<AgentConfig>>>,
    }

    struct RequestCapturingModelAdapter {
        requests: Arc<Mutex<Vec<ModelGenerateRequest>>>,
    }

    #[async_trait]
    impl ModelAdapter for RequestCapturingModelAdapter {
        fn provider(&self) -> &str {
            "request-capturing"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            self.requests
                .lock()
                .expect("capture lock should not be poisoned")
                .push(request.clone());
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
                    ..TokenUsage::default()
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
    async fn commit_for_an_agent_deleted_mid_run_is_discarded() {
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

        // Bypasses the route's in-flight guard, like an internal removal path.
        let persist_request = {
            let mut guard = state.write().await;
            guard.remove_agent(&agent_id);
            guard.control_plane_persist_request()
        };
        persist_request
            .save()
            .await
            .expect("deletion should persist");
        release.add_permits(1);
        let error = run
            .await
            .expect("run task should join")
            .expect_err("a commit for a deleted agent is discarded");

        assert_eq!(error.status(), StatusCode::NOT_FOUND);
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
        assert!(
            persisted.runs.is_empty(),
            "the discarded run belonged to a deleted agent"
        );

        let _ = std::fs::remove_file(store_path);
    }

    #[tokio::test]
    async fn deleting_an_agent_with_a_run_in_flight_is_rejected_until_the_run_finishes() {
        let store_path = std::env::temp_dir().join(format!(
            "anima-delete-busy-{}-{}.json",
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

        let error = handle_delete_agent(&agent_id, &state)
            .await
            .expect_err("an in-flight run blocks deletion");
        assert_eq!(error.status(), StatusCode::CONFLICT);
        assert_eq!(
            error.message(),
            "Agent has a run in progress; wait for it to finish before deleting it"
        );
        assert!(state.read().await.get_agent(&agent_id).is_some());

        release.add_permits(1);
        run.await
            .expect("run task should join")
            .expect("the run commits normally");
        handle_delete_agent(&agent_id, &state)
            .await
            .expect("deletion succeeds once the run finished");
        assert!(state.read().await.get_agent(&agent_id).is_none());
        let persisted = load_control_plane_snapshot(&store_config)
            .await
            .expect("control-plane snapshot should load")
            .expect("control-plane snapshot should exist");
        assert!(persisted.agents.is_empty());

        let _ = std::fs::remove_file(store_path);
    }

    #[tokio::test]
    async fn run_body_metadata_cannot_choose_the_runtime_retry_key() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            RequestCapturingModelAdapter {
                requests: Arc::clone(&requests),
            },
        ))));
        let agent_id = state
            .write()
            .await
            .create_agent(test_config("operator"))
            .expect("agent should be created")
            .state
            .id;

        handle_run_agent(
            &agent_id,
            br#"{"text":"keyed by the client","metadata":{"idempotencyKey":"telegram-a:update:42","idempotency_key":"client-key","retryKey":"client-key","retry_key":"client-key","source":"client"}}"#
                .to_vec(),
            &state,
        )
        .await
        .expect("run should succeed");

        let requests = requests
            .lock()
            .expect("capture lock should not be poisoned");
        let input = requests[0]
            .messages
            .last()
            .expect("the model receives the run input");
        let metadata = input
            .content
            .metadata
            .as_ref()
            .expect("other client metadata is kept");
        for key in ["retryKey", "retry_key", "idempotencyKey", "idempotency_key"] {
            assert!(
                !metadata.contains_key(key),
                "client metadata `{key}` reached the runtime input"
            );
        }
        assert_eq!(
            metadata.get("source"),
            Some(&DataValue::String("client".into()))
        );
        assert_eq!(
            state.read().await.runs.for_agent(&agent_id)[0].idempotency_key,
            None,
            "the run records no client-chosen retry key"
        );
    }

    #[tokio::test]
    async fn direct_run_for_a_helper_fails_fast_while_its_slot_is_held() {
        let (adapter, entered, release) = pending_adapter();
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let (companion_id, helper_id) = {
            let mut guard = state.write().await;
            let mut companion = test_config("companion");
            companion
                .settings
                .as_mut()
                .expect("test config has settings")
                .additional
                .insert("workspaceRole".into(), DataValue::String("lead".into()));
            let companion_id = guard
                .create_agent(companion)
                .expect("companion should be created")
                .state
                .id;
            let mut helper = test_config("helper");
            helper
                .settings
                .as_mut()
                .expect("test config has settings")
                .additional = BTreeMap::from([
                ("workspaceRole".into(), DataValue::String("helper".into())),
                (
                    "parentAgentId".into(),
                    DataValue::String(companion_id.clone()),
                ),
            ]);
            let helper_id = guard
                .create_agent(helper)
                .expect("helper should be created")
                .state
                .id;
            (companion_id, helper_id)
        };
        let coordinator = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
        // The companion's delegated run holds the helper's slot while it waits
        // in the model.
        let delegated = {
            let coordinator = coordinator.clone();
            let request = AgentRunRequest {
                agent_id: helper_id.clone(),
                content: Content {
                    text: "delegated task".into(),
                    ..Content::default()
                },
                room: RunRoom::Delegated {
                    parent_id: companion_id,
                },
                idempotency_key: None,
                source: RunSource::Delegation,
                source_ref: None,
                parent: None,
            };
            tokio::spawn(async move { coordinator.run(request).await })
        };
        entered
            .acquire()
            .await
            .expect("the delegated run should enter the model")
            .forget();

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            handle_run_agent_with_coordinator(
                &helper_id,
                br#"{"text":"bypass the companion"}"#.to_vec(),
                &coordinator,
            ),
        )
        .await
        .expect("an invalid run fails before waiting for the helper's slot")
        .expect_err("helpers run only through their companion");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            error.message(),
            "Helpers must run through their owning companion"
        );

        release.add_permits(1);
        delegated
            .await
            .expect("delegated run should join")
            .expect("delegated run should finish");
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

    #[tokio::test]
    async fn run_route_rejects_reserved_room_prefixes() {
        let state = Arc::new(RwLock::new(DaemonState::new()));
        let agent_id = state
            .write()
            .await
            .create_agent(test_config("operator"))
            .expect("agent should be created")
            .state
            .id;

        for room in [
            "telegram:connector-1",
            "schedule:schedule-1",
            "job:job-1",
            "peer:alice:bob",
            "legacy-room:abc",
        ] {
            let body = serde_json::json!({"text": "hello", "roomId": room})
                .to_string()
                .into_bytes();
            let error = handle_run_agent(&agent_id, body, &state)
                .await
                .expect_err(room);
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{room}");
            assert_eq!(
                error.message(),
                "roomId must be non-empty, at most 256 bytes, and outside the reserved telegram:, schedule:, job:, peer:, and legacy-room: namespaces"
            );
        }
        let accepted = handle_run_agent(
            &agent_id,
            br#"{"text":"hello","roomId":"direct:operator"}"#.to_vec(),
            &state,
        )
        .await
        .expect("ordinary rooms stay available");
        assert_eq!(accepted.result.status, "success");
        assert!(state
            .read()
            .await
            .get_agent(&agent_id)
            .unwrap()
            .messages
            .iter()
            .all(|message| message.room_id == "direct:operator"));
    }
}
