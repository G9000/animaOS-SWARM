use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::lock::Mutex;
use uuid::Uuid;

use super::{
    CreateRun, DurableResultMutation, ExecutionCommit, ExecutionCommitOutcome, ExecutionLease,
    ExecutionStore, ExecutionStoreError, ExecutionStoreErrorCode, RuntimeEvent,
    SessionConcurrencyPolicy, StoredRun,
};
use crate::{CommandReceipt, DurableCapabilityResult, RunState};

#[derive(Clone)]
struct StoredSession {
    version: u64,
    policy: SessionConcurrencyPolicy,
}

#[derive(Clone)]
struct RunAggregate {
    stored: StoredRun,
    lease: Option<ExecutionLease>,
    checkpoint: Option<(u64, super::CheckpointV1)>,
    events: Vec<RuntimeEvent>,
    steps: BTreeMap<String, super::Step>,
    attempts: BTreeMap<(Uuid, u32), super::InvocationAttemptRecord>,
    results: BTreeMap<Uuid, (u32, DurableCapabilityResult)>,
}

#[derive(Clone, Default)]
struct State {
    sessions: BTreeMap<Uuid, StoredSession>,
    runs: BTreeMap<Uuid, RunAggregate>,
    serial_claims: BTreeMap<Uuid, Uuid>,
    receipts: BTreeMap<Uuid, CommandReceipt>,
    outcomes: BTreeMap<Uuid, ExecutionCommitOutcome>,
    grant_consumptions: BTreeSet<(String, u32, Uuid)>,
    grant_use_counts: BTreeMap<(String, u32), u32>,
}

/// In-process reference adapter. It clones no externally visible state until validation succeeds.
#[derive(Default)]
pub struct InMemoryExecutionStore {
    state: Mutex<State>,
}

#[async_trait]
impl ExecutionStore for InMemoryExecutionStore {
    async fn create_run(&self, request: CreateRun) -> Result<StoredRun, ExecutionStoreError> {
        let session = request.session();
        if request.run().session_id() != session.id()
            || request.run().definition_id() != session.definition().id()
            || request.run().definition_version() != session.definition().version()
            || session.concurrency()? != request.concurrency_policy()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }

        let mut state = self.state.lock().await;
        if state.runs.contains_key(&request.run().id()) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            ));
        }

        let session_version = match state.sessions.get(&session.id()) {
            Some(current)
                if current.version == request.expected_session_version()
                    && current.policy == request.concurrency_policy() =>
            {
                current.version.saturating_add(1)
            }
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::VersionConflict,
                ))
            }
            None if request.expected_session_version() == 0 => 1,
            None => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::VersionConflict,
                ))
            }
        };

        if request.concurrency_policy() == SessionConcurrencyPolicy::Serial
            && state.serial_claims.contains_key(&session.id())
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::ActiveRunConflict,
            ));
        }

        state.sessions.insert(
            session.id(),
            StoredSession {
                version: session_version,
                policy: request.concurrency_policy(),
            },
        );
        if request.concurrency_policy() == SessionConcurrencyPolicy::Serial {
            state.serial_claims.insert(session.id(), request.run().id());
        }
        let stored = StoredRun::new(request.run().clone(), 1, session_version)?;
        state.runs.insert(
            request.run().id(),
            RunAggregate {
                stored: stored.clone(),
                lease: None,
                checkpoint: None,
                events: vec![],
                steps: BTreeMap::new(),
                attempts: BTreeMap::new(),
                results: BTreeMap::new(),
            },
        );
        Ok(stored)
    }

    async fn acquire_lease(
        &self,
        run_id: Uuid,
        expected_run_version: u64,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        if duration_ms == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let mut state = self.state.lock().await;
        let aggregate = state
            .runs
            .get_mut(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        if aggregate.stored.run_version() != expected_run_version {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            ));
        }
        if aggregate.stored.run().state().is_terminal() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        if aggregate
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at_ms() > now_ms())
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseConflict,
            ));
        }
        let lease =
            ExecutionLease::new(run_id, Uuid::new_v4(), now_ms().saturating_add(duration_ms))
                .map_err(ExecutionStoreError::from)?;
        aggregate.lease = Some(lease.clone());
        Ok(lease)
    }

    async fn renew_lease(
        &self,
        lease: ExecutionLease,
        duration_ms: u64,
    ) -> Result<ExecutionLease, ExecutionStoreError> {
        if duration_ms == 0 {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let mut state = self.state.lock().await;
        let aggregate = state
            .runs
            .get_mut(&lease.run_id())
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        if aggregate.lease.as_ref() != Some(&lease) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseExpired,
            ));
        }
        if lease.expires_at_ms() <= now_ms() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseExpired,
            ));
        }
        let renewed = ExecutionLease::new(
            lease.run_id(),
            lease.fence(),
            now_ms().saturating_add(duration_ms),
        )
        .map_err(ExecutionStoreError::from)?;
        aggregate.lease = Some(renewed.clone());
        Ok(renewed)
    }

    async fn commit_execution(
        &self,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError> {
        let mut state = self.state.lock().await;
        let run_id = commit.lease().run_id();
        if commit.command().target_run_id() != run_id || commit.target_run().id() != run_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        if let Some(receipt) = state.receipts.get(&commit.command().id()) {
            receipt
                .replay(commit.command())
                .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::CommandConflict))?;
            return state
                .outcomes
                .get(&commit.command().id())
                .cloned()
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound));
        }

        let mut next = state.clone();
        let aggregate = next
            .runs
            .get_mut(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        if aggregate.stored.run_version() != commit.expected_run_version() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            ));
        }
        let checkpoint_version = aggregate
            .checkpoint
            .as_ref()
            .map_or(0, |(version, _)| *version);
        if checkpoint_version != commit.expected_checkpoint_version() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::CheckpointConflict,
            ));
        }
        if aggregate.lease.as_ref() != Some(commit.lease())
            || commit.lease().expires_at_ms() <= now_ms()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseExpired,
            ));
        }
        if !valid_target(aggregate.stored.run(), commit.target_run()) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let first_sequence = aggregate.events.len() as u64 + 1;
        RuntimeEvent::validate_batch(first_sequence, commit.events())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::EventConflict))?;
        if commit.events().iter().any(|event| {
            event.run_id() != run_id || event.session_id() != aggregate.stored.run().session_id()
        }) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::EventConflict,
            ));
        }
        if let Some(checkpoint) = commit.checkpoint() {
            if checkpoint.run_id() != run_id
                || checkpoint.session_id() != aggregate.stored.run().session_id()
                || checkpoint.state() != commit.target_run().state()
                || checkpoint.last_durable_event_sequence()
                    != first_sequence
                        .saturating_add(commit.events().len() as u64)
                        .saturating_sub(1)
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::CheckpointConflict,
                ));
            }
            checkpoint.validate().map_err(|_| {
                ExecutionStoreError::new(ExecutionStoreErrorCode::CheckpointConflict)
            })?;
        }
        if let Some(approval) = commit.approval() {
            let resumed = aggregate
                .stored
                .run()
                .apply_resume_command(commit.command(), Some(approval.claim()), None)
                .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest))?;
            if resumed.run() != commit.target_run()
                || resumed.grant_consumption() != approval.grant_consumption()
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
            if let Some(consumption) = approval.grant_consumption() {
                let key = (
                    consumption.grant_id.clone(),
                    consumption.grant_revision,
                    consumption.logical_invocation_id,
                );
                if !next.grant_consumptions.insert(key) {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::GrantAlreadyConsumed,
                    ));
                }
                let grant_key = (consumption.grant_id.clone(), consumption.grant_revision);
                let used = next.grant_use_counts.get(&grant_key).copied().unwrap_or(0);
                if approval.remaining_uses().is_some_and(|limit| used >= limit) {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::GrantAlreadyConsumed,
                    ));
                }
                next.grant_use_counts
                    .insert(grant_key, used.saturating_add(1));
            }
        }
        for step in commit.steps() {
            if step.run_id() != run_id {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
            aggregate
                .steps
                .insert(step.logical_step_id().into(), step.clone());
        }
        for attempt in commit.attempts() {
            if attempt.invocation().run_id() != run_id {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
            aggregate.attempts.insert(
                (attempt.invocation().id(), attempt.attempt_number()),
                attempt.clone(),
            );
        }
        for mutation in commit.results() {
            insert_result(aggregate, mutation)?;
        }
        aggregate.events.extend_from_slice(commit.events());
        if let Some(checkpoint) = commit.checkpoint() {
            aggregate.checkpoint = Some((checkpoint_version.saturating_add(1), checkpoint.clone()));
        }
        let mut stored = StoredRun::new(
            commit.target_run().clone(),
            aggregate.stored.run_version().saturating_add(1),
            aggregate.stored.session_version(),
        )?;
        aggregate.stored = stored.clone();
        if commit.target_run().state().is_terminal() {
            aggregate.lease = None;
            if next.serial_claims.get(&commit.target_run().session_id()) == Some(&run_id) {
                next.serial_claims.remove(&commit.target_run().session_id());
                let session = next
                    .sessions
                    .get_mut(&commit.target_run().session_id())
                    .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
                session.version = session.version.saturating_add(1);
                stored = StoredRun::new(
                    commit.target_run().clone(),
                    stored.run_version(),
                    session.version,
                )?;
                aggregate.stored = stored.clone();
            }
        }
        let receipt =
            CommandReceipt::accepted(commit.command()).map_err(ExecutionStoreError::from)?;
        next.receipts.insert(commit.command().id(), receipt.clone());
        next.outcomes.insert(
            commit.command().id(),
            ExecutionCommitOutcome::new(stored.clone(), receipt.clone()),
        );
        *state = next;
        Ok(ExecutionCommitOutcome::new(stored, receipt))
    }

    async fn load_run(&self, run_id: Uuid) -> Result<Option<StoredRun>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .runs
            .get(&run_id)
            .map(|aggregate| aggregate.stored.clone()))
    }

    async fn load_checkpoint(
        &self,
        run_id: Uuid,
    ) -> Result<Option<(u64, super::CheckpointV1)>, ExecutionStoreError> {
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .checkpoint
            .clone())
    }

    async fn load_steps(&self, run_id: Uuid) -> Result<Vec<super::Step>, ExecutionStoreError> {
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .steps
            .values()
            .cloned()
            .collect())
    }

    async fn load_attempts(
        &self,
        run_id: Uuid,
    ) -> Result<Vec<super::InvocationAttemptRecord>, ExecutionStoreError> {
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .attempts
            .values()
            .cloned()
            .collect())
    }

    async fn load_durable_result(
        &self,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError> {
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .results
            .get(&logical_invocation_id)
            .map(|(_, result)| result.clone()))
    }

    async fn replay_events(
        &self,
        run_id: Uuid,
        after_sequence: u64,
    ) -> Result<Vec<RuntimeEvent>, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        Ok(aggregate
            .events
            .iter()
            .filter(|event| event.sequence() > after_sequence)
            .cloned()
            .collect())
    }
}

fn insert_result(
    aggregate: &mut RunAggregate,
    mutation: &DurableResultMutation,
) -> Result<(), ExecutionStoreError> {
    let completed = mutation.completed();
    if completed.result_ref().value() != mutation.result().result_ref().handle() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::ResultConflict,
        ));
    }
    let key = completed.invocation().id();
    let value = (completed.attempt_number(), mutation.result().clone());
    match aggregate.results.get(&key) {
        Some(existing) if existing == &value => Ok(()),
        Some(_) => Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::ResultConflict,
        )),
        None => {
            aggregate.results.insert(key, value);
            Ok(())
        }
    }
}

fn valid_target(current: &super::Run, target: &super::Run) -> bool {
    if current.id() != target.id()
        || current.session_id() != target.session_id()
        || current.definition_id() != target.definition_id()
        || current.definition_version() != target.definition_version()
        || current.state().is_terminal()
    {
        return false;
    }
    matches!(
        (current.state(), target.state()),
        (RunState::Queued, RunState::Queued | RunState::Running)
            | (
                RunState::Running,
                RunState::Running
                    | RunState::WaitingForApproval
                    | RunState::Paused
                    | RunState::Completed
                    | RunState::Failed
                    | RunState::Cancelled
            )
            | (
                RunState::WaitingForApproval,
                RunState::WaitingForApproval | RunState::Running | RunState::Cancelled
            )
            | (
                RunState::Paused,
                RunState::Paused | RunState::Running | RunState::Failed | RunState::Cancelled
            )
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
