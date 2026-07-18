use anima_core::{
    assert_execution_store_conformance, AuthoritativeGrantChange, AuthoritativeGrantState,
    CheckpointV1, CreateRun, DurableCapabilityResult, EventReplayPage, ExecutionCommit,
    ExecutionCommitOutcome, ExecutionLease, ExecutionStep, ExecutionStore, ExecutionStoreError,
    ExecutionStoreFactory, GrantAuthorityKey, InMemoryExecutionStore, InvocationAttemptRecord,
    ManualExecutionClock, StoreHistoryPage, StoreReadPage, StoredRun,
};
use std::sync::Arc;
use uuid::Uuid;

/// A separately compiled adapter demonstrates that the complete scoped port is implementable
/// without access to crate-private authority material.
struct ExternalStyleAdapter {
    durable: InMemoryExecutionStore,
}

#[async_trait::async_trait]
impl ExecutionStore for ExternalStyleAdapter {
    async fn apply_authoritative_grant(
        &self,
        owner_id: Uuid,
        change: AuthoritativeGrantChange,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError> {
        self.durable
            .apply_authoritative_grant(owner_id, change)
            .await
    }

    async fn load_authoritative_grant(
        &self,
        owner_id: Uuid,
        authority_key: &GrantAuthorityKey,
    ) -> Result<Option<AuthoritativeGrantState>, ExecutionStoreError> {
        self.durable
            .load_authoritative_grant(owner_id, authority_key)
            .await
    }

    async fn create_run(
        &self,
        owner_id: Uuid,
        request: CreateRun,
    ) -> Result<StoredRun, ExecutionStoreError> {
        self.durable.create_run(owner_id, request).await
    }

    async fn acquire_lease(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.durable
            .acquire_lease(owner_id, run_id, expected_run_version, duration_ms)
            .await
    }

    async fn renew_lease(
        &self,
        owner_id: Uuid,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.durable.renew_lease(owner_id, lease, duration_ms).await
    }

    async fn commit_execution(
        &self,
        owner_id: Uuid,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError> {
        self.durable.commit_execution(owner_id, commit).await
    }

    async fn load_run(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<StoredRun>, ExecutionStoreError> {
        self.durable.load_run(owner_id, run_id).await
    }

    async fn load_checkpoint(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<(u64, CheckpointV1)>, ExecutionStoreError> {
        self.durable.load_checkpoint(owner_id, run_id).await
    }

    async fn load_steps_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<ExecutionStep>, ExecutionStoreError> {
        self.durable.load_steps_page(owner_id, run_id, page).await
    }

    async fn load_attempts_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<StoreHistoryPage<InvocationAttemptRecord>, ExecutionStoreError> {
        self.durable
            .load_attempts_page(owner_id, run_id, page)
            .await
    }

    async fn load_durable_result(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError> {
        self.durable
            .load_durable_result(owner_id, run_id, logical_invocation_id)
            .await
    }

    async fn replay_events(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<EventReplayPage, ExecutionStoreError> {
        self.durable.replay_events(owner_id, run_id, page).await
    }
}

#[derive(Default)]
struct ExternalStyleAdapterFactory {
    clock: ManualExecutionClock,
}

#[async_trait::async_trait]
impl ExecutionStoreFactory for ExternalStyleAdapterFactory {
    type Store = ExternalStyleAdapter;

    async fn create_execution_store(&self) -> Result<Self::Store, ExecutionStoreError> {
        Ok(ExternalStyleAdapter {
            durable: InMemoryExecutionStore::with_clock(Arc::new(self.clock.clone())),
        })
    }

    fn advance_clock(&self, duration_ms: u64) -> Result<(), ExecutionStoreError> {
        self.clock.advance_ms(duration_ms)
    }
}

#[tokio::test]
async fn external_adapter_runs_the_full_scoped_public_conformance_suite() {
    assert_execution_store_conformance(&ExternalStyleAdapterFactory::default())
        .await
        .unwrap();
}
