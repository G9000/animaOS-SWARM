use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentDefinition, ApprovalDecision, AuthoritativePolicyChange, AuthoritativePolicyState,
    CapabilityError, CapabilityExecutionContext, CapabilityKind, CapabilityManifest,
    CapabilityManifestInput, CapabilityReferenceId, CapabilityResult, CapabilityRetryAuthorization,
    CreateRun, CurrentPolicyResolution, CurrentPolicyResolver, DefinitionPin, DefinitionResolver,
    DurableAgentEngine, DurableCapabilityResult, DurableCapabilityStatus, DurableEngineConfig,
    EngineBoundaryAction, EngineCapabilityResult, EngineCapabilityRuntime, EngineControlSignal,
    EngineCrashInjector, EngineCrashPoint, EngineError, EngineErrorCode, EngineLiveEvent,
    EngineLiveEventSink, EnginePolicyRequest, EngineRunOutcome, EventReplayPage, ExecutionCommit,
    ExecutionCommitOutcome, ExecutionLease, ExecutionStep, ExecutionStore, ExecutionStoreError,
    ExecutionStoreErrorCode, GrantAuthorityKey, InMemoryExecutionStore, InvocationAttemptRecord,
    LifecyclePolicy, MemoryPolicy, ModelPolicy, PolicyContext, PolicyRestrictions, ProfileRef,
    RecoveryAction, RecoveryMode, ResolvedCapability, RiskLevel, Run, RuntimeCompatibility,
    RuntimeEventKind, RuntimeLimits, Session, SessionConcurrencyPolicy, StoreHistoryPage,
    StoreReadPage, StoredRun,
};
use anima_model_adapters::{ProviderAdapterConfig, ProviderCredential, ProviderModelAdapter};
use async_trait::async_trait;
use axum::{extract::Json, routing::post, Router};
use serde_json::Value;
use tokio::net::TcpListener;
use uuid::Uuid;

#[derive(Clone)]
struct StaticDefinition(AgentDefinition);

#[async_trait]
impl DefinitionResolver for StaticDefinition {
    async fn resolve(
        &self,
        _owner_id: Uuid,
        pin: &DefinitionPin,
    ) -> Result<AgentDefinition, EngineError> {
        if pin.id() == self.0.id && pin.version() == self.0.version {
            Ok(self.0.clone())
        } else {
            Err(EngineError::new(EngineErrorCode::DefinitionUnavailable))
        }
    }
}

struct AllowCurrentPolicy;

#[async_trait]
impl CurrentPolicyResolver for AllowCurrentPolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "provider-loop-actor",
            request.definition().id.clone(),
            request.definition().version,
            "provider-loop-workspace",
            CapabilityReferenceId::new(request.invocation().run_id()),
            request.manifest(),
            request.invocation(),
            request.definition().approval_policy_revision,
            PolicyRestrictions::default(),
            1_000_000,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        Ok(CurrentPolicyResolution::new(context, vec![], None))
    }
}

struct ApprovalCurrentPolicy {
    approved: AtomicBool,
}

#[async_trait]
impl CurrentPolicyResolver for ApprovalCurrentPolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "provider-loop-actor",
            request.definition().id.clone(),
            request.definition().version,
            "provider-loop-workspace",
            CapabilityReferenceId::new(request.invocation().run_id()),
            request.manifest(),
            request.invocation(),
            request.definition().approval_policy_revision,
            PolicyRestrictions::default(),
            1_000_000,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        let approval = if self.approved.load(Ordering::SeqCst) {
            request
                .pending_approval()
                .map(|pending| ApprovalDecision::new_approved(pending.clone(), 1_000_000))
                .transpose()
                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?
        } else {
            None
        };
        Ok(CurrentPolicyResolution::new(context, vec![], approval))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CrashAfterCommit {
    Never,
    ModelCompleted,
    CapabilityCompleted,
}

struct CrashAfterCommitStore {
    inner: InMemoryExecutionStore,
    boundary: CrashAfterCommit,
    armed: AtomicBool,
}

impl CrashAfterCommitStore {
    fn new(boundary: CrashAfterCommit, clock: Arc<anima_core::ManualExecutionClock>) -> Self {
        Self {
            inner: InMemoryExecutionStore::with_clock(clock),
            boundary,
            armed: AtomicBool::new(boundary != CrashAfterCommit::Never),
        }
    }

    fn should_crash(&self, commit: &ExecutionCommit) -> bool {
        let kind = match self.boundary {
            CrashAfterCommit::Never => return false,
            CrashAfterCommit::ModelCompleted => RuntimeEventKind::ModelCompleted,
            CrashAfterCommit::CapabilityCompleted => RuntimeEventKind::CapabilityCompleted,
        };
        commit.events().iter().any(|event| event.kind() == kind)
            && self
                .armed
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }
}

#[async_trait]
impl ExecutionStore for CrashAfterCommitStore {
    async fn apply_authoritative_policy(
        &self,
        owner_id: Uuid,
        change: AuthoritativePolicyChange,
    ) -> Result<AuthoritativePolicyState, ExecutionStoreError> {
        self.inner
            .apply_authoritative_policy(owner_id, change)
            .await
    }

    async fn load_authoritative_policy(
        &self,
        owner_id: Uuid,
        definition_id: &str,
        definition_version: u32,
    ) -> Result<Option<AuthoritativePolicyState>, ExecutionStoreError> {
        self.inner
            .load_authoritative_policy(owner_id, definition_id, definition_version)
            .await
    }

    async fn apply_authoritative_grant(
        &self,
        owner_id: Uuid,
        change: anima_core::AuthoritativeGrantChange,
    ) -> Result<anima_core::AuthoritativeGrantState, ExecutionStoreError> {
        self.inner.apply_authoritative_grant(owner_id, change).await
    }

    async fn load_authoritative_grant(
        &self,
        owner_id: Uuid,
        authority_key: &GrantAuthorityKey,
    ) -> Result<Option<anima_core::AuthoritativeGrantState>, ExecutionStoreError> {
        self.inner
            .load_authoritative_grant(owner_id, authority_key)
            .await
    }

    async fn create_run(
        &self,
        owner_id: Uuid,
        request: CreateRun,
    ) -> Result<StoredRun, ExecutionStoreError> {
        self.inner.create_run(owner_id, request).await
    }

    async fn acquire_lease(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.inner
            .acquire_lease(owner_id, run_id, expected_run_version, duration_ms)
            .await
    }

    async fn renew_lease(
        &self,
        owner_id: Uuid,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.inner.renew_lease(owner_id, lease, duration_ms).await
    }

    async fn commit_execution(
        &self,
        owner_id: Uuid,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError> {
        let crash = self.should_crash(&commit);
        let outcome = self.inner.commit_execution(owner_id, commit).await?;
        if crash {
            Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        } else {
            Ok(outcome)
        }
    }

    async fn load_run(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<StoredRun>, ExecutionStoreError> {
        self.inner.load_run(owner_id, run_id).await
    }

    async fn load_checkpoint(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<(u64, anima_core::CheckpointV1)>, ExecutionStoreError> {
        self.inner.load_checkpoint(owner_id, run_id).await
    }

    async fn load_steps_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<ExecutionStep>, ExecutionStoreError> {
        self.inner.load_steps_page(owner_id, run_id, page).await
    }

    async fn load_attempts_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<InvocationAttemptRecord>, ExecutionStoreError> {
        self.inner.load_attempts_page(owner_id, run_id, page).await
    }

    async fn load_durable_result(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError> {
        self.inner
            .load_durable_result(owner_id, run_id, logical_invocation_id)
            .await
    }

    async fn replay_events(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<EventReplayPage, ExecutionStoreError> {
        self.inner.replay_events(owner_id, run_id, page).await
    }
}

struct RecordingRuntime {
    manifest: CapabilityManifest,
    calls: Mutex<Vec<CapabilityExecutionContext>>,
}

#[async_trait]
impl EngineCapabilityRuntime for RecordingRuntime {
    fn manifest(&self, id: &str, version: u32) -> Option<CapabilityManifest> {
        (id == self.manifest.id && version == self.manifest.version).then(|| self.manifest.clone())
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        self.calls.lock().unwrap().push(context.clone());
        let durable = DurableCapabilityResult::new(
            CapabilityReferenceId::new(Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                context.invocation().id().as_bytes(),
            )),
            format!("jcs-v1:{}", "a".repeat(64)),
            self.manifest.schema_digest(),
            1,
            DurableCapabilityStatus::Completed,
        )?;
        Ok(EngineCapabilityResult {
            output: CapabilityResult::new(serde_json::json!({"ok": true})),
            durable,
        })
    }

    async fn recover_dispatched(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        Err(CapabilityError::unavailable())
    }

    async fn execute_retry(
        &self,
        _context: CapabilityExecutionContext,
        _authorization: CapabilityRetryAuthorization,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        Err(CapabilityError::unavailable())
    }
}

struct Continue;

impl EngineControlSignal for Continue {
    fn at_boundary(&self) -> EngineBoundaryAction {
        EngineBoundaryAction::Continue
    }
}

struct NeverCrash;

impl EngineCrashInjector for NeverCrash {
    fn should_crash(&self, _point: EngineCrashPoint) -> bool {
        false
    }
}

struct IgnoreLive;

#[async_trait]
impl EngineLiveEventSink for IgnoreLive {
    async fn emit(&self, _event: EngineLiveEvent) -> Result<(), EngineError> {
        Ok(())
    }
}

#[tokio::test]
async fn openai_and_anthropic_real_adapters_drive_the_durable_capability_loop() {
    for (offset, provider) in ["openai", "anthropic"].into_iter().enumerate() {
        run_provider_loop(provider, 0x900 + (offset as u128 * 0x10)).await;
    }
}

#[tokio::test]
async fn restart_after_durable_model_completion_reuses_the_provider_response() {
    for (offset, provider) in ["openai", "anthropic"].into_iter().enumerate() {
        run_provider_restart(
            provider,
            0xa00 + (offset as u128 * 0x10),
            RestartScenario::ModelCompleted,
        )
        .await;
    }
}

#[tokio::test]
async fn restart_after_durable_capability_result_replays_the_correlated_transcript() {
    for (offset, provider) in ["openai", "anthropic"].into_iter().enumerate() {
        run_provider_restart(
            provider,
            0xb00 + (offset as u128 * 0x10),
            RestartScenario::CapabilityCompleted,
        )
        .await;
    }
}

#[tokio::test]
async fn approval_resume_replays_the_assistant_tool_call_before_its_result() {
    for (offset, provider) in ["openai", "anthropic"].into_iter().enumerate() {
        run_provider_restart(
            provider,
            0xc00 + (offset as u128 * 0x10),
            RestartScenario::ApprovalResume,
        )
        .await;
    }
}

#[derive(Clone, Copy)]
enum RestartScenario {
    ModelCompleted,
    CapabilityCompleted,
    ApprovalResume,
}

async fn run_provider_restart(provider: &str, seed: u128, scenario: RestartScenario) {
    let requests = Arc::new(AtomicUsize::new(0));
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let app = match provider {
        "openai" => Router::new().route(
            "/v1/chat/completions",
            post({
                let requests = requests.clone();
                let request_bodies = request_bodies.clone();
                move |Json(request): Json<Value>| {
                    let requests = requests.clone();
                    let request_bodies = request_bodies.clone();
                    async move {
                        request_bodies.lock().unwrap().push(request);
                        let body = if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            concat!(
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_restart\",\"type\":\"function\",\"function\":{\"name\":\"workspace.write\",\"arguments\":\"{\\\"path\\\":\\\"restart.txt\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                                "data: [DONE]\n\n"
                            )
                        } else {
                            concat!(
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"restart-complete\"},\"finish_reason\":\"stop\"}]}\n\n",
                                "data: [DONE]\n\n"
                            )
                        };
                        ([("content-type", "text/event-stream")], body)
                    }
                }
            }),
        ),
        "anthropic" => Router::new().route(
            "/v1/messages",
            post({
                let requests = requests.clone();
                let request_bodies = request_bodies.clone();
                move |Json(request): Json<Value>| {
                    let requests = requests.clone();
                    let request_bodies = request_bodies.clone();
                    async move {
                        request_bodies.lock().unwrap().push(request);
                        let body = if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            concat!(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_restart\",\"name\":\"workspace.write\",\"input\":{}}}\n\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"restart.txt\\\"}\"}}\n\n",
                                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":1}}\n\n",
                                "data: {\"type\":\"message_stop\"}\n\n"
                            )
                        } else {
                            concat!(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"restart-complete\"}}\n\n",
                                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                                "data: {\"type\":\"message_stop\"}\n\n"
                            )
                        };
                        ([("content-type", "text/event-stream")], body)
                    }
                }
            }),
        ),
        _ => unreachable!(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let manifest =
        capability_manifest_with_risk(if matches!(scenario, RestartScenario::ApprovalResume) {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        });
    let definition = definition(provider, &manifest);
    let owner_id = Uuid::from_u128(seed + 1);
    let run_id = Uuid::from_u128(seed + 3);
    let session = Session::new_for_definition(
        Uuid::from_u128(seed + 2),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let clock = Arc::new(anima_core::ManualExecutionClock::default());
    let store = Arc::new(CrashAfterCommitStore::new(
        match scenario {
            RestartScenario::ModelCompleted => CrashAfterCommit::ModelCompleted,
            RestartScenario::CapabilityCompleted => CrashAfterCommit::CapabilityCompleted,
            RestartScenario::ApprovalResume => CrashAfterCommit::Never,
        },
        clock.clone(),
    ));
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session,
                queued,
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::create(
                AuthoritativePolicyState::active(owner_id, &definition.id, 1, 1, None).unwrap(),
            ),
        )
        .await
        .unwrap();
    let runtime = Arc::new(RecordingRuntime {
        manifest,
        calls: Mutex::new(vec![]),
    });
    let provider_base = if provider == "openai" {
        format!("{base_url}/v1")
    } else {
        base_url
    };
    let adapter = ProviderModelAdapter::new(ProviderAdapterConfig {
        providers: BTreeMap::from([(
            provider.to_owned(),
            ProviderCredential {
                api_key: Some("test-key".into()),
                base_url: provider_base,
            },
        )]),
    });
    let policy = Arc::new(ApprovalCurrentPolicy {
        approved: AtomicBool::new(false),
    });
    let engine = DurableAgentEngine::new(
        store,
        Arc::new(adapter),
        Arc::new(StaticDefinition(definition)),
        policy.clone(),
        runtime.clone(),
        clock.clone(),
        Arc::new(IgnoreLive),
        Arc::new(NeverCrash),
        DurableEngineConfig {
            lease_duration_ms: 5,
        },
    )
    .unwrap();

    match scenario {
        RestartScenario::ModelCompleted | RestartScenario::CapabilityCompleted => {
            let error = engine.run(owner_id, run_id, &Continue).await.unwrap_err();
            assert_eq!(error.code(), EngineErrorCode::Store);
        }
        RestartScenario::ApprovalResume => {
            assert_eq!(
                engine.run(owner_id, run_id, &Continue).await.unwrap(),
                EngineRunOutcome::WaitingForApproval
            );
            policy.approved.store(true, Ordering::SeqCst);
        }
    }
    clock.advance_ms(10).unwrap();
    let outcome = engine.run(owner_id, run_id, &Continue).await.unwrap();
    assert!(
        matches!(
            outcome.clone(),
            EngineRunOutcome::Completed { ref content } if content.text == "restart-complete"
        ),
        "unexpected restart outcome: {outcome:?}"
    );
    assert_eq!(runtime.calls.lock().unwrap().len(), 1);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_provider_continuation(provider, &request_bodies.lock().unwrap()[1]);
}

fn assert_provider_continuation(provider: &str, request: &Value) {
    match provider {
        "openai" => {
            assert_eq!(request["tools"][0]["function"]["name"], "workspace.write");
            let messages = request["messages"].as_array().unwrap();
            let assistant = messages
                .iter()
                .position(|message| {
                    message["role"] == "assistant"
                        && message["tool_calls"][0]["function"]["name"] == "workspace.write"
                })
                .expect("assistant tool call");
            let tool = messages
                .iter()
                .position(|message| message["role"] == "tool")
                .expect("tool result");
            assert!(assistant < tool);
            assert_eq!(
                messages[assistant]["tool_calls"][0]["id"],
                messages[tool]["tool_call_id"]
            );
        }
        "anthropic" => {
            assert_eq!(request["tools"][0]["name"], "workspace.write");
            let messages = request["messages"].as_array().unwrap();
            let assistant = messages
                .iter()
                .position(|message| {
                    message["role"] == "assistant"
                        && message["content"][0]["type"] == "tool_use"
                        && message["content"][0]["name"] == "workspace.write"
                })
                .expect("assistant tool use");
            let tool = messages
                .iter()
                .position(|message| {
                    message["role"] == "user" && message["content"][0]["type"] == "tool_result"
                })
                .expect("tool result");
            assert!(assistant < tool);
            assert_eq!(
                messages[assistant]["content"][0]["id"],
                messages[tool]["content"][0]["tool_use_id"]
            );
        }
        _ => unreachable!(),
    }
}

async fn run_provider_loop(provider: &str, seed: u128) {
    let requests = Arc::new(AtomicUsize::new(0));
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let app = match provider {
        "openai" => Router::new().route(
            "/v1/chat/completions",
            post({
                let requests = requests.clone();
                let request_bodies = request_bodies.clone();
                move |Json(request): Json<Value>| {
                    let requests = requests.clone();
                    let request_bodies = request_bodies.clone();
                    async move {
                        request_bodies.lock().unwrap().push(request);
                        let body = if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            concat!(
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_\",\"type\":\"function\",\"function\":{\"name\":\"workspace.\",\"arguments\":\"{\\\"path\\\":\\\"provider\"}}]}}]}\n\n",
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"engine\",\"function\":{\"name\":\"write\",\"arguments\":\".txt\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                                "data: [DONE]\n\n"
                            )
                        } else {
                            concat!(
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"provider-complete\"},\"finish_reason\":\"stop\"}]}\n\n",
                                "data: [DONE]\n\n"
                            )
                        };
                        ([("content-type", "text/event-stream")], body)
                    }
                }
            }),
        ),
        "anthropic" => Router::new().route(
            "/v1/messages",
            post({
                let requests = requests.clone();
                let request_bodies = request_bodies.clone();
                move |Json(request): Json<Value>| {
                    let requests = requests.clone();
                    let request_bodies = request_bodies.clone();
                    async move {
                        request_bodies.lock().unwrap().push(request);
                        let body = if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            concat!(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_engine\",\"name\":\"workspace.write\",\"input\":{}}}\n\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"provider.txt\\\"}\"}}\n\n",
                                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":1}}\n\n",
                                "data: {\"type\":\"message_stop\"}\n\n"
                            )
                        } else {
                            concat!(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"provider-complete\"}}\n\n",
                                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                                "data: {\"type\":\"message_stop\"}\n\n"
                            )
                        };
                        ([("content-type", "text/event-stream")], body)
                    }
                }
            }),
        ),
        _ => unreachable!(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let manifest = capability_manifest();
    let definition = definition(provider, &manifest);
    let owner_id = Uuid::from_u128(seed + 1);
    let run_id = Uuid::from_u128(seed + 3);
    let session = Session::new_for_definition(
        Uuid::from_u128(seed + 2),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let store = Arc::new(InMemoryExecutionStore::default());
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session,
                queued,
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::create(
                AuthoritativePolicyState::active(owner_id, &definition.id, 1, 1, None).unwrap(),
            ),
        )
        .await
        .unwrap();
    let runtime = Arc::new(RecordingRuntime {
        manifest,
        calls: Mutex::new(vec![]),
    });
    let provider_base = if provider == "openai" {
        format!("{base_url}/v1")
    } else {
        base_url
    };
    let adapter = ProviderModelAdapter::new(ProviderAdapterConfig {
        providers: BTreeMap::from([(
            provider.to_owned(),
            ProviderCredential {
                api_key: Some("test-key".into()),
                base_url: provider_base,
            },
        )]),
    });
    let engine = DurableAgentEngine::new(
        store,
        Arc::new(adapter),
        Arc::new(StaticDefinition(definition)),
        Arc::new(AllowCurrentPolicy),
        runtime.clone(),
        Arc::new(anima_core::ManualExecutionClock::default()),
        Arc::new(IgnoreLive),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = engine.run(owner_id, run_id, &Continue).await.unwrap();
    assert!(matches!(
        outcome,
        EngineRunOutcome::Completed { ref content } if content.text == "provider-complete"
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let request_bodies = request_bodies.lock().unwrap();
    assert_eq!(request_bodies.len(), 2);
    match provider {
        "openai" => {
            assert_eq!(
                request_bodies[0]["tools"][0]["function"]["name"],
                "workspace.write"
            );
            assert!(request_bodies[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["role"] == "assistant"
                        && message["tool_calls"][0]["id"] == "call_engine"
                        && message["tool_calls"][0]["function"]["name"] == "workspace.write"
                }));
            assert!(request_bodies[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["role"] == "tool" && message["tool_call_id"] == "call_engine"
                }));
        }
        "anthropic" => {
            assert_eq!(request_bodies[0]["tools"][0]["name"], "workspace.write");
            assert!(request_bodies[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["role"] == "assistant"
                        && message["content"][0]["type"] == "tool_use"
                        && message["content"][0]["id"] == "tool_engine"
                }));
            assert!(request_bodies[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["role"] == "user"
                        && message["content"][0]["type"] == "tool_result"
                        && message["content"][0]["tool_use_id"] == "tool_engine"
                }));
        }
        _ => unreachable!(),
    }
    let calls = runtime.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].invocation().capability_id(), "workspace.write");
    assert_eq!(
        calls[0].normalized_arguments(),
        &serde_json::json!({"path": "provider.txt"})
    );
}

fn capability_manifest() -> CapabilityManifest {
    capability_manifest_with_risk(RiskLevel::Low)
}

fn capability_manifest_with_risk(risk_level: RiskLevel) -> CapabilityManifest {
    CapabilityManifest::new(CapabilityManifestInput {
        id: "workspace.write".into(),
        version: 1,
        kind: CapabilityKind::Workspace,
        label: "Write".into(),
        description: "writes one fixture".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}},
            "additionalProperties": false
        }),
        output_schema: serde_json::json!({
            "type": "object",
            "required": ["ok"],
            "properties": {"ok": {"type": "boolean"}},
            "additionalProperties": false
        }),
        side_effects: true,
        risk_level,
        host_permissions: vec![],
        secret_references: vec![],
        environment_requirements: vec![],
        timeout_ms: 1_000,
        cancellation_supported: true,
        max_retries: 1,
        idempotent: true,
        recovery_mode: RecoveryMode::KeyedIdempotent,
        supports_streaming: false,
        supports_artifacts: false,
        supports_citations: false,
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    })
    .unwrap()
}

fn definition(provider: &str, manifest: &CapabilityManifest) -> AgentDefinition {
    AgentDefinition {
        schema_version: 1,
        id: "provider-loop".into(),
        version: 1,
        name: "provider-loop".into(),
        display_name: "Provider Loop".into(),
        description: "test".into(),
        persona: "test".into(),
        system: "call the tool".into(),
        model: ModelPolicy {
            provider: provider.into(),
            model: "fixture".into(),
            credential_reference: None,
            temperature: None,
        },
        source_profile: ProfileRef {
            profile_id: "empty".into(),
            profile_version: 1,
        },
        resolved_capabilities: vec![ResolvedCapability {
            capability_id: manifest.id.clone(),
            manifest_version: manifest.version,
            schema_digest: manifest.schema_digest().into(),
            override_config: None,
            approval_policy_revision: 1,
        }],
        memory: MemoryPolicy {
            enabled: false,
            namespace: "test".into(),
            retention_days: None,
        },
        approval_policy_id: "default".into(),
        approval_policy_revision: 1,
        approval_restrictions: vec![],
        limits: RuntimeLimits {
            max_turns: 4,
            timeout_ms: 30_000,
            max_concurrent_tasks: 1,
        },
        lifecycle: LifecyclePolicy {
            auto_start: false,
            restart_on_failure: false,
            max_restarts: 0,
            allows_concurrent_sessions: false,
        },
        host_requirements: vec![],
    }
}
