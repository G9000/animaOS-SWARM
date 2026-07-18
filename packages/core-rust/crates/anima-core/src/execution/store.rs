use async_trait::async_trait;
use std::fmt;

use uuid::Uuid;

use super::{
    ApprovalResumeClaim, Budget, CheckpointV1, CheckpointV1Builder, CommandReceipt,
    CompletedInvocationRecord, DefinitionPin, ExecutionError, ExecutionLease,
    InvocationAttemptRecord, Run, RunState, RuntimeCommand, RuntimeEvent, RuntimeEventKind,
    Session, SessionConcurrencyPolicy, Step, StepKind, Usage,
};
use crate::{
    AgentDefinition, ApprovalDecision, AutonomyGrant, CapabilityKind, CapabilityManifest,
    CapabilityReferenceId, DurableCapabilityResult, DurableCapabilityStatus, GrantConsumption,
    GrantEffect, GrantScope, GrantStatus, LifecyclePolicy, LogicalInvocation, ManifestPin,
    MemoryPolicy, ModelPolicy, OpaqueReference, PolicyContext, PolicyEngine, PolicyRestrictions,
    ProfileRef, RecoveryMode, RiskLevel, RuntimeCompatibility, RuntimeLimits,
};

/// Input for atomically creating a durable run and claiming its session when required.
#[derive(Clone, Debug)]
pub struct CreateRun {
    session: Session,
    run: Run,
    expected_session_version: u64,
    concurrency_policy: SessionConcurrencyPolicy,
}

impl CreateRun {
    pub fn new(
        session: Session,
        run: Run,
        expected_session_version: u64,
        concurrency_policy: SessionConcurrencyPolicy,
    ) -> Self {
        Self {
            session,
            run,
            expected_session_version,
            concurrency_policy,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn run(&self) -> &Run {
        &self.run
    }

    pub fn expected_session_version(&self) -> u64 {
        self.expected_session_version
    }

    pub fn concurrency_policy(&self) -> SessionConcurrencyPolicy {
        self.concurrency_policy
    }
}

/// A versioned durable run returned by an execution-store adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRun {
    run: Run,
    run_version: u64,
    session_version: u64,
}

/// A validated approval claim and its one-time grant consumption, committed together with a run.
#[derive(Clone, Debug)]
pub struct ApprovalGrantMutation {
    claim: ApprovalResumeClaim,
}

impl ApprovalGrantMutation {
    /// Preserves Task 4's validated approval claim and its exact consumption as one commit input.
    pub fn from_claim(claim: ApprovalResumeClaim) -> Self {
        Self { claim }
    }

    pub fn claim(&self) -> &ApprovalResumeClaim {
        &self.claim
    }

    pub fn grant_consumption(&self) -> Option<&GrantConsumption> {
        self.claim.grant_consumption()
    }

    pub fn remaining_uses(&self) -> Option<u32> {
        self.claim
            .grant_consumption_snapshot()
            .map(|snapshot| snapshot.remaining_uses())
    }
}

/// A durable result is keyed by a logical invocation and can only be recorded identically once.
#[derive(Clone, Debug)]
pub struct DurableResultMutation {
    completed: CompletedInvocationRecord,
    result: DurableCapabilityResult,
}

impl DurableResultMutation {
    pub fn new(completed: CompletedInvocationRecord, result: DurableCapabilityResult) -> Self {
        Self { completed, result }
    }

    pub fn completed(&self) -> &CompletedInvocationRecord {
        &self.completed
    }

    pub fn result(&self) -> &DurableCapabilityResult {
        &self.result
    }
}

/// All state mutations which must succeed or fail together for one leased execution command.
#[derive(Clone, Debug)]
pub struct ExecutionCommit {
    expected_run_version: u64,
    expected_checkpoint_version: u64,
    lease: ExecutionLease,
    command: RuntimeCommand,
    events: Vec<RuntimeEvent>,
    steps: Vec<Step>,
    attempts: Vec<InvocationAttemptRecord>,
    results: Vec<DurableResultMutation>,
    approval: Option<ApprovalGrantMutation>,
    checkpoint: Option<CheckpointV1>,
    target_run: Run,
}

impl ExecutionCommit {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        expected_run_version: u64,
        expected_checkpoint_version: u64,
        lease: ExecutionLease,
        command: RuntimeCommand,
        events: Vec<RuntimeEvent>,
        steps: Vec<Step>,
        attempts: Vec<InvocationAttemptRecord>,
        results: Vec<DurableResultMutation>,
        approval: Option<ApprovalGrantMutation>,
        target_run: Run,
    ) -> Self {
        Self {
            expected_run_version,
            expected_checkpoint_version,
            lease,
            command,
            events,
            steps,
            attempts,
            results,
            approval,
            checkpoint: None,
            target_run,
        }
    }

    pub fn with_checkpoint(mut self, checkpoint: CheckpointV1) -> Self {
        self.checkpoint = Some(checkpoint);
        self
    }

    pub fn expected_run_version(&self) -> u64 {
        self.expected_run_version
    }

    pub fn expected_checkpoint_version(&self) -> u64 {
        self.expected_checkpoint_version
    }

    pub fn lease(&self) -> &ExecutionLease {
        &self.lease
    }

    pub fn command(&self) -> &RuntimeCommand {
        &self.command
    }

    pub fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn attempts(&self) -> &[InvocationAttemptRecord] {
        &self.attempts
    }

    pub fn results(&self) -> &[DurableResultMutation] {
        &self.results
    }

    pub fn approval(&self) -> Option<&ApprovalGrantMutation> {
        self.approval.as_ref()
    }

    pub fn checkpoint(&self) -> Option<&CheckpointV1> {
        self.checkpoint.as_ref()
    }

    pub fn target_run(&self) -> &Run {
        &self.target_run
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionCommitOutcome {
    stored_run: StoredRun,
    receipt: CommandReceipt,
}

impl ExecutionCommitOutcome {
    pub fn new(stored_run: StoredRun, receipt: CommandReceipt) -> Self {
        Self {
            stored_run,
            receipt,
        }
    }

    pub fn stored_run(&self) -> &StoredRun {
        &self.stored_run
    }

    pub fn receipt(&self) -> &CommandReceipt {
        &self.receipt
    }
}

impl StoredRun {
    pub fn new(
        run: Run,
        run_version: u64,
        session_version: u64,
    ) -> Result<Self, ExecutionStoreError> {
        if run_version == 0 || session_version == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            run,
            run_version,
            session_version,
        })
    }

    pub fn run(&self) -> &Run {
        &self.run
    }

    pub fn run_version(&self) -> u64 {
        self.run_version
    }

    pub fn session_version(&self) -> u64 {
        self.session_version
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionStoreErrorCode {
    NotFound,
    VersionConflict,
    ActiveRunConflict,
    LeaseConflict,
    LeaseExpired,
    CommandConflict,
    EventConflict,
    CheckpointConflict,
    GrantAlreadyConsumed,
    ResultConflict,
    InvalidRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionStoreError {
    code: ExecutionStoreErrorCode,
}

impl ExecutionStoreError {
    pub const fn new(code: ExecutionStoreErrorCode) -> Self {
        Self { code }
    }

    pub const fn code(self) -> ExecutionStoreErrorCode {
        self.code
    }
}

impl From<ExecutionError> for ExecutionStoreError {
    fn from(_: ExecutionError) -> Self {
        Self::new(ExecutionStoreErrorCode::InvalidRequest)
    }
}

impl fmt::Display for ExecutionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            ExecutionStoreErrorCode::NotFound => "execution run was not found",
            ExecutionStoreErrorCode::VersionConflict => "execution version did not match",
            ExecutionStoreErrorCode::ActiveRunConflict => "session already has an active run",
            ExecutionStoreErrorCode::LeaseConflict => "execution lease is held by another worker",
            ExecutionStoreErrorCode::LeaseExpired => "execution lease is expired or fenced",
            ExecutionStoreErrorCode::CommandConflict => {
                "command identifier conflicts with its receipt"
            }
            ExecutionStoreErrorCode::EventConflict => "execution event sequence is not contiguous",
            ExecutionStoreErrorCode::CheckpointConflict => {
                "execution checkpoint did not match commit"
            }
            ExecutionStoreErrorCode::GrantAlreadyConsumed => "autonomy grant was already consumed",
            ExecutionStoreErrorCode::ResultConflict => "durable invocation result conflicts",
            ExecutionStoreErrorCode::InvalidRequest => "execution store request is invalid",
        })
    }
}

impl std::error::Error for ExecutionStoreError {}

/// Adapter port for durable execution state. Each operation is atomic.
#[async_trait]
pub trait ExecutionStore: Send + Sync {
    async fn create_run(&self, request: CreateRun) -> Result<StoredRun, ExecutionStoreError>;

    /// Acquires a new fence only if no current lease is active under adapter-authoritative time.
    async fn acquire_lease(
        &self,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError>;

    /// Renews precisely the supplied active fence; an expired lease is never resurrected.
    async fn renew_lease(
        &self,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError>;

    async fn commit_execution(
        &self,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError>;

    async fn load_run(&self, run_id: Uuid) -> Result<Option<StoredRun>, ExecutionStoreError>;

    async fn load_checkpoint(
        &self,
        run_id: Uuid,
    ) -> Result<Option<(u64, CheckpointV1)>, ExecutionStoreError>;

    async fn load_steps(&self, run_id: Uuid) -> Result<Vec<Step>, ExecutionStoreError>;

    async fn load_attempts(
        &self,
        run_id: Uuid,
    ) -> Result<Vec<InvocationAttemptRecord>, ExecutionStoreError>;

    async fn load_durable_result(
        &self,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError>;

    async fn replay_events(
        &self,
        run_id: Uuid,
        after_sequence: u64,
    ) -> Result<Vec<RuntimeEvent>, ExecutionStoreError>;
}

#[async_trait]
pub trait ExecutionStoreFactory: Send + Sync {
    type Store: ExecutionStore;

    async fn create_execution_store(&self) -> Result<Self::Store, ExecutionStoreError>;
}

/// Runs portable adapter checks using only the public execution-store port.
pub async fn assert_execution_store_conformance<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session_id = Uuid::from_u128(0xfeed);
    let session = Session::new(
        session_id,
        "execution-store-contract",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let run = Run::queued(
        Uuid::from_u128(0xfeed_1),
        session_id,
        "execution-store-contract",
        1,
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(CreateRun::new(
            session.clone(),
            run.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    let conflicting = Run::queued(
        Uuid::from_u128(0xfeed_2),
        session_id,
        "execution-store-contract",
        1,
    )
    .map_err(ExecutionStoreError::from)?;
    match store
        .create_run(CreateRun::new(
            session,
            conflicting,
            1,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
    {
        Err(error) if error.code() == ExecutionStoreErrorCode::ActiveRunConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    };
    let lease = store
        .acquire_lease(run.id(), created.run_version(), 1_000)
        .await?;
    match store
        .acquire_lease(run.id(), created.run_version(), 1_000)
        .await
    {
        Err(error) if error.code() == ExecutionStoreErrorCode::LeaseConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    let lease = store.renew_lease(lease, 1_000).await?;
    let running = run
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let command = RuntimeCommand::start(Uuid::from_u128(0xfeed_3), session_id, run.id())
        .map_err(ExecutionStoreError::from)?;
    let gap = vec![
        RuntimeEvent::new(
            Uuid::from_u128(0xfeed_4),
            Uuid::from_u128(0xfeed_5),
            session_id,
            run.id(),
            1,
            1,
            RuntimeEventKind::RunStarted,
        )
        .map_err(ExecutionStoreError::from)?,
        RuntimeEvent::new(
            Uuid::from_u128(0xfeed_8),
            Uuid::from_u128(0xfeed_5),
            session_id,
            run.id(),
            2,
            3,
            RuntimeEventKind::StepStarted,
        )
        .map_err(ExecutionStoreError::from)?,
    ];
    let gap_commit = ExecutionCommit::new(
        created.run_version(),
        0,
        lease.clone(),
        command.clone(),
        gap,
        vec![],
        vec![],
        vec![],
        None,
        running.clone(),
    );
    match store.commit_execution(gap_commit).await {
        Err(error) if error.code() == ExecutionStoreErrorCode::EventConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    let events = vec![
        RuntimeEvent::new(
            Uuid::from_u128(0xfeed_6),
            Uuid::from_u128(0xfeed_5),
            session_id,
            run.id(),
            1,
            1,
            RuntimeEventKind::RunStarted,
        )
        .map_err(ExecutionStoreError::from)?,
        RuntimeEvent::new(
            Uuid::from_u128(0xfeed_7),
            Uuid::from_u128(0xfeed_5),
            session_id,
            run.id(),
            2,
            2,
            RuntimeEventKind::StepStarted,
        )
        .map_err(ExecutionStoreError::from)?,
    ];
    let invocation = conformance_value(LogicalInvocation::new(
        run.id(),
        "execution-store-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "contract.txt"}),
    ))?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:execution-store-step",
        RecoveryMode::KeyedIdempotent,
    ))?;
    let attempt = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Pending,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let step = Step::new(run.id(), "execution-store-step", StepKind::Capability)
        .map_err(ExecutionStoreError::from)?;
    let checkpoint = CheckpointV1Builder::new(
        session_id,
        run.id(),
        DefinitionPin::new(1, "execution-store-contract", 1).map_err(ExecutionStoreError::from)?,
        2,
        vec![manifest],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .build()
    .map_err(ExecutionStoreError::from)?;
    let commit = ExecutionCommit::new(
        created.run_version(),
        0,
        lease.clone(),
        command,
        events.clone(),
        vec![step.clone()],
        vec![attempt.clone()],
        vec![],
        None,
        running.clone(),
    )
    .with_checkpoint(checkpoint.clone());
    let outcome = store.commit_execution(commit.clone()).await?;
    if store.replay_events(run.id(), 0).await? != events
        || store.load_checkpoint(run.id()).await? != Some((1, checkpoint))
        || store.load_steps(run.id()).await? != vec![step]
        || store.load_attempts(run.id()).await? != vec![attempt]
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let paused = running
        .transition(RunState::Paused, Some(super::RunPauseReason::Requested))
        .map_err(ExecutionStoreError::from)?;
    let paused_event = RuntimeEvent::new(
        Uuid::from_u128(0xfeed_b),
        Uuid::from_u128(0xfeed_5),
        session_id,
        run.id(),
        3,
        3,
        RuntimeEventKind::RunPaused,
    )
    .map_err(ExecutionStoreError::from)?;
    let stale_run = store
        .commit_execution(ExecutionCommit::new(
            created.run_version(),
            1,
            lease.clone(),
            RuntimeCommand::pause(Uuid::from_u128(0xfeed_9), session_id, run.id())
                .map_err(ExecutionStoreError::from)?,
            vec![paused_event.clone()],
            vec![],
            vec![],
            vec![],
            None,
            paused.clone(),
        ))
        .await;
    if !matches!(
        stale_run,
        Err(error) if error.code() == ExecutionStoreErrorCode::VersionConflict
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let stale_checkpoint = store
        .commit_execution(ExecutionCommit::new(
            outcome.stored_run().run_version(),
            0,
            lease.clone(),
            RuntimeCommand::pause(Uuid::from_u128(0xfeed_a), session_id, run.id())
                .map_err(ExecutionStoreError::from)?,
            vec![paused_event.clone()],
            vec![],
            vec![],
            vec![],
            None,
            paused.clone(),
        ))
        .await;
    if !matches!(
        stale_checkpoint,
        Err(error) if error.code() == ExecutionStoreErrorCode::CheckpointConflict
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    if store.load_run(run.id()).await?.as_ref() != Some(outcome.stored_run())
        || store.replay_events(run.id(), 0).await? != events
        || store
            .load_checkpoint(run.id())
            .await?
            .as_ref()
            .map(|(version, _)| *version)
            != Some(1)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let paused_outcome = store
        .commit_execution(ExecutionCommit::new(
            outcome.stored_run().run_version(),
            1,
            lease,
            RuntimeCommand::pause(Uuid::from_u128(0xfeed_c), session_id, run.id())
                .map_err(ExecutionStoreError::from)?,
            vec![paused_event.clone()],
            vec![],
            vec![],
            vec![],
            None,
            paused.clone(),
        ))
        .await?;
    if store.commit_execution(commit).await? != outcome
        || store.load_run(run.id()).await?.as_ref().map(StoredRun::run) != Some(&paused)
        || store.replay_events(run.id(), 0).await?
            != events
                .into_iter()
                .chain(std::iter::once(paused_event))
                .collect::<Vec<_>>()
        || paused_outcome.stored_run().run_version()
            != outcome.stored_run().run_version().saturating_add(1)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    assert_concurrent_session_contract(factory).await?;
    assert_stale_cas_contract(factory).await?;
    assert_lease_reclamation_contract(factory).await?;
    assert_serial_claim_lifecycle_contract(factory).await?;
    assert_uncounted_approval_commit_contract(factory).await?;
    assert_command_conflict_contract(factory).await?;
    assert_atomic_failure_contract(factory).await?;
    assert_counted_grant_contract(factory).await?;
    assert_durable_result_contract(factory).await
}

fn conformance_definition(allows_concurrent_sessions: bool) -> AgentDefinition {
    AgentDefinition {
        schema_version: 1,
        id: "execution-store-concurrent".into(),
        name: "contract".into(),
        display_name: "contract".into(),
        description: "contract".into(),
        persona: "contract".into(),
        system: "contract".into(),
        version: 1,
        model: ModelPolicy {
            provider: "test".into(),
            model: "test".into(),
            credential_reference: None,
            temperature: None,
        },
        source_profile: ProfileRef {
            profile_id: "contract".into(),
            profile_version: 1,
        },
        resolved_capabilities: vec![],
        memory: MemoryPolicy {
            enabled: false,
            namespace: "contract".into(),
            retention_days: None,
        },
        approval_policy_id: "contract".into(),
        approval_policy_revision: 1,
        approval_restrictions: vec![],
        limits: RuntimeLimits {
            max_turns: 1,
            timeout_ms: 1,
            max_concurrent_tasks: 1,
        },
        lifecycle: LifecyclePolicy {
            auto_start: false,
            restart_on_failure: false,
            max_restarts: 0,
            allows_concurrent_sessions,
        },
        host_requirements: vec![],
    }
}

async fn assert_concurrent_session_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    if Session::new_for_definition(
        Uuid::from_u128(0xc0),
        &conformance_definition(false),
        SessionConcurrencyPolicy::Concurrent,
    )
    .is_ok()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let store = factory.create_execution_store().await?;
    let session = Session::new_for_definition(
        Uuid::from_u128(0xc1),
        &conformance_definition(true),
        SessionConcurrencyPolicy::Concurrent,
    )
    .map_err(ExecutionStoreError::from)?;
    let first = Run::queued(
        Uuid::from_u128(0xc2),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let second = Run::queued(
        Uuid::from_u128(0xc3),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let one = store
        .create_run(CreateRun::new(
            session.clone(),
            first,
            0,
            SessionConcurrencyPolicy::Concurrent,
        ))
        .await?;
    let two = store
        .create_run(CreateRun::new(
            session,
            second,
            one.session_version(),
            SessionConcurrencyPolicy::Concurrent,
        ))
        .await?;
    if two.session_version() != one.session_version().saturating_add(1) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_stale_cas_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = Session::new_for_definition(
        Uuid::from_u128(0xca_10),
        &conformance_definition(true),
        SessionConcurrencyPolicy::Concurrent,
    )
    .map_err(ExecutionStoreError::from)?;
    let first = Run::queued(
        Uuid::from_u128(0xca_11),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let second = Run::queued(
        Uuid::from_u128(0xca_12),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    store
        .create_run(CreateRun::new(
            session.clone(),
            first.clone(),
            0,
            SessionConcurrencyPolicy::Concurrent,
        ))
        .await?;
    let stale = store
        .create_run(CreateRun::new(
            session,
            second.clone(),
            0,
            SessionConcurrencyPolicy::Concurrent,
        ))
        .await;
    if !matches!(
        stale,
        Err(error) if error.code() == ExecutionStoreErrorCode::VersionConflict
    ) || store.load_run(second.id()).await?.is_some()
        || store.load_run(first.id()).await?.is_none()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_lease_reclamation_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = Session::new(
        Uuid::from_u128(0x1e_a0),
        "execution-store-lease",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(0x1e_a1),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    let expired = store
        .acquire_lease(queued.id(), created.run_version(), 1)
        .await?;
    futures_timer::Delay::new(std::time::Duration::from_millis(10)).await;
    if !matches!(
        store.renew_lease(expired.clone(), 1_000).await,
        Err(error) if error.code() == ExecutionStoreErrorCode::LeaseExpired
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let current = store
        .acquire_lease(queued.id(), created.run_version(), 1_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let events = vec![
        RuntimeEvent::new(
            Uuid::from_u128(0x1e_a2),
            Uuid::from_u128(0x1e_a3),
            session.id(),
            queued.id(),
            1,
            1,
            RuntimeEventKind::RunStarted,
        )
        .map_err(ExecutionStoreError::from)?,
        RuntimeEvent::new(
            Uuid::from_u128(0x1e_a4),
            Uuid::from_u128(0x1e_a3),
            session.id(),
            queued.id(),
            2,
            2,
            RuntimeEventKind::StepStarted,
        )
        .map_err(ExecutionStoreError::from)?,
    ];
    let stale_commit = ExecutionCommit::new(
        created.run_version(),
        0,
        expired,
        RuntimeCommand::start(Uuid::from_u128(0x1e_a5), session.id(), queued.id())
            .map_err(ExecutionStoreError::from)?,
        events.clone(),
        vec![],
        vec![],
        vec![],
        None,
        running.clone(),
    );
    if !matches!(
        store.commit_execution(stale_commit).await,
        Err(error) if error.code() == ExecutionStoreErrorCode::LeaseExpired
    ) || !store.replay_events(queued.id(), 0).await?.is_empty()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    store
        .commit_execution(ExecutionCommit::new(
            created.run_version(),
            0,
            current,
            RuntimeCommand::start(Uuid::from_u128(0x1e_a6), session.id(), queued.id())
                .map_err(ExecutionStoreError::from)?,
            events.clone(),
            vec![],
            vec![],
            vec![],
            None,
            running,
        ))
        .await?;
    if store.replay_events(queued.id(), 0).await? != events {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_serial_claim_lifecycle_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = Session::new(
        Uuid::from_u128(0x5e_10),
        "execution-store-serial-lifecycle",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let first = Run::queued(
        Uuid::from_u128(0x5e_11),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(CreateRun::new(
            session.clone(),
            first.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    let lease = store
        .acquire_lease(first.id(), created.run_version(), 1_000)
        .await?;
    let running = first
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let started = RuntimeEvent::new(
        Uuid::from_u128(0x5e_12),
        Uuid::from_u128(0x5e_13),
        session.id(),
        first.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let running_outcome = store
        .commit_execution(ExecutionCommit::new(
            created.run_version(),
            0,
            lease.clone(),
            RuntimeCommand::start(Uuid::from_u128(0x5e_14), session.id(), first.id())
                .map_err(ExecutionStoreError::from)?,
            vec![started],
            vec![],
            vec![],
            vec![],
            None,
            running.clone(),
        ))
        .await?;
    let cancelled = running
        .transition(RunState::Cancelled, None)
        .map_err(ExecutionStoreError::from)?;
    let cancelled_event = RuntimeEvent::new(
        Uuid::from_u128(0x5e_15),
        Uuid::from_u128(0x5e_13),
        session.id(),
        first.id(),
        2,
        2,
        RuntimeEventKind::RunCancelled,
    )
    .map_err(ExecutionStoreError::from)?;
    let terminal = store
        .commit_execution(ExecutionCommit::new(
            running_outcome.stored_run().run_version(),
            0,
            store.renew_lease(lease, 1_000).await?,
            RuntimeCommand::cancel(Uuid::from_u128(0x5e_16), session.id(), first.id())
                .map_err(ExecutionStoreError::from)?,
            vec![cancelled_event],
            vec![],
            vec![],
            vec![],
            None,
            cancelled.clone(),
        ))
        .await?;
    let next = Run::queued(
        Uuid::from_u128(0x5e_17),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let next_stored = store
        .create_run(CreateRun::new(
            session,
            next.clone(),
            terminal.stored_run().session_version(),
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    if store
        .load_run(first.id())
        .await?
        .as_ref()
        .map(StoredRun::run)
        != Some(&cancelled)
        || store
            .load_run(next.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&next)
        || next_stored.session_version()
            != terminal.stored_run().session_version().saturating_add(1)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_uncounted_approval_commit_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = Session::new_for_definition(
        Uuid::from_u128(0xa9_10),
        &conformance_definition(true),
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let run_id = Uuid::from_u128(0xa9_11);
    let (invocation, context) = conformance_policy_context(run_id)?;
    let mut grant = conformance_counted_grant(&context)?;
    grant.remaining_uses = None;
    let (waiting, target, command, approval) = conformance_approval_resume(
        &session,
        run_id,
        Uuid::from_u128(0xa9_12),
        &invocation,
        &context,
        &grant,
    )?;
    if approval.grant_consumption().is_some() || approval.remaining_uses().is_some() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let created = store
        .create_run(CreateRun::new(
            session.clone(),
            waiting.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    let lease = store
        .acquire_lease(run_id, created.run_version(), 1_000)
        .await?;
    let event = RuntimeEvent::new(
        Uuid::from_u128(0xa9_13),
        Uuid::from_u128(0xa9_14),
        session.id(),
        run_id,
        1,
        1,
        RuntimeEventKind::RunResumed,
    )
    .map_err(ExecutionStoreError::from)?;
    let commit = ExecutionCommit::new(
        created.run_version(),
        0,
        lease,
        command,
        vec![event.clone()],
        vec![],
        vec![],
        vec![],
        Some(approval),
        target.clone(),
    );
    let outcome = store.commit_execution(commit.clone()).await?;
    if store.commit_execution(commit).await? != outcome
        || store.load_run(run_id).await?.as_ref().map(StoredRun::run) != Some(&target)
        || store.replay_events(run_id, 0).await? != vec![event]
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_command_conflict_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = Session::new(
        Uuid::from_u128(0xcc_10),
        "execution-store-command-conflict",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(0xcc_11),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    let lease = store
        .acquire_lease(queued.id(), created.run_version(), 1_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let command_id = Uuid::from_u128(0xcc_12);
    let event = RuntimeEvent::new(
        Uuid::from_u128(0xcc_13),
        Uuid::from_u128(0xcc_14),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let original = ExecutionCommit::new(
        created.run_version(),
        0,
        lease.clone(),
        RuntimeCommand::start(command_id, session.id(), queued.id())
            .map_err(ExecutionStoreError::from)?,
        vec![event.clone()],
        vec![],
        vec![],
        vec![],
        None,
        running.clone(),
    );
    let original_outcome = store.commit_execution(original.clone()).await?;
    let cancelled = running
        .transition(RunState::Cancelled, None)
        .map_err(ExecutionStoreError::from)?;
    let conflicting_event = RuntimeEvent::new(
        Uuid::from_u128(0xcc_15),
        Uuid::from_u128(0xcc_14),
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::RunCancelled,
    )
    .map_err(ExecutionStoreError::from)?;
    let conflict = store
        .commit_execution(ExecutionCommit::new(
            original_outcome.stored_run().run_version(),
            0,
            lease,
            RuntimeCommand::cancel(command_id, session.id(), queued.id())
                .map_err(ExecutionStoreError::from)?,
            vec![conflicting_event],
            vec![],
            vec![],
            vec![],
            None,
            cancelled,
        ))
        .await;
    if !matches!(
        conflict,
        Err(error) if error.code() == ExecutionStoreErrorCode::CommandConflict
    ) || store.commit_execution(original).await? != original_outcome
        || store.load_run(queued.id()).await?.as_ref() != Some(original_outcome.stored_run())
        || store.replay_events(queued.id(), 0).await? != vec![event]
        || store.load_checkpoint(queued.id()).await?.is_some()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_atomic_failure_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = Session::new(
        Uuid::from_u128(0xaf_10),
        "execution-store-atomic",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(0xaf_11),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "atomic-result",
        "workspace.write",
        1,
        serde_json::json!({"path": "baseline.txt"}),
    ))?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:execution-store-atomic",
        RecoveryMode::KeyedIdempotent,
    ))?;
    let baseline_attempt = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Completed,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let result_reference = CapabilityReferenceId::new(Uuid::from_u128(0xaf_12));
    let completed = conformance_value(CompletedInvocationRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let result = conformance_value(DurableCapabilityResult::new(
        result_reference.clone(),
        "jcs-v1:atomic-baseline",
        "sha256:execution-store-atomic",
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let lease = store
        .acquire_lease(queued.id(), created.run_version(), 1_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let baseline_event = RuntimeEvent::new(
        Uuid::from_u128(0xaf_13),
        Uuid::from_u128(0xaf_14),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let baseline = store
        .commit_execution(ExecutionCommit::new(
            created.run_version(),
            0,
            lease.clone(),
            RuntimeCommand::start(Uuid::from_u128(0xaf_15), session.id(), queued.id())
                .map_err(ExecutionStoreError::from)?,
            vec![baseline_event.clone()],
            vec![],
            vec![baseline_attempt.clone()],
            vec![DurableResultMutation::new(
                completed.clone(),
                result.clone(),
            )],
            None,
            running.clone(),
        ))
        .await?;
    let candidate_invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "atomic-candidate",
        "workspace.write",
        1,
        serde_json::json!({"path": "candidate.txt"}),
    ))?;
    let candidate_attempt = conformance_value(InvocationAttemptRecord::new(
        candidate_invocation.binding(),
        1,
        super::AttemptRecordState::Pending,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let candidate_step = Step::new(queued.id(), "atomic-candidate", StepKind::Capability)
        .map_err(ExecutionStoreError::from)?;
    let checkpoint = CheckpointV1Builder::new(
        session.id(),
        queued.id(),
        DefinitionPin::new(1, "execution-store-atomic", 1).map_err(ExecutionStoreError::from)?,
        2,
        vec![manifest],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .build()
    .map_err(ExecutionStoreError::from)?;
    let candidate_event = RuntimeEvent::new(
        Uuid::from_u128(0xaf_16),
        Uuid::from_u128(0xaf_14),
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::StepCompleted,
    )
    .map_err(ExecutionStoreError::from)?;
    let conflicting_result = conformance_value(DurableCapabilityResult::new(
        result_reference,
        "jcs-v1:atomic-conflict",
        "sha256:execution-store-atomic",
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let failed = store
        .commit_execution(
            ExecutionCommit::new(
                baseline.stored_run().run_version(),
                0,
                store.renew_lease(lease, 1_000).await?,
                RuntimeCommand::start(Uuid::from_u128(0xaf_17), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![candidate_event],
                vec![candidate_step],
                vec![candidate_attempt],
                vec![DurableResultMutation::new(completed, conflicting_result)],
                None,
                running,
            )
            .with_checkpoint(checkpoint),
        )
        .await;
    if !matches!(
        failed,
        Err(error) if error.code() == ExecutionStoreErrorCode::ResultConflict
    ) || store.load_run(queued.id()).await?.as_ref() != Some(baseline.stored_run())
        || store.load_checkpoint(queued.id()).await?.is_some()
        || !store.load_steps(queued.id()).await?.is_empty()
        || store.load_attempts(queued.id()).await? != vec![baseline_attempt]
        || store.replay_events(queued.id(), 0).await? != vec![baseline_event]
        || store
            .load_durable_result(queued.id(), invocation.id())
            .await?
            .as_ref()
            != Some(&result)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

fn conformance_value<T, E>(result: Result<T, E>) -> Result<T, ExecutionStoreError> {
    result.map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))
}

fn conformance_capability_manifest() -> CapabilityManifest {
    CapabilityManifest {
        id: "workspace.write".into(),
        version: 1,
        kind: CapabilityKind::Workspace,
        label: "Write".into(),
        description: "Writes a workspace file".into(),
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: serde_json::json!({"type": "object"}),
        side_effects: true,
        risk_level: RiskLevel::High,
        host_permissions: vec![],
        secret_references: vec![],
        environment_requirements: vec![],
        timeout_ms: 1_000,
        cancellation_supported: true,
        max_retries: 0,
        idempotent: false,
        recovery_mode: RecoveryMode::NonRetryable,
        supports_streaming: false,
        supports_artifacts: false,
        supports_citations: false,
        schema_digest: "sha256:execution-store-contract:1".into(),
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    }
}

fn conformance_policy_context(
    run_id: Uuid,
) -> Result<(LogicalInvocation, PolicyContext), ExecutionStoreError> {
    let manifest = conformance_capability_manifest();
    let invocation = conformance_value(LogicalInvocation::new(
        run_id,
        "counted-grant-step",
        manifest.id.clone(),
        manifest.version,
        serde_json::json!({"path": "contract.txt"}),
    ))?;
    let context = conformance_value(PolicyContext::new(
        "contract-owner",
        "contract-actor",
        "execution-store-concurrent",
        1,
        "contract-workspace",
        CapabilityReferenceId::new(Uuid::from_u128(0xc0_10)),
        &manifest,
        &invocation,
        1,
        PolicyRestrictions::default(),
        1_000,
    ))?;
    Ok((invocation, context))
}

fn conformance_counted_grant(
    context: &PolicyContext,
) -> Result<AutonomyGrant, ExecutionStoreError> {
    conformance_value(AutonomyGrant::new_with_effect(
        "execution-store-counted-grant",
        1,
        GrantStatus::Active,
        conformance_value(GrantScope::new(
            context.owner_id.clone(),
            context.actor_id.clone(),
            context.agent_definition_id.clone(),
            context.agent_definition_version,
            context.workspace_id.clone(),
            context.resource_boundary.clone(),
            context.capability_id.clone(),
            context.manifest_version,
            Some(context.canonical_argument_digest),
        ))?,
        RiskLevel::High,
        500,
        Some(2_000),
        Some(1),
        GrantEffect::ApprovalRequired,
    ))
}

fn conformance_approval_resume(
    session: &Session,
    run_id: Uuid,
    command_id: Uuid,
    invocation: &LogicalInvocation,
    context: &PolicyContext,
    grant: &AutonomyGrant,
) -> Result<(Run, Run, RuntimeCommand, ApprovalGrantMutation), ExecutionStoreError> {
    let request = conformance_value(PolicyEngine::approval_request(context, Some(grant)))?;
    let decision = conformance_value(ApprovalDecision::new_approved(request.clone(), 1_000))?;
    let claim = conformance_value(ApprovalResumeClaim::new(
        &request,
        &decision,
        context,
        &[grant.clone()],
    ))?;
    let expected_consumption = grant
        .remaining_uses
        .map(|_| {
            conformance_value(GrantConsumption::new(
                grant.id.clone(),
                grant.revision,
                invocation.id(),
            ))
        })
        .transpose()?;
    if claim.grant_consumption() != expected_consumption.as_ref() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let waiting = Run::queued(
        run_id,
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .and_then(|run| run.transition(RunState::Running, None))
    .and_then(|run| run.wait_for_approval(request))
    .map_err(ExecutionStoreError::from)?;
    let command = RuntimeCommand::resume_with_approval(
        command_id,
        session.id(),
        run_id,
        claim.binding().clone(),
    )
    .map_err(ExecutionStoreError::from)?;
    let target = waiting
        .apply_resume_command(&command, Some(&claim), None)
        .map_err(ExecutionStoreError::from)?
        .run()
        .clone();
    Ok((
        waiting,
        target,
        command,
        ApprovalGrantMutation::from_claim(claim),
    ))
}

async fn assert_counted_grant_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = conformance_value(Session::new_for_definition(
        Uuid::from_u128(0xc0_20),
        &conformance_definition(true),
        SessionConcurrencyPolicy::Concurrent,
    ))?;
    let (first_invocation, first_context) = conformance_policy_context(Uuid::from_u128(0xc0_21))?;
    let (second_invocation, second_context) = conformance_policy_context(Uuid::from_u128(0xc0_22))?;
    let grant = conformance_counted_grant(&first_context)?;
    let request = conformance_value(PolicyEngine::approval_request(&first_context, Some(&grant)))?;
    let decision = conformance_value(ApprovalDecision::new_approved(request.clone(), 1_000))?;
    let mut uncounted_substitution = grant.clone();
    uncounted_substitution.remaining_uses = None;
    let mut inflated_substitution = grant.clone();
    inflated_substitution.remaining_uses = Some(2);
    if ApprovalResumeClaim::new_with_grants(
        &request,
        &decision,
        &first_context,
        &[uncounted_substitution.clone()],
    )
    .is_ok()
        || ApprovalResumeClaim::new_with_grants(
            &request,
            &decision,
            &first_context,
            &[inflated_substitution],
        )
        .is_ok()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let uncounted_request = conformance_value(PolicyEngine::approval_request(
        &first_context,
        Some(&uncounted_substitution),
    ))?;
    let uncounted_decision = conformance_value(ApprovalDecision::new_approved(
        uncounted_request.clone(),
        1_000,
    ))?;
    let uncounted_claim = conformance_value(ApprovalResumeClaim::new_with_grants(
        &uncounted_request,
        &uncounted_decision,
        &first_context,
        &[uncounted_substitution],
    ))?;
    let uncounted_mutation = ApprovalGrantMutation::from_claim(uncounted_claim);
    if uncounted_mutation.grant_consumption().is_some()
        || uncounted_mutation.remaining_uses().is_some()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let (first_waiting, first_target, first_command, first_approval) = conformance_approval_resume(
        &session,
        Uuid::from_u128(0xc0_21),
        Uuid::from_u128(0xc0_23),
        &first_invocation,
        &first_context,
        &grant,
    )?;
    let (second_waiting, second_target, second_command, second_approval) =
        conformance_approval_resume(
            &session,
            Uuid::from_u128(0xc0_22),
            Uuid::from_u128(0xc0_24),
            &second_invocation,
            &second_context,
            &grant,
        )?;
    if first_invocation.id() == second_invocation.id() || first_command.id() == second_command.id()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let first_created = store
        .create_run(CreateRun::new(
            session.clone(),
            first_waiting.clone(),
            0,
            SessionConcurrencyPolicy::Concurrent,
        ))
        .await?;
    let second_created = store
        .create_run(CreateRun::new(
            session.clone(),
            second_waiting.clone(),
            first_created.session_version(),
            SessionConcurrencyPolicy::Concurrent,
        ))
        .await?;
    let first_lease = store
        .acquire_lease(first_waiting.id(), first_created.run_version(), 1_000)
        .await?;
    let second_lease = store
        .acquire_lease(second_waiting.id(), second_created.run_version(), 1_000)
        .await?;
    let first_event = RuntimeEvent::new(
        Uuid::from_u128(0xc0_25),
        Uuid::from_u128(0xc0_27),
        session.id(),
        first_waiting.id(),
        1,
        1,
        RuntimeEventKind::RunResumed,
    )
    .map_err(ExecutionStoreError::from)?;
    let second_event = RuntimeEvent::new(
        Uuid::from_u128(0xc0_26),
        Uuid::from_u128(0xc0_28),
        session.id(),
        second_waiting.id(),
        1,
        1,
        RuntimeEventKind::RunResumed,
    )
    .map_err(ExecutionStoreError::from)?;
    let first_commit = ExecutionCommit::new(
        first_created.run_version(),
        0,
        first_lease,
        first_command,
        vec![first_event],
        vec![],
        vec![],
        vec![],
        Some(first_approval),
        first_target,
    );
    let second_commit = ExecutionCommit::new(
        second_created.run_version(),
        0,
        second_lease,
        second_command,
        vec![second_event],
        vec![],
        vec![],
        vec![],
        Some(second_approval),
        second_target,
    );
    let (first_result, second_result) = futures::join!(
        store.commit_execution(first_commit.clone()),
        store.commit_execution(second_commit.clone())
    );
    let (winning_commit, original_outcome, losing_waiting) = match (first_result, second_result) {
        (Ok(outcome), Err(error))
            if error.code() == ExecutionStoreErrorCode::GrantAlreadyConsumed =>
        {
            (first_commit, outcome, second_waiting)
        }
        (Err(error), Ok(outcome))
            if error.code() == ExecutionStoreErrorCode::GrantAlreadyConsumed =>
        {
            (second_commit, outcome, first_waiting)
        }
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    };
    if store.commit_execution(winning_commit).await? != original_outcome
        || store
            .load_run(losing_waiting.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&losing_waiting)
        || !store
            .replay_events(losing_waiting.id(), 0)
            .await?
            .is_empty()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_durable_result_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session = Session::new(
        Uuid::from_u128(0xd0_10),
        "execution-store-result",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(0xd0_11),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "durable-result-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "contract.txt"}),
    ))?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:execution-store-result",
        RecoveryMode::KeyedIdempotent,
    ))?;
    let attempt = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Completed,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let result_reference = CapabilityReferenceId::new(Uuid::from_u128(0xd0_12));
    let completed = conformance_value(CompletedInvocationRecord::new(
        invocation.binding(),
        1,
        manifest,
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let result = conformance_value(DurableCapabilityResult::new(
        result_reference.clone(),
        "jcs-v1:execution-store-result",
        "sha256:execution-store-result",
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let lease = store
        .acquire_lease(queued.id(), created.run_version(), 1_000)
        .await?;
    let first_event = RuntimeEvent::new(
        Uuid::from_u128(0xd0_13),
        Uuid::from_u128(0xd0_14),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let first_outcome = store
        .commit_execution(ExecutionCommit::new(
            created.run_version(),
            0,
            lease.clone(),
            RuntimeCommand::start(Uuid::from_u128(0xd0_15), session.id(), queued.id())
                .map_err(ExecutionStoreError::from)?,
            vec![first_event],
            vec![],
            vec![attempt.clone()],
            vec![DurableResultMutation::new(
                completed.clone(),
                result.clone(),
            )],
            None,
            running.clone(),
        ))
        .await?;
    if store
        .load_durable_result(queued.id(), invocation.id())
        .await?
        .as_ref()
        != Some(&result)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let renewed = store.renew_lease(lease, 1_000).await?;
    let second_event = RuntimeEvent::new(
        Uuid::from_u128(0xd0_16),
        Uuid::from_u128(0xd0_14),
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::StepCompleted,
    )
    .map_err(ExecutionStoreError::from)?;
    let identical_commit = ExecutionCommit::new(
        first_outcome.stored_run().run_version(),
        0,
        renewed.clone(),
        RuntimeCommand::start(Uuid::from_u128(0xd0_17), session.id(), queued.id())
            .map_err(ExecutionStoreError::from)?,
        vec![second_event],
        vec![],
        vec![attempt.clone()],
        vec![DurableResultMutation::new(
            completed.clone(),
            result.clone(),
        )],
        None,
        running.clone(),
    );
    let identical_outcome = store.commit_execution(identical_commit.clone()).await?;
    if store.commit_execution(identical_commit).await? != identical_outcome
        || store
            .load_durable_result(queued.id(), invocation.id())
            .await?
            .as_ref()
            != Some(&result)
        || store.load_attempts(queued.id()).await? != vec![attempt]
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let before_conflict = store.load_run(queued.id()).await?;
    let conflicting_result = conformance_value(DurableCapabilityResult::new(
        result_reference,
        "jcs-v1:conflicting-execution-store-result",
        "sha256:execution-store-result",
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let conflicting_event = RuntimeEvent::new(
        Uuid::from_u128(0xd0_18),
        Uuid::from_u128(0xd0_14),
        session.id(),
        queued.id(),
        3,
        3,
        RuntimeEventKind::StepCompleted,
    )
    .map_err(ExecutionStoreError::from)?;
    let conflict = store
        .commit_execution(ExecutionCommit::new(
            identical_outcome.stored_run().run_version(),
            0,
            store.renew_lease(renewed, 1_000).await?,
            RuntimeCommand::start(Uuid::from_u128(0xd0_19), session.id(), queued.id())
                .map_err(ExecutionStoreError::from)?,
            vec![conflicting_event],
            vec![],
            vec![],
            vec![DurableResultMutation::new(completed, conflicting_result)],
            None,
            running,
        ))
        .await;
    if !matches!(
        conflict,
        Err(error) if error.code() == ExecutionStoreErrorCode::ResultConflict
    ) || store.load_run(queued.id()).await? != before_conflict
        || store
            .load_durable_result(queued.id(), invocation.id())
            .await?
            .as_ref()
            != Some(&result)
        || store.replay_events(queued.id(), 0).await?.len() != 2
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}
