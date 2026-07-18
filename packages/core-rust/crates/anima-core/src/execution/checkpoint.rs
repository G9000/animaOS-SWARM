use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::state::{
    valid_id, valid_uuid, valid_version, Attempt, Budget, ExecutionError, ExecutionErrorCode,
    RunPauseReason, RunState, Usage,
};
use crate::{
    AgentDefinition, ApprovalRequest, CapabilityManifest, ManifestCatalog,
    SUPPORTED_DEFINITION_SCHEMA_VERSION,
};

pub const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
pub const RUNTIME_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionPin {
    pub schema_version: u32,
    pub id: String,
    pub version: u32,
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
    pub fn from_definition(definition: &AgentDefinition) -> Result<Self, ExecutionError> {
        Self::new(
            definition.schema_version,
            definition.id.clone(),
            definition.version,
        )
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
    pub id: String,
    pub version: u32,
    pub schema_digest: String,
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
    pub fn from_manifest(manifest: &CapabilityManifest) -> Result<Self, ExecutionError> {
        Self::new(
            manifest.id.clone(),
            manifest.version,
            manifest.schema_digest.clone(),
        )
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        valid_id(&self.id)?;
        valid_version(self.version)?;
        if !self.schema_digest.starts_with("sha256:")
            || self.schema_digest.len() <= 7
            || self.schema_digest.len() > MAX_DIGEST_BYTES
            || !self.schema_digest[7..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        Ok(())
    }
}
impl<'de> Deserialize<'de> for ManifestPin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = ManifestPinWire::deserialize(d)?;
        Self::new(w.id, w.version, w.schema_digest).map_err(serde::de::Error::custom)
    }
}
const MAX_DIGEST_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecord {
    pub logical_invocation_id: Uuid,
    pub mode: RunPauseReason,
    pub recovery_key: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryRecordWire {
    logical_invocation_id: Uuid,
    mode: RunPauseReason,
    recovery_key: Option<Uuid>,
}
impl RecoveryRecord {
    fn validate(&self) -> Result<(), ExecutionError> {
        valid_uuid(self.logical_invocation_id)?;
        if self.mode != RunPauseReason::RecoveryRequired {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if let Some(key) = self.recovery_key {
            valid_uuid(key)?;
        }
        Ok(())
    }
}
impl<'de> Deserialize<'de> for RecoveryRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = RecoveryRecordWire::deserialize(d)?;
        let value = Self {
            logical_invocation_id: w.logical_invocation_id,
            mode: w.mode,
            recovery_key: w.recovery_key,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
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
    attempts: Vec<Attempt>,
    message_context_refs: Vec<Uuid>,
    model_context_refs: Vec<Uuid>,
    completed_logical_invocation_ids: Vec<Uuid>,
    uncertain_invocations: Vec<RecoveryRecord>,
    pending_approval: Option<ApprovalRequest>,
    budget: Budget,
    usage: Usage,
    memory_refs: Vec<Uuid>,
    artifact_refs: Vec<Uuid>,
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
    attempts: Vec<Attempt>,
    message_context_refs: Vec<Uuid>,
    model_context_refs: Vec<Uuid>,
    completed_logical_invocation_ids: Vec<Uuid>,
    uncertain_invocations: Vec<RecoveryRecord>,
    pending_approval: Option<ApprovalRequest>,
    budget: Budget,
    usage: Usage,
    memory_refs: Vec<Uuid>,
    artifact_refs: Vec<Uuid>,
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
        Self::from_parts(CheckpointWire {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            runtime_schema_version: RUNTIME_SCHEMA_VERSION,
            definition: DefinitionPin::new(
                SUPPORTED_DEFINITION_SCHEMA_VERSION,
                definition_id,
                definition_version,
            )?,
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
            completed_logical_invocation_ids: vec![],
            uncertain_invocations: vec![],
            pending_approval: None,
            budget: Budget {
                max_wall_time_ms: None,
                max_turns: None,
                max_capability_steps: None,
                max_input_tokens: None,
                max_output_tokens: None,
                max_total_tokens: None,
                max_estimated_cost_micros: None,
                max_concurrent_runs: None,
                max_artifact_bytes: None,
                max_download_bytes: None,
                require_approval_at_percent: None,
            },
            usage: Usage::default(),
            memory_refs: vec![],
            artifact_refs: vec![],
        })
    }
    fn from_parts(w: CheckpointWire) -> Result<Self, ExecutionError> {
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
            completed_logical_invocation_ids: w.completed_logical_invocation_ids,
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
        if (self.state == RunState::Paused) != self.pause_reason.is_some() {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.state == RunState::WaitingForApproval && self.pending_approval.is_none() {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.state != RunState::WaitingForApproval && self.pending_approval.is_some() {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if self.pause_reason == Some(RunPauseReason::RecoveryRequired)
            && self.uncertain_invocations.is_empty()
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        if let Some(cursor) = &self.cursor_step_id {
            valid_id(cursor)?;
        }
        canonical(&self.manifests)?;
        for pin in &self.manifests {
            pin.validate()?;
        }
        for a in &self.attempts {
            if a.number() == 0 {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
            }
        }
        canonical_attempts(&self.attempts)?;
        for ids in [
            &self.message_context_refs,
            &self.model_context_refs,
            &self.completed_logical_invocation_ids,
            &self.memory_refs,
            &self.artifact_refs,
        ] {
            canonical_uuids(ids)?;
        }
        for r in &self.uncertain_invocations {
            r.validate()?;
        }
        if self
            .pending_approval
            .as_ref()
            .is_some_and(|approval| approval.run_id != self.run_id)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
        self.budget.validate()?;
        if self.usage.total_tokens
            != self
                .usage
                .input_tokens
                .checked_add(self.usage.output_tokens)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidUsage));
        }
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
            if pin.schema_digest != cap.schema_digest {
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
}
impl<'de> Deserialize<'de> for CheckpointV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::from_parts(CheckpointWire::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
fn canonical<T: Ord>(items: &[T]) -> Result<(), ExecutionError> {
    if items.windows(2).any(|w| w[0] >= w[1]) {
        Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint))
    } else {
        Ok(())
    }
}
fn canonical_uuids(items: &[Uuid]) -> Result<(), ExecutionError> {
    for id in items {
        valid_uuid(*id)?;
    }
    canonical(items)
}
fn canonical_attempts(items: &[Attempt]) -> Result<(), ExecutionError> {
    let mut seen = BTreeSet::new();
    for a in items {
        if !seen.insert((a.logical_invocation_id(), a.number())) {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidCheckpoint));
        }
    }
    Ok(())
}
