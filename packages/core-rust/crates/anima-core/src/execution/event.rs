use super::state::{valid_uuid, ExecutionError, ExecutionErrorCode};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

pub const RUNTIME_EVENT_SCHEMA_VERSION: u32 = 1;
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
    StepStarted,
    StepRetried,
    StepCompleted,
    StepFailed,
    CapabilityProposed,
    CapabilityApproved,
    CapabilityDenied,
    InvocationDispatchPrepared,
    CapabilityStarted,
    CapabilityCompleted,
    CapabilityFailed,
    PolicyEvaluated,
    ApprovalRequested,
    ApprovalResolved,
    MemoryRead,
    MemoryProposed,
    MemoryWritten,
    MemorySuperseded,
    MemoryForgotten,
    CheckpointSaved,
    ArtifactRecorded,
    ArtifactProposed,
    ArtifactCreated,
    ArtifactUpdated,
    BudgetUpdated,
    BudgetExtensionRequested,
    BudgetExhausted,
    UsageRecorded,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeEventPayload {
    None,
    Reference {
        reference: Uuid,
    },
    Error {
        code: ExecutionErrorCode,
        reference: Option<Uuid>,
    },
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PayloadRef<'a> {
    None,
    Reference {
        reference: &'a Uuid,
    },
    Error {
        code: ExecutionErrorCode,
        reference: Option<&'a Uuid>,
    },
}
impl Serialize for SafeEventPayload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        match self {
            Self::None => PayloadRef::None,
            Self::Reference { reference } => PayloadRef::Reference { reference },
            Self::Error { code, reference } => PayloadRef::Error {
                code: *code,
                reference: reference.as_ref(),
            },
        }
        .serialize(serializer)
    }
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PayloadWire {
    None,
    Reference {
        reference: Uuid,
    },
    Error {
        code: ExecutionErrorCode,
        reference: Option<Uuid>,
    },
}
impl SafeEventPayload {
    fn validate(&self) -> Result<(), ExecutionError> {
        match self {
            Self::Reference { reference } => valid_uuid(*reference),
            Self::Error {
                reference: Some(reference),
                ..
            } => valid_uuid(*reference),
            _ => Ok(()),
        }
    }
}
impl<'de> Deserialize<'de> for SafeEventPayload {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let p = match PayloadWire::deserialize(d)? {
            PayloadWire::None => Self::None,
            PayloadWire::Reference { reference } => Self::Reference { reference },
            PayloadWire::Error { code, reference } => Self::Error { code, reference },
        };
        p.validate().map_err(serde::de::Error::custom)?;
        Ok(p)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeEvent {
    schema_version: u32,
    event_id: Uuid,
    owner_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
    timestamp_ms: u64,
    sequence: u64,
    kind: RuntimeEventKind,
    payload: SafeEventPayload,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    schema_version: u32,
    event_id: Uuid,
    owner_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
    timestamp_ms: u64,
    sequence: u64,
    kind: RuntimeEventKind,
    payload: SafeEventPayload,
}
impl RuntimeEvent {
    pub fn new(
        event_id: Uuid,
        owner_id: Uuid,
        session_id: Uuid,
        run_id: Uuid,
        timestamp_ms: u64,
        sequence: u64,
        kind: RuntimeEventKind,
    ) -> Result<Self, ExecutionError> {
        Self::with_payload(
            event_id,
            owner_id,
            session_id,
            run_id,
            timestamp_ms,
            sequence,
            kind,
            SafeEventPayload::None,
        )
    }
    pub fn with_payload(
        event_id: Uuid,
        owner_id: Uuid,
        session_id: Uuid,
        run_id: Uuid,
        timestamp_ms: u64,
        sequence: u64,
        kind: RuntimeEventKind,
        payload: SafeEventPayload,
    ) -> Result<Self, ExecutionError> {
        for id in [event_id, owner_id, session_id, run_id] {
            valid_uuid(id)?;
        }
        if timestamp_ms == 0 || sequence == 0 {
            return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent));
        }
        payload.validate()?;
        Ok(Self {
            schema_version: RUNTIME_EVENT_SCHEMA_VERSION,
            event_id,
            owner_id,
            session_id,
            run_id,
            timestamp_ms,
            sequence,
            kind,
            payload,
        })
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn event_id(&self) -> Uuid {
        self.event_id
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
    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }
    pub fn kind(&self) -> RuntimeEventKind {
        self.kind
    }
    pub fn payload(&self) -> &SafeEventPayload {
        &self.payload
    }
    pub fn validate_batch(start: u64, events: &[Self]) -> Result<(), ExecutionError> {
        let mut expected = start;
        let mut ids = std::collections::BTreeSet::new();
        let mut scope = None;
        let mut prior_timestamp_ms = None;
        for e in events {
            if e.schema_version != RUNTIME_EVENT_SCHEMA_VERSION
                || e.sequence != expected
                || e.timestamp_ms == 0
                || prior_timestamp_ms.is_some_and(|prior| e.timestamp_ms < prior)
                || !ids.insert(e.event_id)
            {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent));
            }
            let s = (e.owner_id, e.session_id, e.run_id);
            if scope.is_some_and(|x| x != s) {
                return Err(ExecutionError::new(ExecutionErrorCode::InvalidEvent));
            }
            scope = Some(s);
            prior_timestamp_ms = Some(e.timestamp_ms);
            expected = expected
                .checked_add(1)
                .ok_or_else(|| ExecutionError::new(ExecutionErrorCode::ArithmeticOverflow))?;
        }
        Ok(())
    }
}
impl<'de> Deserialize<'de> for RuntimeEvent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = Wire::deserialize(d)?;
        if w.schema_version != RUNTIME_EVENT_SCHEMA_VERSION {
            return Err(serde::de::Error::custom(ExecutionError::new(
                ExecutionErrorCode::InvalidEvent,
            )));
        }
        Self::with_payload(
            w.event_id,
            w.owner_id,
            w.session_id,
            w.run_id,
            w.timestamp_ms,
            w.sequence,
            w.kind,
            w.payload,
        )
        .map_err(serde::de::Error::custom)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiveRuntimeEvent {
    run_id: Uuid,
    input_tokens: u64,
    output_tokens: u64,
}
impl LiveRuntimeEvent {
    pub fn new(
        run_id: Uuid,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<Self, ExecutionError> {
        valid_uuid(run_id)?;
        Ok(Self {
            run_id,
            input_tokens,
            output_tokens,
        })
    }
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens
    }
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveRuntimeEventWire {
    run_id: Uuid,
    input_tokens: u64,
    output_tokens: u64,
}
impl<'de> Deserialize<'de> for LiveRuntimeEvent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = LiveRuntimeEventWire::deserialize(d)?;
        Self::new(w.run_id, w.input_tokens, w.output_tokens).map_err(serde::de::Error::custom)
    }
}
