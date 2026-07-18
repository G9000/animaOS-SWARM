//! Portable durable execution values. Hosts persist these values atomically in their own store.

mod checkpoint;
mod event;
mod memory_store;
mod state;
mod store;

pub use checkpoint::{
    AttemptRecordState, CheckpointCursor, CheckpointV1, CheckpointV1Builder,
    CompletedInvocationRecord, DefinitionPin, InvocationAttemptRecord, ManifestPin,
    OpaqueReference, PendingApprovalRecord, RecoveryPauseReason, RecoveryPauseRecord,
    RecoveryRecord, UncertainInvocationRecord,
};
pub use event::{LiveRuntimeEvent, RuntimeEvent, RuntimeEventKind, SafeEventPayload};
pub use memory_store::InMemoryExecutionStore;
pub use state::{
    ApprovalResumeBinding, ApprovalResumeClaim, ApprovalResumeOutcome, Attempt, Budget,
    BudgetDecision, CommandOutcome, CommandReceipt, ExecutionError, ExecutionErrorCode,
    ExecutionLease, RecoveryTerminalOutcome, RecoveryTerminalResolution, Run, RunPauseReason,
    RunState, RuntimeCommand, RuntimeCommandKind, Session, SessionConcurrencyPolicy, Step,
    StepKind, Usage,
};
pub use store::{
    ApprovalGrantMutation, CreateRun, DurableResultMutation, ExecutionCommit,
    ExecutionCommitOutcome, ExecutionStore, ExecutionStoreError, ExecutionStoreErrorCode,
    StoredRun,
};
