use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::state::{valid_uuid, ExecutionError, ExecutionErrorCode};

pub const RUNTIME_EVENT_SCHEMA_VERSION: u32 = 1;

/// Stable durable lifecycle kinds. `UsageTokenDelta` is explicitly live-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    RunQueued,
    RunStarted,
    RunWaitingForApproval,
    RunPaused,
    RunResumed,
    RunCompleted,
    RunFailed,
    RunCancelled,
    ModelStarted,
    ModelCompleted,
    CapabilityStarted,
    CapabilityCompleted,
    PolicyEvaluated,
    ApprovalRequested,
    ApprovalResolved,
    MemoryRead,
    MemoryWritten,
    CheckpointSaved,
    ArtifactRecorded,
    UsageRecorded,
    UsageTokenDelta,
    Error,
}

/// Only safe codes and opaque UUID references may cross the durable event boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SafeEventPayload {
    None,
    Reference {
        reference: Uuid,
    },
    Error {
        code: ExecutionErrorCode,
        reference: Option<Uuid>,
    },
    TokenDelta {
        input_tokens: u64,
        output_tokens: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeEvent {
    schema_version: u32,
    sequence: u64,
    run_id: Uuid,
    kind: RuntimeEventKind,
    payload: SafeEventPayload,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeEventWire {
    schema_version: u32,
    sequence: u64,
    run_id: Uuid,
    kind: RuntimeEventKind,
    payload: SafeEventPayload,
}

impl RuntimeEvent {
    pub fn new(
        sequence: u64,
        run_id: Uuid,
        kind: RuntimeEventKind,
    ) -> Result<Self, ExecutionError> {
        Self::with_payload(sequence, run_id, kind, SafeEventPayload::None)
    }
    pub fn with_payload(
        sequence: u64,
        run_id: Uuid,
        kind: RuntimeEventKind,
        payload: SafeEventPayload,
    ) -> Result<Self, ExecutionError> {
        if sequence == 0 {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent));
        }
        valid_uuid(run_id)?;
        match (&kind, &payload) {
            (RuntimeEventKind::UsageTokenDelta, SafeEventPayload::TokenDelta { .. }) => {}
            (RuntimeEventKind::UsageTokenDelta, _) => {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent))
            }
            (_, SafeEventPayload::TokenDelta { .. }) => {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent))
            }
            (_, SafeEventPayload::Reference { reference }) => valid_uuid(*reference)?,
            (
                _,
                SafeEventPayload::Error {
                    reference: Some(reference),
                    ..
                },
            ) => valid_uuid(*reference)?,
            _ => {}
        }
        Ok(Self {
            schema_version: RUNTIME_EVENT_SCHEMA_VERSION,
            sequence,
            run_id,
            kind,
            payload,
        })
    }
    pub fn token_delta(
        sequence: u64,
        run_id: Uuid,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<Self, ExecutionError> {
        Self::with_payload(
            sequence,
            run_id,
            RuntimeEventKind::UsageTokenDelta,
            SafeEventPayload::TokenDelta {
                input_tokens,
                output_tokens,
            },
        )
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn kind(&self) -> RuntimeEventKind {
        self.kind
    }
    pub fn is_checkpoint_semantic(&self) -> bool {
        self.kind != RuntimeEventKind::UsageTokenDelta
    }
    pub fn validate_batch(events: &[Self]) -> Result<(), ExecutionError> {
        let mut expected = 1u64;
        let mut run = None;
        for event in events {
            if event.sequence != expected || event.schema_version != RUNTIME_EVENT_SCHEMA_VERSION {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent));
            }
            if let Some(id) = run {
                if id != event.run_id {
                    return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent));
                }
            } else {
                run = Some(event.run_id)
            }
            expected = expected
                .checked_add(1)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?;
        }
        Ok(())
    }
}
impl<'de> Deserialize<'de> for RuntimeEvent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = RuntimeEventWire::deserialize(d)?;
        if w.schema_version != RUNTIME_EVENT_SCHEMA_VERSION {
            return Err(serde::de::Error::custom(ExecutionError::new(
                ExecutionErrorCode::InvalidEvent,
            )));
        }
        Self::with_payload(w.sequence, w.run_id, w.kind, w.payload)
            .map_err(serde::de::Error::custom)
    }
}
