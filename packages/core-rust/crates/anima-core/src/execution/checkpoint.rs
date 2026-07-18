use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::state::{
    valid_id, valid_uuid, valid_version, Budget, ExecutionError, ExecutionErrorCode,
    RunPauseReason, RunState, Usage,
};
use crate::{
    AgentDefinition, ApprovalDecision, ApprovalDecisionKind, ApprovalRequest, CapabilityManifest,
    LogicalInvocationBinding, ManifestCatalog, RecoveryMode, RecoveryResumeBinding,
    SUPPORTED_DEFINITION_SCHEMA_VERSION,
};

pub const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
pub const RUNTIME_SCHEMA_VERSION: u32 = 1;
const MAX_DIGEST_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionPin {
    schema_version: u32,
    id: String,
    version: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefinitionPinWire {
    schema_version: u32,
    id: String,
    version: u32,
}
impl DefinitionPin {
    pub fn new(
        schema_version: u32,
        id: impl Into<String>,
        version: u32,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            schema_version,
            id: id.into(),
            version,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn from_definition(value: &AgentDefinition) -> Result<Self, ExecutionError> {
        Self::new(value.schema_version, value.id.clone(), value.version)
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        if self.schema_version != SUPPORTED_DEFINITION_SCHEMA_VERSION {
            return Err(ExecutionError::new(
                ExecutionErrorCode::IncompatibleCheckpoint,
            ));
        }
        valid_id(&self.id)?;
        valid_version(self.version)
    }
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn version(&self) -> u32 {
        self.version
    }
}
impl<'de> Deserialize<'de> for DefinitionPin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = DefinitionPinWire::deserialize(d)?;
        Self::new(w.schema_version, w.id, w.version).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPin {
    id: String,
    version: u32,
    schema_digest: String,
    recovery_mode: RecoveryMode,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPinWire {
    id: String,
    version: u32,
    schema_digest: String,
    recovery_mode: RecoveryMode,
}
impl ManifestPin {
    pub fn new(
        id: impl Into<String>,
        version: u32,
        schema_digest: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        Self::new_with_recovery_mode(id, version, schema_digest, RecoveryMode::None)
    }
    pub fn new_with_recovery_mode(
        id: impl Into<String>,
        version: u32,
        schema_digest: impl Into<String>,
        recovery_mode: RecoveryMode,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            id: id.into(),
            version,
            schema_digest: schema_digest.into(),
            recovery_mode,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn from_manifest(value: &CapabilityManifest) -> Result<Self, ExecutionError> {
        Self::new_with_recovery_mode(
            value.id.clone(),
            value.version,
            value.schema_digest().to_owned(),
            value.recovery_mode,
        )
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        valid_id(&self.id)?;
        valid_version(self.version)?;
        if !self.schema_digest.starts_with("sha256:")
            || self.schema_digest.len() <= 7
            || self.schema_digest.len() > MAX_DIGEST_BYTES
            || self.schema_digest.chars().any(char::is_whitespace)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        Ok(())
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn schema_digest(&self) -> &str {
        &self.schema_digest
    }
    pub fn recovery_mode(&self) -> RecoveryMode {
        self.recovery_mode
    }
}
impl<'de> Deserialize<'de> for ManifestPin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = ManifestPinWire::deserialize(d)?;
        Self::new_with_recovery_mode(w.id, w.version, w.schema_digest, w.recovery_mode)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct OpaqueReference(Uuid);
impl OpaqueReference {
    pub fn new(value: Uuid) -> Result<Self, ExecutionError> {
        valid_uuid(value)?;
        Ok(Self(value))
    }
    pub fn value(&self) -> Uuid {
        self.0
    }
}
impl<'de> Deserialize<'de> for OpaqueReference {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(Uuid::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptRecordState {
    Pending,
    Completed,
    Uncertain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointCursor {
    logical_invocation_id: Uuid,
    attempt_number: u32,
    logical_step_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointCursorWire {
    logical_invocation_id: Uuid,
    attempt_number: u32,
    logical_step_id: String,
}
impl CheckpointCursor {
    pub fn new(
        logical_invocation_id: Uuid,
        attempt_number: u32,
        logical_step_id: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            logical_invocation_id,
            attempt_number,
            logical_step_id: logical_step_id.into(),
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        valid_uuid(self.logical_invocation_id)?;
        if self.attempt_number == 0 {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        valid_id(&self.logical_step_id)
    }
    pub fn logical_invocation_id(&self) -> Uuid {
        self.logical_invocation_id
    }
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    pub fn logical_step_id(&self) -> &str {
        &self.logical_step_id
    }
}
impl<'de> Deserialize<'de> for CheckpointCursor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = CheckpointCursorWire::deserialize(d)?;
        Self::new(w.logical_invocation_id, w.attempt_number, w.logical_step_id)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationAttemptRecord {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    state: AttemptRecordState,
    manifest: ManifestPin,
    recovery_mode: RecoveryMode,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationAttemptRecordWire {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    state: AttemptRecordState,
    manifest: ManifestPin,
    recovery_mode: RecoveryMode,
}
impl InvocationAttemptRecord {
    pub fn new(
        invocation: LogicalInvocationBinding,
        attempt_number: u32,
        state: AttemptRecordState,
        manifest: ManifestPin,
        recovery_mode: RecoveryMode,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            invocation,
            attempt_number,
            state,
            manifest,
            recovery_mode,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        if self.attempt_number == 0
            || self.invocation.capability_id() != self.manifest.id()
            || self.invocation.manifest_version() != self.manifest.version()
            || self.recovery_mode != self.manifest.recovery_mode()
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        Ok(())
    }
    pub fn invocation(&self) -> &LogicalInvocationBinding {
        &self.invocation
    }
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    pub fn state(&self) -> AttemptRecordState {
        self.state
    }
    pub fn manifest(&self) -> &ManifestPin {
        &self.manifest
    }
    pub fn recovery_mode(&self) -> RecoveryMode {
        self.recovery_mode
    }
}
impl<'de> Deserialize<'de> for InvocationAttemptRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = InvocationAttemptRecordWire::deserialize(d)?;
        Self::new(
            w.invocation,
            w.attempt_number,
            w.state,
            w.manifest,
            w.recovery_mode,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedInvocationRecord {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    manifest: ManifestPin,
    recovery_mode: RecoveryMode,
    result_ref: OpaqueReference,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletedInvocationRecordWire {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    manifest: ManifestPin,
    recovery_mode: RecoveryMode,
    result_ref: OpaqueReference,
}
impl CompletedInvocationRecord {
    pub fn new(
        invocation: LogicalInvocationBinding,
        attempt_number: u32,
        manifest: ManifestPin,
        recovery_mode: RecoveryMode,
        result_ref: OpaqueReference,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            invocation,
            attempt_number,
            manifest,
            recovery_mode,
            result_ref,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        InvocationAttemptRecord::new(
            self.invocation.clone(),
            self.attempt_number,
            AttemptRecordState::Completed,
            self.manifest.clone(),
            self.recovery_mode,
        )
        .map(|_| ())
    }
    pub fn invocation(&self) -> &LogicalInvocationBinding {
        &self.invocation
    }
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    pub fn result_ref(&self) -> &OpaqueReference {
        &self.result_ref
    }
    pub fn manifest(&self) -> &ManifestPin {
        &self.manifest
    }
    pub fn recovery_mode(&self) -> RecoveryMode {
        self.recovery_mode
    }
}
impl<'de> Deserialize<'de> for CompletedInvocationRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = CompletedInvocationRecordWire::deserialize(d)?;
        Self::new(
            w.invocation,
            w.attempt_number,
            w.manifest,
            w.recovery_mode,
            w.result_ref,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPauseReason {
    Retryable,
    AuthoritativeAbsence,
    UncertainOutcome,
    ReconciliationPending,
    ManualReview,
    HostRecovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPauseRecord {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    manifest: ManifestPin,
    reason: RecoveryPauseReason,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryPauseRecordWire {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    manifest: ManifestPin,
    reason: RecoveryPauseReason,
}
impl RecoveryPauseRecord {
    pub fn new(
        invocation: LogicalInvocationBinding,
        attempt_number: u32,
        manifest: ManifestPin,
        reason: RecoveryPauseReason,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            invocation,
            attempt_number,
            manifest,
            reason,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        InvocationAttemptRecord::new(
            self.invocation.clone(),
            self.attempt_number,
            AttemptRecordState::Uncertain,
            self.manifest.clone(),
            self.manifest.recovery_mode(),
        )
        .map(|_| ())
    }
    pub fn invocation(&self) -> &LogicalInvocationBinding {
        &self.invocation
    }
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    pub fn manifest(&self) -> &ManifestPin {
        &self.manifest
    }
    pub fn reason(&self) -> RecoveryPauseReason {
        self.reason
    }
    pub fn allows_automatic_resume(&self) -> bool {
        matches!(
            self.reason,
            RecoveryPauseReason::Retryable | RecoveryPauseReason::AuthoritativeAbsence
        ) && matches!(
            self.manifest.recovery_mode(),
            RecoveryMode::InherentlyIdempotent
                | RecoveryMode::KeyedIdempotent
                | RecoveryMode::Reconcilable
                | RecoveryMode::Retry
        )
    }
    pub fn matches_resume_binding(&self, binding: &RecoveryResumeBinding) -> bool {
        self.allows_automatic_resume()
            && binding.logical_invocation_id() == self.invocation.id()
            && binding.run_id() == self.invocation.run_id()
            && binding.logical_step_id() == self.invocation.logical_step_id()
            && binding.capability_id() == self.invocation.capability_id()
            && binding.canonical_argument_digest() == self.invocation.canonical_argument_digest()
            && binding.completed_attempt_number() == self.attempt_number
            && binding.manifest_id() == self.manifest.id()
            && binding.manifest_version() == self.manifest.version()
            && binding.manifest_digest() == self.manifest.schema_digest()
            && binding.recovery_mode() == self.manifest.recovery_mode()
            && binding.idempotency_key() == self.invocation.idempotency_key()
    }
}
impl<'de> Deserialize<'de> for RecoveryPauseRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = RecoveryPauseRecordWire::deserialize(d)?;
        Self::new(w.invocation, w.attempt_number, w.manifest, w.reason)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncertainInvocationRecord {
    pause: RecoveryPauseRecord,
    resume_binding: Option<RecoveryResumeBinding>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncertainInvocationRecordWire {
    pause: RecoveryPauseRecord,
    resume_binding: Option<RecoveryResumeBinding>,
}
impl UncertainInvocationRecord {
    pub fn new(
        invocation: LogicalInvocationBinding,
        attempt_number: u32,
        manifest: ManifestPin,
        recovery_mode: RecoveryMode,
        recovery_binding: Option<RecoveryResumeBinding>,
    ) -> Result<Self, ExecutionError> {
        if recovery_mode != manifest.recovery_mode() {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        let pause = RecoveryPauseRecord::new(
            invocation,
            attempt_number,
            manifest,
            if recovery_binding.is_some() {
                RecoveryPauseReason::AuthoritativeAbsence
            } else {
                RecoveryPauseReason::UncertainOutcome
            },
        )?;
        Self::new_with_pause(pause, recovery_binding)
    }
    pub fn new_with_pause(
        pause: RecoveryPauseRecord,
        resume_binding: Option<RecoveryResumeBinding>,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            pause,
            resume_binding,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        self.pause.validate()?;
        if let Some(binding) = &self.resume_binding {
            if !self.pause.matches_resume_binding(binding) {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
        }
        Ok(())
    }
    pub fn invocation(&self) -> &LogicalInvocationBinding {
        self.pause.invocation()
    }
    pub fn attempt_number(&self) -> u32 {
        self.pause.attempt_number()
    }
    pub fn recovery_binding(&self) -> Option<&RecoveryResumeBinding> {
        self.resume_binding.as_ref()
    }
    pub fn pause(&self) -> &RecoveryPauseRecord {
        &self.pause
    }
}
impl<'de> Deserialize<'de> for UncertainInvocationRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = UncertainInvocationRecordWire::deserialize(d)?;
        Self::new_with_pause(w.pause, w.resume_binding).map_err(serde::de::Error::custom)
    }
}
pub type RecoveryRecord = UncertainInvocationRecord;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingApprovalRecord {
    request: ApprovalRequest,
    decision: Option<ApprovalDecision>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingApprovalRecordWire {
    request: ApprovalRequest,
    decision: Option<ApprovalDecision>,
}
impl PendingApprovalRecord {
    pub fn new(
        request: ApprovalRequest,
        decision: Option<ApprovalDecision>,
    ) -> Result<Self, ExecutionError> {
        let value = Self { request, decision };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        self.request
            .validate()
            .map_err(|_| ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint))?;
        if let Some(decision) = &self.decision {
            decision
                .validate()
                .map_err(|_| ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint))?;
        }
        if self
            .decision
            .as_ref()
            .is_some_and(|d| d.request != self.request || d.kind != ApprovalDecisionKind::Approve)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        Ok(())
    }
    fn validate_against(
        &self,
        definition: &DefinitionPin,
        attempt: &InvocationAttemptRecord,
    ) -> Result<(), ExecutionError> {
        self.validate()?;
        let request = &self.request;
        let invocation = attempt.invocation();
        if request.agent_definition_id != definition.id()
            || request.agent_definition_version != definition.version()
            || request.run_id != invocation.run_id()
            || request.logical_step_id != invocation.logical_step_id()
            || request.logical_invocation_id != invocation.id()
            || request.capability_id != invocation.capability_id()
            || request.manifest_version != invocation.manifest_version()
            || request.capability_id != attempt.manifest().id()
            || request.manifest_version != attempt.manifest().version()
            || request.canonical_argument_digest != invocation.canonical_argument_digest()
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        Ok(())
    }
    pub fn request(&self) -> &ApprovalRequest {
        &self.request
    }
    pub fn decision(&self) -> Option<&ApprovalDecision> {
        self.decision.as_ref()
    }
}
impl<'de> Deserialize<'de> for PendingApprovalRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = PendingApprovalRecordWire::deserialize(d)?;
        Self::new(w.request, w.decision).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CheckpointV1 {
    schema_version: u32,
    runtime_schema_version: u32,
    definition: DefinitionPin,
    manifests: Vec<ManifestPin>,
    session_id: Uuid,
    run_id: Uuid,
    last_durable_event_sequence: u64,
    state: RunState,
    pause_reason: Option<RunPauseReason>,
    cursor: Option<CheckpointCursor>,
    attempts: Vec<InvocationAttemptRecord>,
    message_context_refs: Vec<OpaqueReference>,
    model_context_refs: Vec<OpaqueReference>,
    completed_invocations: Vec<CompletedInvocationRecord>,
    uncertain_invocations: Vec<UncertainInvocationRecord>,
    pending_approval: Option<PendingApprovalRecord>,
    budget: Budget,
    usage: Usage,
    memory_refs: Vec<OpaqueReference>,
    artifact_refs: Vec<OpaqueReference>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointWire {
    schema_version: u32,
    runtime_schema_version: u32,
    definition: DefinitionPin,
    manifests: Vec<ManifestPin>,
    session_id: Uuid,
    run_id: Uuid,
    last_durable_event_sequence: u64,
    state: RunState,
    pause_reason: Option<RunPauseReason>,
    cursor: Option<CheckpointCursor>,
    attempts: Vec<InvocationAttemptRecord>,
    message_context_refs: Vec<OpaqueReference>,
    model_context_refs: Vec<OpaqueReference>,
    completed_invocations: Vec<CompletedInvocationRecord>,
    uncertain_invocations: Vec<UncertainInvocationRecord>,
    pending_approval: Option<PendingApprovalRecord>,
    budget: Budget,
    usage: Usage,
    memory_refs: Vec<OpaqueReference>,
    artifact_refs: Vec<OpaqueReference>,
}

pub struct CheckpointV1Builder {
    wire: CheckpointWire,
    legacy_cursor_step_id: Option<String>,
}
impl CheckpointV1Builder {
    pub fn new(
        session_id: Uuid,
        run_id: Uuid,
        definition: DefinitionPin,
        last_durable_event_sequence: u64,
        mut manifests: Vec<ManifestPin>,
        budget: Budget,
        usage: Usage,
    ) -> Self {
        manifests.sort();
        Self {
            wire: CheckpointWire {
                schema_version: CHECKPOINT_SCHEMA_VERSION,
                runtime_schema_version: RUNTIME_SCHEMA_VERSION,
                definition,
                manifests,
                session_id,
                run_id,
                last_durable_event_sequence,
                state: RunState::Queued,
                pause_reason: None,
                cursor: None,
                attempts: vec![],
                message_context_refs: vec![],
                model_context_refs: vec![],
                completed_invocations: vec![],
                uncertain_invocations: vec![],
                pending_approval: None,
                budget,
                usage,
                memory_refs: vec![],
                artifact_refs: vec![],
            },
            legacy_cursor_step_id: None,
        }
    }
    pub fn state(mut self, state: RunState, pause_reason: Option<RunPauseReason>) -> Self {
        self.wire.state = state;
        self.wire.pause_reason = pause_reason;
        self
    }
    pub fn cursor_step_id(mut self, value: Option<String>) -> Self {
        self.legacy_cursor_step_id = value;
        self
    }
    pub fn cursor(mut self, value: Option<CheckpointCursor>) -> Self {
        self.wire.cursor = value;
        self
    }
    pub fn attempts(mut self, mut value: Vec<InvocationAttemptRecord>) -> Self {
        value.sort_by_key(|r| (r.invocation.id(), r.attempt_number));
        self.wire.attempts = value;
        self
    }
    pub fn completed_invocations(mut self, mut value: Vec<CompletedInvocationRecord>) -> Self {
        value.sort_by_key(|r| (r.invocation.id(), r.attempt_number));
        self.wire.completed_invocations = value;
        self
    }
    pub fn uncertain_invocations(mut self, mut value: Vec<UncertainInvocationRecord>) -> Self {
        value.sort_by_key(|r| (r.invocation().id(), r.attempt_number()));
        self.wire.uncertain_invocations = value;
        self
    }
    pub fn pending_approval(mut self, value: Option<PendingApprovalRecord>) -> Self {
        self.wire.pending_approval = value;
        self
    }
    pub fn message_context_refs(mut self, mut value: Vec<OpaqueReference>) -> Self {
        value.sort();
        self.wire.message_context_refs = value;
        self
    }
    pub fn model_context_refs(mut self, mut value: Vec<OpaqueReference>) -> Self {
        value.sort();
        self.wire.model_context_refs = value;
        self
    }
    pub fn memory_refs(mut self, mut value: Vec<OpaqueReference>) -> Self {
        value.sort();
        self.wire.memory_refs = value;
        self
    }
    pub fn artifact_refs(mut self, mut value: Vec<OpaqueReference>) -> Self {
        value.sort();
        self.wire.artifact_refs = value;
        self
    }
    pub fn build(mut self) -> Result<CheckpointV1, ExecutionError> {
        if self.wire.cursor.is_none() {
            if let Some(step) = self.legacy_cursor_step_id {
                let matching: Vec<_> = self
                    .wire
                    .attempts
                    .iter()
                    .filter(|a| a.invocation.logical_step_id() == step)
                    .collect();
                if let Some(record) = matching.iter().max_by_key(|a| a.attempt_number) {
                    self.wire.cursor = Some(CheckpointCursor::new(
                        record.invocation.id(),
                        record.attempt_number,
                        step,
                    )?);
                } else {
                    return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
                }
            }
        }
        CheckpointV1::from_wire(self.wire)
    }
}

impl CheckpointV1 {
    pub fn new_minimal(
        session_id: Uuid,
        run_id: Uuid,
        definition_id: impl Into<String>,
        definition_version: u32,
        last_durable_event_sequence: u64,
        manifests: Vec<ManifestPin>,
    ) -> Result<Self, ExecutionError> {
        CheckpointV1Builder::new(
            session_id,
            run_id,
            DefinitionPin::new(
                SUPPORTED_DEFINITION_SCHEMA_VERSION,
                definition_id,
                definition_version,
            )?,
            last_durable_event_sequence,
            manifests,
            Budget::default(),
            Usage::default(),
        )
        .build()
    }
    fn from_wire(w: CheckpointWire) -> Result<Self, ExecutionError> {
        let value = Self {
            schema_version: w.schema_version,
            runtime_schema_version: w.runtime_schema_version,
            definition: w.definition,
            manifests: w.manifests,
            session_id: w.session_id,
            run_id: w.run_id,
            last_durable_event_sequence: w.last_durable_event_sequence,
            state: w.state,
            pause_reason: w.pause_reason,
            cursor: w.cursor,
            attempts: w.attempts,
            message_context_refs: w.message_context_refs,
            model_context_refs: w.model_context_refs,
            completed_invocations: w.completed_invocations,
            uncertain_invocations: w.uncertain_invocations,
            pending_approval: w.pending_approval,
            budget: w.budget,
            usage: w.usage,
            memory_refs: w.memory_refs,
            artifact_refs: w.artifact_refs,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), ExecutionError> {
        if self.schema_version != CHECKPOINT_SCHEMA_VERSION
            || self.runtime_schema_version != RUNTIME_SCHEMA_VERSION
        {
            return Err(ExecutionError::new(
                ExecutionErrorCode::IncompatibleCheckpoint,
            ));
        }
        self.definition.validate()?;
        valid_uuid(self.session_id)?;
        valid_uuid(self.run_id)?;
        if self.last_durable_event_sequence == 0 {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if (self.state == RunState::Paused) != self.pause_reason.is_some()
            || self.pause_reason == Some(RunPauseReason::RecoveryRequired)
            || (self.state == RunState::WaitingForApproval) != self.pending_approval.is_some()
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if let Some(cursor) = &self.cursor {
            cursor.validate()?;
        }
        canonical(&self.manifests)?;
        if self
            .manifests
            .windows(2)
            .any(|w| w[0].id() == w[1].id() && w[0].version() == w[1].version())
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        canonical(&self.message_context_refs)?;
        canonical(&self.model_context_refs)?;
        canonical(&self.memory_refs)?;
        canonical(&self.artifact_refs)?;
        let manifest_map: BTreeMap<_, _> = self
            .manifests
            .iter()
            .map(|m| ((m.id(), m.version()), m))
            .collect();
        let mut previous = None;
        let mut last_by_invocation = BTreeMap::new();
        for record in &self.attempts {
            record.validate()?;
            if record.invocation.run_id() != self.run_id
                || manifest_map.get(&(record.manifest.id(), record.manifest.version()))
                    != Some(&&record.manifest)
            {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
            let key = (record.invocation.id(), record.attempt_number);
            let prior = last_by_invocation.insert(record.invocation.id(), record.attempt_number);
            if previous.is_some_and(|p| p >= key)
                || prior.map_or(record.attempt_number != 1, |n| {
                    n.checked_add(1) != Some(record.attempt_number)
                })
            {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
            previous = Some(key);
        }
        canonical_by_pair(&self.completed_invocations, |r| {
            (r.invocation.id(), r.attempt_number)
        })?;
        canonical_by_pair(&self.uncertain_invocations, |r| {
            (r.invocation().id(), r.attempt_number())
        })?;
        let completed_keys: BTreeSet<_> = self
            .completed_invocations
            .iter()
            .map(|r| (r.invocation.id(), r.attempt_number))
            .collect();
        if self
            .uncertain_invocations
            .iter()
            .any(|r| completed_keys.contains(&(r.invocation().id(), r.attempt_number())))
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        for completed in &self.completed_invocations {
            completed.validate()?;
            if !self.attempts.iter().any(|a| {
                a.invocation == completed.invocation
                    && a.attempt_number == completed.attempt_number
                    && a.state == AttemptRecordState::Completed
                    && a.manifest == completed.manifest
                    && a.recovery_mode == completed.recovery_mode
            }) {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
        }
        for uncertain in &self.uncertain_invocations {
            uncertain.validate()?;
            if !self.attempts.iter().any(|a| {
                a.invocation == *uncertain.invocation()
                    && a.attempt_number == uncertain.attempt_number()
                    && a.state == AttemptRecordState::Uncertain
                    && a.manifest == *uncertain.pause().manifest()
                    && a.recovery_mode == uncertain.pause().manifest().recovery_mode()
            }) {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
        }
        for attempt in &self.attempts {
            match attempt.state {
                AttemptRecordState::Completed
                    if !self.completed_invocations.iter().any(|r| {
                        r.invocation() == &attempt.invocation
                            && r.attempt_number() == attempt.attempt_number
                    }) =>
                {
                    return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
                }
                AttemptRecordState::Uncertain
                    if !self.uncertain_invocations.iter().any(|r| {
                        r.invocation() == &attempt.invocation
                            && r.attempt_number() == attempt.attempt_number
                    }) =>
                {
                    return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
                }
                _ => {}
            }
        }
        let pending: Vec<_> = self
            .attempts
            .iter()
            .filter(|a| a.state == AttemptRecordState::Pending)
            .collect();
        if pending.len() > 1 {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.state == RunState::RecoveryRequired
            && (self.uncertain_invocations.is_empty()
                || self
                    .uncertain_invocations
                    .iter()
                    .any(|r| r.pause().invocation().run_id() != self.run_id))
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.state != RunState::RecoveryRequired
            && self
                .uncertain_invocations
                .iter()
                .any(|r| r.recovery_binding().is_some())
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        let cursor_attempt = self.cursor.as_ref().and_then(|cursor| {
            self.attempts.iter().find(|a| {
                a.invocation.id() == cursor.logical_invocation_id
                    && a.attempt_number == cursor.attempt_number
                    && a.invocation.logical_step_id() == cursor.logical_step_id
            })
        });
        if self.cursor.is_some() != cursor_attempt.is_some()
            || cursor_attempt.is_some_and(|current| {
                self.attempts.iter().any(|a| {
                    a.invocation.id() == current.invocation.id()
                        && a.attempt_number > current.attempt_number
                })
            })
            || pending.first().is_some_and(|a| cursor_attempt != Some(*a))
            || self.state == RunState::Queued
                && (!self.attempts.is_empty() || self.cursor.is_some())
            || matches!(
                self.state,
                RunState::Completed | RunState::Failed | RunState::Cancelled
            ) && self.cursor.is_some()
            || matches!(
                self.state,
                RunState::Running
                    | RunState::WaitingForApproval
                    | RunState::Paused
                    | RunState::RecoveryRequired
            ) && !self.attempts.is_empty()
                && self.cursor.is_none()
            || self.state == RunState::RecoveryRequired
                && cursor_attempt.is_none_or(|a| a.state != AttemptRecordState::Uncertain)
            || matches!(self.state, RunState::Running | RunState::WaitingForApproval)
                && cursor_attempt.is_some_and(|a| a.state != AttemptRecordState::Pending)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if let Some(approval) = &self.pending_approval {
            approval.validate_against(
                &self.definition,
                cursor_attempt
                    .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint))?,
            )?;
        }
        self.budget.validate()?;
        self.usage.validate()?;
        Ok(())
    }
    pub fn assert_compatible(
        &self,
        definition: &AgentDefinition,
        catalog: &ManifestCatalog,
    ) -> Result<(), ExecutionError> {
        self.validate()?;
        if self.definition != DefinitionPin::from_definition(definition)? {
            return Err(ExecutionError::new(
                ExecutionErrorCode::IncompatibleCheckpoint,
            ));
        }
        if let Some(approval) = &self.pending_approval {
            let request = approval.request();
            let capability = definition.resolved_capabilities.iter().find(|capability| {
                capability.capability_id == request.capability_id
                    && capability.manifest_version == request.manifest_version
            });
            if request.policy_revision != definition.approval_policy_revision
                || capability.is_none_or(|capability| {
                    capability.approval_policy_revision != request.policy_revision
                })
            {
                return Err(ExecutionError::new(
                    ExecutionErrorCode::IncompatibleCheckpoint,
                ));
            }
        }
        let mut expected = Vec::new();
        for cap in &definition.resolved_capabilities {
            let manifest = catalog
                .manifest(&cap.capability_id, cap.manifest_version)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::IncompatibleCheckpoint))?;
            let pin = ManifestPin::from_manifest(manifest)?;
            if pin.schema_digest() != cap.schema_digest {
                return Err(ExecutionError::new(
                    ExecutionErrorCode::IncompatibleCheckpoint,
                ));
            }
            expected.push(pin);
        }
        expected.sort();
        if self.manifests != expected {
            return Err(ExecutionError::new(
                ExecutionErrorCode::IncompatibleCheckpoint,
            ));
        }
        Ok(())
    }
    pub fn definition(&self) -> &DefinitionPin {
        &self.definition
    }
    pub fn manifests(&self) -> &[ManifestPin] {
        &self.manifests
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn last_durable_event_sequence(&self) -> u64 {
        self.last_durable_event_sequence
    }
    pub fn state(&self) -> RunState {
        self.state
    }
    pub fn pause_reason(&self) -> Option<RunPauseReason> {
        self.pause_reason
    }
    pub fn cursor_step_id(&self) -> Option<&str> {
        self.cursor.as_ref().map(CheckpointCursor::logical_step_id)
    }
    pub fn cursor(&self) -> Option<&CheckpointCursor> {
        self.cursor.as_ref()
    }
    pub fn attempts(&self) -> &[InvocationAttemptRecord] {
        &self.attempts
    }
    pub fn completed_invocations(&self) -> &[CompletedInvocationRecord] {
        &self.completed_invocations
    }
    pub fn uncertain_invocations(&self) -> &[UncertainInvocationRecord] {
        &self.uncertain_invocations
    }
    pub fn pending_approval(&self) -> Option<&PendingApprovalRecord> {
        self.pending_approval.as_ref()
    }
    pub fn message_context_refs(&self) -> &[OpaqueReference] {
        &self.message_context_refs
    }
    pub fn model_context_refs(&self) -> &[OpaqueReference] {
        &self.model_context_refs
    }
    pub fn budget(&self) -> &Budget {
        &self.budget
    }
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
    pub fn memory_refs(&self) -> &[OpaqueReference] {
        &self.memory_refs
    }
    pub fn artifact_refs(&self) -> &[OpaqueReference] {
        &self.artifact_refs
    }
}
impl<'de> Deserialize<'de> for CheckpointV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::from_wire(CheckpointWire::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
fn canonical<T: Ord>(items: &[T]) -> Result<(), ExecutionError> {
    if items.windows(2).any(|w| w[0] >= w[1]) {
        Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint))
    } else {
        Ok(())
    }
}
fn canonical_by_pair<T, F: Fn(&T) -> (Uuid, u32)>(
    items: &[T],
    key: F,
) -> Result<(), ExecutionError> {
    if items.windows(2).any(|w| key(&w[0]) >= key(&w[1])) {
        Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint))
    } else {
        Ok(())
    }
}
