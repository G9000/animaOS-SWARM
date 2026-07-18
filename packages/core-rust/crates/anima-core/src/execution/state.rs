use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::checkpoint::{DefinitionPin, OpaqueReference, RecoveryPauseRecord};
use crate::{
    AgentDefinition, ApprovalDecision, ApprovalDecisionKind, ApprovalRequest, ApprovalValidity,
    AutonomyGrant, GrantConsumption, PolicyContext, PolicyEngine, PolicyReasonCode,
    RecoveryResumeBinding, ValidatedRecoveryResume, CAPABILITY_INVOCATION_NAMESPACE,
    SUPPORTED_DEFINITION_SCHEMA_VERSION,
};

const MAX_ID_BYTES: usize = 256;
const COMMAND_NAMESPACE: Uuid = Uuid::from_u128(0xb83b_d813_b90b_5300_83c7_1766_04a9_c05e);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionConcurrencyPolicy {
    #[default]
    Serial,
    Concurrent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Session {
    id: Uuid,
    definition: DefinitionPin,
    concurrency: SessionConcurrencyPolicy,
    allows_concurrent_sessions: bool,
    #[serde(skip)]
    verified: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SessionWire {
    Current(CurrentSessionWire),
    Legacy(LegacySessionWire),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentSessionWire {
    id: Uuid,
    definition: DefinitionPin,
    concurrency: SessionConcurrencyPolicy,
    allows_concurrent_sessions: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySessionWire {
    id: Uuid,
    definition_id: String,
    definition_version: u32,
    concurrency: SessionConcurrencyPolicy,
    concurrency_pinned: bool,
}

impl Session {
    pub fn new(
        id: Uuid,
        definition_id: impl Into<String>,
        definition_version: u32,
        concurrency: SessionConcurrencyPolicy,
    ) -> Result<Self, ExecutionError> {
        if concurrency == SessionConcurrencyPolicy::Concurrent {
            return Err(ExecutionError::new(ExecutionErrorCode::ConcurrentNotPinned));
        }
        Self::new_inner(
            id,
            DefinitionPin::new(
                SUPPORTED_DEFINITION_SCHEMA_VERSION,
                definition_id,
                definition_version,
            )?,
            concurrency,
            false,
            true,
        )
    }

    pub fn new_for_definition(
        id: Uuid,
        definition: &AgentDefinition,
        concurrency: SessionConcurrencyPolicy,
    ) -> Result<Self, ExecutionError> {
        Self::new_inner(
            id,
            DefinitionPin::from_definition(definition)?,
            concurrency,
            definition.lifecycle.allows_concurrent_sessions,
            true,
        )
    }

    fn new_inner(
        id: Uuid,
        definition: DefinitionPin,
        concurrency: SessionConcurrencyPolicy,
        allows_concurrent_sessions: bool,
        verified: bool,
    ) -> Result<Self, ExecutionError> {
        valid_uuid(id)?;
        if concurrency == SessionConcurrencyPolicy::Concurrent && !allows_concurrent_sessions {
            return Err(ExecutionError::new(ExecutionErrorCode::ConcurrentNotPinned));
        }
        Ok(Self {
            id,
            definition,
            concurrency,
            allows_concurrent_sessions,
            verified,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn definition(&self) -> &DefinitionPin {
        &self.definition
    }

    pub fn concurrency(&self) -> Result<SessionConcurrencyPolicy, ExecutionError> {
        if !self.verified {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        Ok(self.concurrency)
    }

    pub fn assert_compatible(&self, definition: &AgentDefinition) -> Result<Self, ExecutionError> {
        let live_pin = DefinitionPin::from_definition(definition)?;
        let live_allows_concurrent = definition.lifecycle.allows_concurrent_sessions;
        if self.definition != live_pin
            || self.allows_concurrent_sessions != live_allows_concurrent
            || (self.concurrency == SessionConcurrencyPolicy::Concurrent && !live_allows_concurrent)
        {
            return Err(ExecutionError::new(
                ExecutionErrorCode::IncompatibleCheckpoint,
            ));
        }
        let mut verified = self.clone();
        verified.verified = true;
        Ok(verified)
    }
}
impl<'de> Deserialize<'de> for Session {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match SessionWire::deserialize(d)? {
            SessionWire::Current(w) => Self::new_inner(
                w.id,
                w.definition,
                w.concurrency,
                w.allows_concurrent_sessions,
                w.concurrency == SessionConcurrencyPolicy::Serial,
            ),
            SessionWire::Legacy(w) => DefinitionPin::new(
                SUPPORTED_DEFINITION_SCHEMA_VERSION,
                w.definition_id,
                w.definition_version,
            )
            .and_then(|definition| {
                Self::new_inner(
                    w.id,
                    definition,
                    w.concurrency,
                    w.concurrency_pinned,
                    w.concurrency == SessionConcurrencyPolicy::Serial,
                )
            }),
        }
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    WaitingForApproval,
    Paused,
    Completed,
    Failed,
    Cancelled,
}
impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPauseReason {
    Requested,
    Budget,
    RecoveryRequired,
    HostShutdown,
    Policy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryTerminalResolution {
    Cancel,
    Fail,
    AdoptExternallyVerifiedResult { result_ref: Uuid },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryTerminalOutcome {
    run: Run,
    adopted_result_ref: Option<OpaqueReference>,
}
impl RecoveryTerminalOutcome {
    pub fn run(&self) -> &Run {
        &self.run
    }
    pub fn adopted_result_ref(&self) -> Option<&OpaqueReference> {
        self.adopted_result_ref.as_ref()
    }
}
impl Deref for RecoveryTerminalOutcome {
    type Target = Run;

    fn deref(&self) -> &Self::Target {
        &self.run
    }
}

/// A validated approval resume and the counted-grant consumption that the host must commit
/// atomically with the resumed run state.
#[must_use = "persist grant_consumption atomically with the resumed run state"]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalResumeOutcome {
    run: Run,
    grant_consumption: Option<GrantConsumption>,
}
impl ApprovalResumeOutcome {
    pub fn run(&self) -> &Run {
        &self.run
    }
    pub fn grant_consumption(&self) -> Option<&GrantConsumption> {
        self.grant_consumption.as_ref()
    }
    pub fn into_parts(self) -> (Run, Option<GrantConsumption>) {
        (self.run, self.grant_consumption)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Run {
    id: Uuid,
    session_id: Uuid,
    definition_id: String,
    definition_version: u32,
    state: RunState,
    pause_reason: Option<RunPauseReason>,
    pending_approval: Option<ApprovalRequest>,
    recovery_pause: Option<RecoveryPauseRecord>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunWire {
    id: Uuid,
    session_id: Uuid,
    definition_id: String,
    definition_version: u32,
    state: RunState,
    pause_reason: Option<RunPauseReason>,
    #[serde(default)]
    pending_approval: Option<ApprovalRequest>,
    #[serde(default)]
    recovery_pause: Option<RecoveryPauseRecord>,
}
impl Run {
    pub fn queued(
        id: Uuid,
        session_id: Uuid,
        definition_id: impl Into<String>,
        definition_version: u32,
    ) -> Result<Self, ExecutionError> {
        Self::from_parts(
            id,
            session_id,
            definition_id.into(),
            definition_version,
            RunState::Queued,
            None,
            None,
            None,
        )
    }
    fn from_parts(
        id: Uuid,
        session_id: Uuid,
        definition_id: String,
        definition_version: u32,
        state: RunState,
        pause_reason: Option<RunPauseReason>,
        pending_approval: Option<ApprovalRequest>,
        recovery_pause: Option<RecoveryPauseRecord>,
    ) -> Result<Self, ExecutionError> {
        valid_uuid(id)?;
        valid_uuid(session_id)?;
        valid_id(&definition_id)?;
        valid_version(definition_version)?;
        if (state == RunState::Paused) != pause_reason.is_some() {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidState));
        }
        if (state == RunState::WaitingForApproval) != pending_approval.is_some()
            || (pause_reason == Some(RunPauseReason::RecoveryRequired)) != recovery_pause.is_some()
            || pending_approval
                .as_ref()
                .is_some_and(|request| request.run_id != id)
            || recovery_pause
                .as_ref()
                .is_some_and(|pause| pause.invocation().run_id() != id)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidState));
        }
        Ok(Self {
            id,
            session_id,
            definition_id,
            definition_version,
            state,
            pause_reason,
            pending_approval,
            recovery_pause,
        })
    }
    pub fn id(&self) -> Uuid {
        self.id
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
    pub fn pause_reason(&self) -> Option<RunPauseReason> {
        self.pause_reason
    }
    pub fn pending_approval(&self) -> Option<&ApprovalRequest> {
        self.pending_approval.as_ref()
    }
    pub fn recovery_pause(&self) -> Option<&RecoveryPauseRecord> {
        self.recovery_pause.as_ref()
    }
    pub fn transition(
        &self,
        target: RunState,
        pause_reason: Option<RunPauseReason>,
    ) -> Result<Self, ExecutionError> {
        if self.state.is_terminal() || !legal_transition(self.state, target) {
            return Err(ExecutionError::new(ExecutionErrorCode::IllegalTransition));
        }
        if target == RunState::Paused && pause_reason.is_none() {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        if target != RunState::Paused && pause_reason.is_some() {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidState));
        }
        if target == RunState::WaitingForApproval
            || pause_reason == Some(RunPauseReason::RecoveryRequired)
        {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        Self::from_parts(
            self.id,
            self.session_id,
            self.definition_id.clone(),
            self.definition_version,
            target,
            pause_reason,
            None,
            None,
        )
    }

    pub fn wait_for_approval(&self, request: ApprovalRequest) -> Result<Self, ExecutionError> {
        if self.state != RunState::Running || request.run_id != self.id {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        Self::from_parts(
            self.id,
            self.session_id,
            self.definition_id.clone(),
            self.definition_version,
            RunState::WaitingForApproval,
            None,
            Some(request),
            None,
        )
    }

    pub fn pause_for_recovery(&self, pause: RecoveryPauseRecord) -> Result<Self, ExecutionError> {
        if self.state != RunState::Running || pause.invocation().run_id() != self.id {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        Self::from_parts(
            self.id,
            self.session_id,
            self.definition_id.clone(),
            self.definition_version,
            RunState::Paused,
            Some(RunPauseReason::RecoveryRequired),
            None,
            Some(pause),
        )
    }
    pub fn resume(
        &self,
        _approval_claim: Option<(Uuid, Uuid)>,
        _recovery_key: Option<Uuid>,
    ) -> Result<Self, ExecutionError> {
        match self.state {
            RunState::WaitingForApproval => {
                Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite))
            }
            RunState::Paused if self.pause_reason == Some(RunPauseReason::RecoveryRequired) => {
                Err(ExecutionError::new(ExecutionErrorCode::RecoveryRequired))
            }
            RunState::Paused => Self::from_parts(
                self.id,
                self.session_id,
                self.definition_id.clone(),
                self.definition_version,
                RunState::Running,
                None,
                None,
                None,
            ),
            _ => Err(ExecutionError::new(ExecutionErrorCode::IllegalTransition)),
        }
    }
    pub fn resume_with_claim(
        &self,
        _request_id: Uuid,
        _decision_id: Uuid,
    ) -> Result<Self, ExecutionError> {
        Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite))
    }
    pub fn resume_with_recovery(&self, _key: Uuid) -> Result<Self, ExecutionError> {
        Err(ExecutionError::new(ExecutionErrorCode::RecoveryRequired))
    }
    pub fn resume_with_pending_approval(
        &self,
        pending: &ApprovalRequest,
        decision: &ApprovalDecision,
        context: &PolicyContext,
        grants: &[AutonomyGrant],
    ) -> Result<ApprovalResumeOutcome, ExecutionError> {
        let claim = ApprovalResumeClaim::new(pending, decision, context, grants)?;
        if self.pending_approval.as_ref() != Some(pending)
            || claim.binding.decision.request != *pending
        {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        let run = Self::from_parts(
            self.id,
            self.session_id,
            self.definition_id.clone(),
            self.definition_version,
            RunState::Running,
            None,
            None,
            None,
        )?;
        Ok(ApprovalResumeOutcome {
            run,
            grant_consumption: claim.grant_consumption,
        })
    }

    pub fn apply_resume_command(
        &self,
        command: &RuntimeCommand,
        approval_claim: Option<&ApprovalResumeClaim>,
        recovery_claim: Option<&ValidatedRecoveryResume>,
    ) -> Result<Self, ExecutionError> {
        let Some((run_id, approval_binding, recovery_binding)) = command.resume_parts() else {
            return Err(ExecutionError::new(ExecutionErrorCode::IllegalTransition));
        };
        if command.session_id != self.session_id || run_id != self.id {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        match self.state {
            RunState::WaitingForApproval => {
                let (Some(expected), Some(command_binding), Some(claim)) = (
                    self.pending_approval.as_ref(),
                    approval_binding,
                    approval_claim,
                ) else {
                    return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
                };
                if recovery_binding.is_some()
                    || recovery_claim.is_some()
                    || claim.binding() != command_binding
                    || &claim.binding().decision.request != expected
                {
                    return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
                }
            }
            RunState::Paused if self.pause_reason == Some(RunPauseReason::RecoveryRequired) => {
                let (Some(expected), Some(command_binding), Some(claim)) = (
                    self.recovery_pause.as_ref(),
                    recovery_binding,
                    recovery_claim,
                ) else {
                    return Err(ExecutionError::new(ExecutionErrorCode::RecoveryRequired));
                };
                if approval_binding.is_some()
                    || approval_claim.is_some()
                    || claim.binding() != command_binding
                    || !expected.matches_resume_binding(command_binding)
                {
                    return Err(ExecutionError::new(ExecutionErrorCode::RecoveryRequired));
                }
            }
            RunState::Paused => {
                if approval_binding.is_some()
                    || recovery_binding.is_some()
                    || approval_claim.is_some()
                    || recovery_claim.is_some()
                {
                    return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
                }
            }
            _ => return Err(ExecutionError::new(ExecutionErrorCode::IllegalTransition)),
        }
        Self::from_parts(
            self.id,
            self.session_id,
            self.definition_id.clone(),
            self.definition_version,
            RunState::Running,
            None,
            None,
            None,
        )
    }

    pub fn resolve_recovery_terminal(
        &self,
        resolution: RecoveryTerminalResolution,
    ) -> Result<RecoveryTerminalOutcome, ExecutionError> {
        if self.state != RunState::Paused
            || self.pause_reason != Some(RunPauseReason::RecoveryRequired)
            || self.recovery_pause.is_none()
        {
            return Err(ExecutionError::new(ExecutionErrorCode::IllegalTransition));
        }
        let (target, adopted_result_ref) = match resolution {
            RecoveryTerminalResolution::Cancel => (RunState::Cancelled, None),
            RecoveryTerminalResolution::Fail => (RunState::Failed, None),
            RecoveryTerminalResolution::AdoptExternallyVerifiedResult { result_ref } => {
                (RunState::Completed, Some(OpaqueReference::new(result_ref)?))
            }
        };
        let run = Self::from_parts(
            self.id,
            self.session_id,
            self.definition_id.clone(),
            self.definition_version,
            target,
            None,
            None,
            None,
        )?;
        Ok(RecoveryTerminalOutcome {
            run,
            adopted_result_ref,
        })
    }
    /// A host records control intent while work is active, then calls this at a safe boundary.
    pub fn request_pause_or_cancel(
        &self,
        cancel_requested: bool,
        pause_requested: bool,
        safe_boundary: bool,
    ) -> Result<Self, ExecutionError> {
        if !safe_boundary {
            return Ok(self.clone());
        }
        if cancel_requested {
            return self.transition(RunState::Cancelled, None);
        }
        if pause_requested {
            return self.transition(RunState::Paused, Some(RunPauseReason::Requested));
        }
        Ok(self.clone())
    }
}
impl<'de> Deserialize<'de> for Run {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = RunWire::deserialize(d)?;
        Self::from_parts(
            w.id,
            w.session_id,
            w.definition_id,
            w.definition_version,
            w.state,
            w.pause_reason,
            w.pending_approval,
            w.recovery_pause,
        )
        .map_err(serde::de::Error::custom)
    }
}
fn legal_transition(from: RunState, to: RunState) -> bool {
    matches!(
        (from, to),
        (RunState::Queued, RunState::Running)
            | (
                RunState::Running,
                RunState::WaitingForApproval
                    | RunState::Paused
                    | RunState::Completed
                    | RunState::Failed
                    | RunState::Cancelled
            )
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Model,
    Capability,
    Policy,
    Memory,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Step {
    run_id: Uuid,
    logical_step_id: String,
    kind: StepKind,
    attempts: Vec<Attempt>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepWire {
    run_id: Uuid,
    logical_step_id: String,
    kind: StepKind,
    attempts: Vec<Attempt>,
}
impl Step {
    pub fn new(
        run_id: Uuid,
        logical_step_id: impl Into<String>,
        kind: StepKind,
    ) -> Result<Self, ExecutionError> {
        let logical_step_id = logical_step_id.into();
        valid_uuid(run_id)?;
        valid_id(&logical_step_id)?;
        Ok(Self {
            run_id,
            logical_step_id,
            kind,
            attempts: vec![],
        })
    }
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn logical_step_id(&self) -> &str {
        &self.logical_step_id
    }
    pub fn kind(&self) -> StepKind {
        self.kind
    }
    pub fn start_attempt(&self, logical_invocation_id: Uuid) -> Result<Attempt, ExecutionError> {
        Attempt::new(
            logical_invocation_id,
            (self.attempts.len() as u32)
                .checked_add(1)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?,
        )
    }
    pub fn append_attempt(&self, attempt: Attempt) -> Result<Self, ExecutionError> {
        if self
            .attempts
            .first()
            .is_some_and(|first| first.logical_invocation_id() != attempt.logical_invocation_id())
            || attempt.number() != self.attempts.len() as u32 + 1
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidAttempt));
        }
        let mut next = self.clone();
        next.attempts.push(attempt);
        Ok(next)
    }
}
impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = StepWire::deserialize(d)?;
        let mut step =
            Self::new(w.run_id, w.logical_step_id, w.kind).map_err(serde::de::Error::custom)?;
        let invocation = w.attempts.first().map(Attempt::logical_invocation_id);
        for (i, a) in w.attempts.iter().enumerate() {
            if a.number != (i as u32) + 1 || invocation != Some(a.logical_invocation_id()) {
                return Err(serde::de::Error::custom(ExecutionError::new(
                    ExecutionErrorCode::InvalidAttempt,
                )));
            }
        }
        step.attempts = w.attempts;
        Ok(step)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Attempt {
    id: Uuid,
    number: u32,
    logical_invocation_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptWire {
    id: Uuid,
    number: u32,
    logical_invocation_id: Uuid,
}
impl Attempt {
    pub fn new(logical_invocation_id: Uuid, number: u32) -> Result<Self, ExecutionError> {
        valid_uuid(logical_invocation_id)?;
        if number == 0 {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidAttempt));
        }
        let id = attempt_uuid(logical_invocation_id, number);
        Ok(Self {
            id,
            number,
            logical_invocation_id,
        })
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn number(&self) -> u32 {
        self.number
    }
    pub fn logical_invocation_id(&self) -> Uuid {
        self.logical_invocation_id
    }
    pub fn retry(&self) -> Result<Self, ExecutionError> {
        Self::new(
            self.logical_invocation_id,
            self.number
                .checked_add(1)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?,
        )
    }
}
impl<'de> Deserialize<'de> for Attempt {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = AttemptWire::deserialize(d)?;
        let a = Self::new(w.logical_invocation_id, w.number).map_err(serde::de::Error::custom)?;
        if a.id != w.id {
            return Err(serde::de::Error::custom(ExecutionError::new(
                ExecutionErrorCode::InvalidAttempt,
            )));
        }
        Ok(a)
    }
}
fn attempt_uuid(invocation: Uuid, number: u32) -> Uuid {
    Uuid::new_v5(
        &CAPABILITY_INVOCATION_NAMESPACE,
        format!("attempt={invocation}:number={number}").as_bytes(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionLease {
    pub run_id: Uuid,
    pub fence: Uuid,
    pub expires_at_ms: u64,
}
impl Serialize for ExecutionLease {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        ExecutionLeaseWireRef {
            run_id: self.run_id,
            fence: self.fence,
            expires_at_ms: self.expires_at_ms,
        }
        .serialize(serializer)
    }
}
#[derive(Serialize)]
struct ExecutionLeaseWireRef {
    run_id: Uuid,
    fence: Uuid,
    expires_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionLeaseWire {
    run_id: Uuid,
    fence: Uuid,
    expires_at_ms: u64,
}
impl ExecutionLease {
    pub fn new(run_id: Uuid, fence: Uuid, expires_at_ms: u64) -> Result<Self, ExecutionError> {
        let value = Self {
            run_id,
            fence,
            expires_at_ms,
        };
        value.validate()?;
        Ok(value)
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
    pub fn validate(&self) -> Result<(), ExecutionError> {
        valid_uuid(self.run_id)?;
        valid_uuid(self.fence)?;
        if self.expires_at_ms == 0 {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidIdentifier));
        }
        Ok(())
    }
}
impl<'de> Deserialize<'de> for ExecutionLease {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = ExecutionLeaseWire::deserialize(d)?;
        Self::new(w.run_id, w.fence, w.expires_at_ms).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCommandKind {
    Start,
    Pause,
    Resume,
    Cancel,
    Retry,
}

/// Durable, non-authorizing intent for one exact validated approval decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResumeBinding {
    decision: ApprovalDecision,
}

/// A live policy validation prerequisite. Hosts recreate it against current policy context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalResumeClaim {
    binding: ApprovalResumeBinding,
    grant_consumption: Option<GrantConsumption>,
}

impl ApprovalResumeClaim {
    pub fn new(
        pending: &ApprovalRequest,
        decision: &ApprovalDecision,
        context: &PolicyContext,
        grants: &[AutonomyGrant],
    ) -> Result<Self, ExecutionError> {
        if decision.kind != ApprovalDecisionKind::Approve
            || decision.request != *pending
            || PolicyEngine::validate_approval_with_grants(decision, context, grants)
                != ApprovalValidity::Valid
        {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        let evaluation = PolicyEngine::evaluate_with_approval(context, grants, Some(decision))
            .map_err(|_| ExecutionError::new(ExecutionErrorCode::MissingPrerequisite))?;
        if evaluation.decision.kind() != PolicyReasonCode::AllowedByApproval {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        Ok(Self {
            binding: ApprovalResumeBinding {
                decision: decision.clone(),
            },
            grant_consumption: evaluation.consumption,
        })
    }

    pub fn binding(&self) -> &ApprovalResumeBinding {
        &self.binding
    }
    pub fn grant_consumption(&self) -> Option<&GrantConsumption> {
        self.grant_consumption.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RuntimeCommandPayload {
    Start {
        run_id: Uuid,
    },
    Pause {
        run_id: Uuid,
    },
    Resume {
        run_id: Uuid,
        approval_binding: Option<ApprovalResumeBinding>,
        recovery_binding: Option<RecoveryResumeBinding>,
    },
    Cancel {
        run_id: Uuid,
    },
    Retry {
        run_id: Uuid,
        logical_invocation_id: Uuid,
    },
}
impl RuntimeCommandPayload {
    fn kind(&self) -> RuntimeCommandKind {
        match self {
            Self::Start { .. } => RuntimeCommandKind::Start,
            Self::Pause { .. } => RuntimeCommandKind::Pause,
            Self::Resume { .. } => RuntimeCommandKind::Resume,
            Self::Cancel { .. } => RuntimeCommandKind::Cancel,
            Self::Retry { .. } => RuntimeCommandKind::Retry,
        }
    }
    fn run_id(&self) -> Uuid {
        match self {
            Self::Start { run_id }
            | Self::Pause { run_id }
            | Self::Resume { run_id, .. }
            | Self::Cancel { run_id }
            | Self::Retry { run_id, .. } => *run_id,
        }
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        let ids = match self {
            Self::Start { run_id } | Self::Pause { run_id } | Self::Cancel { run_id } => {
                vec![*run_id]
            }
            Self::Retry {
                run_id,
                logical_invocation_id,
            } => vec![*run_id, *logical_invocation_id],
            Self::Resume {
                run_id,
                approval_binding,
                recovery_binding,
            } => {
                if approval_binding.is_some() && recovery_binding.is_some() {
                    return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
                }
                vec![*run_id]
            }
        };
        for id in ids {
            valid_uuid(id)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeCommand {
    id: Uuid,
    session_id: Uuid,
    payload: RuntimeCommandPayload,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeCommandWire {
    id: Uuid,
    session_id: Uuid,
    payload: RuntimeCommandPayload,
}
impl RuntimeCommand {
    fn new(
        id: Uuid,
        session_id: Uuid,
        payload: RuntimeCommandPayload,
    ) -> Result<Self, ExecutionError> {
        valid_uuid(id)?;
        valid_uuid(session_id)?;
        payload.validate()?;
        Ok(Self {
            id,
            session_id,
            payload,
        })
    }
    pub fn start(id: Uuid, session_id: Uuid, run_id: Uuid) -> Result<Self, ExecutionError> {
        Self::new(id, session_id, RuntimeCommandPayload::Start { run_id })
    }
    pub fn pause(id: Uuid, session_id: Uuid, run_id: Uuid) -> Result<Self, ExecutionError> {
        Self::new(id, session_id, RuntimeCommandPayload::Pause { run_id })
    }
    pub fn cancel(id: Uuid, session_id: Uuid, run_id: Uuid) -> Result<Self, ExecutionError> {
        Self::new(id, session_id, RuntimeCommandPayload::Cancel { run_id })
    }
    pub fn resume(
        id: Uuid,
        session_id: Uuid,
        run_id: Uuid,
        claim: Option<(Uuid, Uuid)>,
        recovery_key: Option<Uuid>,
    ) -> Result<Self, ExecutionError> {
        if claim.is_some() || recovery_key.is_some() {
            return Err(ExecutionError::new(ExecutionErrorCode::MissingPrerequisite));
        }
        Self::new(
            id,
            session_id,
            RuntimeCommandPayload::Resume {
                run_id,
                approval_binding: None,
                recovery_binding: None,
            },
        )
    }
    pub fn resume_with_approval(
        id: Uuid,
        session_id: Uuid,
        run_id: Uuid,
        approval_binding: ApprovalResumeBinding,
    ) -> Result<Self, ExecutionError> {
        Self::new(
            id,
            session_id,
            RuntimeCommandPayload::Resume {
                run_id,
                approval_binding: Some(approval_binding),
                recovery_binding: None,
            },
        )
    }
    pub fn resume_with_recovery_binding(
        id: Uuid,
        session_id: Uuid,
        run_id: Uuid,
        recovery_binding: RecoveryResumeBinding,
    ) -> Result<Self, ExecutionError> {
        Self::new(
            id,
            session_id,
            RuntimeCommandPayload::Resume {
                run_id,
                approval_binding: None,
                recovery_binding: Some(recovery_binding),
            },
        )
    }
    pub fn retry(
        id: Uuid,
        session_id: Uuid,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Self, ExecutionError> {
        Self::new(
            id,
            session_id,
            RuntimeCommandPayload::Retry {
                run_id,
                logical_invocation_id,
            },
        )
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
    pub fn target_run_id(&self) -> Uuid {
        self.payload.run_id()
    }
    pub fn kind(&self) -> RuntimeCommandKind {
        self.payload.kind()
    }
    pub fn payload_digest(&self) -> Uuid {
        self.digest()
    }
    fn resume_parts(
        &self,
    ) -> Option<(
        Uuid,
        Option<&ApprovalResumeBinding>,
        Option<&RecoveryResumeBinding>,
    )> {
        match &self.payload {
            RuntimeCommandPayload::Resume {
                run_id,
                approval_binding,
                recovery_binding,
            } => Some((
                *run_id,
                approval_binding.as_ref(),
                recovery_binding.as_ref(),
            )),
            _ => None,
        }
    }
    fn digest(&self) -> Uuid {
        let bytes = serde_jcs::to_vec(&(self.session_id, &self.payload))
            .expect("typed payload is canonical");
        Uuid::new_v5(&COMMAND_NAMESPACE, &bytes)
    }
}
impl<'de> Deserialize<'de> for RuntimeCommand {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = RuntimeCommandWire::deserialize(d)?;
        Self::new(w.id, w.session_id, w.payload).map_err(serde::de::Error::custom)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutcome {
    Accepted,
    Rejected,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    command_id: Uuid,
    payload_digest: Uuid,
    outcome: CommandOutcome,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandReceiptWire {
    command_id: Uuid,
    payload_digest: Uuid,
    outcome: CommandOutcome,
}
impl CommandReceipt {
    fn new(command: &RuntimeCommand, outcome: CommandOutcome) -> Result<Self, ExecutionError> {
        Ok(Self {
            command_id: command.id,
            payload_digest: command.digest(),
            outcome,
        })
    }
    pub fn accepted(command: &RuntimeCommand) -> Result<Self, ExecutionError> {
        Self::new(command, CommandOutcome::Accepted)
    }
    pub fn rejected(command: &RuntimeCommand) -> Result<Self, ExecutionError> {
        Self::new(command, CommandOutcome::Rejected)
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn payload_digest(&self) -> Uuid {
        self.payload_digest
    }
    pub fn outcome(&self) -> CommandOutcome {
        self.outcome
    }
    pub fn replay(&self, command: &RuntimeCommand) -> Result<CommandOutcome, ExecutionError> {
        if self.command_id != command.id || self.payload_digest != command.digest() {
            return Err(ExecutionError::new(ExecutionErrorCode::CommandConflict));
        }
        Ok(self.outcome)
    }
}
impl<'de> Deserialize<'de> for CommandReceipt {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = CommandReceiptWire::deserialize(d)?;
        valid_uuid(w.command_id).map_err(serde::de::Error::custom)?;
        valid_uuid(w.payload_digest).map_err(serde::de::Error::custom)?;
        Ok(Self {
            command_id: w.command_id,
            payload_digest: w.payload_digest,
            outcome: w.outcome,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Usage {
    pub wall_time_ms: u64,
    pub turns: u64,
    pub capability_steps: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost_micros: u64,
    pub concurrent_runs: u64,
    pub artifact_bytes: u64,
    pub download_bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageWire {
    wall_time_ms: u64,
    turns: u64,
    capability_steps: u64,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    estimated_cost_micros: u64,
    concurrent_runs: u64,
    artifact_bytes: u64,
    download_bytes: u64,
}
impl Usage {
    pub fn wall_time_ms(&self) -> u64 {
        self.wall_time_ms
    }
    pub fn turns(&self) -> u64 {
        self.turns
    }
    pub fn capability_steps(&self) -> u64 {
        self.capability_steps
    }
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens
    }
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens
    }
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }
    pub fn estimated_cost_micros(&self) -> u64 {
        self.estimated_cost_micros
    }
    pub fn concurrent_runs(&self) -> u64 {
        self.concurrent_runs
    }
    pub fn with_concurrent_runs(mut self, concurrent_runs: u64) -> Self {
        self.concurrent_runs = concurrent_runs;
        self
    }
    pub fn artifact_bytes(&self) -> u64 {
        self.artifact_bytes
    }
    pub fn download_bytes(&self) -> u64 {
        self.download_bytes
    }
    pub fn validate(&self) -> Result<(), ExecutionError> {
        if self.total_tokens
            != self
                .input_tokens
                .checked_add(self.output_tokens)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidUsage));
        }
        Ok(())
    }
    pub fn checked_add(&self, other: &Self) -> Result<Self, ExecutionError> {
        self.validate()?;
        other.validate()?;
        macro_rules! add {
            ($f:ident) => {
                self.$f
                    .checked_add(other.$f)
                    .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?
            };
        }
        let result = Self {
            wall_time_ms: add!(wall_time_ms),
            turns: add!(turns),
            capability_steps: add!(capability_steps),
            input_tokens: add!(input_tokens),
            output_tokens: add!(output_tokens),
            total_tokens: add!(total_tokens),
            estimated_cost_micros: add!(estimated_cost_micros),
            concurrent_runs: other.concurrent_runs,
            artifact_bytes: add!(artifact_bytes),
            download_bytes: add!(download_bytes),
        };
        if result.total_tokens
            != result
                .input_tokens
                .checked_add(result.output_tokens)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?
        {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidUsage));
        }
        Ok(result)
    }
}
impl Serialize for Usage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        UsageWireRef::from(self).serialize(serializer)
    }
}
#[derive(Serialize)]
struct UsageWireRef {
    wall_time_ms: u64,
    turns: u64,
    capability_steps: u64,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    estimated_cost_micros: u64,
    concurrent_runs: u64,
    artifact_bytes: u64,
    download_bytes: u64,
}
impl From<&Usage> for UsageWireRef {
    fn from(v: &Usage) -> Self {
        Self {
            wall_time_ms: v.wall_time_ms,
            turns: v.turns,
            capability_steps: v.capability_steps,
            input_tokens: v.input_tokens,
            output_tokens: v.output_tokens,
            total_tokens: v.total_tokens,
            estimated_cost_micros: v.estimated_cost_micros,
            concurrent_runs: v.concurrent_runs,
            artifact_bytes: v.artifact_bytes,
            download_bytes: v.download_bytes,
        }
    }
}
impl<'de> Deserialize<'de> for Usage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = UsageWire::deserialize(d)?;
        let value = Self {
            wall_time_ms: w.wall_time_ms,
            turns: w.turns,
            capability_steps: w.capability_steps,
            input_tokens: w.input_tokens,
            output_tokens: w.output_tokens,
            total_tokens: w.total_tokens,
            estimated_cost_micros: w.estimated_cost_micros,
            concurrent_runs: w.concurrent_runs,
            artifact_bytes: w.artifact_bytes,
            download_bytes: w.download_bytes,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    pub max_wall_time_ms: Option<u64>,
    pub max_turns: Option<u64>,
    pub max_capability_steps: Option<u64>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub max_estimated_cost_micros: Option<u64>,
    pub max_concurrent_runs: Option<u64>,
    pub max_artifact_bytes: Option<u64>,
    pub max_download_bytes: Option<u64>,
    pub require_approval_at_percent: Option<u8>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
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
        }
    }
}
impl Serialize for Budget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        BudgetWireRef::from(self).serialize(serializer)
    }
}
#[derive(Serialize)]
struct BudgetWireRef {
    max_wall_time_ms: Option<u64>,
    max_turns: Option<u64>,
    max_capability_steps: Option<u64>,
    max_input_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    max_total_tokens: Option<u64>,
    max_estimated_cost_micros: Option<u64>,
    max_concurrent_runs: Option<u64>,
    max_artifact_bytes: Option<u64>,
    max_download_bytes: Option<u64>,
    require_approval_at_percent: Option<u8>,
}
impl From<&Budget> for BudgetWireRef {
    fn from(v: &Budget) -> Self {
        Self {
            max_wall_time_ms: v.max_wall_time_ms,
            max_turns: v.max_turns,
            max_capability_steps: v.max_capability_steps,
            max_input_tokens: v.max_input_tokens,
            max_output_tokens: v.max_output_tokens,
            max_total_tokens: v.max_total_tokens,
            max_estimated_cost_micros: v.max_estimated_cost_micros,
            max_concurrent_runs: v.max_concurrent_runs,
            max_artifact_bytes: v.max_artifact_bytes,
            max_download_bytes: v.max_download_bytes,
            require_approval_at_percent: v.require_approval_at_percent,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetWire {
    max_wall_time_ms: Option<u64>,
    max_turns: Option<u64>,
    max_capability_steps: Option<u64>,
    max_input_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    max_total_tokens: Option<u64>,
    max_estimated_cost_micros: Option<u64>,
    max_concurrent_runs: Option<u64>,
    max_artifact_bytes: Option<u64>,
    max_download_bytes: Option<u64>,
    require_approval_at_percent: Option<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDecision {
    Continue,
    RequireApproval,
    Exhausted,
}
impl Budget {
    pub fn max_wall_time_ms(&self) -> Option<u64> {
        self.max_wall_time_ms
    }
    pub fn max_turns(&self) -> Option<u64> {
        self.max_turns
    }
    pub fn max_capability_steps(&self) -> Option<u64> {
        self.max_capability_steps
    }
    pub fn max_input_tokens(&self) -> Option<u64> {
        self.max_input_tokens
    }
    pub fn max_output_tokens(&self) -> Option<u64> {
        self.max_output_tokens
    }
    pub fn max_total_tokens(&self) -> Option<u64> {
        self.max_total_tokens
    }
    pub fn max_estimated_cost_micros(&self) -> Option<u64> {
        self.max_estimated_cost_micros
    }
    pub fn max_concurrent_runs(&self) -> Option<u64> {
        self.max_concurrent_runs
    }
    pub fn max_artifact_bytes(&self) -> Option<u64> {
        self.max_artifact_bytes
    }
    pub fn max_download_bytes(&self) -> Option<u64> {
        self.max_download_bytes
    }
    pub fn require_approval_at_percent(&self) -> Option<u8> {
        self.require_approval_at_percent
    }
    pub fn validate(&self) -> Result<(), ExecutionError> {
        for max in [
            self.max_wall_time_ms,
            self.max_turns,
            self.max_capability_steps,
            self.max_input_tokens,
            self.max_output_tokens,
            self.max_total_tokens,
            self.max_estimated_cost_micros,
            self.max_concurrent_runs,
            self.max_artifact_bytes,
            self.max_download_bytes,
        ] {
            if max == Some(0) {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidBudget));
            }
        }
        if let Some(p) = self.require_approval_at_percent {
            if p == 0 || p > 100 {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidBudget));
            }
        }
        Ok(())
    }
    pub fn evaluate(&self, usage: &Usage) -> Result<BudgetDecision, ExecutionError> {
        self.validate()?;
        usage.validate()?;
        let pairs = [
            (usage.wall_time_ms, self.max_wall_time_ms),
            (usage.turns, self.max_turns),
            (usage.capability_steps, self.max_capability_steps),
            (usage.input_tokens, self.max_input_tokens),
            (usage.output_tokens, self.max_output_tokens),
            (usage.total_tokens, self.max_total_tokens),
            (usage.estimated_cost_micros, self.max_estimated_cost_micros),
            (usage.concurrent_runs, self.max_concurrent_runs),
            (usage.artifact_bytes, self.max_artifact_bytes),
            (usage.download_bytes, self.max_download_bytes),
        ];
        if pairs
            .iter()
            .any(|(used, max)| max.is_some_and(|m| *used > m))
        {
            return Ok(BudgetDecision::Exhausted);
        }
        if let Some(p) = self.require_approval_at_percent {
            if pairs.iter().any(|(used, max)| {
                max.is_some_and(|m| (*used as u128) * 100 >= (m as u128) * (p as u128))
            }) {
                return Ok(BudgetDecision::RequireApproval);
            }
        }
        Ok(BudgetDecision::Continue)
    }
    pub fn accumulate(&self, usage: &Usage, delta: &Usage) -> Result<Usage, ExecutionError> {
        self.validate()?;
        usage.validate()?;
        delta.validate()?;
        let next = usage.checked_add(delta)?;
        match self.evaluate(&next)? {
            BudgetDecision::Exhausted => {
                Err(ExecutionError::new(ExecutionErrorCode::BudgetExceeded))
            }
            _ => Ok(next),
        }
    }
}
impl<'de> Deserialize<'de> for Budget {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = BudgetWire::deserialize(d)?;
        let value = Self {
            max_wall_time_ms: w.max_wall_time_ms,
            max_turns: w.max_turns,
            max_capability_steps: w.max_capability_steps,
            max_input_tokens: w.max_input_tokens,
            max_output_tokens: w.max_output_tokens,
            max_total_tokens: w.max_total_tokens,
            max_estimated_cost_micros: w.max_estimated_cost_micros,
            max_concurrent_runs: w.max_concurrent_runs,
            max_artifact_bytes: w.max_artifact_bytes,
            max_download_bytes: w.max_download_bytes,
            require_approval_at_percent: w.require_approval_at_percent,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionErrorCode {
    InvalidIdentifier,
    InvalidVersion,
    InvalidState,
    IllegalTransition,
    MissingPrerequisite,
    RecoveryRequired,
    InvalidAttempt,
    ConcurrentNotPinned,
    CommandConflict,
    ArithmeticOverflow,
    InvalidUsage,
    InvalidBudget,
    BudgetExceeded,
    InvalidCheckpoint,
    IncompatibleCheckpoint,
    InvalidEvent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionError {
    code: ExecutionErrorCode,
}
impl ExecutionError {
    pub fn new(code: ExecutionErrorCode) -> Self {
        Self { code }
    }
    pub fn code(self) -> ExecutionErrorCode {
        self.code
    }
}
impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.code {
            ExecutionErrorCode::InvalidIdentifier => "execution identifier is invalid",
            ExecutionErrorCode::InvalidVersion => "execution version is invalid",
            ExecutionErrorCode::InvalidState => "execution state is invalid",
            ExecutionErrorCode::IllegalTransition => "execution transition is not allowed",
            ExecutionErrorCode::MissingPrerequisite => "execution prerequisite is missing",
            ExecutionErrorCode::RecoveryRequired => "recovery authorization is required",
            ExecutionErrorCode::InvalidAttempt => "execution attempt is invalid",
            ExecutionErrorCode::ConcurrentNotPinned => {
                "concurrent sessions require a pinned definition setting"
            }
            ExecutionErrorCode::CommandConflict => {
                "command identifier conflicts with its canonical payload"
            }
            ExecutionErrorCode::ArithmeticOverflow => "execution arithmetic overflowed",
            ExecutionErrorCode::InvalidUsage => "execution usage is inconsistent",
            ExecutionErrorCode::InvalidBudget => "execution budget is invalid",
            ExecutionErrorCode::BudgetExceeded => "execution budget is exhausted",
            ExecutionErrorCode::InvalidCheckpoint => "execution checkpoint is invalid",
            ExecutionErrorCode::IncompatibleCheckpoint => "execution checkpoint is incompatible",
            ExecutionErrorCode::InvalidEvent => "execution event is invalid",
        })
    }
}
impl std::error::Error for ExecutionError {}
pub(crate) fn valid_uuid(id: Uuid) -> Result<(), ExecutionError> {
    if id.is_nil() {
        Err(ExecutionError::new(ExecutionErrorCode::InvalidIdentifier))
    } else {
        Ok(())
    }
}
pub(crate) fn valid_id(value: &str) -> Result<(), ExecutionError> {
    if value.trim().is_empty() || value.len() > MAX_ID_BYTES {
        Err(ExecutionError::new(ExecutionErrorCode::InvalidIdentifier))
    } else {
        Ok(())
    }
}
pub(crate) fn valid_version(value: u32) -> Result<(), ExecutionError> {
    if value == 0 {
        Err(ExecutionError::new(ExecutionErrorCode::InvalidVersion))
    } else {
        Ok(())
    }
}
