use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::{select, Either};
use futures::lock::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::{
    AgentConfig, AgentDefinition, ApprovalDecision, ApprovalGrantMutation, ApprovalRequest,
    ApprovalResumeClaim, AutonomyGrant, Budget, CapabilityAttempt, CapabilityError,
    CapabilityExecutionContext, CapabilityManifest, CapabilityResult, CapabilityRetryAuthorization,
    CheckpointCursor, CheckpointV1, CheckpointV1Builder, CompletedInvocationRecord, Content,
    DefinitionPin, DispatchGrantMutation, DispatchPolicyGuard, DurableCapabilityResult,
    DurableResultMutation, ExecutionClock, ExecutionStep, ExecutionStore, InvocationAttemptRecord,
    LogicalInvocation, ManifestPin, Message, MessageRole, ModelAdapter, ModelGenerateRequest,
    ModelGenerateResponse, ModelStreamFrame, ModelStreamSink, OpaqueReference,
    PendingApprovalRecord, PolicyContext, PolicyDecision, PolicyEngine, RecoveryAction, Run,
    RunState, RuntimeCommand, RuntimeEvent, RuntimeEventKind, StepKind, ToolCall, Usage,
};

const ENGINE_ID_NAMESPACE: Uuid = Uuid::from_u128(0x8d6d_9262_9e34_5b79_8ff0_89c1_240c_b241);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineBoundaryAction {
    Continue,
    Pause,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineCrashPoint {
    AfterDispatchCommitBeforeExecutor,
    AfterExecutorBeforeResultCommit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineRunOutcome {
    Completed { content: Content },
    WaitingForApproval,
    PausedForBudget,
    PausedByRequest,
    Cancelled,
    RecoveryRequired,
    Denied,
}

type CapabilityCommit = (
    crate::StoredRun,
    crate::ExecutionLease,
    u64,
    CheckpointV1,
    Message,
);

enum CapabilityStepOutcome {
    Committed(Box<CapabilityCommit>),
    Adopted(EngineRunOutcome),
}

impl CapabilityStepOutcome {
    fn committed(value: CapabilityCommit) -> Self {
        Self::Committed(Box::new(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineErrorCode {
    Store,
    DefinitionUnavailable,
    ManifestUnavailable,
    Model,
    Policy,
    Capability,
    InvalidState,
    CrashInjected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineError {
    code: EngineErrorCode,
}

impl EngineError {
    pub const fn new(code: EngineErrorCode) -> Self {
        Self { code }
    }

    pub const fn code(self) -> EngineErrorCode {
        self.code
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            EngineErrorCode::Store => "durable execution store rejected the operation",
            EngineErrorCode::DefinitionUnavailable => "the pinned agent definition is unavailable",
            EngineErrorCode::ManifestUnavailable => "a pinned capability manifest is unavailable",
            EngineErrorCode::Model => "the model adapter failed",
            EngineErrorCode::Policy => "current policy evaluation failed",
            EngineErrorCode::Capability => "capability execution failed",
            EngineErrorCode::InvalidState => "the durable engine state is invalid",
            EngineErrorCode::CrashInjected => "a deterministic crash was injected",
        })
    }
}

impl std::error::Error for EngineError {}

#[derive(Clone)]
pub struct EnginePolicyRequest {
    owner_id: Uuid,
    definition: AgentDefinition,
    manifest: CapabilityManifest,
    invocation: LogicalInvocation,
    pending_approval: Option<ApprovalRequest>,
}

impl EnginePolicyRequest {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn definition(&self) -> &AgentDefinition {
        &self.definition
    }
    pub fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }
    pub fn invocation(&self) -> &LogicalInvocation {
        &self.invocation
    }
    pub fn pending_approval(&self) -> Option<&ApprovalRequest> {
        self.pending_approval.as_ref()
    }
}

impl fmt::Debug for EnginePolicyRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnginePolicyRequest")
            .field("owner_id", &"REDACTED")
            .field("definition_version", &self.definition.version)
            .field("manifest_version", &self.manifest.version)
            .field("logical_invocation_id", &self.invocation.id())
            .field("pending_approval", &self.pending_approval.is_some())
            .finish()
    }
}

#[derive(Clone)]
pub struct CurrentPolicyResolution {
    context: PolicyContext,
    grants: Vec<AutonomyGrant>,
    approval: Option<ApprovalDecision>,
}

impl CurrentPolicyResolution {
    pub fn new(
        context: PolicyContext,
        grants: Vec<AutonomyGrant>,
        approval: Option<ApprovalDecision>,
    ) -> Self {
        Self {
            context,
            grants,
            approval,
        }
    }
    pub fn context(&self) -> &PolicyContext {
        &self.context
    }
    pub fn grants(&self) -> &[AutonomyGrant] {
        &self.grants
    }
    pub fn approval(&self) -> Option<&ApprovalDecision> {
        self.approval.as_ref()
    }
}

impl fmt::Debug for CurrentPolicyResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CurrentPolicyResolution")
            .field("policy_revision", &self.context.policy_revision)
            .field("grant_count", &self.grants.len())
            .field("approval_present", &self.approval.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct EngineCapabilityResult {
    pub output: CapabilityResult,
    pub durable: DurableCapabilityResult,
}

impl fmt::Debug for EngineCapabilityResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EngineCapabilityResult")
            .field("output", &"REDACTED")
            .field("durable", &self.durable)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub enum EngineLiveEvent {
    TextDelta {
        run_id: Uuid,
        text: String,
    },
    Semantic {
        run_id: Uuid,
        sequence: u64,
        kind: RuntimeEventKind,
    },
}

impl fmt::Debug for EngineLiveEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TextDelta { run_id, .. } => formatter
                .debug_struct("TextDelta")
                .field("run_id", run_id)
                .field("text", &"REDACTED")
                .finish(),
            Self::Semantic {
                run_id,
                sequence,
                kind,
            } => formatter
                .debug_struct("Semantic")
                .field("run_id", run_id)
                .field("sequence", sequence)
                .field("kind", kind)
                .finish(),
        }
    }
}

#[async_trait]
pub trait DefinitionResolver: Send + Sync {
    async fn resolve(&self, pin: &DefinitionPin) -> Result<AgentDefinition, EngineError>;
}

#[async_trait]
pub trait CurrentPolicyResolver: Send + Sync {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError>;
}

#[async_trait]
pub trait EngineCapabilityRuntime: Send + Sync {
    fn manifest(&self, id: &str, version: u32) -> Option<CapabilityManifest>;

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError>;

    async fn recover_dispatched(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError>;

    async fn execute_retry(
        &self,
        context: CapabilityExecutionContext,
        authorization: CapabilityRetryAuthorization,
    ) -> Result<EngineCapabilityResult, CapabilityError>;
}

#[async_trait]
impl EngineCapabilityRuntime for crate::CapabilityRegistry {
    fn manifest(&self, id: &str, version: u32) -> Option<CapabilityManifest> {
        crate::CapabilityRegistry::manifest(self, id, version).cloned()
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        let output = crate::CapabilityRegistry::execute(self, context.clone()).await?;
        let RecoveryAction::Completed(durable) =
            crate::CapabilityRegistry::recover(self, context).await?
        else {
            return Err(CapabilityError::reconciliation());
        };
        Ok(EngineCapabilityResult { output, durable })
    }

    async fn recover_dispatched(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        crate::CapabilityRegistry::recover_dispatched(self, context).await
    }

    async fn execute_retry(
        &self,
        context: CapabilityExecutionContext,
        authorization: CapabilityRetryAuthorization,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        let output =
            crate::CapabilityRegistry::execute_retry(self, context.clone(), authorization).await?;
        let RecoveryAction::Completed(durable) =
            crate::CapabilityRegistry::recover(self, context).await?
        else {
            return Err(CapabilityError::reconciliation());
        };
        Ok(EngineCapabilityResult { output, durable })
    }
}

#[async_trait]
pub trait EngineLiveEventSink: Send + Sync {
    async fn emit(&self, event: EngineLiveEvent) -> Result<(), EngineError>;
}

pub trait EngineControlSignal: Send + Sync {
    fn at_boundary(&self) -> EngineBoundaryAction;
}

pub trait EngineCrashInjector: Send + Sync {
    fn should_crash(&self, point: EngineCrashPoint) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableEngineConfig {
    pub lease_duration_ms: u64,
}

impl Default for DurableEngineConfig {
    fn default() -> Self {
        Self {
            lease_duration_ms: 30_000,
        }
    }
}

pub struct DurableAgentEngine<S = (), M = (), D = (), P = (), R = (), C = (), L = (), X = ()> {
    store: Arc<S>,
    model: Arc<M>,
    definitions: Arc<D>,
    policy: Arc<P>,
    capabilities: Arc<R>,
    clock: Arc<C>,
    live: Arc<L>,
    crash: Arc<X>,
    config: DurableEngineConfig,
}

impl<S, M, D, P, R, C, L, X> DurableAgentEngine<S, M, D, P, R, C, L, X>
where
    S: ExecutionStore,
    M: ModelAdapter,
    D: DefinitionResolver,
    P: CurrentPolicyResolver,
    R: EngineCapabilityRuntime,
    C: ExecutionClock,
    L: EngineLiveEventSink,
    X: EngineCrashInjector,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<S>,
        model: Arc<M>,
        definitions: Arc<D>,
        policy: Arc<P>,
        capabilities: Arc<R>,
        clock: Arc<C>,
        live: Arc<L>,
        crash: Arc<X>,
        config: DurableEngineConfig,
    ) -> Result<Self, EngineError> {
        if config.lease_duration_ms == 0 {
            return Err(EngineError::new(EngineErrorCode::InvalidState));
        }
        Ok(Self {
            store,
            model,
            definitions,
            policy,
            capabilities,
            clock,
            live,
            crash,
            config,
        })
    }

    pub async fn run(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        signal: &dyn EngineControlSignal,
    ) -> Result<EngineRunOutcome, EngineError> {
        let stored = self
            .store
            .load_run(owner_id, run_id)
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?
            .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
        if stored.owner_id() != owner_id {
            return Err(EngineError::new(EngineErrorCode::InvalidState));
        }
        let definition_pin = DefinitionPin::new(
            1,
            stored.run().definition_id(),
            stored.run().definition_version(),
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let definition = self.definitions.resolve(&definition_pin).await?;
        if DefinitionPin::from_definition(&definition)
            .map_err(|_| EngineError::new(EngineErrorCode::DefinitionUnavailable))?
            != definition_pin
        {
            return Err(EngineError::new(EngineErrorCode::DefinitionUnavailable));
        }
        let mut manifest_pins = Vec::with_capacity(definition.resolved_capabilities.len());
        for pin in &definition.resolved_capabilities {
            let manifest = self
                .capabilities
                .manifest(&pin.capability_id, pin.manifest_version)
                .ok_or_else(|| EngineError::new(EngineErrorCode::ManifestUnavailable))?;
            if manifest.schema_digest() != pin.schema_digest {
                return Err(EngineError::new(EngineErrorCode::ManifestUnavailable));
            }
            manifest_pins.push(
                ManifestPin::from_manifest(&manifest)
                    .map_err(|_| EngineError::new(EngineErrorCode::ManifestUnavailable))?,
            );
        }
        let mut lease = self
            .store
            .acquire_lease(
                owner_id,
                run_id,
                stored.run_version(),
                self.config.lease_duration_ms,
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        let mut current = stored;
        let loaded_checkpoint = self
            .store
            .load_checkpoint(owner_id, run_id)
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        let (mut checkpoint_version, mut checkpoint, checkpoint_was_loaded) =
            match loaded_checkpoint {
                Some((version, checkpoint)) => (version, checkpoint, true),
                None => (
                    0,
                    CheckpointV1Builder::new(
                        current.run().session_id(),
                        run_id,
                        definition_pin.clone(),
                        1,
                        manifest_pins,
                        Budget {
                            max_turns: Some(u64::from(definition.limits.max_turns)),
                            ..Budget::default()
                        },
                        Usage::default(),
                    )
                    .state(RunState::Running, None)
                    .build()
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    false,
                ),
            };
        if current.run().state() == RunState::Queued {
            let running = current
                .run()
                .transition(RunState::Running, None)
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let sequence = if checkpoint_was_loaded {
                checkpoint.last_durable_event_sequence() + 1
            } else {
                1
            };
            let event = self.event(
                owner_id,
                current.run(),
                sequence,
                RuntimeEventKind::RunStarted,
            )?;
            if checkpoint_was_loaded {
                checkpoint = self.rebuild_checkpoint(
                    &checkpoint,
                    sequence,
                    RunState::Running,
                    Usage::default(),
                )?;
            }
            let outcome = self
                .store
                .commit_execution(
                    owner_id,
                    crate::ExecutionCommit::new(
                        current.run_version(),
                        checkpoint_version,
                        lease.clone(),
                        RuntimeCommand::start(
                            deterministic_id(run_id, "start", sequence),
                            current.run().session_id(),
                            run_id,
                        )
                        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                        vec![event],
                        vec![],
                        vec![],
                        vec![],
                        None,
                        running,
                    )
                    .with_checkpoint(checkpoint.clone()),
                )
                .await
                .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
            self.emit_semantic(run_id, sequence, RuntimeEventKind::RunStarted)
                .await;
            current = outcome.stored_run().clone();
            checkpoint_version = outcome.checkpoint_version();
            lease = self.renew(owner_id, lease).await?;
        } else if !checkpoint_was_loaded {
            return Err(EngineError::new(EngineErrorCode::InvalidState));
        }
        if !matches!(
            current.run().state(),
            RunState::Running | RunState::WaitingForApproval | RunState::RecoveryRequired
        ) {
            return Err(EngineError::new(EngineErrorCode::InvalidState));
        }
        match signal.at_boundary() {
            EngineBoundaryAction::Continue => {}
            EngineBoundaryAction::Pause => {
                return self
                    .pause(owner_id, current, lease, checkpoint_version, &checkpoint)
                    .await
            }
            EngineBoundaryAction::Cancel => {
                return self
                    .cancel(owner_id, current, lease, checkpoint_version, &checkpoint)
                    .await
            }
        }

        if current.run().state() == RunState::RecoveryRequired {
            return Ok(EngineRunOutcome::RecoveryRequired);
        }
        let mut messages = Vec::new();
        if let Some(active) = checkpoint.attempts().iter().find(|attempt| {
            matches!(
                attempt.state(),
                crate::AttemptRecordState::Pending
                    | crate::AttemptRecordState::Dispatching
                    | crate::AttemptRecordState::Uncertain
            )
        }) {
            let invocation = active
                .durable_invocation()
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?
                .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
            let tool_call = tool_call_from_invocation(&invocation)?;
            let executed = self
                .execute_capability(
                    owner_id,
                    &definition,
                    current,
                    lease,
                    checkpoint_version,
                    checkpoint,
                    tool_call,
                )
                .await?;
            let CapabilityStepOutcome::Committed(executed) = executed else {
                let CapabilityStepOutcome::Adopted(outcome) = executed else {
                    unreachable!()
                };
                return Ok(outcome);
            };
            current = executed.0;
            lease = executed.1;
            checkpoint_version = executed.2;
            checkpoint = executed.3;
            messages.push(executed.4);
            match current.run().state() {
                RunState::WaitingForApproval => return Ok(EngineRunOutcome::WaitingForApproval),
                RunState::RecoveryRequired => return Ok(EngineRunOutcome::RecoveryRequired),
                RunState::Paused
                    if current.run().pause_reason()
                        == Some(crate::RunPauseReason::PolicyDenied) =>
                {
                    return Ok(EngineRunOutcome::Denied)
                }
                RunState::Running => {}
                _ => return Err(EngineError::new(EngineErrorCode::InvalidState)),
            }
        } else if current.run().state() == RunState::WaitingForApproval {
            return Err(EngineError::new(EngineErrorCode::InvalidState));
        }
        let response = loop {
            if checkpoint
                .budget()
                .max_turns()
                .is_some_and(|maximum| checkpoint.usage().turns() >= maximum)
            {
                return self
                    .pause_for_budget(owner_id, current, lease, checkpoint_version, &checkpoint)
                    .await;
            }
            {
                let started_sequence = checkpoint.last_durable_event_sequence() + 1;
                let started_event = self.event(
                    owner_id,
                    current.run(),
                    started_sequence,
                    RuntimeEventKind::ModelStarted,
                )?;
                checkpoint = self.rebuild_checkpoint(
                    &checkpoint,
                    started_sequence,
                    RunState::Running,
                    checkpoint.usage().clone(),
                )?;
                let started = self
                    .store
                    .commit_execution(
                        owner_id,
                        crate::ExecutionCommit::new(
                            current.run_version(),
                            checkpoint_version,
                            lease.clone(),
                            RuntimeCommand::record_progress(
                                deterministic_id(run_id, "model-started", started_sequence),
                                current.run().session_id(),
                                run_id,
                            )
                            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                            vec![started_event],
                            vec![],
                            vec![],
                            vec![],
                            None,
                            current.run().clone(),
                        )
                        .with_checkpoint(checkpoint.clone()),
                    )
                    .await
                    .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
                self.emit_semantic(run_id, started_sequence, RuntimeEventKind::ModelStarted)
                    .await;
                current = started.stored_run().clone();
                checkpoint_version = started.checkpoint_version();
                lease = self.renew(owner_id, lease).await?;
            }

            let collector = EngineModelSink {
                run_id,
                live: self.live.as_ref(),
                final_response: Mutex::new(None),
            };
            let model_request = ModelGenerateRequest {
                system: definition.system.clone(),
                messages: messages.clone(),
                temperature: definition.model.temperature,
                max_tokens: None,
            };
            let (stream_result, renewed) = self
                .await_with_lease_heartbeat(
                    owner_id,
                    lease,
                    self.model
                        .stream(&agent_config(&definition), &model_request, &collector),
                )
                .await?;
            lease = renewed;
            stream_result.map_err(|_| EngineError::new(EngineErrorCode::Model))?;
            let response = collector
                .final_response
                .into_inner()
                .map_err(|_| EngineError::new(EngineErrorCode::Model))?
                .ok_or_else(|| EngineError::new(EngineErrorCode::Model))?;
            {
                let usage = checkpoint
                    .usage()
                    .checked_add(&Usage {
                        turns: 1,
                        input_tokens: response.usage.prompt_tokens,
                        output_tokens: response.usage.completion_tokens,
                        total_tokens: response.usage.total_tokens,
                        ..Usage::default()
                    })
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
                let completed_sequence = checkpoint.last_durable_event_sequence() + 1;
                let model_completed = self.event(
                    owner_id,
                    current.run(),
                    completed_sequence,
                    RuntimeEventKind::ModelCompleted,
                )?;
                checkpoint = self.rebuild_checkpoint(
                    &checkpoint,
                    completed_sequence,
                    RunState::Running,
                    usage,
                )?;
                let recorded = self
                    .store
                    .commit_execution(
                        owner_id,
                        crate::ExecutionCommit::new(
                            current.run_version(),
                            checkpoint_version,
                            lease.clone(),
                            RuntimeCommand::record_progress(
                                deterministic_id(run_id, "model-completed", completed_sequence),
                                current.run().session_id(),
                                run_id,
                            )
                            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                            vec![model_completed],
                            vec![],
                            vec![],
                            vec![],
                            None,
                            current.run().clone(),
                        )
                        .with_checkpoint(checkpoint.clone()),
                    )
                    .await
                    .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
                self.emit_semantic(run_id, completed_sequence, RuntimeEventKind::ModelCompleted)
                    .await;
                current = recorded.stored_run().clone();
                checkpoint_version = recorded.checkpoint_version();
                lease = self.renew(owner_id, lease).await?;
            }

            let tool_calls = response.tool_calls.clone().unwrap_or_default();
            if tool_calls.is_empty() {
                break response;
            }
            for tool_call in tool_calls {
                let executed = self
                    .execute_capability(
                        owner_id,
                        &definition,
                        current,
                        lease,
                        checkpoint_version,
                        checkpoint,
                        tool_call,
                    )
                    .await?;
                let CapabilityStepOutcome::Committed(executed) = executed else {
                    let CapabilityStepOutcome::Adopted(outcome) = executed else {
                        unreachable!()
                    };
                    return Ok(outcome);
                };
                current = executed.0;
                lease = executed.1;
                checkpoint_version = executed.2;
                checkpoint = executed.3;
                messages.push(executed.4);
                match current.run().state() {
                    RunState::WaitingForApproval => {
                        return Ok(EngineRunOutcome::WaitingForApproval)
                    }
                    RunState::RecoveryRequired => return Ok(EngineRunOutcome::RecoveryRequired),
                    RunState::Paused
                        if current.run().pause_reason()
                            == Some(crate::RunPauseReason::PolicyDenied) =>
                    {
                        return Ok(EngineRunOutcome::Denied)
                    }
                    _ => {}
                }
            }
            match signal.at_boundary() {
                EngineBoundaryAction::Continue => {}
                EngineBoundaryAction::Pause => {
                    return self
                        .pause(owner_id, current, lease, checkpoint_version, &checkpoint)
                        .await
                }
                EngineBoundaryAction::Cancel => {
                    return self
                        .cancel(owner_id, current, lease, checkpoint_version, &checkpoint)
                        .await
                }
            }
        };

        let terminal = current
            .run()
            .transition(RunState::Completed, None)
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let sequence = checkpoint.last_durable_event_sequence() + 1;
        let event = self.event(
            owner_id,
            current.run(),
            sequence,
            RuntimeEventKind::RunCompleted,
        )?;
        self.store
            .commit_execution(
                owner_id,
                crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease,
                    RuntimeCommand::complete(
                        deterministic_id(run_id, "complete", sequence),
                        current.run().session_id(),
                        run_id,
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    vec![event],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    terminal,
                ),
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        self.emit_semantic(run_id, sequence, RuntimeEventKind::RunCompleted)
            .await;
        Ok(EngineRunOutcome::Completed {
            content: response.content,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_capability(
        &self,
        owner_id: Uuid,
        definition: &AgentDefinition,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        checkpoint: CheckpointV1,
        tool_call: ToolCall,
    ) -> Result<CapabilityStepOutcome, EngineError> {
        let pin = definition
            .resolved_capabilities
            .iter()
            .find(|pin| pin.capability_id == tool_call.name)
            .ok_or_else(|| EngineError::new(EngineErrorCode::ManifestUnavailable))?;
        let manifest = self
            .capabilities
            .manifest(&pin.capability_id, pin.manifest_version)
            .ok_or_else(|| EngineError::new(EngineErrorCode::ManifestUnavailable))?;
        if manifest.schema_digest() != pin.schema_digest {
            return Err(EngineError::new(EngineErrorCode::ManifestUnavailable));
        }
        let arguments = crate::runtime_serde::data_value_to_json(&crate::DataValue::Object(
            tool_call.args.clone(),
        ));
        let invocation = LogicalInvocation::new(
            current.run().id(),
            tool_call.id.clone(),
            tool_call.name.clone(),
            pin.manifest_version,
            arguments,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
        let recovery_attempt_number = checkpoint
            .attempts()
            .iter()
            .find(|attempt| {
                attempt.invocation().id() == invocation.id()
                    && matches!(
                        attempt.state(),
                        crate::AttemptRecordState::Dispatching
                            | crate::AttemptRecordState::Uncertain
                    )
            })
            .map(InvocationAttemptRecord::attempt_number);
        if let Some(attempt_number) = recovery_attempt_number {
            return self
                .recover_checkpointed_capability(
                    owner_id,
                    definition,
                    current,
                    lease,
                    checkpoint_version,
                    checkpoint,
                    invocation,
                    tool_call,
                    manifest,
                    attempt_number,
                )
                .await;
        }
        let request = || EnginePolicyRequest {
            owner_id,
            definition: definition.clone(),
            manifest: manifest.clone(),
            invocation: invocation.clone(),
            pending_approval: current.run().pending_approval().cloned(),
        };
        let first = self.policy.resolve(request()).await?;
        let first_evaluation =
            PolicyEngine::evaluate_with_approval(first.context(), first.grants(), first.approval())
                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        if !policy_context_matches(first.context(), definition, &invocation) {
            return self
                .commit_policy_denied(
                    owner_id,
                    definition,
                    current,
                    lease,
                    checkpoint_version,
                    checkpoint,
                    invocation,
                )
                .await
                .map(CapabilityStepOutcome::committed);
        }
        if current.run().state() == RunState::WaitingForApproval
            && !matches!(first_evaluation.decision, PolicyDecision::Allow(_))
        {
            let latest = self.policy.resolve(request()).await?;
            let latest_evaluation = PolicyEngine::evaluate_with_approval(
                latest.context(),
                latest.grants(),
                latest.approval(),
            )
            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
            if !matches!(latest_evaluation.decision, PolicyDecision::Allow(_)) {
                return self
                    .commit_policy_denied(
                        owner_id,
                        definition,
                        current,
                        lease,
                        checkpoint_version,
                        checkpoint,
                        invocation,
                    )
                    .await
                    .map(CapabilityStepOutcome::committed);
            }
        }
        if matches!(first_evaluation.decision, PolicyDecision::Deny(_)) {
            let latest = self.policy.resolve(request()).await?;
            let latest_evaluation = PolicyEngine::evaluate_with_approval(
                latest.context(),
                latest.grants(),
                latest.approval(),
            )
            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
            if !matches!(latest_evaluation.decision, PolicyDecision::Allow(_)) {
                return self
                    .commit_policy_denied(
                        owner_id,
                        definition,
                        current,
                        lease,
                        checkpoint_version,
                        checkpoint,
                        invocation,
                    )
                    .await
                    .map(CapabilityStepOutcome::committed);
            }
        }
        if matches!(
            first_evaluation.decision,
            PolicyDecision::RequireApproval(_)
        ) {
            let approval_request = PolicyEngine::approval_request(first.context(), None)
                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
            let pending = InvocationAttemptRecord::new_durable(
                &invocation,
                1,
                crate::AttemptRecordState::Pending,
                ManifestPin::from_manifest(&manifest)
                    .map_err(|_| EngineError::new(EngineErrorCode::ManifestUnavailable))?,
                manifest.recovery_mode,
            )
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let step = ExecutionStep::new(
                current.run().id(),
                tool_call.id.clone(),
                StepKind::Capability,
            )
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let first_sequence = checkpoint.last_durable_event_sequence() + 1;
            let kinds = [
                RuntimeEventKind::CapabilityProposed,
                RuntimeEventKind::PolicyEvaluated,
                RuntimeEventKind::ApprovalRequested,
                RuntimeEventKind::RunWaitingForApproval,
            ];
            let events = kinds
                .into_iter()
                .enumerate()
                .map(|(offset, kind)| {
                    self.event(
                        owner_id,
                        current.run(),
                        first_sequence + u64::try_from(offset).unwrap_or(0),
                        kind,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let last_sequence = first_sequence + 3;
            let mut attempts = checkpoint.attempts().to_vec();
            attempts.push(pending.clone());
            let cursor = CheckpointCursor::new(invocation.id(), 1, tool_call.id.clone())
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let waiting_checkpoint = CheckpointV1Builder::new(
                checkpoint.session_id(),
                checkpoint.run_id(),
                checkpoint.definition().clone(),
                last_sequence,
                checkpoint.manifests().to_vec(),
                checkpoint.budget().clone(),
                checkpoint.usage().clone(),
            )
            .state(RunState::WaitingForApproval, None)
            .attempts(attempts)
            .completed_invocations(checkpoint.completed_invocations().to_vec())
            .uncertain_invocations(checkpoint.uncertain_invocations().to_vec())
            .pending_approval(Some(
                PendingApprovalRecord::new(approval_request.clone(), None)
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
            ))
            .message_context_refs(checkpoint.message_context_refs().to_vec())
            .model_context_refs(checkpoint.model_context_refs().to_vec())
            .memory_refs(checkpoint.memory_refs().to_vec())
            .artifact_refs(checkpoint.artifact_refs().to_vec())
            .cursor(Some(cursor))
            .build()
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let waiting = current
                .run()
                .wait_for_approval(approval_request.clone())
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let committed = self
                .store
                .commit_execution(
                    owner_id,
                    crate::ExecutionCommit::new(
                        current.run_version(),
                        checkpoint_version,
                        lease.clone(),
                        RuntimeCommand::request_approval(
                            deterministic_id(current.run().id(), "approval", last_sequence),
                            current.run().session_id(),
                            current.run().id(),
                            approval_request,
                        )
                        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                        events,
                        vec![step],
                        vec![pending],
                        vec![],
                        None,
                        waiting,
                    )
                    .with_checkpoint(waiting_checkpoint.clone()),
                )
                .await
                .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
            for (offset, kind) in kinds.into_iter().enumerate() {
                self.emit_semantic(
                    current.run().id(),
                    first_sequence + u64::try_from(offset).unwrap_or(0),
                    kind,
                )
                .await;
            }
            let renewed = self.renew(owner_id, lease).await?;
            return Ok(CapabilityStepOutcome::committed((
                committed.stored_run().clone(),
                renewed,
                committed.checkpoint_version(),
                waiting_checkpoint,
                Message {
                    id: format!("approval:{}", invocation.id()),
                    agent_id: definition.id.clone(),
                    room_id: current.run().session_id().to_string(),
                    content: Content::default(),
                    role: MessageRole::Tool,
                    created_at_ms: self.clock.now_ms(),
                },
            )));
        }
        if !matches!(first_evaluation.decision, PolicyDecision::Allow(_)) {
            return Err(EngineError::new(EngineErrorCode::Policy));
        }
        let current_policy = self.policy.resolve(request()).await?;
        let current_evaluation = PolicyEngine::evaluate_with_approval(
            current_policy.context(),
            current_policy.grants(),
            current_policy.approval(),
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        if !policy_context_matches(current_policy.context(), definition, &invocation) {
            return self
                .commit_policy_denied(
                    owner_id,
                    definition,
                    current,
                    lease,
                    checkpoint_version,
                    checkpoint,
                    invocation,
                )
                .await
                .map(CapabilityStepOutcome::committed);
        }
        if !matches!(current_evaluation.decision, PolicyDecision::Allow(_)) {
            return self
                .commit_policy_denied(
                    owner_id,
                    definition,
                    current,
                    lease,
                    checkpoint_version,
                    checkpoint,
                    invocation,
                )
                .await
                .map(CapabilityStepOutcome::committed);
        }
        let policy_guard = DispatchPolicyGuard::from_current_policy(
            owner_id,
            current_policy.context(),
            current_policy.grants(),
            current_policy.approval(),
            &current_evaluation,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        let dispatch_grant = if current.run().state() == RunState::Running {
            DispatchGrantMutation::from_current_policy(
                owner_id,
                current_policy.context(),
                current_policy.grants(),
                &current_evaluation,
            )
            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?
        } else {
            None
        };
        let approval_resume = if current.run().state() == RunState::WaitingForApproval {
            let pending = current
                .run()
                .pending_approval()
                .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
            let decision = current_policy
                .approval()
                .ok_or_else(|| EngineError::new(EngineErrorCode::Policy))?;
            let claim = ApprovalResumeClaim::new(
                pending,
                decision,
                current_policy.context(),
                current_policy.grants(),
            )
            .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
            let command = RuntimeCommand::resume_approval(
                deterministic_id(
                    current.run().id(),
                    "approval-resume",
                    checkpoint.last_durable_event_sequence() + 1,
                ),
                current.run().session_id(),
                current.run().id(),
                claim.binding().clone(),
            )
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let target = current
                .run()
                .apply_resume_command(&command, Some(&claim), None)
                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?
                .run()
                .clone();
            Some((command, target, ApprovalGrantMutation::from_claim(claim)))
        } else {
            None
        };
        let manifest_pin = ManifestPin::from_manifest(&manifest)
            .map_err(|_| EngineError::new(EngineErrorCode::ManifestUnavailable))?;
        let dispatching = InvocationAttemptRecord::new_durable(
            &invocation,
            1,
            crate::AttemptRecordState::Dispatching,
            manifest_pin.clone(),
            manifest.recovery_mode,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let step = ExecutionStep::new(
            current.run().id(),
            tool_call.id.clone(),
            StepKind::Capability,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let first_sequence = checkpoint.last_durable_event_sequence() + 1;
        let dispatch_kinds = if approval_resume.is_some() {
            vec![
                RuntimeEventKind::ApprovalResolved,
                RuntimeEventKind::CapabilityApproved,
                RuntimeEventKind::InvocationDispatchPrepared,
            ]
        } else {
            vec![
                RuntimeEventKind::CapabilityProposed,
                RuntimeEventKind::PolicyEvaluated,
                RuntimeEventKind::InvocationDispatchPrepared,
            ]
        };
        let events = dispatch_kinds
            .iter()
            .copied()
            .enumerate()
            .map(|(offset, kind)| {
                self.event(
                    owner_id,
                    current.run(),
                    first_sequence + u64::try_from(offset).unwrap_or(0),
                    kind,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let dispatch_sequence = first_sequence + 2;
        let mut dispatch_attempts = checkpoint.attempts().to_vec();
        if let Some(existing) = dispatch_attempts.iter_mut().find(|attempt| {
            attempt.invocation().id() == invocation.id() && attempt.attempt_number() == 1
        }) {
            if existing.state() != crate::AttemptRecordState::Pending || approval_resume.is_none() {
                return Err(EngineError::new(EngineErrorCode::InvalidState));
            }
            *existing = dispatching.clone();
        } else {
            dispatch_attempts.push(dispatching.clone());
        }
        let cursor = Some(
            CheckpointCursor::new(invocation.id(), 1, tool_call.id.clone())
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
        );
        let dispatch_checkpoint = if approval_resume.is_some() {
            self.rebuild_checkpoint_with_history_and_pending(
                &checkpoint,
                dispatch_sequence,
                checkpoint.usage().clone(),
                dispatch_attempts,
                checkpoint.completed_invocations().to_vec(),
                cursor,
                None,
            )?
        } else {
            self.rebuild_checkpoint_with_history(
                &checkpoint,
                dispatch_sequence,
                checkpoint.usage().clone(),
                dispatch_attempts,
                checkpoint.completed_invocations().to_vec(),
                cursor,
            )?
        };
        let (command, target, approval_mutation, steps) = match approval_resume {
            Some((command, target, mutation)) => (command, target, Some(mutation), vec![]),
            None => (
                RuntimeCommand::prepare_dispatch(
                    deterministic_id(current.run().id(), "dispatch", dispatch_sequence),
                    current.run().session_id(),
                    current.run().id(),
                )
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                current.run().clone(),
                None,
                vec![step.clone()],
            ),
        };
        let mut dispatch_commit = crate::ExecutionCommit::new(
            current.run_version(),
            checkpoint_version,
            lease.clone(),
            command,
            events,
            steps,
            vec![dispatching],
            vec![],
            approval_mutation,
            target,
        )
        .with_checkpoint(dispatch_checkpoint.clone())
        .with_policy_guard(policy_guard);
        if let Some(dispatch_grant) = dispatch_grant {
            dispatch_commit = dispatch_commit.with_dispatch_grant(dispatch_grant);
        }
        let prepared = match self.store.commit_execution(owner_id, dispatch_commit).await {
            Ok(prepared) => prepared,
            Err(error)
                if matches!(
                    error.code(),
                    crate::ExecutionStoreErrorCode::PolicyConflict
                        | crate::ExecutionStoreErrorCode::GrantConflict
                        | crate::ExecutionStoreErrorCode::GrantAlreadyConsumed
                ) =>
            {
                return self
                    .commit_policy_denied(
                        owner_id,
                        definition,
                        current,
                        lease,
                        checkpoint_version,
                        checkpoint,
                        invocation,
                    )
                    .await
                    .map(CapabilityStepOutcome::committed)
            }
            Err(error)
                if matches!(
                    error.code(),
                    crate::ExecutionStoreErrorCode::VersionConflict
                        | crate::ExecutionStoreErrorCode::CheckpointConflict
                ) =>
            {
                return self
                    .adopt_dispatch_conflict(owner_id, definition, current.run().id())
                    .await
            }
            Err(_) => return Err(EngineError::new(EngineErrorCode::Store)),
        };
        for (offset, kind) in dispatch_kinds.into_iter().enumerate() {
            self.emit_semantic(
                current.run().id(),
                first_sequence + u64::try_from(offset).unwrap_or(0),
                kind,
            )
            .await;
        }
        if self
            .crash
            .should_crash(EngineCrashPoint::AfterDispatchCommitBeforeExecutor)
        {
            return Err(EngineError::new(EngineErrorCode::CrashInjected));
        }
        let renewed = self.renew(owner_id, lease).await?;
        let attempt = CapabilityAttempt::new(&invocation, 1)
            .map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
        let context = CapabilityExecutionContext::for_attempt(invocation.clone(), attempt)
            .map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
        let (result, renewed) = self
            .await_with_lease_heartbeat(owner_id, renewed, self.capabilities.execute(context))
            .await?;
        let result = result.map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
        if self
            .crash
            .should_crash(EngineCrashPoint::AfterExecutorBeforeResultCommit)
        {
            return Err(EngineError::new(EngineErrorCode::CrashInjected));
        }
        self.commit_capability_result(
            owner_id,
            definition,
            prepared.stored_run().clone(),
            renewed,
            prepared.checkpoint_version(),
            dispatch_checkpoint,
            invocation,
            tool_call,
            manifest,
            1,
            step,
            result,
        )
        .await
        .map(CapabilityStepOutcome::committed)
    }

    #[allow(clippy::too_many_arguments)]
    async fn recover_checkpointed_capability(
        &self,
        owner_id: Uuid,
        definition: &AgentDefinition,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        checkpoint: CheckpointV1,
        invocation: LogicalInvocation,
        tool_call: ToolCall,
        manifest: CapabilityManifest,
        attempt_number: u32,
    ) -> Result<CapabilityStepOutcome, EngineError> {
        let lease = self.renew(owner_id, lease).await?;
        let attempt = CapabilityAttempt::new(&invocation, attempt_number)
            .map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
        let context = CapabilityExecutionContext::for_attempt(invocation.clone(), attempt)
            .map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
        let (recovery, lease) = self
            .await_with_lease_heartbeat(
                owner_id,
                lease,
                self.capabilities.recover_dispatched(context),
            )
            .await?;
        let recovery = recovery.map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
        let step = ExecutionStep::new(
            current.run().id(),
            tool_call.id.clone(),
            StepKind::Capability,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        match recovery {
            RecoveryAction::Completed(durable) => self
                .commit_capability_result(
                    owner_id,
                    definition,
                    current,
                    lease,
                    checkpoint_version,
                    checkpoint,
                    invocation,
                    tool_call,
                    manifest,
                    attempt_number,
                    step,
                    EngineCapabilityResult {
                        output: CapabilityResult::new(serde_json::json!({
                            "durable_result_ref": durable.result_ref().handle()
                        })),
                        durable,
                    },
                )
                .await
                .map(CapabilityStepOutcome::committed),
            RecoveryAction::RetrySameKey { authorization, .. } => {
                let retry_attempt_number = authorization.resume_binding().retry_attempt_number();
                if retry_attempt_number != attempt_number.saturating_add(1) {
                    return Err(EngineError::new(EngineErrorCode::Capability));
                }
                let request = EnginePolicyRequest {
                    owner_id,
                    definition: definition.clone(),
                    manifest: manifest.clone(),
                    invocation: invocation.clone(),
                    pending_approval: current.run().pending_approval().cloned(),
                };
                let current_policy = self.policy.resolve(request).await?;
                let evaluation = PolicyEngine::evaluate_with_approval(
                    current_policy.context(),
                    current_policy.grants(),
                    current_policy.approval(),
                )
                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
                if !policy_context_matches(current_policy.context(), definition, &invocation)
                    || !matches!(evaluation.decision, PolicyDecision::Allow(_))
                {
                    return self
                        .commit_policy_denied(
                            owner_id,
                            definition,
                            current,
                            lease,
                            checkpoint_version,
                            checkpoint,
                            invocation,
                        )
                        .await
                        .map(CapabilityStepOutcome::committed);
                }
                let policy_guard = DispatchPolicyGuard::from_current_policy(
                    owner_id,
                    current_policy.context(),
                    current_policy.grants(),
                    current_policy.approval(),
                    &evaluation,
                )
                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
                let dispatch_grant = DispatchGrantMutation::from_current_policy(
                    owner_id,
                    current_policy.context(),
                    current_policy.grants(),
                    &evaluation,
                )
                .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
                let manifest_pin = ManifestPin::from_manifest(&manifest)
                    .map_err(|_| EngineError::new(EngineErrorCode::ManifestUnavailable))?;
                let recovery_pause = crate::RecoveryPauseRecord::new(
                    invocation.binding(),
                    attempt_number,
                    manifest_pin.clone(),
                    crate::RecoveryPauseReason::AuthoritativeAbsence,
                )
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
                let recovery_record =
                    crate::RecoveryRecord::new_with_pause(recovery_pause, None)
                        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
                let uncertain = InvocationAttemptRecord::new_durable(
                    &invocation,
                    attempt_number,
                    crate::AttemptRecordState::Uncertain,
                    manifest_pin.clone(),
                    manifest.recovery_mode,
                )
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
                let dispatching = InvocationAttemptRecord::new_durable(
                    &invocation,
                    retry_attempt_number,
                    crate::AttemptRecordState::Dispatching,
                    manifest_pin,
                    manifest.recovery_mode,
                )
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
                let first_sequence = checkpoint.last_durable_event_sequence() + 1;
                let kinds = [
                    RuntimeEventKind::CapabilityFailed,
                    RuntimeEventKind::PolicyEvaluated,
                    RuntimeEventKind::InvocationDispatchPrepared,
                ];
                let events = kinds
                    .into_iter()
                    .enumerate()
                    .map(|(offset, kind)| {
                        self.event(
                            owner_id,
                            current.run(),
                            first_sequence + u64::try_from(offset).unwrap_or(0),
                            kind,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let dispatch_sequence = first_sequence + 2;
                let mut attempts = checkpoint.attempts().to_vec();
                let existing = attempts
                    .iter_mut()
                    .find(|attempt| {
                        attempt.invocation().id() == invocation.id()
                            && attempt.attempt_number() == attempt_number
                    })
                    .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
                *existing = uncertain.clone();
                attempts.push(dispatching.clone());
                let mut uncertain_invocations = checkpoint.uncertain_invocations().to_vec();
                if !uncertain_invocations.iter().any(|record| {
                    record.invocation().id() == invocation.id()
                        && record.attempt_number() == attempt_number
                }) {
                    uncertain_invocations.push(recovery_record.clone());
                }
                let retry_checkpoint = CheckpointV1Builder::new(
                    checkpoint.session_id(),
                    checkpoint.run_id(),
                    checkpoint.definition().clone(),
                    dispatch_sequence,
                    checkpoint.manifests().to_vec(),
                    checkpoint.budget().clone(),
                    checkpoint.usage().clone(),
                )
                .state(RunState::Running, None)
                .attempts(attempts)
                .completed_invocations(checkpoint.completed_invocations().to_vec())
                .uncertain_invocations(uncertain_invocations)
                .pending_approval(None)
                .message_context_refs(checkpoint.message_context_refs().to_vec())
                .model_context_refs(checkpoint.model_context_refs().to_vec())
                .memory_refs(checkpoint.memory_refs().to_vec())
                .artifact_refs(checkpoint.artifact_refs().to_vec())
                .cursor(Some(
                    CheckpointCursor::new(
                        invocation.id(),
                        retry_attempt_number,
                        tool_call.id.clone(),
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                ))
                .build()
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
                let mut commit = crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease.clone(),
                    RuntimeCommand::prepare_recovery_dispatch(
                        deterministic_id(
                            current.run().id(),
                            "recovery-dispatch",
                            dispatch_sequence,
                        ),
                        current.run().session_id(),
                        current.run().id(),
                        recovery_record,
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    events,
                    vec![],
                    vec![uncertain, dispatching],
                    vec![],
                    None,
                    current.run().clone(),
                )
                .with_checkpoint(retry_checkpoint.clone())
                .with_policy_guard(policy_guard);
                if let Some(dispatch_grant) = dispatch_grant {
                    commit = commit.with_dispatch_grant(dispatch_grant);
                }
                let prepared = match self.store.commit_execution(owner_id, commit).await {
                    Ok(prepared) => prepared,
                    Err(error)
                        if matches!(
                            error.code(),
                            crate::ExecutionStoreErrorCode::PolicyConflict
                                | crate::ExecutionStoreErrorCode::GrantConflict
                                | crate::ExecutionStoreErrorCode::GrantAlreadyConsumed
                        ) =>
                    {
                        return self
                            .commit_policy_denied(
                                owner_id,
                                definition,
                                current,
                                lease,
                                checkpoint_version,
                                checkpoint,
                                invocation,
                            )
                            .await
                            .map(CapabilityStepOutcome::committed)
                    }
                    Err(error)
                        if matches!(
                            error.code(),
                            crate::ExecutionStoreErrorCode::VersionConflict
                                | crate::ExecutionStoreErrorCode::CheckpointConflict
                        ) =>
                    {
                        return self
                            .adopt_dispatch_conflict(owner_id, definition, current.run().id())
                            .await
                    }
                    Err(_) => return Err(EngineError::new(EngineErrorCode::Store)),
                };
                for (offset, kind) in kinds.into_iter().enumerate() {
                    self.emit_semantic(
                        current.run().id(),
                        first_sequence + u64::try_from(offset).unwrap_or(0),
                        kind,
                    )
                    .await;
                }
                if self
                    .crash
                    .should_crash(EngineCrashPoint::AfterDispatchCommitBeforeExecutor)
                {
                    return Err(EngineError::new(EngineErrorCode::CrashInjected));
                }
                let lease = self.renew(owner_id, lease).await?;
                let retry = CapabilityAttempt::new(&invocation, retry_attempt_number)
                    .map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
                let retry_context =
                    CapabilityExecutionContext::for_attempt(invocation.clone(), retry)
                        .map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
                let (result, lease) = self
                    .await_with_lease_heartbeat(
                        owner_id,
                        lease,
                        self.capabilities
                            .execute_retry(retry_context, authorization),
                    )
                    .await?;
                let result = result.map_err(|_| EngineError::new(EngineErrorCode::Capability))?;
                if self
                    .crash
                    .should_crash(EngineCrashPoint::AfterExecutorBeforeResultCommit)
                {
                    return Err(EngineError::new(EngineErrorCode::CrashInjected));
                }
                self.commit_capability_result(
                    owner_id,
                    definition,
                    prepared.stored_run().clone(),
                    lease,
                    prepared.checkpoint_version(),
                    retry_checkpoint,
                    invocation,
                    tool_call,
                    manifest,
                    retry_attempt_number,
                    step,
                    result,
                )
                .await
                .map(CapabilityStepOutcome::committed)
            }
            other => self
                .commit_recovery_required(
                    owner_id,
                    definition,
                    current,
                    lease,
                    checkpoint_version,
                    checkpoint,
                    invocation,
                    tool_call,
                    manifest,
                    attempt_number,
                    other,
                )
                .await
                .map(CapabilityStepOutcome::committed),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_recovery_required(
        &self,
        owner_id: Uuid,
        definition: &AgentDefinition,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        checkpoint: CheckpointV1,
        invocation: LogicalInvocation,
        tool_call: ToolCall,
        manifest: CapabilityManifest,
        attempt_number: u32,
        action: RecoveryAction,
    ) -> Result<
        (
            crate::StoredRun,
            crate::ExecutionLease,
            u64,
            CheckpointV1,
            Message,
        ),
        EngineError,
    > {
        let reason = match action {
            RecoveryAction::Pending => crate::RecoveryPauseReason::ReconciliationPending,
            RecoveryAction::AuthoritativeAbsence | RecoveryAction::RetrySameKey { .. } => {
                crate::RecoveryPauseReason::AuthoritativeAbsence
            }
            RecoveryAction::CompensationRequired | RecoveryAction::RecoveryRequired => {
                crate::RecoveryPauseReason::ManualReview
            }
            RecoveryAction::Completed(_) => {
                return Err(EngineError::new(EngineErrorCode::InvalidState))
            }
        };
        let manifest_pin = ManifestPin::from_manifest(&manifest)
            .map_err(|_| EngineError::new(EngineErrorCode::ManifestUnavailable))?;
        let pause = crate::RecoveryPauseRecord::new(
            invocation.binding(),
            attempt_number,
            manifest_pin.clone(),
            reason,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let recovery = crate::RecoveryRecord::new_with_pause(pause.clone(), None)
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let uncertain = InvocationAttemptRecord::new_durable(
            &invocation,
            attempt_number,
            crate::AttemptRecordState::Uncertain,
            manifest_pin,
            manifest.recovery_mode,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let target = current
            .run()
            .require_recovery(recovery.clone())
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let sequence = checkpoint.last_durable_event_sequence() + 1;
        let event = self.event(
            owner_id,
            current.run(),
            sequence,
            RuntimeEventKind::CapabilityFailed,
        )?;
        let mut attempts = checkpoint.attempts().to_vec();
        let existing = attempts
            .iter_mut()
            .find(|attempt| {
                attempt.invocation().id() == invocation.id()
                    && attempt.attempt_number() == attempt_number
            })
            .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
        *existing = uncertain.clone();
        let mut uncertain_invocations = checkpoint.uncertain_invocations().to_vec();
        uncertain_invocations.push(recovery.clone());
        let recovery_checkpoint = CheckpointV1Builder::new(
            checkpoint.session_id(),
            checkpoint.run_id(),
            checkpoint.definition().clone(),
            sequence,
            checkpoint.manifests().to_vec(),
            checkpoint.budget().clone(),
            checkpoint.usage().clone(),
        )
        .state(RunState::RecoveryRequired, None)
        .attempts(attempts)
        .completed_invocations(checkpoint.completed_invocations().to_vec())
        .uncertain_invocations(uncertain_invocations)
        .message_context_refs(checkpoint.message_context_refs().to_vec())
        .model_context_refs(checkpoint.model_context_refs().to_vec())
        .memory_refs(checkpoint.memory_refs().to_vec())
        .artifact_refs(checkpoint.artifact_refs().to_vec())
        .cursor(Some(
            CheckpointCursor::new(invocation.id(), attempt_number, tool_call.id.clone())
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
        ))
        .build()
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let committed = self
            .store
            .commit_execution(
                owner_id,
                crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease.clone(),
                    RuntimeCommand::require_recovery(
                        deterministic_id(current.run().id(), "recovery", sequence),
                        current.run().session_id(),
                        current.run().id(),
                        recovery,
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    vec![event],
                    vec![],
                    vec![uncertain],
                    vec![],
                    None,
                    target,
                )
                .with_checkpoint(recovery_checkpoint.clone()),
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        self.emit_semantic(
            current.run().id(),
            sequence,
            RuntimeEventKind::CapabilityFailed,
        )
        .await;
        let renewed = self.renew(owner_id, lease).await?;
        Ok((
            committed.stored_run().clone(),
            renewed,
            committed.checkpoint_version(),
            recovery_checkpoint,
            Message {
                id: format!("recovery:{}", invocation.id()),
                agent_id: definition.id.clone(),
                room_id: current.run().session_id().to_string(),
                content: Content::default(),
                role: MessageRole::Tool,
                created_at_ms: self.clock.now_ms(),
            },
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_policy_denied(
        &self,
        owner_id: Uuid,
        definition: &AgentDefinition,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        checkpoint: CheckpointV1,
        invocation: LogicalInvocation,
    ) -> Result<
        (
            crate::StoredRun,
            crate::ExecutionLease,
            u64,
            CheckpointV1,
            Message,
        ),
        EngineError,
    > {
        let target = current
            .run()
            .transition(RunState::Paused, Some(crate::RunPauseReason::PolicyDenied))
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let first_sequence = checkpoint.last_durable_event_sequence() + 1;
        let kinds = if current.run().state() == RunState::WaitingForApproval {
            [
                RuntimeEventKind::ApprovalResolved,
                RuntimeEventKind::CapabilityDenied,
                RuntimeEventKind::RunPaused,
            ]
        } else {
            [
                RuntimeEventKind::PolicyEvaluated,
                RuntimeEventKind::CapabilityDenied,
                RuntimeEventKind::RunPaused,
            ]
        };
        let events = kinds
            .into_iter()
            .enumerate()
            .map(|(offset, kind)| {
                self.event(
                    owner_id,
                    current.run(),
                    first_sequence + u64::try_from(offset).unwrap_or(0),
                    kind,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let last_sequence = first_sequence + 2;
        let mut attempts = checkpoint.attempts().to_vec();
        let mut uncertain_invocations = checkpoint.uncertain_invocations().to_vec();
        let mut attempt_mutations = vec![];
        let mut denial_recovery = None;
        if let Some(existing) = attempts.iter_mut().find(|attempt| {
            attempt.invocation().id() == invocation.id()
                && attempt.state() == crate::AttemptRecordState::Dispatching
        }) {
            let durable_invocation = existing
                .durable_invocation()
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?
                .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
            let uncertain = InvocationAttemptRecord::new_durable(
                &durable_invocation,
                existing.attempt_number(),
                crate::AttemptRecordState::Uncertain,
                existing.manifest().clone(),
                existing.recovery_mode(),
            )
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let pause = crate::RecoveryPauseRecord::new(
                uncertain.invocation().clone(),
                uncertain.attempt_number(),
                uncertain.manifest().clone(),
                crate::RecoveryPauseReason::AuthoritativeAbsence,
            )
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            let recovery = crate::RecoveryRecord::new_with_pause(pause, None)
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
            *existing = uncertain.clone();
            attempt_mutations.push(uncertain);
            if !uncertain_invocations.iter().any(|record| {
                record.invocation().id() == recovery.invocation().id()
                    && record.attempt_number() == recovery.attempt_number()
            }) {
                uncertain_invocations.push(recovery.clone());
            }
            denial_recovery = Some(recovery);
        }
        let denied_checkpoint = CheckpointV1Builder::new(
            checkpoint.session_id(),
            checkpoint.run_id(),
            checkpoint.definition().clone(),
            last_sequence,
            checkpoint.manifests().to_vec(),
            checkpoint.budget().clone(),
            checkpoint.usage().clone(),
        )
        .state(RunState::Paused, Some(crate::RunPauseReason::PolicyDenied))
        .attempts(attempts)
        .completed_invocations(checkpoint.completed_invocations().to_vec())
        .uncertain_invocations(uncertain_invocations)
        .pending_approval(None)
        .message_context_refs(checkpoint.message_context_refs().to_vec())
        .model_context_refs(checkpoint.model_context_refs().to_vec())
        .memory_refs(checkpoint.memory_refs().to_vec())
        .artifact_refs(checkpoint.artifact_refs().to_vec())
        .cursor(checkpoint.cursor().cloned())
        .build()
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let command = if let Some(recovery) = denial_recovery {
            RuntimeCommand::pause_with_recovery(
                deterministic_id(current.run().id(), "policy-denied", last_sequence),
                current.run().session_id(),
                current.run().id(),
                crate::RunPauseReason::PolicyDenied,
                recovery,
            )
        } else {
            RuntimeCommand::pause(
                deterministic_id(current.run().id(), "policy-denied", last_sequence),
                current.run().session_id(),
                current.run().id(),
                crate::RunPauseReason::PolicyDenied,
            )
        }
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let committed = self
            .store
            .commit_execution(
                owner_id,
                crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease.clone(),
                    command,
                    events,
                    vec![],
                    attempt_mutations,
                    vec![],
                    None,
                    target,
                )
                .with_checkpoint(denied_checkpoint.clone()),
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        for (offset, kind) in kinds.into_iter().enumerate() {
            self.emit_semantic(
                current.run().id(),
                first_sequence + u64::try_from(offset).unwrap_or(0),
                kind,
            )
            .await;
        }
        let renewed = self.renew(owner_id, lease).await?;
        Ok((
            committed.stored_run().clone(),
            renewed,
            committed.checkpoint_version(),
            denied_checkpoint,
            Message {
                id: format!("denied:{}", invocation.id()),
                agent_id: definition.id.clone(),
                room_id: current.run().session_id().to_string(),
                content: Content::default(),
                role: MessageRole::Tool,
                created_at_ms: self.clock.now_ms(),
            },
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_capability_result(
        &self,
        owner_id: Uuid,
        definition: &AgentDefinition,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        dispatch_checkpoint: CheckpointV1,
        invocation: LogicalInvocation,
        tool_call: ToolCall,
        manifest: CapabilityManifest,
        attempt_number: u32,
        step: ExecutionStep,
        result: EngineCapabilityResult,
    ) -> Result<
        (
            crate::StoredRun,
            crate::ExecutionLease,
            u64,
            CheckpointV1,
            Message,
        ),
        EngineError,
    > {
        let manifest_pin = ManifestPin::from_manifest(&manifest)
            .map_err(|_| EngineError::new(EngineErrorCode::ManifestUnavailable))?;
        let completed_attempt = InvocationAttemptRecord::new_durable(
            &invocation,
            attempt_number,
            crate::AttemptRecordState::Completed,
            manifest_pin.clone(),
            manifest.recovery_mode,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let completed = CompletedInvocationRecord::new(
            invocation.binding(),
            attempt_number,
            manifest_pin,
            manifest.recovery_mode,
            OpaqueReference::new(result.durable.result_ref().handle())
                .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let first_completed_sequence = dispatch_checkpoint.last_durable_event_sequence() + 1;
        let completed_events = [
            RuntimeEventKind::CapabilityCompleted,
            RuntimeEventKind::StepCompleted,
        ]
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| {
            self.event(
                owner_id,
                current.run(),
                first_completed_sequence + u64::try_from(offset).unwrap_or(0),
                kind,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
        let completed_sequence = first_completed_sequence + 1;
        let mut completed_attempts = dispatch_checkpoint.attempts().to_vec();
        let stored_attempt = completed_attempts
            .iter_mut()
            .find(|attempt| {
                attempt.invocation().id() == invocation.id()
                    && attempt.attempt_number() == attempt_number
            })
            .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
        *stored_attempt = completed_attempt.clone();
        let mut completed_invocations = dispatch_checkpoint.completed_invocations().to_vec();
        completed_invocations.push(completed.clone());
        let usage = dispatch_checkpoint
            .usage()
            .checked_add(&Usage {
                capability_steps: 1,
                ..Usage::default()
            })
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let completed_checkpoint = self.rebuild_checkpoint_with_history(
            &dispatch_checkpoint,
            completed_sequence,
            usage,
            completed_attempts,
            completed_invocations,
            Some(
                CheckpointCursor::new(invocation.id(), attempt_number, tool_call.id.clone())
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
            ),
        )?;
        let recorded = self
            .store
            .commit_execution(
                owner_id,
                crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease.clone(),
                    RuntimeCommand::record_progress(
                        deterministic_id(current.run().id(), "result", completed_sequence),
                        current.run().session_id(),
                        current.run().id(),
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    completed_events,
                    vec![step],
                    vec![completed_attempt],
                    vec![DurableResultMutation::new(completed, result.durable)],
                    None,
                    current.run().clone(),
                )
                .with_checkpoint(completed_checkpoint.clone()),
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        for (offset, kind) in [
            RuntimeEventKind::CapabilityCompleted,
            RuntimeEventKind::StepCompleted,
        ]
        .into_iter()
        .enumerate()
        {
            self.emit_semantic(
                current.run().id(),
                first_completed_sequence + u64::try_from(offset).unwrap_or(0),
                kind,
            )
            .await;
        }
        let renewed = self.renew(owner_id, lease).await?;
        let message = Message {
            id: format!("tool:{}", invocation.id()),
            agent_id: definition.id.clone(),
            room_id: current.run().session_id().to_string(),
            content: Content {
                text: serde_json::to_string(&result.output.output)
                    .map_err(|_| EngineError::new(EngineErrorCode::Capability))?,
                attachments: None,
                metadata: None,
            },
            role: MessageRole::Tool,
            created_at_ms: self.clock.now_ms(),
        };
        Ok((
            recorded.stored_run().clone(),
            renewed,
            recorded.checkpoint_version(),
            completed_checkpoint,
            message,
        ))
    }

    async fn adopt_dispatch_conflict(
        &self,
        owner_id: Uuid,
        definition: &AgentDefinition,
        run_id: Uuid,
    ) -> Result<CapabilityStepOutcome, EngineError> {
        let stored = self
            .store
            .load_run(owner_id, run_id)
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?
            .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
        let (_, checkpoint) = self
            .store
            .load_checkpoint(owner_id, run_id)
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?
            .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
        let expected_definition = DefinitionPin::from_definition(definition)
            .map_err(|_| EngineError::new(EngineErrorCode::DefinitionUnavailable))?;
        if stored.owner_id() != owner_id
            || stored.run().id() != run_id
            || stored.run().definition_id() != expected_definition.id()
            || stored.run().definition_version() != expected_definition.version()
            || checkpoint.run_id() != run_id
            || checkpoint.session_id() != stored.run().session_id()
            || checkpoint.definition() != &expected_definition
            || checkpoint.state() != stored.run().state()
            || checkpoint.pause_reason() != stored.run().pause_reason()
        {
            return Err(EngineError::new(EngineErrorCode::InvalidState));
        }
        if stored.run().state() != RunState::Paused
            || stored.run().pause_reason() != Some(crate::RunPauseReason::PolicyDenied)
        {
            return Err(EngineError::new(EngineErrorCode::InvalidState));
        }
        Ok(CapabilityStepOutcome::Adopted(EngineRunOutcome::Denied))
    }

    async fn await_with_lease_heartbeat<T, F>(
        &self,
        owner_id: Uuid,
        lease: crate::ExecutionLease,
        operation: F,
    ) -> Result<(T, crate::ExecutionLease), EngineError>
    where
        F: Future<Output = T>,
    {
        let lease = Arc::new(AsyncMutex::new(lease));
        let heartbeat_lease = lease.clone();
        let engine = self;
        let heartbeat = async move {
            loop {
                let current = heartbeat_lease.lock().await.clone();
                let renew_at_ms = current
                    .expires_at_ms()
                    .saturating_sub(engine.config.lease_duration_ms.saturating_div(2).max(1));
                engine.clock.wait_until_ms(renew_at_ms).await;
                let current = heartbeat_lease.lock().await.clone();
                let renewed = engine.renew(owner_id, current).await?;
                *heartbeat_lease.lock().await = renewed;
            }
            #[allow(unreachable_code)]
            Ok::<(), EngineError>(())
        };
        match select(Box::pin(operation), Box::pin(heartbeat)).await {
            Either::Left((output, _heartbeat)) => {
                let latest = lease.lock().await.clone();
                Ok((output, latest))
            }
            Either::Right((heartbeat_result, _operation)) => {
                heartbeat_result?;
                Err(EngineError::new(EngineErrorCode::Store))
            }
        }
    }

    async fn renew(
        &self,
        owner_id: Uuid,
        lease: crate::ExecutionLease,
    ) -> Result<crate::ExecutionLease, EngineError> {
        self.store
            .renew_lease(owner_id, lease, self.config.lease_duration_ms)
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))
    }

    fn event(
        &self,
        owner_id: Uuid,
        run: &Run,
        sequence: u64,
        kind: RuntimeEventKind,
    ) -> Result<RuntimeEvent, EngineError> {
        RuntimeEvent::new(
            deterministic_id(run.id(), "event", sequence),
            owner_id,
            run.session_id(),
            run.id(),
            self.clock.now_ms(),
            sequence,
            kind,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))
    }

    fn rebuild_checkpoint(
        &self,
        checkpoint: &CheckpointV1,
        sequence: u64,
        state: RunState,
        usage: Usage,
    ) -> Result<CheckpointV1, EngineError> {
        self.rebuild_checkpoint_for_state(checkpoint, sequence, state, None, usage)
    }

    fn rebuild_checkpoint_for_state(
        &self,
        checkpoint: &CheckpointV1,
        sequence: u64,
        state: RunState,
        pause_reason: Option<crate::RunPauseReason>,
        usage: Usage,
    ) -> Result<CheckpointV1, EngineError> {
        CheckpointV1Builder::new(
            checkpoint.session_id(),
            checkpoint.run_id(),
            checkpoint.definition().clone(),
            sequence,
            checkpoint.manifests().to_vec(),
            checkpoint.budget().clone(),
            usage,
        )
        .state(state, pause_reason)
        .attempts(checkpoint.attempts().to_vec())
        .cursor(checkpoint.cursor().cloned())
        .completed_invocations(checkpoint.completed_invocations().to_vec())
        .uncertain_invocations(checkpoint.uncertain_invocations().to_vec())
        .pending_approval(checkpoint.pending_approval().cloned())
        .message_context_refs(checkpoint.message_context_refs().to_vec())
        .model_context_refs(checkpoint.model_context_refs().to_vec())
        .memory_refs(checkpoint.memory_refs().to_vec())
        .artifact_refs(checkpoint.artifact_refs().to_vec())
        .build()
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))
    }

    #[allow(clippy::too_many_arguments)]
    fn rebuild_checkpoint_with_history(
        &self,
        checkpoint: &CheckpointV1,
        sequence: u64,
        usage: Usage,
        attempts: Vec<InvocationAttemptRecord>,
        completed_invocations: Vec<CompletedInvocationRecord>,
        cursor: Option<CheckpointCursor>,
    ) -> Result<CheckpointV1, EngineError> {
        self.rebuild_checkpoint_with_history_and_pending(
            checkpoint,
            sequence,
            usage,
            attempts,
            completed_invocations,
            cursor,
            checkpoint.pending_approval().cloned(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn rebuild_checkpoint_with_history_and_pending(
        &self,
        checkpoint: &CheckpointV1,
        sequence: u64,
        usage: Usage,
        attempts: Vec<InvocationAttemptRecord>,
        completed_invocations: Vec<CompletedInvocationRecord>,
        cursor: Option<CheckpointCursor>,
        pending_approval: Option<PendingApprovalRecord>,
    ) -> Result<CheckpointV1, EngineError> {
        CheckpointV1Builder::new(
            checkpoint.session_id(),
            checkpoint.run_id(),
            checkpoint.definition().clone(),
            sequence,
            checkpoint.manifests().to_vec(),
            checkpoint.budget().clone(),
            usage,
        )
        .state(RunState::Running, None)
        .attempts(attempts)
        .completed_invocations(completed_invocations)
        .uncertain_invocations(checkpoint.uncertain_invocations().to_vec())
        .pending_approval(pending_approval)
        .message_context_refs(checkpoint.message_context_refs().to_vec())
        .model_context_refs(checkpoint.model_context_refs().to_vec())
        .memory_refs(checkpoint.memory_refs().to_vec())
        .artifact_refs(checkpoint.artifact_refs().to_vec())
        .cursor(cursor)
        .build()
        .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))
    }

    async fn emit_semantic(&self, run_id: Uuid, sequence: u64, kind: RuntimeEventKind) {
        let _ = self
            .live
            .emit(EngineLiveEvent::Semantic {
                run_id,
                sequence,
                kind,
            })
            .await;
    }

    async fn pause(
        &self,
        owner_id: Uuid,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        checkpoint: &CheckpointV1,
    ) -> Result<EngineRunOutcome, EngineError> {
        let target = current
            .run()
            .transition(RunState::Paused, Some(crate::RunPauseReason::Requested))
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let sequence = checkpoint.last_durable_event_sequence() + 1;
        let event = self.event(
            owner_id,
            current.run(),
            sequence,
            RuntimeEventKind::RunPaused,
        )?;
        let paused_checkpoint = self.rebuild_checkpoint_for_state(
            checkpoint,
            sequence,
            RunState::Paused,
            Some(crate::RunPauseReason::Requested),
            checkpoint.usage().clone(),
        )?;
        self.store
            .commit_execution(
                owner_id,
                crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease,
                    RuntimeCommand::pause(
                        deterministic_id(current.run().id(), "pause", sequence),
                        current.run().session_id(),
                        current.run().id(),
                        crate::RunPauseReason::Requested,
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    vec![event],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    target,
                )
                .with_checkpoint(paused_checkpoint),
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        self.emit_semantic(current.run().id(), sequence, RuntimeEventKind::RunPaused)
            .await;
        Ok(EngineRunOutcome::PausedByRequest)
    }

    async fn cancel(
        &self,
        owner_id: Uuid,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        checkpoint: &CheckpointV1,
    ) -> Result<EngineRunOutcome, EngineError> {
        let target = current
            .run()
            .transition(RunState::Cancelled, None)
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let sequence = checkpoint.last_durable_event_sequence() + 1;
        let event = self.event(
            owner_id,
            current.run(),
            sequence,
            RuntimeEventKind::RunCancelled,
        )?;
        self.store
            .commit_execution(
                owner_id,
                crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease,
                    RuntimeCommand::cancel(
                        deterministic_id(current.run().id(), "cancel", sequence),
                        current.run().session_id(),
                        current.run().id(),
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    vec![event],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    target,
                ),
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        self.emit_semantic(current.run().id(), sequence, RuntimeEventKind::RunCancelled)
            .await;
        Ok(EngineRunOutcome::Cancelled)
    }

    async fn pause_for_budget(
        &self,
        owner_id: Uuid,
        current: crate::StoredRun,
        lease: crate::ExecutionLease,
        checkpoint_version: u64,
        checkpoint: &CheckpointV1,
    ) -> Result<EngineRunOutcome, EngineError> {
        let target = current
            .run()
            .transition(RunState::Paused, Some(crate::RunPauseReason::Budget))
            .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?;
        let first_sequence = checkpoint.last_durable_event_sequence() + 1;
        let events = [
            RuntimeEventKind::BudgetExhausted,
            RuntimeEventKind::RunPaused,
        ]
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| {
            self.event(
                owner_id,
                current.run(),
                first_sequence + u64::try_from(offset).unwrap_or(0),
                kind,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
        let last_sequence = first_sequence + 1;
        let paused_checkpoint = self.rebuild_checkpoint_for_state(
            checkpoint,
            last_sequence,
            RunState::Paused,
            Some(crate::RunPauseReason::Budget),
            checkpoint.usage().clone(),
        )?;
        self.store
            .commit_execution(
                owner_id,
                crate::ExecutionCommit::new(
                    current.run_version(),
                    checkpoint_version,
                    lease,
                    RuntimeCommand::pause(
                        deterministic_id(current.run().id(), "budget", last_sequence),
                        current.run().session_id(),
                        current.run().id(),
                        crate::RunPauseReason::Budget,
                    )
                    .map_err(|_| EngineError::new(EngineErrorCode::InvalidState))?,
                    events,
                    vec![],
                    vec![],
                    vec![],
                    None,
                    target,
                )
                .with_checkpoint(paused_checkpoint),
            )
            .await
            .map_err(|_| EngineError::new(EngineErrorCode::Store))?;
        self.emit_semantic(
            current.run().id(),
            first_sequence,
            RuntimeEventKind::BudgetExhausted,
        )
        .await;
        self.emit_semantic(
            current.run().id(),
            last_sequence,
            RuntimeEventKind::RunPaused,
        )
        .await;
        Ok(EngineRunOutcome::PausedForBudget)
    }
}

struct EngineModelSink<'a, L> {
    run_id: Uuid,
    live: &'a L,
    final_response: Mutex<Option<ModelGenerateResponse>>,
}

#[async_trait]
impl<L: EngineLiveEventSink> ModelStreamSink for EngineModelSink<'_, L> {
    async fn emit(&self, frame: ModelStreamFrame) -> Result<(), String> {
        match frame {
            ModelStreamFrame::TextDelta(text) => {
                let _ = self
                    .live
                    .emit(EngineLiveEvent::TextDelta {
                        run_id: self.run_id,
                        text,
                    })
                    .await;
            }
            ModelStreamFrame::Final(response) => {
                let mut final_response = self
                    .final_response
                    .lock()
                    .map_err(|_| "model stream sink unavailable".to_owned())?;
                if final_response.is_some() {
                    return Err("model adapter emitted multiple final frames".to_owned());
                }
                *final_response = Some(response);
            }
        }
        Ok(())
    }
}

fn deterministic_id(run_id: Uuid, label: &str, sequence: u64) -> Uuid {
    Uuid::new_v5(
        &ENGINE_ID_NAMESPACE,
        format!("{run_id}:{label}:{sequence}").as_bytes(),
    )
}

fn tool_call_from_invocation(invocation: &LogicalInvocation) -> Result<ToolCall, EngineError> {
    let arguments = invocation
        .normalized_arguments()
        .as_object()
        .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))?;
    let args = arguments
        .iter()
        .map(|(key, value)| {
            json_to_data_value(value)
                .map(|value| (key.clone(), value))
                .ok_or_else(|| EngineError::new(EngineErrorCode::InvalidState))
        })
        .collect::<Result<_, _>>()?;
    Ok(ToolCall {
        id: invocation.logical_step_id().to_owned(),
        name: invocation.capability_id().to_owned(),
        args,
    })
}

fn policy_context_matches(
    context: &PolicyContext,
    definition: &AgentDefinition,
    invocation: &LogicalInvocation,
) -> bool {
    context.agent_definition_id == definition.id
        && context.agent_definition_version == definition.version
        && context.run_id == invocation.run_id()
        && context.logical_step_id == invocation.logical_step_id()
        && context.logical_invocation_id == invocation.id()
        && context.capability_id == invocation.capability_id()
        && context.manifest_version == invocation.manifest_version()
        && context.canonical_argument_digest == invocation.canonical_argument_digest()
        && context.policy_revision == definition.approval_policy_revision
}

fn json_to_data_value(value: &serde_json::Value) -> Option<crate::DataValue> {
    match value {
        serde_json::Value::Null => Some(crate::DataValue::Null),
        serde_json::Value::Bool(value) => Some(crate::DataValue::Bool(*value)),
        serde_json::Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(crate::DataValue::Number),
        serde_json::Value::String(value) => Some(crate::DataValue::String(value.clone())),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_to_data_value)
            .collect::<Option<Vec<_>>>()
            .map(crate::DataValue::Array),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(key, value)| json_to_data_value(value).map(|value| (key.clone(), value)))
            .collect::<Option<std::collections::BTreeMap<_, _>>>()
            .map(crate::DataValue::Object),
    }
}

fn agent_config(definition: &AgentDefinition) -> AgentConfig {
    AgentConfig {
        name: definition.name.clone(),
        model: definition.model.model.clone(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some(definition.model.provider.clone()),
        system: Some(definition.system.clone()),
        tools: None,
        plugins: None,
        settings: None,
    }
}
