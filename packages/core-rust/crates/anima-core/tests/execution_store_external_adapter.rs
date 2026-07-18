use anima_core::{
    assert_execution_store_conformance, AuthoritativeGrantChange, AuthoritativeGrantState,
    CheckpointV1, CreateRun, DurableCapabilityResult, EventReplayPage, ExecutionCommit,
    ExecutionCommitOutcome, ExecutionLease, ExecutionStep, ExecutionStore, ExecutionStoreError,
    ExecutionStoreFactory, GrantAuthorityKey, InMemoryExecutionStore, InvocationAttemptRecord,
    ManualExecutionClock, StoreHistoryPage, StoreReadPage, StoredRun,
};
use std::sync::Arc;
use uuid::Uuid;

/// Compile-boundary proof that a downstream crate can implement the complete public store port.
///
/// This wrapper deliberately delegates semantics to the in-memory reference adapter. It is not an
/// independent persistence implementation; the SQLite adapter belongs to a later task.
struct PublicApiDelegatingAdapter {
    reference: InMemoryExecutionStore,
}

#[async_trait::async_trait]
impl ExecutionStore for PublicApiDelegatingAdapter {
    async fn apply_authoritative_grant(
        &self,
        owner_id: Uuid,
        change: AuthoritativeGrantChange,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError> {
        self.reference
            .apply_authoritative_grant(owner_id, change)
            .await
    }

    async fn load_authoritative_grant(
        &self,
        owner_id: Uuid,
        authority_key: &GrantAuthorityKey,
    ) -> Result<Option<AuthoritativeGrantState>, ExecutionStoreError> {
        self.reference
            .load_authoritative_grant(owner_id, authority_key)
            .await
    }

    async fn create_run(
        &self,
        owner_id: Uuid,
        request: CreateRun,
    ) -> Result<StoredRun, ExecutionStoreError> {
        self.reference.create_run(owner_id, request).await
    }

    async fn acquire_lease(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.reference
            .acquire_lease(owner_id, run_id, expected_run_version, duration_ms)
            .await
    }

    async fn renew_lease(
        &self,
        owner_id: Uuid,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.reference
            .renew_lease(owner_id, lease, duration_ms)
            .await
    }

    async fn commit_execution(
        &self,
        owner_id: Uuid,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError> {
        self.reference.commit_execution(owner_id, commit).await
    }

    async fn load_run(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<StoredRun>, ExecutionStoreError> {
        self.reference.load_run(owner_id, run_id).await
    }

    async fn load_checkpoint(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<(u64, CheckpointV1)>, ExecutionStoreError> {
        self.reference.load_checkpoint(owner_id, run_id).await
    }

    async fn load_steps_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<ExecutionStep>, ExecutionStoreError> {
        self.reference.load_steps_page(owner_id, run_id, page).await
    }

    async fn load_attempts_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<InvocationAttemptRecord>, ExecutionStoreError> {
        self.reference
            .load_attempts_page(owner_id, run_id, page)
            .await
    }

    async fn load_durable_result(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError> {
        self.reference
            .load_durable_result(owner_id, run_id, logical_invocation_id)
            .await
    }

    async fn replay_events(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<EventReplayPage, ExecutionStoreError> {
        self.reference.replay_events(owner_id, run_id, page).await
    }
}

#[derive(Default)]
struct PublicApiDelegatingAdapterFactory {
    clock: ManualExecutionClock,
}

#[async_trait::async_trait]
impl ExecutionStoreFactory for PublicApiDelegatingAdapterFactory {
    type Store = PublicApiDelegatingAdapter;

    async fn create_execution_store(&self) -> Result<Self::Store, ExecutionStoreError> {
        Ok(PublicApiDelegatingAdapter {
            reference: InMemoryExecutionStore::with_clock(Arc::new(self.clock.clone())),
        })
    }

    fn advance_clock(&self, duration_ms: u64) -> Result<(), ExecutionStoreError> {
        self.clock.advance_ms(duration_ms)
    }
}

#[tokio::test]
async fn public_api_delegating_adapter_runs_the_full_scoped_conformance_suite() {
    assert_execution_store_conformance(&PublicApiDelegatingAdapterFactory::default())
        .await
        .unwrap();
}
