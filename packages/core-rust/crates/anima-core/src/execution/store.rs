use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
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
    MemoryPolicy, ModelPolicy, OpaqueReference, PolicyContext, PolicyDecision, PolicyEngine,
    PolicyEvaluation, PolicyRestrictions, ProfileRef, RecoveryMode, RecoveryResumeBinding,
    RiskLevel, RuntimeCompatibility, RuntimeLimits,
};

const CONFORMANCE_KEYED_MANIFEST_ID: &str = "workspace.write";
const CONFORMANCE_KEYED_ALTERNATE_MANIFEST_ID: &str = "anima.conformance.workspace.keyed-alternate";
const CONFORMANCE_RETRY_MANIFEST_ID: &str = "anima.conformance.workspace.retry";
const CONFORMANCE_MANUAL_MANIFEST_ID: &str = "anima.conformance.workspace.manual";
const CONFORMANCE_NON_RETRYABLE_MANIFEST_ID: &str = "anima.conformance.workspace.non-retryable";
const CONFORMANCE_COMPENSATE_MANIFEST_ID: &str = "anima.conformance.workspace.compensate";
pub const MAX_DURABLE_RUN_INPUT_BYTES: usize = 32 * 1024;

/// The owner/run/session-bound user input that starts one durable run.
///
/// The payload is deliberately bounded and text-only. Hosts that persist non-empty inputs must
/// use a persistence protection mode that permits model payload surfaces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRunInput {
    owner_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
    text: String,
}

impl DurableRunInput {
    pub fn text(
        owner_id: Uuid,
        session_id: Uuid,
        run_id: Uuid,
        text: impl Into<String>,
    ) -> Result<Self, ExecutionStoreError> {
        let value = Self {
            owner_id,
            session_id,
            run_id,
            text: text.into(),
        };
        value.validate()?;
        Ok(value)
    }

    fn empty(owner_id: Uuid, session_id: Uuid, run_id: Uuid) -> Self {
        Self {
            owner_id,
            session_id,
            run_id,
            text: String::new(),
        }
    }

    fn validate(&self) -> Result<(), ExecutionStoreError> {
        if self.owner_id.is_nil()
            || self.session_id.is_nil()
            || self.run_id.is_nil()
            || self.text.len() > MAX_DURABLE_RUN_INPUT_BYTES
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(())
    }

    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn run_id(&self) -> Uuid {
        self.run_id
    }

    pub fn text_value(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

#[derive(Clone, Copy)]
enum ConformanceManifestKind {
    Keyed,
    KeyedAlternate,
    Retry,
    Manual,
    NonRetryable,
    Compensate,
}

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

    fn binding(&self) -> Result<GrantAuthorityBinding, ExecutionStoreError> {
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
    /// Digest of immutable grant authority fields; the live use count is bound separately.
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

    fn as_revoked(&self) -> Self {
        let mut revoked = self.clone();
        revoked.status = GrantStatus::Revoked;
        revoked
    }

    /// Validates that this exact authoritative snapshot is active at adapter time.
    pub fn validate_binding(
        &self,
        binding: &GrantAuthorityBinding,
        now_ms: u64,
    ) -> Result<(), ExecutionStoreError> {
        let now_ms = i64::try_from(now_ms)
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        if self.binding()? != *binding
            || self.status != GrantStatus::Active
            || now_ms < self.valid_from_ms
            || self.valid_until_ms.is_some_and(|until| now_ms >= until)
            || self.remaining_uses == Some(0)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        Ok(())
    }

    /// Returns the next counted-grant snapshot after validating the exact immutable binding.
    pub fn consume(
        &self,
        binding: &GrantAuthorityBinding,
        now_ms: u64,
    ) -> Result<Self, ExecutionStoreError> {
        self.validate_binding(binding, now_ms)?;
        let remaining = self
            .remaining_uses
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        let mut consumed = self.clone();
        consumed.remaining_uses = Some(
            remaining
                .checked_sub(1)
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?,
        );
        Ok(consumed)
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

    /// Applies a validated grant mutation as a pure CAS transition for durable adapters.
    pub fn apply_to(
        &self,
        current: Option<&AuthoritativeGrantState>,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError> {
        match (&self.kind, current) {
            (AuthoritativeGrantChangeKind::Create(next), None) => Ok(next.clone()),
            (AuthoritativeGrantChangeKind::Create(_), Some(_)) => Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            )),
            (
                AuthoritativeGrantChangeKind::Update {
                    expected_revision,
                    state: next,
                },
                Some(current),
            ) if current.owner_id == next.owner_id
                && current.authority_key == next.authority_key
                && current.revision == *expected_revision
                && next.revision > current.revision =>
            {
                Ok(next.clone())
            }
            (
                AuthoritativeGrantChangeKind::Revoke {
                    authority_key,
                    expected_revision,
                },
                Some(current),
            ) if current.authority_key == *authority_key
                && current.revision == *expected_revision =>
            {
                Ok(current.as_revoked())
            }
            _ => Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoritativePolicyStatus {
    Active,
    Revoked,
}

/// Store-authoritative high-water mark for one owner's pinned definition policy.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativePolicyState {
    owner_id: Uuid,
    agent_definition_id: String,
    agent_definition_version: u32,
    revision: u32,
    status: AuthoritativePolicyStatus,
    valid_until_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoritativePolicyStateWire {
    owner_id: Uuid,
    agent_definition_id: String,
    agent_definition_version: u32,
    revision: u32,
    status: AuthoritativePolicyStatus,
    valid_until_ms: Option<u64>,
}

impl AuthoritativePolicyState {
    pub fn active(
        owner_id: Uuid,
        agent_definition_id: impl Into<String>,
        agent_definition_version: u32,
        revision: u32,
        valid_until_ms: Option<u64>,
    ) -> Result<Self, ExecutionStoreError> {
        let value = Self {
            owner_id,
            agent_definition_id: agent_definition_id.into(),
            agent_definition_version,
            revision,
            status: AuthoritativePolicyStatus::Active,
            valid_until_ms,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), ExecutionStoreError> {
        if self.owner_id.is_nil()
            || self.agent_definition_id.trim().is_empty()
            || self.agent_definition_id.len() > crate::MAX_CAPABILITY_ID_BYTES
            || self.agent_definition_version == 0
            || self.revision == 0
            || self.valid_until_ms == Some(0)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(())
    }

    pub fn owner_id(&self) -> Uuid {
        self.owner_id
    }
    pub fn agent_definition_id(&self) -> &str {
        &self.agent_definition_id
    }
    pub fn agent_definition_version(&self) -> u32 {
        self.agent_definition_version
    }
    pub fn revision(&self) -> u32 {
        self.revision
    }
    pub fn status(&self) -> AuthoritativePolicyStatus {
        self.status
    }
    pub fn valid_until_ms(&self) -> Option<u64> {
        self.valid_until_ms
    }

    fn key(&self) -> (String, u32) {
        (
            self.agent_definition_id.clone(),
            self.agent_definition_version,
        )
    }

    fn revoked(&self) -> Self {
        let mut value = self.clone();
        value.status = AuthoritativePolicyStatus::Revoked;
        value
    }

    pub(super) fn validates(&self, guard: &DispatchPolicyGuard, now_ms: u64) -> bool {
        self.owner_id == guard.owner_id
            && self.agent_definition_id == guard.agent_definition_id
            && self.agent_definition_version == guard.agent_definition_version
            && self.revision == guard.policy_revision
            && self.status == AuthoritativePolicyStatus::Active
            && self.valid_until_ms.is_none_or(|until| now_ms < until)
    }
}

impl<'de> Deserialize<'de> for AuthoritativePolicyState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = AuthoritativePolicyStateWire::deserialize(deserializer)?;
        let value = Self {
            owner_id: wire.owner_id,
            agent_definition_id: wire.agent_definition_id,
            agent_definition_version: wire.agent_definition_version,
            revision: wire.revision,
            status: wire.status,
            valid_until_ms: wire.valid_until_ms,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

impl fmt::Debug for AuthoritativePolicyState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthoritativePolicyState")
            .field("owner_id", &"REDACTED")
            .field("agent_definition_id", &"REDACTED")
            .field("agent_definition_version", &self.agent_definition_version)
            .field("revision", &self.revision)
            .field("status", &self.status)
            .field("valid_until_ms", &self.valid_until_ms)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthoritativePolicyChange {
    Create(AuthoritativePolicyState),
    Update {
        expected_revision: u32,
        state: AuthoritativePolicyState,
    },
    Revoke {
        agent_definition_id: String,
        agent_definition_version: u32,
        expected_revision: u32,
    },
}

impl AuthoritativePolicyChange {
    pub fn create(state: AuthoritativePolicyState) -> Self {
        Self::Create(state)
    }

    pub fn update(
        expected_revision: u32,
        state: AuthoritativePolicyState,
    ) -> Result<Self, ExecutionStoreError> {
        if expected_revision == 0 || state.revision <= expected_revision {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self::Update {
            expected_revision,
            state,
        })
    }

    pub fn revoke(
        agent_definition_id: impl Into<String>,
        agent_definition_version: u32,
        expected_revision: u32,
    ) -> Result<Self, ExecutionStoreError> {
        let agent_definition_id = agent_definition_id.into();
        if agent_definition_id.trim().is_empty()
            || agent_definition_version == 0
            || expected_revision == 0
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self::Revoke {
            agent_definition_id,
            agent_definition_version,
            expected_revision,
        })
    }

    pub fn key(&self) -> (String, u32) {
        match self {
            Self::Create(state) | Self::Update { state, .. } => state.key(),
            Self::Revoke {
                agent_definition_id,
                agent_definition_version,
                ..
            } => (agent_definition_id.clone(), *agent_definition_version),
        }
    }

    pub fn apply_to(
        &self,
        current: Option<&AuthoritativePolicyState>,
    ) -> Result<AuthoritativePolicyState, ExecutionStoreError> {
        match (self, current) {
            (Self::Create(next), None) => Ok(next.clone()),
            (Self::Create(_), Some(_)) => Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            )),
            (
                Self::Update {
                    expected_revision,
                    state: next,
                },
                Some(current),
            ) if current.owner_id == next.owner_id
                && current.key() == next.key()
                && current.revision == *expected_revision
                && next.revision > current.revision =>
            {
                Ok(next.clone())
            }
            (
                Self::Revoke {
                    agent_definition_id,
                    agent_definition_version,
                    expected_revision,
                },
                Some(current),
            ) if current.agent_definition_id == *agent_definition_id
                && current.agent_definition_version == *agent_definition_version
                && current.revision == *expected_revision =>
            {
                Ok(current.revoked())
            }
            _ => Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            )),
        }
    }
}

/// Opaque exact-policy proof required on every external dispatch preparation.
#[derive(Clone)]
pub struct DispatchPolicyGuard {
    owner_id: Uuid,
    run_id: Uuid,
    agent_definition_id: String,
    agent_definition_version: u32,
    logical_invocation_id: Uuid,
    capability_id: String,
    manifest_version: u32,
    canonical_argument_digest: Uuid,
    policy_revision: u32,
    decision_digest: String,
}

impl DispatchPolicyGuard {
    pub fn from_current_policy(
        owner_id: Uuid,
        context: &PolicyContext,
        grants: &[AutonomyGrant],
        approval: Option<&ApprovalDecision>,
        evaluation: &PolicyEvaluation,
    ) -> Result<Self, ExecutionStoreError> {
        if owner_id.is_nil()
            || context.owner_id != owner_id.to_string()
            || !matches!(evaluation.decision, PolicyDecision::Allow(_))
            || PolicyEngine::evaluate_with_approval(context, grants, approval)
                .ok()
                .as_ref()
                != Some(evaluation)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::PolicyConflict,
            ));
        }
        let digest = serde_jcs::to_vec(evaluation)
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::PolicyConflict))?;
        Ok(Self {
            owner_id,
            run_id: context.run_id,
            agent_definition_id: context.agent_definition_id.clone(),
            agent_definition_version: context.agent_definition_version,
            logical_invocation_id: context.logical_invocation_id,
            capability_id: context.capability_id.clone(),
            manifest_version: context.manifest_version,
            canonical_argument_digest: context.canonical_argument_digest,
            policy_revision: context.policy_revision,
            decision_digest: format!("sha256:{:x}", Sha256::digest(digest)),
        })
    }

    pub fn policy_revision(&self) -> u32 {
        self.policy_revision
    }

    pub(super) fn agent_definition_id(&self) -> &str {
        &self.agent_definition_id
    }

    pub(super) fn agent_definition_version(&self) -> u32 {
        self.agent_definition_version
    }

    pub fn matches_attempt(&self, owner_id: Uuid, attempt: &InvocationAttemptRecord) -> bool {
        self.owner_id == owner_id
            && self.run_id == attempt.invocation().run_id()
            && self.logical_invocation_id == attempt.invocation().id()
            && self.capability_id == attempt.invocation().capability_id()
            && self.manifest_version == attempt.invocation().manifest_version()
            && self.canonical_argument_digest == attempt.invocation().canonical_argument_digest()
            && !self.decision_digest.is_empty()
    }
}

impl fmt::Debug for DispatchPolicyGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DispatchPolicyGuard")
            .field("owner_id", &"REDACTED")
            .field("run_id", &self.run_id)
            .field("logical_invocation_id", &self.logical_invocation_id)
            .field("policy_revision", &self.policy_revision)
            .field("decision_digest", &"REDACTED")
            .finish()
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
    initial_input: DurableRunInput,
}

impl CreateRun {
    pub fn new_for_owner(
        owner_id: Uuid,
        session: Session,
        run: Run,
        expected_session_version: u64,
        concurrency_policy: SessionConcurrencyPolicy,
    ) -> Self {
        let initial_input = DurableRunInput::empty(owner_id, session.id(), run.id());
        Self {
            owner_id,
            session,
            run,
            expected_session_version,
            concurrency_policy,
            initial_input,
        }
    }

    pub fn with_initial_input(
        mut self,
        initial_input: DurableRunInput,
    ) -> Result<Self, ExecutionStoreError> {
        initial_input.validate()?;
        if initial_input.owner_id() != self.owner_id
            || initial_input.session_id() != self.session.id()
            || initial_input.run_id() != self.run.id()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        self.initial_input = initial_input;
        Ok(self)
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

    pub fn initial_input(&self) -> &DurableRunInput {
        &self.initial_input
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryRun {
    command_id: Uuid,
    source_run_id: Uuid,
    new_run_id: Uuid,
}

impl RetryRun {
    pub fn new(
        command_id: Uuid,
        source_run_id: Uuid,
        new_run_id: Uuid,
    ) -> Result<Self, ExecutionStoreError> {
        if command_id.is_nil()
            || source_run_id.is_nil()
            || new_run_id.is_nil()
            || source_run_id == new_run_id
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            command_id,
            source_run_id,
            new_run_id,
        })
    }

    pub fn command_id(&self) -> Uuid {
        self.command_id
    }

    pub fn source_run_id(&self) -> Uuid {
        self.source_run_id
    }

    pub fn new_run_id(&self) -> Uuid {
        self.new_run_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryRunOutcome {
    source_run_id: Uuid,
    run: StoredRun,
    receipt: CommandReceipt,
}

impl RetryRunOutcome {
    pub fn new(
        source_run_id: Uuid,
        run: StoredRun,
        receipt: CommandReceipt,
    ) -> Result<Self, ExecutionStoreError> {
        if source_run_id.is_nil() || source_run_id == run.run().id() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            source_run_id,
            run,
            receipt,
        })
    }

    pub fn source_run_id(&self) -> Uuid {
        self.source_run_id
    }

    pub fn run(&self) -> &StoredRun {
        &self.run
    }

    pub fn receipt(&self) -> &CommandReceipt {
        &self.receipt
    }
}

/// A versioned durable run returned by an execution-store adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRun {
    owner_id: Uuid,
    run: Run,
    run_version: u64,
    session_version: u64,
    initial_input: DurableRunInput,
}

/// A validated approval claim and its one-time grant consumption, committed together with a run.
#[derive(Clone, Debug)]
pub struct ApprovalGrantMutation {
    claim: ApprovalResumeClaim,
}

/// Exact counted AutoAllow authority consumed atomically with one dispatch-preparation commit.
#[derive(Clone)]
pub struct DispatchGrantMutation {
    owner_id: Uuid,
    run_id: Uuid,
    logical_invocation_id: Uuid,
    capability_id: String,
    canonical_argument_digest: Uuid,
    policy_revision: u32,
    consumption: GrantConsumption,
    authority_binding: GrantAuthorityBinding,
    remaining_uses: u32,
}

impl DispatchGrantMutation {
    pub fn from_current_policy(
        owner_id: Uuid,
        context: &PolicyContext,
        grants: &[AutonomyGrant],
        evaluation: &PolicyEvaluation,
    ) -> Result<Option<Self>, ExecutionStoreError> {
        let Some(consumption) = evaluation.consumption.as_ref() else {
            return Ok(None);
        };
        if owner_id.is_nil()
            || context.owner_id != owner_id.to_string()
            || context.run_id.is_nil()
            || context.logical_invocation_id != consumption.logical_invocation_id
            || context.canonical_argument_digest.is_nil()
            || context.policy_revision == 0
            || PolicyEngine::evaluate(context, grants).ok().as_ref() != Some(evaluation)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        let grant = grants
            .iter()
            .find(|grant| {
                grant.id == consumption.grant_id && grant.revision == consumption.grant_revision
            })
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        if grant.effect != GrantEffect::AutoAllow {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        let remaining_uses = grant
            .remaining_uses
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        let authority_binding =
            GrantAuthorityBinding::from_grant(grant).map_err(ExecutionStoreError::from)?;
        if !authority_binding.matches_consumption(consumption) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        Ok(Some(Self {
            owner_id,
            run_id: context.run_id,
            logical_invocation_id: context.logical_invocation_id,
            capability_id: context.capability_id.clone(),
            canonical_argument_digest: context.canonical_argument_digest,
            policy_revision: context.policy_revision,
            consumption: consumption.clone(),
            authority_binding,
            remaining_uses,
        }))
    }

    pub fn consumption(&self) -> &GrantConsumption {
        &self.consumption
    }
    pub fn authority_binding(&self) -> &GrantAuthorityBinding {
        &self.authority_binding
    }
    pub fn remaining_uses(&self) -> u32 {
        self.remaining_uses
    }
    pub fn matches_attempt(&self, owner_id: Uuid, attempt: &InvocationAttemptRecord) -> bool {
        self.owner_id == owner_id
            && self.run_id == attempt.invocation().run_id()
            && self.logical_invocation_id == attempt.invocation().id()
            && self.capability_id == attempt.invocation().capability_id()
            && self.canonical_argument_digest == attempt.invocation().canonical_argument_digest()
            && self.policy_revision > 0
    }
}

impl fmt::Debug for DispatchGrantMutation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DispatchGrantMutation")
            .field("owner_id", &"REDACTED")
            .field("run_id", &self.run_id)
            .field("logical_invocation_id", &self.logical_invocation_id)
            .field("capability_id", &"REDACTED")
            .field("canonical_argument_digest", &self.canonical_argument_digest)
            .field("policy_revision", &self.policy_revision)
            .field("consumption", &self.consumption)
            .field("authority_binding", &"REDACTED")
            .field("remaining_uses", &self.remaining_uses)
            .finish()
    }
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
    Replace(Box<CheckpointV1>),
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
    policy_guard: Option<DispatchPolicyGuard>,
    dispatch_grant: Option<DispatchGrantMutation>,
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
            policy_guard: None,
            dispatch_grant: None,
            checkpoint: CheckpointMutation::Clear,
            target_run,
        }
    }

    pub fn with_checkpoint(mut self, checkpoint: CheckpointV1) -> Self {
        self.checkpoint = CheckpointMutation::Replace(Box::new(checkpoint));
        self
    }

    pub fn with_dispatch_grant(mut self, mutation: DispatchGrantMutation) -> Self {
        self.dispatch_grant = Some(mutation);
        self
    }

    pub fn with_policy_guard(mut self, guard: DispatchPolicyGuard) -> Self {
        self.policy_guard = Some(guard);
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

    pub fn dispatch_grant(&self) -> Option<&DispatchGrantMutation> {
        self.dispatch_grant.as_ref()
    }

    pub fn policy_guard(&self) -> Option<&DispatchPolicyGuard> {
        self.policy_guard.as_ref()
    }

    pub fn checkpoint(&self) -> Option<&CheckpointV1> {
        match &self.checkpoint {
            CheckpointMutation::Replace(checkpoint) => Some(checkpoint.as_ref()),
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
        let initial_input = DurableRunInput::empty(owner_id, run.session_id(), run.id());
        Self::new_with_initial_input(owner_id, run, run_version, session_version, initial_input)
    }

    pub fn new_with_initial_input(
        owner_id: Uuid,
        run: Run,
        run_version: u64,
        session_version: u64,
        initial_input: DurableRunInput,
    ) -> Result<Self, ExecutionStoreError> {
        if owner_id.is_nil() || run_version == 0 || session_version == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        initial_input.validate()?;
        if initial_input.owner_id() != owner_id
            || initial_input.session_id() != run.session_id()
            || initial_input.run_id() != run.id()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok(Self {
            owner_id,
            run,
            run_version,
            session_version,
            initial_input,
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

    pub fn initial_input(&self) -> &DurableRunInput {
        &self.initial_input
    }
}

/// Stable categories returned by an [`ExecutionStore`](crate::ExecutionStore).
///
/// New categories may be added during the crate's pre-1.0 evolution. Downstream
/// callers must include a wildcard arm when matching this enum.
///
/// ```compile_fail
/// use anima_core::ExecutionStoreErrorCode;
///
/// fn classify(code: ExecutionStoreErrorCode) -> &'static str {
///     match code {
///         ExecutionStoreErrorCode::NotFound => "not_found",
///         ExecutionStoreErrorCode::VersionConflict => "version_conflict",
///         ExecutionStoreErrorCode::ActiveRunConflict => "active_run_conflict",
///         ExecutionStoreErrorCode::LeaseConflict => "lease_conflict",
///         ExecutionStoreErrorCode::LeaseExpired => "lease_expired",
///         ExecutionStoreErrorCode::CommandConflict => "command_conflict",
///         ExecutionStoreErrorCode::EventConflict => "event_conflict",
///         ExecutionStoreErrorCode::CheckpointConflict => "checkpoint_conflict",
///         ExecutionStoreErrorCode::GrantAlreadyConsumed => "grant_already_consumed",
///         ExecutionStoreErrorCode::GrantConflict => "grant_conflict",
///         ExecutionStoreErrorCode::PolicyConflict => "policy_conflict",
///         ExecutionStoreErrorCode::LineageConflict => "lineage_conflict",
///         ExecutionStoreErrorCode::HistoryConflict => "history_conflict",
///         ExecutionStoreErrorCode::BoundsExceeded => "bounds_exceeded",
///         ExecutionStoreErrorCode::ArithmeticOverflow => "arithmetic_overflow",
///         ExecutionStoreErrorCode::ResultConflict => "result_conflict",
///         ExecutionStoreErrorCode::StorageUnavailable => "storage_unavailable",
///         ExecutionStoreErrorCode::CorruptState => "corrupt_state",
///         ExecutionStoreErrorCode::InvalidRequest => "invalid_request",
///     }
/// }
/// ```
#[non_exhaustive]
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
    PolicyConflict,
    LineageConflict,
    HistoryConflict,
    BoundsExceeded,
    ArithmeticOverflow,
    ResultConflict,
    StorageUnavailable,
    CorruptState,
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
            ExecutionStoreErrorCode::PolicyConflict => {
                "policy authority is missing, stale, expired, or revoked"
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
            ExecutionStoreErrorCode::StorageUnavailable => {
                "execution store persistence is unavailable"
            }
            ExecutionStoreErrorCode::CorruptState => {
                "execution store durable state failed integrity validation"
            }
            ExecutionStoreErrorCode::InvalidRequest => "execution store request is invalid",
        })
    }
}

impl std::error::Error for ExecutionStoreError {}

/// Adapter port for durable execution state. Each operation is atomic.
#[async_trait]
pub trait ExecutionStore: Send + Sync {
    async fn apply_authoritative_policy(
        &self,
        owner_id: Uuid,
        change: AuthoritativePolicyChange,
    ) -> Result<AuthoritativePolicyState, ExecutionStoreError>;

    async fn load_authoritative_policy(
        &self,
        owner_id: Uuid,
        agent_definition_id: &str,
        agent_definition_version: u32,
    ) -> Result<Option<AuthoritativePolicyState>, ExecutionStoreError>;

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

    async fn retry_run(
        &self,
        _owner_id: Uuid,
        _request: RetryRun,
    ) -> Result<RetryRunOutcome, ExecutionStoreError> {
        Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ))
    }

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

#[async_trait]
pub trait ExecutionClock: Send + Sync {
    fn now_ms(&self) -> u64;

    async fn wait_until_ms(&self, deadline_ms: u64) {
        let delay_ms = deadline_ms.saturating_sub(self.now_ms());
        super::clock::wait_for_wall_clock_ms(delay_ms).await;
    }
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
    let initial_input = DurableRunInput::text(
        owner_id,
        session_id,
        run.id(),
        "portable execution store input",
    )?;
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session.clone(),
                run.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            )
            .with_initial_input(initial_input.clone())?,
        )
        .await?;
    if created.initial_input() != &initial_input
        || store
            .load_run(owner_id, run.id())
            .await?
            .as_ref()
            .map(StoredRun::initial_input)
            != Some(&initial_input)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::create(AuthoritativePolicyState::active(
                owner_id,
                session.definition().id(),
                session.definition().version(),
                1,
                None,
            )?),
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
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
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
        vec![manifest.clone()],
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
    assert_authoritative_policy_contract(factory).await?;
    assert_dispatching_attempt_transition_contract(factory).await?;
    assert_plain_prepare_dispatch_lineage_rejections(factory).await?;
    assert_retry_attempt_adjacency_rejections(factory).await?;
    assert_retry_attempt_lineage_contract(factory).await?;
    assert_auto_allow_dispatch_grant_contract(factory).await?;
    assert_policy_denied_transition_contract(factory).await?;
    assert_counted_grant_contract(factory).await?;
    assert_multi_use_counted_grant_contract(factory).await?;
    assert_counted_grant_revoke_race_contract(factory).await?;
    assert_durable_result_contract(factory).await
}

async fn assert_authoritative_policy_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(0xd0_01);
    let first = AuthoritativePolicyState::active(owner_id, "policy-contract", 1, 1, None)?;
    if store
        .apply_authoritative_policy(owner_id, AuthoritativePolicyChange::create(first.clone()))
        .await?
        != first
        || store
            .load_authoritative_policy(owner_id, "policy-contract", 1)
            .await?
            .as_ref()
            != Some(&first)
        || store
            .load_authoritative_policy(Uuid::from_u128(0xd0_02), "policy-contract", 1)
            .await?
            .is_some()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    if !matches!(
        store
            .apply_authoritative_policy(
                owner_id,
                AuthoritativePolicyChange::create(first.clone())
            )
            .await,
        Err(error) if error.code() == ExecutionStoreErrorCode::VersionConflict
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let second = AuthoritativePolicyState::active(owner_id, "policy-contract", 1, 2, None)?;
    let stale_high = AuthoritativePolicyState::active(owner_id, "policy-contract", 1, 10, None)?;
    if !matches!(
        store
            .apply_authoritative_policy(
                owner_id,
                AuthoritativePolicyChange::update(9, stale_high)?
            )
            .await,
        Err(error) if error.code() == ExecutionStoreErrorCode::VersionConflict
    ) || store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::update(1, second.clone())?,
        )
        .await?
        != second
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let revoked = store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::revoke("policy-contract", 1, 2)?,
        )
        .await?;
    if revoked.status() != AuthoritativePolicyStatus::Revoked || revoked.revision() != 2 {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let third = AuthoritativePolicyState::active(owner_id, "policy-contract", 1, 3, None)?;
    if store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::update(2, third.clone())?,
        )
        .await?
        != third
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_dispatching_attempt_transition_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let (store, owner_id, session, queued, started, lease) =
        create_running_run(factory, 0xd1_00).await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "dispatching-transition",
        "workspace.write",
        1,
        serde_json::json!({"path": "dispatching.txt"}),
    ))?;
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
    let dispatching = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Dispatching,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let bypass = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::record_progress(
                    Uuid::from_u128(0xd1_05),
                    session.id(),
                    queued.id(),
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![dispatching.clone()],
                vec![],
                None,
                started.stored_run().run().clone(),
            ),
        )
        .await;
    if !matches!(
        bypass,
        Err(error) if error.code() == ExecutionStoreErrorCode::InvalidRequest
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let prepared = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::prepare_dispatch(
                    Uuid::from_u128(0xd1_08),
                    session.id(),
                    queued.id(),
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![dispatching],
                vec![],
                None,
                started.stored_run().run().clone(),
            )
            .with_policy_guard(conformance_dispatch_guard(
                owner_id,
                &session,
                &invocation,
            )?),
        )
        .await?;
    let completed_attempt = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Completed,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let result_reference = CapabilityReferenceId::new(Uuid::from_u128(0xd1_06));
    let completed = conformance_value(CompletedInvocationRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let durable = conformance_value(DurableCapabilityResult::new(
        result_reference,
        format!("jcs-v1:{}", "e".repeat(64)),
        manifest.schema_digest(),
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let command =
        RuntimeCommand::record_progress(Uuid::from_u128(0xd1_07), session.id(), queued.id())
            .map_err(ExecutionStoreError::from)?;
    let completed_commit = ExecutionCommit::new(
        prepared.stored_run().run_version(),
        prepared.checkpoint_version(),
        lease,
        command,
        vec![],
        vec![],
        vec![completed_attempt.clone()],
        vec![DurableResultMutation::new(completed, durable.clone())],
        None,
        prepared.stored_run().run().clone(),
    );
    let completed_outcome = store
        .commit_execution(owner_id, completed_commit.clone())
        .await?;
    if store.commit_execution(owner_id, completed_commit).await? != completed_outcome
        || store
            .load_attempts_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [completed_attempt]
        || store
            .load_durable_result(owner_id, queued.id(), invocation.id())
            .await?
            .as_ref()
            != Some(&durable)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_retry_attempt_lineage_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let (store, owner_id, session, queued, started, lease) =
        create_running_run(factory, 0xd1_40).await?;
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "retry-lineage",
        "workspace.write",
        1,
        serde_json::json!({"path": "retry-lineage.txt"}),
    ))?;
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
    let first_dispatching = conformance_value(InvocationAttemptRecord::new_durable(
        &invocation,
        1,
        super::AttemptRecordState::Dispatching,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let first = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::prepare_dispatch(
                    Uuid::from_u128(0xd1_45),
                    session.id(),
                    queued.id(),
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![first_dispatching],
                vec![],
                None,
                started.stored_run().run().clone(),
            )
            .with_policy_guard(conformance_dispatch_guard(
                owner_id,
                &session,
                &invocation,
            )?),
        )
        .await?;
    let first_uncertain = conformance_value(InvocationAttemptRecord::new_durable(
        &invocation,
        1,
        super::AttemptRecordState::Uncertain,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let second_dispatching = conformance_value(InvocationAttemptRecord::new_durable(
        &invocation,
        2,
        super::AttemptRecordState::Dispatching,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let pause = conformance_value(super::RecoveryPauseRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        super::RecoveryPauseReason::AuthoritativeAbsence,
    ))?;
    let recovery = conformance_value(super::RecoveryRecord::new_with_pause(pause, None))?;
    let second = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                first.stored_run().run_version(),
                first.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::prepare_recovery_dispatch(
                    Uuid::from_u128(0xd1_46),
                    session.id(),
                    queued.id(),
                    recovery,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![first_uncertain.clone(), second_dispatching],
                vec![],
                None,
                first.stored_run().run().clone(),
            )
            .with_policy_guard(conformance_dispatch_guard(
                owner_id,
                &session,
                &invocation,
            )?),
        )
        .await?;
    let second_completed = conformance_value(InvocationAttemptRecord::new_durable(
        &invocation,
        2,
        super::AttemptRecordState::Completed,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    ))?;
    let result_reference = CapabilityReferenceId::new(Uuid::from_u128(0xd1_47));
    let completed = conformance_value(CompletedInvocationRecord::new(
        invocation.binding(),
        2,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let durable = conformance_value(DurableCapabilityResult::new(
        result_reference,
        format!("jcs-v1:{}", "1".repeat(64)),
        manifest.schema_digest(),
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                second.stored_run().run_version(),
                second.checkpoint_version(),
                lease,
                RuntimeCommand::record_progress(
                    Uuid::from_u128(0xd1_48),
                    session.id(),
                    queued.id(),
                )
                .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![second_completed.clone()],
                vec![DurableResultMutation::new(completed, durable)],
                None,
                second.stored_run().run().clone(),
            ),
        )
        .await?;
    let attempts = store
        .load_attempts_page(
            owner_id,
            queued.id(),
            StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
        )
        .await?;
    if attempts.items() != [first_uncertain, second_completed] {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PlainPrepareLineageCase {
    InitialAttemptTwo,
    MissingUncertainTransition,
    ChangedLogicalBinding,
    ChangedNormalizedArguments,
    ChangedManifest,
    ChangedRecoveryMode,
}

#[derive(Debug, PartialEq, Eq)]
struct ConformanceRunSnapshot {
    run: Option<StoredRun>,
    events: Vec<RuntimeEvent>,
    steps: Vec<Step>,
    attempts: Vec<InvocationAttemptRecord>,
    checkpoint: Option<(u64, CheckpointV1)>,
}

async fn conformance_run_snapshot<S: ExecutionStore>(
    store: &S,
    owner_id: Uuid,
    run_id: Uuid,
) -> Result<ConformanceRunSnapshot, ExecutionStoreError> {
    Ok(ConformanceRunSnapshot {
        run: store.load_run(owner_id, run_id).await?,
        events: replay_all_events(store, owner_id, run_id).await?,
        steps: store
            .load_steps_page(
                owner_id,
                run_id,
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            .to_vec(),
        attempts: store
            .load_attempts_page(
                owner_id,
                run_id,
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            .to_vec(),
        checkpoint: store.load_checkpoint(owner_id, run_id).await?,
    })
}

async fn assert_plain_prepare_dispatch_lineage_rejections<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    for (offset, case) in [
        PlainPrepareLineageCase::InitialAttemptTwo,
        PlainPrepareLineageCase::MissingUncertainTransition,
        PlainPrepareLineageCase::ChangedLogicalBinding,
        PlainPrepareLineageCase::ChangedNormalizedArguments,
        PlainPrepareLineageCase::ChangedManifest,
        PlainPrepareLineageCase::ChangedRecoveryMode,
    ]
    .into_iter()
    .enumerate()
    {
        let seed = 0xd1_20 + (offset as u128 * 0x10);
        let (store, owner_id, session, queued, started, lease) =
            create_running_run(factory, seed).await?;
        let invocation = conformance_value(LogicalInvocation::new(
            queued.id(),
            "plain-prepare-lineage",
            "workspace.write",
            1,
            serde_json::json!({"path": "lineage.txt"}),
        ))?;
        let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
        let baseline = if matches!(case, PlainPrepareLineageCase::InitialAttemptTwo) {
            started
        } else {
            let first = conformance_value(InvocationAttemptRecord::new_durable(
                &invocation,
                1,
                super::AttemptRecordState::Dispatching,
                manifest.clone(),
                RecoveryMode::KeyedIdempotent,
            ))?;
            store
                .commit_execution(
                    owner_id,
                    ExecutionCommit::new(
                        started.stored_run().run_version(),
                        started.checkpoint_version(),
                        lease.clone(),
                        RuntimeCommand::prepare_dispatch(
                            Uuid::from_u128(seed + 5),
                            session.id(),
                            queued.id(),
                        )
                        .map_err(ExecutionStoreError::from)?,
                        vec![],
                        vec![],
                        vec![first],
                        vec![],
                        None,
                        started.stored_run().run().clone(),
                    )
                    .with_policy_guard(conformance_dispatch_guard(
                        owner_id,
                        &session,
                        &invocation,
                    )?),
                )
                .await?
        };
        let candidate_invocation = match case {
            PlainPrepareLineageCase::ChangedLogicalBinding => {
                conformance_value(LogicalInvocation::new(
                    queued.id(),
                    "changed-plain-prepare-lineage",
                    "workspace.write",
                    1,
                    serde_json::json!({"path": "lineage.txt"}),
                ))?
            }
            PlainPrepareLineageCase::ChangedNormalizedArguments => {
                conformance_value(LogicalInvocation::new(
                    queued.id(),
                    "plain-prepare-lineage",
                    "workspace.write",
                    1,
                    serde_json::json!({"path": "changed.txt"}),
                ))?
            }
            PlainPrepareLineageCase::ChangedManifest => {
                let alternate = conformance_manifest(ConformanceManifestKind::KeyedAlternate);
                conformance_value(LogicalInvocation::new(
                    queued.id(),
                    "plain-prepare-lineage",
                    alternate.id.clone(),
                    alternate.version,
                    serde_json::json!({"path": "lineage.txt"}),
                ))?
            }
            PlainPrepareLineageCase::ChangedRecoveryMode => {
                let retry = conformance_manifest(ConformanceManifestKind::Retry);
                conformance_value(LogicalInvocation::new(
                    queued.id(),
                    "plain-prepare-lineage",
                    retry.id.clone(),
                    retry.version,
                    serde_json::json!({"path": "lineage.txt"}),
                ))?
            }
            _ => invocation.clone(),
        };
        let (candidate_manifest, candidate_mode) = match case {
            PlainPrepareLineageCase::ChangedManifest => (
                conformance_manifest_pin(ConformanceManifestKind::KeyedAlternate)?,
                RecoveryMode::KeyedIdempotent,
            ),
            PlainPrepareLineageCase::ChangedRecoveryMode => (
                conformance_manifest_pin(ConformanceManifestKind::Retry)?,
                RecoveryMode::Retry,
            ),
            _ => (manifest, RecoveryMode::KeyedIdempotent),
        };
        let candidate = conformance_value(InvocationAttemptRecord::new_durable(
            &candidate_invocation,
            2,
            super::AttemptRecordState::Dispatching,
            candidate_manifest,
            candidate_mode,
        ))?;
        let before = conformance_run_snapshot(&store, owner_id, queued.id()).await?;
        let rejected = store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    baseline.stored_run().run_version(),
                    baseline.checkpoint_version(),
                    lease,
                    RuntimeCommand::prepare_dispatch(
                        Uuid::from_u128(seed + 6),
                        session.id(),
                        queued.id(),
                    )
                    .map_err(ExecutionStoreError::from)?,
                    vec![],
                    vec![],
                    vec![candidate],
                    vec![],
                    None,
                    baseline.stored_run().run().clone(),
                )
                .with_policy_guard(conformance_dispatch_guard(
                    owner_id,
                    &session,
                    &candidate_invocation,
                )?),
            )
            .await;
        let after = conformance_run_snapshot(&store, owner_id, queued.id()).await?;
        if !matches!(
            rejected,
            Err(error) if error.code() == ExecutionStoreErrorCode::LineageConflict
        ) || before != after
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum InvalidRetryAttemptCase {
    SameAttempt,
    SkippedAttempt,
    DifferentInvocation,
    DifferentManifest,
    UnexpectedAttempt,
    Overflow,
}

async fn assert_retry_attempt_adjacency_rejections<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    for (offset, case) in [
        InvalidRetryAttemptCase::SameAttempt,
        InvalidRetryAttemptCase::SkippedAttempt,
        InvalidRetryAttemptCase::DifferentInvocation,
        InvalidRetryAttemptCase::DifferentManifest,
        InvalidRetryAttemptCase::UnexpectedAttempt,
        InvalidRetryAttemptCase::Overflow,
    ]
    .into_iter()
    .enumerate()
    {
        let seed = 0xd1_80 + (offset as u128 * 0x10);
        let (store, owner_id, session, queued, started, lease) =
            create_running_run(factory, seed).await?;
        let invocation = conformance_value(LogicalInvocation::new(
            queued.id(),
            "retry-adjacency",
            "workspace.write",
            1,
            serde_json::json!({"path": "adjacent.txt"}),
        ))?;
        let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
        let first_number = 1;
        let first_dispatching = conformance_value(InvocationAttemptRecord::new_durable(
            &invocation,
            first_number,
            super::AttemptRecordState::Dispatching,
            manifest.clone(),
            RecoveryMode::KeyedIdempotent,
        ))?;
        let first = store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    started.stored_run().run_version(),
                    started.checkpoint_version(),
                    lease.clone(),
                    RuntimeCommand::prepare_dispatch(
                        Uuid::from_u128(seed + 5),
                        session.id(),
                        queued.id(),
                    )
                    .map_err(ExecutionStoreError::from)?,
                    vec![],
                    vec![],
                    vec![first_dispatching.clone()],
                    vec![],
                    None,
                    started.stored_run().run().clone(),
                )
                .with_policy_guard(conformance_dispatch_guard(
                    owner_id,
                    &session,
                    &invocation,
                )?),
            )
            .await?;
        let first_uncertain = conformance_value(InvocationAttemptRecord::new_durable(
            &invocation,
            first_number,
            super::AttemptRecordState::Uncertain,
            manifest.clone(),
            RecoveryMode::KeyedIdempotent,
        ))?;
        let retry_invocation = match case {
            InvalidRetryAttemptCase::DifferentInvocation => {
                conformance_value(LogicalInvocation::new(
                    queued.id(),
                    "retry-adjacency",
                    "workspace.write",
                    1,
                    serde_json::json!({"path": "different.txt"}),
                ))?
            }
            InvalidRetryAttemptCase::DifferentManifest => {
                let alternate = conformance_manifest(ConformanceManifestKind::KeyedAlternate);
                conformance_value(LogicalInvocation::new(
                    queued.id(),
                    "retry-adjacency",
                    alternate.id.clone(),
                    alternate.version,
                    serde_json::json!({"path": "adjacent.txt"}),
                ))?
            }
            _ => invocation.clone(),
        };
        let retry_manifest = if matches!(case, InvalidRetryAttemptCase::DifferentManifest) {
            conformance_manifest_pin(ConformanceManifestKind::KeyedAlternate)?
        } else {
            manifest.clone()
        };
        let retry_number = match case {
            InvalidRetryAttemptCase::SameAttempt => first_number,
            InvalidRetryAttemptCase::Overflow => u32::MAX,
            InvalidRetryAttemptCase::SkippedAttempt => first_number + 2,
            InvalidRetryAttemptCase::DifferentInvocation
            | InvalidRetryAttemptCase::DifferentManifest
            | InvalidRetryAttemptCase::UnexpectedAttempt => first_number + 1,
        };
        let retry_dispatching = conformance_value(InvocationAttemptRecord::new_durable(
            &retry_invocation,
            retry_number,
            super::AttemptRecordState::Dispatching,
            retry_manifest,
            RecoveryMode::KeyedIdempotent,
        ))?;
        if matches!(case, InvalidRetryAttemptCase::SkippedAttempt) {
            let plain_skipped = store
                .commit_execution(
                    owner_id,
                    ExecutionCommit::new(
                        first.stored_run().run_version(),
                        first.checkpoint_version(),
                        lease.clone(),
                        RuntimeCommand::prepare_dispatch(
                            Uuid::from_u128(seed + 7),
                            session.id(),
                            queued.id(),
                        )
                        .map_err(ExecutionStoreError::from)?,
                        vec![],
                        vec![],
                        vec![retry_dispatching.clone()],
                        vec![],
                        None,
                        first.stored_run().run().clone(),
                    )
                    .with_policy_guard(conformance_dispatch_guard(
                        owner_id,
                        &session,
                        &retry_invocation,
                    )?),
                )
                .await;
            if !matches!(
                plain_skipped,
                Err(error) if error.code() == ExecutionStoreErrorCode::LineageConflict
            ) || store
                .load_attempts_page(
                    owner_id,
                    queued.id(),
                    StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
                )
                .await?
                .items()
                != [first_dispatching.clone()]
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
        }
        let pause = conformance_value(super::RecoveryPauseRecord::new(
            invocation.binding(),
            first_number,
            manifest.clone(),
            super::RecoveryPauseReason::AuthoritativeAbsence,
        ))?;
        let recovery = conformance_value(super::RecoveryRecord::new_with_pause(pause, None))?;
        let mut recovery_attempts = vec![first_uncertain, retry_dispatching];
        if matches!(case, InvalidRetryAttemptCase::UnexpectedAttempt) {
            let unexpected_invocation = conformance_value(LogicalInvocation::new(
                queued.id(),
                "unexpected-retry-attempt",
                "workspace.write",
                1,
                serde_json::json!({"path": "unexpected.txt"}),
            ))?;
            recovery_attempts.push(conformance_value(InvocationAttemptRecord::new_durable(
                &unexpected_invocation,
                1,
                super::AttemptRecordState::Pending,
                manifest.clone(),
                RecoveryMode::KeyedIdempotent,
            ))?);
        }
        let rejected = store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    first.stored_run().run_version(),
                    first.checkpoint_version(),
                    lease,
                    RuntimeCommand::prepare_recovery_dispatch(
                        Uuid::from_u128(seed + 6),
                        session.id(),
                        queued.id(),
                        recovery,
                    )
                    .map_err(ExecutionStoreError::from)?,
                    vec![],
                    vec![],
                    recovery_attempts,
                    vec![],
                    None,
                    first.stored_run().run().clone(),
                )
                .with_policy_guard(conformance_dispatch_guard(
                    owner_id,
                    &session,
                    &retry_invocation,
                )?),
            )
            .await;
        if !matches!(
            rejected,
            Err(error) if error.code() == ExecutionStoreErrorCode::LineageConflict
        ) || store
            .load_attempts_page(
                owner_id,
                queued.id(),
                StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
            )
            .await?
            .items()
            != [first_dispatching]
            || store.load_run(owner_id, queued.id()).await?.as_ref() != Some(first.stored_run())
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
    }
    Ok(())
}

async fn assert_auto_allow_dispatch_grant_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let (store, owner_id, session, queued, started, lease) =
        create_running_run(factory, 0xd2_00).await?;
    let manifest = conformance_capability_manifest();
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "auto-allow-dispatch",
        manifest.id.clone(),
        manifest.version,
        serde_json::json!({"path": "contract.txt"}),
    ))?;
    let context = conformance_value(PolicyContext::new(
        owner_id.to_string(),
        "contract-actor",
        session.definition().id(),
        session.definition().version(),
        "contract-workspace",
        CapabilityReferenceId::new(queued.id()),
        &manifest,
        &invocation,
        1,
        PolicyRestrictions::default(),
        1_000,
    ))?;
    let grant = conformance_value(AutonomyGrant::new(
        "dispatch-auto-allow",
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
        RiskLevel::Critical,
        500,
        Some(2_000_000),
        Some(1),
    ))?;
    let evaluation = conformance_value(PolicyEngine::evaluate(
        &context,
        std::slice::from_ref(&grant),
    ))?;
    let mutation = DispatchGrantMutation::from_current_policy(
        owner_id,
        &context,
        std::slice::from_ref(&grant),
        &evaluation,
    )?
    .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
    let policy_guard = DispatchPolicyGuard::from_current_policy(
        owner_id,
        &context,
        std::slice::from_ref(&grant),
        None,
        &evaluation,
    )?;
    let authority = AuthoritativeGrantState::from_grant(owner_id, &grant)?;
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::create(authority.clone()),
        )
        .await?;
    let attempt = conformance_value(InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        super::AttemptRecordState::Dispatching,
        conformance_value(ManifestPin::from_manifest(&manifest))?,
        manifest.recovery_mode,
    ))?;
    let event = RuntimeEvent::new(
        Uuid::from_u128(0xd2_10),
        owner_id,
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::InvocationDispatchPrepared,
    )
    .map_err(ExecutionStoreError::from)?;
    let commit = ExecutionCommit::new(
        started.stored_run().run_version(),
        started.checkpoint_version(),
        lease,
        RuntimeCommand::prepare_dispatch(Uuid::from_u128(0xd2_11), session.id(), queued.id())
            .map_err(ExecutionStoreError::from)?,
        vec![event],
        vec![conformance_value(Step::new(
            queued.id(),
            "auto-allow-dispatch",
            StepKind::Capability,
        ))?],
        vec![attempt],
        vec![],
        None,
        started.stored_run().run().clone(),
    )
    .with_policy_guard(policy_guard)
    .with_dispatch_grant(mutation);
    let outcome = store.commit_execution(owner_id, commit.clone()).await?;
    if outcome.grant_consumption() != evaluation.consumption.as_ref()
        || store.commit_execution(owner_id, commit).await? != outcome
        || store
            .load_authoritative_grant(owner_id, authority.authority_key())
            .await?
            .and_then(|state| state.remaining_uses())
            != Some(0)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_policy_denied_transition_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(0xd3_01);
    let session = conformance_value(Session::new_for_definition(
        Uuid::from_u128(0xd3_02),
        &conformance_definition(false),
        SessionConcurrencyPolicy::Serial,
    ))?;
    let run_id = Uuid::from_u128(0xd3_03);
    let (_, context) = conformance_policy_context(run_id)?;
    let request = conformance_value(PolicyEngine::approval_request(&context, None))?;
    let queued = conformance_value(Run::queued(
        run_id,
        session.id(),
        session.definition().id(),
        session.definition().version(),
    ))?;
    let running = conformance_value(queued.transition(RunState::Running, None))?;
    let waiting = conformance_value(running.wait_for_approval(request))?;
    let (created, lease) = create_waiting_run(
        &store,
        WaitingRunSetup {
            owner_id,
            session: &session,
            waiting: &waiting,
            expected_session_version: 0,
            concurrency_policy: SessionConcurrencyPolicy::Serial,
            start_command_id: Uuid::from_u128(0xd3_04),
            approval_command_id: Uuid::from_u128(0xd3_05),
        },
    )
    .await?;
    let denied = conformance_value(
        waiting.transition(RunState::Paused, Some(super::RunPauseReason::PolicyDenied)),
    )?;
    let event = RuntimeEvent::new(
        Uuid::from_u128(0xd3_06),
        owner_id,
        session.id(),
        run_id,
        1,
        1,
        RuntimeEventKind::CapabilityDenied,
    )
    .map_err(ExecutionStoreError::from)?;
    let outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease,
                RuntimeCommand::pause(
                    Uuid::from_u128(0xd3_07),
                    session.id(),
                    run_id,
                    super::RunPauseReason::PolicyDenied,
                )
                .map_err(ExecutionStoreError::from)?,
                vec![event],
                vec![],
                vec![],
                vec![],
                None,
                denied.clone(),
            ),
        )
        .await?;
    if outcome.stored_run().run() != &denied
        || outcome.stored_run().run().pending_approval().is_some()
        || outcome.stored_run().run().pause_reason() != Some(super::RunPauseReason::PolicyDenied)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
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
    store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::create(AuthoritativePolicyState::active(
                owner_id,
                session.definition().id(),
                session.definition().version(),
                1,
                None,
            )?),
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
    let manifest_kind = match recovery_mode {
        RecoveryMode::Manual => ConformanceManifestKind::Manual,
        RecoveryMode::NonRetryable => ConformanceManifestKind::NonRetryable,
        RecoveryMode::Compensate => ConformanceManifestKind::Compensate,
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    };
    let manifest_identity = conformance_manifest(manifest_kind);
    let invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "blocked-recovery",
        manifest_identity.id.clone(),
        manifest_identity.version,
        serde_json::json!({"path": "blocked.txt"}),
    ))?;
    let manifest = conformance_manifest_pin(manifest_kind)?;
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
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
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
    let (_, context) = conformance_policy_context(queued.id())?;
    let grant = conformance_counted_grant(&context)?;
    let request = conformance_value(PolicyEngine::approval_request(&context, Some(&grant)))?;
    let waiting = conformance_value(running.wait_for_approval(request))?;
    let paused = conformance_value(
        running.transition(RunState::Paused, Some(super::RunPauseReason::Requested)),
    )?;
    let manual_manifest = conformance_manifest(ConformanceManifestKind::Manual);
    let manual_invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "portable-create-run-manual-review",
        manual_manifest.id.clone(),
        manual_manifest.version,
        serde_json::json!({"path": "manual-review.txt"}),
    ))?;
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Manual)?;
    let recovery_pause = conformance_value(super::RecoveryPauseRecord::new(
        manual_invocation.binding(),
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
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
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
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
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
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
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
                lease.clone(),
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
    let persisted_checkpoint = CheckpointV1Builder::new(
        session.id(),
        queued.id(),
        DefinitionPin::new(1, "portable-checkpoint", 1).map_err(ExecutionStoreError::from)?,
        1,
        vec![],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .build()
    .map_err(ExecutionStoreError::from)?;
    let installed = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                baseline.stored_run().run_version(),
                baseline.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::record_progress(Uuid::from_u128(53_007), session.id(), queued.id())
                    .map_err(ExecutionStoreError::from)?,
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                baseline.stored_run().run().clone(),
            )
            .with_checkpoint(persisted_checkpoint.clone()),
        )
        .await?;
    if installed.checkpoint_version() != 1
        || installed.checkpoint() != Some(&persisted_checkpoint)
        || store.load_checkpoint(owner_id, queued.id()).await? != Some((1, persisted_checkpoint))
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let clear = ExecutionCommit::new(
        installed.stored_run().run_version(),
        installed.checkpoint_version(),
        lease,
        RuntimeCommand::record_progress(Uuid::from_u128(53_008), session.id(), queued.id())
            .map_err(ExecutionStoreError::from)?,
        vec![],
        vec![],
        vec![],
        vec![],
        None,
        installed.stored_run().run().clone(),
    )
    .with_checkpoint_mutation(CheckpointMutation::Clear);
    let cleared = store.commit_execution(owner_id, clear.clone()).await?;
    if cleared.checkpoint_version() != 2
        || cleared.checkpoint().is_some()
        || store
            .load_checkpoint(owner_id, queued.id())
            .await?
            .is_some()
        || store.commit_execution(owner_id, clear).await? != cleared
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
            WaitingRunSetup {
                owner_id,
                session: &session,
                waiting: &waiting,
                expected_session_version: 0,
                concurrency_policy: SessionConcurrencyPolicy::Serial,
                start_command_id: Uuid::from_u128(0xb0_70 + mutation * 2),
                approval_command_id: Uuid::from_u128(0xb0_71 + mutation * 2),
            },
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

struct WaitingRunSetup<'a> {
    owner_id: Uuid,
    session: &'a Session,
    waiting: &'a Run,
    expected_session_version: u64,
    concurrency_policy: SessionConcurrencyPolicy,
    start_command_id: Uuid,
    approval_command_id: Uuid,
}

async fn create_waiting_run<S: ExecutionStore>(
    store: &S,
    setup: WaitingRunSetup<'_>,
) -> Result<(StoredRun, ExecutionLease), ExecutionStoreError> {
    let WaitingRunSetup {
        owner_id,
        session,
        waiting,
        expected_session_version,
        concurrency_policy,
        start_command_id,
        approval_command_id,
    } = setup;
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
    let resume_lease = store
        .acquire_lease(
            owner_id,
            queued.id(),
            waiting_outcome.stored_run().run_version(),
            1_000,
        )
        .await?;
    Ok((waiting_outcome.stored_run().clone(), resume_lease))
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
        WaitingRunSetup {
            owner_id,
            session: &session,
            waiting: &waiting,
            expected_session_version: 0,
            concurrency_policy: SessionConcurrencyPolicy::Serial,
            start_command_id: Uuid::from_u128(0xa9_15),
            approval_command_id: Uuid::from_u128(0x00a9_0016),
        },
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
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
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
        manifest.schema_digest(),
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
        vec![manifest.clone()],
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
        manifest.schema_digest(),
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

/// The complete immutable manifest inventory exercised by the portable execution-store suite.
///
/// Store adapters that seal durable snapshots must configure this exact inventory before running
/// [`assert_execution_store_conformance`]. It intentionally uses distinct stable identities for
/// recovery modes so a protected adapter never has to accept a pin assembled from test-only
/// digest literals.
pub fn execution_store_conformance_manifest_inventory() -> Vec<CapabilityManifest> {
    [
        ConformanceManifestKind::Keyed,
        ConformanceManifestKind::KeyedAlternate,
        ConformanceManifestKind::Retry,
        ConformanceManifestKind::Manual,
        ConformanceManifestKind::NonRetryable,
        ConformanceManifestKind::Compensate,
    ]
    .into_iter()
    .map(conformance_manifest)
    .collect()
}

fn conformance_manifest(kind: ConformanceManifestKind) -> CapabilityManifest {
    let (id, version, recovery_mode) = match kind {
        ConformanceManifestKind::Keyed => (
            CONFORMANCE_KEYED_MANIFEST_ID,
            1,
            RecoveryMode::KeyedIdempotent,
        ),
        ConformanceManifestKind::KeyedAlternate => (
            CONFORMANCE_KEYED_ALTERNATE_MANIFEST_ID,
            1,
            RecoveryMode::KeyedIdempotent,
        ),
        ConformanceManifestKind::Retry => (CONFORMANCE_RETRY_MANIFEST_ID, 1, RecoveryMode::Retry),
        ConformanceManifestKind::Manual => {
            (CONFORMANCE_MANUAL_MANIFEST_ID, 1, RecoveryMode::Manual)
        }
        ConformanceManifestKind::NonRetryable => (
            CONFORMANCE_NON_RETRYABLE_MANIFEST_ID,
            1,
            RecoveryMode::NonRetryable,
        ),
        ConformanceManifestKind::Compensate => (
            CONFORMANCE_COMPENSATE_MANIFEST_ID,
            1,
            RecoveryMode::Compensate,
        ),
    };
    CapabilityManifest::new(crate::CapabilityManifestInput {
        id: id.into(),
        version,
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
    .expect("execution-store conformance manifest must be valid")
}

fn conformance_capability_manifest() -> CapabilityManifest {
    conformance_manifest(ConformanceManifestKind::NonRetryable)
}

fn conformance_manifest_pin(
    kind: ConformanceManifestKind,
) -> Result<ManifestPin, ExecutionStoreError> {
    conformance_value(ManifestPin::from_manifest(&conformance_manifest(kind)))
}

fn conformance_dispatch_guard(
    owner_id: Uuid,
    session: &Session,
    invocation: &LogicalInvocation,
) -> Result<DispatchPolicyGuard, ExecutionStoreError> {
    let manifest = match (invocation.capability_id(), invocation.manifest_version()) {
        (CONFORMANCE_KEYED_MANIFEST_ID, 1) => conformance_manifest(ConformanceManifestKind::Keyed),
        (CONFORMANCE_KEYED_ALTERNATE_MANIFEST_ID, 1) => {
            conformance_manifest(ConformanceManifestKind::KeyedAlternate)
        }
        (CONFORMANCE_RETRY_MANIFEST_ID, 1) => conformance_manifest(ConformanceManifestKind::Retry),
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    };
    let context = conformance_value(PolicyContext::new(
        owner_id.to_string(),
        "contract-actor",
        session.definition().id(),
        session.definition().version(),
        "contract-workspace",
        CapabilityReferenceId::new(invocation.run_id()),
        &manifest,
        invocation,
        1,
        PolicyRestrictions::default(),
        1_000,
    ))?;
    let grant = conformance_value(AutonomyGrant::new(
        format!("dispatch-guard-{}", invocation.logical_step_id()),
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
        RiskLevel::Critical,
        0,
        None,
        None,
    ))?;
    let evaluation = conformance_value(PolicyEngine::evaluate(
        &context,
        std::slice::from_ref(&grant),
    ))?;
    DispatchPolicyGuard::from_current_policy(
        owner_id,
        &context,
        std::slice::from_ref(&grant),
        None,
        &evaluation,
    )
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
    conformance_counted_grant_with_uses(context, 1)
}

fn conformance_counted_grant_with_uses(
    context: &PolicyContext,
    remaining_uses: u32,
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
        Some(2_000_000),
        Some(remaining_uses),
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
        WaitingRunSetup {
            owner_id,
            session: &session,
            waiting: &first_waiting,
            expected_session_version: 0,
            concurrency_policy: SessionConcurrencyPolicy::Concurrent,
            start_command_id: Uuid::from_u128(0xc0_28),
            approval_command_id: Uuid::from_u128(0xc0_29),
        },
    )
    .await?;
    let (second_created, second_lease) = create_waiting_run(
        &store,
        WaitingRunSetup {
            owner_id,
            session: &session,
            waiting: &second_waiting,
            expected_session_version: first_created.session_version(),
            concurrency_policy: SessionConcurrencyPolicy::Concurrent,
            start_command_id: Uuid::from_u128(0xc0_2a),
            approval_command_id: Uuid::from_u128(0xc0_2b),
        },
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

async fn prepare_counted_grant_commit<S: ExecutionStore>(
    store: &S,
    owner_id: Uuid,
    session: &Session,
    expected_session_version: u64,
    seed: u128,
    grant: &AutonomyGrant,
) -> Result<(ExecutionCommit, Run, u64), ExecutionStoreError> {
    let run_id = Uuid::from_u128(seed);
    let (invocation, context) = conformance_policy_context(run_id)?;
    let (waiting, target, command, approval) = conformance_approval_resume(
        session,
        run_id,
        Uuid::from_u128(seed + 1),
        &invocation,
        &context,
        grant,
    )?;
    let (created, lease) = create_waiting_run(
        store,
        WaitingRunSetup {
            owner_id,
            session,
            waiting: &waiting,
            expected_session_version,
            concurrency_policy: SessionConcurrencyPolicy::Concurrent,
            start_command_id: Uuid::from_u128(seed + 2),
            approval_command_id: Uuid::from_u128(seed + 3),
        },
    )
    .await?;
    let event = RuntimeEvent::new(
        Uuid::from_u128(seed + 4),
        owner_id,
        session.id(),
        run_id,
        1,
        1,
        RuntimeEventKind::RunResumed,
    )
    .map_err(ExecutionStoreError::from)?;
    Ok((
        ExecutionCommit::new(
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
        ),
        waiting,
        created.session_version(),
    ))
}

async fn assert_multi_use_counted_grant_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(0xc0_50);
    let session = conformance_value(Session::new_for_definition(
        Uuid::from_u128(0xc0_51),
        &conformance_definition(true),
        SessionConcurrencyPolicy::Concurrent,
    ))?;
    let (_, first_context) = conformance_policy_context(Uuid::from_u128(0xc0_60))?;
    let grant_with_three = conformance_counted_grant_with_uses(&first_context, 3)?;
    let authority = AuthoritativeGrantState::from_grant(owner_id, &grant_with_three)?;
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::create(authority.clone()),
        )
        .await?;

    let (first, _, first_session_version) =
        prepare_counted_grant_commit(&store, owner_id, &session, 0, 0xc0_60, &grant_with_three)
            .await?;
    let (stale, stale_waiting, stale_session_version) = prepare_counted_grant_commit(
        &store,
        owner_id,
        &session,
        first_session_version,
        0xc0_70,
        &grant_with_three,
    )
    .await?;
    let first_outcome = store.commit_execution(owner_id, first.clone()).await?;
    if first_outcome.grant_consumption().is_none()
        || store
            .load_authoritative_grant(owner_id, authority.authority_key())
            .await?
            .and_then(|state| state.remaining_uses())
            != Some(2)
        || !matches!(
            store.commit_execution(owner_id, stale).await,
            Err(error) if error.code() == ExecutionStoreErrorCode::GrantConflict
        )
        || store
            .load_run(owner_id, stale_waiting.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&stale_waiting)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }

    let mut grant_with_two = grant_with_three.clone();
    grant_with_two.remaining_uses = Some(2);
    let (second, _, second_session_version) = prepare_counted_grant_commit(
        &store,
        owner_id,
        &session,
        stale_session_version,
        0xc0_80,
        &grant_with_two,
    )
    .await?;
    let second_outcome = store.commit_execution(owner_id, second.clone()).await?;
    if second_outcome.grant_consumption().is_none()
        || store
            .load_authoritative_grant(owner_id, authority.authority_key())
            .await?
            .and_then(|state| state.remaining_uses())
            != Some(1)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }

    let mut grant_with_one = grant_with_three.clone();
    grant_with_one.remaining_uses = Some(1);
    let (third, _, third_session_version) = prepare_counted_grant_commit(
        &store,
        owner_id,
        &session,
        second_session_version,
        0xc0_90,
        &grant_with_one,
    )
    .await?;
    let (fourth, fourth_waiting, _) = prepare_counted_grant_commit(
        &store,
        owner_id,
        &session,
        third_session_version,
        0xc0_a0,
        &grant_with_one,
    )
    .await?;
    let consumption_ids = [
        first
            .approval()
            .and_then(ApprovalGrantMutation::grant_consumption)
            .map(|consumption| consumption.logical_invocation_id),
        second
            .approval()
            .and_then(ApprovalGrantMutation::grant_consumption)
            .map(|consumption| consumption.logical_invocation_id),
        third
            .approval()
            .and_then(ApprovalGrantMutation::grant_consumption)
            .map(|consumption| consumption.logical_invocation_id),
    ];
    if consumption_ids.iter().any(Option::is_none)
        || consumption_ids[0] == consumption_ids[1]
        || consumption_ids[0] == consumption_ids[2]
        || consumption_ids[1] == consumption_ids[2]
        || first.command().id() == second.command().id()
        || first.command().id() == third.command().id()
        || second.command().id() == third.command().id()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    let third_outcome = store.commit_execution(owner_id, third).await?;
    if third_outcome.grant_consumption().is_none()
        || store
            .load_authoritative_grant(owner_id, authority.authority_key())
            .await?
            .and_then(|state| state.remaining_uses())
            != Some(0)
        || !matches!(
            store.commit_execution(owner_id, fourth).await,
            Err(error) if error.code() == ExecutionStoreErrorCode::GrantConflict
        )
        || store
            .load_run(owner_id, fourth_waiting.id())
            .await?
            .as_ref()
            .map(StoredRun::run)
            != Some(&fourth_waiting)
        || store.commit_execution(owner_id, first).await? != first_outcome
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

async fn assert_counted_grant_revoke_race_contract<F>(
    factory: &F,
) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    let store = factory.create_execution_store().await?;
    let owner_id = Uuid::from_u128(0xc0_37);
    let session = conformance_value(Session::new_for_definition(
        Uuid::from_u128(0xc0_30),
        &conformance_definition(true),
        SessionConcurrencyPolicy::Concurrent,
    ))?;
    let run_id = Uuid::from_u128(0xc0_31);
    let (invocation, context) = conformance_policy_context(run_id)?;
    let grant = conformance_counted_grant(&context)?;
    let authority = AuthoritativeGrantState::from_grant(owner_id, &grant)?;
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::create(authority.clone()),
        )
        .await?;
    let (waiting, target, command, approval) = conformance_approval_resume(
        &session,
        run_id,
        Uuid::from_u128(0xc032),
        &invocation,
        &context,
        &grant,
    )?;
    let (created, lease) = create_waiting_run(
        &store,
        WaitingRunSetup {
            owner_id,
            session: &session,
            waiting: &waiting,
            expected_session_version: 0,
            concurrency_policy: SessionConcurrencyPolicy::Concurrent,
            start_command_id: Uuid::from_u128(0xc0_33),
            approval_command_id: Uuid::from_u128(0xc0_34),
        },
    )
    .await?;
    let event = RuntimeEvent::new(
        Uuid::from_u128(0xc0_35),
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
        target.clone(),
    );
    let revoke = AuthoritativeGrantChange::revoke(authority.authority_key().clone(), 1)?;
    let (commit_result, revoke_result) = futures::join!(
        store.commit_execution(owner_id, commit),
        store.apply_authoritative_grant(owner_id, revoke)
    );
    let revoked = revoke_result?;
    if revoked.status() != GrantStatus::Revoked {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
    match commit_result {
        Ok(outcome)
            if outcome.stored_run().run() == &target && outcome.grant_consumption().is_some() => {}
        Err(error) if error.code() == ExecutionStoreErrorCode::GrantConflict => {
            if store
                .load_run(owner_id, run_id)
                .await?
                .as_ref()
                .map(StoredRun::run)
                != Some(&waiting)
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
        }
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ))
        }
    }
    Ok(())
}

async fn assert_durable_result_contract<F>(factory: &F) -> Result<(), ExecutionStoreError>
where
    F: ExecutionStoreFactory,
{
    if ManifestPin::new("malformed", 1, "sha256:abc").is_ok() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }
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
    let manifest = conformance_manifest_pin(ConformanceManifestKind::Keyed)?;
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
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let alternate_manifest = conformance_manifest(ConformanceManifestKind::KeyedAlternate);
    let cross_manifest_invocation = conformance_value(LogicalInvocation::new(
        queued.id(),
        "durable-result-step",
        alternate_manifest.id.clone(),
        alternate_manifest.version,
        serde_json::json!({"path": "contract.txt"}),
    ))?;
    let cross_manifest = conformance_manifest_pin(ConformanceManifestKind::KeyedAlternate)?;
    let cross_manifest_completed = conformance_value(CompletedInvocationRecord::new(
        cross_manifest_invocation.binding(),
        1,
        cross_manifest,
        RecoveryMode::KeyedIdempotent,
        conformance_value(OpaqueReference::new(result_reference.handle()))?,
    ))?;
    let result = conformance_value(DurableCapabilityResult::new(
        result_reference.clone(),
        format!("jcs-v1:{}", "d".repeat(64)),
        manifest.schema_digest(),
        1,
        DurableCapabilityStatus::Completed,
    ))?;
    let mismatched_result = conformance_value(DurableCapabilityResult::new(
        result_reference.clone(),
        format!("jcs-v1:{}", "4".repeat(64)),
        format!("sha256:{}", "4".repeat(64)),
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
    for (command_id, completed_without_lineage, attempts, candidate_result) in [
        (
            Uuid::from_u128(0xd0_21),
            completed.clone(),
            vec![],
            result.clone(),
        ),
        (
            Uuid::from_u128(0xd0_22),
            cross_run_completed,
            vec![attempt.clone()],
            result.clone(),
        ),
        (
            Uuid::from_u128(0xd0_23),
            cross_manifest_completed,
            vec![attempt.clone()],
            mismatched_result.clone(),
        ),
        (
            Uuid::from_u128(0xd0_24),
            completed.clone(),
            vec![attempt.clone()],
            mismatched_result.clone(),
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
                        candidate_result,
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
            || !store
                .load_attempts_page(
                    owner_id,
                    queued.id(),
                    StoreReadPage::first(MAX_STORE_READ_PAGE_SIZE)?,
                )
                .await?
                .items()
                .is_empty()
            || store
                .load_durable_result(owner_id, queued.id(), invocation.id())
                .await?
                .is_some()
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
        manifest.schema_digest(),
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
