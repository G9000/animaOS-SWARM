use std::collections::BTreeMap;

use anima_core::{
    AuthoritativeGrantChange, AuthoritativeGrantChangeKind, AuthoritativeGrantState, CheckpointV1,
    CreateRun, DurableCapabilityResult, EventReplayPage, ExecutionCommit, ExecutionCommitOutcome,
    ExecutionLease, ExecutionStep, ExecutionStore, ExecutionStoreError, ExecutionStoreErrorCode,
    InMemoryExecutionStore, InvocationAttemptRecord, StoreReadPage, StoredRun,
};
use futures::lock::Mutex;
use uuid::Uuid;

#[derive(Default)]
struct ExternalStyleAdapter {
    runs: InMemoryExecutionStore,
    grants: Mutex<BTreeMap<String, AuthoritativeGrantState>>,
}

#[async_trait::async_trait]
impl ExecutionStore for ExternalStyleAdapter {
    async fn apply_authoritative_grant(
        &self,
        change: AuthoritativeGrantChange,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError> {
        let mut grants = self.grants.lock().await;
        let next = match change.kind() {
            AuthoritativeGrantChangeKind::Create(state) => {
                if grants.contains_key(state.grant_id()) {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::VersionConflict,
                    ));
                }
                state.clone()
            }
            AuthoritativeGrantChangeKind::Update {
                expected_revision,
                state,
            } => {
                let current = grants.get(state.grant_id()).ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::VersionConflict)
                })?;
                if current.revision() != *expected_revision {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::VersionConflict,
                    ));
                }
                state.clone()
            }
            AuthoritativeGrantChangeKind::Revoke {
                grant_id,
                expected_revision,
            } => {
                let current = grants.get(grant_id).ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::VersionConflict)
                })?;
                if current.revision() != *expected_revision {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::VersionConflict,
                    ));
                }
                AuthoritativeGrantState::revoked(
                    grant_id,
                    current.revision(),
                    current.remaining_uses(),
                )?
            }
        };
        grants.insert(next.grant_id().to_owned(), next.clone());
        Ok(next)
    }

    async fn load_authoritative_grant(
        &self,
        grant_id: &str,
    ) -> Result<Option<AuthoritativeGrantState>, ExecutionStoreError> {
        Ok(self.grants.lock().await.get(grant_id).cloned())
    }

    async fn create_run(&self, request: CreateRun) -> Result<StoredRun, ExecutionStoreError> {
        self.runs.create_run(request).await
    }

    async fn acquire_lease(
        &self,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.runs
            .acquire_lease(run_id, expected_run_version, duration_ms)
            .await
    }

    async fn renew_lease(
        &self,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        self.runs.renew_lease(lease, duration_ms).await
    }

    async fn commit_execution(
        &self,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError> {
        self.runs.commit_execution(commit).await
    }

    async fn load_run(&self, run_id: Uuid) -> Result<Option<StoredRun>, ExecutionStoreError> {
        self.runs.load_run(run_id).await
    }

    async fn load_checkpoint(
        &self,
        run_id: Uuid,
    ) -> Result<Option<(u64, CheckpointV1)>, ExecutionStoreError> {
        self.runs.load_checkpoint(run_id).await
    }

    async fn load_steps_page(
        &self,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<Vec<ExecutionStep>, ExecutionStoreError> {
        self.runs.load_steps_page(run_id, page).await
    }

    async fn load_attempts_page(
        &self,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<Vec<InvocationAttemptRecord>, ExecutionStoreError> {
        self.runs.load_attempts_page(run_id, page).await
    }

    async fn load_durable_result(
        &self,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError> {
        self.runs
            .load_durable_result(run_id, logical_invocation_id)
            .await
    }

    async fn replay_events(
        &self,
        run_id: Uuid,
        page: StoreReadPage,
    ) -> Result<EventReplayPage, ExecutionStoreError> {
        self.runs.replay_events(run_id, page).await
    }
}

#[tokio::test]
async fn external_adapter_can_implement_the_complete_public_grant_port() {
    let adapter = ExternalStyleAdapter::default();
    let created = adapter
        .apply_authoritative_grant(AuthoritativeGrantChange::create(
            AuthoritativeGrantState::active("external-adapter-grant", 1, Some(2)).unwrap(),
        ))
        .await
        .unwrap();
    let upgraded = adapter
        .apply_authoritative_grant(
            AuthoritativeGrantChange::update(
                created.revision(),
                AuthoritativeGrantState::active("external-adapter-grant", 2, Some(1)).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let revoked = adapter
        .apply_authoritative_grant(
            AuthoritativeGrantChange::revoke("external-adapter-grant", upgraded.revision())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        revoked.status(),
        anima_core::AuthoritativeGrantStatus::Revoked
    );
    assert_eq!(
        adapter
            .load_authoritative_grant("external-adapter-grant")
            .await
            .unwrap(),
        Some(revoked)
    );
}
