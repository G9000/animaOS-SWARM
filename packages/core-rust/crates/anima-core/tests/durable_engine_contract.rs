use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentDefinition, ApprovalDecision, AuthoritativeGrantChange,
    AuthoritativeGrantState, AuthoritativePolicyChange, AuthoritativePolicyState, AutonomyGrant,
    CapabilityError, CapabilityExecutionContext, CapabilityExecutor, CapabilityKind,
    CapabilityManifest, CapabilityManifestInput, CapabilityReferenceId, CapabilityRegistry,
    CapabilityResult, CapabilityRetryAuthorization, Content, CreateRun, CurrentPolicyResolution,
    CurrentPolicyResolver, DataValue, DefinitionPin, DefinitionResolver, DurableAgentEngine,
    DurableEngineConfig, EngineBoundaryAction, EngineCapabilityResult, EngineCapabilityRuntime,
    EngineControlSignal, EngineCrashInjector, EngineCrashPoint, EngineError, EngineErrorCode,
    EngineLiveEvent, EngineLiveEventSink, EnginePolicyRequest, EngineRunOutcome, ExecutionClock,
    ExecutionStore, GrantScope, GrantStatus, InMemoryExecutionStore, LifecyclePolicy,
    LogicalInvocation, ManifestCatalog, ManualExecutionClock, MemoryPolicy, ModelAdapter,
    ModelGenerateRequest, ModelGenerateResponse, ModelPolicy, ModelStopReason, ModelStreamFrame,
    ModelStreamSink, PolicyContext, PolicyRestrictions, ProfileRef, ReconcileOutcome,
    RecoveryAction, RecoveryMode, ResolvedCapability, RiskLevel, Run, RuntimeCompatibility,
    RuntimeEventKind, RuntimeLimits, Session, SessionConcurrencyPolicy, StoreReadPage, TokenUsage,
    ToolCall,
};
use async_trait::async_trait;
use uuid::Uuid;

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn durable_engine_exposes_typed_host_boundaries() {
    assert_send_sync::<DurableAgentEngine>();
    assert_eq!(
        EngineBoundaryAction::Continue,
        EngineBoundaryAction::Continue
    );
    assert_eq!(
        EngineCrashPoint::AfterDispatchCommitBeforeExecutor,
        EngineCrashPoint::AfterDispatchCommitBeforeExecutor,
    );
    assert_eq!(EngineRunOutcome::Denied, EngineRunOutcome::Denied);
}

#[derive(Clone)]
struct StaticDefinitions(AgentDefinition);

#[async_trait]
impl DefinitionResolver for StaticDefinitions {
    async fn resolve(&self, pin: &DefinitionPin) -> Result<AgentDefinition, EngineError> {
        if pin.id() == self.0.id && pin.version() == self.0.version {
            Ok(self.0.clone())
        } else {
            Err(EngineError::new(EngineErrorCode::DefinitionUnavailable))
        }
    }
}

struct NeverPolicy;

#[async_trait]
impl CurrentPolicyResolver for NeverPolicy {
    async fn resolve(
        &self,
        _request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        panic!("a final-only model must not evaluate capability policy")
    }
}

struct NoCapabilities;

#[async_trait]
impl EngineCapabilityRuntime for NoCapabilities {
    fn manifest(&self, _id: &str, _version: u32) -> Option<CapabilityManifest> {
        None
    }

    async fn execute(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        Err(CapabilityError::unavailable())
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

struct StreamingModel {
    turns: Mutex<VecDeque<Vec<ModelStreamFrame>>>,
}

#[async_trait]
impl ModelAdapter for StreamingModel {
    fn provider(&self) -> &str {
        "deterministic"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Err("stream must be used".into())
    }

    async fn stream(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        let frames = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "no model turn".to_owned())?;
        for frame in frames {
            sink.emit(frame).await?;
        }
        Ok(())
    }
}

async fn cooperative_yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|context| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            context.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}

async fn advance_across_heartbeats(clock: &ManualExecutionClock, advances: &[u64]) {
    for advance in advances {
        clock.advance_ms(*advance).unwrap();
        cooperative_yield_once().await;
    }
}

struct AdvancingModel {
    clock: Arc<ManualExecutionClock>,
    advances: Vec<u64>,
    response: ModelGenerateResponse,
    emitted: Arc<AtomicBool>,
}

#[async_trait]
impl ModelAdapter for AdvancingModel {
    fn provider(&self) -> &str {
        "advancing"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Err("stream must be used".into())
    }

    async fn stream(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        advance_across_heartbeats(&self.clock, &self.advances).await;
        self.emitted.store(true, Ordering::SeqCst);
        sink.emit(ModelStreamFrame::Final(self.response.clone()))
            .await
    }
}

#[derive(Default)]
struct RecordingLiveSink(Mutex<Vec<EngineLiveEvent>>);

#[async_trait]
impl EngineLiveEventSink for RecordingLiveSink {
    async fn emit(&self, event: EngineLiveEvent) -> Result<(), EngineError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

struct NeverCrash;

impl EngineCrashInjector for NeverCrash {
    fn should_crash(&self, _point: EngineCrashPoint) -> bool {
        false
    }
}

struct CrashOnceAfterExecutor(AtomicBool);

impl EngineCrashInjector for CrashOnceAfterExecutor {
    fn should_crash(&self, point: EngineCrashPoint) -> bool {
        point == EngineCrashPoint::AfterExecutorBeforeResultCommit
            && self
                .0
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }
}

struct CrashOnceAfterDispatch(AtomicBool);

impl EngineCrashInjector for CrashOnceAfterDispatch {
    fn should_crash(&self, point: EngineCrashPoint) -> bool {
        point == EngineCrashPoint::AfterDispatchCommitBeforeExecutor
            && self
                .0
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }
}

struct ContinueSignal;

impl EngineControlSignal for ContinueSignal {
    fn at_boundary(&self) -> EngineBoundaryAction {
        EngineBoundaryAction::Continue
    }
}

struct FixedSignal(EngineBoundaryAction);

impl EngineControlSignal for FixedSignal {
    fn at_boundary(&self) -> EngineBoundaryAction {
        self.0
    }
}

fn definition() -> AgentDefinition {
    AgentDefinition {
        schema_version: 1,
        id: "durable-engine".into(),
        version: 1,
        name: "durable-engine".into(),
        display_name: "Durable Engine".into(),
        description: "test".into(),
        persona: "test".into(),
        system: "be deterministic".into(),
        model: ModelPolicy {
            provider: "deterministic".into(),
            model: "fixture".into(),
            credential_reference: None,
            temperature: None,
        },
        source_profile: ProfileRef {
            profile_id: "empty".into(),
            profile_version: 1,
        },
        resolved_capabilities: vec![],
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

async fn authorize_policy(
    store: &InMemoryExecutionStore,
    owner_id: Uuid,
    definition: &AgentDefinition,
) {
    store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::create(
                AuthoritativePolicyState::active(
                    owner_id,
                    definition.id.clone(),
                    definition.version,
                    definition.approval_policy_revision,
                    None,
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap();
}

async fn create_engine_run(
    seed: u128,
    definition: &AgentDefinition,
    clock: Arc<ManualExecutionClock>,
) -> (Uuid, Uuid, Arc<InMemoryExecutionStore>) {
    let owner_id = Uuid::from_u128(seed + 1);
    let run_id = Uuid::from_u128(seed + 3);
    let session = Session::new_for_definition(
        Uuid::from_u128(seed + 2),
        definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let store = Arc::new(InMemoryExecutionStore::with_clock(clock));
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
    authorize_policy(store.as_ref(), owner_id, definition).await;
    (owner_id, run_id, store)
}

fn manifest(recovery_mode: RecoveryMode) -> CapabilityManifest {
    manifest_with_risk(recovery_mode, RiskLevel::Low)
}

fn manifest_with_risk(recovery_mode: RecoveryMode, risk_level: RiskLevel) -> CapabilityManifest {
    CapabilityManifest::new(CapabilityManifestInput {
        id: "workspace.write".into(),
        version: 1,
        kind: CapabilityKind::Workspace,
        label: "Write".into(),
        description: "write a bounded fixture".into(),
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
        idempotent: matches!(
            recovery_mode,
            RecoveryMode::InherentlyIdempotent | RecoveryMode::KeyedIdempotent
        ),
        recovery_mode,
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

struct AllowPolicy;

#[async_trait]
impl CurrentPolicyResolver for AllowPolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "fixture-actor",
            request.definition().id.clone(),
            request.definition().version,
            "fixture-workspace",
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

struct ApprovePendingPolicy;

#[async_trait]
impl CurrentPolicyResolver for ApprovePendingPolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "fixture-actor",
            request.definition().id.clone(),
            request.definition().version,
            "fixture-workspace",
            CapabilityReferenceId::new(request.invocation().run_id()),
            request.manifest(),
            request.invocation(),
            request.definition().approval_policy_revision,
            PolicyRestrictions::default(),
            1_000_000,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        let approval = request
            .pending_approval()
            .cloned()
            .map(|pending| ApprovalDecision::new_approved(pending, 1_000_000))
            .transpose()
            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        Ok(CurrentPolicyResolution::new(context, vec![], approval))
    }
}

struct DenyPendingPolicy {
    stale: bool,
}

#[derive(Clone, Copy)]
enum GuardFailureMode {
    Revoke,
    AdvanceRevision,
    ArgumentDrift,
}

struct GuardFailurePolicy {
    mode: GuardFailureMode,
    store: Arc<InMemoryExecutionStore>,
    calls: AtomicUsize,
    trigger_call: usize,
}

#[async_trait]
impl CurrentPolicyResolver for GuardFailurePolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == self.trigger_call {
            match self.mode {
                GuardFailureMode::Revoke => {
                    self.store
                        .apply_authoritative_policy(
                            request.owner_id(),
                            AuthoritativePolicyChange::revoke(
                                request.definition().id.clone(),
                                request.definition().version,
                                request.definition().approval_policy_revision,
                            )
                            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?,
                        )
                        .await
                        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
                }
                GuardFailureMode::AdvanceRevision => {
                    self.store
                        .apply_authoritative_policy(
                            request.owner_id(),
                            AuthoritativePolicyChange::update(
                                request.definition().approval_policy_revision,
                                AuthoritativePolicyState::active(
                                    request.owner_id(),
                                    request.definition().id.clone(),
                                    request.definition().version,
                                    request.definition().approval_policy_revision + 1,
                                    None,
                                )
                                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?,
                            )
                            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?,
                        )
                        .await
                        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
                }
                GuardFailureMode::ArgumentDrift => {}
            }
        }
        let invocation = if matches!(self.mode, GuardFailureMode::ArgumentDrift) {
            LogicalInvocation::new(
                request.invocation().run_id(),
                request.invocation().logical_step_id(),
                request.invocation().capability_id(),
                request.invocation().manifest_version(),
                serde_json::json!({"path": "changed-by-policy.txt"}),
            )
            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?
        } else {
            request.invocation().clone()
        };
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "fixture-actor",
            request.definition().id.clone(),
            request.definition().version,
            "fixture-workspace",
            CapabilityReferenceId::new(invocation.run_id()),
            request.manifest(),
            &invocation,
            request.definition().approval_policy_revision,
            PolicyRestrictions::default(),
            1_000_000,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        Ok(CurrentPolicyResolution::new(context, vec![], None))
    }
}

#[async_trait]
impl CurrentPolicyResolver for DenyPendingPolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let pending = request.pending_approval().cloned();
        let now_ms = if self.stale {
            pending
                .as_ref()
                .map_or(1_000_000, |request| request.expires_at_ms)
        } else {
            1_000_000
        };
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "fixture-actor",
            request.definition().id.clone(),
            request.definition().version,
            "fixture-workspace",
            CapabilityReferenceId::new(request.invocation().run_id()),
            request.manifest(),
            request.invocation(),
            request.definition().approval_policy_revision,
            PolicyRestrictions::default(),
            now_ms,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        let approval = pending
            .map(|pending| {
                let decided_at_ms = pending.requested_at_ms;
                if self.stale {
                    ApprovalDecision::new_approved(pending, decided_at_ms)
                } else {
                    ApprovalDecision::new_denied(pending, decided_at_ms)
                }
            })
            .transpose()
            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        Ok(CurrentPolicyResolution::new(context, vec![], approval))
    }
}

fn counted_auto_allow_grant(context: &PolicyContext) -> AutonomyGrant {
    AutonomyGrant::new(
        "engine-counted-grant",
        1,
        GrantStatus::Active,
        GrantScope::new(
            context.owner_id.clone(),
            context.actor_id.clone(),
            context.agent_definition_id.clone(),
            context.agent_definition_version,
            context.workspace_id.clone(),
            context.resource_boundary.clone(),
            context.capability_id.clone(),
            context.manifest_version,
            Some(context.canonical_argument_digest),
        )
        .unwrap(),
        RiskLevel::Critical,
        500_000,
        Some(2_000_000),
        Some(1),
    )
    .unwrap()
}

struct CountedGrantPolicy;

#[async_trait]
impl CurrentPolicyResolver for CountedGrantPolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "fixture-actor",
            request.definition().id.clone(),
            request.definition().version,
            "fixture-workspace",
            CapabilityReferenceId::new(request.invocation().run_id()),
            request.manifest(),
            request.invocation(),
            request.definition().approval_policy_revision,
            PolicyRestrictions::default(),
            1_000_000,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        let grant = counted_auto_allow_grant(&context);
        Ok(CurrentPolicyResolution::new(context, vec![grant], None))
    }
}

struct RecordingCapabilities {
    manifest: CapabilityManifest,
    calls: Mutex<Vec<Uuid>>,
    completed: Mutex<Option<anima_core::DurableCapabilityResult>>,
}

struct AbsentThenSuccessfulExecutor {
    manifest: CapabilityManifest,
    calls: Arc<Mutex<u32>>,
}

struct AdvancingCapabilities {
    manifest: CapabilityManifest,
    clock: Arc<ManualExecutionClock>,
    advances: Vec<u64>,
    execute_calls: AtomicUsize,
    recovery_calls: AtomicUsize,
    recovery_action: RecoveryAction,
}

impl AdvancingCapabilities {
    fn completed_result(
        &self,
        context: &CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        let durable = anima_core::DurableCapabilityResult::new(
            CapabilityReferenceId::new(Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                context.invocation().id().as_bytes(),
            )),
            format!("jcs-v1:{}", "b".repeat(64)),
            self.manifest.schema_digest(),
            1,
            anima_core::DurableCapabilityStatus::Completed,
        )?;
        Ok(EngineCapabilityResult {
            output: CapabilityResult::new(serde_json::json!({"ok": true})),
            durable,
        })
    }
}

#[async_trait]
impl EngineCapabilityRuntime for AdvancingCapabilities {
    fn manifest(&self, id: &str, version: u32) -> Option<CapabilityManifest> {
        (id == self.manifest.id && version == self.manifest.version).then(|| self.manifest.clone())
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        self.execute_calls.fetch_add(1, Ordering::SeqCst);
        advance_across_heartbeats(&self.clock, &self.advances).await;
        self.completed_result(&context)
    }

    async fn recover_dispatched(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        self.recovery_calls.fetch_add(1, Ordering::SeqCst);
        advance_across_heartbeats(&self.clock, &self.advances).await;
        Ok(self.recovery_action.clone())
    }

    async fn execute_retry(
        &self,
        context: CapabilityExecutionContext,
        _authorization: CapabilityRetryAuthorization,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        self.execute(context).await
    }
}

#[async_trait]
impl CapabilityExecutor for AbsentThenSuccessfulExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        *self.calls.lock().unwrap() += 1;
        Ok(CapabilityResult::new(serde_json::json!({"ok": true})))
    }

    async fn reconcile(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        Ok(ReconcileOutcome::AuthoritativeAbsence)
    }
}

#[async_trait]
impl EngineCapabilityRuntime for RecordingCapabilities {
    fn manifest(&self, id: &str, version: u32) -> Option<CapabilityManifest> {
        (id == self.manifest.id && version == self.manifest.version).then(|| self.manifest.clone())
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        self.calls.lock().unwrap().push(context.invocation().id());
        let durable = anima_core::DurableCapabilityResult::new(
            CapabilityReferenceId::new(Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                context.invocation().id().as_bytes(),
            )),
            format!("jcs-v1:{}", "a".repeat(64)),
            self.manifest.schema_digest(),
            1,
            anima_core::DurableCapabilityStatus::Completed,
        )?;
        *self.completed.lock().unwrap() = Some(durable.clone());
        Ok(EngineCapabilityResult {
            output: CapabilityResult::new(serde_json::json!({"ok": true})),
            durable,
        })
    }

    async fn recover_dispatched(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        self.completed
            .lock()
            .unwrap()
            .clone()
            .map(RecoveryAction::Completed)
            .ok_or_else(CapabilityError::reconciliation)
    }

    async fn execute_retry(
        &self,
        _context: CapabilityExecutionContext,
        _authorization: CapabilityRetryAuthorization,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        Err(CapabilityError::reconciliation())
    }
}

#[tokio::test]
async fn final_model_turn_emits_live_deltas_and_durable_semantic_events() {
    let owner_id = Uuid::from_u128(0x601);
    let run_id = Uuid::from_u128(0x603);
    let definition = definition();
    let session = Session::new_for_definition(
        Uuid::from_u128(0x602),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let clock = Arc::new(ManualExecutionClock::default());
    let store = Arc::new(InMemoryExecutionStore::with_clock(clock.clone()));
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let final_response = ModelGenerateResponse {
        content: Content {
            text: "hello".into(),
            attachments: None,
            metadata: None,
        },
        tool_calls: None,
        usage: TokenUsage {
            prompt_tokens: 2,
            completion_tokens: 2,
            total_tokens: 4,
        },
        stop_reason: ModelStopReason::End,
    };
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([vec![
            ModelStreamFrame::TextDelta("hel".into()),
            ModelStreamFrame::TextDelta("lo".into()),
            ModelStreamFrame::Final(final_response),
        ]])),
    });
    let live = Arc::new(RecordingLiveSink::default());
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(NeverPolicy),
        Arc::new(NoCapabilities),
        clock,
        live.clone(),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = match engine.run(owner_id, run_id, &ContinueSignal).await {
        Ok(outcome) => outcome,
        Err(error) => panic!(
            "engine failed: {error:?}; run={:?}; checkpoint={:?}; events={:?}",
            store.load_run(owner_id, run_id).await.unwrap(),
            store.load_checkpoint(owner_id, run_id).await.unwrap(),
            store
                .replay_events(owner_id, run_id, StoreReadPage::first(256).unwrap())
                .await
                .unwrap()
                .events(),
        ),
    };

    assert_eq!(
        outcome,
        EngineRunOutcome::Completed {
            content: Content {
                text: "hello".into(),
                attachments: None,
                metadata: None,
            },
        }
    );
    let live_events = live.0.lock().unwrap().clone();
    assert!(live_events.iter().any(|event| matches!(
        event,
        EngineLiveEvent::TextDelta { text, .. } if text == "hel"
    )));
    assert!(live_events.iter().any(|event| matches!(
        event,
        EngineLiveEvent::TextDelta { text, .. } if text == "lo"
    )));
    let durable = store
        .replay_events(owner_id, run_id, StoreReadPage::first(256).unwrap())
        .await
        .unwrap();
    let kinds = durable
        .events()
        .iter()
        .map(|event| event.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            RuntimeEventKind::RunStarted,
            RuntimeEventKind::ModelStarted,
            RuntimeEventKind::ModelCompleted,
            RuntimeEventKind::RunCompleted,
        ]
    );
}

#[tokio::test]
async fn unavailable_exact_manifest_pin_fails_before_model_or_executor_entry() {
    let owner_id = Uuid::from_u128(0x607);
    let run_id = Uuid::from_u128(0x609);
    let missing = manifest(RecoveryMode::KeyedIdempotent);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: missing.id.clone(),
        manifest_version: missing.version,
        schema_digest: missing.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(0x608),
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
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([vec![ModelStreamFrame::Final(
            ModelGenerateResponse {
                content: Content::default(),
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            },
        )]])),
    });
    let engine = DurableAgentEngine::new(
        store,
        model.clone(),
        Arc::new(StaticDefinitions(definition)),
        Arc::new(NeverPolicy),
        Arc::new(NoCapabilities),
        Arc::new(ManualExecutionClock::default()),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let error = engine
        .run(owner_id, run_id, &ContinueSignal)
        .await
        .unwrap_err();

    assert_eq!(error.code(), EngineErrorCode::ManifestUnavailable);
    assert_eq!(model.turns.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn model_capability_result_loop_commits_dispatch_before_executor_entry() {
    let owner_id = Uuid::from_u128(0x611);
    let run_id = Uuid::from_u128(0x613);
    let capability_manifest = manifest(RecoveryMode::KeyedIdempotent);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(0x612),
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let tool_call = ToolCall {
        id: "write-step".into(),
        name: capability_manifest.id.clone(),
        args: BTreeMap::from([("path".into(), DataValue::String("note.txt".into()))]),
    };
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![tool_call]),
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                stop_reason: ModelStopReason::ToolCall,
            })],
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content {
                    text: "wrote it".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 2,
                    completion_tokens: 1,
                    total_tokens: 3,
                },
                stop_reason: ModelStopReason::End,
            })],
        ])),
    });
    let capabilities = Arc::new(RecordingCapabilities {
        manifest: capability_manifest,
        calls: Mutex::new(vec![]),
        completed: Mutex::new(None),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(AllowPolicy),
        capabilities.clone(),
        Arc::new(ManualExecutionClock::default()),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = match engine.run(owner_id, run_id, &ContinueSignal).await {
        Ok(outcome) => outcome,
        Err(error) => panic!(
            "engine failed: {error:?}; run={:?}; checkpoint={:?}; events={:?}",
            store.load_run(owner_id, run_id).await.unwrap(),
            store.load_checkpoint(owner_id, run_id).await.unwrap(),
            store
                .replay_events(owner_id, run_id, StoreReadPage::first(256).unwrap())
                .await
                .unwrap()
                .events(),
        ),
    };

    assert!(matches!(
        outcome,
        EngineRunOutcome::Completed { ref content } if content.text == "wrote it"
    ));
    assert_eq!(capabilities.calls.lock().unwrap().len(), 1);
    let durable = store
        .replay_events(owner_id, run_id, StoreReadPage::first(256).unwrap())
        .await
        .unwrap();
    let kinds = durable
        .events()
        .iter()
        .map(|event| event.kind())
        .collect::<Vec<_>>();
    let dispatch = kinds
        .iter()
        .position(|kind| *kind == RuntimeEventKind::InvocationDispatchPrepared)
        .unwrap();
    let completed = kinds
        .iter()
        .position(|kind| *kind == RuntimeEventKind::CapabilityCompleted)
        .unwrap();
    assert!(dispatch < completed);
}

#[tokio::test]
async fn counted_auto_allow_grant_is_consumed_atomically_with_dispatch_preparation() {
    let owner_id = Uuid::from_u128(0x615);
    let run_id = Uuid::from_u128(0x617);
    let capability_manifest =
        manifest_with_risk(RecoveryMode::KeyedIdempotent, RiskLevel::Critical);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(0x616),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let clock = Arc::new(ManualExecutionClock::default());
    let store = Arc::new(InMemoryExecutionStore::with_clock(clock.clone()));
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let invocation = LogicalInvocation::new(
        run_id,
        "counted-step",
        capability_manifest.id.clone(),
        capability_manifest.version,
        serde_json::json!({"path": "counted.txt"}),
    )
    .unwrap();
    let policy_context = PolicyContext::new(
        owner_id.to_string(),
        "fixture-actor",
        definition.id.clone(),
        definition.version,
        "fixture-workspace",
        CapabilityReferenceId::new(run_id),
        &capability_manifest,
        &invocation,
        definition.approval_policy_revision,
        PolicyRestrictions::default(),
        1_000_000,
    )
    .unwrap();
    let grant = counted_auto_allow_grant(&policy_context);
    let authority = AuthoritativeGrantState::from_grant(owner_id, &grant).unwrap();
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::create(authority.clone()),
        )
        .await
        .unwrap();
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![ToolCall {
                    id: "counted-step".into(),
                    name: capability_manifest.id.clone(),
                    args: BTreeMap::from([(
                        "path".into(),
                        DataValue::String("counted.txt".into()),
                    )]),
                }]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            })],
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content {
                    text: "counted".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })],
        ])),
    });
    let capabilities = Arc::new(RecordingCapabilities {
        manifest: capability_manifest,
        calls: Mutex::new(vec![]),
        completed: Mutex::new(None),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(CountedGrantPolicy),
        capabilities,
        clock,
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();

    assert!(matches!(outcome, EngineRunOutcome::Completed { .. }));
    let consumed = store
        .load_authoritative_grant(owner_id, authority.authority_key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consumed.remaining_uses(), Some(0));
}

async fn run_final_with_boundary(
    action: EngineBoundaryAction,
) -> (
    EngineRunOutcome,
    Arc<InMemoryExecutionStore>,
    Arc<StreamingModel>,
    Uuid,
    Uuid,
) {
    let owner_id = Uuid::from_u128(0x621 + action as u128);
    let run_id = Uuid::from_u128(0x631 + action as u128);
    let definition = definition();
    let session = Session::new_for_definition(
        Uuid::from_u128(0x641 + action as u128),
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
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([vec![ModelStreamFrame::Final(
            ModelGenerateResponse {
                content: Content {
                    text: "must not run".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            },
        )]])),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model.clone(),
        Arc::new(StaticDefinitions(definition)),
        Arc::new(NeverPolicy),
        Arc::new(NoCapabilities),
        Arc::new(ManualExecutionClock::default()),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();
    let outcome = engine
        .run(owner_id, run_id, &FixedSignal(action))
        .await
        .unwrap();
    (outcome, store, model, owner_id, run_id)
}

#[tokio::test]
async fn cancellation_and_requested_pause_are_observed_before_model_entry() {
    let (cancelled, cancel_store, cancel_model, cancel_owner, cancel_run) =
        run_final_with_boundary(EngineBoundaryAction::Cancel).await;
    assert_eq!(cancelled, EngineRunOutcome::Cancelled);
    assert_eq!(cancel_model.turns.lock().unwrap().len(), 1);
    assert_eq!(
        cancel_store
            .load_run(cancel_owner, cancel_run)
            .await
            .unwrap()
            .unwrap()
            .run()
            .state(),
        anima_core::RunState::Cancelled
    );

    let (paused, pause_store, pause_model, pause_owner, pause_run) =
        run_final_with_boundary(EngineBoundaryAction::Pause).await;
    assert_eq!(paused, EngineRunOutcome::PausedByRequest);
    assert_eq!(pause_model.turns.lock().unwrap().len(), 1);
    let paused_run = pause_store
        .load_run(pause_owner, pause_run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paused_run.run().state(), anima_core::RunState::Paused);
    assert_eq!(
        paused_run.run().pause_reason(),
        Some(anima_core::RunPauseReason::Requested)
    );
}

#[tokio::test]
async fn outer_lease_heartbeat_renews_during_long_model_execution() {
    let definition = definition();
    let clock = Arc::new(ManualExecutionClock::default());
    let (owner_id, run_id, store) = create_engine_run(0x800, &definition, clock.clone()).await;
    let emitted = Arc::new(AtomicBool::new(false));
    let model = Arc::new(AdvancingModel {
        clock: clock.clone(),
        advances: vec![600, 600, 600],
        response: ModelGenerateResponse {
            content: Content {
                text: "heartbeat-model".into(),
                attachments: None,
                metadata: None,
            },
            tool_calls: None,
            usage: TokenUsage::default(),
            stop_reason: ModelStopReason::End,
        },
        emitted: emitted.clone(),
    });
    let engine = DurableAgentEngine::new(
        store,
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(NeverPolicy),
        Arc::new(NoCapabilities),
        clock,
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig {
            lease_duration_ms: 1_000,
        },
    )
    .unwrap();

    let outcome = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();
    assert!(matches!(
        outcome,
        EngineRunOutcome::Completed { ref content } if content.text == "heartbeat-model"
    ));
    assert!(emitted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn outer_lease_loss_cancels_model_before_final_frame_or_later_commit() {
    let definition = definition();
    let clock = Arc::new(ManualExecutionClock::default());
    let (owner_id, run_id, store) = create_engine_run(0x810, &definition, clock.clone()).await;
    let emitted = Arc::new(AtomicBool::new(false));
    let model = Arc::new(AdvancingModel {
        clock: clock.clone(),
        advances: vec![1_001],
        response: ModelGenerateResponse {
            content: Content {
                text: "must-not-commit".into(),
                attachments: None,
                metadata: None,
            },
            tool_calls: None,
            usage: TokenUsage::default(),
            stop_reason: ModelStopReason::End,
        },
        emitted: emitted.clone(),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(NeverPolicy),
        Arc::new(NoCapabilities),
        clock,
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig {
            lease_duration_ms: 1_000,
        },
    )
    .unwrap();

    let error = engine
        .run(owner_id, run_id, &ContinueSignal)
        .await
        .unwrap_err();
    assert_eq!(error.code(), EngineErrorCode::Store);
    assert!(!emitted.load(Ordering::SeqCst));
    let checkpoint = store
        .load_checkpoint(owner_id, run_id)
        .await
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(checkpoint.usage().turns, 0);
}

#[tokio::test]
async fn outer_lease_heartbeat_renews_during_executor_and_reconciliation() {
    for (offset, crash_before_executor) in [false, true].into_iter().enumerate() {
        let capability_manifest = manifest(RecoveryMode::KeyedIdempotent);
        let mut definition = definition();
        definition.resolved_capabilities = vec![ResolvedCapability {
            capability_id: capability_manifest.id.clone(),
            manifest_version: capability_manifest.version,
            schema_digest: capability_manifest.schema_digest().into(),
            override_config: None,
            approval_policy_revision: 1,
        }];
        let clock = Arc::new(ManualExecutionClock::default());
        let (owner_id, run_id, store) =
            create_engine_run(0x820 + (offset as u128 * 0x10), &definition, clock.clone()).await;
        let tool_call = ToolCall {
            id: "heartbeat-tool".into(),
            name: capability_manifest.id.clone(),
            args: BTreeMap::from([("path".into(), DataValue::String("heartbeat.txt".into()))]),
        };
        let turns = if crash_before_executor {
            VecDeque::from([vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![tool_call]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            })]])
        } else {
            VecDeque::from([
                vec![ModelStreamFrame::Final(ModelGenerateResponse {
                    content: Content::default(),
                    tool_calls: Some(vec![tool_call]),
                    usage: TokenUsage::default(),
                    stop_reason: ModelStopReason::ToolCall,
                })],
                vec![ModelStreamFrame::Final(ModelGenerateResponse {
                    content: Content {
                        text: "executor-heartbeat".into(),
                        attachments: None,
                        metadata: None,
                    },
                    tool_calls: None,
                    usage: TokenUsage::default(),
                    stop_reason: ModelStopReason::End,
                })],
            ])
        };
        let capabilities = Arc::new(AdvancingCapabilities {
            manifest: capability_manifest,
            clock: clock.clone(),
            advances: vec![600, 600, 600],
            execute_calls: AtomicUsize::new(0),
            recovery_calls: AtomicUsize::new(0),
            recovery_action: RecoveryAction::RecoveryRequired,
        });
        let engine = DurableAgentEngine::new(
            store,
            Arc::new(StreamingModel {
                turns: Mutex::new(turns),
            }),
            Arc::new(StaticDefinitions(definition)),
            Arc::new(AllowPolicy),
            capabilities.clone(),
            clock.clone(),
            Arc::new(RecordingLiveSink::default()),
            Arc::new(CrashOnceAfterDispatch(AtomicBool::new(
                crash_before_executor,
            ))),
            DurableEngineConfig {
                lease_duration_ms: 1_000,
            },
        )
        .unwrap();

        if crash_before_executor {
            let crashed = engine
                .run(owner_id, run_id, &ContinueSignal)
                .await
                .unwrap_err();
            assert_eq!(crashed.code(), EngineErrorCode::CrashInjected);
            clock.advance_ms(1_001).unwrap();
            assert_eq!(
                engine.run(owner_id, run_id, &ContinueSignal).await.unwrap(),
                EngineRunOutcome::RecoveryRequired
            );
            assert_eq!(capabilities.execute_calls.load(Ordering::SeqCst), 0);
            assert_eq!(capabilities.recovery_calls.load(Ordering::SeqCst), 1);
        } else {
            let outcome = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();
            assert!(matches!(
                outcome,
                EngineRunOutcome::Completed { ref content } if content.text == "executor-heartbeat"
            ));
            assert_eq!(capabilities.execute_calls.load(Ordering::SeqCst), 1);
            assert_eq!(capabilities.recovery_calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn crash_after_external_completion_recovers_without_a_second_external_call() {
    for (offset, recovery_mode) in [
        RecoveryMode::InherentlyIdempotent,
        RecoveryMode::KeyedIdempotent,
        RecoveryMode::Reconcilable,
        RecoveryMode::Retry,
        RecoveryMode::NonRetryable,
        RecoveryMode::None,
        RecoveryMode::Manual,
        RecoveryMode::Compensate,
    ]
    .into_iter()
    .enumerate()
    {
        let seed = 0x650 + (offset as u128 * 0x10);
        let owner_id = Uuid::from_u128(seed + 1);
        let run_id = Uuid::from_u128(seed + 3);
        let capability_manifest = manifest(recovery_mode);
        let mut definition = definition();
        definition.resolved_capabilities = vec![ResolvedCapability {
            capability_id: capability_manifest.id.clone(),
            manifest_version: capability_manifest.version,
            schema_digest: capability_manifest.schema_digest().into(),
            override_config: None,
            approval_policy_revision: 1,
        }];
        let session = Session::new_for_definition(
            Uuid::from_u128(seed + 2),
            &definition,
            SessionConcurrencyPolicy::Serial,
        )
        .unwrap();
        let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
        let clock = Arc::new(ManualExecutionClock::default());
        let store = Arc::new(InMemoryExecutionStore::with_clock(clock.clone()));
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
        authorize_policy(store.as_ref(), owner_id, &definition).await;
        let call = || ToolCall {
            id: "write-crash-step".into(),
            name: "workspace.write".into(),
            args: BTreeMap::from([("path".into(), DataValue::String("crash.txt".into()))]),
        };
        let model = Arc::new(StreamingModel {
            turns: Mutex::new(VecDeque::from([
                vec![ModelStreamFrame::Final(ModelGenerateResponse {
                    content: Content::default(),
                    tool_calls: Some(vec![call()]),
                    usage: TokenUsage::default(),
                    stop_reason: ModelStopReason::ToolCall,
                })],
                vec![ModelStreamFrame::Final(ModelGenerateResponse {
                    content: Content {
                        text: "recovered".into(),
                        attachments: None,
                        metadata: None,
                    },
                    tool_calls: None,
                    usage: TokenUsage::default(),
                    stop_reason: ModelStopReason::End,
                })],
            ])),
        });
        let capabilities = Arc::new(RecordingCapabilities {
            manifest: capability_manifest,
            calls: Mutex::new(vec![]),
            completed: Mutex::new(None),
        });
        let engine = DurableAgentEngine::new(
            store.clone(),
            model,
            Arc::new(StaticDefinitions(definition)),
            Arc::new(AllowPolicy),
            capabilities.clone(),
            clock.clone(),
            Arc::new(RecordingLiveSink::default()),
            Arc::new(CrashOnceAfterExecutor(AtomicBool::new(true))),
            DurableEngineConfig {
                lease_duration_ms: 1_000,
            },
        )
        .unwrap();

        let crashed = engine
            .run(owner_id, run_id, &ContinueSignal)
            .await
            .unwrap_err();
        assert_eq!(crashed.code(), EngineErrorCode::CrashInjected);
        assert_eq!(capabilities.calls.lock().unwrap().len(), 1);
        clock.advance_ms(1_001).unwrap();

        let recovered = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();
        assert!(matches!(
            recovered,
            EngineRunOutcome::Completed { ref content } if content.text == "recovered"
        ));
        assert_eq!(capabilities.calls.lock().unwrap().len(), 1);
        let attempts = store
            .load_attempts_page(owner_id, run_id, StoreReadPage::first(32).unwrap())
            .await
            .unwrap();
        assert_eq!(attempts.items().len(), 1);
        assert_eq!(attempts.items()[0].attempt_number(), 1);
        assert_eq!(
            attempts.items()[0].state(),
            anima_core::AttemptRecordState::Completed
        );
    }
}

async fn run_after_dispatch_crash(
    recovery_mode: RecoveryMode,
    seed: u128,
    revoke_before_retry_dispatch: bool,
) -> (
    EngineRunOutcome,
    u32,
    Vec<(Uuid, u32, anima_core::AttemptRecordState)>,
) {
    let owner_id = Uuid::from_u128(seed + 1);
    let run_id = Uuid::from_u128(seed + 3);
    let capability_manifest = manifest(recovery_mode);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(seed + 2),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let clock = Arc::new(ManualExecutionClock::default());
    let store = Arc::new(InMemoryExecutionStore::with_clock(clock.clone()));
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let call = || ToolCall {
        id: "retry-step".into(),
        name: "workspace.write".into(),
        args: BTreeMap::from([("path".into(), DataValue::String("retry.txt".into()))]),
    };
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![call()]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            })],
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content {
                    text: "model-changed-without-tool".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })],
        ])),
    });
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(capability_manifest.clone())
        .unwrap();
    let external_calls = Arc::new(Mutex::new(0));
    let mut capabilities = CapabilityRegistry::new(catalog);
    capabilities
        .register_executor(Arc::new(AbsentThenSuccessfulExecutor {
            manifest: capability_manifest,
            calls: external_calls.clone(),
        }))
        .unwrap();
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(GuardFailurePolicy {
            mode: GuardFailureMode::Revoke,
            store: store.clone(),
            calls: AtomicUsize::new(0),
            trigger_call: if revoke_before_retry_dispatch {
                2
            } else {
                usize::MAX
            },
        }),
        Arc::new(capabilities),
        clock.clone(),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(CrashOnceAfterDispatch(AtomicBool::new(true))),
        DurableEngineConfig {
            lease_duration_ms: 1_000,
        },
    )
    .unwrap();

    let crashed = engine
        .run(owner_id, run_id, &ContinueSignal)
        .await
        .unwrap_err();
    assert_eq!(crashed.code(), EngineErrorCode::CrashInjected);
    assert_eq!(*external_calls.lock().unwrap(), 0);
    clock.advance_ms(1_001).unwrap();

    let recovered = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();
    let calls = *external_calls.lock().unwrap();
    let attempts = store
        .load_attempts_page(owner_id, run_id, StoreReadPage::first(32).unwrap())
        .await
        .unwrap()
        .items()
        .iter()
        .map(|attempt| {
            (
                attempt.invocation().id(),
                attempt.attempt_number(),
                attempt.state(),
            )
        })
        .collect();
    (recovered, calls, attempts)
}

#[tokio::test]
async fn crash_after_dispatch_uses_authorized_same_key_retry_for_every_safe_retry_mode() {
    for (offset, mode) in [
        RecoveryMode::InherentlyIdempotent,
        RecoveryMode::KeyedIdempotent,
        RecoveryMode::Reconcilable,
        RecoveryMode::Retry,
    ]
    .into_iter()
    .enumerate()
    {
        let (outcome, calls, attempts) =
            run_after_dispatch_crash(mode, 0x690 + (offset as u128 * 0x10), false).await;
        assert!(matches!(
            outcome,
            EngineRunOutcome::Completed { ref content } if content.text == "model-changed-without-tool"
        ));
        assert_eq!(calls, 1);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].0, attempts[1].0);
        assert_eq!(attempts[0].1, 1);
        assert_eq!(attempts[0].2, anima_core::AttemptRecordState::Uncertain);
        assert_eq!(attempts[1].1, 2);
        assert_eq!(attempts[1].2, anima_core::AttemptRecordState::Completed);
    }
}

#[tokio::test]
async fn crash_after_dispatch_pauses_every_manual_or_compensating_recovery_mode() {
    for (offset, mode) in [
        RecoveryMode::NonRetryable,
        RecoveryMode::None,
        RecoveryMode::Manual,
        RecoveryMode::Compensate,
    ]
    .into_iter()
    .enumerate()
    {
        let (outcome, calls, attempts) =
            run_after_dispatch_crash(mode, 0x6d0 + (offset as u128 * 0x10), false).await;
        assert_eq!(outcome, EngineRunOutcome::RecoveryRequired);
        assert_eq!(calls, 0);
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].1, 1);
        assert_eq!(attempts[0].2, anima_core::AttemptRecordState::Uncertain);
    }
}

#[tokio::test]
async fn revoked_policy_before_recovery_retry_denies_without_calling_executor() {
    let (outcome, calls, attempts) =
        run_after_dispatch_crash(RecoveryMode::KeyedIdempotent, 0x750, true).await;

    assert_eq!(outcome, EngineRunOutcome::Denied);
    assert_eq!(calls, 0);
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].1, 1);
    assert_eq!(attempts[0].2, anima_core::AttemptRecordState::Uncertain);
}

#[tokio::test]
async fn approval_required_capability_checkpoints_before_waiting_and_never_calls_executor() {
    let owner_id = Uuid::from_u128(0x661);
    let run_id = Uuid::from_u128(0x663);
    let capability_manifest = manifest_with_risk(RecoveryMode::NonRetryable, RiskLevel::Medium);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(0x662),
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([vec![ModelStreamFrame::Final(
            ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![ToolCall {
                    id: "approval-step".into(),
                    name: "workspace.write".into(),
                    args: BTreeMap::from([(
                        "path".into(),
                        DataValue::String("approval.txt".into()),
                    )]),
                }]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            },
        )]])),
    });
    let capabilities = Arc::new(RecordingCapabilities {
        manifest: capability_manifest,
        calls: Mutex::new(vec![]),
        completed: Mutex::new(None),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(AllowPolicy),
        capabilities.clone(),
        Arc::new(ManualExecutionClock::default()),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();

    assert_eq!(outcome, EngineRunOutcome::WaitingForApproval);
    assert!(capabilities.calls.lock().unwrap().is_empty());
    let waiting = store.load_run(owner_id, run_id).await.unwrap().unwrap();
    assert_eq!(
        waiting.run().state(),
        anima_core::RunState::WaitingForApproval
    );
    let (_, checkpoint) = store
        .load_checkpoint(owner_id, run_id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.pending_approval().is_some());
    assert_eq!(
        checkpoint.cursor().unwrap().logical_step_id(),
        "approval-step"
    );
}

#[tokio::test]
async fn approved_waiting_invocation_is_claimed_and_dispatched_exactly_once() {
    let owner_id = Uuid::from_u128(0x681);
    let run_id = Uuid::from_u128(0x683);
    let capability_manifest = manifest_with_risk(RecoveryMode::NonRetryable, RiskLevel::Medium);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(0x682),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let clock = Arc::new(ManualExecutionClock::default());
    let store = Arc::new(InMemoryExecutionStore::with_clock(clock.clone()));
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let call = || ToolCall {
        id: "approved-step".into(),
        name: "workspace.write".into(),
        args: BTreeMap::from([("path".into(), DataValue::String("approved.txt".into()))]),
    };
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![call()]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            })],
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content {
                    text: "approved".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })],
        ])),
    });
    let capabilities = Arc::new(RecordingCapabilities {
        manifest: capability_manifest,
        calls: Mutex::new(vec![]),
        completed: Mutex::new(None),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(ApprovePendingPolicy),
        capabilities.clone(),
        clock.clone(),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig {
            lease_duration_ms: 1_000,
        },
    )
    .unwrap();

    assert_eq!(
        engine.run(owner_id, run_id, &ContinueSignal).await.unwrap(),
        EngineRunOutcome::WaitingForApproval
    );
    assert!(capabilities.calls.lock().unwrap().is_empty());
    clock.advance_ms(1_001).unwrap();

    let resumed = match engine.run(owner_id, run_id, &ContinueSignal).await {
        Ok(outcome) => outcome,
        Err(error) => panic!(
            "resume failed: {error:?}; run={:?}; checkpoint={:?}; events={:?}",
            store.load_run(owner_id, run_id).await.unwrap(),
            store.load_checkpoint(owner_id, run_id).await.unwrap(),
            store
                .replay_events(owner_id, run_id, StoreReadPage::first(256).unwrap())
                .await
                .unwrap()
                .events(),
        ),
    };

    assert!(matches!(
        resumed,
        EngineRunOutcome::Completed { ref content } if content.text == "approved"
    ));
    assert_eq!(capabilities.calls.lock().unwrap().len(), 1);
    let events = store
        .replay_events(owner_id, run_id, StoreReadPage::first(256).unwrap())
        .await
        .unwrap();
    assert!(events
        .events()
        .iter()
        .any(|event| event.kind() == RuntimeEventKind::ApprovalResolved));
}

async fn assert_rejected_waiting_approval(stale: bool, seed: u128) {
    let owner_id = Uuid::from_u128(seed + 1);
    let run_id = Uuid::from_u128(seed + 3);
    let capability_manifest = manifest_with_risk(RecoveryMode::NonRetryable, RiskLevel::Medium);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(seed + 2),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let clock = Arc::new(ManualExecutionClock::default());
    let store = Arc::new(InMemoryExecutionStore::with_clock(clock.clone()));
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let call = || ToolCall {
        id: "denied-step".into(),
        name: "workspace.write".into(),
        args: BTreeMap::from([("path".into(), DataValue::String("denied.txt".into()))]),
    };
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![call()]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            })],
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![call()]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            })],
        ])),
    });
    let capabilities = Arc::new(RecordingCapabilities {
        manifest: capability_manifest,
        calls: Mutex::new(vec![]),
        completed: Mutex::new(None),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        Arc::new(DenyPendingPolicy { stale }),
        capabilities.clone(),
        clock.clone(),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig {
            lease_duration_ms: 1_000,
        },
    )
    .unwrap();

    assert_eq!(
        engine.run(owner_id, run_id, &ContinueSignal).await.unwrap(),
        EngineRunOutcome::WaitingForApproval
    );
    clock.advance_ms(1_001).unwrap();

    let outcome = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();

    assert_eq!(outcome, EngineRunOutcome::Denied);
    assert!(capabilities.calls.lock().unwrap().is_empty());
    let denied = store.load_run(owner_id, run_id).await.unwrap().unwrap();
    assert_eq!(denied.run().state(), anima_core::RunState::Paused);
    assert_eq!(
        denied.run().pause_reason(),
        Some(anima_core::RunPauseReason::PolicyDenied)
    );
    assert!(denied.run().resume(None, None).is_err());
    assert!(denied.run().pending_approval().is_none());
    let (_, checkpoint) = store
        .load_checkpoint(owner_id, run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.state(), anima_core::RunState::Paused);
    assert!(checkpoint.pending_approval().is_none());
}

#[tokio::test]
async fn denied_or_stale_waiting_approval_pauses_durably_without_executor_entry() {
    assert_rejected_waiting_approval(false, 0x684).await;
    assert_rejected_waiting_approval(true, 0x694).await;
}

async fn assert_dispatch_guard_rejection(mode: Option<GuardFailureMode>, seed: u128) {
    let owner_id = Uuid::from_u128(seed + 1);
    let run_id = Uuid::from_u128(seed + 3);
    let capability_manifest = manifest(RecoveryMode::KeyedIdempotent);
    let mut definition = definition();
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(seed + 2),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let clock = Arc::new(ManualExecutionClock::default());
    let store = Arc::new(InMemoryExecutionStore::with_clock(clock.clone()));
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
                AuthoritativePolicyState::active(
                    owner_id,
                    definition.id.clone(),
                    definition.version,
                    definition.approval_policy_revision,
                    mode.is_none().then_some(clock.now_ms()),
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([vec![ModelStreamFrame::Final(
            ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![ToolCall {
                    id: "guarded-step".into(),
                    name: capability_manifest.id.clone(),
                    args: BTreeMap::from([(
                        "path".into(),
                        DataValue::String("guarded.txt".into()),
                    )]),
                }]),
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::ToolCall,
            },
        )]])),
    });
    let capabilities = Arc::new(RecordingCapabilities {
        manifest: capability_manifest,
        calls: Mutex::new(vec![]),
        completed: Mutex::new(None),
    });
    let policy = Arc::new(GuardFailurePolicy {
        mode: mode.unwrap_or(GuardFailureMode::Revoke),
        store: store.clone(),
        calls: AtomicUsize::new(if mode.is_none() { 2 } else { 0 }),
        trigger_call: 1,
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model,
        Arc::new(StaticDefinitions(definition)),
        policy,
        capabilities.clone(),
        clock,
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();

    assert_eq!(outcome, EngineRunOutcome::Denied);
    assert!(capabilities.calls.lock().unwrap().is_empty());
    let denied = store.load_run(owner_id, run_id).await.unwrap().unwrap();
    assert_eq!(denied.run().state(), anima_core::RunState::Paused);
    assert_eq!(
        denied.run().pause_reason(),
        Some(anima_core::RunPauseReason::PolicyDenied)
    );
}

#[tokio::test]
async fn dispatch_policy_guard_denies_revoke_expiry_argument_drift_and_lost_cas() {
    assert_dispatch_guard_rejection(Some(GuardFailureMode::Revoke), 0x710).await;
    assert_dispatch_guard_rejection(Some(GuardFailureMode::AdvanceRevision), 0x720).await;
    assert_dispatch_guard_rejection(Some(GuardFailureMode::ArgumentDrift), 0x730).await;
    assert_dispatch_guard_rejection(None, 0x740).await;
}

#[tokio::test]
async fn exhausted_turn_budget_pauses_durably_before_the_next_model_boundary() {
    let owner_id = Uuid::from_u128(0x671);
    let run_id = Uuid::from_u128(0x673);
    let capability_manifest = manifest(RecoveryMode::KeyedIdempotent);
    let mut definition = definition();
    definition.limits.max_turns = 1;
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: capability_manifest.id.clone(),
        manifest_version: capability_manifest.version,
        schema_digest: capability_manifest.schema_digest().into(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let session = Session::new_for_definition(
        Uuid::from_u128(0x672),
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
    authorize_policy(store.as_ref(), owner_id, &definition).await;
    let model = Arc::new(StreamingModel {
        turns: Mutex::new(VecDeque::from([
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: Some(vec![ToolCall {
                    id: "budget-step".into(),
                    name: "workspace.write".into(),
                    args: BTreeMap::from([("path".into(), DataValue::String("budget.txt".into()))]),
                }]),
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                stop_reason: ModelStopReason::ToolCall,
            })],
            vec![ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content::default(),
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })],
        ])),
    });
    let capabilities = Arc::new(RecordingCapabilities {
        manifest: capability_manifest,
        calls: Mutex::new(vec![]),
        completed: Mutex::new(None),
    });
    let engine = DurableAgentEngine::new(
        store.clone(),
        model.clone(),
        Arc::new(StaticDefinitions(definition)),
        Arc::new(AllowPolicy),
        capabilities,
        Arc::new(ManualExecutionClock::default()),
        Arc::new(RecordingLiveSink::default()),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = engine.run(owner_id, run_id, &ContinueSignal).await.unwrap();

    assert_eq!(outcome, EngineRunOutcome::PausedForBudget);
    assert_eq!(model.turns.lock().unwrap().len(), 1);
    let paused = store.load_run(owner_id, run_id).await.unwrap().unwrap();
    assert_eq!(paused.run().state(), anima_core::RunState::Paused);
    assert_eq!(
        paused.run().pause_reason(),
        Some(anima_core::RunPauseReason::Budget)
    );
    let (_, checkpoint) = store
        .load_checkpoint(owner_id, run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.state(), anima_core::RunState::Paused);
    assert_eq!(
        checkpoint.pause_reason(),
        Some(anima_core::RunPauseReason::Budget)
    );
}
