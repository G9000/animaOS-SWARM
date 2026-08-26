use futures::task::AtomicWaker;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Included};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::lock::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AuthoritativeGrantChange, AuthoritativeGrantState, AuthoritativePolicyChange,
    AuthoritativePolicyState, CheckpointMutation, CreateRun, ExecutionClock, ExecutionCommit,
    ExecutionCommitOutcome, ExecutionLease, ExecutionStore, ExecutionStoreError,
    ExecutionStoreErrorCode, RetryRun, RetryRunOutcome, RuntimeEvent, SessionConcurrencyPolicy,
    StoredRun,
};
use crate::{CommandReceipt, DurableCapabilityResult, RunState};

#[derive(Clone)]
struct StoredSession {
    version: u64,
    owner_id: Uuid,
    definition: super::DefinitionPin,
    policy: SessionConcurrencyPolicy,
}

struct RunAggregate {
    stored: StoredRun,
    lease: Option<ExecutionLease>,
    checkpoint_version: u64,
    checkpoint: Option<Arc<super::CheckpointV1>>,
    events: Vec<RuntimeEvent>,
    steps: BTreeMap<String, super::Step>,
    step_order: BTreeMap<u64, String>,
    next_step_sequence: u64,
    attempts: BTreeMap<(Uuid, u32), super::InvocationAttemptRecord>,
    attempt_order: BTreeMap<u64, (Uuid, u32)>,
    next_attempt_sequence: u64,
    attempts_fingerprint: super::checkpoint::HistoryFingerprint,
    results: BTreeMap<Uuid, StoredDurableResult>,
    completed_fingerprint: super::checkpoint::HistoryFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredDurableResult {
    completed: super::CompletedInvocationRecord,
    result: DurableCapabilityResult,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<(Uuid, Uuid), StoredSession>,
    runs: BTreeMap<(Uuid, Uuid), RunAggregate>,
    serial_claims: BTreeMap<(Uuid, Uuid), Uuid>,
    receipts: BTreeMap<(Uuid, Uuid), CommandReceipt>,
    commands: BTreeMap<(Uuid, Uuid), CommandIndexRecord>,
    outcomes: BTreeMap<(Uuid, Uuid), ExecutionCommitOutcome>,
    authoritative_grants: BTreeMap<(Uuid, String), AuthoritativeGrantState>,
    authoritative_policies: BTreeMap<(Uuid, String, u32), AuthoritativePolicyState>,
    grant_consumptions: BTreeSet<(Uuid, String, u32, Uuid)>,
    approvals: BTreeMap<(Uuid, Uuid), ApprovalIndexRecord>,
}

/// Canonical scalar command index state.
///
/// Raw command payloads remain transient. This sealed record binds the durable idempotency
/// receipt to its exact session, run, command kind, and payload digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandIndexRecord {
    session_id: Uuid,
    run_id: Uuid,
    kind: super::RuntimeCommandKind,
    payload_digest: Uuid,
}

/// Canonical scalar approval index state.
///
/// The full request/decision remains in the run/checkpoint and command validation paths. This
/// record intentionally duplicates only linkage and timestamp scalars, so it can be sealed and
/// projected without introducing a second persistence copy of policy identity or restrictions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalIndexRecord {
    run_id: Uuid,
    requested_at_ms: i64,
    expires_at_ms: i64,
    decision_command_id: Option<Uuid>,
    decision_kind: Option<crate::ApprovalDecisionKind>,
    decided_at_ms: Option<i64>,
}

/// A non-serializable, safe-to-index view of durable execution state.
///
/// This deliberately contains identifiers, enums, versions, and digests only. It never exposes
/// normalized arguments, provider transcripts, checkpoint bodies, event payloads, or command
/// payloads. Durable adapters can use it to maintain query indexes while the authenticated
/// execution-store snapshot remains the recovery authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionStoreProjection {
    definition_pins: Vec<ExecutionDefinitionPinProjection>,
    sessions: Vec<ExecutionSessionProjection>,
    runs: Vec<ExecutionRunProjection>,
    serial_claims: Vec<ExecutionSerialClaimProjection>,
    leases: Vec<ExecutionLeaseProjection>,
    events: Vec<ExecutionEventProjection>,
    steps: Vec<ExecutionStepProjection>,
    attempts: Vec<ExecutionAttemptProjection>,
    logical_invocations: Vec<ExecutionLogicalInvocationProjection>,
    durable_results: Vec<ExecutionDurableResultProjection>,
    commands: Vec<ExecutionCommandProjection>,
    receipts: Vec<ExecutionReceiptProjection>,
    outcomes: Vec<ExecutionOutcomeProjection>,
    checkpoints: Vec<ExecutionCheckpointProjection>,
    policies: Vec<ExecutionPolicyProjection>,
    grants: Vec<ExecutionGrantProjection>,
    grant_consumptions: Vec<ExecutionGrantConsumptionProjection>,
    approvals: Vec<ExecutionApprovalProjection>,
    decisions: Vec<ExecutionDecisionProjection>,
}

impl ExecutionStoreProjection {
    pub fn definition_pins(&self) -> &[ExecutionDefinitionPinProjection] {
        &self.definition_pins
    }
    pub fn sessions(&self) -> &[ExecutionSessionProjection] {
        &self.sessions
    }
    pub fn runs(&self) -> &[ExecutionRunProjection] {
        &self.runs
    }
    pub fn serial_claims(&self) -> &[ExecutionSerialClaimProjection] {
        &self.serial_claims
    }
    pub fn leases(&self) -> &[ExecutionLeaseProjection] {
        &self.leases
    }
    pub fn events(&self) -> &[ExecutionEventProjection] {
        &self.events
    }
    pub fn steps(&self) -> &[ExecutionStepProjection] {
        &self.steps
    }
    pub fn attempts(&self) -> &[ExecutionAttemptProjection] {
        &self.attempts
    }
    pub fn logical_invocations(&self) -> &[ExecutionLogicalInvocationProjection] {
        &self.logical_invocations
    }
    pub fn durable_results(&self) -> &[ExecutionDurableResultProjection] {
        &self.durable_results
    }
    pub fn commands(&self) -> &[ExecutionCommandProjection] {
        &self.commands
    }
    pub fn receipts(&self) -> &[ExecutionReceiptProjection] {
        &self.receipts
    }
    pub fn outcomes(&self) -> &[ExecutionOutcomeProjection] {
        &self.outcomes
    }
    pub fn checkpoints(&self) -> &[ExecutionCheckpointProjection] {
        &self.checkpoints
    }
    pub fn policies(&self) -> &[ExecutionPolicyProjection] {
        &self.policies
    }
    pub fn grants(&self) -> &[ExecutionGrantProjection] {
        &self.grants
    }
    pub fn grant_consumptions(&self) -> &[ExecutionGrantConsumptionProjection] {
        &self.grant_consumptions
    }
    pub fn approvals(&self) -> &[ExecutionApprovalProjection] {
        &self.approvals
    }
    pub fn decisions(&self) -> &[ExecutionDecisionProjection] {
        &self.decisions
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionCommandProjection {
    owner_id: Uuid,
    command_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
    kind: super::RuntimeCommandKind,
    payload_digest: Uuid,
}
impl ExecutionCommandProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn kind(&self) -> super::RuntimeCommandKind {
        self.kind
    }
    pub fn payload_digest(&self) -> Uuid {
        self.payload_digest
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionApprovalProjection {
    owner_id: Uuid,
    id: Uuid,
    run_id: Uuid,
    decision_id: Option<Uuid>,
    requested_at_ms: i64,
    expires_at_ms: i64,
}
impl ExecutionApprovalProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn decision_id(&self) -> Option<Uuid> {
        self.decision_id
    }
    pub fn requested_at_ms(&self) -> i64 {
        self.requested_at_ms
    }
    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionDecisionProjection {
    owner_id: Uuid,
    id: Uuid,
    approval_id: Uuid,
    claimed: bool,
    kind: crate::ApprovalDecisionKind,
    decided_at_ms: i64,
}
impl ExecutionDecisionProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn approval_id(&self) -> Uuid {
        self.approval_id
    }
    pub fn claimed(&self) -> bool {
        self.claimed
    }
    pub fn kind(&self) -> crate::ApprovalDecisionKind {
        self.kind
    }
    pub fn decided_at_ms(&self) -> i64 {
        self.decided_at_ms
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionDefinitionPinProjection {
    owner_id: Uuid,
    id: String,
    version: u32,
    schema_version: u32,
}
impl ExecutionDefinitionPinProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionSessionProjection {
    owner_id: Uuid,
    session_id: Uuid,
    definition_id: String,
    definition_version: u32,
    definition_schema_version: u32,
    version: u64,
    concurrency: SessionConcurrencyPolicy,
}
impl ExecutionSessionProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }
    pub fn definition_version(&self) -> u32 {
        self.definition_version
    }
    pub fn definition_schema_version(&self) -> u32 {
        self.definition_schema_version
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn concurrency(&self) -> SessionConcurrencyPolicy {
        self.concurrency
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionRunProjection {
    owner_id: Uuid,
    run_id: Uuid,
    session_id: Uuid,
    definition_id: String,
    definition_version: u32,
    state: RunState,
    run_version: u64,
    session_version: u64,
    checkpoint_version: u64,
}
impl ExecutionRunProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }
    pub fn definition_version(&self) -> u32 {
        self.definition_version
    }
    pub fn state(&self) -> RunState {
        self.state
    }
    pub fn run_version(&self) -> u64 {
        self.run_version
    }
    pub fn session_version(&self) -> u64 {
        self.session_version
    }
    pub fn checkpoint_version(&self) -> u64 {
        self.checkpoint_version
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionSerialClaimProjection {
    owner_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
}
impl ExecutionSerialClaimProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionLeaseProjection {
    owner_id: Uuid,
    run_id: Uuid,
    fence: Uuid,
    expires_at_ms: u64,
}
impl ExecutionLeaseProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn fence(&self) -> Uuid {
        self.fence
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionEventProjection {
    owner_id: Uuid,
    event_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
    schema_version: u32,
    timestamp_ms: u64,
    sequence: u64,
    kind: super::RuntimeEventKind,
}
impl ExecutionEventProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn event_id(&self) -> Uuid {
        self.event_id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn kind(&self) -> super::RuntimeEventKind {
        self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionStepProjection {
    owner_id: Uuid,
    run_id: Uuid,
    logical_step_id: String,
    sequence: u64,
    kind: super::StepKind,
}
impl ExecutionStepProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn logical_step_id(&self) -> &str {
        &self.logical_step_id
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn kind(&self) -> super::StepKind {
        self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionLogicalInvocationProjection {
    owner_id: Uuid,
    id: Uuid,
    run_id: Uuid,
    logical_step_id: String,
    capability_id: String,
    manifest_version: u32,
    canonical_argument_digest: Uuid,
    idempotency_key: String,
}
impl ExecutionLogicalInvocationProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn logical_step_id(&self) -> &str {
        &self.logical_step_id
    }
    pub fn capability_id(&self) -> &str {
        &self.capability_id
    }
    pub fn manifest_version(&self) -> u32 {
        self.manifest_version
    }
    pub fn canonical_argument_digest(&self) -> Uuid {
        self.canonical_argument_digest
    }
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionAttemptProjection {
    owner_id: Uuid,
    run_id: Uuid,
    logical_invocation_id: Uuid,
    attempt_number: u32,
    sequence: u64,
    state: super::AttemptRecordState,
    manifest_id: String,
    manifest_version: u32,
    manifest_schema_digest: String,
    recovery_mode: crate::RecoveryMode,
}
impl ExecutionAttemptProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn logical_invocation_id(&self) -> Uuid {
        self.logical_invocation_id
    }
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn state(&self) -> super::AttemptRecordState {
        self.state
    }
    pub fn manifest_id(&self) -> &str {
        &self.manifest_id
    }
    pub fn manifest_version(&self) -> u32 {
        self.manifest_version
    }
    pub fn manifest_schema_digest(&self) -> &str {
        &self.manifest_schema_digest
    }
    pub fn recovery_mode(&self) -> crate::RecoveryMode {
        self.recovery_mode
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionDurableResultProjection {
    owner_id: Uuid,
    run_id: Uuid,
    logical_invocation_id: Uuid,
    attempt_number: u32,
    result_reference: Uuid,
    content_digest: String,
    schema_digest: String,
    size_bytes: u64,
    status: crate::DurableCapabilityStatus,
}
impl ExecutionDurableResultProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn logical_invocation_id(&self) -> Uuid {
        self.logical_invocation_id
    }
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    pub fn result_reference(&self) -> Uuid {
        self.result_reference
    }
    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }
    pub fn schema_digest(&self) -> &str {
        &self.schema_digest
    }
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    pub fn status(&self) -> crate::DurableCapabilityStatus {
        self.status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionReceiptProjection {
    owner_id: Uuid,
    command_id: Uuid,
    payload_digest: Uuid,
    outcome: super::CommandOutcome,
}
impl ExecutionReceiptProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn payload_digest(&self) -> Uuid {
        self.payload_digest
    }
    pub fn outcome(&self) -> super::CommandOutcome {
        self.outcome
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionOutcomeProjection {
    owner_id: Uuid,
    command_id: Uuid,
    run_id: Uuid,
    run_version: u64,
    session_version: u64,
    checkpoint_version: u64,
    grant_consumed: bool,
}
impl ExecutionOutcomeProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn run_version(&self) -> u64 {
        self.run_version
    }
    pub fn session_version(&self) -> u64 {
        self.session_version
    }
    pub fn checkpoint_version(&self) -> u64 {
        self.checkpoint_version
    }
    pub fn grant_consumed(&self) -> bool {
        self.grant_consumed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionCheckpointProjection {
    owner_id: Uuid,
    run_id: Uuid,
    session_id: Uuid,
    version: u64,
    definition_id: String,
    definition_version: u32,
    state: RunState,
    last_durable_event_sequence: u64,
}
impl ExecutionCheckpointProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }
    pub fn definition_version(&self) -> u32 {
        self.definition_version
    }
    pub fn state(&self) -> RunState {
        self.state
    }
    pub fn last_durable_event_sequence(&self) -> u64 {
        self.last_durable_event_sequence
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionPolicyProjection {
    owner_id: Uuid,
    definition_id: String,
    definition_version: u32,
    revision: u32,
    status: super::AuthoritativePolicyStatus,
    valid_until_ms: Option<u64>,
}
impl ExecutionPolicyProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }
    pub fn definition_version(&self) -> u32 {
        self.definition_version
    }
    pub fn revision(&self) -> u32 {
        self.revision
    }
    pub fn status(&self) -> super::AuthoritativePolicyStatus {
        self.status
    }
    pub fn valid_until_ms(&self) -> Option<u64> {
        self.valid_until_ms
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionGrantProjection {
    owner_id: Uuid,
    authority_key: String,
    full_grant_digest: String,
    scope_digest: String,
    revision: u32,
    status: crate::GrantStatus,
    effect: crate::GrantEffect,
    maximum_risk: crate::RiskLevel,
    valid_from_ms: i64,
    valid_until_ms: Option<i64>,
    remaining_uses: Option<u32>,
}
impl ExecutionGrantProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn authority_key(&self) -> &str {
        &self.authority_key
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
    pub fn status(&self) -> crate::GrantStatus {
        self.status
    }
    pub fn effect(&self) -> crate::GrantEffect {
        self.effect
    }
    pub fn maximum_risk(&self) -> crate::RiskLevel {
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionGrantConsumptionProjection {
    owner_id: Uuid,
    authority_key: String,
    revision: u32,
    logical_invocation_id: Uuid,
}
impl ExecutionGrantConsumptionProjection {
    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn authority_key(&self) -> &str {
        &self.authority_key
    }
    pub fn revision(&self) -> u32 {
        self.revision
    }
    pub fn logical_invocation_id(&self) -> Uuid {
        self.logical_invocation_id
    }
}

const CANONICAL_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const PERSISTENCE_SNAPSHOT_SCHEMA_VERSION: u32 = 2;
const PERSISTENCE_SNAPSHOT_PAYLOAD_ENCODING: &str = "hex";
const PERSISTENCE_SNAPSHOT_AUTHENTICATION: &str = "hmac-sha256";
const PERSISTENCE_SNAPSHOT_MAC_DOMAIN: &[u8] = b"anima-core.execution-store.persistence.v2\0";
const SNAPSHOT_SEAL_KEY_BYTES: usize = 32;
/// Hard upper bound for a stored sealed snapshot, including its JSON envelope.
pub const MAX_PERSISTENCE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_HEX_BYTES: usize =
    MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_BYTES * 2;
const SNAPSHOT_HMAC_HEX_BYTES: usize = 64;
const MAX_PERSISTENCE_ROOT_ENTRIES: usize = 16_384;
const MAX_PERSISTENCE_RUN_ENTRIES: usize = 16_384;
const MAX_PERSISTED_JSON_NODES: usize = 65_536;
const MAX_PERSISTED_JSON_COLLECTION_ENTRIES: usize = 16_384;
const MAX_PERSISTED_JSON_DEPTH: usize = 128;
const MAX_PERSISTED_STRING_FRAGMENTS: usize = 4_096;

/// Host-owned authentication key for durable snapshot provenance.
///
/// The key is deliberately neither serializable nor debuggable and must live outside the
/// database that stores the sealed snapshot.
#[derive(Clone)]
pub struct PersistenceSnapshotSealKey(Arc<[u8; SNAPSHOT_SEAL_KEY_BYTES]>);

impl PersistenceSnapshotSealKey {
    pub fn new(bytes: [u8; SNAPSHOT_SEAL_KEY_BYTES]) -> Result<Self, ExecutionStoreError> {
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(invalid_snapshot());
        }
        Ok(Self(Arc::new(bytes)))
    }

    fn bytes(&self) -> &[u8; SNAPSHOT_SEAL_KEY_BYTES] {
        &self.0
    }
}

/// One exact secret value supplied by a host resolver.
///
/// Secret material is intentionally neither serializable nor debuggable.
#[derive(Clone)]
pub struct PersistenceSecretMaterial(Arc<str>);

impl PersistenceSecretMaterial {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionStoreError> {
        let value = value.into();
        if value.is_empty() || value.len() > crate::MAX_CAPABILITY_ARGUMENT_BYTES {
            return Err(invalid_snapshot());
        }
        Ok(Self(Arc::from(value)))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistenceSecretLocator {
    capability_id: String,
    manifest_version: u32,
    reference: crate::CapabilitySecretReferenceId,
}

impl PartialOrd for PersistenceSecretLocator {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PersistenceSecretLocator {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            &self.capability_id,
            self.manifest_version,
            self.reference.manifest_index(),
        )
            .cmp(&(
                &other.capability_id,
                other.manifest_version,
                other.reference.manifest_index(),
            ))
    }
}

impl PersistenceSecretLocator {
    fn validate(&self) -> Result<(), ExecutionStoreError> {
        if self.capability_id.trim().is_empty()
            || self.capability_id.len() > crate::MAX_CAPABILITY_ID_BYTES
            || self.manifest_version == 0
        {
            return Err(invalid_snapshot());
        }
        Ok(())
    }
}

/// Fully resolved secret inventory for one validated capability manifest.
///
/// The constructor deliberately takes the manifest itself rather than an arbitrary id/version:
/// a host cannot accidentally persist a capability scope without accounting for every declared
/// secret reference.
#[derive(Clone)]
pub struct PersistenceCapabilitySecretInventory {
    capability_id: String,
    manifest_version: u32,
    schema_digest: String,
    recovery_mode: crate::RecoveryMode,
    materials: Vec<PersistenceSecretMaterial>,
}

impl PersistenceCapabilitySecretInventory {
    pub fn new(
        manifest: &crate::CapabilityManifest,
        materials: Vec<PersistenceSecretMaterial>,
    ) -> Result<Self, ExecutionStoreError> {
        if manifest.secret_references.len() != materials.len() {
            return Err(invalid_snapshot());
        }
        let locator = PersistenceSecretLocator {
            capability_id: manifest.id.clone(),
            manifest_version: manifest.version,
            reference: crate::CapabilitySecretReferenceId::from_manifest_index(0),
        };
        locator.validate()?;
        Ok(Self {
            capability_id: manifest.id.clone(),
            manifest_version: manifest.version,
            schema_digest: manifest.schema_digest().to_owned(),
            recovery_mode: manifest.recovery_mode,
            materials,
        })
    }
}

/// Host configuration used to redact, seal, authenticate, and restore durable snapshots.
///
/// Every capability scope must be backed by a complete, validated manifest inventory before it
/// can be persisted. This makes raw persistence an unavailable accidental fallback.
#[derive(Clone)]
pub struct PersistenceProtection {
    seal_key: PersistenceSnapshotSealKey,
    secrets: Arc<BTreeMap<PersistenceSecretLocator, PersistenceSecretMaterial>>,
    manifests: Arc<BTreeMap<(String, u32), (String, crate::RecoveryMode)>>,
    model_payloads_allowed: bool,
}

impl PersistenceProtection {
    pub fn new(
        seal_key: PersistenceSnapshotSealKey,
        inventories: Vec<PersistenceCapabilitySecretInventory>,
    ) -> Result<Self, ExecutionStoreError> {
        let mut secrets = BTreeMap::new();
        let mut material_owners = BTreeMap::<String, PersistenceSecretLocator>::new();
        let mut manifests = BTreeMap::new();
        for inventory in inventories {
            let scope = (inventory.capability_id.clone(), inventory.manifest_version);
            if manifests
                .insert(scope, (inventory.schema_digest, inventory.recovery_mode))
                .is_some()
            {
                return Err(invalid_snapshot());
            }
            for (index, material) in inventory.materials.into_iter().enumerate() {
                let locator = PersistenceSecretLocator {
                    capability_id: inventory.capability_id.clone(),
                    manifest_version: inventory.manifest_version,
                    reference: crate::CapabilitySecretReferenceId::from_manifest_index(
                        u16::try_from(index).map_err(|_| invalid_snapshot())?,
                    ),
                };
                locator.validate()?;
                if let Some(existing) =
                    material_owners.insert(material.expose().to_owned(), locator.clone())
                {
                    if existing != locator {
                        return Err(invalid_snapshot());
                    }
                }
                if secrets.insert(locator, material).is_some() {
                    return Err(invalid_snapshot());
                }
            }
        }
        Ok(Self {
            seal_key,
            secrets: Arc::new(secrets),
            model_payloads_allowed: false,
            manifests: Arc::new(manifests),
        })
    }

    /// Explicitly permits bounded model input/transcript payloads in the protected snapshot.
    ///
    /// Capability payloads still require their exact manifest inventories. This flag only
    /// enables model-owned payload surfaces and does not weaken manifest validation.
    pub fn allow_model_payloads(
        seal_key: PersistenceSnapshotSealKey,
        inventories: Vec<PersistenceCapabilitySecretInventory>,
    ) -> Result<Self, ExecutionStoreError> {
        let mut protection = Self::new(seal_key, inventories)?;
        protection.model_payloads_allowed = true;
        Ok(protection)
    }

    /// Builds protection for state that contains no capability or model payload surfaces.
    ///
    /// Model-only transcript persistence is intentionally unsupported until the host can supply
    /// a separate, complete model-secret inventory. This mode therefore accepts bookkeeping
    /// state only and rejects durable normalized arguments and provider transcript payloads.
    pub fn payload_free(seal_key: PersistenceSnapshotSealKey) -> Result<Self, ExecutionStoreError> {
        Self::new(seal_key, Vec::new())
    }

    fn require_manifest_scope(
        &self,
        capability_id: &str,
        manifest_version: u32,
    ) -> Result<(), ExecutionStoreError> {
        if self
            .manifests
            .contains_key(&(capability_id.to_owned(), manifest_version))
        {
            Ok(())
        } else {
            Err(invalid_snapshot())
        }
    }

    fn require_manifest_pin(&self, pin: &super::ManifestPin) -> Result<(), ExecutionStoreError> {
        if self
            .manifests
            .get(&(pin.id().to_owned(), pin.version()))
            .is_some_and(|(schema_digest, recovery_mode)| {
                schema_digest == pin.schema_digest() && *recovery_mode == pin.recovery_mode()
            })
        {
            Ok(())
        } else {
            Err(invalid_snapshot())
        }
    }

    fn material(&self, locator: &PersistenceSecretLocator) -> Option<&str> {
        self.secrets
            .get(locator)
            .map(PersistenceSecretMaterial::expose)
    }

    fn candidates(&self, scope: Option<(&str, u32)>) -> Vec<(&PersistenceSecretLocator, &str)> {
        let mut candidates = self
            .secrets
            .iter()
            .filter(|(locator, _)| {
                scope.map_or(true, |(capability_id, manifest_version)| {
                    locator.capability_id == capability_id
                        && locator.manifest_version == manifest_version
                })
            })
            .map(|(locator, material)| (locator, material.expose()))
            .collect::<Vec<_>>();
        candidates.sort_by(|(left_locator, left), (right_locator, right)| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left_locator.cmp(right_locator))
        });
        candidates
    }
}

/// Opaque, validated serialization of the canonical execution-store state machine.
///
/// Durable adapters may persist these bytes as their transaction-local state image, but cannot
/// construct or mutate one without passing the same validation used by the in-memory reference
/// adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionStoreSnapshot(Vec<u8>);

impl ExecutionStoreSnapshot {
    pub const fn maximum_bytes() -> usize {
        MAX_PERSISTENCE_SNAPSHOT_BYTES
    }

    pub fn from_bytes(
        bytes: Vec<u8>,
        protection: &PersistenceProtection,
    ) -> Result<Self, ExecutionStoreError> {
        if bytes.len() > MAX_PERSISTENCE_SNAPSHOT_BYTES {
            return Err(invalid_snapshot());
        }
        decode_persisted_snapshot(&bytes, protection)?;
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedSnapshotEnvelope {
    schema_version: u32,
    payload_encoding: String,
    payload: String,
    authentication: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistenceSnapshotWire {
    cursor_key: [u8; 16],
    sessions: Vec<SessionSnapshot>,
    runs: Vec<PersistenceRunSnapshot>,
    serial_claims: Vec<(Uuid, Uuid, Uuid)>,
    receipts: Vec<(Uuid, CommandReceipt)>,
    commands: Vec<(Uuid, Uuid, CommandIndexRecord)>,
    outcomes: Vec<PersistenceOutcomeSnapshot>,
    authoritative_grants: Vec<AuthoritativeGrantState>,
    authoritative_policies: Vec<AuthoritativePolicyState>,
    grant_consumptions: Vec<(Uuid, String, u32, Uuid)>,
    approvals: Vec<(Uuid, Uuid, ApprovalIndexRecord)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistenceRunSnapshot {
    owner_id: Uuid,
    run: super::Run,
    initial_input: PersistedJsonValue,
    run_version: u64,
    session_version: u64,
    lease: Option<ExecutionLease>,
    checkpoint_version: u64,
    checkpoint: Option<Value>,
    events: Vec<RuntimeEvent>,
    steps: Vec<(String, u64, super::Step)>,
    next_step_sequence: u64,
    attempts: Vec<(Uuid, u32, u64, Value)>,
    next_attempt_sequence: u64,
    results: Vec<(
        Uuid,
        super::CompletedInvocationRecord,
        DurableCapabilityResult,
    )>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistenceOutcomeSnapshot {
    owner_id: Uuid,
    command_id: Uuid,
    run: super::Run,
    initial_input: PersistedJsonValue,
    run_version: u64,
    session_version: u64,
    receipt: CommandReceipt,
    checkpoint_version: u64,
    checkpoint: Option<Value>,
    grant_consumption: Option<crate::GrantConsumption>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PersistedJsonValue {
    Null {
        pointer: String,
    },
    Bool {
        pointer: String,
        value: bool,
    },
    Number {
        pointer: String,
        value: Number,
    },
    String {
        pointer: String,
        fragments: Vec<PersistedStringFragment>,
    },
    Array {
        values: Vec<PersistedJsonValue>,
    },
    Object {
        entries: Vec<PersistedJsonObjectEntry>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedJsonObjectEntry {
    key: PersistedJsonValue,
    value: PersistedJsonValue,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PersistedStringFragment {
    Literal {
        value: String,
    },
    SecretReference {
        locator: PersistenceSecretLocator,
        material_binding: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotWire {
    schema_version: u32,
    cursor_key: [u8; 16],
    sessions: Vec<SessionSnapshot>,
    runs: Vec<RunSnapshot>,
    serial_claims: Vec<(Uuid, Uuid, Uuid)>,
    receipts: Vec<(Uuid, CommandReceipt)>,
    commands: Vec<(Uuid, Uuid, CommandIndexRecord)>,
    outcomes: Vec<OutcomeSnapshot>,
    authoritative_grants: Vec<AuthoritativeGrantState>,
    authoritative_policies: Vec<AuthoritativePolicyState>,
    grant_consumptions: Vec<(Uuid, String, u32, Uuid)>,
    approvals: Vec<(Uuid, Uuid, ApprovalIndexRecord)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionSnapshot {
    owner_id: Uuid,
    session_id: Uuid,
    version: u64,
    definition: super::DefinitionPin,
    policy: SessionConcurrencyPolicy,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunSnapshot {
    owner_id: Uuid,
    run: super::Run,
    initial_input: super::DurableRunInput,
    run_version: u64,
    session_version: u64,
    lease: Option<ExecutionLease>,
    checkpoint_version: u64,
    checkpoint: Option<super::CheckpointV1>,
    events: Vec<RuntimeEvent>,
    steps: Vec<(String, u64, super::Step)>,
    next_step_sequence: u64,
    attempts: Vec<(Uuid, u32, u64, super::InvocationAttemptRecord)>,
    next_attempt_sequence: u64,
    results: Vec<(
        Uuid,
        super::CompletedInvocationRecord,
        DurableCapabilityResult,
    )>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeSnapshot {
    owner_id: Uuid,
    command_id: Uuid,
    run: super::Run,
    initial_input: super::DurableRunInput,
    run_version: u64,
    session_version: u64,
    receipt: CommandReceipt,
    checkpoint_version: u64,
    checkpoint: Option<super::CheckpointV1>,
    grant_consumption: Option<crate::GrantConsumption>,
}

struct CommitPatch {
    stored: StoredRun,
    lease: Option<ExecutionLease>,
    checkpoint_version: u64,
    checkpoint: Option<Arc<super::CheckpointV1>>,
    events: Vec<RuntimeEvent>,
    steps: Vec<(String, u64, super::Step)>,
    next_step_sequence: u64,
    attempts: Vec<((Uuid, u32), u64, super::InvocationAttemptRecord)>,
    next_attempt_sequence: u64,
    attempts_fingerprint: super::checkpoint::HistoryFingerprint,
    results: Vec<(Uuid, StoredDurableResult)>,
    completed_fingerprint: super::checkpoint::HistoryFingerprint,
    authoritative_grant_update: Option<AuthoritativeGrantState>,
    grant_consumption_key: Option<(Uuid, String, u32, Uuid)>,
    session_update: Option<StoredSession>,
    release_serial_claim: bool,
    receipt: CommandReceipt,
    command_index: CommandIndexRecord,
    grant_consumption: Option<crate::GrantConsumption>,
    approval_index_update: Option<(Uuid, ApprovalIndexRecord)>,
    approval_index_removal: Option<Uuid>,
}

/// In-process reference adapter. It clones no externally visible state until validation succeeds.
pub struct InMemoryExecutionStore {
    state: Mutex<State>,
    cursor_key: [u8; 16],
    clock: Arc<dyn ExecutionClock>,
}

impl Default for InMemoryExecutionStore {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemExecutionClock))
    }
}

impl InMemoryExecutionStore {
    pub fn with_clock(clock: Arc<dyn ExecutionClock>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            cursor_key: *Uuid::new_v4().as_bytes(),
            clock,
        }
    }

    /// Restores a canonical store from a previously validated opaque snapshot.
    pub fn from_snapshot(
        snapshot: &ExecutionStoreSnapshot,
        protection: &PersistenceProtection,
        clock: Arc<dyn ExecutionClock>,
    ) -> Result<Self, ExecutionStoreError> {
        let (state, cursor_key) = decode_persisted_snapshot(snapshot.as_bytes(), protection)?;
        Ok(Self {
            state: Mutex::new(state),
            cursor_key,
            clock,
        })
    }

    /// Captures all state required to preserve exact store semantics across process restarts.
    pub async fn export_snapshot(
        &self,
        protection: &PersistenceProtection,
    ) -> Result<ExecutionStoreSnapshot, ExecutionStoreError> {
        let state = self.state.lock().await;
        let canonical = encode_snapshot(&state, self.cursor_key);
        validate_snapshot_payload_surfaces(&canonical, protection)?;
        let payload = protect_snapshot(canonical, protection, self.cursor_key)?;
        let payload_bytes = serde_jcs::to_vec(&payload).map_err(|_| invalid_snapshot())?;
        if payload_bytes.len() > MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_BYTES {
            return Err(invalid_snapshot());
        }
        let envelope = seal_snapshot_payload(&payload_bytes, protection);
        let bytes = serde_json::to_vec(&envelope).map_err(|_| invalid_snapshot())?;
        if bytes.len() > MAX_PERSISTENCE_SNAPSHOT_BYTES {
            return Err(invalid_snapshot());
        }
        ExecutionStoreSnapshot::from_bytes(bytes, protection)
    }

    /// Exports only the typed scalar metadata an adapter may index beside the sealed snapshot.
    ///
    /// The projection intentionally omits every raw payload-bearing execution value. It is not a
    /// recovery format and cannot be used to reconstruct execution state.
    pub async fn export_projection(&self) -> ExecutionStoreProjection {
        let state = self.state.lock().await;
        projection_from_state(&state)
    }
}

fn projection_from_state(state: &State) -> ExecutionStoreProjection {
    let mut definition_pins = BTreeSet::new();
    let sessions = state
        .sessions
        .iter()
        .map(|((owner_id, session_id), session)| {
            definition_pins.insert((
                *owner_id,
                session.definition.id().to_owned(),
                session.definition.version(),
                session.definition.schema_version(),
            ));
            ExecutionSessionProjection {
                owner_id: *owner_id,
                session_id: *session_id,
                definition_id: session.definition.id().to_owned(),
                definition_version: session.definition.version(),
                definition_schema_version: session.definition.schema_version(),
                version: session.version,
                concurrency: session.policy,
            }
        })
        .collect();
    let mut leases = Vec::new();
    let mut events = Vec::new();
    let mut steps = Vec::new();
    let mut attempts = Vec::new();
    let mut logical_invocations = BTreeMap::new();
    let mut durable_results = Vec::new();
    let mut checkpoints = Vec::new();
    let runs = state
        .runs
        .iter()
        .map(|((owner_id, run_id), aggregate)| {
            let run = aggregate.stored.run();
            if let Some(lease) = &aggregate.lease {
                leases.push(ExecutionLeaseProjection {
                    owner_id: *owner_id,
                    run_id: *run_id,
                    fence: lease.fence(),
                    expires_at_ms: lease.expires_at_ms(),
                });
            }
            for event in &aggregate.events {
                events.push(ExecutionEventProjection {
                    owner_id: *owner_id,
                    event_id: event.event_id(),
                    session_id: event.session_id(),
                    run_id: event.run_id(),
                    schema_version: event.schema_version(),
                    timestamp_ms: event.timestamp_ms(),
                    sequence: event.sequence(),
                    kind: event.kind(),
                });
            }
            for (sequence, key) in &aggregate.step_order {
                if let Some(step) = aggregate.steps.get(key) {
                    steps.push(ExecutionStepProjection {
                        owner_id: *owner_id,
                        run_id: step.run_id(),
                        logical_step_id: step.logical_step_id().to_owned(),
                        sequence: *sequence,
                        kind: step.kind(),
                    });
                }
            }
            for (sequence, key) in &aggregate.attempt_order {
                if let Some(attempt) = aggregate.attempts.get(key) {
                    let binding = attempt.invocation();
                    logical_invocations
                        .entry((*owner_id, binding.id()))
                        .or_insert_with(|| ExecutionLogicalInvocationProjection {
                            owner_id: *owner_id,
                            id: binding.id(),
                            run_id: binding.run_id(),
                            logical_step_id: binding.logical_step_id().to_owned(),
                            capability_id: binding.capability_id().to_owned(),
                            manifest_version: binding.manifest_version(),
                            canonical_argument_digest: binding.canonical_argument_digest(),
                            idempotency_key: binding.idempotency_key().to_owned(),
                        });
                    attempts.push(ExecutionAttemptProjection {
                        owner_id: *owner_id,
                        run_id: binding.run_id(),
                        logical_invocation_id: binding.id(),
                        attempt_number: attempt.attempt_number(),
                        sequence: *sequence,
                        state: attempt.state(),
                        manifest_id: attempt.manifest().id().to_owned(),
                        manifest_version: attempt.manifest().version(),
                        manifest_schema_digest: attempt.manifest().schema_digest().to_owned(),
                        recovery_mode: attempt.recovery_mode(),
                    });
                }
            }
            for (invocation_id, stored) in &aggregate.results {
                let binding = stored.completed.invocation();
                logical_invocations
                    .entry((*owner_id, binding.id()))
                    .or_insert_with(|| ExecutionLogicalInvocationProjection {
                        owner_id: *owner_id,
                        id: binding.id(),
                        run_id: binding.run_id(),
                        logical_step_id: binding.logical_step_id().to_owned(),
                        capability_id: binding.capability_id().to_owned(),
                        manifest_version: binding.manifest_version(),
                        canonical_argument_digest: binding.canonical_argument_digest(),
                        idempotency_key: binding.idempotency_key().to_owned(),
                    });
                durable_results.push(ExecutionDurableResultProjection {
                    owner_id: *owner_id,
                    run_id: run.id(),
                    logical_invocation_id: *invocation_id,
                    attempt_number: stored.completed.attempt_number(),
                    result_reference: stored.result.result_ref().handle(),
                    content_digest: stored.result.content_digest().to_owned(),
                    schema_digest: stored.result.schema_digest().to_owned(),
                    size_bytes: stored.result.size_bytes(),
                    status: stored.result.status(),
                });
            }
            if let Some(checkpoint) = &aggregate.checkpoint {
                let checkpoint = checkpoint.as_ref();
                checkpoints.push(ExecutionCheckpointProjection {
                    owner_id: *owner_id,
                    run_id: checkpoint.run_id(),
                    session_id: checkpoint.session_id(),
                    version: aggregate.checkpoint_version,
                    definition_id: checkpoint.definition().id().to_owned(),
                    definition_version: checkpoint.definition().version(),
                    state: checkpoint.state(),
                    last_durable_event_sequence: checkpoint.last_durable_event_sequence(),
                });
            }
            ExecutionRunProjection {
                owner_id: *owner_id,
                run_id: *run_id,
                session_id: run.session_id(),
                definition_id: run.definition_id().to_owned(),
                definition_version: run.definition_version(),
                state: run.state(),
                run_version: aggregate.stored.run_version(),
                session_version: aggregate.stored.session_version(),
                checkpoint_version: aggregate.checkpoint_version,
            }
        })
        .collect();
    let serial_claims = state
        .serial_claims
        .iter()
        .map(
            |((owner_id, session_id), run_id)| ExecutionSerialClaimProjection {
                owner_id: *owner_id,
                session_id: *session_id,
                run_id: *run_id,
            },
        )
        .collect();
    let receipts = state
        .receipts
        .iter()
        .map(|((owner_id, _), receipt)| ExecutionReceiptProjection {
            owner_id: *owner_id,
            command_id: receipt.command_id(),
            payload_digest: receipt.payload_digest(),
            outcome: receipt.outcome(),
        })
        .collect();
    let outcomes = state
        .outcomes
        .iter()
        .map(
            |((owner_id, command_id), outcome)| ExecutionOutcomeProjection {
                owner_id: *owner_id,
                command_id: *command_id,
                run_id: outcome.stored_run().run().id(),
                run_version: outcome.stored_run().run_version(),
                session_version: outcome.stored_run().session_version(),
                checkpoint_version: outcome.checkpoint_version(),
                grant_consumed: outcome.grant_consumption().is_some(),
            },
        )
        .collect();
    let policies = state
        .authoritative_policies
        .values()
        .map(|policy| ExecutionPolicyProjection {
            owner_id: policy.owner_id(),
            definition_id: policy.agent_definition_id().to_owned(),
            definition_version: policy.agent_definition_version(),
            revision: policy.revision(),
            status: policy.status(),
            valid_until_ms: policy.valid_until_ms(),
        })
        .collect();
    let grants = state
        .authoritative_grants
        .values()
        .map(|grant| ExecutionGrantProjection {
            owner_id: grant.owner_id(),
            authority_key: grant.authority_key_encoded().to_owned(),
            full_grant_digest: grant.full_grant_digest().to_owned(),
            scope_digest: grant.scope_digest().to_owned(),
            revision: grant.revision(),
            status: grant.status(),
            effect: grant.effect(),
            maximum_risk: grant.maximum_risk(),
            valid_from_ms: grant.valid_from_ms(),
            valid_until_ms: grant.valid_until_ms(),
            remaining_uses: grant.remaining_uses(),
        })
        .collect();
    let grant_consumptions = state
        .grant_consumptions
        .iter()
        .map(|(owner_id, authority_key, revision, invocation_id)| {
            ExecutionGrantConsumptionProjection {
                owner_id: *owner_id,
                authority_key: authority_key.clone(),
                revision: *revision,
                logical_invocation_id: *invocation_id,
            }
        })
        .collect();
    let approvals = state
        .approvals
        .iter()
        .map(
            |((owner_id, approval_id), record)| ExecutionApprovalProjection {
                owner_id: *owner_id,
                id: *approval_id,
                run_id: record.run_id,
                decision_id: record.decision_command_id,
                requested_at_ms: record.requested_at_ms,
                expires_at_ms: record.expires_at_ms,
            },
        )
        .collect();
    let decisions = state
        .approvals
        .iter()
        .filter_map(|((owner_id, approval_id), record)| {
            Some(ExecutionDecisionProjection {
                owner_id: *owner_id,
                id: record.decision_command_id?,
                approval_id: *approval_id,
                claimed: true,
                kind: record.decision_kind?,
                decided_at_ms: record.decided_at_ms?,
            })
        })
        .collect();
    let commands = state
        .commands
        .iter()
        .map(
            |((owner_id, command_id), record)| ExecutionCommandProjection {
                owner_id: *owner_id,
                command_id: *command_id,
                session_id: record.session_id,
                run_id: record.run_id,
                kind: record.kind,
                payload_digest: record.payload_digest,
            },
        )
        .collect();

    ExecutionStoreProjection {
        definition_pins: definition_pins
            .into_iter()
            .map(
                |(owner_id, id, version, schema_version)| ExecutionDefinitionPinProjection {
                    owner_id,
                    id,
                    version,
                    schema_version,
                },
            )
            .collect(),
        sessions,
        runs,
        serial_claims,
        leases,
        events,
        steps,
        attempts,
        logical_invocations: logical_invocations.into_values().collect(),
        durable_results,
        commands,
        receipts,
        outcomes,
        checkpoints,
        policies,
        grants,
        grant_consumptions,
        approvals,
        decisions,
    }
}

fn invalid_snapshot() -> ExecutionStoreError {
    ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest)
}

fn encode_snapshot(state: &State, cursor_key: [u8; 16]) -> SnapshotWire {
    SnapshotWire {
        schema_version: CANONICAL_SNAPSHOT_SCHEMA_VERSION,
        cursor_key,
        sessions: state
            .sessions
            .iter()
            .map(|((owner_id, session_id), session)| SessionSnapshot {
                owner_id: *owner_id,
                session_id: *session_id,
                version: session.version,
                definition: session.definition.clone(),
                policy: session.policy,
            })
            .collect(),
        runs: state
            .runs
            .iter()
            .map(|((owner_id, _), aggregate)| RunSnapshot {
                owner_id: *owner_id,
                run: aggregate.stored.run().clone(),
                initial_input: aggregate.stored.initial_input().clone(),
                run_version: aggregate.stored.run_version(),
                session_version: aggregate.stored.session_version(),
                lease: aggregate.lease.clone(),
                checkpoint_version: aggregate.checkpoint_version,
                checkpoint: aggregate
                    .checkpoint
                    .as_ref()
                    .map(|checkpoint| checkpoint.as_ref().clone()),
                events: aggregate.events.clone(),
                steps: aggregate
                    .step_order
                    .iter()
                    .filter_map(|(sequence, key)| {
                        aggregate
                            .steps
                            .get(key)
                            .cloned()
                            .map(|step| (key.clone(), *sequence, step))
                    })
                    .collect(),
                next_step_sequence: aggregate.next_step_sequence,
                attempts: aggregate
                    .attempt_order
                    .iter()
                    .filter_map(|(sequence, key)| {
                        aggregate
                            .attempts
                            .get(key)
                            .cloned()
                            .map(|attempt| (key.0, key.1, *sequence, attempt))
                    })
                    .collect(),
                next_attempt_sequence: aggregate.next_attempt_sequence,
                results: aggregate
                    .results
                    .iter()
                    .map(|(invocation_id, stored)| {
                        (
                            *invocation_id,
                            stored.completed.clone(),
                            stored.result.clone(),
                        )
                    })
                    .collect(),
            })
            .collect(),
        serial_claims: state
            .serial_claims
            .iter()
            .map(|((owner_id, session_id), run_id)| (*owner_id, *session_id, *run_id))
            .collect(),
        receipts: state
            .receipts
            .iter()
            .map(|((owner_id, _), receipt)| (*owner_id, receipt.clone()))
            .collect(),
        commands: state
            .commands
            .iter()
            .map(|((owner_id, command_id), record)| (*owner_id, *command_id, record.clone()))
            .collect(),
        outcomes: state
            .outcomes
            .iter()
            .map(|((owner_id, command_id), outcome)| OutcomeSnapshot {
                owner_id: *owner_id,
                command_id: *command_id,
                run: outcome.stored_run().run().clone(),
                initial_input: outcome.stored_run().initial_input().clone(),
                run_version: outcome.stored_run().run_version(),
                session_version: outcome.stored_run().session_version(),
                receipt: outcome.receipt().clone(),
                checkpoint_version: outcome.checkpoint_version(),
                checkpoint: outcome.checkpoint().cloned(),
                grant_consumption: outcome.grant_consumption().cloned(),
            })
            .collect(),
        authoritative_grants: state.authoritative_grants.values().cloned().collect(),
        authoritative_policies: state.authoritative_policies.values().cloned().collect(),
        grant_consumptions: state.grant_consumptions.iter().cloned().collect(),
        approvals: state
            .approvals
            .iter()
            .map(|((owner_id, approval_id), record)| (*owner_id, *approval_id, record.clone()))
            .collect(),
    }
}

fn protect_snapshot(
    wire: SnapshotWire,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<PersistenceSnapshotWire, ExecutionStoreError> {
    let runs = wire
        .runs
        .into_iter()
        .map(|run| {
            Ok(PersistenceRunSnapshot {
                owner_id: run.owner_id,
                run: run.run,
                initial_input: protect_json(
                    &serde_json::to_value(&run.initial_input).map_err(|_| invalid_snapshot())?,
                    "/initial_input",
                    protection,
                    None,
                    cursor_key,
                )?,
                run_version: run.run_version,
                session_version: run.session_version,
                lease: run.lease,
                checkpoint_version: run.checkpoint_version,
                checkpoint: run
                    .checkpoint
                    .as_ref()
                    .map(|checkpoint| protect_checkpoint(checkpoint, protection, cursor_key))
                    .transpose()?,
                events: run.events,
                steps: run.steps,
                next_step_sequence: run.next_step_sequence,
                attempts: run
                    .attempts
                    .into_iter()
                    .map(|(invocation_id, attempt_number, sequence, attempt)| {
                        protect_attempt(&attempt, protection, cursor_key)
                            .map(|attempt| (invocation_id, attempt_number, sequence, attempt))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                next_attempt_sequence: run.next_attempt_sequence,
                results: run.results,
            })
        })
        .collect::<Result<Vec<_>, ExecutionStoreError>>()?;
    let outcomes = wire
        .outcomes
        .into_iter()
        .map(|outcome| {
            Ok(PersistenceOutcomeSnapshot {
                owner_id: outcome.owner_id,
                command_id: outcome.command_id,
                run: outcome.run,
                initial_input: protect_json(
                    &serde_json::to_value(&outcome.initial_input)
                        .map_err(|_| invalid_snapshot())?,
                    "/initial_input",
                    protection,
                    None,
                    cursor_key,
                )?,
                run_version: outcome.run_version,
                session_version: outcome.session_version,
                receipt: outcome.receipt,
                checkpoint_version: outcome.checkpoint_version,
                checkpoint: outcome
                    .checkpoint
                    .as_ref()
                    .map(|checkpoint| protect_checkpoint(checkpoint, protection, cursor_key))
                    .transpose()?,
                grant_consumption: outcome.grant_consumption,
            })
        })
        .collect::<Result<Vec<_>, ExecutionStoreError>>()?;
    Ok(PersistenceSnapshotWire {
        cursor_key: wire.cursor_key,
        sessions: wire.sessions,
        runs,
        serial_claims: wire.serial_claims,
        receipts: wire.receipts,
        commands: wire.commands,
        outcomes,
        authoritative_grants: wire.authoritative_grants,
        authoritative_policies: wire.authoritative_policies,
        grant_consumptions: wire.grant_consumptions,
        approvals: wire.approvals,
    })
}

fn validate_snapshot_payload_surfaces(
    wire: &SnapshotWire,
    protection: &PersistenceProtection,
) -> Result<(), ExecutionStoreError> {
    let capability_payload_free = protection.manifests.is_empty();
    for run in &wire.runs {
        if !protection.model_payloads_allowed && !run.initial_input.is_empty() {
            return Err(invalid_snapshot());
        }
        if capability_payload_free && !run.attempts.is_empty() {
            return Err(invalid_snapshot());
        }
        if let Some(checkpoint) = &run.checkpoint {
            validate_checkpoint_payload_surfaces(
                checkpoint,
                capability_payload_free,
                protection.model_payloads_allowed,
            )?;
        }
    }
    for outcome in &wire.outcomes {
        if !protection.model_payloads_allowed && !outcome.initial_input.is_empty() {
            return Err(invalid_snapshot());
        }
        if let Some(checkpoint) = &outcome.checkpoint {
            validate_checkpoint_payload_surfaces(
                checkpoint,
                capability_payload_free,
                protection.model_payloads_allowed,
            )?;
        }
    }
    Ok(())
}

fn validate_checkpoint_payload_surfaces(
    checkpoint: &super::CheckpointV1,
    capability_payload_free: bool,
    model_payloads_allowed: bool,
) -> Result<(), ExecutionStoreError> {
    if capability_payload_free && !checkpoint.attempts().is_empty() {
        return Err(invalid_snapshot());
    }
    if !model_payloads_allowed && !checkpoint.provider_transcript().is_empty() {
        return Err(invalid_snapshot());
    }
    Ok(())
}

fn restore_snapshot(
    wire: PersistenceSnapshotWire,
    protection: &PersistenceProtection,
) -> Result<SnapshotWire, ExecutionStoreError> {
    validate_persistence_wire_bounds(&wire)?;
    let cursor_key = wire.cursor_key;
    let runs = wire
        .runs
        .into_iter()
        .map(|run| {
            Ok(RunSnapshot {
                owner_id: run.owner_id,
                run: run.run,
                initial_input: serde_json::from_value(restore_json(
                    run.initial_input,
                    "/initial_input",
                    protection,
                    cursor_key,
                )?)
                .map_err(|_| invalid_snapshot())?,
                run_version: run.run_version,
                session_version: run.session_version,
                lease: run.lease,
                checkpoint_version: run.checkpoint_version,
                checkpoint: run
                    .checkpoint
                    .map(|checkpoint| restore_checkpoint(checkpoint, protection, cursor_key))
                    .transpose()?,
                events: run.events,
                steps: run.steps,
                next_step_sequence: run.next_step_sequence,
                attempts: run
                    .attempts
                    .into_iter()
                    .map(|(invocation_id, attempt_number, sequence, attempt)| {
                        restore_attempt(attempt, protection, cursor_key)
                            .map(|attempt| (invocation_id, attempt_number, sequence, attempt))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                next_attempt_sequence: run.next_attempt_sequence,
                results: run.results,
            })
        })
        .collect::<Result<Vec<_>, ExecutionStoreError>>()?;
    let outcomes = wire
        .outcomes
        .into_iter()
        .map(|outcome| {
            Ok(OutcomeSnapshot {
                owner_id: outcome.owner_id,
                command_id: outcome.command_id,
                run: outcome.run,
                initial_input: serde_json::from_value(restore_json(
                    outcome.initial_input,
                    "/initial_input",
                    protection,
                    cursor_key,
                )?)
                .map_err(|_| invalid_snapshot())?,
                run_version: outcome.run_version,
                session_version: outcome.session_version,
                receipt: outcome.receipt,
                checkpoint_version: outcome.checkpoint_version,
                checkpoint: outcome
                    .checkpoint
                    .map(|checkpoint| restore_checkpoint(checkpoint, protection, cursor_key))
                    .transpose()?,
                grant_consumption: outcome.grant_consumption,
            })
        })
        .collect::<Result<Vec<_>, ExecutionStoreError>>()?;
    Ok(SnapshotWire {
        schema_version: CANONICAL_SNAPSHOT_SCHEMA_VERSION,
        cursor_key,
        sessions: wire.sessions,
        runs,
        serial_claims: wire.serial_claims,
        receipts: wire.receipts,
        commands: wire.commands,
        outcomes,
        authoritative_grants: wire.authoritative_grants,
        authoritative_policies: wire.authoritative_policies,
        grant_consumptions: wire.grant_consumptions,
        approvals: wire.approvals,
    })
}

fn validate_persistence_wire_bounds(
    wire: &PersistenceSnapshotWire,
) -> Result<(), ExecutionStoreError> {
    for length in [
        wire.sessions.len(),
        wire.runs.len(),
        wire.serial_claims.len(),
        wire.receipts.len(),
        wire.commands.len(),
        wire.outcomes.len(),
        wire.authoritative_grants.len(),
        wire.authoritative_policies.len(),
        wire.grant_consumptions.len(),
        wire.approvals.len(),
    ] {
        if length > MAX_PERSISTENCE_ROOT_ENTRIES {
            return Err(invalid_snapshot());
        }
    }
    for run in &wire.runs {
        for length in [
            run.events.len(),
            run.steps.len(),
            run.attempts.len(),
            run.results.len(),
        ] {
            if length > MAX_PERSISTENCE_RUN_ENTRIES {
                return Err(invalid_snapshot());
            }
        }
    }
    Ok(())
}

fn protect_attempt(
    attempt: &super::InvocationAttemptRecord,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<Value, ExecutionStoreError> {
    protection.require_manifest_pin(attempt.manifest())?;
    protection.require_manifest_scope(
        attempt.invocation().capability_id(),
        attempt.invocation().manifest_version(),
    )?;
    let mut value = serde_json::to_value(attempt).map_err(|_| invalid_snapshot())?;
    let object = value.as_object_mut().ok_or_else(invalid_snapshot)?;
    if let Some(arguments) = object.remove("normalized_arguments") {
        let protected = protect_json(
            &arguments,
            "",
            protection,
            Some((
                attempt.invocation().capability_id(),
                attempt.invocation().manifest_version(),
            )),
            cursor_key,
        )?;
        object.insert(
            "normalized_arguments".into(),
            serde_json::to_value(protected).map_err(|_| invalid_snapshot())?,
        );
    }
    Ok(value)
}

fn restore_attempt(
    mut value: Value,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<super::InvocationAttemptRecord, ExecutionStoreError> {
    let object = value.as_object_mut().ok_or_else(invalid_snapshot)?;
    if let Some(arguments) = object.remove("normalized_arguments") {
        let protected: PersistedJsonValue =
            serde_json::from_value(arguments).map_err(|_| invalid_snapshot())?;
        object.insert(
            "normalized_arguments".into(),
            restore_json(protected, "", protection, cursor_key)?,
        );
    }
    let attempt: super::InvocationAttemptRecord =
        serde_json::from_value(value).map_err(|_| invalid_snapshot())?;
    protection.require_manifest_pin(attempt.manifest())?;
    protection.require_manifest_scope(
        attempt.invocation().capability_id(),
        attempt.invocation().manifest_version(),
    )?;
    Ok(attempt)
}

fn protect_checkpoint(
    checkpoint: &super::CheckpointV1,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<Value, ExecutionStoreError> {
    for manifest in checkpoint.manifests() {
        protection.require_manifest_pin(manifest)?;
    }
    let mut value = serde_json::to_value(checkpoint).map_err(|_| invalid_snapshot())?;
    let object = value.as_object_mut().ok_or_else(invalid_snapshot)?;
    object.insert(
        "attempts".into(),
        Value::Array(
            checkpoint
                .attempts()
                .iter()
                .map(|attempt| protect_attempt(attempt, protection, cursor_key))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    if let Some(transcript) = object.get_mut("provider_transcript") {
        let transcript = transcript.as_array_mut().ok_or_else(invalid_snapshot)?;
        for (index, entry) in transcript.iter_mut().enumerate() {
            protect_transcript_entry(entry, index, protection, cursor_key)?;
        }
    }
    Ok(value)
}

fn restore_checkpoint(
    mut value: Value,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<super::CheckpointV1, ExecutionStoreError> {
    let object = value.as_object_mut().ok_or_else(invalid_snapshot)?;
    let attempts = object
        .get_mut("attempts")
        .and_then(Value::as_array_mut)
        .ok_or_else(invalid_snapshot)?;
    for attempt in attempts {
        *attempt = serde_json::to_value(restore_attempt(
            std::mem::take(attempt),
            protection,
            cursor_key,
        )?)
        .map_err(|_| invalid_snapshot())?;
    }
    if let Some(transcript) = object.get_mut("provider_transcript") {
        let transcript = transcript.as_array_mut().ok_or_else(invalid_snapshot)?;
        for (index, entry) in transcript.iter_mut().enumerate() {
            restore_transcript_entry(entry, index, protection, cursor_key)?;
        }
    }
    let checkpoint: super::CheckpointV1 =
        serde_json::from_value(value).map_err(|_| invalid_snapshot())?;
    for manifest in checkpoint.manifests() {
        protection.require_manifest_pin(manifest)?;
    }
    Ok(checkpoint)
}

fn protect_transcript_entry(
    entry: &mut Value,
    index: usize,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<(), ExecutionStoreError> {
    let object = entry.as_object_mut().ok_or_else(invalid_snapshot)?;
    let content = object
        .get_mut("content")
        .and_then(Value::as_object_mut)
        .ok_or_else(invalid_snapshot)?;
    protect_json_field(
        content,
        "text",
        &format!("/provider_transcript/{index}/content/text"),
        protection,
        cursor_key,
    )?;
    if content.contains_key("attachments") {
        protect_json_field(
            content,
            "attachments",
            &format!("/provider_transcript/{index}/content/attachments"),
            protection,
            cursor_key,
        )?;
    }
    if content.contains_key("metadata") {
        protect_json_field(
            content,
            "metadata",
            &format!("/provider_transcript/{index}/content/metadata"),
            protection,
            cursor_key,
        )?;
    }
    if let Some(tool_calls) = object.get_mut("tool_calls") {
        let tool_calls = tool_calls.as_array_mut().ok_or_else(invalid_snapshot)?;
        for (tool_index, tool_call) in tool_calls.iter_mut().enumerate() {
            let tool_call = tool_call.as_object_mut().ok_or_else(invalid_snapshot)?;
            protect_json_field(
                tool_call,
                "arguments",
                &format!("/provider_transcript/{index}/tool_calls/{tool_index}/arguments"),
                protection,
                cursor_key,
            )?;
        }
    }
    Ok(())
}

fn restore_transcript_entry(
    entry: &mut Value,
    index: usize,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<(), ExecutionStoreError> {
    let object = entry.as_object_mut().ok_or_else(invalid_snapshot)?;
    let content = object
        .get_mut("content")
        .and_then(Value::as_object_mut)
        .ok_or_else(invalid_snapshot)?;
    restore_json_field(
        content,
        "text",
        &format!("/provider_transcript/{index}/content/text"),
        protection,
        cursor_key,
    )?;
    if content.contains_key("attachments") {
        restore_json_field(
            content,
            "attachments",
            &format!("/provider_transcript/{index}/content/attachments"),
            protection,
            cursor_key,
        )?;
    }
    if content.contains_key("metadata") {
        restore_json_field(
            content,
            "metadata",
            &format!("/provider_transcript/{index}/content/metadata"),
            protection,
            cursor_key,
        )?;
    }
    if let Some(tool_calls) = object.get_mut("tool_calls") {
        let tool_calls = tool_calls.as_array_mut().ok_or_else(invalid_snapshot)?;
        for (tool_index, tool_call) in tool_calls.iter_mut().enumerate() {
            let tool_call = tool_call.as_object_mut().ok_or_else(invalid_snapshot)?;
            restore_json_field(
                tool_call,
                "arguments",
                &format!("/provider_transcript/{index}/tool_calls/{tool_index}/arguments"),
                protection,
                cursor_key,
            )?;
        }
    }
    Ok(())
}

fn protect_json_field(
    object: &mut serde_json::Map<String, Value>,
    field: &str,
    pointer: &str,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<(), ExecutionStoreError> {
    let value = object.remove(field).ok_or_else(invalid_snapshot)?;
    let protected = protect_json(&value, pointer, protection, None, cursor_key)?;
    object.insert(
        field.into(),
        serde_json::to_value(protected).map_err(|_| invalid_snapshot())?,
    );
    Ok(())
}

fn restore_json_field(
    object: &mut serde_json::Map<String, Value>,
    field: &str,
    pointer: &str,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<(), ExecutionStoreError> {
    let value = object.remove(field).ok_or_else(invalid_snapshot)?;
    let protected: PersistedJsonValue =
        serde_json::from_value(value).map_err(|_| invalid_snapshot())?;
    object.insert(
        field.into(),
        restore_json(protected, pointer, protection, cursor_key)?,
    );
    Ok(())
}

fn protect_json(
    value: &Value,
    pointer: &str,
    protection: &PersistenceProtection,
    scope: Option<(&str, u32)>,
    cursor_key: [u8; 16],
) -> Result<PersistedJsonValue, ExecutionStoreError> {
    match value {
        Value::Null => Ok(PersistedJsonValue::Null {
            pointer: pointer.into(),
        }),
        Value::Bool(value) => Ok(PersistedJsonValue::Bool {
            pointer: pointer.into(),
            value: *value,
        }),
        Value::Number(value) => Ok(PersistedJsonValue::Number {
            pointer: pointer.into(),
            value: value.clone(),
        }),
        Value::String(value) => Ok(PersistedJsonValue::String {
            pointer: pointer.into(),
            fragments: protect_string(value, protection, scope, cursor_key)?,
        }),
        Value::Array(values) => Ok(PersistedJsonValue::Array {
            values: values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    protect_json(
                        value,
                        &join_json_pointer(pointer, &index.to_string()),
                        protection,
                        scope,
                        cursor_key,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        }),
        Value::Object(values) => Ok(PersistedJsonValue::Object {
            entries: values
                .iter()
                .enumerate()
                .map(|(index, (key, value))| {
                    let entry_pointer = join_json_pointer(pointer, &format!("entries/{index}"));
                    Ok(PersistedJsonObjectEntry {
                        key: PersistedJsonValue::String {
                            pointer: join_json_pointer(&entry_pointer, "key"),
                            fragments: protect_string(key, protection, scope, cursor_key)?,
                        },
                        value: protect_json(
                            value,
                            &join_json_pointer(&entry_pointer, "value"),
                            protection,
                            scope,
                            cursor_key,
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, ExecutionStoreError>>()?,
        }),
    }
}

fn restore_json(
    value: PersistedJsonValue,
    pointer: &str,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<Value, ExecutionStoreError> {
    let mut budget = RestoreJsonBudget::default();
    restore_json_with_budget(value, pointer, protection, cursor_key, &mut budget)
}

#[derive(Default)]
struct RestoreJsonBudget {
    nodes: usize,
    depth: usize,
}

fn restore_json_with_budget(
    value: PersistedJsonValue,
    pointer: &str,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
    budget: &mut RestoreJsonBudget,
) -> Result<Value, ExecutionStoreError> {
    budget.nodes = budget.nodes.checked_add(1).ok_or_else(invalid_snapshot)?;
    budget.depth = budget.depth.checked_add(1).ok_or_else(invalid_snapshot)?;
    if budget.nodes > MAX_PERSISTED_JSON_NODES || budget.depth > MAX_PERSISTED_JSON_DEPTH {
        return Err(invalid_snapshot());
    }
    let restored = match value {
        PersistedJsonValue::Null { pointer: persisted } if persisted == pointer => Ok(Value::Null),
        PersistedJsonValue::Bool {
            pointer: persisted,
            value,
        } if persisted == pointer => Ok(Value::Bool(value)),
        PersistedJsonValue::Number {
            pointer: persisted,
            value,
        } if persisted == pointer => Ok(Value::Number(value)),
        PersistedJsonValue::String {
            pointer: persisted,
            fragments,
        } if persisted == pointer => {
            restore_string(fragments, protection, cursor_key).map(Value::String)
        }
        PersistedJsonValue::Array { values } => {
            if values.len() > MAX_PERSISTED_JSON_COLLECTION_ENTRIES {
                return Err(invalid_snapshot());
            }
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    restore_json_with_budget(
                        value,
                        &join_json_pointer(pointer, &index.to_string()),
                        protection,
                        cursor_key,
                        budget,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        PersistedJsonValue::Object { entries } => {
            if entries.len() > MAX_PERSISTED_JSON_COLLECTION_ENTRIES {
                return Err(invalid_snapshot());
            }
            let mut values = serde_json::Map::new();
            for (index, entry) in entries.into_iter().enumerate() {
                let entry_pointer = join_json_pointer(pointer, &format!("entries/{index}"));
                let Value::String(key) = restore_json_with_budget(
                    entry.key,
                    &join_json_pointer(&entry_pointer, "key"),
                    protection,
                    cursor_key,
                    budget,
                )?
                else {
                    return Err(invalid_snapshot());
                };
                let value = restore_json_with_budget(
                    entry.value,
                    &join_json_pointer(&entry_pointer, "value"),
                    protection,
                    cursor_key,
                    budget,
                )?;
                if values.insert(key, value).is_some() {
                    return Err(invalid_snapshot());
                }
            }
            Ok(Value::Object(values))
        }
        _ => Err(invalid_snapshot()),
    };
    budget.depth = budget.depth.checked_sub(1).ok_or_else(invalid_snapshot)?;
    restored
}

fn protect_string(
    value: &str,
    protection: &PersistenceProtection,
    scope: Option<(&str, u32)>,
    cursor_key: [u8; 16],
) -> Result<Vec<PersistedStringFragment>, ExecutionStoreError> {
    let candidates = protection.candidates(scope);
    let mut fragments = Vec::new();
    let mut cursor = 0;
    let mut literal_start = 0;
    while cursor < value.len() {
        let matched = candidates
            .iter()
            .find(|(_, material)| value[cursor..].starts_with(*material));
        if let Some((locator, material)) = matched {
            if literal_start < cursor {
                fragments.push(PersistedStringFragment::Literal {
                    value: value[literal_start..cursor].into(),
                });
            }
            fragments.push(PersistedStringFragment::SecretReference {
                locator: (*locator).clone(),
                material_binding: secret_material_binding(
                    protection, cursor_key, locator, material,
                )?,
            });
            cursor += material.len();
            literal_start = cursor;
        } else {
            let next = value[cursor..]
                .chars()
                .next()
                .ok_or_else(invalid_snapshot)?;
            cursor += next.len_utf8();
        }
    }
    if literal_start < value.len() || fragments.is_empty() {
        fragments.push(PersistedStringFragment::Literal {
            value: value[literal_start..].into(),
        });
    }
    Ok(fragments)
}

fn restore_string(
    fragments: Vec<PersistedStringFragment>,
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
) -> Result<String, ExecutionStoreError> {
    if fragments.is_empty() || fragments.len() > MAX_PERSISTED_STRING_FRAGMENTS {
        return Err(invalid_snapshot());
    }
    let mut restored = String::new();
    let fragment_count = fragments.len();
    let mut previous_literal = false;
    for fragment in fragments {
        match fragment {
            PersistedStringFragment::Literal { value } => {
                if previous_literal || (value.is_empty() && fragment_count != 1) {
                    return Err(invalid_snapshot());
                }
                if value.len() > crate::MAX_CAPABILITY_ARGUMENT_BYTES {
                    return Err(invalid_snapshot());
                }
                restored.push_str(&value);
                previous_literal = true;
            }
            PersistedStringFragment::SecretReference {
                locator,
                material_binding,
            } => {
                if material_binding.len() != SNAPSHOT_HMAC_HEX_BYTES {
                    return Err(invalid_snapshot());
                }
                locator.validate()?;
                let material = protection.material(&locator).ok_or_else(invalid_snapshot)?;
                if !constant_time_eq(
                    material_binding.as_bytes(),
                    secret_material_binding(protection, cursor_key, &locator, material)?.as_bytes(),
                ) {
                    return Err(invalid_snapshot());
                }
                restored.push_str(material);
                previous_literal = false;
            }
        }
        if restored.len() > crate::MAX_CAPABILITY_ARGUMENT_BYTES {
            return Err(invalid_snapshot());
        }
    }
    Ok(restored)
}

fn join_json_pointer(parent: &str, child: &str) -> String {
    format!("{parent}/{child}")
}

fn secret_material_binding(
    protection: &PersistenceProtection,
    cursor_key: [u8; 16],
    locator: &PersistenceSecretLocator,
    material: &str,
) -> Result<String, ExecutionStoreError> {
    let locator = serde_jcs::to_vec(locator).map_err(|_| invalid_snapshot())?;
    let mut message = b"anima-core.persistence-secret-material.v1\0".to_vec();
    message.extend_from_slice(&cursor_key);
    message.extend_from_slice(&locator);
    message.push(0);
    message.extend_from_slice(material.as_bytes());
    Ok(encode_snapshot_hex(&hmac_sha256(
        protection.seal_key.bytes(),
        &message,
    )))
}

fn seal_snapshot_payload(
    payload: &[u8],
    protection: &PersistenceProtection,
) -> SealedSnapshotEnvelope {
    let mut authenticated = PERSISTENCE_SNAPSHOT_MAC_DOMAIN.to_vec();
    authenticated.extend_from_slice(payload);
    SealedSnapshotEnvelope {
        schema_version: PERSISTENCE_SNAPSHOT_SCHEMA_VERSION,
        payload_encoding: PERSISTENCE_SNAPSHOT_PAYLOAD_ENCODING.into(),
        payload: encode_snapshot_hex(payload),
        authentication: format!(
            "{}:{}",
            PERSISTENCE_SNAPSHOT_AUTHENTICATION,
            encode_snapshot_hex(&hmac_sha256(protection.seal_key.bytes(), &authenticated))
        ),
    }
}

fn decode_persisted_snapshot(
    bytes: &[u8],
    protection: &PersistenceProtection,
) -> Result<(State, [u8; 16]), ExecutionStoreError> {
    if bytes.len() > MAX_PERSISTENCE_SNAPSHOT_BYTES {
        return Err(invalid_snapshot());
    }
    let envelope: SealedSnapshotEnvelope =
        serde_json::from_slice(bytes).map_err(|_| invalid_snapshot())?;
    if envelope.schema_version != PERSISTENCE_SNAPSHOT_SCHEMA_VERSION
        || envelope.payload_encoding != PERSISTENCE_SNAPSHOT_PAYLOAD_ENCODING
        || envelope.payload.len() > MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_HEX_BYTES
    {
        return Err(invalid_snapshot());
    }
    let expected_prefix = format!("{PERSISTENCE_SNAPSHOT_AUTHENTICATION}:");
    if envelope.authentication.len() != expected_prefix.len() + SNAPSHOT_HMAC_HEX_BYTES {
        return Err(invalid_snapshot());
    }
    let payload = decode_snapshot_hex(&envelope.payload, MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_BYTES)?;
    let supplied = envelope
        .authentication
        .strip_prefix(&expected_prefix)
        .ok_or_else(invalid_snapshot)
        .and_then(|authentication| decode_snapshot_hex(authentication, 32))?;
    if supplied.len() != 32 {
        return Err(invalid_snapshot());
    }
    let mut authenticated = PERSISTENCE_SNAPSHOT_MAC_DOMAIN.to_vec();
    authenticated.extend_from_slice(&payload);
    let expected = hmac_sha256(protection.seal_key.bytes(), &authenticated);
    if !constant_time_eq(&supplied, &expected) {
        return Err(invalid_snapshot());
    }

    // The protected payload is parsed only after its external-key MAC authenticates it.
    let protected: PersistenceSnapshotWire =
        serde_json::from_slice(&payload).map_err(|_| invalid_snapshot())?;
    let canonical = restore_snapshot(protected, protection)?;
    validate_snapshot_payload_surfaces(&canonical, protection)?;
    decode_snapshot_wire(canonical)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_BYTES: usize = 64;
    let mut key_block = [0_u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK_BYTES];
    let mut outer_pad = [0x5c_u8; BLOCK_BYTES];
    for index in 0..BLOCK_BYTES {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner);
    outer.finalize().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn encode_snapshot_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_snapshot_hex(value: &str, maximum_bytes: usize) -> Result<Vec<u8>, ExecutionStoreError> {
    if value.len() % 2 != 0 || value.len() > maximum_bytes.saturating_mul(2) {
        return Err(invalid_snapshot());
    }
    let mut decoded = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = decode_snapshot_hex_nibble(pair[0]).ok_or_else(invalid_snapshot)?;
        let low = decode_snapshot_hex_nibble(pair[1]).ok_or_else(invalid_snapshot)?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn decode_snapshot_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn decode_snapshot_wire(wire: SnapshotWire) -> Result<(State, [u8; 16]), ExecutionStoreError> {
    if wire.schema_version != CANONICAL_SNAPSHOT_SCHEMA_VERSION || wire.cursor_key == [0; 16] {
        return Err(invalid_snapshot());
    }
    let mut state = State::default();
    for session in wire.sessions {
        if session.owner_id.is_nil()
            || session.session_id.is_nil()
            || session.version == 0
            || state
                .sessions
                .insert(
                    (session.owner_id, session.session_id),
                    StoredSession {
                        version: session.version,
                        owner_id: session.owner_id,
                        definition: session.definition,
                        policy: session.policy,
                    },
                )
                .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for run in wire.runs {
        let run_id = run.run.id();
        let stored = StoredRun::new_with_initial_input(
            run.owner_id,
            run.run,
            run.run_version,
            run.session_version,
            run.initial_input,
        )?;
        let session = state
            .sessions
            .get(&(run.owner_id, stored.run().session_id()))
            .ok_or_else(invalid_snapshot)?;
        if stored.run().definition_id() != session.definition.id()
            || stored.run().definition_version() != session.definition.version()
            || stored.session_version() > session.version
        {
            return Err(invalid_snapshot());
        }
        let mut steps = BTreeMap::new();
        let mut step_order = BTreeMap::new();
        for (key, sequence, step) in run.steps {
            if sequence == 0
                || key != step.logical_step_id()
                || step.run_id() != run_id
                || steps.insert(key.clone(), step).is_some()
                || step_order.insert(sequence, key).is_some()
            {
                return Err(invalid_snapshot());
            }
        }
        let mut attempts = BTreeMap::new();
        let mut attempt_order = BTreeMap::new();
        let mut attempts_fingerprint = super::checkpoint::HistoryFingerprint::default();
        for (invocation_id, attempt_number, sequence, attempt) in run.attempts {
            let key = (invocation_id, attempt_number);
            if sequence == 0
                || invocation_id != attempt.invocation().id()
                || attempt_number != attempt.attempt_number()
                || attempt.invocation().run_id() != run_id
                || attempts.insert(key, attempt.clone()).is_some()
                || attempt_order.insert(sequence, key).is_some()
            {
                return Err(invalid_snapshot());
            }
            attempts_fingerprint
                .include(&attempt)
                .map_err(ExecutionStoreError::from)?;
        }
        let mut results = BTreeMap::new();
        let mut completed_fingerprint = super::checkpoint::HistoryFingerprint::default();
        for (invocation_id, completed, result) in run.results {
            if invocation_id != completed.invocation().id()
                || completed.invocation().run_id() != run_id
                || results
                    .insert(
                        invocation_id,
                        StoredDurableResult {
                            completed: completed.clone(),
                            result,
                        },
                    )
                    .is_some()
            {
                return Err(invalid_snapshot());
            }
            completed_fingerprint
                .include(&completed)
                .map_err(ExecutionStoreError::from)?;
        }
        let aggregate = RunAggregate {
            stored,
            lease: run.lease,
            checkpoint_version: run.checkpoint_version,
            checkpoint: run.checkpoint.map(Arc::new),
            events: run.events,
            steps,
            step_order,
            next_step_sequence: run.next_step_sequence,
            attempts,
            attempt_order,
            next_attempt_sequence: run.next_attempt_sequence,
            attempts_fingerprint,
            results,
            completed_fingerprint,
        };
        if Some(aggregate.next_step_sequence)
            != aggregate
                .step_order
                .last_key_value()
                .map_or(Some(1), |(value, _)| value.checked_add(1))
            || Some(aggregate.next_attempt_sequence)
                != aggregate
                    .attempt_order
                    .last_key_value()
                    .map_or(Some(1), |(value, _)| value.checked_add(1))
            || validate_snapshot_run(run.owner_id, &aggregate).is_err()
            || state
                .runs
                .insert((run.owner_id, run_id), aggregate)
                .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for (owner_id, session_id, run_id) in wire.serial_claims {
        if !state.runs.contains_key(&(owner_id, run_id))
            || state
                .serial_claims
                .insert((owner_id, session_id), run_id)
                .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for (owner_id, receipt) in wire.receipts {
        if state
            .receipts
            .insert((owner_id, receipt.command_id()), receipt)
            .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for (owner_id, command_id, command) in wire.commands {
        if owner_id.is_nil()
            || command_id.is_nil()
            || command.session_id.is_nil()
            || command.run_id.is_nil()
            || command.payload_digest.is_nil()
            || state
                .commands
                .insert((owner_id, command_id), command)
                .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for outcome in wire.outcomes {
        let stored = StoredRun::new_with_initial_input(
            outcome.owner_id,
            outcome.run,
            outcome.run_version,
            outcome.session_version,
            outcome.initial_input,
        )?;
        if outcome.command_id != outcome.receipt.command_id()
            || state
                .outcomes
                .insert(
                    (outcome.owner_id, outcome.command_id),
                    ExecutionCommitOutcome::new(
                        stored,
                        outcome.receipt,
                        outcome.checkpoint_version,
                        outcome.checkpoint,
                        outcome.grant_consumption,
                    ),
                )
                .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for grant in wire.authoritative_grants {
        if state
            .authoritative_grants
            .insert(
                (grant.owner_id(), grant.authority_key_encoded().to_owned()),
                grant,
            )
            .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for policy in wire.authoritative_policies {
        if state
            .authoritative_policies
            .insert(
                (
                    policy.owner_id(),
                    policy.agent_definition_id().to_owned(),
                    policy.agent_definition_version(),
                ),
                policy,
            )
            .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    for key in wire.grant_consumptions {
        if !state.grant_consumptions.insert(key) {
            return Err(invalid_snapshot());
        }
    }
    for (owner_id, approval_id, record) in wire.approvals {
        if owner_id.is_nil()
            || approval_id.is_nil()
            || state
                .approvals
                .insert((owner_id, approval_id), record)
                .is_some()
        {
            return Err(invalid_snapshot());
        }
    }
    validate_snapshot_state(&state)?;
    Ok((state, wire.cursor_key))
}

fn validate_snapshot_run(
    owner_id: Uuid,
    aggregate: &RunAggregate,
) -> Result<(), ExecutionStoreError> {
    let run = aggregate.stored.run();
    if aggregate
        .lease
        .as_ref()
        .is_some_and(|lease| lease.validate().is_err() || lease.run_id() != run.id())
        || RuntimeEvent::validate_batch(1, &aggregate.events).is_err()
        || aggregate.events.iter().any(|event| {
            event.owner_id() != owner_id
                || event.session_id() != run.session_id()
                || event.run_id() != run.id()
        })
        || aggregate
            .step_order
            .keys()
            .copied()
            .enumerate()
            .any(|(index, sequence)| sequence != u64::try_from(index + 1).unwrap_or(u64::MAX))
        || aggregate
            .attempt_order
            .keys()
            .copied()
            .enumerate()
            .any(|(index, sequence)| sequence != u64::try_from(index + 1).unwrap_or(u64::MAX))
    {
        return Err(invalid_snapshot());
    }

    let mut last_attempt_by_invocation = BTreeMap::new();
    for key in aggregate.attempt_order.values() {
        let attempt = aggregate.attempts.get(key).ok_or_else(invalid_snapshot)?;
        let expected = last_attempt_by_invocation
            .get(&attempt.invocation().id())
            .copied()
            .map_or(1, |number: u32| number.checked_add(1).unwrap_or(0));
        if attempt.attempt_number() != expected {
            return Err(invalid_snapshot());
        }
        last_attempt_by_invocation.insert(attempt.invocation().id(), attempt.attempt_number());
    }
    for (invocation_id, stored) in &aggregate.results {
        let completed = &stored.completed;
        let attempt = aggregate
            .attempts
            .get(&(*invocation_id, completed.attempt_number()))
            .ok_or_else(invalid_snapshot)?;
        if attempt.invocation() != completed.invocation()
            || attempt.state() != super::AttemptRecordState::Completed
            || attempt.manifest() != completed.manifest()
            || attempt.recovery_mode() != completed.recovery_mode()
            || stored.result.schema_digest() != attempt.manifest().schema_digest()
            || stored.result.result_ref().handle() != completed.result_ref().value()
        {
            return Err(invalid_snapshot());
        }
    }
    if let Some(checkpoint) = &aggregate.checkpoint {
        checkpoint.validate().map_err(ExecutionStoreError::from)?;
        if checkpoint.run_id() != run.id()
            || checkpoint.session_id() != run.session_id()
            || checkpoint.definition().id() != run.definition_id()
            || checkpoint.definition().version() != run.definition_version()
            || checkpoint.state() != run.state()
            || checkpoint.last_durable_event_sequence()
                != aggregate.events.last().map_or(0, RuntimeEvent::sequence)
            || checkpoint.attempts().len() != aggregate.attempts.len()
            || checkpoint.completed_invocations().len() != aggregate.results.len()
            || checkpoint.attempts_fingerprint() != aggregate.attempts_fingerprint
            || checkpoint.completed_fingerprint() != aggregate.completed_fingerprint
            || checkpoint
                .cursor()
                .is_some_and(|cursor| !aggregate.steps.contains_key(cursor.logical_step_id()))
        {
            return Err(invalid_snapshot());
        }
    }
    Ok(())
}

fn validate_snapshot_state(state: &State) -> Result<(), ExecutionStoreError> {
    for ((owner_id, run_id), aggregate) in &state.runs {
        let session = state
            .sessions
            .get(&(*owner_id, aggregate.stored.run().session_id()))
            .ok_or_else(invalid_snapshot)?;
        if session.policy == SessionConcurrencyPolicy::Serial
            && !aggregate.stored.run().state().is_terminal()
            && state
                .serial_claims
                .get(&(*owner_id, aggregate.stored.run().session_id()))
                != Some(run_id)
        {
            return Err(invalid_snapshot());
        }
    }
    for ((owner_id, session_id), run_id) in &state.serial_claims {
        let session = state
            .sessions
            .get(&(*owner_id, *session_id))
            .ok_or_else(invalid_snapshot)?;
        let aggregate = state
            .runs
            .get(&(*owner_id, *run_id))
            .ok_or_else(invalid_snapshot)?;
        if session.policy != SessionConcurrencyPolicy::Serial
            || aggregate.stored.run().session_id() != *session_id
            || aggregate.stored.run().state().is_terminal()
        {
            return Err(invalid_snapshot());
        }
    }

    if state.receipts.len() != state.outcomes.len() || state.receipts.len() != state.commands.len()
    {
        return Err(invalid_snapshot());
    }
    for ((owner_id, command_id), receipt) in &state.receipts {
        let outcome = state
            .outcomes
            .get(&(*owner_id, *command_id))
            .ok_or_else(invalid_snapshot)?;
        let stored = outcome.stored_run();
        let command = state
            .commands
            .get(&(*owner_id, *command_id))
            .ok_or_else(invalid_snapshot)?;
        let aggregate = state
            .runs
            .get(&(*owner_id, stored.run().id()))
            .ok_or_else(invalid_snapshot)?;
        if outcome.receipt() != receipt
            || command.payload_digest != receipt.payload_digest()
            || command.run_id != stored.run().id()
            || command.session_id != stored.run().session_id()
            || stored.owner_id() != *owner_id
            || stored.run().session_id() != aggregate.stored.run().session_id()
            || stored.run_version() > aggregate.stored.run_version()
            || stored.session_version() > aggregate.stored.session_version()
            || outcome.checkpoint_version() > aggregate.checkpoint_version
        {
            return Err(invalid_snapshot());
        }
        if let Some(consumption) = outcome.grant_consumption() {
            if state
                .grant_consumptions
                .iter()
                .filter(|(consumption_owner, _, revision, invocation_id)| {
                    consumption_owner == owner_id
                        && *revision == consumption.grant_revision
                        && *invocation_id == consumption.logical_invocation_id
                })
                .count()
                != 1
            {
                return Err(invalid_snapshot());
            }
        }
    }
    for (_owner_id, authority_key, revision, invocation_id) in &state.grant_consumptions {
        if authority_key.is_empty() || *revision == 0 || invocation_id.is_nil() {
            return Err(invalid_snapshot());
        }
    }
    for ((owner_id, approval_id), record) in &state.approvals {
        let aggregate = state
            .runs
            .get(&(*owner_id, record.run_id))
            .ok_or_else(invalid_snapshot)?;
        if record.requested_at_ms < 0 || record.expires_at_ms <= record.requested_at_ms {
            return Err(invalid_snapshot());
        }
        match (
            record.decision_command_id,
            record.decision_kind,
            record.decided_at_ms,
        ) {
            (None, None, None) => {
                if aggregate.stored.run().state() != RunState::WaitingForApproval
                    || !aggregate
                        .stored
                        .run()
                        .pending_approval()
                        .is_some_and(|request| {
                            request.logical_invocation_id == *approval_id
                                && request.run_id == record.run_id
                                && request.requested_at_ms == record.requested_at_ms
                                && request.expires_at_ms == record.expires_at_ms
                        })
                {
                    return Err(invalid_snapshot());
                }
            }
            (Some(command_id), Some(crate::ApprovalDecisionKind::Approve), Some(decided_at_ms)) => {
                if command_id.is_nil()
                    || decided_at_ms < record.requested_at_ms
                    || decided_at_ms >= record.expires_at_ms
                    || !state.receipts.contains_key(&(*owner_id, command_id))
                    || state
                        .outcomes
                        .get(&(*owner_id, command_id))
                        .is_none_or(|outcome| outcome.stored_run().run().id() != record.run_id)
                {
                    return Err(invalid_snapshot());
                }
            }
            _ => return Err(invalid_snapshot()),
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct SystemExecutionClock;

#[async_trait]
impl ExecutionClock for SystemExecutionClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct ManualExecutionClock {
    now_ms: Arc<AtomicU64>,
    waker: Arc<AtomicWaker>,
}

impl ManualExecutionClock {
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(now_ms)),
            waker: Arc::new(AtomicWaker::new()),
        }
    }

    pub fn advance_ms(&self, duration_ms: u64) -> Result<(), ExecutionStoreError> {
        let advanced = self
            .now_ms
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |now| {
                now.checked_add(duration_ms)
            })
            .map(|_| ())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow));
        if advanced.is_ok() {
            self.waker.wake();
        }
        advanced
    }
}

impl Default for ManualExecutionClock {
    fn default() -> Self {
        Self::new(1_000_000)
    }
}

#[async_trait]
impl ExecutionClock for ManualExecutionClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    async fn wait_until_ms(&self, deadline_ms: u64) {
        futures::future::poll_fn(|context| {
            if self.now_ms() >= deadline_ms {
                return std::task::Poll::Ready(());
            }
            self.waker.register(context.waker());
            if self.now_ms() >= deadline_ms {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum HistoryCollection {
    Events = 1,
    Steps = 2,
    Attempts = 3,
}

impl InMemoryExecutionStore {
    fn read_window(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        collection: HistoryCollection,
        current_snapshot: u64,
        page: &super::StoreReadPage,
    ) -> Result<(u64, u64), ExecutionStoreError> {
        let Some(cursor) = page.cursor() else {
            return Ok((current_snapshot, 0));
        };
        let (cursor_owner, cursor_run, cursor_collection, snapshot, last) =
            self.decode_cursor(cursor)?;
        if cursor_owner != owner_id
            || cursor_run != run_id
            || cursor_collection != collection
            || snapshot > current_snapshot
            || last >= snapshot
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok((snapshot, last))
    }

    fn write_cursor(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        collection: HistoryCollection,
        snapshot: u64,
        last: u64,
    ) -> Result<super::StoreReadCursor, ExecutionStoreError> {
        let mut payload = Vec::with_capacity(50);
        payload.push(1);
        payload.push(collection as u8);
        payload.extend_from_slice(owner_id.as_bytes());
        payload.extend_from_slice(run_id.as_bytes());
        payload.extend_from_slice(&snapshot.to_be_bytes());
        payload.extend_from_slice(&last.to_be_bytes());
        let mut hasher = Sha256::new();
        hasher.update(&payload);
        hasher.update(self.cursor_key);
        payload.extend_from_slice(&hasher.finalize());
        super::StoreReadCursor::from_opaque(format!("sc1.{}", encode_hex(&payload)))
    }

    fn decode_cursor(
        &self,
        cursor: &super::StoreReadCursor,
    ) -> Result<(Uuid, Uuid, HistoryCollection, u64, u64), ExecutionStoreError> {
        let encoded = cursor
            .as_str()
            .strip_prefix("sc1.")
            .ok_or_else(invalid_cursor)?;
        let bytes = decode_hex(encoded).ok_or_else(invalid_cursor)?;
        if bytes.len() != 82 || bytes[0] != 1 {
            return Err(invalid_cursor());
        }
        let collection = match bytes[1] {
            1 => HistoryCollection::Events,
            2 => HistoryCollection::Steps,
            3 => HistoryCollection::Attempts,
            _ => return Err(invalid_cursor()),
        };
        let payload = &bytes[..50];
        let mut hasher = Sha256::new();
        hasher.update(payload);
        hasher.update(self.cursor_key);
        let expected = hasher.finalize();
        if expected
            .iter()
            .zip(&bytes[50..])
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            != 0
        {
            return Err(invalid_cursor());
        }
        let owner_id = Uuid::from_slice(&bytes[2..18]).map_err(|_| invalid_cursor())?;
        let run_id = Uuid::from_slice(&bytes[18..34]).map_err(|_| invalid_cursor())?;
        let snapshot = u64::from_be_bytes(bytes[34..42].try_into().map_err(|_| invalid_cursor())?);
        let last = u64::from_be_bytes(bytes[42..50].try_into().map_err(|_| invalid_cursor())?);
        Ok((owner_id, run_id, collection, snapshot, last))
    }
}

fn invalid_cursor() -> ExecutionStoreError {
    ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest)
}

fn page_take(page: &super::StoreReadPage) -> Result<usize, ExecutionStoreError> {
    usize::try_from(page.limit())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?
        .checked_add(1)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((hex_value(pair[0])? << 4) | hex_value(pair[1])?))
        .collect()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[async_trait]
impl ExecutionStore for InMemoryExecutionStore {
    async fn apply_authoritative_policy(
        &self,
        owner_id: Uuid,
        change: AuthoritativePolicyChange,
    ) -> Result<AuthoritativePolicyState, ExecutionStoreError> {
        if owner_id.is_nil() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let (definition_id, definition_version) = change.key();
        let key = (owner_id, definition_id, definition_version);
        let mut state = self.state.lock().await;
        let next = change.apply_to(state.authoritative_policies.get(&key))?;
        if next.owner_id() != owner_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        state.authoritative_policies.insert(key, next.clone());
        Ok(next)
    }

    async fn load_authoritative_policy(
        &self,
        owner_id: Uuid,
        agent_definition_id: &str,
        agent_definition_version: u32,
    ) -> Result<Option<AuthoritativePolicyState>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .authoritative_policies
            .get(&(
                owner_id,
                agent_definition_id.to_owned(),
                agent_definition_version,
            ))
            .cloned())
    }

    async fn apply_authoritative_grant(
        &self,
        owner_id: Uuid,
        change: AuthoritativeGrantChange,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError> {
        if owner_id.is_nil()
            || change
                .new_state()
                .is_some_and(|next| next.owner_id() != owner_id)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let mut state = self.state.lock().await;
        let key = (owner_id, change.authority_key().as_str().to_owned());
        let next = change.apply_to(state.authoritative_grants.get(&key))?;
        state.authoritative_grants.insert(
            (owner_id, next.authority_key_encoded().to_owned()),
            next.clone(),
        );
        Ok(next)
    }

    async fn load_authoritative_grant(
        &self,
        owner_id: Uuid,
        authority_key: &super::GrantAuthorityKey,
    ) -> Result<Option<AuthoritativeGrantState>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .authoritative_grants
            .get(&(owner_id, authority_key.as_str().to_owned()))
            .cloned())
    }

    async fn create_run(
        &self,
        owner_id: Uuid,
        request: CreateRun,
    ) -> Result<StoredRun, ExecutionStoreError> {
        let session = request.session();
        if owner_id.is_nil()
            || request.owner_id() != owner_id
            || request.run().state() != RunState::Queued
            || request.run().session_id() != session.id()
            || request.run().definition_id() != session.definition().id()
            || request.run().definition_version() != session.definition().version()
            || session.concurrency()? != request.concurrency_policy()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }

        let mut state = self.state.lock().await;
        if state.runs.contains_key(&(owner_id, request.run().id())) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            ));
        }

        let session_key = (owner_id, session.id());
        let session_version = match state.sessions.get(&session_key) {
            Some(current)
                if current.version == request.expected_session_version()
                    && current.owner_id == request.owner_id()
                    && current.definition == *session.definition()
                    && current.policy == request.concurrency_policy() =>
            {
                current.version.checked_add(1).ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
                })?
            }
            Some(current)
                if current.owner_id != request.owner_id()
                    || current.definition != *session.definition() =>
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ))
            }
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::VersionConflict,
                ))
            }
            None if request.expected_session_version() == 0 => 1,
            None => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::VersionConflict,
                ))
            }
        };

        if request.concurrency_policy() == SessionConcurrencyPolicy::Serial
            && state.serial_claims.contains_key(&session_key)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::ActiveRunConflict,
            ));
        }

        state.sessions.insert(
            session_key,
            StoredSession {
                version: session_version,
                owner_id: request.owner_id(),
                definition: session.definition().clone(),
                policy: request.concurrency_policy(),
            },
        );
        if request.concurrency_policy() == SessionConcurrencyPolicy::Serial {
            state.serial_claims.insert(session_key, request.run().id());
        }
        let stored = StoredRun::new_with_initial_input(
            owner_id,
            request.run().clone(),
            1,
            session_version,
            request.initial_input().clone(),
        )?;
        state.runs.insert(
            (owner_id, request.run().id()),
            RunAggregate {
                stored: stored.clone(),
                lease: None,
                checkpoint_version: 0,
                checkpoint: None,
                events: vec![],
                steps: BTreeMap::new(),
                step_order: BTreeMap::new(),
                next_step_sequence: 1,
                attempts: BTreeMap::new(),
                attempt_order: BTreeMap::new(),
                next_attempt_sequence: 1,
                attempts_fingerprint: super::checkpoint::HistoryFingerprint::default(),
                results: BTreeMap::new(),
                completed_fingerprint: super::checkpoint::HistoryFingerprint::default(),
            },
        );
        Ok(stored)
    }

    async fn retry_run(
        &self,
        owner_id: Uuid,
        request: RetryRun,
    ) -> Result<RetryRunOutcome, ExecutionStoreError> {
        if owner_id.is_nil() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let mut state = self.state.lock().await;
        let (
            source_stored,
            source_checkpoint_version,
            session_id,
            definition_id,
            definition_version,
            initial_text,
        ) = {
            let source = state
                .runs
                .get(&(owner_id, request.source_run_id()))
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
            if !matches!(
                source.stored.run().state(),
                RunState::Failed | RunState::Cancelled
            ) {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
            (
                source.stored.clone(),
                source.checkpoint_version,
                source.stored.run().session_id(),
                source.stored.run().definition_id().to_owned(),
                source.stored.run().definition_version(),
                source.stored.initial_input().text_value().to_owned(),
            )
        };
        let command = super::RuntimeCommand::retry(
            request.command_id(),
            session_id,
            request.source_run_id(),
            request.new_run_id(),
        )
        .map_err(ExecutionStoreError::from)?;
        let command_key = (owner_id, request.command_id());
        if let Some(existing) = state.commands.get(&command_key) {
            if existing.session_id != session_id
                || existing.run_id != request.source_run_id()
                || existing.kind != super::RuntimeCommandKind::Retry
                || existing.payload_digest != command.payload_digest()
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::CommandConflict,
                ));
            }
            let run = state
                .runs
                .get(&(owner_id, request.new_run_id()))
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict))?
                .stored
                .clone();
            let receipt = state
                .receipts
                .get(&command_key)
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict))?
                .clone();
            return RetryRunOutcome::new(request.source_run_id(), run, receipt);
        }
        if state.runs.contains_key(&(owner_id, request.new_run_id())) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            ));
        }
        let session_key = (owner_id, session_id);
        let session = state
            .sessions
            .get(&session_key)
            .cloned()
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        if session.definition.id() != definition_id
            || session.definition.version() != definition_version
            || (session.policy == SessionConcurrencyPolicy::Serial
                && state.serial_claims.contains_key(&session_key))
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::ActiveRunConflict,
            ));
        }
        let session_version = session
            .version
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let run = super::Run::queued(
            request.new_run_id(),
            session_id,
            definition_id,
            definition_version,
        )
        .map_err(ExecutionStoreError::from)?;
        let input =
            super::DurableRunInput::text(owner_id, session_id, request.new_run_id(), initial_text)?;
        let stored = StoredRun::new_with_initial_input(owner_id, run, 1, session_version, input)?;
        state.sessions.insert(
            session_key,
            StoredSession {
                version: session_version,
                ..session
            },
        );
        if session.policy == SessionConcurrencyPolicy::Serial {
            state
                .serial_claims
                .insert(session_key, request.new_run_id());
        }
        state.runs.insert(
            (owner_id, request.new_run_id()),
            RunAggregate {
                stored: stored.clone(),
                lease: None,
                checkpoint_version: 0,
                checkpoint: None,
                events: vec![],
                steps: BTreeMap::new(),
                step_order: BTreeMap::new(),
                next_step_sequence: 1,
                attempts: BTreeMap::new(),
                attempt_order: BTreeMap::new(),
                next_attempt_sequence: 1,
                attempts_fingerprint: super::checkpoint::HistoryFingerprint::default(),
                results: BTreeMap::new(),
                completed_fingerprint: super::checkpoint::HistoryFingerprint::default(),
            },
        );
        let receipt = CommandReceipt::accepted(&command).map_err(ExecutionStoreError::from)?;
        state.receipts.insert(command_key, receipt.clone());
        state.outcomes.insert(
            command_key,
            ExecutionCommitOutcome::new(
                source_stored,
                receipt.clone(),
                source_checkpoint_version,
                None,
                None,
            ),
        );
        state.commands.insert(
            command_key,
            CommandIndexRecord {
                session_id,
                run_id: request.source_run_id(),
                kind: super::RuntimeCommandKind::Retry,
                payload_digest: command.payload_digest(),
            },
        );
        RetryRunOutcome::new(request.source_run_id(), stored, receipt)
    }

    async fn acquire_lease(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        if duration_ms == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let mut state = self.state.lock().await;
        let aggregate = state
            .runs
            .get_mut(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        if aggregate.stored.run_version() != expected_run_version {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            ));
        }
        if aggregate.stored.run().state().is_terminal() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let now_ms = self.clock.now_ms();
        if aggregate
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at_ms() > now_ms)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseConflict,
            ));
        }
        let expires_at_ms = now_ms
            .checked_add(duration_ms)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let lease = ExecutionLease::new(run_id, Uuid::new_v4(), expires_at_ms)
            .map_err(ExecutionStoreError::from)?;
        aggregate.lease = Some(lease.clone());
        Ok(lease)
    }

    async fn renew_lease(
        &self,
        owner_id: Uuid,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        if duration_ms == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let mut state = self.state.lock().await;
        let aggregate = state
            .runs
            .get_mut(&(owner_id, lease.run_id()))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        if aggregate.lease.as_ref() != Some(&lease) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseExpired,
            ));
        }
        let now_ms = self.clock.now_ms();
        if lease.expires_at_ms() <= now_ms {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseExpired,
            ));
        }
        let expires_at_ms = now_ms
            .checked_add(duration_ms)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let renewed = ExecutionLease::new(lease.run_id(), lease.fence(), expires_at_ms)
            .map_err(ExecutionStoreError::from)?;
        aggregate.lease = Some(renewed.clone());
        Ok(renewed)
    }

    async fn commit_execution(
        &self,
        owner_id: Uuid,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError> {
        commit.validate_bounds()?;
        let mut state = self.state.lock().await;
        let run_id = commit.lease().run_id();
        if commit.command().target_run_id() != run_id || commit.target_run().id() != run_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let command_key = (owner_id, commit.command().id());
        if let Some(receipt) = state.receipts.get(&command_key) {
            receipt
                .replay(commit.command())
                .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::CommandConflict))?;
            return state
                .outcomes
                .get(&command_key)
                .cloned()
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound));
        }

        let mut patch = build_commit_patch(&state, owner_id, &commit, self.clock.now_ms())?;
        patch.checkpoint = match commit.into_checkpoint_mutation() {
            CheckpointMutation::Unchanged => patch.checkpoint,
            CheckpointMutation::Replace(checkpoint) => Some(Arc::from(checkpoint)),
            CheckpointMutation::Clear => None,
        };
        let outcome = ExecutionCommitOutcome::new_shared(
            patch.stored.clone(),
            patch.receipt.clone(),
            patch.checkpoint_version,
            patch.checkpoint.clone(),
            patch.grant_consumption.clone(),
        );
        apply_commit_patch(&mut state, owner_id, run_id, patch, outcome.clone());
        Ok(outcome)
    }
    async fn load_run(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<StoredRun>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .runs
            .get(&(owner_id, run_id))
            .map(|aggregate| aggregate.stored.clone()))
    }

    async fn load_checkpoint(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<(u64, super::CheckpointV1)>, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        Ok(aggregate
            .checkpoint
            .as_ref()
            .map(|checkpoint| (aggregate.checkpoint_version, checkpoint.as_ref().clone())))
    }

    async fn load_steps_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<super::StoreHistoryPage<super::Step>, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        let snapshot = aggregate
            .step_order
            .last_key_value()
            .map_or(0, |(sequence, _)| *sequence);
        let (snapshot, last) =
            self.read_window(owner_id, run_id, HistoryCollection::Steps, snapshot, &page)?;
        let take = page_take(&page)?;
        let mut items = aggregate
            .step_order
            .range((Excluded(last), Included(snapshot)))
            .take(take)
            .map(|(_, key)| {
                aggregate.steps.get(key).cloned().ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > usize::try_from(page.limit()).unwrap_or(usize::MAX);
        if has_more {
            items.pop();
        }
        let next_cursor = if has_more {
            let last = aggregate
                .step_order
                .range((Excluded(last), Included(snapshot)))
                .nth(items.len().saturating_sub(1))
                .map(|(sequence, _)| *sequence)
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })?;
            Some(self.write_cursor(owner_id, run_id, HistoryCollection::Steps, snapshot, last)?)
        } else {
            None
        };
        super::StoreHistoryPage::new(items, next_cursor)
    }

    async fn load_attempts_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<super::StoreHistoryPage<super::InvocationAttemptRecord>, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        let snapshot = aggregate
            .attempt_order
            .last_key_value()
            .map_or(0, |(sequence, _)| *sequence);
        let (snapshot, last) = self.read_window(
            owner_id,
            run_id,
            HistoryCollection::Attempts,
            snapshot,
            &page,
        )?;
        let take = page_take(&page)?;
        let mut page_entries = aggregate
            .attempt_order
            .range((Excluded(last), Included(snapshot)))
            .take(take)
            .collect::<Vec<_>>();
        let has_more = page_entries.len() > usize::try_from(page.limit()).unwrap_or(usize::MAX);
        if has_more {
            page_entries.pop();
        }
        let next_cursor = if has_more {
            let last = page_entries
                .last()
                .map(|(sequence, _)| **sequence)
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })?;
            Some(self.write_cursor(
                owner_id,
                run_id,
                HistoryCollection::Attempts,
                snapshot,
                last,
            )?)
        } else {
            None
        };
        let items = page_entries
            .into_iter()
            .map(|(_, key)| {
                aggregate.attempts.get(key).cloned().ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        super::StoreHistoryPage::new(items, next_cursor)
    }

    async fn load_durable_result(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError> {
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .results
            .get(&logical_invocation_id)
            .map(|stored| stored.result.clone()))
    }

    async fn replay_events(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<super::EventReplayPage, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        let current_snapshot = u64::try_from(aggregate.events.len())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let (snapshot, last) = self.read_window(
            owner_id,
            run_id,
            HistoryCollection::Events,
            current_snapshot,
            &page,
        )?;
        let start = usize::try_from(last)
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let end = usize::try_from(snapshot)
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let take = page_take(&page)?;
        let mut events = aggregate.events[start..end]
            .iter()
            .take(take)
            .cloned()
            .collect::<Vec<_>>();
        let has_more = events.len() > usize::try_from(page.limit()).unwrap_or(usize::MAX);
        if has_more {
            events.pop();
        }
        let next_cursor = if has_more {
            let last = events
                .last()
                .map(RuntimeEvent::sequence)
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::EventConflict))?;
            Some(self.write_cursor(owner_id, run_id, HistoryCollection::Events, snapshot, last)?)
        } else {
            None
        };
        super::EventReplayPage::new(events, next_cursor)
    }
}

fn build_commit_patch(
    state: &State,
    owner_id: Uuid,
    commit: &ExecutionCommit,
    now_ms: u64,
) -> Result<CommitPatch, ExecutionStoreError> {
    let run_id = commit.lease().run_id();
    let aggregate = state
        .runs
        .get(&(owner_id, run_id))
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
    if aggregate.stored.run_version() != commit.expected_run_version() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::VersionConflict,
        ));
    }
    if aggregate.checkpoint_version != commit.expected_checkpoint_version() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::CheckpointConflict,
        ));
    }
    if aggregate.lease.as_ref() != Some(commit.lease()) || commit.lease().expires_at_ms() <= now_ms
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::LeaseExpired,
        ));
    }
    if !valid_command_transition(
        aggregate.stored.run(),
        commit.target_run(),
        commit.command(),
        commit.approval(),
        commit.dispatch_grant(),
        commit.attempts(),
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }

    if commit.command().kind() == super::RuntimeCommandKind::PrepareDispatch {
        if let Some(recovery) = commit.command().recovery_record() {
            let expected_retry_number =
                recovery.attempt_number().checked_add(1).ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::LineageConflict)
                })?;
            let exact_uncertain = commit.attempts().iter().filter(|attempt| {
                attempt.state() == super::AttemptRecordState::Uncertain
                    && attempt.invocation() == recovery.invocation()
                    && attempt.attempt_number() == recovery.attempt_number()
                    && attempt.manifest() == recovery.pause().manifest()
                    && attempt.recovery_mode() == recovery.pause().manifest().recovery_mode()
            });
            let exact_dispatch = commit.attempts().iter().filter(|attempt| {
                attempt.state() == super::AttemptRecordState::Dispatching
                    && attempt.invocation() == recovery.invocation()
                    && attempt.attempt_number() == expected_retry_number
                    && attempt.manifest() == recovery.pause().manifest()
                    && attempt.recovery_mode() == recovery.pause().manifest().recovery_mode()
            });
            if commit.attempts().len() != 2
                || exact_uncertain.count() != 1
                || exact_dispatch.count() != 1
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::LineageConflict,
                ));
            }
        }
    }

    let dispatching_attempts = commit
        .attempts()
        .iter()
        .filter(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
        .collect::<Vec<_>>();
    match (dispatching_attempts.as_slice(), commit.policy_guard()) {
        ([attempt], Some(guard)) if guard.matches_attempt(owner_id, attempt) => {
            let authority = state
                .authoritative_policies
                .get(&(
                    owner_id,
                    guard.agent_definition_id().to_owned(),
                    guard.agent_definition_version(),
                ))
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::PolicyConflict))?;
            if !authority.validates(guard, now_ms) {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::PolicyConflict,
                ));
            }
        }
        ([], None) => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::PolicyConflict,
            ))
        }
    }

    let current_event_count = u64::try_from(aggregate.events.len())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let first_sequence = current_event_count
        .checked_add(1)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let event_count = u64::try_from(commit.events().len())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let last_sequence = current_event_count
        .checked_add(event_count)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    RuntimeEvent::validate_batch(first_sequence, commit.events())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::EventConflict))?;
    let session = state
        .sessions
        .get(&(owner_id, aggregate.stored.run().session_id()))
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
    if commit.events().iter().any(|event| {
        event.owner_id() != session.owner_id
            || event.run_id() != run_id
            || event.session_id() != aggregate.stored.run().session_id()
    }) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::EventConflict,
        ));
    }
    if let CheckpointMutation::Replace(checkpoint) = commit.checkpoint_mutation() {
        if checkpoint.run_id() != run_id
            || checkpoint.session_id() != aggregate.stored.run().session_id()
            || checkpoint.definition().id() != aggregate.stored.run().definition_id()
            || checkpoint.definition().version() != aggregate.stored.run().definition_version()
            || checkpoint.state() != commit.target_run().state()
            || checkpoint.pause_reason() != commit.target_run().pause_reason()
            || checkpoint
                .pending_approval()
                .map(|pending| pending.request())
                != commit.target_run().pending_approval()
            || commit.target_run().state() == RunState::RecoveryRequired
                && !checkpoint.uncertain_invocations().iter().any(|record| {
                    commit.target_run().recovery_pause() == Some(record.pause())
                        && commit.target_run().recovery_binding() == record.recovery_binding()
                })
            || checkpoint.last_durable_event_sequence() != last_sequence
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::CheckpointConflict,
            ));
        }
    }

    if commit.approval().is_some() && commit.dispatch_grant().is_some() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let mut authoritative_grant_update = None;
    let mut grant_consumption_key = None;
    if let Some(approval) = commit.approval() {
        let resumed = aggregate
            .stored
            .run()
            .apply_resume_command(commit.command(), Some(approval.claim()), None)
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
        if resumed.run() != commit.target_run()
            || resumed.grant_consumption() != approval.grant_consumption()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        match (
            approval.grant_id(),
            approval.grant_revision(),
            approval.grant_remaining_uses(),
        ) {
            (Some(_), Some(grant_revision), claimed_remaining) => {
                let binding = approval.claim().grant_authority_binding().ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                })?;
                let mut authoritative = state
                    .authoritative_grants
                    .get(&(owner_id, binding.authority_key().as_str().to_owned()))
                    .cloned()
                    .ok_or_else(|| {
                        ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                    })?;
                authoritative.validate_binding(binding, now_ms)?;
                if authoritative.revision() != grant_revision
                    || authoritative.remaining_uses() != claimed_remaining
                {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::GrantConflict,
                    ));
                }
                match (approval.grant_consumption(), claimed_remaining) {
                    (Some(consumption), Some(remaining)) => {
                        let snapshot =
                            approval
                                .claim()
                                .grant_consumption_snapshot()
                                .ok_or_else(|| {
                                    ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                                })?;
                        if snapshot.remaining_uses() != remaining
                            || !binding.matches_consumption(consumption)
                        {
                            return Err(ExecutionStoreError::new(
                                ExecutionStoreErrorCode::GrantConflict,
                            ));
                        }
                        let key = (
                            owner_id,
                            binding.authority_key().as_str().to_owned(),
                            consumption.grant_revision,
                            consumption.logical_invocation_id,
                        );
                        if state.grant_consumptions.contains(&key) {
                            return Err(ExecutionStoreError::new(
                                ExecutionStoreErrorCode::GrantAlreadyConsumed,
                            ));
                        }
                        authoritative = authoritative.consume(binding, now_ms)?;
                        grant_consumption_key = Some(key);
                    }
                    (None, None) => {}
                    _ => {
                        return Err(ExecutionStoreError::new(
                            ExecutionStoreErrorCode::GrantConflict,
                        ))
                    }
                }
                authoritative_grant_update = Some(authoritative);
            }
            (None, None, None) if approval.grant_consumption().is_none() => {}
            _ => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::GrantConflict,
                ))
            }
        }
    }
    if let Some(dispatch) = commit.dispatch_grant() {
        let attempt = commit
            .attempts()
            .iter()
            .find(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        if commit.command().kind() != super::RuntimeCommandKind::PrepareDispatch
            || !dispatch.matches_attempt(owner_id, attempt)
            || commit
                .attempts()
                .iter()
                .filter(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
                .count()
                != 1
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        let binding = dispatch.authority_binding();
        let authoritative = state
            .authoritative_grants
            .get(&(owner_id, binding.authority_key().as_str().to_owned()))
            .cloned()
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        if authoritative.remaining_uses() != Some(dispatch.remaining_uses())
            || !binding.matches_consumption(dispatch.consumption())
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        let key = (
            owner_id,
            binding.authority_key().as_str().to_owned(),
            dispatch.consumption().grant_revision,
            dispatch.consumption().logical_invocation_id,
        );
        if state.grant_consumptions.contains(&key) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantAlreadyConsumed,
            ));
        }
        authoritative_grant_update = Some(authoritative.consume(binding, now_ms)?);
        grant_consumption_key = Some(key);
    }

    let mut step_patches = BTreeMap::new();
    let mut next_step_sequence = aggregate.next_step_sequence;
    for step in commit.steps() {
        if step.run_id() != run_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        match aggregate.steps.get(step.logical_step_id()) {
            Some(existing) if existing == step => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        match step_patches.get(step.logical_step_id()) {
            Some((_, existing)) if existing == step => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        let sequence = next_step_sequence;
        next_step_sequence = next_step_sequence
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        step_patches.insert(step.logical_step_id().to_owned(), (sequence, step.clone()));
    }

    let mut attempt_patches = BTreeMap::new();
    let mut attempt_transition_keys = BTreeSet::new();
    let mut new_attempt_count = 0usize;
    let mut next_attempt_sequence = aggregate.next_attempt_sequence;
    let mut attempts_fingerprint = aggregate.attempts_fingerprint;
    for attempt in commit.attempts() {
        if attempt.invocation().run_id() != run_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let key = (attempt.invocation().id(), attempt.attempt_number());
        let existing_sequence = aggregate
            .attempt_order
            .iter()
            .find_map(|(sequence, stored_key)| (stored_key == &key).then_some(*sequence));
        match aggregate.attempts.get(&key) {
            Some(existing) if existing == attempt => continue,
            Some(existing)
                if matches!(
                    (existing.state(), attempt.state()),
                    (
                        super::AttemptRecordState::Dispatching,
                        super::AttemptRecordState::Completed | super::AttemptRecordState::Uncertain
                    ) | (
                        super::AttemptRecordState::Pending,
                        super::AttemptRecordState::Dispatching
                    )
                ) && existing.invocation() == attempt.invocation()
                    && existing.manifest() == attempt.manifest()
                    && existing.recovery_mode() == attempt.recovery_mode() =>
            {
                let sequence = existing_sequence.ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })?;
                attempts_fingerprint
                    .include(existing)
                    .map_err(ExecutionStoreError::from)?;
                attempts_fingerprint
                    .include(attempt)
                    .map_err(ExecutionStoreError::from)?;
                attempt_transition_keys.insert(key);
                attempt_patches.insert(key, (sequence, attempt.clone()));
                continue;
            }
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        match attempt_patches.get(&key) {
            Some((_, existing)) if existing == attempt => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        let sequence = next_attempt_sequence;
        next_attempt_sequence = next_attempt_sequence
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        attempts_fingerprint
            .include(attempt)
            .map_err(ExecutionStoreError::from)?;
        new_attempt_count = new_attempt_count
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        attempt_patches.insert(key, (sequence, attempt.clone()));
    }

    for (key, (_, dispatching)) in &attempt_patches {
        if dispatching.state() != super::AttemptRecordState::Dispatching
            || aggregate.attempts.contains_key(key)
        {
            continue;
        }
        let prior_dispatch = aggregate
            .attempts
            .iter()
            .find(|((invocation_id, _), attempt)| {
                *invocation_id == dispatching.invocation().id()
                    && attempt.state() == super::AttemptRecordState::Dispatching
            });
        let Some(((prior_invocation_id, prior_number), prior)) = prior_dispatch else {
            if dispatching.attempt_number() != 1
                || aggregate
                    .attempts
                    .keys()
                    .any(|(invocation_id, _)| *invocation_id == dispatching.invocation().id())
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::LineageConflict,
                ));
            }
            continue;
        };
        let expected_number = prior_number
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::LineageConflict))?;
        let transitioned = attempt_patches
            .get(&(*prior_invocation_id, *prior_number))
            .map(|(_, attempt)| attempt);
        if dispatching.attempt_number() != expected_number
            || prior.invocation() != dispatching.invocation()
            || prior.manifest() != dispatching.manifest()
            || prior.recovery_mode() != dispatching.recovery_mode()
            || !transitioned.is_some_and(|attempt| {
                attempt.state() == super::AttemptRecordState::Uncertain
                    && attempt.invocation() == prior.invocation()
                    && attempt.manifest() == prior.manifest()
                    && attempt.recovery_mode() == prior.recovery_mode()
            })
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LineageConflict,
            ));
        }
    }

    let mut result_patches = BTreeMap::new();
    let mut completed_fingerprint = aggregate.completed_fingerprint;
    for mutation in commit.results() {
        let completed = mutation.completed();
        if completed.invocation().run_id() != run_id
            || completed.result_ref().value() != mutation.result().result_ref().handle()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LineageConflict,
            ));
        }
        let key = completed.invocation().id();
        let attempt_key = (key, completed.attempt_number());
        let attempt = attempt_patches
            .get(&attempt_key)
            .map(|(_, attempt)| attempt)
            .or_else(|| aggregate.attempts.get(&attempt_key))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::LineageConflict))?;
        if attempt.invocation() != completed.invocation()
            || attempt.state() != super::AttemptRecordState::Completed
            || attempt.manifest() != completed.manifest()
            || attempt.recovery_mode() != completed.recovery_mode()
            || mutation.result().schema_digest() != attempt.manifest().schema_digest()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LineageConflict,
            ));
        }
        let value = StoredDurableResult {
            completed: completed.clone(),
            result: mutation.result().clone(),
        };
        match aggregate.results.get(&key) {
            Some(existing) if existing == &value => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::ResultConflict,
                ))
            }
            None => {}
        }
        match result_patches.get(&key) {
            Some(existing) if existing == &value => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::ResultConflict,
                ))
            }
            None => {}
        }
        completed_fingerprint
            .include(completed)
            .map_err(ExecutionStoreError::from)?;
        result_patches.insert(key, value);
    }

    for key in &attempt_transition_keys {
        let attempt = attempt_patches
            .get(key)
            .map(|(_, attempt)| attempt)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict))?;
        match attempt.state() {
            super::AttemptRecordState::Completed => {
                if !result_patches
                    .get(&attempt.invocation().id())
                    .is_some_and(|stored| {
                        stored.completed.invocation() == attempt.invocation()
                            && stored.completed.attempt_number() == attempt.attempt_number()
                            && stored.completed.manifest() == attempt.manifest()
                            && stored.completed.recovery_mode() == attempt.recovery_mode()
                    })
                {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::LineageConflict,
                    ));
                }
            }
            super::AttemptRecordState::Uncertain => {
                let exact_recovery = commit.command().recovery_record().is_some_and(|recovery| {
                    let pause = recovery.pause();
                    pause.invocation() == attempt.invocation()
                        && pause.attempt_number() == attempt.attempt_number()
                        && pause.manifest() == attempt.manifest()
                        && pause.manifest().recovery_mode() == attempt.recovery_mode()
                });
                if !exact_recovery {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::LineageConflict,
                    ));
                }
            }
            super::AttemptRecordState::Dispatching => {
                let pending = aggregate.stored.run().pending_approval();
                let exact_resume = commit.command().kind()
                    == super::RuntimeCommandKind::ResumeApproval
                    && commit.approval().is_some()
                    && aggregate.stored.run().state() == RunState::WaitingForApproval
                    && commit.target_run().state() == RunState::Running
                    && pending.is_some_and(|request| {
                        request.logical_invocation_id == attempt.invocation().id()
                            && request.canonical_argument_digest
                                == attempt.invocation().canonical_argument_digest()
                            && request.capability_id == attempt.invocation().capability_id()
                            && request.manifest_version == attempt.invocation().manifest_version()
                    });
                if !exact_resume {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::LineageConflict,
                    ));
                }
            }
            _ => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ));
            }
        }
    }

    if let CheckpointMutation::Replace(checkpoint) = commit.checkpoint_mutation() {
        let expected_attempts = aggregate
            .attempts
            .len()
            .checked_add(new_attempt_count)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let expected_completed = aggregate
            .results
            .len()
            .checked_add(result_patches.len())
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        if checkpoint.attempts().len() != expected_attempts
            || checkpoint.completed_invocations().len() != expected_completed
            || checkpoint.attempts_fingerprint() != attempts_fingerprint
            || checkpoint.completed_fingerprint() != completed_fingerprint
            || checkpoint.cursor().is_some_and(|cursor| {
                !aggregate.steps.contains_key(cursor.logical_step_id())
                    && !step_patches.contains_key(cursor.logical_step_id())
            })
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::CheckpointConflict,
            ));
        }
    }

    let aggregate_changed = aggregate.stored.run() != commit.target_run()
        || !commit.events().is_empty()
        || !step_patches.is_empty()
        || !attempt_patches.is_empty()
        || !result_patches.is_empty();
    if aggregate_changed && matches!(commit.checkpoint_mutation(), CheckpointMutation::Unchanged) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::CheckpointConflict,
        ));
    }
    if commit.target_run().state().is_terminal()
        && !matches!(commit.checkpoint_mutation(), CheckpointMutation::Clear)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::CheckpointConflict,
        ));
    }

    let checkpoint_version = match commit.checkpoint_mutation() {
        CheckpointMutation::Unchanged => aggregate.checkpoint_version,
        CheckpointMutation::Replace(_) => aggregate
            .checkpoint_version
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?,
        CheckpointMutation::Clear if aggregate.checkpoint.is_some() => aggregate
            .checkpoint_version
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?,
        CheckpointMutation::Clear => aggregate.checkpoint_version,
    };
    let checkpoint = match commit.checkpoint_mutation() {
        CheckpointMutation::Unchanged => aggregate.checkpoint.clone(),
        CheckpointMutation::Replace(_) => None,
        CheckpointMutation::Clear => None,
    };
    let run_version = aggregate
        .stored
        .run_version()
        .checked_add(1)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let mut stored = StoredRun::new_with_initial_input(
        owner_id,
        commit.target_run().clone(),
        run_version,
        aggregate.stored.session_version(),
        aggregate.stored.initial_input().clone(),
    )?;
    let mut lease = aggregate.lease.clone();
    let mut session_update = None;
    let mut release_serial_claim = false;
    if commit.target_run().state() == RunState::WaitingForApproval
        || (commit.target_run().state() == RunState::Paused
            && commit.target_run().pause_reason() == Some(super::RunPauseReason::Requested))
    {
        lease = None;
    }
    if commit.target_run().state().is_terminal() {
        if state
            .serial_claims
            .get(&(owner_id, commit.target_run().session_id()))
            == Some(&run_id)
        {
            let mut session = session.clone();
            session.version = session.version.checked_add(1).ok_or_else(|| {
                ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
            })?;
            stored = StoredRun::new_with_initial_input(
                owner_id,
                commit.target_run().clone(),
                run_version,
                session.version,
                aggregate.stored.initial_input().clone(),
            )?;
            session_update = Some(session);
            release_serial_claim = true;
        }
    }
    let receipt = CommandReceipt::accepted(commit.command()).map_err(ExecutionStoreError::from)?;
    let approval_index_update = match commit.command().kind() {
        super::RuntimeCommandKind::RequestApproval => {
            let request = commit
                .command()
                .approval_request()
                .cloned()
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
            let approval_id = request.logical_invocation_id;
            if state.approvals.contains_key(&(owner_id, approval_id)) {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::CommandConflict,
                ));
            }
            Some((
                approval_id,
                ApprovalIndexRecord {
                    run_id,
                    requested_at_ms: request.requested_at_ms,
                    expires_at_ms: request.expires_at_ms,
                    decision_command_id: None,
                    decision_kind: None,
                    decided_at_ms: None,
                },
            ))
        }
        super::RuntimeCommandKind::ResumeApproval => {
            let approval = commit
                .approval()
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
            let decision = approval.claim().binding().decision().clone();
            let approval_id = decision.request.logical_invocation_id;
            let existing = state
                .approvals
                .get(&(owner_id, approval_id))
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
            if existing.run_id != run_id
                || existing.requested_at_ms != decision.request.requested_at_ms
                || existing.expires_at_ms != decision.request.expires_at_ms
                || existing.decision_command_id.is_some()
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::CommandConflict,
                ));
            }
            Some((
                approval_id,
                ApprovalIndexRecord {
                    run_id,
                    requested_at_ms: existing.requested_at_ms,
                    expires_at_ms: existing.expires_at_ms,
                    decision_command_id: Some(commit.command().id()),
                    decision_kind: Some(decision.kind),
                    decided_at_ms: Some(decision.decided_at_ms),
                },
            ))
        }
        _ => None,
    };
    let approval_index_removal = if commit.command().kind()
        != super::RuntimeCommandKind::ResumeApproval
        && commit.target_run().pending_approval().is_none()
    {
        aggregate
            .stored
            .run()
            .pending_approval()
            .map(|pending| {
                let approval_id = pending.logical_invocation_id;
                let existing = state
                    .approvals
                    .get(&(owner_id, approval_id))
                    .ok_or_else(|| {
                        ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest)
                    })?;
                if existing.run_id != run_id
                    || existing.requested_at_ms != pending.requested_at_ms
                    || existing.expires_at_ms != pending.expires_at_ms
                    || existing.decision_command_id.is_some()
                {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::CommandConflict,
                    ));
                }
                Ok(approval_id)
            })
            .transpose()?
    } else {
        None
    };
    let grant_consumption = commit
        .approval()
        .and_then(|approval| approval.grant_consumption())
        .or_else(|| {
            commit
                .dispatch_grant()
                .map(|dispatch| dispatch.consumption())
        })
        .cloned();
    Ok(CommitPatch {
        stored,
        lease,
        checkpoint_version,
        checkpoint,
        events: commit.events().to_vec(),
        steps: step_patches
            .into_iter()
            .map(|(key, (sequence, step))| (key, sequence, step))
            .collect(),
        next_step_sequence,
        attempts: attempt_patches
            .into_iter()
            .map(|(key, (sequence, attempt))| (key, sequence, attempt))
            .collect(),
        next_attempt_sequence,
        attempts_fingerprint,
        results: result_patches.into_iter().collect(),
        completed_fingerprint,
        authoritative_grant_update,
        grant_consumption_key,
        session_update,
        release_serial_claim,
        receipt,
        command_index: CommandIndexRecord {
            session_id: commit.command().session_id(),
            run_id: commit.command().target_run_id(),
            kind: commit.command().kind(),
            payload_digest: commit.command().payload_digest(),
        },
        grant_consumption,
        approval_index_update,
        approval_index_removal,
    })
}

fn apply_commit_patch(
    state: &mut State,
    owner_id: Uuid,
    run_id: Uuid,
    patch: CommitPatch,
    outcome: ExecutionCommitOutcome,
) {
    let command_key = (owner_id, patch.receipt.command_id());
    let session_id = patch.stored.run().session_id();
    {
        let aggregate = state
            .runs
            .get_mut(&(owner_id, run_id))
            .expect("validated run aggregate must remain present under the owner lock");
        aggregate.stored = patch.stored;
        aggregate.lease = patch.lease;
        aggregate.checkpoint_version = patch.checkpoint_version;
        aggregate.checkpoint = patch.checkpoint;
        aggregate.events.extend(patch.events);
        for (key, sequence, step) in patch.steps {
            aggregate.step_order.insert(sequence, key.clone());
            aggregate.steps.insert(key, step);
        }
        aggregate.next_step_sequence = patch.next_step_sequence;
        for (key, sequence, attempt) in patch.attempts {
            aggregate.attempt_order.insert(sequence, key);
            aggregate.attempts.insert(key, attempt);
        }
        aggregate.next_attempt_sequence = patch.next_attempt_sequence;
        aggregate.attempts_fingerprint = patch.attempts_fingerprint;
        for (key, result) in patch.results {
            aggregate.results.insert(key, result);
        }
        aggregate.completed_fingerprint = patch.completed_fingerprint;
    }
    if let Some(authoritative) = patch.authoritative_grant_update {
        state.authoritative_grants.insert(
            (owner_id, authoritative.authority_key_encoded().to_owned()),
            authoritative,
        );
    }
    if let Some(key) = patch.grant_consumption_key {
        state.grant_consumptions.insert(key);
    }
    if let Some(session) = patch.session_update {
        state.sessions.insert((owner_id, session_id), session);
    }
    if patch.release_serial_claim {
        state.serial_claims.remove(&(owner_id, session_id));
    }
    if let Some(approval_id) = patch.approval_index_removal {
        state.approvals.remove(&(owner_id, approval_id));
    }
    if let Some((approval_id, record)) = patch.approval_index_update {
        state.approvals.insert((owner_id, approval_id), record);
    }
    state.receipts.insert(command_key, patch.receipt);
    state.commands.insert(command_key, patch.command_index);
    state.outcomes.insert(command_key, outcome);
}

fn valid_command_transition(
    current: &super::Run,
    target: &super::Run,
    command: &super::RuntimeCommand,
    approval: Option<&super::ApprovalGrantMutation>,
    dispatch_grant: Option<&super::DispatchGrantMutation>,
    attempts: &[super::InvocationAttemptRecord],
) -> bool {
    if current.id() != target.id()
        || current.session_id() != target.session_id()
        || current.definition_id() != target.definition_id()
        || current.definition_version() != target.definition_version()
        || command.session_id() != current.session_id()
        || command.target_run_id() != current.id()
        || current.state().is_terminal()
    {
        return false;
    }
    if attempts
        .iter()
        .any(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
        && !matches!(
            command.kind(),
            super::RuntimeCommandKind::PrepareDispatch | super::RuntimeCommandKind::ResumeApproval
        )
    {
        return false;
    }
    match command.kind() {
        super::RuntimeCommandKind::Start => {
            approval.is_none()
                && current.state() == RunState::Queued
                && target.state() == RunState::Running
        }
        super::RuntimeCommandKind::RecordProgress => {
            approval.is_none()
                && dispatch_grant.is_none()
                && attempts
                    .iter()
                    .all(|attempt| attempt.state() != super::AttemptRecordState::Dispatching)
                && current.state() == RunState::Running
                && current == target
        }
        super::RuntimeCommandKind::PrepareDispatch => {
            approval.is_none()
                && attempts
                    .iter()
                    .filter(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
                    .count()
                    == 1
                && current.state() == RunState::Running
                && current == target
        }
        super::RuntimeCommandKind::RequestApproval => {
            approval.is_none()
                && command.approval_request().is_some_and(|request| {
                    current
                        .wait_for_approval(request.clone())
                        .is_ok_and(|expected| &expected == target)
                })
        }
        super::RuntimeCommandKind::RequireRecovery => {
            approval.is_none()
                && command.recovery_record().is_some_and(|recovery| {
                    let pause = recovery.pause();
                    attempts.iter().any(|attempt| {
                        attempt.invocation() == pause.invocation()
                            && attempt.attempt_number() == pause.attempt_number()
                            && attempt.state() == super::AttemptRecordState::Uncertain
                            && attempt.manifest() == pause.manifest()
                            && attempt.recovery_mode() == pause.manifest().recovery_mode()
                    }) && current
                        .require_recovery(recovery.clone())
                        .is_ok_and(|expected| &expected == target)
                })
        }
        super::RuntimeCommandKind::Pause => {
            approval.is_none()
                && command.pause_reason().is_some_and(|reason| {
                    current
                        .transition(RunState::Paused, Some(reason))
                        .is_ok_and(|expected| &expected == target)
                })
        }
        super::RuntimeCommandKind::Resume => {
            approval.is_none()
                && dispatch_grant.is_none()
                && attempts
                    .iter()
                    .all(|attempt| attempt.state() != super::AttemptRecordState::Dispatching)
                && current
                    .resume(None, None)
                    .is_ok_and(|expected| &expected == target)
        }
        super::RuntimeCommandKind::Retry => false,
        super::RuntimeCommandKind::ResumeApproval => current
            .apply_resume_command(command, approval.map(|mutation| mutation.claim()), None)
            .is_ok_and(|outcome| outcome.run() == target),
        super::RuntimeCommandKind::ResumeRecovery => {
            approval.is_none()
                && current.state() == RunState::RecoveryRequired
                && current.recovery_pause().is_some_and(|pause| {
                    command.recovery_binding().is_some_and(|binding| {
                        current.recovery_binding() == Some(binding)
                            && pause.matches_resume_binding(binding)
                            && attempts.iter().any(|attempt| {
                                attempt.invocation().id() == binding.logical_invocation_id()
                                    && attempt.attempt_number() == binding.retry_attempt_number()
                                    && attempt.state() == super::AttemptRecordState::Pending
                                    && attempt.manifest().id() == binding.manifest_id()
                                    && attempt.manifest().version() == binding.manifest_version()
                                    && attempt.manifest().schema_digest()
                                        == binding.manifest_digest()
                                    && attempt.recovery_mode() == binding.recovery_mode()
                            })
                    })
                })
                && target.state() == RunState::Running
                && target.pause_reason().is_none()
                && target.pending_approval().is_none()
                && target.recovery_pause().is_none()
        }
        super::RuntimeCommandKind::Complete => {
            approval.is_none()
                && current
                    .transition(RunState::Completed, None)
                    .is_ok_and(|expected| &expected == target)
        }
        super::RuntimeCommandKind::Fail => {
            approval.is_none()
                && match current.state() {
                    RunState::Running => current
                        .transition(RunState::Failed, None)
                        .is_ok_and(|expected| &expected == target),
                    RunState::RecoveryRequired => current
                        .resolve_recovery_terminal(super::RecoveryTerminalResolution::Fail)
                        .is_ok_and(|expected| expected.run() == target),
                    _ => false,
                }
        }
        super::RuntimeCommandKind::Cancel => {
            approval.is_none()
                && current
                    .cancel_at_boundary()
                    .is_ok_and(|expected| &expected == target)
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    const SHORT_SECRET: &str = "task8-short-secret";
    const LONG_SECRET: &str = "task8-short-secret-with-long-suffix";
    const PROVIDER_SENTINEL: &str = "task8-provider-raw-sentinel";
    const EXECUTOR_SENTINEL: &str = "task8-executor-raw-sentinel";
    const SQL_SENTINEL: &str = "task8-sql-raw-sentinel";

    fn snapshot_seal_key() -> PersistenceSnapshotSealKey {
        PersistenceSnapshotSealKey::new([0x5a; 32]).expect("snapshot seal key")
    }

    fn no_secret_protection() -> PersistenceProtection {
        PersistenceProtection::payload_free(snapshot_seal_key()).expect("payload-free protection")
    }

    fn snapshot_payload(snapshot: &ExecutionStoreSnapshot) -> serde_json::Value {
        let envelope: SealedSnapshotEnvelope =
            serde_json::from_slice(snapshot.as_bytes()).expect("snapshot envelope");
        serde_json::from_slice(
            &decode_snapshot_hex(&envelope.payload, MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_BYTES)
                .expect("snapshot payload hex"),
        )
        .expect("snapshot payload")
    }

    fn reseal_payload(payload: &serde_json::Value, protection: &PersistenceProtection) -> Vec<u8> {
        let payload = serde_jcs::to_vec(payload).expect("canonical payload");
        serde_json::to_vec(&seal_snapshot_payload(&payload, protection)).expect("sealed payload")
    }

    fn persistence_manifest(secret_references: Vec<String>) -> crate::CapabilityManifest {
        crate::CapabilityManifest::new(crate::CapabilityManifestInput {
            id: "workspace.write".into(),
            version: 1,
            kind: crate::CapabilityKind::Workspace,
            label: "Write".into(),
            description: "Writes a workspace file".into(),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: serde_json::json!({"type": "object"}),
            side_effects: true,
            risk_level: crate::RiskLevel::High,
            host_permissions: vec![],
            secret_references,
            environment_requirements: vec![],
            timeout_ms: 1_000,
            cancellation_supported: true,
            max_retries: 0,
            idempotent: false,
            recovery_mode: crate::RecoveryMode::KeyedIdempotent,
            supports_streaming: false,
            supports_artifacts: false,
            supports_citations: false,
            compatibility: crate::RuntimeCompatibility {
                minimum_runtime_schema_version: 1,
                maximum_runtime_schema_version: 1,
                manifest_schema_version: 1,
            },
        })
        .expect("persistence manifest")
    }

    fn persistence_protection(values: [&str; 5]) -> PersistenceProtection {
        PersistenceProtection::allow_model_payloads(
            snapshot_seal_key(),
            vec![PersistenceCapabilitySecretInventory::new(
                &persistence_manifest(
                    (0..values.len())
                        .map(|index| format!("secret-{index}"))
                        .collect(),
                ),
                values
                    .into_iter()
                    .map(|value| PersistenceSecretMaterial::new(value).expect("secret material"))
                    .collect(),
            )
            .expect("secret inventory")],
        )
        .expect("persistence protection")
    }

    #[tokio::test]
    async fn capability_inventory_does_not_implicitly_enable_model_payload_persistence() {
        let owner_id = Uuid::from_u128(0x51a1);
        let session_id = Uuid::from_u128(0x51a2);
        let run_id = Uuid::from_u128(0x51a3);
        let store = InMemoryExecutionStore::default();
        let session = super::super::Session::new(
            session_id,
            "explicit-model-opt-in",
            1,
            SessionConcurrencyPolicy::Serial,
        )
        .unwrap();
        store
            .create_run(
                owner_id,
                super::super::CreateRun::new_for_owner(
                    owner_id,
                    session,
                    super::super::Run::queued(run_id, session_id, "explicit-model-opt-in", 1)
                        .unwrap(),
                    0,
                    SessionConcurrencyPolicy::Serial,
                )
                .with_initial_input(
                    super::super::DurableRunInput::text(
                        owner_id,
                        session_id,
                        run_id,
                        "must be explicitly permitted",
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let protection = PersistenceProtection::new(
            snapshot_seal_key(),
            vec![
                PersistenceCapabilitySecretInventory::new(&persistence_manifest(vec![]), vec![])
                    .unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(
            store.export_snapshot(&protection).await.unwrap_err().code(),
            ExecutionStoreErrorCode::InvalidRequest
        );
    }

    async fn sensitive_run_store() -> (
        InMemoryExecutionStore,
        Arc<ManualExecutionClock>,
        Uuid,
        Uuid,
        crate::LogicalInvocation,
        super::super::CheckpointV1,
    ) {
        use crate::{
            Attachment, AttachmentType, Budget, Content, DataValue, ModelGenerateResponse,
            ModelStopReason, ProviderTranscriptEntry, TokenUsage, ToolCall, Usage,
        };

        let clock = Arc::new(ManualExecutionClock::default());
        let store = InMemoryExecutionStore::with_clock(clock.clone());
        let owner_id = Uuid::from_u128(0x5201);
        let session_id = Uuid::from_u128(0x5202);
        let run_id = Uuid::from_u128(0x5203);
        let session = super::super::Session::new(
            session_id,
            "snapshot-agent",
            1,
            SessionConcurrencyPolicy::Serial,
        )
        .expect("session");
        let run = super::super::Run::queued(run_id, session_id, "snapshot-agent", 1).expect("run");
        store
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
            .await
            .expect("run creation");
        let created = store
            .load_run(owner_id, run_id)
            .await
            .expect("run load")
            .expect("created run");
        let lease = store
            .acquire_lease(owner_id, run_id, created.run_version(), 10_000)
            .await
            .expect("lease");

        let manifest = super::super::ManifestPin::from_manifest(&persistence_manifest(
            (0..5).map(|index| format!("secret-{index}")).collect(),
        ))
        .expect("manifest");
        let mut arguments = serde_json::json!({
            "nested": [
                format!("{LONG_SECRET}:{SHORT_SECRET}"),
                {
                    "collision": {
                        "kind": "secret_reference",
                        "reference": 0,
                    },
                    "provider": PROVIDER_SENTINEL,
                }
            ],
            "executor": EXECUTOR_SENTINEL,
            "sql": SQL_SENTINEL,
        });
        arguments
            .as_object_mut()
            .expect("argument object")
            .insert(format!("key-{SHORT_SECRET}"), Value::String("safe".into()));
        let invocation =
            crate::LogicalInvocation::new(run_id, "restart-step", "workspace.write", 1, arguments)
                .expect("invocation");
        let attempt = super::super::InvocationAttemptRecord::new_durable(
            &invocation,
            1,
            super::super::AttemptRecordState::Pending,
            manifest.clone(),
            crate::RecoveryMode::KeyedIdempotent,
        )
        .expect("attempt");

        let transcript = ProviderTranscriptEntry::from_model_response(&ModelGenerateResponse {
            content: Content {
                text: format!("provider={PROVIDER_SENTINEL}; secret={LONG_SECRET}"),
                attachments: Some(vec![Attachment {
                    attachment_type: AttachmentType::File,
                    name: "provider-output.txt".into(),
                    data: EXECUTOR_SENTINEL.into(),
                }]),
                metadata: Some(BTreeMap::from([
                    (
                        format!("metadata-{PROVIDER_SENTINEL}"),
                        DataValue::String(SQL_SENTINEL.into()),
                    ),
                    (
                        "nested".into(),
                        DataValue::Array(vec![DataValue::String(SHORT_SECRET.into())]),
                    ),
                ])),
            },
            tool_calls: Some(vec![ToolCall {
                id: "call-1".into(),
                name: "workspace.write".into(),
                args: BTreeMap::from([(
                    "content".into(),
                    DataValue::String(format!("{EXECUTOR_SENTINEL}:{SHORT_SECRET}")),
                )]),
            }]),
            usage: TokenUsage::default(),
            stop_reason: ModelStopReason::ToolCall,
        })
        .expect("provider transcript");
        let checkpoint = super::super::CheckpointV1Builder::new(
            session_id,
            run_id,
            session.definition().clone(),
            1,
            vec![manifest],
            Budget::default(),
            Usage::default(),
        )
        .state(RunState::Running, None)
        .cursor(Some(
            super::super::CheckpointCursor::new(invocation.id(), 1, "restart-step")
                .expect("checkpoint cursor"),
        ))
        .attempts(vec![attempt.clone()])
        .provider_transcript(vec![transcript])
        .build()
        .expect("checkpoint");
        let event = super::super::RuntimeEvent::new(
            Uuid::from_u128(0x5204),
            owner_id,
            session_id,
            run_id,
            1,
            1,
            super::super::RuntimeEventKind::RunStarted,
        )
        .expect("event");
        let running = run
            .transition(RunState::Running, None)
            .expect("running run");
        store
            .commit_execution(
                owner_id,
                super::super::ExecutionCommit::new(
                    created.run_version(),
                    0,
                    lease,
                    super::super::RuntimeCommand::start(
                        Uuid::from_u128(0x5205),
                        session_id,
                        run_id,
                    )
                    .expect("start command"),
                    vec![event],
                    vec![super::super::ExecutionStep::new(
                        run_id,
                        "restart-step",
                        super::super::StepKind::Capability,
                    )
                    .expect("step")],
                    vec![attempt],
                    vec![],
                    None,
                    running,
                )
                .with_checkpoint(checkpoint.clone()),
            )
            .await
            .expect("execution commit");

        (store, clock, owner_id, run_id, invocation, checkpoint)
    }

    async fn model_only_transcript_store() -> InMemoryExecutionStore {
        use crate::{
            Attachment, AttachmentType, Content, DataValue, ModelGenerateResponse, ModelStopReason,
            ProviderTranscriptEntry, TokenUsage, ToolCall,
        };

        let store = InMemoryExecutionStore::default();
        let owner_id = Uuid::from_u128(0x5301);
        let session_id = Uuid::from_u128(0x5302);
        let run_id = Uuid::from_u128(0x5303);
        let session = super::super::Session::new(
            session_id,
            "model-only-agent",
            1,
            SessionConcurrencyPolicy::Serial,
        )
        .expect("session");
        let queued =
            super::super::Run::queued(run_id, session_id, "model-only-agent", 1).expect("run");
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
            .await
            .expect("run creation");
        let lease = store
            .acquire_lease(owner_id, run_id, created.run_version(), 10_000)
            .await
            .expect("lease");
        let transcript = ProviderTranscriptEntry::from_model_response(&ModelGenerateResponse {
            content: Content {
                text: "model-only-sensitive-transcript".into(),
                attachments: Some(vec![Attachment {
                    attachment_type: AttachmentType::File,
                    name: "secret.txt".into(),
                    data: "model-only-sensitive-attachment".into(),
                }]),
                metadata: Some(BTreeMap::from([(
                    "private".into(),
                    DataValue::String("model-only-sensitive-metadata".into()),
                )])),
            },
            tool_calls: Some(vec![ToolCall {
                id: "model-call".into(),
                name: "model.tool".into(),
                args: BTreeMap::from([(
                    "secret".into(),
                    DataValue::String("model-only-sensitive-tool-argument".into()),
                )]),
            }]),
            usage: TokenUsage::default(),
            stop_reason: ModelStopReason::ToolCall,
        })
        .expect("provider transcript");
        let checkpoint = super::super::CheckpointV1Builder::new(
            session_id,
            run_id,
            session.definition().clone(),
            1,
            vec![],
            crate::Budget::default(),
            crate::Usage::default(),
        )
        .state(RunState::Running, None)
        .provider_transcript(vec![transcript])
        .build()
        .expect("model checkpoint");
        let event = super::super::RuntimeEvent::new(
            Uuid::from_u128(0x5304),
            owner_id,
            session_id,
            run_id,
            1,
            1,
            super::super::RuntimeEventKind::RunStarted,
        )
        .expect("event");
        store
            .commit_execution(
                owner_id,
                super::super::ExecutionCommit::new(
                    created.run_version(),
                    0,
                    lease,
                    super::super::RuntimeCommand::start(
                        Uuid::from_u128(0x5305),
                        session_id,
                        run_id,
                    )
                    .expect("start command"),
                    vec![event],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    queued.transition(RunState::Running, None).expect("running"),
                )
                .with_checkpoint(checkpoint),
            )
            .await
            .expect("checkpoint commit");
        store
    }

    async fn serial_run_snapshot() -> (ExecutionStoreSnapshot, Uuid, Uuid, Uuid) {
        let store = InMemoryExecutionStore::default();
        let owner_id = Uuid::from_u128(0x5102);
        let session_id = Uuid::from_u128(0x5103);
        let run_id = Uuid::from_u128(0x5104);
        let session = super::super::Session::new(
            session_id,
            "snapshot-agent",
            1,
            SessionConcurrencyPolicy::Serial,
        )
        .expect("session");
        let run = super::super::Run::queued(run_id, session_id, "snapshot-agent", 1).expect("run");
        store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    owner_id,
                    session,
                    run,
                    0,
                    SessionConcurrencyPolicy::Serial,
                ),
            )
            .await
            .expect("run creation");
        (
            store
                .export_snapshot(&no_secret_protection())
                .await
                .expect("snapshot"),
            owner_id,
            session_id,
            run_id,
        )
    }

    #[tokio::test]
    async fn projection_exposes_safe_indexes_without_sensitive_payloads() {
        let (store, _clock, owner_id, run_id, invocation, _checkpoint) =
            sensitive_run_store().await;

        let projection = store.export_projection().await;
        assert_eq!(projection.sessions().len(), 1);
        assert_eq!(projection.runs().len(), 1);
        assert_eq!(projection.serial_claims().len(), 1);
        assert_eq!(projection.leases().len(), 1);
        assert_eq!(projection.events().len(), 1);
        assert_eq!(projection.steps().len(), 1);
        assert_eq!(projection.attempts().len(), 1);
        assert_eq!(projection.logical_invocations().len(), 1);
        assert_eq!(projection.checkpoints().len(), 1);
        assert_eq!(projection.runs()[0].owner_id(), owner_id);
        assert_eq!(projection.runs()[0].run_id(), run_id);
        assert_eq!(projection.logical_invocations()[0].id(), invocation.id());

        let debug = format!("{projection:?}");
        for forbidden in [
            SHORT_SECRET,
            LONG_SECRET,
            PROVIDER_SENTINEL,
            EXECUTOR_SENTINEL,
            SQL_SENTINEL,
        ] {
            assert!(
                !debug.contains(forbidden),
                "projection debug must not expose persisted payload value {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn command_projection_survives_snapshot_and_rejects_linkage_tampering() {
        let clock = Arc::new(ManualExecutionClock::default());
        let store = InMemoryExecutionStore::with_clock(clock);
        let owner_id = Uuid::from_u128(0x520d);
        let session_id = Uuid::from_u128(0x520e);
        let run_id = Uuid::from_u128(0x520f);
        let command_id = Uuid::from_u128(0x5210);
        let session = super::super::Session::new(
            session_id,
            "command-projection-agent",
            1,
            SessionConcurrencyPolicy::Serial,
        )
        .expect("session");
        let queued = super::super::Run::queued(run_id, session_id, "command-projection-agent", 1)
            .expect("run");
        let created = store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    owner_id,
                    session,
                    queued.clone(),
                    0,
                    SessionConcurrencyPolicy::Serial,
                ),
            )
            .await
            .expect("create run");
        let lease = store
            .acquire_lease(owner_id, run_id, created.run_version(), 10_000)
            .await
            .expect("lease");
        let command = super::super::RuntimeCommand::start(command_id, session_id, run_id)
            .expect("start command");
        let payload_digest = command.payload_digest();
        store
            .commit_execution(
                owner_id,
                super::super::ExecutionCommit::new(
                    created.run_version(),
                    0,
                    lease,
                    command,
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    queued.transition(RunState::Running, None).expect("running"),
                ),
            )
            .await
            .expect("commit");

        let projection = store.export_projection().await;
        assert_eq!(projection.commands().len(), 1);
        let projected = &projection.commands()[0];
        assert_eq!(projected.owner_id(), owner_id);
        assert_eq!(projected.command_id(), command_id);
        assert_eq!(projected.session_id(), session_id);
        assert_eq!(projected.run_id(), run_id);
        assert_eq!(projected.kind(), super::super::RuntimeCommandKind::Start);
        assert_eq!(projected.payload_digest(), payload_digest);

        let protection = no_secret_protection();
        let snapshot = store.export_snapshot(&protection).await.expect("snapshot");
        let restored = InMemoryExecutionStore::from_snapshot(
            &snapshot,
            &protection,
            Arc::new(ManualExecutionClock::default()),
        )
        .expect("restore");
        assert_eq!(
            restored.export_projection().await.commands(),
            projection.commands()
        );

        let mut tampered = snapshot_payload(&snapshot);
        tampered["commands"][0][2]["run_id"] = serde_json::json!(Uuid::from_u128(0x5211));
        assert!(ExecutionStoreSnapshot::from_bytes(
            reseal_payload(&tampered, &protection),
            &protection,
        )
        .is_err());
    }

    #[tokio::test]
    async fn pending_request_approval_without_an_attempt_survives_snapshot_round_trip() {
        use crate::{
            CapabilityReferenceId, LogicalInvocation, PolicyContext, PolicyEngine,
            PolicyRestrictions, RuntimeCommand,
        };

        let clock = Arc::new(ManualExecutionClock::default());
        let store = InMemoryExecutionStore::with_clock(clock.clone());
        let owner_id = Uuid::from_u128(0x5207);
        let session_id = Uuid::from_u128(0x5208);
        let run_id = Uuid::from_u128(0x5209);
        let session = super::super::Session::new(
            session_id,
            "snapshot-agent",
            1,
            SessionConcurrencyPolicy::Serial,
        )
        .expect("session");
        let queued =
            super::super::Run::queued(run_id, session_id, "snapshot-agent", 1).expect("run");
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
            .await
            .expect("run creation");
        let lease = store
            .acquire_lease(owner_id, run_id, created.run_version(), 10_000)
            .await
            .expect("lease");
        let running = queued
            .transition(RunState::Running, None)
            .expect("running run");
        let started = store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    created.run_version(),
                    0,
                    lease.clone(),
                    RuntimeCommand::start(Uuid::from_u128(0x520a), session_id, run_id)
                        .expect("start command"),
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    running,
                ),
            )
            .await
            .expect("start commit");
        let manifest = persistence_manifest(vec![]);
        let invocation = LogicalInvocation::new(
            run_id,
            "approval-step",
            manifest.id.clone(),
            manifest.version,
            serde_json::json!({ "path": "snapshot.txt" }),
        )
        .expect("invocation");
        let context = PolicyContext::new(
            "approval-owner",
            "approval-actor",
            "snapshot-agent",
            1,
            "approval-workspace",
            CapabilityReferenceId::new(Uuid::from_u128(0x520b)),
            &manifest,
            &invocation,
            1,
            PolicyRestrictions::default(),
            1_000,
        )
        .expect("policy context");
        let request = PolicyEngine::approval_request(&context, None).expect("approval request");
        let waiting = started
            .stored_run()
            .run()
            .wait_for_approval(request.clone())
            .expect("waiting run");
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    started.stored_run().run_version(),
                    started.checkpoint_version(),
                    lease,
                    RuntimeCommand::request_approval(
                        Uuid::from_u128(0x520c),
                        session_id,
                        run_id,
                        request.clone(),
                    )
                    .expect("request command"),
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    waiting,
                ),
            )
            .await
            .expect("approval commit");

        let snapshot = store
            .export_snapshot(&no_secret_protection())
            .await
            .expect("snapshot");
        let restored =
            InMemoryExecutionStore::from_snapshot(&snapshot, &no_secret_protection(), clock)
                .expect("restore");
        let restored_run = restored
            .load_run(owner_id, run_id)
            .await
            .expect("run load")
            .expect("run");
        assert_eq!(restored_run.run().pending_approval(), Some(&request));
    }

    #[tokio::test]
    async fn approval_projection_survives_snapshot_and_rejects_linkage_tampering() {
        use crate::{
            ApprovalDecision, ApprovalDecisionKind, ApprovalGrantMutation, ApprovalResumeClaim,
            CapabilityReferenceId, CheckpointV1Builder, PendingApprovalRecord, PolicyContext,
            PolicyEngine, PolicyRestrictions, RuntimeCommand,
        };

        let (store, clock, owner_id, run_id, invocation, checkpoint) = sensitive_run_store().await;
        let manifest =
            persistence_manifest((0..5).map(|index| format!("secret-{index}")).collect());
        let context = PolicyContext::new(
            "approval-owner",
            "approval-actor",
            "snapshot-agent",
            1,
            "approval-workspace",
            CapabilityReferenceId::new(Uuid::from_u128(0x5210)),
            &manifest,
            &invocation,
            1,
            PolicyRestrictions::default(),
            1_000,
        )
        .expect("policy context");
        let request = PolicyEngine::approval_request(&context, None).expect("approval request");
        let stored = store
            .load_run(owner_id, run_id)
            .await
            .expect("run load")
            .expect("run");
        let checkpoint_version = store
            .load_checkpoint(owner_id, run_id)
            .await
            .expect("checkpoint load")
            .expect("checkpoint")
            .0;
        let waiting = stored
            .run()
            .wait_for_approval(request.clone())
            .expect("waiting run");
        let waiting_checkpoint = CheckpointV1Builder::new(
            checkpoint.session_id(),
            checkpoint.run_id(),
            checkpoint.definition().clone(),
            checkpoint.last_durable_event_sequence(),
            checkpoint.manifests().to_vec(),
            checkpoint.budget().clone(),
            checkpoint.usage().clone(),
        )
        .state(RunState::WaitingForApproval, None)
        .cursor(checkpoint.cursor().cloned())
        .attempts(checkpoint.attempts().to_vec())
        .provider_transcript(checkpoint.provider_transcript().to_vec())
        .pending_approval(Some(
            PendingApprovalRecord::new(request.clone(), None).expect("pending"),
        ))
        .build()
        .expect("waiting checkpoint");
        let lease = store
            .state
            .lock()
            .await
            .runs
            .get(&(owner_id, run_id))
            .and_then(|aggregate| aggregate.lease.clone())
            .expect("lease");
        let waiting_outcome = store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    stored.run_version(),
                    checkpoint_version,
                    lease.clone(),
                    RuntimeCommand::request_approval(
                        Uuid::from_u128(0x5211),
                        checkpoint.session_id(),
                        run_id,
                        request.clone(),
                    )
                    .expect("request command"),
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    waiting,
                )
                .with_checkpoint(waiting_checkpoint),
            )
            .await
            .expect("waiting commit");

        let pending = store.export_projection().await;
        assert_eq!(pending.approvals().len(), 1);
        assert!(pending.decisions().is_empty());
        assert_eq!(pending.approvals()[0].id(), invocation.id());
        assert_eq!(pending.approvals()[0].decision_id(), None);

        let resume_context = PolicyContext::new(
            "approval-owner",
            "approval-actor",
            "snapshot-agent",
            1,
            "approval-workspace",
            CapabilityReferenceId::new(Uuid::from_u128(0x5210)),
            &manifest,
            &invocation,
            1,
            PolicyRestrictions::default(),
            1_001,
        )
        .expect("resume policy context");
        let decision = ApprovalDecision::new_approved(request, 1_001).expect("decision");
        let claim = ApprovalResumeClaim::new(&decision.request, &decision, &resume_context, &[])
            .expect("claim");
        let command_id = Uuid::from_u128(0x5212);
        let command = RuntimeCommand::resume_with_approval(
            command_id,
            checkpoint.session_id(),
            run_id,
            claim.binding().clone(),
        )
        .expect("resume command");
        let target = waiting_outcome
            .stored_run()
            .run()
            .apply_resume_command(&command, Some(&claim), None)
            .expect("resume target")
            .run()
            .clone();
        let lease = store
            .acquire_lease(
                owner_id,
                run_id,
                waiting_outcome.stored_run().run_version(),
                10_000,
            )
            .await
            .expect("resume lease");
        let resumed_checkpoint = CheckpointV1Builder::new(
            checkpoint.session_id(),
            checkpoint.run_id(),
            checkpoint.definition().clone(),
            checkpoint.last_durable_event_sequence(),
            checkpoint.manifests().to_vec(),
            checkpoint.budget().clone(),
            checkpoint.usage().clone(),
        )
        .state(RunState::Running, None)
        .cursor(checkpoint.cursor().cloned())
        .attempts(checkpoint.attempts().to_vec())
        .provider_transcript(checkpoint.provider_transcript().to_vec())
        .build()
        .expect("resumed checkpoint");
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    waiting_outcome.stored_run().run_version(),
                    waiting_outcome.checkpoint_version(),
                    lease,
                    command,
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                    Some(ApprovalGrantMutation::from_claim(claim)),
                    target,
                )
                .with_checkpoint(resumed_checkpoint),
            )
            .await
            .expect("resume commit");

        let projection = store.export_projection().await;
        assert_eq!(projection.approvals().len(), 1);
        assert_eq!(projection.decisions().len(), 1);
        assert_eq!(projection.approvals()[0].decision_id(), Some(command_id));
        assert!(projection.decisions()[0].claimed());
        assert_eq!(
            projection.decisions()[0].kind(),
            ApprovalDecisionKind::Approve
        );

        let protection = persistence_protection([
            SHORT_SECRET,
            LONG_SECRET,
            PROVIDER_SENTINEL,
            EXECUTOR_SENTINEL,
            SQL_SENTINEL,
        ]);
        let snapshot = store.export_snapshot(&protection).await.expect("snapshot");
        let restored =
            InMemoryExecutionStore::from_snapshot(&snapshot, &protection, clock).expect("restore");
        assert_eq!(
            restored.export_projection().await.approvals(),
            projection.approvals()
        );

        let mut tampered = snapshot_payload(&snapshot);
        tampered["approvals"][0][2]["run_id"] = serde_json::json!(Uuid::from_u128(0x5213));
        assert!(ExecutionStoreSnapshot::from_bytes(
            reseal_payload(&tampered, &protection),
            &protection
        )
        .is_err());
    }

    #[tokio::test]
    async fn projection_releases_serial_claim_when_a_run_becomes_terminal() {
        let (store, clock, owner_id, run_id, _invocation, _checkpoint) =
            sensitive_run_store().await;
        let before = store.export_projection().await;
        assert_eq!(before.serial_claims().len(), 1);

        clock.advance_ms(10_001).expect("lease expires");
        let stored = store
            .load_run(owner_id, run_id)
            .await
            .expect("load run")
            .expect("run exists");
        let lease = store
            .acquire_lease(owner_id, run_id, stored.run_version(), 10_000)
            .await
            .expect("replacement lease");
        let terminal = stored
            .run()
            .transition(RunState::Completed, None)
            .expect("terminal transition");
        store
            .commit_execution(
                owner_id,
                super::super::ExecutionCommit::new(
                    stored.run_version(),
                    1,
                    lease,
                    super::super::RuntimeCommand::complete(
                        Uuid::from_u128(0x5206),
                        stored.run().session_id(),
                        run_id,
                    )
                    .expect("complete command"),
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    terminal,
                ),
            )
            .await
            .expect("terminal commit");

        let projection = store.export_projection().await;
        assert!(projection.serial_claims().is_empty());
        assert_eq!(projection.runs()[0].state(), RunState::Completed);
    }

    #[tokio::test]
    async fn snapshot_round_trip_preserves_authoritative_state() {
        let clock = Arc::new(ManualExecutionClock::default());
        let store = InMemoryExecutionStore::with_clock(clock.clone());
        let owner_id = Uuid::from_u128(0x5101);
        let policy = AuthoritativePolicyState::active(owner_id, "snapshot-agent", 1, 1, None)
            .expect("policy");
        store
            .apply_authoritative_policy(owner_id, AuthoritativePolicyChange::create(policy.clone()))
            .await
            .expect("apply policy");

        let protection = no_secret_protection();
        let snapshot = store
            .export_snapshot(&protection)
            .await
            .expect("export snapshot");
        let restored = InMemoryExecutionStore::from_snapshot(&snapshot, &protection, clock)
            .expect("restore snapshot");
        assert_eq!(
            restored
                .load_authoritative_policy(owner_id, "snapshot-agent", 1)
                .await
                .expect("load policy"),
            Some(policy)
        );
    }

    #[test]
    fn malformed_snapshot_is_rejected_fail_closed() {
        let protection = no_secret_protection();
        assert!(ExecutionStoreSnapshot::from_bytes(br#"{}"#.to_vec(), &protection).is_err());
        assert!(ExecutionStoreSnapshot::from_bytes(b"not-json".to_vec(), &protection).is_err());
    }

    #[test]
    fn snapshot_rejects_oversized_envelope_before_decoding() {
        let protection = no_secret_protection();
        let oversized = vec![b' '; ExecutionStoreSnapshot::maximum_bytes() + 1];
        assert!(ExecutionStoreSnapshot::from_bytes(oversized, &protection).is_err());
    }

    #[test]
    fn payload_free_protection_rejects_an_undeclared_capability_scope() {
        let protection = no_secret_protection();
        assert!(protection
            .require_manifest_scope("workspace.write", 1)
            .is_err());
    }

    #[tokio::test]
    async fn payload_free_protection_rejects_model_only_transcript_snapshots() {
        let store = model_only_transcript_store().await;
        let protection = PersistenceProtection::payload_free(snapshot_seal_key())
            .expect("payload-free protection");

        assert!(store.export_snapshot(&protection).await.is_err());

        let state = store.state.lock().await;
        let canonical = encode_snapshot(&state, store.cursor_key);
        drop(state);
        let protected = protect_snapshot(canonical, &protection, store.cursor_key)
            .expect("legacy unguarded projection");
        let payload = serde_jcs::to_vec(&protected).expect("legacy payload");
        assert!(payload
            .windows("model-only-sensitive-transcript".len())
            .any(|window| window == b"model-only-sensitive-transcript"));
        let bytes = serde_json::to_vec(&seal_snapshot_payload(&payload, &protection))
            .expect("legacy envelope");
        assert!(ExecutionStoreSnapshot::from_bytes(bytes, &protection).is_err());
    }

    #[tokio::test]
    async fn snapshot_rejects_semantic_tampering() {
        let (snapshot, owner_id, _session_id, run_id) = serial_run_snapshot().await;
        let protection = no_secret_protection();

        let mut zero_cursor = snapshot_payload(&snapshot);
        zero_cursor["cursor_key"] = serde_json::json!(vec![0; 16]);
        assert!(ExecutionStoreSnapshot::from_bytes(
            reseal_payload(&zero_cursor, &protection),
            &protection,
        )
        .is_err());

        let mut foreign_serial_claim = snapshot_payload(&snapshot);
        foreign_serial_claim["serial_claims"] =
            serde_json::json!([[owner_id, Uuid::from_u128(0x5105), run_id]]);
        assert!(ExecutionStoreSnapshot::from_bytes(
            reseal_payload(&foreign_serial_claim, &protection),
            &protection,
        )
        .is_err());

        let mut missing_serial_claim = snapshot_payload(&snapshot);
        missing_serial_claim["serial_claims"] = serde_json::json!([]);
        assert!(ExecutionStoreSnapshot::from_bytes(
            reseal_payload(&missing_serial_claim, &protection),
            &protection,
        )
        .is_err());
    }

    #[tokio::test]
    async fn persistence_snapshot_replaces_declared_values_and_restores_exact_bindings() {
        let (store, clock, owner_id, run_id, invocation, checkpoint) = sensitive_run_store().await;
        let protection = persistence_protection([
            SHORT_SECRET,
            LONG_SECRET,
            PROVIDER_SENTINEL,
            EXECUTOR_SENTINEL,
            SQL_SENTINEL,
        ]);

        let snapshot = store
            .export_snapshot(&protection)
            .await
            .expect("protected snapshot");
        let persisted = String::from_utf8(snapshot.as_bytes().to_vec()).expect("snapshot utf8");
        let persisted_payload = serde_json::to_string(&snapshot_payload(&snapshot))
            .expect("protected snapshot payload");
        for forbidden in [
            SHORT_SECRET,
            LONG_SECRET,
            PROVIDER_SENTINEL,
            EXECUTOR_SENTINEL,
            SQL_SENTINEL,
        ] {
            assert!(
                !persisted.contains(forbidden) && !persisted_payload.contains(forbidden),
                "raw sensitive value reached persistence: {forbidden}"
            );
        }
        assert!(persisted_payload.contains("/entries/"));
        assert!(persisted_payload.contains("\"pointer\""));
        assert!(persisted_payload.contains("secret_reference"));

        let restored = InMemoryExecutionStore::from_snapshot(&snapshot, &protection, clock)
            .expect("protected restore");
        let attempts = restored
            .load_attempts_page(
                owner_id,
                run_id,
                super::super::StoreReadPage::first(8).expect("page"),
            )
            .await
            .expect("attempt history");
        let restored_invocation = attempts.items()[0]
            .durable_invocation()
            .expect("durable invocation")
            .expect("invocation arguments");
        assert_eq!(restored_invocation.binding(), invocation.binding());
        assert_eq!(
            restored_invocation.normalized_arguments(),
            invocation.normalized_arguments()
        );
        assert_eq!(
            restored
                .load_checkpoint(owner_id, run_id)
                .await
                .expect("checkpoint load"),
            Some((1, checkpoint))
        );
    }

    #[tokio::test]
    async fn missing_or_rotated_secret_reference_fails_closed() {
        let (store, clock, _owner_id, _run_id, _invocation, _checkpoint) =
            sensitive_run_store().await;
        let protection = persistence_protection([
            SHORT_SECRET,
            LONG_SECRET,
            PROVIDER_SENTINEL,
            EXECUTOR_SENTINEL,
            SQL_SENTINEL,
        ]);
        let snapshot = store
            .export_snapshot(&protection)
            .await
            .expect("protected snapshot");

        assert!(InMemoryExecutionStore::from_snapshot(
            &snapshot,
            &PersistenceProtection::payload_free(snapshot_seal_key())
                .expect("payload-free protection"),
            clock.clone(),
        )
        .is_err());

        let rotated = persistence_protection([
            "rotated-short",
            "rotated-long",
            "rotated-provider",
            "rotated-executor",
            "rotated-sql",
        ]);
        assert!(InMemoryExecutionStore::from_snapshot(&snapshot, &rotated, clock).is_err());
    }

    #[tokio::test]
    async fn legacy_raw_snapshot_schema_is_rejected() {
        let (store, _clock, _owner_id, _run_id, _invocation, _checkpoint) =
            sensitive_run_store().await;
        let protection = persistence_protection([
            SHORT_SECRET,
            LONG_SECRET,
            PROVIDER_SENTINEL,
            EXECUTOR_SENTINEL,
            SQL_SENTINEL,
        ]);
        let snapshot = store
            .export_snapshot(&protection)
            .await
            .expect("protected snapshot");
        let mut legacy: serde_json::Value =
            serde_json::from_slice(snapshot.as_bytes()).expect("snapshot json");
        legacy["schema_version"] = serde_json::json!(1);
        assert!(ExecutionStoreSnapshot::from_bytes(
            serde_json::to_vec(&legacy).expect("legacy json"),
            &protection,
        )
        .is_err());
    }

    #[tokio::test]
    async fn authenticated_snapshot_rejects_state_and_cached_outcome_tampering() {
        let (store, _clock, _owner_id, _run_id, _invocation, _checkpoint) =
            sensitive_run_store().await;
        let protection = persistence_protection([
            SHORT_SECRET,
            LONG_SECRET,
            PROVIDER_SENTINEL,
            EXECUTOR_SENTINEL,
            SQL_SENTINEL,
        ]);
        let snapshot = store
            .export_snapshot(&protection)
            .await
            .expect("protected snapshot");
        let envelope: SealedSnapshotEnvelope =
            serde_json::from_slice(snapshot.as_bytes()).expect("snapshot envelope");
        let original: serde_json::Value = serde_json::from_slice(
            &decode_snapshot_hex(&envelope.payload, MAX_PERSISTENCE_SNAPSHOT_PAYLOAD_BYTES)
                .expect("payload hex"),
        )
        .expect("snapshot payload");

        let mut state_tamper = original.clone();
        state_tamper["runs"][0]["run"]["state"] = serde_json::json!("paused");
        let mut state_envelope: serde_json::Value =
            serde_json::from_slice(snapshot.as_bytes()).expect("state envelope");
        state_envelope["payload"] = serde_json::json!(encode_snapshot_hex(
            &serde_json::to_vec(&state_tamper).expect("state payload")
        ));
        assert!(ExecutionStoreSnapshot::from_bytes(
            serde_json::to_vec(&state_envelope).expect("state tamper json"),
            &protection,
        )
        .is_err());

        let mut outcome_tamper = original;
        outcome_tamper["outcomes"][0]["run_version"] = serde_json::json!(999);
        let mut outcome_envelope: serde_json::Value =
            serde_json::from_slice(snapshot.as_bytes()).expect("outcome envelope");
        outcome_envelope["payload"] = serde_json::json!(encode_snapshot_hex(
            &serde_json::to_vec(&outcome_tamper).expect("outcome payload")
        ));
        assert!(ExecutionStoreSnapshot::from_bytes(
            serde_json::to_vec(&outcome_envelope).expect("outcome tamper json"),
            &protection,
        )
        .is_err());
    }
}
