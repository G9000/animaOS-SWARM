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
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPinWire {
    id: String,
    version: u32,
    schema_digest: String,
}
impl ManifestPin {
    pub fn new(
        id: impl Into<String>,
        version: u32,
        schema_digest: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            id: id.into(),
            version,
            schema_digest: schema_digest.into(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn from_manifest(value: &CapabilityManifest) -> Result<Self, ExecutionError> {
        Self::new(value.id.clone(), value.version, value.schema_digest.clone())
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
}
impl<'de> Deserialize<'de> for ManifestPin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = ManifestPinWire::deserialize(d)?;
        Self::new(w.id, w.version, w.schema_digest).map_err(serde::de::Error::custom)
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
            || matches!(
                self.recovery_mode,
                RecoveryMode::None | RecoveryMode::Compensate | RecoveryMode::Manual
            )
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncertainInvocationRecord {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    manifest: ManifestPin,
    recovery_mode: RecoveryMode,
    recovery_binding: Option<RecoveryResumeBinding>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncertainInvocationRecordWire {
    invocation: LogicalInvocationBinding,
    attempt_number: u32,
    manifest: ManifestPin,
    recovery_mode: RecoveryMode,
    recovery_binding: Option<RecoveryResumeBinding>,
}
impl UncertainInvocationRecord {
    pub fn new(
        invocation: LogicalInvocationBinding,
        attempt_number: u32,
        manifest: ManifestPin,
        recovery_mode: RecoveryMode,
        recovery_binding: Option<RecoveryResumeBinding>,
    ) -> Result<Self, ExecutionError> {
        let value = Self {
            invocation,
            attempt_number,
            manifest,
            recovery_mode,
            recovery_binding,
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
            self.recovery_mode,
        )?;
        if let Some(binding) = &self.recovery_binding {
            if binding.logical_invocation_id() != self.invocation.id()
                || binding.completed_attempt_number() != self.attempt_number
                || binding.manifest_id() != self.manifest.id()
                || binding.manifest_version() != self.manifest.version()
                || binding.manifest_digest() != self.manifest.schema_digest()
                || binding.recovery_mode() != self.recovery_mode
                || binding.idempotency_key() != self.invocation.idempotency_key()
            {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
        }
        Ok(())
    }
    pub fn invocation(&self) -> &LogicalInvocationBinding {
        &self.invocation
    }
    pub fn attempt_number(&self) -> u32 {
        self.attempt_number
    }
    pub fn recovery_binding(&self) -> Option<&RecoveryResumeBinding> {
        self.recovery_binding.as_ref()
    }
}
impl<'de> Deserialize<'de> for UncertainInvocationRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = UncertainInvocationRecordWire::deserialize(d)?;
        Self::new(
            w.invocation,
            w.attempt_number,
            w.manifest,
            w.recovery_mode,
            w.recovery_binding,
        )
        .map_err(serde::de::Error::custom)
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
        if decision
            .as_ref()
            .is_some_and(|d| d.request != request || d.kind != ApprovalDecisionKind::Approve)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        Ok(Self { request, decision })
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
    cursor_step_id: Option<String>,
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
    cursor_step_id: Option<String>,
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
                cursor_step_id: None,
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
        }
    }
    pub fn state(mut self, state: RunState, pause_reason: Option<RunPauseReason>) -> Self {
        self.wire.state = state;
        self.wire.pause_reason = pause_reason;
        self
    }
    pub fn cursor_step_id(mut self, value: Option<String>) -> Self {
        self.wire.cursor_step_id = value;
        self
    }
    pub fn attempts(mut self, mut value: Vec<InvocationAttemptRecord>) -> Self {
        value.sort_by_key(|r| (r.invocation.id(), r.attempt_number));
        self.wire.attempts = value;
        self
    }
    pub fn completed_invocations(mut self, mut value: Vec<CompletedInvocationRecord>) -> Self {
        value.sort_by_key(|r| r.invocation.id());
        self.wire.completed_invocations = value;
        self
    }
    pub fn uncertain_invocations(mut self, mut value: Vec<UncertainInvocationRecord>) -> Self {
        value.sort_by_key(|r| r.invocation.id());
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
    pub fn build(self) -> Result<CheckpointV1, ExecutionError> {
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
            cursor_step_id: w.cursor_step_id,
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
            || (self.state == RunState::WaitingForApproval) != self.pending_approval.is_some()
            || self
                .pending_approval
                .as_ref()
                .is_some_and(|p| p.request.run_id != self.run_id)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if let Some(cursor) = &self.cursor_step_id {
            valid_id(cursor)?;
        }
        canonical(&self.manifests)?;
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
        canonical_by_id(&self.completed_invocations, |r| r.invocation.id())?;
        canonical_by_id(&self.uncertain_invocations, |r| r.invocation.id())?;
        let completed_ids: BTreeSet<_> = self
            .completed_invocations
            .iter()
            .map(|r| r.invocation.id())
            .collect();
        if self
            .uncertain_invocations
            .iter()
            .any(|r| completed_ids.contains(&r.invocation.id()))
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
                a.invocation == uncertain.invocation
                    && a.attempt_number == uncertain.attempt_number
                    && a.state == AttemptRecordState::Uncertain
                    && a.manifest == uncertain.manifest
                    && a.recovery_mode == uncertain.recovery_mode
            }) {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
        }
        for attempt in &self.attempts {
            match attempt.state {
                AttemptRecordState::Completed
                    if !self.completed_invocations.iter().any(|r| {
                        r.invocation == attempt.invocation
                            && r.attempt_number == attempt.attempt_number
                    }) =>
                {
                    return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
                }
                AttemptRecordState::Uncertain
                    if !self.uncertain_invocations.iter().any(|r| {
                        r.invocation == attempt.invocation
                            && r.attempt_number == attempt.attempt_number
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
        if pending.len() > 1
            || pending.first().is_some_and(|a| {
                self.cursor_step_id.as_deref() != Some(a.invocation.logical_step_id())
            })
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.pause_reason == Some(RunPauseReason::RecoveryRequired)
            && (self.uncertain_invocations.is_empty()
                || self
                    .uncertain_invocations
                    .iter()
                    .any(|r| r.recovery_binding.is_none()))
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.pause_reason != Some(RunPauseReason::RecoveryRequired)
            && self
                .uncertain_invocations
                .iter()
                .any(|r| r.recovery_binding.is_some())
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.cursor_step_id.as_ref().is_some_and(|cursor| {
            !self
                .attempts
                .iter()
                .any(|a| a.invocation.logical_step_id() == cursor)
        }) || self.state == RunState::Queued
            && (!self.attempts.is_empty() || self.cursor_step_id.is_some())
            || self.state == RunState::Completed
                && (self.cursor_step_id.is_some() || !self.uncertain_invocations.is_empty())
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
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
        self.cursor_step_id.as_deref()
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
fn canonical_by_id<T, F: Fn(&T) -> Uuid>(items: &[T], id: F) -> Result<(), ExecutionError> {
    if items.windows(2).any(|w| id(&w[0]) >= id(&w[1])) {
        Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint))
    } else {
        Ok(())
    }
}
