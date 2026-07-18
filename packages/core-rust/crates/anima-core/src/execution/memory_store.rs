use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::lock::Mutex;
use uuid::Uuid;

use super::{
    store::AuthoritativeGrantChangeKind, AuthoritativeGrantChange, AuthoritativeGrantState,
    AuthoritativeGrantStatus, CreateRun, DurableResultMutation, ExecutionCommit,
    ExecutionCommitOutcome, ExecutionLease, ExecutionStore, ExecutionStoreError,
    ExecutionStoreErrorCode, RuntimeEvent, SessionConcurrencyPolicy, StoredRun,
};
use crate::{CommandReceipt, DurableCapabilityResult, RunState};

#[derive(Clone)]
struct StoredSession {
    version: u64,
    owner_id: Uuid,
    definition: super::DefinitionPin,
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
    results: BTreeMap<Uuid, StoredDurableResult>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredDurableResult {
    completed: super::CompletedInvocationRecord,
    result: DurableCapabilityResult,
}

#[derive(Clone, Default)]
struct State {
    sessions: BTreeMap<Uuid, StoredSession>,
    runs: BTreeMap<Uuid, RunAggregate>,
    serial_claims: BTreeMap<Uuid, Uuid>,
    receipts: BTreeMap<Uuid, CommandReceipt>,
    outcomes: BTreeMap<Uuid, ExecutionCommitOutcome>,
    authoritative_grants: BTreeMap<String, AuthoritativeGrantState>,
    grant_consumptions: BTreeSet<(String, u32, Uuid)>,
}

/// In-process reference adapter. It clones no externally visible state until validation succeeds.
#[derive(Default)]
pub struct InMemoryExecutionStore {
    state: Mutex<State>,
}

#[async_trait]
impl ExecutionStore for InMemoryExecutionStore {
    async fn apply_authoritative_grant(
        &self,
        change: AuthoritativeGrantChange,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError> {
        let mut state = self.state.lock().await;
        let next = match change.kind() {
            AuthoritativeGrantChangeKind::Create(next) => {
                if state.authoritative_grants.contains_key(next.grant_id()) {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::VersionConflict,
                    ));
                }
                next.clone()
            }
            AuthoritativeGrantChangeKind::Update {
                expected_revision,
                state: next,
            } => {
                let current = state
                    .authoritative_grants
                    .get(next.grant_id())
                    .ok_or_else(|| {
                        ExecutionStoreError::new(ExecutionStoreErrorCode::VersionConflict)
                    })?;
                if current.revision() != *expected_revision || next.revision() <= current.revision()
                {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::VersionConflict,
                    ));
                }
                next.clone()
            }
            AuthoritativeGrantChangeKind::Revoke {
                grant_id,
                expected_revision,
            } => {
                let current = state.authoritative_grants.get(grant_id).ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::VersionConflict)
                })?;
                if current.revision() != *expected_revision {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::VersionConflict,
                    ));
                }
                current.revoked()
            }
        };
        state
            .authoritative_grants
            .insert(next.grant_id().to_owned(), next.clone());
        Ok(next)
    }

    async fn load_authoritative_grant(
        &self,
        grant_id: &str,
    ) -> Result<Option<AuthoritativeGrantState>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .authoritative_grants
            .get(grant_id)
            .cloned())
    }

    async fn create_run(&self, request: CreateRun) -> Result<StoredRun, ExecutionStoreError> {
        let session = request.session();
        if request.owner_id().is_nil()
            || request.run().state().is_terminal()
            || request.run().session_id() != session.id()
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
                    && current.owner_id == request.owner_id()
                    && current.definition == *session.definition()
                    && current.policy == request.concurrency_policy() =>
            {
                current.version.checked_add(1).ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
                })?
            }
            Some(current)
                if current.owner_id != request.owner_id()
                    || current.definition != *session.definition() =>
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ))
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
                owner_id: request.owner_id(),
                definition: session.definition().clone(),
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
        let expires_at_ms = now_ms()
            .checked_add(duration_ms)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let lease = ExecutionLease::new(run_id, Uuid::new_v4(), expires_at_ms)
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
        let expires_at_ms = now_ms()
            .checked_add(duration_ms)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let renewed = ExecutionLease::new(lease.run_id(), lease.fence(), expires_at_ms)
            .map_err(ExecutionStoreError::from)?;
        aggregate.lease = Some(renewed.clone());
        Ok(renewed)
    }

    async fn commit_execution(
        &self,
        commit: ExecutionCommit,
    ) -> Result<ExecutionCommitOutcome, ExecutionStoreError> {
        commit.validate_bounds()?;
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

        let mut aggregate = state
            .runs
            .get(&run_id)
            .cloned()
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
        if !valid_command_transition(
            aggregate.stored.run(),
            commit.target_run(),
            commit.command(),
            commit.approval(),
        ) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let first_sequence = u64::try_from(aggregate.events.len())
            .ok()
            .and_then(|sequence| sequence.checked_add(1))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let event_count = u64::try_from(commit.events().len())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let last_sequence = first_sequence
            .checked_add(event_count)
            .and_then(|sequence| sequence.checked_sub(1))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        RuntimeEvent::validate_batch(first_sequence, commit.events())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::EventConflict))?;
        let owner_id = state
            .sessions
            .get(&aggregate.stored.run().session_id())
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .owner_id;
        if commit.events().iter().any(|event| {
            event.owner_id() != owner_id
                || event.run_id() != run_id
                || event.session_id() != aggregate.stored.run().session_id()
        }) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::EventConflict,
            ));
        }
        if let Some(checkpoint) = commit.checkpoint() {
            if checkpoint.run_id() != run_id
                || checkpoint.session_id() != aggregate.stored.run().session_id()
                || checkpoint.definition().id() != aggregate.stored.run().definition_id()
                || checkpoint.definition().version() != aggregate.stored.run().definition_version()
                || checkpoint.state() != commit.target_run().state()
                || checkpoint.last_durable_event_sequence() != last_sequence
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::CheckpointConflict,
                ));
            }
            checkpoint.validate().map_err(|_| {
                ExecutionStoreError::new(ExecutionStoreErrorCode::CheckpointConflict)
            })?;
        }
        let mut authoritative_grant_update = None;
        let mut grant_consumption_key = None;
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
            match (
                approval.grant_id(),
                approval.grant_revision(),
                approval.grant_remaining_uses(),
            ) {
                (Some(grant_id), Some(grant_revision), claimed_remaining) => {
                    let mut authoritative = state
                        .authoritative_grants
                        .get(grant_id)
                        .cloned()
                        .ok_or_else(|| {
                            ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                        })?;
                    if authoritative.status() != AuthoritativeGrantStatus::Active
                        || authoritative.revision() != grant_revision
                        || authoritative.remaining_uses() != claimed_remaining
                    {
                        return Err(ExecutionStoreError::new(
                            ExecutionStoreErrorCode::GrantConflict,
                        ));
                    }
                    match (approval.grant_consumption(), claimed_remaining) {
                        (Some(consumption), Some(remaining)) => {
                            let snapshot = approval
                                .claim()
                                .grant_consumption_snapshot()
                                .ok_or_else(|| {
                                    ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                                })?;
                            if snapshot.remaining_uses() != remaining
                                || consumption.grant_id != grant_id
                                || consumption.grant_revision != grant_revision
                            {
                                return Err(ExecutionStoreError::new(
                                    ExecutionStoreErrorCode::GrantConflict,
                                ));
                            }
                            let key = (
                                consumption.grant_id.clone(),
                                consumption.grant_revision,
                                consumption.logical_invocation_id,
                            );
                            if state.grant_consumptions.contains(&key) {
                                return Err(ExecutionStoreError::new(
                                    ExecutionStoreErrorCode::GrantAlreadyConsumed,
                                ));
                            }
                            authoritative.consume_one()?;
                            grant_consumption_key = Some(key);
                        }
                        (None, None) => {}
                        _ => {
                            return Err(ExecutionStoreError::new(
                                ExecutionStoreErrorCode::GrantConflict,
                            ))
                        }
                    }
                    authoritative_grant_update = Some(authoritative);
                }
                (None, None, None) if approval.grant_consumption().is_none() => {}
                _ => {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::GrantConflict,
                    ))
                }
            }
        }
        for step in commit.steps() {
            if step.run_id() != run_id {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
            match aggregate.steps.get(step.logical_step_id()) {
                Some(existing) if existing == step => {}
                Some(_) => {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::HistoryConflict,
                    ))
                }
                None => {
                    aggregate
                        .steps
                        .insert(step.logical_step_id().into(), step.clone());
                }
            }
        }
        for attempt in commit.attempts() {
            if attempt.invocation().run_id() != run_id {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::InvalidRequest,
                ));
            }
            let key = (attempt.invocation().id(), attempt.attempt_number());
            match aggregate.attempts.get(&key) {
                Some(existing) if existing == attempt => {}
                Some(_) => {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::HistoryConflict,
                    ))
                }
                None => {
                    aggregate.attempts.insert(key, attempt.clone());
                }
            }
        }
        for mutation in commit.results() {
            insert_result(&mut aggregate, mutation)?;
        }
        if let Some(checkpoint) = commit.checkpoint() {
            let attempts: Vec<_> = aggregate.attempts.values().cloned().collect();
            let completed: Vec<_> = aggregate
                .results
                .values()
                .map(|stored| stored.completed.clone())
                .collect();
            if checkpoint.attempts() != attempts
                || checkpoint.completed_invocations() != completed
                || checkpoint
                    .cursor()
                    .is_some_and(|cursor| !aggregate.steps.contains_key(cursor.logical_step_id()))
            {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::CheckpointConflict,
                ));
            }
        }
        aggregate.events.extend_from_slice(commit.events());
        if let Some(checkpoint) = commit.checkpoint() {
            let next_checkpoint_version = checkpoint_version.checked_add(1).ok_or_else(|| {
                ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
            })?;
            aggregate.checkpoint = Some((next_checkpoint_version, checkpoint.clone()));
        }
        let mut stored = StoredRun::new(
            commit.target_run().clone(),
            aggregate
                .stored
                .run_version()
                .checked_add(1)
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
                })?,
            aggregate.stored.session_version(),
        )?;
        aggregate.stored = stored.clone();
        let mut session_update = None;
        let mut release_serial_claim = false;
        if commit.target_run().state().is_terminal() {
            aggregate.lease = None;
            if state.serial_claims.get(&commit.target_run().session_id()) == Some(&run_id) {
                let mut session = state
                    .sessions
                    .get(&commit.target_run().session_id())
                    .cloned()
                    .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
                session.version = session.version.checked_add(1).ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
                })?;
                stored = StoredRun::new(
                    commit.target_run().clone(),
                    stored.run_version(),
                    session.version,
                )?;
                aggregate.stored = stored.clone();
                session_update = Some(session);
                release_serial_claim = true;
            }
        }
        let receipt =
            CommandReceipt::accepted(commit.command()).map_err(ExecutionStoreError::from)?;
        if let Some(authoritative) = authoritative_grant_update {
            state
                .authoritative_grants
                .insert(authoritative.grant_id().to_owned(), authoritative);
        }
        if let Some(key) = grant_consumption_key {
            state.grant_consumptions.insert(key);
        }
        if let Some(session) = session_update {
            state
                .sessions
                .insert(commit.target_run().session_id(), session);
        }
        if release_serial_claim {
            state
                .serial_claims
                .remove(&commit.target_run().session_id());
        }
        state.runs.insert(run_id, aggregate);
        state
            .receipts
            .insert(commit.command().id(), receipt.clone());
        state.outcomes.insert(
            commit.command().id(),
            ExecutionCommitOutcome::new(stored.clone(), receipt.clone()),
        );
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

    async fn load_steps_page(
        &self,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<Vec<super::Step>, ExecutionStoreError> {
        let offset = usize::try_from(page.offset())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let limit = usize::try_from(page.limit())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .steps
            .values()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn load_attempts_page(
        &self,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<Vec<super::InvocationAttemptRecord>, ExecutionStoreError> {
        let offset = usize::try_from(page.offset())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let limit = usize::try_from(page.limit())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&run_id)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .attempts
            .values()
            .skip(offset)
            .take(limit)
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
            .map(|stored| stored.result.clone()))
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
    if completed.invocation().run_id() != aggregate.stored.run().id()
        || completed.result_ref().value() != mutation.result().result_ref().handle()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::LineageConflict,
        ));
    }
    let key = completed.invocation().id();
    let attempt = aggregate
        .attempts
        .get(&(key, completed.attempt_number()))
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::LineageConflict))?;
    if attempt.invocation() != completed.invocation()
        || attempt.state() != super::AttemptRecordState::Completed
        || attempt.manifest() != completed.manifest()
        || attempt.recovery_mode() != completed.recovery_mode()
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::LineageConflict,
        ));
    }
    let value = StoredDurableResult {
        completed: completed.clone(),
        result: mutation.result().clone(),
    };
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

fn valid_command_transition(
    current: &super::Run,
    target: &super::Run,
    command: &super::RuntimeCommand,
    approval: Option<&super::ApprovalGrantMutation>,
) -> bool {
    if current.id() != target.id()
        || current.session_id() != target.session_id()
        || current.definition_id() != target.definition_id()
        || current.definition_version() != target.definition_version()
        || command.session_id() != current.session_id()
        || command.target_run_id() != current.id()
        || current.state().is_terminal()
    {
        return false;
    }
    match command.kind() {
        super::RuntimeCommandKind::Start => {
            approval.is_none()
                && current.state() == RunState::Queued
                && target.state() == RunState::Running
        }
        super::RuntimeCommandKind::Advance => approval.is_none() && current == target,
        super::RuntimeCommandKind::Pause => {
            approval.is_none()
                && current.state() == RunState::Running
                && target.state() == RunState::Paused
                && target.pause_reason() != Some(super::RunPauseReason::RecoveryRequired)
        }
        super::RuntimeCommandKind::Cancel => {
            approval.is_none() && target.state() == RunState::Cancelled
        }
        super::RuntimeCommandKind::Resume => current
            .apply_resume_command(command, approval.map(|mutation| mutation.claim()), None)
            .is_ok_and(|outcome| outcome.run() == target),
        super::RuntimeCommandKind::Retry => {
            approval.is_none()
                && current == target
                && current.pause_reason() == Some(super::RunPauseReason::RecoveryRequired)
                && current.recovery_pause().is_some_and(|pause| {
                    command.retry_invocation_id() == Some(pause.invocation().id())
                })
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
