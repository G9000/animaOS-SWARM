use async_trait::async_trait;
use std::fmt;

use uuid::Uuid;

use super::{
    ApprovalResumeClaim, Budget, CheckpointV1, CheckpointV1Builder, CommandReceipt,
    CompletedInvocationRecord, DefinitionPin, ExecutionError, ExecutionLease,
    InvocationAttemptRecord, Run, RunState, RuntimeCommand, RuntimeEvent, RuntimeEventKind,
    Session, SessionConcurrencyPolicy, Step, Usage,
};
use crate::{
    AgentDefinition, DurableCapabilityResult, GrantConsumption, LifecyclePolicy, MemoryPolicy,
    ModelPolicy, ProfileRef, RuntimeLimits,
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
    grant_consumption: Option<GrantConsumption>,
    remaining_uses: Option<u32>,
}

impl ApprovalGrantMutation {
    /// Preserves Task 4's validated approval claim and its exact consumption as one commit input.
    pub fn from_claim(
        claim: ApprovalResumeClaim,
        remaining_uses: Option<u32>,
    ) -> Result<Self, ExecutionStoreError> {
        if remaining_uses == Some(0) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            grant_consumption: claim.grant_consumption().cloned(),
            claim,
            remaining_uses,
        })
    }

    pub fn claim(&self) -> &ApprovalResumeClaim {
        &self.claim
    }

    pub fn grant_consumption(&self) -> Option<&GrantConsumption> {
        self.grant_consumption.as_ref()
    }

    pub fn remaining_uses(&self) -> Option<u32> {
        self.remaining_uses
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
    let gap = RuntimeEvent::new(
        Uuid::from_u128(0xfeed_4),
        Uuid::from_u128(0xfeed_5),
        session_id,
        run.id(),
        1,
        2,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let gap_commit = ExecutionCommit::new(
        created.run_version(),
        0,
        lease.clone(),
        command.clone(),
        vec![gap],
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
    let checkpoint = CheckpointV1Builder::new(
        session_id,
        run.id(),
        DefinitionPin::new(1, "execution-store-contract", 1).map_err(ExecutionStoreError::from)?,
        2,
        vec![],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .build()
    .map_err(ExecutionStoreError::from)?;
    let commit = ExecutionCommit::new(
        created.run_version(),
        0,
        lease,
        command,
        events.clone(),
        vec![],
        vec![],
        vec![],
        None,
        running,
    )
    .with_checkpoint(checkpoint);
    let outcome = store.commit_execution(commit.clone()).await?;
    if store.commit_execution(commit).await? != outcome
        || store.replay_events(run.id(), 0).await? != events
        || store
            .load_checkpoint(run.id())
            .await?
            .map(|(version, _)| version)
            != Some(1)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    assert_concurrent_session_contract(factory).await
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
