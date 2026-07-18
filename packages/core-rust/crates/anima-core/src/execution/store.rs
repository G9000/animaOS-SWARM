use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::sync::Arc;

use uuid::Uuid;

use super::{
    ApprovalResumeClaim, Budget, CheckpointV1, CheckpointV1Builder, CommandReceipt,
    CompletedInvocationRecord, DefinitionPin, ExecutionError, ExecutionLease,
    GrantAuthorityBinding, GrantAuthorityKey, InvocationAttemptRecord, Run, RunState,
    RuntimeCommand, RuntimeCommandKind, RuntimeEvent, RuntimeEventKind, Session,
    SessionConcurrencyPolicy, Step, StepKind, Usage,
};
use crate::{
    AgentDefinition, ApprovalDecision, AutonomyGrant, CapabilityKind, CapabilityManifest,
    CapabilityReferenceId, DurableCapabilityResult, DurableCapabilityStatus, GrantConsumption,
    GrantEffect, GrantScope, GrantStatus, LifecyclePolicy, LogicalInvocation, ManifestPin,
    MemoryPolicy, ModelPolicy, OpaqueReference, PolicyContext, PolicyEngine, PolicyRestrictions,
    ProfileRef, RecoveryMode, RecoveryResumeBinding, RiskLevel, RuntimeCompatibility,
    RuntimeLimits,
};

pub const MAX_COMMIT_EVENTS: usize = 256;
pub const MAX_COMMIT_STEPS: usize = 128;
pub const MAX_COMMIT_ATTEMPTS: usize = 256;
pub const MAX_COMMIT_RESULTS: usize = 128;
pub const MAX_COMMIT_BATCH_ITEMS: usize = 512;
pub const MAX_STORE_READ_PAGE_SIZE: u32 = 256;
pub const MAX_STORE_READ_CURSOR_BYTES: usize = 256;
const MAX_COMMIT_CHECKPOINT_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreReadCursor(String);

impl StoreReadCursor {
    pub fn from_opaque(value: impl Into<String>) -> Result<Self, ExecutionStoreError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_STORE_READ_CURSOR_BYTES || !value.is_ascii() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::BoundsExceeded,
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreReadPage {
    cursor: Option<StoreReadCursor>,
    limit: u32,
}

impl StoreReadPage {
    pub fn new(cursor: Option<StoreReadCursor>, limit: u32) -> Result<Self, ExecutionStoreError> {
        if limit == 0 || limit > MAX_STORE_READ_PAGE_SIZE {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::BoundsExceeded,
            ));
        }
        Ok(Self { cursor, limit })
    }

    pub fn first(limit: u32) -> Result<Self, ExecutionStoreError> {
        Self::new(None, limit)
    }

    pub fn after(cursor: StoreReadCursor, limit: u32) -> Result<Self, ExecutionStoreError> {
        Self::new(Some(cursor), limit)
    }

    pub fn cursor(&self) -> Option<&StoreReadCursor> {
        self.cursor.as_ref()
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreHistoryPage<T> {
    items: Vec<T>,
    next_cursor: Option<StoreReadCursor>,
}

impl<T> StoreHistoryPage<T> {
    pub fn new(
        items: Vec<T>,
        next_cursor: Option<StoreReadCursor>,
    ) -> Result<Self, ExecutionStoreError> {
        if items.len() > MAX_STORE_READ_PAGE_SIZE as usize
            || items.is_empty() && next_cursor.is_some()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::BoundsExceeded,
            ));
        }
        Ok(Self { items, next_cursor })
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn into_items(self) -> Vec<T> {
        self.items
    }

    pub fn next_cursor(&self) -> Option<&StoreReadCursor> {
        self.next_cursor.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventReplayPage {
    events: Vec<RuntimeEvent>,
    next_cursor: Option<StoreReadCursor>,
}

impl EventReplayPage {
    pub fn new(
        events: Vec<RuntimeEvent>,
        next_cursor: Option<StoreReadCursor>,
    ) -> Result<Self, ExecutionStoreError> {
        if events.len() > MAX_STORE_READ_PAGE_SIZE as usize
            || events.is_empty() && next_cursor.is_some()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::BoundsExceeded,
            ));
        }
        if let Some(first) = events.first() {
            RuntimeEvent::validate_batch(first.sequence(), &events)
                .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::EventConflict))?;
        }
        Ok(Self {
            events,
            next_cursor,
        })
    }

    pub fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }

    pub fn into_events(self) -> Vec<RuntimeEvent> {
        self.events
    }

    pub fn next_cursor(&self) -> Option<&StoreReadCursor> {
        self.next_cursor.as_ref()
    }
}

pub type AuthoritativeGrantStatus = GrantStatus;

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeGrantState {
    owner_id: Uuid,
    authority_key: GrantAuthorityKey,
    full_grant_digest: String,
    scope_digest: String,
    revision: u32,
    status: GrantStatus,
    effect: GrantEffect,
    maximum_risk: RiskLevel,
    valid_from_ms: i64,
    valid_until_ms: Option<i64>,
    remaining_uses: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoritativeGrantStateWire {
    owner_id: Uuid,
    authority_key: String,
    full_grant_digest: String,
    scope_digest: String,
    revision: u32,
    status: GrantStatus,
    effect: GrantEffect,
    maximum_risk: RiskLevel,
    valid_from_ms: i64,
    valid_until_ms: Option<i64>,
    remaining_uses: Option<u32>,
}

impl AuthoritativeGrantState {
    pub fn from_grant(owner_id: Uuid, grant: &AutonomyGrant) -> Result<Self, ExecutionStoreError> {
        if owner_id.is_nil() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let binding =
            GrantAuthorityBinding::from_grant(grant).map_err(ExecutionStoreError::from)?;
        Self::from_binding(owner_id, binding)
    }

    fn from_binding(
        owner_id: Uuid,
        binding: GrantAuthorityBinding,
    ) -> Result<Self, ExecutionStoreError> {
        if owner_id.is_nil() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            owner_id,
            authority_key: binding.authority_key().clone(),
            full_grant_digest: binding.full_grant_digest().to_owned(),
            scope_digest: binding.scope_digest().to_owned(),
            revision: binding.revision(),
            status: binding.status(),
            effect: binding.effect(),
            maximum_risk: binding.maximum_risk(),
            valid_from_ms: binding.valid_from_ms(),
            valid_until_ms: binding.valid_until_ms(),
            remaining_uses: binding.remaining_uses(),
        })
    }

    pub(crate) fn binding(&self) -> Result<GrantAuthorityBinding, ExecutionStoreError> {
        GrantAuthorityBinding::from_parts(
            self.authority_key.as_str().to_owned(),
            self.full_grant_digest.clone(),
            self.scope_digest.clone(),
            self.revision,
            self.status,
            self.effect,
            self.maximum_risk,
            self.valid_from_ms,
            self.valid_until_ms,
            self.remaining_uses,
        )
        .map_err(ExecutionStoreError::from)
    }

    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn authority_key(&self) -> &GrantAuthorityKey {
        &self.authority_key
    }
    pub fn authority_key_encoded(&self) -> &str {
        self.authority_key.as_str()
    }
    pub fn full_grant_digest(&self) -> &str {
        &self.full_grant_digest
    }
    pub fn scope_digest(&self) -> &str {
        &self.scope_digest
    }
    pub fn revision(&self) -> u32 {
        self.revision
    }
    pub fn status(&self) -> GrantStatus {
        self.status
    }
    pub fn effect(&self) -> GrantEffect {
        self.effect
    }
    pub fn maximum_risk(&self) -> RiskLevel {
        self.maximum_risk
    }
    pub fn valid_from_ms(&self) -> i64 {
        self.valid_from_ms
    }
    pub fn valid_until_ms(&self) -> Option<i64> {
        self.valid_until_ms
    }
    pub fn remaining_uses(&self) -> Option<u32> {
        self.remaining_uses
    }

    pub(crate) fn as_revoked(&self) -> Self {
        let mut revoked = self.clone();
        revoked.status = GrantStatus::Revoked;
        revoked
    }

    pub(crate) fn consume_one(&mut self) -> Result<(), ExecutionStoreError> {
        let remaining = self
            .remaining_uses
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        self.remaining_uses = Some(remaining.checked_sub(1).ok_or_else(|| {
            ExecutionStoreError::new(ExecutionStoreErrorCode::GrantAlreadyConsumed)
        })?);
        Ok(())
    }
}

impl<'de> Deserialize<'de> for AuthoritativeGrantState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = AuthoritativeGrantStateWire::deserialize(deserializer)?;
        let binding = GrantAuthorityBinding::from_parts(
            wire.authority_key,
            wire.full_grant_digest,
            wire.scope_digest,
            wire.revision,
            wire.status,
            wire.effect,
            wire.maximum_risk,
            wire.valid_from_ms,
            wire.valid_until_ms,
            wire.remaining_uses,
        )
        .map_err(serde::de::Error::custom)?;
        Self::from_binding(wire.owner_id, binding).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for AuthoritativeGrantState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthoritativeGrantState")
            .field("owner_id", &"REDACTED")
            .field("authority_key", &"REDACTED")
            .field("full_grant_digest", &"REDACTED")
            .field("scope_digest", &"REDACTED")
            .field("revision", &self.revision)
            .field("status", &self.status)
            .field("effect", &self.effect)
            .field("maximum_risk", &self.maximum_risk)
            .field("valid_from_ms", &self.valid_from_ms)
            .field("valid_until_ms", &self.valid_until_ms)
            .field("remaining_uses", &self.remaining_uses)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoritativeGrantChange {
    kind: AuthoritativeGrantChangeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthoritativeGrantChangeKind {
    Create(AuthoritativeGrantState),
    Update {
        expected_revision: u32,
        state: AuthoritativeGrantState,
    },
    Revoke {
        authority_key: GrantAuthorityKey,
        expected_revision: u32,
    },
}

impl AuthoritativeGrantChange {
    pub fn create(state: AuthoritativeGrantState) -> Self {
        Self {
            kind: AuthoritativeGrantChangeKind::Create(state),
        }
    }

    pub fn update(
        expected_revision: u32,
        state: AuthoritativeGrantState,
    ) -> Result<Self, ExecutionStoreError> {
        if expected_revision == 0 || state.revision() <= expected_revision {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            kind: AuthoritativeGrantChangeKind::Update {
                expected_revision,
                state,
            },
        })
    }

    pub fn revoke(
        authority_key: GrantAuthorityKey,
        expected_revision: u32,
    ) -> Result<Self, ExecutionStoreError> {
        if expected_revision == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            kind: AuthoritativeGrantChangeKind::Revoke {
                authority_key,
                expected_revision,
            },
        })
    }

    pub fn kind(&self) -> &AuthoritativeGrantChangeKind {
        &self.kind
    }

    pub fn expected_revision(&self) -> Option<u32> {
        match &self.kind {
            AuthoritativeGrantChangeKind::Create(_) => None,
            AuthoritativeGrantChangeKind::Update {
                expected_revision, ..
            }
            | AuthoritativeGrantChangeKind::Revoke {
                expected_revision, ..
            } => Some(*expected_revision),
        }
    }

    pub fn new_state(&self) -> Option<&AuthoritativeGrantState> {
        match &self.kind {
            AuthoritativeGrantChangeKind::Create(state)
            | AuthoritativeGrantChangeKind::Update { state, .. } => Some(state),
            AuthoritativeGrantChangeKind::Revoke { .. } => None,
        }
    }

    pub fn authority_key(&self) -> &GrantAuthorityKey {
        match &self.kind {
            AuthoritativeGrantChangeKind::Create(state)
            | AuthoritativeGrantChangeKind::Update { state, .. } => state.authority_key(),
            AuthoritativeGrantChangeKind::Revoke { authority_key, .. } => authority_key,
        }
    }
}

/// Input for atomically creating a durable run and claiming its session when required.
#[derive(Clone, Debug)]
pub struct CreateRun {
    owner_id: Uuid,
    session: Session,
    run: Run,
    expected_session_version: u64,
    concurrency_policy: SessionConcurrencyPolicy,
}

impl CreateRun {
    pub fn new_for_owner(
        owner_id: Uuid,
        session: Session,
        run: Run,
        expected_session_version: u64,
        concurrency_policy: SessionConcurrencyPolicy,
    ) -> Self {
        Self {
            owner_id,
            session,
            run,
            expected_session_version,
            concurrency_policy,
        }
    }

    pub fn owner_id(&self) -> Uuid {
        self.owner_id
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
    owner_id: Uuid,
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

    pub fn grant_id(&self) -> Option<&str> {
        self.claim.grant_id()
    }

    pub fn grant_revision(&self) -> Option<u32> {
        self.claim.grant_revision()
    }

    pub fn grant_remaining_uses(&self) -> Option<u32> {
        self.claim.grant_remaining_uses()
    }
}

/// A durable result is keyed by a logical invocation and can only be recorded identically once.
#[derive(Clone, Debug)]
pub struct DurableResultMutation {
    completed: CompletedInvocationRecord,
    result: DurableCapabilityResult,
}

/// Explicit checkpoint freshness decision for one atomic execution commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointMutation {
    /// Retain the current checkpoint only when the post-commit aggregate is still identical to it.
    Unchanged,
    /// Replace the current checkpoint with an exact snapshot of the post-commit aggregate.
    Replace(CheckpointV1),
    /// Invalidate any current checkpoint while advancing the checkpoint generation.
    Clear,
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
    checkpoint: CheckpointMutation,
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
            checkpoint: CheckpointMutation::Clear,
            target_run,
        }
    }

    pub fn with_checkpoint(mut self, checkpoint: CheckpointV1) -> Self {
        self.checkpoint = CheckpointMutation::Replace(checkpoint);
        self
    }

    pub fn with_checkpoint_mutation(mut self, mutation: CheckpointMutation) -> Self {
        self.checkpoint = mutation;
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
        match &self.checkpoint {
            CheckpointMutation::Replace(checkpoint) => Some(checkpoint),
            CheckpointMutation::Unchanged | CheckpointMutation::Clear => None,
        }
    }

    pub fn checkpoint_mutation(&self) -> &CheckpointMutation {
        &self.checkpoint
    }

    pub(super) fn into_checkpoint_mutation(self) -> CheckpointMutation {
        self.checkpoint
    }

    pub fn target_run(&self) -> &Run {
        &self.target_run
    }

    pub fn validate_bounds(&self) -> Result<(), ExecutionStoreError> {
        if self.events.len() > MAX_COMMIT_EVENTS
            || self.steps.len() > MAX_COMMIT_STEPS
            || self.attempts.len() > MAX_COMMIT_ATTEMPTS
            || self.results.len() > MAX_COMMIT_RESULTS
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::BoundsExceeded,
            ));
        }
        let total = self
            .events
            .len()
            .checked_add(self.steps.len())
            .and_then(|value| value.checked_add(self.attempts.len()))
            .and_then(|value| value.checked_add(self.results.len()))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        if total > MAX_COMMIT_BATCH_ITEMS
            || self.checkpoint().is_some_and(|checkpoint| {
                serde_jcs::to_vec(checkpoint)
                    .map_or(true, |bytes| bytes.len() > MAX_COMMIT_CHECKPOINT_BYTES)
            })
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::BoundsExceeded,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionCommitOutcome {
    stored_run: StoredRun,
    receipt: CommandReceipt,
    checkpoint_version: u64,
    checkpoint: Option<Arc<CheckpointV1>>,
    grant_consumption: Option<GrantConsumption>,
}

impl ExecutionCommitOutcome {
    pub fn new(
        stored_run: StoredRun,
        receipt: CommandReceipt,
        checkpoint_version: u64,
        checkpoint: Option<CheckpointV1>,
        grant_consumption: Option<GrantConsumption>,
    ) -> Self {
        Self::new_shared(
            stored_run,
            receipt,
            checkpoint_version,
            checkpoint.map(Arc::new),
            grant_consumption,
        )
    }

    pub(super) fn new_shared(
        stored_run: StoredRun,
        receipt: CommandReceipt,
        checkpoint_version: u64,
        checkpoint: Option<Arc<CheckpointV1>>,
        grant_consumption: Option<GrantConsumption>,
    ) -> Self {
        Self {
            stored_run,
            receipt,
            checkpoint_version,
            checkpoint,
            grant_consumption,
        }
    }

    pub fn stored_run(&self) -> &StoredRun {
        &self.stored_run
    }

    pub fn receipt(&self) -> &CommandReceipt {
        &self.receipt
    }

    pub fn checkpoint_version(&self) -> u64 {
        self.checkpoint_version
    }

    pub fn checkpoint(&self) -> Option<&CheckpointV1> {
        self.checkpoint.as_deref()
    }

    pub fn grant_consumption(&self) -> Option<&GrantConsumption> {
        self.grant_consumption.as_ref()
    }
}

impl StoredRun {
    pub fn new(
        owner_id: Uuid,
        run: Run,
        run_version: u64,
        session_version: u64,
    ) -> Result<Self, ExecutionStoreError> {
        if owner_id.is_nil() || run_version == 0 || session_version == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            owner_id,
            run,
            run_version,
            session_version,
        })
    }

    pub fn owner_id(&self) -> Uuid {
        self.owner_id
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
    GrantConflict,
    LineageConflict,
    HistoryConflict,
    BoundsExceeded,
    ArithmeticOverflow,
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
            ExecutionStoreErrorCode::GrantConflict => {
                "autonomy grant authority is missing, stale, or revoked"
            }
            ExecutionStoreErrorCode::LineageConflict => {
                "durable result does not match completed invocation lineage"
            }
            ExecutionStoreErrorCode::HistoryConflict => {
                "execution history key conflicts with its persisted value"
            }
            ExecutionStoreErrorCode::BoundsExceeded => "execution store batch or page is too large",
            ExecutionStoreErrorCode::ArithmeticOverflow => {
                "execution store version arithmetic overflowed"
            }
            ExecutionStoreErrorCode::ResultConflict => "durable invocation result conflicts",
            ExecutionStoreErrorCode::InvalidRequest => "execution store request is invalid",
        })
    }
}

impl std::error::Error for ExecutionStoreError {}

/// Adapter port for durable execution state. Each operation is atomic.
#[async_trait]
pub trait ExecutionStore: Send + Sync {
    async fn apply_authoritative_grant(
        &self,
        owner_id: Uuid,
        change: AuthoritativeGrantChange,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError>;

    async fn load_authoritative_grant(
        &self,
        owner_id: Uuid,
        authority_key: &GrantAuthorityKey,
    ) -> Result<Option<AuthoritativeGrantState>, ExecutionStoreError>;

    async fn create_run(
        &self,
        owner_id: Uuid,
        request: CreateRun,
    ) -> Result<StoredRun, ExecutionStoreError>;

    /// Acquires a new fence only if no current lease is active under adapter-authoritative time.
    async fn acquire_lease(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError>;

    /// Renews precisely the supplied active fence; an expired lease is never resurrected.
    async fn renew_lease(
        &self,
        owner_id: Uuid,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError>;

    async fn commit_execution(
        &self,
        owner_id: Uuid,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError>;

    async fn load_run(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<StoredRun>, ExecutionStoreError>;

    async fn load_checkpoint(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<(u64, CheckpointV1)>, ExecutionStoreError>;

    async fn load_steps_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<Step>, ExecutionStoreError>;

    async fn load_attempts_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<InvocationAttemptRecord>, ExecutionStoreError>;

    async fn load_durable_result(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError>;

    async fn replay_events(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<EventReplayPage, ExecutionStoreError>;
}

pub trait ExecutionClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

#[async_trait]
pub trait ExecutionStoreFactory: Send + Sync {
    type Store: ExecutionStore;

    async fn create_execution_store(&self) -> Result<Self::Store, ExecutionStoreError>;

    fn advance_clock(&self, duration_ms: u64) -> Result<(), ExecutionStoreError>;
}

/// Runs portable adapter checks using only the public execution-store port.
pub async fn assert_execution_store_conformance<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let session_id = Uuid::from_u128(0xfeed);
    let owner_id = Uuid::from_u128(0x000f_eed5);
    let session = Session::new(
        session_id,
        "execution-store-contract",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let run = Run::queued(
        Uuid::from_u128(0x000f_eed1),
        session_id,
        "execution-store-contract",
        1,
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                run.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let conflicting = Run::queued(
        Uuid::from_u128(0x000f_eed2),
        session_id,
        "execution-store-contract",
        1,
    )
    .map_err(ExecutionStoreError::from)?;
    match store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session,
                conflicting,
                1,
                SessionConcurrencyPolicy::Serial,
            ),
        )
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
        .acquire_lease(owner_id, run.id(), created.run_version(), 1_000)
        .await?;
    match store
        .acquire_lease(owner_id, run.id(), created.run_version(), 1_000)
        .await
    {
        Err(error) if error.code() == ExecutionStoreErrorCode::LeaseConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    let lease = store.renew_lease(owner_id, lease, 1_000).await?;
    let running = run
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let command = RuntimeCommand::start(Uuid::from_u128(0x000f_eed3), session_id, run.id())
        .map_err(ExecutionStoreError::from)?;
    let gap = vec![
        RuntimeEvent::new(
            Uuid::from_u128(0x000f_eed4),
            Uuid::from_u128(0x000f_eed5),
            session_id,
            run.id(),
            1,
            1,
            RuntimeEventKind::RunStarted,
        )
        .map_err(ExecutionStoreError::from)?,
        RuntimeEvent::new(
            Uuid::from_u128(0x000f_eed8),
            Uuid::from_u128(0x000f_eed5),
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
    match store.commit_execution(owner_id, gap_commit).await {
        Err(error) if error.code() == ExecutionStoreErrorCode::EventConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    let events = vec![
        RuntimeEvent::new(
            Uuid::from_u128(0x000f_eed6),
            Uuid::from_u128(0x000f_eed5),
            session_id,
            run.id(),
            1,
            1,
            RuntimeEventKind::RunStarted,
        )
        .map_err(ExecutionStoreError::from)?,
        RuntimeEvent::new(
            Uuid::from_u128(0x000f_eed7),
            Uuid::from_u128(0x000f_eed5),
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
    .attempts(vec![attempt.clone()])
    .cursor(Some(
        super::CheckpointCursor::new(invocation.id(), 1, "execution-store-step")
            .map_err(ExecutionStoreError::from)?,
    ))
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
    let outcome = store.commit_execution(owner_id, commit.clone()).await?;
    if replay_all_events(&store, owner_id, run.id()).await? != events
        || store.load_checkpoint(owner_id, run.id()).await? != Some((1, checkpoint))
        || store
            .load_steps_page(
                owner_id,
                run.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [step]
        || store
            .load_attempts_page(
                owner_id,
                run.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [attempt]
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let paused = running
        .transition(RunState::Paused, Some(super::RunPauseReason::Requested))
        .map_err(ExecutionStoreError::from)?;
    let paused_event = RuntimeEvent::new(
        Uuid::from_u128(0x000f_eedb),
        Uuid::from_u128(0x000f_eed5),
        session_id,
        run.id(),
        3,
        3,
        RuntimeEventKind::RunPaused,
    )
    .map_err(ExecutionStoreError::from)?;
    let stale_run = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                1,
                lease.clone(),
                RuntimeCommand::pause(
                    Uuid::from_u128(0x000f_eed9),
                    session_id,
                    run.id(),
                    super::RunPauseReason::Requested,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![paused_event.clone()],
                vec![],
                vec![],
                vec![],
                None,
                paused.clone(),
            ),
        )
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                outcome.stored_run().run_version(),
                0,
                lease.clone(),
                RuntimeCommand::pause(
                    Uuid::from_u128(0x000f_eeda),
                    session_id,
                    run.id(),
                    super::RunPauseReason::Requested,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![paused_event.clone()],
                vec![],
                vec![],
                vec![],
                None,
                paused.clone(),
            ),
        )
        .await;
    if !matches!(
        stale_checkpoint,
        Err(error) if error.code() == ExecutionStoreErrorCode::CheckpointConflict
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    if store.load_run(owner_id, run.id()).await?.as_ref() != Some(outcome.stored_run())
        || replay_all_events(&store, owner_id, run.id()).await? != events
        || store
            .load_checkpoint(owner_id, run.id())
            .await?
            .as_ref()
            .map(|(version, _)| *version)
            != Some(1)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let unchanged_checkpoint = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                outcome.stored_run().run_version(),
                1,
                lease.clone(),
                RuntimeCommand::pause(
                    Uuid::from_u128(0x000f_eedd),
                    session_id,
                    run.id(),
                    super::RunPauseReason::Requested,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![paused_event.clone()],
                vec![],
                vec![],
                vec![],
                None,
                paused.clone(),
            )
            .with_checkpoint_mutation(CheckpointMutation::Unchanged),
        )
        .await;
    if !matches!(
        unchanged_checkpoint,
        Err(error) if error.code() == ExecutionStoreErrorCode::CheckpointConflict
    ) || store.load_run(owner_id, run.id()).await?.as_ref() != Some(outcome.stored_run())
        || replay_all_events(&store, owner_id, run.id()).await? != events
        || store
            .load_checkpoint(owner_id, run.id())
            .await?
            .as_ref()
            .map(|(version, checkpoint)| (*version, checkpoint.state()))
            != Some((1, RunState::Running))
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let paused_outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                outcome.stored_run().run_version(),
                1,
                lease,
                RuntimeCommand::pause(
                    Uuid::from_u128(0x000f_eedc),
                    session_id,
                    run.id(),
                    super::RunPauseReason::Requested,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![paused_event.clone()],
                vec![],
                vec![],
                vec![],
                None,
                paused.clone(),
            ),
        )
        .await?;
    if store.commit_execution(owner_id, commit).await? != outcome
        || store
            .load_run(owner_id, run.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&paused)
        || replay_all_events(&store, owner_id, run.id()).await?
            != events
                .into_iter()
                .chain(std::iter::once(paused_event))
                .collect::<Vec<_>>()
        || paused_outcome.stored_run().run_version()
            != outcome
                .stored_run()
                .run_version()
                .checked_add(1)
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
                })?
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let first_event_page = store
        .replay_events(owner_id, run.id(), StoreReadPage::first(1)?)
        .await?;
    let second_event_page = store
        .replay_events(
            owner_id,
            run.id(),
            StoreReadPage::after(
                first_event_page.next_cursor().cloned().ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest)
                })?,
                1,
            )?,
        )
        .await?;
    if first_event_page.events().len() != 1
        || first_event_page.next_cursor().is_none()
        || second_event_page.events().len() != 1
        || second_event_page.next_cursor().is_none()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    assert_authoritative_grant_cas_contract(factory).await?;
    assert_owner_isolation_contract(factory).await?;
    assert_forged_authority_binding_contract(factory).await?;
    assert_portable_commit_bounds_contract(factory).await?;
    assert_portable_session_identity_contract(factory).await?;
    assert_portable_command_semantics_contract(factory).await?;
    assert_portable_lifecycle_transition_contract(factory).await?;
    assert_portable_append_only_history_contract(factory).await?;
    assert_keyset_history_paging_contract(factory).await?;
    assert_portable_checkpoint_rollback_contract(factory).await?;
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

async fn replay_all_events<S: ExecutionStore + ?Sized>(
    store: &S,
    owner_id: Uuid,
    run_id: Uuid,
) -> Result<Vec<RuntimeEvent>, ExecutionStoreError> {
    let mut cursor = None;
    let mut last_sequence = 0;
    let mut events = Vec::new();
    loop {
        let page = store
            .replay_events(
                owner_id,
                run_id,
                StoreReadPage::new(cursor.take(), MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?;
        if page
            .events()
            .iter()
            .any(|event| event.sequence() <= last_sequence)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::EventConflict,
            ));
        }
        events.extend_from_slice(page.events());
        if let Some(last) = page.events().last() {
            last_sequence = last.sequence();
        }
        match page.next_cursor() {
            Some(next) => cursor = Some(next.clone()),
            None => return Ok(events),
        }
    }
}

async fn create_running_run<F>(
    factory: &F,
    seed: u128,
) -> Result<
    (
        F::Store,
        Uuid,
        Session,
        Run,
        ExecutionCommitOutcome,
        ExecutionLease,
    ),
    ExecutionStoreError,
>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(seed + 1);
    let session = Session::new(
        Uuid::from_u128(seed + 2),
        "portable-lifecycle",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(seed + 3),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let started = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(Uuid::from_u128(seed + 4), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                running,
            ),
        )
        .await?;
    Ok((store, owner_id, session, queued, started, lease))
}

async fn assert_portable_terminal_transition<F>(
    factory: &F,
    seed: u128,
    kind: RuntimeCommandKind,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let (store, owner_id, session, queued, started, lease) =
        create_running_run(factory, seed).await?;
    let (command, target_state) = match kind {
        RuntimeCommandKind::Complete => (
            RuntimeCommand::complete(Uuid::from_u128(seed + 5), session.id(), queued.id()),
            RunState::Completed,
        ),
        RuntimeCommandKind::Fail => (
            RuntimeCommand::fail(Uuid::from_u128(seed + 5), session.id(), queued.id()),
            RunState::Failed,
        ),
        RuntimeCommandKind::Cancel => (
            RuntimeCommand::cancel(Uuid::from_u128(seed + 5), session.id(), queued.id()),
            RunState::Cancelled,
        ),
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    };
    let target = started
        .stored_run()
        .run()
        .transition(target_state, None)
        .map_err(ExecutionStoreError::from)?;
    let outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease,
                command.map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                target.clone(),
            ),
        )
        .await?;
    if outcome.stored_run().run() != &target
        || outcome.checkpoint().is_some()
        || store
            .load_checkpoint(owner_id, queued.id())
            .await?
            .is_some()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

fn conformance_recovery_binding(
    invocation: &LogicalInvocation,
    manifest: &ManifestPin,
    recovery_mode: RecoveryMode,
    authorization_identity: Uuid,
) -> Result<RecoveryResumeBinding, ExecutionStoreError> {
    serde_json::from_value(serde_json::json!({
        "logical_invocation_id": invocation.id(),
        "run_id": invocation.run_id(),
        "logical_step_id": invocation.logical_step_id(),
        "capability_id": invocation.capability_id(),
        "canonical_argument_digest": invocation.canonical_argument_digest(),
        "completed_attempt_number": 1,
        "retry_attempt_number": 2,
        "manifest_id": manifest.id(),
        "manifest_version": manifest.version(),
        "manifest_digest": manifest.schema_digest(),
        "recovery_mode": recovery_mode,
        "idempotency_key": invocation.idempotency_key(),
        "authorization_identity": authorization_identity,
    }))
    .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))
}

async fn assert_recovery_mode_cannot_retry<F>(
    factory: &F,
    seed: u128,
    recovery_mode: RecoveryMode,
    reason: super::RecoveryPauseReason,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let (store, owner_id, session, queued, started, lease) =
        create_running_run(factory, seed).await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "blocked-recovery",
        "workspace.write",
        1,
        serde_json::json!({"path": "blocked.txt"}),
    ))?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:blocked-recovery",
        recovery_mode,
    ))?;
    let pause = conformance_value(super::RecoveryPauseRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        reason,
    ))?;
    let recovery = conformance_value(super::RecoveryRecord::new_with_pause(pause.clone(), None))?;
    let uncertain = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Uncertain,
        manifest.clone(),
        recovery_mode,
    ))?;
    let recovery_required = started
        .stored_run()
        .run()
        .require_recovery(recovery.clone())
        .map_err(ExecutionStoreError::from)?;
    let required = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::require_recovery(
                    Uuid::from_u128(seed + 5),
                    session.id(),
                    queued.id(),
                    recovery,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![uncertain],
                vec![],
                None,
                recovery_required.clone(),
            ),
        )
        .await?;
    let forged_retry = conformance_recovery_binding(
        &invocation,
        &manifest,
        RecoveryMode::KeyedIdempotent,
        Uuid::from_u128(seed + 6),
    )?;
    let retry_attempt = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        2,
        super::AttemptRecordState::Pending,
        manifest,
        recovery_mode,
    ))?;
    let running = Run::queued(
        queued.id(),
        session.id(),
        queued.definition_id(),
        queued.definition_version(),
    )
    .and_then(|run| run.transition(RunState::Running, None))
    .map_err(ExecutionStoreError::from)?;
    let rejected = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                required.stored_run().run_version(),
                required.checkpoint_version(),
                lease,
                RuntimeCommand::resume_recovery(
                    Uuid::from_u128(seed + 7),
                    session.id(),
                    queued.id(),
                    forged_retry,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![retry_attempt],
                vec![],
                None,
                running,
            ),
        )
        .await;
    if !matches!(rejected, Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest)
        || store
            .load_run(owner_id, queued.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&recovery_required)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_portable_lifecycle_transition_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    assert_create_run_rejects_non_queued(factory).await?;
    assert_portable_terminal_transition(factory, 0xe1_00, RuntimeCommandKind::Complete).await?;
    assert_portable_terminal_transition(factory, 0xe2_00, RuntimeCommandKind::Fail).await?;
    assert_portable_terminal_transition(factory, 0xe3_00, RuntimeCommandKind::Cancel).await?;
    assert_recovery_mode_cannot_retry(
        factory,
        0xe5_00,
        RecoveryMode::Manual,
        super::RecoveryPauseReason::ManualReview,
    )
    .await?;
    assert_recovery_mode_cannot_retry(
        factory,
        0xe6_00,
        RecoveryMode::NonRetryable,
        super::RecoveryPauseReason::UncertainOutcome,
    )
    .await?;
    assert_recovery_mode_cannot_retry(
        factory,
        0xe7_00,
        RecoveryMode::Compensate,
        super::RecoveryPauseReason::UncertainOutcome,
    )
    .await?;

    let (store, owner_id, session, queued, started, lease) =
        create_running_run(factory, 0xe4_00).await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "recover-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "recover.txt"}),
    ))?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:portable-recovery",
        RecoveryMode::KeyedIdempotent,
    ))?;
    let pause = conformance_value(super::RecoveryPauseRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        super::RecoveryPauseReason::AuthoritativeAbsence,
    ))?;
    let binding = conformance_recovery_binding(
        &invocation,
        &manifest,
        RecoveryMode::KeyedIdempotent,
        Uuid::from_u128(0xe4_06),
    )?;
    let recovery = conformance_value(super::RecoveryRecord::new_with_pause(
        pause.clone(),
        Some(binding.clone()),
    ))?;
    let uncertain = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Uncertain,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let recovery_required = started
        .stored_run()
        .run()
        .require_recovery(recovery.clone())
        .map_err(ExecutionStoreError::from)?;
    let required = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::require_recovery(
                    Uuid::from_u128(0xe4_05),
                    session.id(),
                    queued.id(),
                    recovery,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![uncertain],
                vec![],
                None,
                recovery_required,
            ),
        )
        .await?;
    let retry_attempt = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        2,
        super::AttemptRecordState::Pending,
        manifest,
        RecoveryMode::KeyedIdempotent,
    ))?;
    let resumed = Run::queued(
        queued.id(),
        session.id(),
        queued.definition_id(),
        queued.definition_version(),
    )
    .and_then(|run| run.transition(RunState::Running, None))
    .map_err(ExecutionStoreError::from)?;
    let resume_command = RuntimeCommand::resume_recovery(
        Uuid::from_u128(0xe4_07),
        session.id(),
        queued.id(),
        binding,
    )
    .map_err(ExecutionStoreError::from)?;
    let resumed_outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                required.stored_run().run_version(),
                required.checkpoint_version(),
                lease,
                resume_command,
                vec![],
                vec![],
                vec![retry_attempt],
                vec![],
                None,
                resumed.clone(),
            ),
        )
        .await?;
    if resumed_outcome.stored_run().run() != &resumed {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_create_run_rejects_non_queued<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let owner_id = Uuid::from_u128(0xe0_00);
    let session = conformance_value(Session::new(
        Uuid::from_u128(0xe0_01),
        "portable-create-run",
        1,
        SessionConcurrencyPolicy::Serial,
    ))?;
    let queued = conformance_value(Run::queued(
        Uuid::from_u128(0xe0_02),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    ))?;
    let running = conformance_value(queued.transition(RunState::Running, None))?;
    let (invocation, context) = conformance_policy_context(queued.id())?;
    let grant = conformance_counted_grant(&context)?;
    let request = conformance_value(PolicyEngine::approval_request(&context, Some(&grant)))?;
    let waiting = conformance_value(running.wait_for_approval(request))?;
    let paused = conformance_value(
        running.transition(RunState::Paused, Some(super::RunPauseReason::Requested)),
    )?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        invocation.capability_id(),
        invocation.manifest_version(),
        "sha256:portable-create-run",
        RecoveryMode::Manual,
    ))?;
    let recovery_pause = conformance_value(super::RecoveryPauseRecord::new(
        invocation.binding(),
        1,
        manifest,
        super::RecoveryPauseReason::ManualReview,
    ))?;
    let recovery_required = conformance_value(running.pause_for_recovery(recovery_pause))?;
    let completed = conformance_value(running.transition(RunState::Completed, None))?;
    let failed = conformance_value(running.transition(RunState::Failed, None))?;
    let cancelled = conformance_value(running.transition(RunState::Cancelled, None))?;

    for run in [
        running,
        waiting,
        paused,
        recovery_required,
        completed,
        failed,
        cancelled,
    ] {
        let store = factory.create_execution_store().await?;
        let result = store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    owner_id,
                    session.clone(),
                    run,
                    0,
                    SessionConcurrencyPolicy::Serial,
                ),
            )
            .await;
        if !matches!(
            result,
            Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
        ) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
    }
    Ok(())
}

async fn assert_portable_commit_bounds_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(9_000);
    let session = Session::new(
        Uuid::from_u128(9_001),
        "portable-bounds",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(9_002),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 5_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let oversized_events = (0..=MAX_COMMIT_EVENTS)
        .map(|offset| {
            let sequence = u64::try_from(offset)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
                })?;
            RuntimeEvent::new(
                Uuid::from_u128(9_100 + offset as u128),
                owner_id,
                session.id(),
                queued.id(),
                sequence,
                sequence,
                RuntimeEventKind::StepStarted,
            )
            .map_err(ExecutionStoreError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let vector_error = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(Uuid::from_u128(9_003), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                oversized_events,
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await;
    if !matches!(
        vector_error,
        Err(error) if error.code() == ExecutionStoreErrorCode::BoundsExceeded
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let references = (0_u128..40_000)
        .map(|offset| {
            OpaqueReference::new(Uuid::from_u128(10_000 + offset))
                .map_err(ExecutionStoreError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let checkpoint = CheckpointV1Builder::new(
        session.id(),
        queued.id(),
        DefinitionPin::new(1, "portable-bounds", 1).map_err(ExecutionStoreError::from)?,
        1,
        vec![],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .message_context_refs(references)
    .build()
    .map_err(ExecutionStoreError::from)?;
    let event = RuntimeEvent::new(
        Uuid::from_u128(9_004),
        owner_id,
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let byte_error = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease,
                RuntimeCommand::start(Uuid::from_u128(9_005), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![event],
                vec![],
                vec![],
                vec![],
                None,
                running,
            )
            .with_checkpoint(checkpoint),
        )
        .await;
    if !matches!(
        byte_error,
        Err(error) if error.code() == ExecutionStoreErrorCode::BoundsExceeded
    ) || store.load_run(owner_id, queued.id()).await?.as_ref() != Some(&created)
        || !replay_all_events(&store, owner_id, queued.id())
            .await?
            .is_empty()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_portable_session_identity_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(50_000);
    let session_id = Uuid::from_u128(50_001);
    let definition = conformance_definition(true);
    let session = Session::new_for_definition(
        session_id,
        &definition,
        SessionConcurrencyPolicy::Concurrent,
    )
    .map_err(ExecutionStoreError::from)?;
    let first = Run::queued(
        Uuid::from_u128(50_002),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                first.clone(),
                0,
                SessionConcurrencyPolicy::Concurrent,
            ),
        )
        .await?;
    let owner_mismatch = Run::queued(
        Uuid::from_u128(50_003),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let owner_error = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                Uuid::from_u128(50_004),
                session.clone(),
                owner_mismatch.clone(),
                created.session_version(),
                SessionConcurrencyPolicy::Concurrent,
            ),
        )
        .await;
    let mut other_definition = definition;
    other_definition.id = "portable-other-definition".into();
    other_definition.version = 2;
    let other_session = Session::new_for_definition(
        session_id,
        &other_definition,
        SessionConcurrencyPolicy::Concurrent,
    )
    .map_err(ExecutionStoreError::from)?;
    let definition_mismatch = Run::queued(
        Uuid::from_u128(50_005),
        session_id,
        other_session.definition().id(),
        other_session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let definition_error = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                other_session,
                definition_mismatch.clone(),
                created.session_version(),
                SessionConcurrencyPolicy::Concurrent,
            ),
        )
        .await;
    if !matches!(
        owner_error,
        Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
    ) || !matches!(
        definition_error,
        Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
    ) || store.load_run(owner_id, first.id()).await?.as_ref() != Some(&created)
        || store
            .load_run(owner_id, owner_mismatch.id())
            .await?
            .is_some()
        || store
            .load_run(owner_id, definition_mismatch.id())
            .await?
            .is_some()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_portable_command_semantics_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(51_000);
    let session = Session::new(
        Uuid::from_u128(51_001),
        "portable-command",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(51_002),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 5_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let invalid = [
        ExecutionCommit::new(
            created.run_version(),
            0,
            lease.clone(),
            RuntimeCommand::start(
                Uuid::from_u128(51_003),
                Uuid::from_u128(51_004),
                queued.id(),
            )
            .map_err(ExecutionStoreError::from)?,
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            running.clone(),
        ),
        ExecutionCommit::new(
            created.run_version(),
            0,
            lease.clone(),
            RuntimeCommand::cancel(Uuid::from_u128(51_005), session.id(), queued.id())
                .map_err(ExecutionStoreError::from)?,
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            running,
        ),
        ExecutionCommit::new(
            created.run_version(),
            0,
            lease,
            RuntimeCommand::start(Uuid::from_u128(51_006), session.id(), queued.id())
                .map_err(ExecutionStoreError::from)?,
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            queued.clone(),
        ),
    ];
    for commit in invalid {
        match store.commit_execution(owner_id, commit).await {
            Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest => {}
            _ => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ))
            }
        }
    }
    if store.load_run(owner_id, queued.id()).await?.as_ref() != Some(&created)
        || !replay_all_events(&store, owner_id, queued.id())
            .await?
            .is_empty()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_portable_append_only_history_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(52_000);
    let session = Session::new(
        Uuid::from_u128(52_001),
        "portable-history",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(52_002),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "portable-history-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "history.txt"}),
    ))?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:portable-history",
        RecoveryMode::KeyedIdempotent,
    ))?;
    let pending = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Pending,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let step = Step::new(queued.id(), "portable-history-step", StepKind::Capability)
        .map_err(ExecutionStoreError::from)?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 5_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let event = RuntimeEvent::new(
        Uuid::from_u128(52_003),
        owner_id,
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let baseline = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(Uuid::from_u128(52_004), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![event.clone()],
                vec![step.clone()],
                vec![pending.clone()],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await?;
    let conflicting_step = Step::new(queued.id(), "portable-history-step", StepKind::Policy)
        .map_err(ExecutionStoreError::from)?;
    let step_error = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                baseline.stored_run().run_version(),
                0,
                lease.clone(),
                RuntimeCommand::record_progress(Uuid::from_u128(52_005), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![conflicting_step],
                vec![],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await;
    let completed = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Completed,
        manifest,
        RecoveryMode::KeyedIdempotent,
    ))?;
    let attempt_error = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                baseline.stored_run().run_version(),
                0,
                lease,
                RuntimeCommand::record_progress(Uuid::from_u128(52_006), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![completed],
                vec![],
                None,
                running,
            ),
        )
        .await;
    if !matches!(
        step_error,
        Err(error) if error.code() == ExecutionStoreErrorCode::HistoryConflict
    ) || !matches!(
        attempt_error,
        Err(error) if error.code() == ExecutionStoreErrorCode::HistoryConflict
    ) || store.load_run(owner_id, queued.id()).await?.as_ref() != Some(baseline.stored_run())
        || store
            .load_steps_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [step]
        || store
            .load_attempts_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [pending]
        || replay_all_events(&store, owner_id, queued.id()).await? != vec![event]
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_keyset_history_paging_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(52_100);
    let session = Session::new(
        Uuid::from_u128(52_101),
        "keyset-history",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(52_102),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:keyset-history",
        RecoveryMode::KeyedIdempotent,
    ))?;
    let make_attempt = |value: u128, step: &str| -> Result<_, ExecutionStoreError> {
        let invocation = conformance_value(LogicalInvocation::new(
            queued.id(),
            step,
            "workspace.write",
            1,
            serde_json::json!({"value": value}),
        ))?;
        conformance_value(InvocationAttemptRecord::new(
            invocation.binding(),
            1,
            super::AttemptRecordState::Pending,
            manifest.clone(),
            RecoveryMode::KeyedIdempotent,
        ))
    };
    let first_steps = vec![
        Step::new(queued.id(), "page-b", StepKind::Capability)
            .map_err(ExecutionStoreError::from)?,
        Step::new(queued.id(), "page-c", StepKind::Capability)
            .map_err(ExecutionStoreError::from)?,
    ];
    let first_attempts = vec![make_attempt(1, "page-b")?, make_attempt(2, "page-c")?];
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 5_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let baseline = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(Uuid::from_u128(52_103), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![],
                first_steps.clone(),
                first_attempts.clone(),
                vec![],
                None,
                running.clone(),
            ),
        )
        .await?;
    let step_page = store
        .load_steps_page(owner_id, queued.id(), StoreReadPage::first(1)?)
        .await?;
    let attempt_page = store
        .load_attempts_page(owner_id, queued.id(), StoreReadPage::first(1)?)
        .await?;
    let step_cursor = step_page
        .next_cursor()
        .cloned()
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
    let attempt_cursor = attempt_page
        .next_cursor()
        .cloned()
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
    if step_page.items() != &first_steps[..1] || attempt_page.items() != &first_attempts[..1] {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                baseline.stored_run().run_version(),
                0,
                store.renew_lease(owner_id, lease, 5_000).await?,
                RuntimeCommand::record_progress(Uuid::from_u128(52_104), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![Step::new(queued.id(), "page-a", StepKind::Capability)
                    .map_err(ExecutionStoreError::from)?],
                vec![make_attempt(3, "page-a")?],
                vec![],
                None,
                running,
            ),
        )
        .await?;
    let remaining_steps = store
        .load_steps_page(
            owner_id,
            queued.id(),
            StoreReadPage::after(step_cursor.clone(), 2)?,
        )
        .await?;
    let remaining_attempts = store
        .load_attempts_page(
            owner_id,
            queued.id(),
            StoreReadPage::after(attempt_cursor, 2)?,
        )
        .await?;
    let other_owner = Uuid::from_u128(52_105);
    store
        .create_run(
            other_owner,
            CreateRun::new_for_owner(
                other_owner,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let other_session = Session::new(
        Uuid::from_u128(52_106),
        "keyset-history-other-run",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let other_run = Run::queued(
        Uuid::from_u128(52_107),
        other_session.id(),
        other_session.definition().id(),
        other_session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                other_session,
                other_run.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let malformed = StoreReadCursor::from_opaque("not-a-store-cursor")?;
    if remaining_steps.items() != &first_steps[1..]
        || remaining_steps.next_cursor().is_some()
        || remaining_attempts.items() != &first_attempts[1..]
        || remaining_attempts.next_cursor().is_some()
        || !matches!(
            store
                .load_attempts_page(
                    owner_id,
                    queued.id(),
                    StoreReadPage::after(step_cursor.clone(), 1)?,
                )
                .await,
            Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
        )
        || !matches!(
            store
                .load_steps_page(
                    other_owner,
                    queued.id(),
                    StoreReadPage::after(step_cursor.clone(), 1)?,
                )
                .await,
            Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
        )
        || !matches!(
            store
                .load_steps_page(
                    owner_id,
                    other_run.id(),
                    StoreReadPage::after(step_cursor, 1)?,
                )
                .await,
            Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
        )
        || !matches!(
            store
                .load_steps_page(
                    owner_id,
                    queued.id(),
                    StoreReadPage::after(malformed, 1)?,
                )
                .await,
            Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
        )
        || StoreReadPage::first(0).is_ok()
        || StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE + 1).is_ok()
        || StoreReadCursor::from_opaque("x".repeat(MAX_STORE_READ_CURSOR_BYTES + 1)).is_ok()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_portable_checkpoint_rollback_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(53_000);
    let session = Session::new(
        Uuid::from_u128(53_001),
        "portable-checkpoint",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(53_002),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 5_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let baseline_event = RuntimeEvent::new(
        Uuid::from_u128(53_003),
        owner_id,
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let baseline = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(Uuid::from_u128(53_004), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![baseline_event.clone()],
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "portable-checkpoint-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "checkpoint.txt"}),
    ))?;
    let manifest = conformance_value(ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:portable-checkpoint",
        RecoveryMode::KeyedIdempotent,
    ))?;
    let pending = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Pending,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let step = Step::new(
        queued.id(),
        "portable-checkpoint-step",
        StepKind::Capability,
    )
    .map_err(ExecutionStoreError::from)?;
    let checkpoint = CheckpointV1Builder::new(
        session.id(),
        queued.id(),
        DefinitionPin::new(1, "portable-checkpoint", 1).map_err(ExecutionStoreError::from)?,
        2,
        vec![manifest],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .build()
    .map_err(ExecutionStoreError::from)?;
    let candidate_event = RuntimeEvent::new(
        Uuid::from_u128(53_005),
        owner_id,
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::StepStarted,
    )
    .map_err(ExecutionStoreError::from)?;
    let failed = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                baseline.stored_run().run_version(),
                0,
                lease,
                RuntimeCommand::record_progress(Uuid::from_u128(53_006), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![candidate_event],
                vec![step],
                vec![pending],
                vec![],
                None,
                running,
            )
            .with_checkpoint(checkpoint),
        )
        .await;
    if !matches!(
        failed,
        Err(error) if error.code() == ExecutionStoreErrorCode::CheckpointConflict
    ) || store.load_run(owner_id, queued.id()).await?.as_ref() != Some(baseline.stored_run())
        || store
            .load_checkpoint(owner_id, queued.id())
            .await?
            .is_some()
        || !store
            .load_steps_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .is_empty()
        || !store
            .load_attempts_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .is_empty()
        || replay_all_events(&store, owner_id, queued.id()).await? != vec![baseline_event]
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_authoritative_grant_cas_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(0x60_001);
    let (_, context) = conformance_policy_context(Uuid::from_u128(0x60_002))?;
    let mut grant = conformance_counted_grant(&context)?;
    grant.id = "execution-store-authoritative-grant".into();
    grant.remaining_uses = Some(2);
    let created = AuthoritativeGrantState::from_grant(owner_id, &grant)?;
    if store
        .apply_authoritative_grant(owner_id, AuthoritativeGrantChange::create(created.clone()))
        .await?
        != created
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    match store
        .apply_authoritative_grant(owner_id, AuthoritativeGrantChange::create(created))
        .await
    {
        Err(error) if error.code() == ExecutionStoreErrorCode::VersionConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    grant.revision = 2;
    grant.remaining_uses = Some(3);
    let upgraded = AuthoritativeGrantState::from_grant(owner_id, &grant)?;
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::update(1, upgraded.clone())?,
        )
        .await?;
    match store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::update(1, {
                let mut stale = grant.clone();
                stale.revision = 3;
                stale.remaining_uses = Some(4);
                AuthoritativeGrantState::from_grant(owner_id, &stale)?
            })?,
        )
        .await
    {
        Err(error) if error.code() == ExecutionStoreErrorCode::VersionConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    let revoked = store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::revoke(upgraded.authority_key().clone(), 2)?,
        )
        .await?;
    if revoked.status() != AuthoritativeGrantStatus::Revoked
        || store
            .load_authoritative_grant(owner_id, upgraded.authority_key())
            .await?
            != Some(revoked)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_owner_isolation_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_a = Uuid::from_u128(0xb0_1a);
    let owner_b = Uuid::from_u128(0xb0_1b);
    let (_, context) = conformance_policy_context(Uuid::from_u128(0xb0_10))?;
    let shared_grant = conformance_counted_grant(&context)?;
    let state_a = AuthoritativeGrantState::from_grant(owner_a, &shared_grant)?;
    let state_b = AuthoritativeGrantState::from_grant(owner_b, &shared_grant)?;
    store
        .apply_authoritative_grant(owner_a, AuthoritativeGrantChange::create(state_a.clone()))
        .await?;
    if store
        .load_authoritative_grant(owner_b, state_a.authority_key())
        .await?
        .is_some()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    store
        .apply_authoritative_grant(owner_b, AuthoritativeGrantChange::create(state_b.clone()))
        .await?;
    if store
        .load_authoritative_grant(owner_a, state_a.authority_key())
        .await?
        != Some(state_a)
        || store
            .load_authoritative_grant(owner_b, state_b.authority_key())
            .await?
            != Some(state_b)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }

    let session = Session::new(
        Uuid::from_u128(0xb0_11),
        "owner-isolation",
        1,
        SessionConcurrencyPolicy::Serial,
    )
    .map_err(ExecutionStoreError::from)?;
    let queued = Run::queued(
        Uuid::from_u128(0xb0_12),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_a,
            CreateRun::new_for_owner(
                owner_a,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    if created.owner_id() != owner_a || store.load_run(owner_b, queued.id()).await?.is_some() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    if !matches!(
        store
            .acquire_lease(owner_b, queued.id(), created.run_version(), 1_000)
            .await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let lease = store
        .acquire_lease(owner_a, queued.id(), created.run_version(), 1_000)
        .await?;
    if !matches!(
        store.renew_lease(owner_b, lease.clone(), 1_000).await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let commit = ExecutionCommit::new(
        created.run_version(),
        0,
        lease,
        RuntimeCommand::start(Uuid::from_u128(0xb0_13), session.id(), queued.id())
            .map_err(ExecutionStoreError::from)?,
        vec![],
        vec![],
        vec![],
        vec![],
        None,
        running,
    );
    if !matches!(
        store.commit_execution(owner_b, commit).await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) || !matches!(
        store.load_checkpoint(owner_b, queued.id()).await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) || !matches!(
        store
            .load_steps_page(owner_b, queued.id(), StoreReadPage::first(1)?)
            .await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) || !matches!(
        store
            .load_attempts_page(owner_b, queued.id(), StoreReadPage::first(1)?)
            .await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) || !matches!(
        store
            .load_durable_result(owner_b, queued.id(), Uuid::from_u128(0xb0_14))
            .await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) || !matches!(
        store
            .replay_events(owner_b, queued.id(), StoreReadPage::first(1)?)
            .await,
        Err(error) if error.code() == ExecutionStoreErrorCode::NotFound
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let same_identifiers_other_owner = store
        .create_run(
            owner_b,
            CreateRun::new_for_owner(
                owner_b,
                session,
                queued,
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    if same_identifiers_other_owner.owner_id() != owner_b {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_forged_authority_binding_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    for mutation in 0_u128..3 {
        let store = factory.create_execution_store().await?;
        let owner_id = Uuid::from_u128(0xb0_20 + mutation);
        let session = Session::new_for_definition(
            Uuid::from_u128(0xb0_30 + mutation),
            &conformance_definition(true),
            SessionConcurrencyPolicy::Serial,
        )
        .map_err(ExecutionStoreError::from)?;
        let run_id = Uuid::from_u128(0xb0_40 + mutation);
        let (invocation, context) = conformance_policy_context(run_id)?;
        let grant = conformance_counted_grant(&context)?;
        let mut forged_authority = grant.clone();
        match mutation {
            0 => forged_authority.scope.workspace_id = "forged-workspace".into(),
            1 => forged_authority.effect = GrantEffect::AutoAllow,
            _ => forged_authority.valid_until_ms = Some(1_500),
        }
        let authority = AuthoritativeGrantState::from_grant(owner_id, &forged_authority)?;
        store
            .apply_authoritative_grant(owner_id, AuthoritativeGrantChange::create(authority))
            .await?;
        let (waiting, target, command, approval) = conformance_approval_resume(
            &session,
            run_id,
            Uuid::from_u128(0xb0_50 + mutation),
            &invocation,
            &context,
            &grant,
        )?;
        let (created, lease) = create_waiting_run(
            &store,
            owner_id,
            &session,
            &waiting,
            0,
            SessionConcurrencyPolicy::Serial,
            Uuid::from_u128(0xb0_70 + mutation * 2),
            Uuid::from_u128(0xb0_71 + mutation * 2),
        )
        .await?;
        let event = RuntimeEvent::new(
            Uuid::from_u128(0xb0_60 + mutation),
            owner_id,
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
            vec![event],
            vec![],
            vec![],
            vec![],
            Some(approval),
            target,
        );
        if !matches!(
            store.commit_execution(owner_id, commit).await,
            Err(error) if error.code() == ExecutionStoreErrorCode::GrantConflict
        ) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
    }
    Ok(())
}

async fn create_waiting_run<S: ExecutionStore>(
    store: &S,
    owner_id: Uuid,
    session: &Session,
    waiting: &Run,
    expected_session_version: u64,
    concurrency_policy: SessionConcurrencyPolicy,
    start_command_id: Uuid,
    approval_command_id: Uuid,
) -> Result<(StoredRun, ExecutionLease), ExecutionStoreError> {
    let queued = Run::queued(
        waiting.id(),
        waiting.session_id(),
        waiting.definition_id(),
        waiting.definition_version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                queued.clone(),
                expected_session_version,
                concurrency_policy,
            ),
        )
        .await?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
        .await?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let started = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(start_command_id, session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await?;
    let request = waiting
        .pending_approval()
        .cloned()
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
    let waiting_outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::request_approval(
                    approval_command_id,
                    session.id(),
                    queued.id(),
                    request,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                waiting.clone(),
            ),
        )
        .await?;
    Ok((waiting_outcome.stored_run().clone(), lease))
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
    let owner_id = Uuid::from_u128(0xc1);
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
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                session.id(),
                session.clone(),
                first,
                0,
                SessionConcurrencyPolicy::Concurrent,
            ),
        )
        .await?;
    let two = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                session.id(),
                session,
                second,
                one.session_version(),
                SessionConcurrencyPolicy::Concurrent,
            ),
        )
        .await?;
    if two.session_version()
        != one
            .session_version()
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?
    {
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
    let owner_id = Uuid::from_u128(0xca_10);
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
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                session.id(),
                session.clone(),
                first.clone(),
                0,
                SessionConcurrencyPolicy::Concurrent,
            ),
        )
        .await?;
    let stale = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                session.id(),
                session,
                second.clone(),
                0,
                SessionConcurrencyPolicy::Concurrent,
            ),
        )
        .await;
    if !matches!(
        stale,
        Err(error) if error.code() == ExecutionStoreErrorCode::VersionConflict
    ) || store.load_run(owner_id, second.id()).await?.is_some()
        || store.load_run(owner_id, first.id()).await?.is_none()
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
    let owner_id = Uuid::from_u128(0x1e_a3);
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
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                Uuid::from_u128(0x1e_a3),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let expired = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 100)
        .await?;
    factory.advance_clock(250)?;
    if !matches!(
        store.renew_lease(owner_id, expired.clone(), 1_000).await,
        Err(error) if error.code() == ExecutionStoreErrorCode::LeaseExpired
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let current = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
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
        store.commit_execution(owner_id, stale_commit).await,
        Err(error) if error.code() == ExecutionStoreErrorCode::LeaseExpired
    ) || !replay_all_events(&store, owner_id, queued.id())
        .await?
        .is_empty()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
        .await?;
    if replay_all_events(&store, owner_id, queued.id()).await? != events {
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
    let owner_id = Uuid::from_u128(0x5e_13);
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
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                Uuid::from_u128(0x5e_13),
                session.clone(),
                first.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let lease = store
        .acquire_lease(owner_id, first.id(), created.run_version(), 1_000)
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                running_outcome.stored_run().run_version(),
                0,
                store.renew_lease(owner_id, lease, 1_000).await?,
                RuntimeCommand::cancel(Uuid::from_u128(0x005e_0016), session.id(), first.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![cancelled_event],
                vec![],
                vec![],
                vec![],
                None,
                cancelled.clone(),
            ),
        )
        .await?;
    let next = Run::queued(
        Uuid::from_u128(0x5e_17),
        session.id(),
        session.definition().id(),
        session.definition().version(),
    )
    .map_err(ExecutionStoreError::from)?;
    let next_stored = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                Uuid::from_u128(0x5e_13),
                session,
                next.clone(),
                terminal.stored_run().session_version(),
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    if store
        .load_run(owner_id, first.id())
        .await?
        .as_ref()
        .map(StoredRun::run)
        != Some(&cancelled)
        || store
            .load_run(owner_id, next.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&next)
        || next_stored.session_version()
            != terminal
                .stored_run()
                .session_version()
                .checked_add(1)
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
                })?
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
    let owner_id = Uuid::from_u128(0xa9_14);
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
    let (created, lease) = create_waiting_run(
        &store,
        owner_id,
        &session,
        &waiting,
        0,
        SessionConcurrencyPolicy::Serial,
        Uuid::from_u128(0xa9_15),
        Uuid::from_u128(0x00a9_0016),
    )
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
    match store.commit_execution(owner_id, commit.clone()).await {
        Err(error) if error.code() == ExecutionStoreErrorCode::GrantConflict => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::create(AuthoritativeGrantState::from_grant(
                owner_id, &grant,
            )?),
        )
        .await?;
    let outcome = store.commit_execution(owner_id, commit.clone()).await?;
    if store.commit_execution(owner_id, commit).await? != outcome
        || outcome.grant_consumption().is_some()
        || store
            .load_run(owner_id, run_id)
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&target)
        || replay_all_events(&store, owner_id, run_id).await? != vec![event]
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
    let owner_id = Uuid::from_u128(0xcc_14);
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
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                Uuid::from_u128(0xcc_14),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
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
    let original_outcome = store.commit_execution(owner_id, original.clone()).await?;
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
        .await;
    if !matches!(
        conflict,
        Err(error) if error.code() == ExecutionStoreErrorCode::CommandConflict
    ) || store.commit_execution(owner_id, original).await? != original_outcome
        || store.load_run(owner_id, queued.id()).await?.as_ref()
            != Some(original_outcome.stored_run())
        || replay_all_events(&store, owner_id, queued.id()).await? != vec![event]
        || store
            .load_checkpoint(owner_id, queued.id())
            .await?
            .is_some()
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
    let owner_id = Uuid::from_u128(0xaf_14);
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
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                Uuid::from_u128(0xaf_14),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
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
        format!("jcs-v1:{}", "a".repeat(64)),
        format!("sha256:{}", "c".repeat(64)),
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
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
        Uuid::from_u128(0x00af_0016),
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
        format!("jcs-v1:{}", "b".repeat(64)),
        format!("sha256:{}", "c".repeat(64)),
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let failed = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                baseline.stored_run().run_version(),
                0,
                store.renew_lease(owner_id, lease, 1_000).await?,
                RuntimeCommand::record_progress(
                    Uuid::from_u128(0xaf_17),
                    session.id(),
                    queued.id(),
                )
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
    ) || store.load_run(owner_id, queued.id()).await?.as_ref() != Some(baseline.stored_run())
        || store
            .load_checkpoint(owner_id, queued.id())
            .await?
            .is_some()
        || !store
            .load_steps_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .is_empty()
        || store
            .load_attempts_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [baseline_attempt]
        || replay_all_events(&store, owner_id, queued.id()).await? != vec![baseline_event]
        || store
            .load_durable_result(owner_id, queued.id(), invocation.id())
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
    CapabilityManifest::new(crate::CapabilityManifestInput {
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
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    })
    .expect("execution-store conformance manifest must be valid")
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
        std::slice::from_ref(grant),
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
    let owner_id = Uuid::from_u128(0xc0_27);
    let session = conformance_value(Session::new_for_definition(
        Uuid::from_u128(0xc0_20),
        &conformance_definition(true),
        SessionConcurrencyPolicy::Concurrent,
    ))?;
    let (first_invocation, first_context) = conformance_policy_context(Uuid::from_u128(0xc0_21))?;
    let (second_invocation, second_context) = conformance_policy_context(Uuid::from_u128(0xc0_22))?;
    let grant = conformance_counted_grant(&first_context)?;
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::create(AuthoritativeGrantState::from_grant(
                owner_id, &grant,
            )?),
        )
        .await?;
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
    let (first_created, first_lease) = create_waiting_run(
        &store,
        owner_id,
        &session,
        &first_waiting,
        0,
        SessionConcurrencyPolicy::Concurrent,
        Uuid::from_u128(0xc0_28),
        Uuid::from_u128(0xc0_29),
    )
    .await?;
    let (second_created, second_lease) = create_waiting_run(
        &store,
        owner_id,
        &session,
        &second_waiting,
        first_created.session_version(),
        SessionConcurrencyPolicy::Concurrent,
        Uuid::from_u128(0xc0_2a),
        Uuid::from_u128(0xc0_2b),
    )
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
        Uuid::from_u128(0xc0_27),
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
    let first_consumption = first_commit
        .approval()
        .and_then(ApprovalGrantMutation::grant_consumption)
        .cloned();
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
    let second_consumption = second_commit
        .approval()
        .and_then(ApprovalGrantMutation::grant_consumption)
        .cloned();
    let (first_result, second_result) = futures::join!(
        store.commit_execution(owner_id, first_commit.clone()),
        store.commit_execution(owner_id, second_commit.clone())
    );
    let (winning_commit, original_outcome, expected_consumption, losing_waiting) =
        match (first_result, second_result) {
            (Ok(outcome), Err(error)) if error.code() == ExecutionStoreErrorCode::GrantConflict => {
                (first_commit, outcome, first_consumption, second_waiting)
            }
            (Err(error), Ok(outcome)) if error.code() == ExecutionStoreErrorCode::GrantConflict => {
                (second_commit, outcome, second_consumption, first_waiting)
            }
            _ => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ))
            }
        };
    if store.commit_execution(owner_id, winning_commit).await? != original_outcome
        || original_outcome.grant_consumption() != expected_consumption.as_ref()
        || store
            .load_run(owner_id, losing_waiting.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&losing_waiting)
        || !replay_all_events(&store, owner_id, losing_waiting.id())
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
    let owner_id = Uuid::from_u128(0xd0_14);
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
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                Uuid::from_u128(0xd0_14),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
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
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let cross_run_invocation = conformance_value(LogicalInvocation::new(
        Uuid::from_u128(0xd0_20),
        "cross-run-result-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "other.txt"}),
    ))?;
    let cross_run_completed = conformance_value(CompletedInvocationRecord::new(
        cross_run_invocation.binding(),
        1,
        manifest,
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let result = conformance_value(DurableCapabilityResult::new(
        result_reference.clone(),
        format!("jcs-v1:{}", "d".repeat(64)),
        format!("sha256:{}", "e".repeat(64)),
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let running = queued
        .transition(RunState::Running, None)
        .map_err(ExecutionStoreError::from)?;
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
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
    for (command_id, completed_without_lineage, attempts) in [
        (Uuid::from_u128(0xd0_21), completed.clone(), vec![]),
        (
            Uuid::from_u128(0xd0_22),
            cross_run_completed,
            vec![attempt.clone()],
        ),
    ] {
        let rejected = store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    created.run_version(),
                    0,
                    lease.clone(),
                    RuntimeCommand::start(command_id, session.id(), queued.id())
                        .map_err(ExecutionStoreError::from)?,
                    vec![first_event.clone()],
                    vec![],
                    attempts,
                    vec![DurableResultMutation::new(
                        completed_without_lineage,
                        result.clone(),
                    )],
                    None,
                    running.clone(),
                ),
            )
            .await;
        if !matches!(
            rejected,
            Err(error) if error.code() == ExecutionStoreErrorCode::LineageConflict
        ) || store.load_run(owner_id, queued.id()).await?.as_ref() != Some(&created)
            || !replay_all_events(&store, owner_id, queued.id())
                .await?
                .is_empty()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
    }
    let first_outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
        .await?;
    if store
        .load_durable_result(owner_id, queued.id(), invocation.id())
        .await?
        .as_ref()
        != Some(&result)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let renewed = store.renew_lease(owner_id, lease, 1_000).await?;
    let second_event = RuntimeEvent::new(
        Uuid::from_u128(0x00d0_0016),
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
        RuntimeCommand::record_progress(Uuid::from_u128(0xd0_17), session.id(), queued.id())
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
    let identical_outcome = store
        .commit_execution(owner_id, identical_commit.clone())
        .await?;
    if store.commit_execution(owner_id, identical_commit).await? != identical_outcome
        || store
            .load_durable_result(owner_id, queued.id(), invocation.id())
            .await?
            .as_ref()
            != Some(&result)
        || store
            .load_attempts_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [attempt]
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let before_conflict = store.load_run(owner_id, queued.id()).await?;
    let conflicting_result = conformance_value(DurableCapabilityResult::new(
        result_reference,
        format!("jcs-v1:{}", "f".repeat(64)),
        format!("sha256:{}", "e".repeat(64)),
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                identical_outcome.stored_run().run_version(),
                0,
                store.renew_lease(owner_id, renewed, 1_000).await?,
                RuntimeCommand::record_progress(
                    Uuid::from_u128(0xd0_19),
                    session.id(),
                    queued.id(),
                )
                .map_err(ExecutionStoreError::from)?,
                vec![conflicting_event],
                vec![],
                vec![],
                vec![DurableResultMutation::new(completed, conflicting_result)],
                None,
                running,
            ),
        )
        .await;
    if !matches!(
        conflict,
        Err(error) if error.code() == ExecutionStoreErrorCode::ResultConflict
    ) || store.load_run(owner_id, queued.id()).await? != before_conflict
        || store
            .load_durable_result(owner_id, queued.id(), invocation.id())
            .await?
            .as_ref()
            != Some(&result)
        || replay_all_events(&store, owner_id, queued.id())
            .await?
            .len()
            != 2
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}
