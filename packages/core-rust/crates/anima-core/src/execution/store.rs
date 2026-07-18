use async_trait::async_trait;
use std::fmt;

use uuid::Uuid;

use super::{
    ApprovalResumeClaim, CheckpointV1, CommandReceipt, CompletedInvocationRecord, ExecutionError,
    ExecutionLease, InvocationAttemptRecord, Run, RuntimeCommand, RuntimeEvent, Session,
    SessionConcurrencyPolicy, Step,
};
use crate::{DurableCapabilityResult, GrantConsumption};

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
}

impl ApprovalGrantMutation {
    /// Preserves Task 4's validated approval claim and its exact consumption as one commit input.
    pub fn from_claim(claim: ApprovalResumeClaim) -> Self {
        Self {
            grant_consumption: claim.grant_consumption().cloned(),
            claim,
        }
    }

    pub fn claim(&self) -> &ApprovalResumeClaim {
        &self.claim
    }

    pub fn grant_consumption(&self) -> Option<&GrantConsumption> {
        self.grant_consumption.as_ref()
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
    pub(crate) fn new(stored_run: StoredRun, receipt: CommandReceipt) -> Self {
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
    pub(crate) fn new(run: Run, run_version: u64, session_version: u64) -> Self {
        Self {
            run,
            run_version,
            session_version,
        }
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

    async fn replay_events(
        &self,
        run_id: Uuid,
        after_sequence: u64,
    ) -> Result<Vec<RuntimeEvent>, ExecutionStoreError>;
}
