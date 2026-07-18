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
pub use memory_store::{InMemoryExecutionStore, ManualExecutionClock};
pub use state::Step as ExecutionStep;
pub use state::{
    ApprovalResumeBinding, ApprovalResumeClaim, ApprovalResumeOutcome, Attempt, Budget,
    BudgetDecision, CommandOutcome, CommandReceipt, ExecutionError, ExecutionErrorCode,
    ExecutionLease, GrantAuthorityBinding, GrantAuthorityKey, GrantConsumptionSnapshot,
    RecoveryTerminalOutcome, RecoveryTerminalResolution, Run, RunPauseReason, RunState,
    RuntimeCommand, RuntimeCommandKind, Session, SessionConcurrencyPolicy, Step, StepKind, Usage,
};
pub use store::{
    assert_execution_store_conformance, ApprovalGrantMutation, AuthoritativeGrantChange,
    AuthoritativeGrantChangeKind, AuthoritativeGrantState, AuthoritativeGrantStatus,
    CheckpointMutation, CreateRun, DispatchGrantMutation, DurableResultMutation, EventReplayPage,
    ExecutionClock, ExecutionCommit, ExecutionCommitOutcome, ExecutionStore, ExecutionStoreError,
    ExecutionStoreErrorCode, ExecutionStoreFactory, StoreHistoryPage, StoreReadCursor,
    StoreReadPage, StoredRun, MAX_COMMIT_ATTEMPTS, MAX_COMMIT_BATCH_ITEMS, MAX_COMMIT_EVENTS,
    MAX_COMMIT_RESULTS, MAX_COMMIT_STEPS, MAX_STORE_READ_CURSOR_BYTES, MAX_STORE_READ_PAGE_SIZE,
};
