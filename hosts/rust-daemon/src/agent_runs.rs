use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use anima_core::{AgentRuntimeSnapshot, Content, DataValue, TaskResult};
use anima_memory::{MemoryType, NewMemory};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::warn;

use crate::app::SharedDaemonState;
use crate::memory_store::save_memory_manager;
use crate::routes::{AgentRunEnvelope, AgentRuntimeSnapshotResponse, ApiError, TaskResultResponse};
use crate::state::DaemonState;

pub(crate) struct AgentRunPermit(OwnedSemaphorePermit);

type AgentLockMap = Arc<StdMutex<HashMap<String, Arc<Mutex<()>>>>>;

struct AgentLockCleanup {
    agent_id: String,
    agent_lock: Arc<Mutex<()>>,
    agent_locks: AgentLockMap,
}

impl Drop for AgentLockCleanup {
    fn drop(&mut self) {
        let mut locks = self
            .agent_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if Arc::strong_count(&self.agent_lock) == 2
            && locks
                .get(&self.agent_id)
                .is_some_and(|candidate| Arc::ptr_eq(candidate, &self.agent_lock))
        {
            locks.remove(&self.agent_id);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RunRoom {
    Generated,
    Stable(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AgentRunRequest {
    pub(crate) agent_id: String,
    pub(crate) content: Content,
    pub(crate) room: RunRoom,
    /// Forwarded into the runtime input so persisted tool steps are replay-safe.
    /// Whole-run completion is owned by the durable caller record (for example,
    /// a Telegram inbound item), not an in-memory response cache here.
    pub(crate) idempotency_key: Option<String>,
}

#[derive(Clone)]
pub(crate) struct AgentRunCoordinator {
    state: SharedDaemonState,
    run_limiter: Arc<Semaphore>,
    agent_locks: AgentLockMap,
}

impl AgentRunCoordinator {
    pub(crate) fn new(state: SharedDaemonState, run_limiter: Arc<Semaphore>) -> Self {
        Self {
            state,
            run_limiter,
            agent_locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    #[allow(dead_code)] // Used by daemon-owned connector and scheduler workers.
    pub(crate) async fn run(&self, request: AgentRunRequest) -> Result<AgentRunEnvelope, ApiError> {
        let permit = self.try_admit()?;
        self.run_admitted(request, permit).await
    }

    pub(crate) async fn run_admitted(
        &self,
        request: AgentRunRequest,
        permit: AgentRunPermit,
    ) -> Result<AgentRunEnvelope, ApiError> {
        self.run_with_commit_admitted(request, permit, |_, _, _| Ok(()))
            .await
    }

    /// Runs with a state commit captured in the same final control-plane snapshot.
    ///
    /// A hook that can fail must finish all validation before its first mutation;
    /// arbitrary `DaemonState` changes cannot be rolled back generically.
    #[allow(dead_code)] // Used by durable connector inbound processing.
    pub(crate) async fn run_with_commit<F>(
        &self,
        request: AgentRunRequest,
        commit: F,
    ) -> Result<AgentRunEnvelope, ApiError>
    where
        F: FnOnce(
                &mut DaemonState,
                &AgentRuntimeSnapshot,
                &TaskResult<Content>,
            ) -> Result<(), ApiError>
            + Send
            + 'static,
    {
        let permit = self.try_admit()?;
        self.run_with_commit_admitted(request, permit, commit).await
    }

    /// Runs durable background work after waiting for shared daemon admission.
    ///
    /// Interactive callers deliberately fail fast when the daemon is saturated,
    /// but daemon-owned workers must not turn temporary saturation into a durable
    /// connector error.
    pub(crate) async fn run_with_commit_waiting<F>(
        &self,
        request: AgentRunRequest,
        commit: F,
    ) -> Result<AgentRunEnvelope, ApiError>
    where
        F: FnOnce(
                &mut DaemonState,
                &AgentRuntimeSnapshot,
                &TaskResult<Content>,
            ) -> Result<(), ApiError>
            + Send
            + 'static,
    {
        let permit = self
            .run_limiter
            .clone()
            .acquire_owned()
            .await
            .map(AgentRunPermit)
            .map_err(|_| ApiError::service_unavailable("agent run admission is unavailable"))?;
        self.run_with_commit_admitted(request, permit, commit).await
    }

    pub(crate) async fn run_with_commit_admitted<F>(
        &self,
        request: AgentRunRequest,
        permit: AgentRunPermit,
        commit: F,
    ) -> Result<AgentRunEnvelope, ApiError>
    where
        F: FnOnce(
                &mut DaemonState,
                &AgentRuntimeSnapshot,
                &TaskResult<Content>,
            ) -> Result<(), ApiError>
            + Send
            + 'static,
    {
        let coordinator = self.clone();
        tokio::spawn(async move { coordinator.run_serialized(request, permit, commit).await })
            .await
            .map_err(|error| {
                warn!(error = %error, "agent run worker stopped unexpectedly");
                ApiError::service_unavailable("agent run worker stopped unexpectedly")
            })?
    }

    async fn run_serialized<F>(
        &self,
        request: AgentRunRequest,
        permit: AgentRunPermit,
        commit: F,
    ) -> Result<AgentRunEnvelope, ApiError>
    where
        F: FnOnce(
                &mut DaemonState,
                &AgentRuntimeSnapshot,
                &TaskResult<Content>,
            ) -> Result<(), ApiError>
            + Send
            + 'static,
    {
        let agent_lock = self.agent_lock(&request.agent_id);
        let _cleanup = AgentLockCleanup {
            agent_id: request.agent_id.clone(),
            agent_lock: Arc::clone(&agent_lock),
            agent_locks: Arc::clone(&self.agent_locks),
        };
        let _agent_guard = agent_lock.lock_owned().await;
        self.run_locked(request, permit, commit).await
    }

    async fn run_locked<F>(
        &self,
        mut request: AgentRunRequest,
        permit: AgentRunPermit,
        commit: F,
    ) -> Result<AgentRunEnvelope, ApiError>
    where
        F: FnOnce(
                &mut DaemonState,
                &AgentRuntimeSnapshot,
                &TaskResult<Content>,
            ) -> Result<(), ApiError>
            + Send,
    {
        let _run_permit = permit.0;

        if let Some(idempotency_key) = request.idempotency_key.take() {
            request
                .content
                .metadata
                .get_or_insert_with(Default::default)
                .insert("idempotencyKey".into(), DataValue::String(idempotency_key));
        }

        let Some((mut runtime, tool_context, running_persist_request)) = ({
            let mut guard = self.state.write().await;
            let taken = guard.take_agent_runtime(&request.agent_id);
            taken.map(|(runtime, tool_context)| {
                (runtime, tool_context, guard.control_plane_persist_request())
            })
        }) else {
            return Err(ApiError::not_found());
        };

        if let Err(error) = running_persist_request.save().await {
            let mut guard = self.state.write().await;
            guard.restore_agent_runtime(runtime);
            return Err(ApiError::service_unavailable(error.to_string()));
        }

        let result = match request.room {
            RunRoom::Generated => {
                runtime
                    .run_with_tools(request.content, |agent, user_message, tool_call| {
                        let tool_context = tool_context.clone();
                        async move {
                            tool_context
                                .execute_tool(agent, user_message, tool_call)
                                .await
                        }
                    })
                    .await
            }
            RunRoom::Stable(room_id) => {
                let history = runtime
                    .messages()
                    .iter()
                    .filter(|message| message.room_id == room_id)
                    .cloned()
                    .collect();
                runtime
                    .run_in_room_with_context_and_tools(
                        room_id,
                        history,
                        request.content,
                        |agent, user_message, tool_call| {
                            let tool_context = tool_context.clone();
                            async move {
                                tool_context
                                    .execute_tool(agent, user_message, tool_call)
                                    .await
                            }
                        },
                    )
                    .await
            }
        };

        let (
            snapshot,
            runtime_id,
            runtime_name,
            memory,
            memory_embeddings,
            memory_store,
            persist_request,
        ) = {
            let mut guard = self.state.write().await;
            let restored = guard.restore_agent_runtime(runtime);
            // Hooks may fail, but arbitrary `DaemonState` mutation cannot be
            // rolled back generically. Callers must perform all fallible
            // validation before their first mutation and then mutate as one
            // infallible unit. In particular, connector hooks must prevalidate
            // the inbound/outbound transition before changing either record.
            commit(&mut guard, &restored.0, &result)?;
            (
                restored.0,
                restored.1,
                restored.2,
                restored.3,
                restored.4,
                restored.5,
                guard.control_plane_persist_request(),
            )
        };
        persist_request
            .save()
            .await
            .map_err(|error| ApiError::service_unavailable(error.to_string()))?;

        persist_task_result_memory(
            &result,
            &runtime_id,
            &runtime_name,
            memory,
            memory_embeddings,
            memory_store,
        )
        .await;

        Ok(AgentRunEnvelope {
            agent: AgentRuntimeSnapshotResponse::from(&snapshot),
            result: TaskResultResponse::from(&result),
        })
    }

    fn agent_lock(&self, agent_id: &str) -> Arc<Mutex<()>> {
        self.agent_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub(crate) fn try_admit(&self) -> Result<AgentRunPermit, ApiError> {
        self.run_limiter
            .clone()
            .try_acquire_owned()
            .map(AgentRunPermit)
            .map_err(|_| ApiError::service_unavailable("too many concurrent run requests"))
    }

    #[cfg(test)]
    fn lock_count(&self) -> usize {
        self.agent_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

async fn persist_task_result_memory(
    result: &TaskResult<Content>,
    runtime_id: &str,
    runtime_name: &str,
    memory: crate::state::SharedMemoryStore,
    memory_embeddings: crate::memory_embeddings::SharedMemoryEmbeddings,
    memory_store: Option<crate::memory_store::MemoryStoreConfig>,
) {
    let Some(content) = result.data.as_ref() else {
        return;
    };

    let persist_result: Result<_, String> = {
        let mut memory_guard = memory.write().await;
        match memory_guard.add(NewMemory {
            agent_id: runtime_id.to_string(),
            agent_name: runtime_name.to_string(),
            memory_type: MemoryType::TaskResult,
            content: content.text.clone(),
            importance: 0.8,
            tags: Some(vec!["runtime".into(), "task-result".into()]),
            scope: None,
            room_id: None,
            world_id: None,
            session_id: None,
        }) {
            Ok(memory) => match save_memory_manager(memory_store.as_ref(), &memory_guard).await {
                Ok(()) => Ok(memory),
                Err(error) => Err(format!("failed to persist memory: {error}")),
            },
            Err(error) => Err(error.message().to_string()),
        }
    };
    match persist_result {
        Ok(memory) => {
            if let Err(error) = memory_embeddings.write().await.upsert_memory(&memory) {
                warn!(
                    agent_id = %runtime_id,
                    memory_id = %memory.id,
                    error = %error,
                    "failed to index runtime task result memory embedding"
                );
            }
        }
        Err(error) => {
            warn!(
                agent_id = %runtime_id,
                error = %error,
                "failed to persist runtime task result memory"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentRunCoordinator, AgentRunRequest, RunRoom};
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
    use crate::routes::ApiError;
    use crate::state::DaemonState;
    use anima_core::{
        AgentConfig, AgentConfigUpdate, AgentRuntime, AgentSettings, AgentStatus, Content,
        DataValue, Message, MessageRole, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse,
        ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::{RwLock, Semaphore};

    struct GateModelAdapter {
        calls: AtomicUsize,
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    struct CapturingModelAdapter {
        requests: Arc<StdMutex<Vec<ModelGenerateRequest>>>,
    }

    #[async_trait]
    impl ModelAdapter for GateModelAdapter {
        fn provider(&self) -> &str {
            "gate"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("release semaphore should remain open")
                .forget();
            Ok(model_response(
                request
                    .messages
                    .last()
                    .map(|message| message.content.text.as_str())
                    .unwrap_or("empty"),
            ))
        }
    }

    #[async_trait]
    impl ModelAdapter for CapturingModelAdapter {
        fn provider(&self) -> &str {
            "capturing"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            self.requests
                .lock()
                .expect("request capture should not be poisoned")
                .push(request.clone());
            Ok(model_response("captured"))
        }
    }

    #[tokio::test]
    async fn same_agent_runs_wait_then_both_execute() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let adapter = Arc::new(GateModelAdapter {
            calls: AtomicUsize::new(0),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let (coordinator, agent_id) = coordinator_with_agent(adapter.clone(), 2).await;

        let first_coordinator = coordinator.clone();
        let first_request = request(&agent_id, "first");
        let first = tokio::spawn(async move { first_coordinator.run(first_request).await });
        entered
            .acquire()
            .await
            .expect("first run should enter model")
            .forget();
        let second_coordinator = coordinator.clone();
        let second_request = request(&agent_id, "second");
        let second = tokio::spawn(async move { second_coordinator.run(second_request).await });
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);

        release.add_permits(1);
        entered
            .acquire()
            .await
            .expect("second run should enter after first completes")
            .forget();
        release.add_permits(1);

        assert!(first.await.expect("first task should join").is_ok());
        assert!(second.await.expect("second task should join").is_ok());
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn aborted_caller_does_not_cancel_restore_or_leave_a_stale_agent_lock() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let adapter = Arc::new(GateModelAdapter {
            calls: AtomicUsize::new(0),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(
            adapter.clone(),
        )));
        let agent_id = state
            .write()
            .await
            .create_agent(test_config("cancel-safe"))
            .expect("agent should be created")
            .state
            .id;
        let coordinator = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(2)));

        let caller_coordinator = coordinator.clone();
        let caller_request = request(&agent_id, "first");
        let caller = tokio::spawn(async move { caller_coordinator.run(caller_request).await });
        entered
            .acquire()
            .await
            .expect("first run should enter model")
            .forget();
        caller.abort();
        assert!(
            caller
                .await
                .expect_err("caller should be aborted")
                .is_cancelled(),
            "aborting the waiter should not abort the owned run"
        );
        release.add_permits(1);

        for _ in 0..100 {
            if coordinator.lock_count() == 0
                && state
                    .read()
                    .await
                    .get_agent(&agent_id)
                    .is_some_and(|snapshot| snapshot.state.status == AgentStatus::Completed)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(coordinator.lock_count(), 0);
        assert!(state.read().await.agents.contains_key(&agent_id));

        let retry_coordinator = coordinator.clone();
        let retry_request = request(&agent_id, "second");
        let retry = tokio::spawn(async move { retry_coordinator.run(retry_request).await });
        entered
            .acquire()
            .await
            .expect("subsequent run should enter model")
            .forget();
        release.add_permits(1);
        assert!(retry.await.expect("retry should join").is_ok());
    }

    #[tokio::test]
    async fn different_agents_run_concurrently_with_available_global_permits() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let adapter = Arc::new(GateModelAdapter {
            calls: AtomicUsize::new(0),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(
            adapter.clone(),
        )));
        let (first_id, second_id) = {
            let mut guard = state.write().await;
            let first = guard
                .create_agent(test_config("first"))
                .expect("first agent should be created")
                .state
                .id;
            let second = guard
                .create_agent(test_config("second"))
                .expect("second agent should be created")
                .state
                .id;
            (first, second)
        };
        let coordinator = AgentRunCoordinator::new(state, Arc::new(Semaphore::new(2)));

        let first_coordinator = coordinator.clone();
        let first_request = request(&first_id, "first");
        let first = tokio::spawn(async move { first_coordinator.run(first_request).await });
        let second_coordinator = coordinator.clone();
        let second_request = request(&second_id, "second");
        let second = tokio::spawn(async move { second_coordinator.run(second_request).await });
        entered
            .acquire_many(2)
            .await
            .expect("both agents should enter the model")
            .forget();
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        release.add_permits(2);
        assert!(first.await.expect("first task should join").is_ok());
        assert!(second.await.expect("second task should join").is_ok());
    }

    #[tokio::test]
    async fn stable_room_passes_only_that_rooms_history_to_model() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let adapter = Arc::new(CapturingModelAdapter {
            requests: Arc::clone(&requests),
        });
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(
            adapter.clone(),
        )));
        let agent_id = {
            let mut guard = state.write().await;
            let snapshot = guard
                .create_agent(test_config("room-agent"))
                .expect("agent should be created");
            let agent_id = snapshot.state.id.clone();
            let mut seeded = snapshot;
            seeded.messages = vec![
                message(&agent_id, "room-web", "ordinary message", MessageRole::User),
                message(
                    &agent_id,
                    "room-telegram",
                    "telegram question",
                    MessageRole::User,
                ),
                message(
                    &agent_id,
                    "room-telegram",
                    "telegram answer",
                    MessageRole::Assistant,
                ),
            ];
            seeded.message_count = seeded.messages.len();
            guard.agents.insert(
                agent_id.clone(),
                AgentRuntime::from_snapshot(seeded.clone(), adapter.clone()),
            );
            guard.agent_snapshots.insert(agent_id.clone(), seeded);
            agent_id
        };
        let coordinator = AgentRunCoordinator::new(state, Arc::new(Semaphore::new(2)));

        coordinator
            .run(AgentRunRequest {
                agent_id,
                content: Content {
                    text: "telegram follow-up".into(),
                    ..Content::default()
                },
                room: RunRoom::Stable("room-telegram".into()),
                idempotency_key: None,
            })
            .await
            .expect("stable room run should succeed");

        let captured = requests
            .lock()
            .expect("request capture should not be poisoned");
        let texts = captured[0]
            .messages
            .iter()
            .map(|message| message.content.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            ["telegram question", "telegram answer", "telegram follow-up"]
        );
        assert!(!texts.contains(&"ordinary message"));
    }

    #[tokio::test]
    async fn idempotency_key_is_propagated_to_runtime_input_metadata() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let adapter = Arc::new(CapturingModelAdapter {
            requests: Arc::clone(&requests),
        });
        let (coordinator, agent_id) = coordinator_with_agent(adapter, 2).await;

        coordinator
            .run(AgentRunRequest {
                agent_id,
                content: Content {
                    text: "retryable".into(),
                    metadata: Some(BTreeMap::from([(
                        "source".into(),
                        DataValue::String("telegram".into()),
                    )])),
                    attachments: None,
                },
                room: RunRoom::Stable("room-telegram".into()),
                idempotency_key: Some("connector:update:42".into()),
            })
            .await
            .expect("run should succeed");

        let captured = requests
            .lock()
            .expect("request capture should not be poisoned");
        let metadata = captured[0].messages[0]
            .content
            .metadata
            .as_ref()
            .expect("metadata should be present");
        assert_eq!(
            metadata.get("idempotencyKey"),
            Some(&DataValue::String("connector:update:42".into()))
        );
        assert_eq!(
            metadata.get("source"),
            Some(&DataValue::String("telegram".into()))
        );
    }

    #[tokio::test]
    async fn commit_runs_after_runtime_restore_and_before_final_snapshot() {
        let path = snapshot_path("commit-order");
        let config = ControlPlaneStoreConfig::Json(path.clone());
        let adapter = Arc::new(CapturingModelAdapter {
            requests: Arc::new(StdMutex::new(Vec::new())),
        });
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let agent_id = {
            let mut guard = state.write().await;
            guard.set_control_plane_store(Some(config.clone()));
            guard
                .create_agent(test_config("before-commit"))
                .expect("agent should be created")
                .state
                .id
        };
        let coordinator = AgentRunCoordinator::new(state, Arc::new(Semaphore::new(2)));
        let commit_agent_id = agent_id.clone();

        coordinator
            .run_with_commit(
                request(&agent_id, "commit me"),
                move |state, snapshot, _| {
                    assert_eq!(snapshot.state.id, commit_agent_id);
                    assert!(state.agents.contains_key(&commit_agent_id));
                    state
                        .update_agent(
                            &commit_agent_id,
                            AgentConfigUpdate {
                                name: Some("after-commit".into()),
                                ..AgentConfigUpdate::default()
                            },
                        )
                        .expect("commit mutation should succeed");
                    Ok(())
                },
            )
            .await
            .expect("run and commit should succeed");

        let persisted = load_control_plane_snapshot(&config)
            .await
            .expect("snapshot should load")
            .expect("snapshot should exist");
        assert_eq!(persisted.agents[0].state.config.name, "after-commit");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn failed_commit_leaves_runtime_restored_without_final_snapshot() {
        let path = snapshot_path("commit-failure");
        let config = ControlPlaneStoreConfig::Json(path.clone());
        let adapter = Arc::new(CapturingModelAdapter {
            requests: Arc::new(StdMutex::new(Vec::new())),
        });
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let agent_id = {
            let mut guard = state.write().await;
            guard.set_control_plane_store(Some(config.clone()));
            guard
                .create_agent(test_config("commit-failure"))
                .expect("agent should be created")
                .state
                .id
        };
        let coordinator = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(2)));

        let result = coordinator
            .run_with_commit(request(&agent_id, "fail commit"), |state, snapshot, _| {
                assert!(state.agents.contains_key(&snapshot.state.id));
                Err(ApiError::bad_request("commit rejected"))
            })
            .await;
        assert!(result.is_err());
        {
            let guard = state.read().await;
            assert!(guard.agents.contains_key(&agent_id));
            assert_eq!(
                guard
                    .get_agent(&agent_id)
                    .expect("runtime should remain visible")
                    .state
                    .status,
                AgentStatus::Completed
            );
        }
        let persisted = load_control_plane_snapshot(&config)
            .await
            .expect("snapshot should load")
            .expect("running snapshot should exist");
        assert_eq!(persisted.agents[0].state.status, AgentStatus::Running);
        let _ = std::fs::remove_file(path);
    }

    fn request(agent_id: &str, text: &str) -> AgentRunRequest {
        AgentRunRequest {
            agent_id: agent_id.to_string(),
            content: Content {
                text: text.to_string(),
                ..Content::default()
            },
            room: RunRoom::Generated,
            idempotency_key: None,
        }
    }

    async fn coordinator_with_agent(
        adapter: Arc<dyn ModelAdapter>,
        permits: usize,
    ) -> (AgentRunCoordinator, String) {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter)));
        let agent_id = state
            .write()
            .await
            .create_agent(test_config("operator"))
            .expect("agent should be created")
            .state
            .id;
        (
            AgentRunCoordinator::new(state, Arc::new(Semaphore::new(permits))),
            agent_id,
        )
    }

    fn message(agent_id: &str, room_id: &str, text: &str, role: MessageRole) -> Message {
        Message {
            id: format!("message-{room_id}-{text}"),
            agent_id: agent_id.to_string(),
            room_id: room_id.to_string(),
            content: Content {
                text: text.to_string(),
                ..Content::default()
            },
            role,
            created_at_ms: 1,
        }
    }

    fn model_response(text: &str) -> ModelGenerateResponse {
        ModelGenerateResponse {
            content: Content {
                text: text.to_string(),
                ..Content::default()
            },
            tool_calls: None,
            usage: TokenUsage::default(),
            stop_reason: ModelStopReason::End,
        }
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

    fn snapshot_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "anima-agent-runs-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ))
    }
}
