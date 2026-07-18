use futures::task::AtomicWaker;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Included};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::lock::Mutex;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AuthoritativeGrantChange, AuthoritativeGrantState, AuthoritativePolicyChange,
    AuthoritativePolicyState, CheckpointMutation, CreateRun, ExecutionClock, ExecutionCommit,
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

struct RunAggregate {
    stored: StoredRun,
    lease: Option<ExecutionLease>,
    checkpoint_version: u64,
    checkpoint: Option<Arc<super::CheckpointV1>>,
    events: Vec<RuntimeEvent>,
    steps: BTreeMap<String, super::Step>,
    step_order: BTreeMap<u64, String>,
    next_step_sequence: u64,
    attempts: BTreeMap<(Uuid, u32), super::InvocationAttemptRecord>,
    attempt_order: BTreeMap<u64, (Uuid, u32)>,
    next_attempt_sequence: u64,
    attempts_fingerprint: super::checkpoint::HistoryFingerprint,
    results: BTreeMap<Uuid, StoredDurableResult>,
    completed_fingerprint: super::checkpoint::HistoryFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredDurableResult {
    completed: super::CompletedInvocationRecord,
    result: DurableCapabilityResult,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<(Uuid, Uuid), StoredSession>,
    runs: BTreeMap<(Uuid, Uuid), RunAggregate>,
    serial_claims: BTreeMap<(Uuid, Uuid), Uuid>,
    receipts: BTreeMap<(Uuid, Uuid), CommandReceipt>,
    outcomes: BTreeMap<(Uuid, Uuid), ExecutionCommitOutcome>,
    authoritative_grants: BTreeMap<(Uuid, String), AuthoritativeGrantState>,
    authoritative_policies: BTreeMap<(Uuid, String, u32), AuthoritativePolicyState>,
    grant_consumptions: BTreeSet<(Uuid, String, u32, Uuid)>,
}

struct CommitPatch {
    stored: StoredRun,
    lease: Option<ExecutionLease>,
    checkpoint_version: u64,
    checkpoint: Option<Arc<super::CheckpointV1>>,
    events: Vec<RuntimeEvent>,
    steps: Vec<(String, u64, super::Step)>,
    next_step_sequence: u64,
    attempts: Vec<((Uuid, u32), u64, super::InvocationAttemptRecord)>,
    next_attempt_sequence: u64,
    attempts_fingerprint: super::checkpoint::HistoryFingerprint,
    results: Vec<(Uuid, StoredDurableResult)>,
    completed_fingerprint: super::checkpoint::HistoryFingerprint,
    authoritative_grant_update: Option<AuthoritativeGrantState>,
    grant_consumption_key: Option<(Uuid, String, u32, Uuid)>,
    session_update: Option<StoredSession>,
    release_serial_claim: bool,
    receipt: CommandReceipt,
    grant_consumption: Option<crate::GrantConsumption>,
}

/// In-process reference adapter. It clones no externally visible state until validation succeeds.
pub struct InMemoryExecutionStore {
    state: Mutex<State>,
    cursor_key: [u8; 16],
    clock: Arc<dyn ExecutionClock>,
}

impl Default for InMemoryExecutionStore {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemExecutionClock))
    }
}

impl InMemoryExecutionStore {
    pub fn with_clock(clock: Arc<dyn ExecutionClock>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            cursor_key: *Uuid::new_v4().as_bytes(),
            clock,
        }
    }
}

#[derive(Debug, Default)]
struct SystemExecutionClock;

#[async_trait]
impl ExecutionClock for SystemExecutionClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct ManualExecutionClock {
    now_ms: Arc<AtomicU64>,
    waker: Arc<AtomicWaker>,
}

impl ManualExecutionClock {
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(now_ms)),
            waker: Arc::new(AtomicWaker::new()),
        }
    }

    pub fn advance_ms(&self, duration_ms: u64) -> Result<(), ExecutionStoreError> {
        let advanced = self
            .now_ms
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |now| {
                now.checked_add(duration_ms)
            })
            .map(|_| ())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow));
        if advanced.is_ok() {
            self.waker.wake();
        }
        advanced
    }
}

impl Default for ManualExecutionClock {
    fn default() -> Self {
        Self::new(1_000_000)
    }
}

#[async_trait]
impl ExecutionClock for ManualExecutionClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    async fn wait_until_ms(&self, deadline_ms: u64) {
        futures::future::poll_fn(|context| {
            if self.now_ms() >= deadline_ms {
                return std::task::Poll::Ready(());
            }
            self.waker.register(context.waker());
            if self.now_ms() >= deadline_ms {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum HistoryCollection {
    Events = 1,
    Steps = 2,
    Attempts = 3,
}

impl InMemoryExecutionStore {
    fn read_window(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        collection: HistoryCollection,
        current_snapshot: u64,
        page: &super::StoreReadPage,
    ) -> Result<(u64, u64), ExecutionStoreError> {
        let Some(cursor) = page.cursor() else {
            return Ok((current_snapshot, 0));
        };
        let (cursor_owner, cursor_run, cursor_collection, snapshot, last) =
            self.decode_cursor(cursor)?;
        if cursor_owner != owner_id
            || cursor_run != run_id
            || cursor_collection != collection
            || snapshot > current_snapshot
            || last >= snapshot
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        Ok((snapshot, last))
    }

    fn write_cursor(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        collection: HistoryCollection,
        snapshot: u64,
        last: u64,
    ) -> Result<super::StoreReadCursor, ExecutionStoreError> {
        let mut payload = Vec::with_capacity(50);
        payload.push(1);
        payload.push(collection as u8);
        payload.extend_from_slice(owner_id.as_bytes());
        payload.extend_from_slice(run_id.as_bytes());
        payload.extend_from_slice(&snapshot.to_be_bytes());
        payload.extend_from_slice(&last.to_be_bytes());
        let mut hasher = Sha256::new();
        hasher.update(&payload);
        hasher.update(self.cursor_key);
        payload.extend_from_slice(&hasher.finalize());
        super::StoreReadCursor::from_opaque(format!("sc1.{}", encode_hex(&payload)))
    }

    fn decode_cursor(
        &self,
        cursor: &super::StoreReadCursor,
    ) -> Result<(Uuid, Uuid, HistoryCollection, u64, u64), ExecutionStoreError> {
        let encoded = cursor
            .as_str()
            .strip_prefix("sc1.")
            .ok_or_else(invalid_cursor)?;
        let bytes = decode_hex(encoded).ok_or_else(invalid_cursor)?;
        if bytes.len() != 82 || bytes[0] != 1 {
            return Err(invalid_cursor());
        }
        let collection = match bytes[1] {
            1 => HistoryCollection::Events,
            2 => HistoryCollection::Steps,
            3 => HistoryCollection::Attempts,
            _ => return Err(invalid_cursor()),
        };
        let payload = &bytes[..50];
        let mut hasher = Sha256::new();
        hasher.update(payload);
        hasher.update(self.cursor_key);
        let expected = hasher.finalize();
        if expected
            .iter()
            .zip(&bytes[50..])
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            != 0
        {
            return Err(invalid_cursor());
        }
        let owner_id = Uuid::from_slice(&bytes[2..18]).map_err(|_| invalid_cursor())?;
        let run_id = Uuid::from_slice(&bytes[18..34]).map_err(|_| invalid_cursor())?;
        let snapshot = u64::from_be_bytes(bytes[34..42].try_into().map_err(|_| invalid_cursor())?);
        let last = u64::from_be_bytes(bytes[42..50].try_into().map_err(|_| invalid_cursor())?);
        Ok((owner_id, run_id, collection, snapshot, last))
    }
}

fn invalid_cursor() -> ExecutionStoreError {
    ExecutionStoreError::new(ExecutionStoreErrorCode::InvalidRequest)
}

fn page_take(page: &super::StoreReadPage) -> Result<usize, ExecutionStoreError> {
    usize::try_from(page.limit())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?
        .checked_add(1)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((hex_value(pair[0])? << 4) | hex_value(pair[1])?))
        .collect()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[async_trait]
impl ExecutionStore for InMemoryExecutionStore {
    async fn apply_authoritative_policy(
        &self,
        owner_id: Uuid,
        change: AuthoritativePolicyChange,
    ) -> Result<AuthoritativePolicyState, ExecutionStoreError> {
        if owner_id.is_nil() {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let (definition_id, definition_version) = change.key();
        let key = (owner_id, definition_id, definition_version);
        let mut state = self.state.lock().await;
        let next = change.apply_to(state.authoritative_policies.get(&key))?;
        if next.owner_id() != owner_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        state.authoritative_policies.insert(key, next.clone());
        Ok(next)
    }

    async fn load_authoritative_policy(
        &self,
        owner_id: Uuid,
        agent_definition_id: &str,
        agent_definition_version: u32,
    ) -> Result<Option<AuthoritativePolicyState>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .authoritative_policies
            .get(&(
                owner_id,
                agent_definition_id.to_owned(),
                agent_definition_version,
            ))
            .cloned())
    }

    async fn apply_authoritative_grant(
        &self,
        owner_id: Uuid,
        change: AuthoritativeGrantChange,
    ) -> Result<AuthoritativeGrantState, ExecutionStoreError> {
        if owner_id.is_nil()
            || change
                .new_state()
                .is_some_and(|next| next.owner_id() != owner_id)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let mut state = self.state.lock().await;
        let key = (owner_id, change.authority_key().as_str().to_owned());
        let next = change.apply_to(state.authoritative_grants.get(&key))?;
        state.authoritative_grants.insert(
            (owner_id, next.authority_key_encoded().to_owned()),
            next.clone(),
        );
        Ok(next)
    }

    async fn load_authoritative_grant(
        &self,
        owner_id: Uuid,
        authority_key: &super::GrantAuthorityKey,
    ) -> Result<Option<AuthoritativeGrantState>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .authoritative_grants
            .get(&(owner_id, authority_key.as_str().to_owned()))
            .cloned())
    }

    async fn create_run(
        &self,
        owner_id: Uuid,
        request: CreateRun,
    ) -> Result<StoredRun, ExecutionStoreError> {
        let session = request.session();
        if owner_id.is_nil()
            || request.owner_id() != owner_id
            || request.run().state() != RunState::Queued
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
        if state.runs.contains_key(&(owner_id, request.run().id())) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::VersionConflict,
            ));
        }

        let session_key = (owner_id, session.id());
        let session_version = match state.sessions.get(&session_key) {
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
            && state.serial_claims.contains_key(&session_key)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::ActiveRunConflict,
            ));
        }

        state.sessions.insert(
            session_key,
            StoredSession {
                version: session_version,
                owner_id: request.owner_id(),
                definition: session.definition().clone(),
                policy: request.concurrency_policy(),
            },
        );
        if request.concurrency_policy() == SessionConcurrencyPolicy::Serial {
            state.serial_claims.insert(session_key, request.run().id());
        }
        let stored = StoredRun::new(owner_id, request.run().clone(), 1, session_version)?;
        state.runs.insert(
            (owner_id, request.run().id()),
            RunAggregate {
                stored: stored.clone(),
                lease: None,
                checkpoint_version: 0,
                checkpoint: None,
                events: vec![],
                steps: BTreeMap::new(),
                step_order: BTreeMap::new(),
                next_step_sequence: 1,
                attempts: BTreeMap::new(),
                attempt_order: BTreeMap::new(),
                next_attempt_sequence: 1,
                attempts_fingerprint: super::checkpoint::HistoryFingerprint::default(),
                results: BTreeMap::new(),
                completed_fingerprint: super::checkpoint::HistoryFingerprint::default(),
            },
        );
        Ok(stored)
    }

    async fn acquire_lease(
        &self,
        owner_id: Uuid,
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
            .get_mut(&(owner_id, run_id))
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
        let now_ms = self.clock.now_ms();
        if aggregate
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at_ms() > now_ms)
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseConflict,
            ));
        }
        let expires_at_ms = now_ms
            .checked_add(duration_ms)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let lease = ExecutionLease::new(run_id, Uuid::new_v4(), expires_at_ms)
            .map_err(ExecutionStoreError::from)?;
        aggregate.lease = Some(lease.clone());
        Ok(lease)
    }

    async fn renew_lease(
        &self,
        owner_id: Uuid,
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
            .get_mut(&(owner_id, lease.run_id()))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        if aggregate.lease.as_ref() != Some(&lease) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseExpired,
            ));
        }
        let now_ms = self.clock.now_ms();
        if lease.expires_at_ms() <= now_ms {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LeaseExpired,
            ));
        }
        let expires_at_ms = now_ms
            .checked_add(duration_ms)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let renewed = ExecutionLease::new(lease.run_id(), lease.fence(), expires_at_ms)
            .map_err(ExecutionStoreError::from)?;
        aggregate.lease = Some(renewed.clone());
        Ok(renewed)
    }

    async fn commit_execution(
        &self,
        owner_id: Uuid,
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
        let command_key = (owner_id, commit.command().id());
        if let Some(receipt) = state.receipts.get(&command_key) {
            receipt
                .replay(commit.command())
                .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::CommandConflict))?;
            return state
                .outcomes
                .get(&command_key)
                .cloned()
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound));
        }

        let mut patch = build_commit_patch(&state, owner_id, &commit, self.clock.now_ms())?;
        patch.checkpoint = match commit.into_checkpoint_mutation() {
            CheckpointMutation::Unchanged => patch.checkpoint,
            CheckpointMutation::Replace(checkpoint) => Some(Arc::from(checkpoint)),
            CheckpointMutation::Clear => None,
        };
        let outcome = ExecutionCommitOutcome::new_shared(
            patch.stored.clone(),
            patch.receipt.clone(),
            patch.checkpoint_version,
            patch.checkpoint.clone(),
            patch.grant_consumption.clone(),
        );
        apply_commit_patch(&mut state, owner_id, run_id, patch, outcome.clone());
        Ok(outcome)
    }
    async fn load_run(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<StoredRun>, ExecutionStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .runs
            .get(&(owner_id, run_id))
            .map(|aggregate| aggregate.stored.clone()))
    }

    async fn load_checkpoint(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<(u64, super::CheckpointV1)>, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        Ok(aggregate
            .checkpoint
            .as_ref()
            .map(|checkpoint| (aggregate.checkpoint_version, checkpoint.as_ref().clone())))
    }

    async fn load_steps_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<super::StoreHistoryPage<super::Step>, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        let snapshot = aggregate
            .step_order
            .last_key_value()
            .map_or(0, |(sequence, _)| *sequence);
        let (snapshot, last) =
            self.read_window(owner_id, run_id, HistoryCollection::Steps, snapshot, &page)?;
        let take = page_take(&page)?;
        let mut items = aggregate
            .step_order
            .range((Excluded(last), Included(snapshot)))
            .take(take)
            .map(|(_, key)| {
                aggregate.steps.get(key).cloned().ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > usize::try_from(page.limit()).unwrap_or(usize::MAX);
        if has_more {
            items.pop();
        }
        let next_cursor = if has_more {
            let last = aggregate
                .step_order
                .range((Excluded(last), Included(snapshot)))
                .nth(items.len().saturating_sub(1))
                .map(|(sequence, _)| *sequence)
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })?;
            Some(self.write_cursor(owner_id, run_id, HistoryCollection::Steps, snapshot, last)?)
        } else {
            None
        };
        super::StoreHistoryPage::new(items, next_cursor)
    }

    async fn load_attempts_page(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<super::StoreHistoryPage<super::InvocationAttemptRecord>, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        let snapshot = aggregate
            .attempt_order
            .last_key_value()
            .map_or(0, |(sequence, _)| *sequence);
        let (snapshot, last) = self.read_window(
            owner_id,
            run_id,
            HistoryCollection::Attempts,
            snapshot,
            &page,
        )?;
        let take = page_take(&page)?;
        let mut page_entries = aggregate
            .attempt_order
            .range((Excluded(last), Included(snapshot)))
            .take(take)
            .collect::<Vec<_>>();
        let has_more = page_entries.len() > usize::try_from(page.limit()).unwrap_or(usize::MAX);
        if has_more {
            page_entries.pop();
        }
        let next_cursor = if has_more {
            let last = page_entries
                .last()
                .map(|(sequence, _)| **sequence)
                .ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })?;
            Some(self.write_cursor(
                owner_id,
                run_id,
                HistoryCollection::Attempts,
                snapshot,
                last,
            )?)
        } else {
            None
        };
        let items = page_entries
            .into_iter()
            .map(|(_, key)| {
                aggregate.attempts.get(key).cloned().ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        super::StoreHistoryPage::new(items, next_cursor)
    }

    async fn load_durable_result(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        logical_invocation_id: Uuid,
    ) -> Result<Option<DurableCapabilityResult>, ExecutionStoreError> {
        let state = self.state.lock().await;
        Ok(state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?
            .results
            .get(&logical_invocation_id)
            .map(|stored| stored.result.clone()))
    }

    async fn replay_events(
        &self,
        owner_id: Uuid,
        run_id: Uuid,
        page: super::StoreReadPage,
    ) -> Result<super::EventReplayPage, ExecutionStoreError> {
        let state = self.state.lock().await;
        let aggregate = state
            .runs
            .get(&(owner_id, run_id))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
        let current_snapshot = u64::try_from(aggregate.events.len())
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let (snapshot, last) = self.read_window(
            owner_id,
            run_id,
            HistoryCollection::Events,
            current_snapshot,
            &page,
        )?;
        let start = usize::try_from(last)
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let end = usize::try_from(snapshot)
            .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::BoundsExceeded))?;
        let take = page_take(&page)?;
        let mut events = aggregate.events[start..end]
            .iter()
            .take(take)
            .cloned()
            .collect::<Vec<_>>();
        let has_more = events.len() > usize::try_from(page.limit()).unwrap_or(usize::MAX);
        if has_more {
            events.pop();
        }
        let next_cursor = if has_more {
            let last = events
                .last()
                .map(RuntimeEvent::sequence)
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::EventConflict))?;
            Some(self.write_cursor(owner_id, run_id, HistoryCollection::Events, snapshot, last)?)
        } else {
            None
        };
        super::EventReplayPage::new(events, next_cursor)
    }
}

fn build_commit_patch(
    state: &State,
    owner_id: Uuid,
    commit: &ExecutionCommit,
    now_ms: u64,
) -> Result<CommitPatch, ExecutionStoreError> {
    let run_id = commit.lease().run_id();
    let aggregate = state
        .runs
        .get(&(owner_id, run_id))
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
    if aggregate.stored.run_version() != commit.expected_run_version() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::VersionConflict,
        ));
    }
    if aggregate.checkpoint_version != commit.expected_checkpoint_version() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::CheckpointConflict,
        ));
    }
    if aggregate.lease.as_ref() != Some(commit.lease()) || commit.lease().expires_at_ms() <= now_ms
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
        commit.dispatch_grant(),
        commit.attempts(),
    ) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
    }

    let dispatching_attempts = commit
        .attempts()
        .iter()
        .filter(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
        .collect::<Vec<_>>();
    match (dispatching_attempts.as_slice(), commit.policy_guard()) {
        ([attempt], Some(guard)) if guard.matches_attempt(owner_id, attempt) => {
            let authority = state
                .authoritative_policies
                .get(&(
                    owner_id,
                    guard.agent_definition_id().to_owned(),
                    guard.agent_definition_version(),
                ))
                .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::PolicyConflict))?;
            if !authority.validates(guard, now_ms) {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::PolicyConflict,
                ));
            }
        }
        ([], None) => {}
        _ => {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::PolicyConflict,
            ))
        }
    }

    let current_event_count = u64::try_from(aggregate.events.len())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let first_sequence = current_event_count
        .checked_add(1)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let event_count = u64::try_from(commit.events().len())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let last_sequence = current_event_count
        .checked_add(event_count)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    RuntimeEvent::validate_batch(first_sequence, commit.events())
        .map_err(|_| ExecutionStoreError::new(ExecutionStoreErrorCode::EventConflict))?;
    let session = state
        .sessions
        .get(&(owner_id, aggregate.stored.run().session_id()))
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::NotFound))?;
    if commit.events().iter().any(|event| {
        event.owner_id() != session.owner_id
            || event.run_id() != run_id
            || event.session_id() != aggregate.stored.run().session_id()
    }) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::EventConflict,
        ));
    }
    if let CheckpointMutation::Replace(checkpoint) = commit.checkpoint_mutation() {
        if checkpoint.run_id() != run_id
            || checkpoint.session_id() != aggregate.stored.run().session_id()
            || checkpoint.definition().id() != aggregate.stored.run().definition_id()
            || checkpoint.definition().version() != aggregate.stored.run().definition_version()
            || checkpoint.state() != commit.target_run().state()
            || checkpoint.pause_reason() != commit.target_run().pause_reason()
            || checkpoint
                .pending_approval()
                .map(|pending| pending.request())
                != commit.target_run().pending_approval()
            || commit.target_run().state() == RunState::RecoveryRequired
                && !checkpoint.uncertain_invocations().iter().any(|record| {
                    commit.target_run().recovery_pause() == Some(record.pause())
                        && commit.target_run().recovery_binding() == record.recovery_binding()
                })
            || checkpoint.last_durable_event_sequence() != last_sequence
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::CheckpointConflict,
            ));
        }
    }

    if commit.approval().is_some() && commit.dispatch_grant().is_some() {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::InvalidRequest,
        ));
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
            (Some(_), Some(grant_revision), claimed_remaining) => {
                let binding = approval.claim().grant_authority_binding().ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                })?;
                let mut authoritative = state
                    .authoritative_grants
                    .get(&(owner_id, binding.authority_key().as_str().to_owned()))
                    .cloned()
                    .ok_or_else(|| {
                        ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                    })?;
                authoritative.validate_binding(binding, now_ms)?;
                if authoritative.revision() != grant_revision
                    || authoritative.remaining_uses() != claimed_remaining
                {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::GrantConflict,
                    ));
                }
                match (approval.grant_consumption(), claimed_remaining) {
                    (Some(consumption), Some(remaining)) => {
                        let snapshot =
                            approval
                                .claim()
                                .grant_consumption_snapshot()
                                .ok_or_else(|| {
                                    ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict)
                                })?;
                        if snapshot.remaining_uses() != remaining
                            || !binding.matches_consumption(consumption)
                        {
                            return Err(ExecutionStoreError::new(
                                ExecutionStoreErrorCode::GrantConflict,
                            ));
                        }
                        let key = (
                            owner_id,
                            binding.authority_key().as_str().to_owned(),
                            consumption.grant_revision,
                            consumption.logical_invocation_id,
                        );
                        if state.grant_consumptions.contains(&key) {
                            return Err(ExecutionStoreError::new(
                                ExecutionStoreErrorCode::GrantAlreadyConsumed,
                            ));
                        }
                        authoritative = authoritative.consume(binding, now_ms)?;
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
    if let Some(dispatch) = commit.dispatch_grant() {
        let attempt = commit
            .attempts()
            .iter()
            .find(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        if commit.command().kind() != super::RuntimeCommandKind::PrepareDispatch
            || !dispatch.matches_attempt(owner_id, attempt)
            || commit
                .attempts()
                .iter()
                .filter(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
                .count()
                != 1
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        let binding = dispatch.authority_binding();
        let authoritative = state
            .authoritative_grants
            .get(&(owner_id, binding.authority_key().as_str().to_owned()))
            .cloned()
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::GrantConflict))?;
        if authoritative.remaining_uses() != Some(dispatch.remaining_uses())
            || !binding.matches_consumption(dispatch.consumption())
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantConflict,
            ));
        }
        let key = (
            owner_id,
            binding.authority_key().as_str().to_owned(),
            dispatch.consumption().grant_revision,
            dispatch.consumption().logical_invocation_id,
        );
        if state.grant_consumptions.contains(&key) {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::GrantAlreadyConsumed,
            ));
        }
        authoritative_grant_update = Some(authoritative.consume(binding, now_ms)?);
        grant_consumption_key = Some(key);
    }

    let mut step_patches = BTreeMap::new();
    let mut next_step_sequence = aggregate.next_step_sequence;
    for step in commit.steps() {
        if step.run_id() != run_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        match aggregate.steps.get(step.logical_step_id()) {
            Some(existing) if existing == step => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        match step_patches.get(step.logical_step_id()) {
            Some((_, existing)) if existing == step => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        let sequence = next_step_sequence;
        next_step_sequence = next_step_sequence
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        step_patches.insert(step.logical_step_id().to_owned(), (sequence, step.clone()));
    }

    let mut attempt_patches = BTreeMap::new();
    let mut attempt_transition_keys = BTreeSet::new();
    let mut new_attempt_count = 0usize;
    let mut next_attempt_sequence = aggregate.next_attempt_sequence;
    let mut attempts_fingerprint = aggregate.attempts_fingerprint;
    for attempt in commit.attempts() {
        if attempt.invocation().run_id() != run_id {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::InvalidRequest,
            ));
        }
        let key = (attempt.invocation().id(), attempt.attempt_number());
        let existing_sequence = aggregate
            .attempt_order
            .iter()
            .find_map(|(sequence, stored_key)| (stored_key == &key).then_some(*sequence));
        match aggregate.attempts.get(&key) {
            Some(existing) if existing == attempt => continue,
            Some(existing)
                if matches!(
                    (existing.state(), attempt.state()),
                    (
                        super::AttemptRecordState::Dispatching,
                        super::AttemptRecordState::Completed | super::AttemptRecordState::Uncertain
                    ) | (
                        super::AttemptRecordState::Pending,
                        super::AttemptRecordState::Dispatching
                    )
                ) && existing.invocation() == attempt.invocation()
                    && existing.manifest() == attempt.manifest()
                    && existing.recovery_mode() == attempt.recovery_mode() =>
            {
                let sequence = existing_sequence.ok_or_else(|| {
                    ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict)
                })?;
                attempts_fingerprint
                    .include(existing)
                    .map_err(ExecutionStoreError::from)?;
                attempts_fingerprint
                    .include(attempt)
                    .map_err(ExecutionStoreError::from)?;
                attempt_transition_keys.insert(key);
                attempt_patches.insert(key, (sequence, attempt.clone()));
                continue;
            }
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        match attempt_patches.get(&key) {
            Some((_, existing)) if existing == attempt => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ))
            }
            None => {}
        }
        let sequence = next_attempt_sequence;
        next_attempt_sequence = next_attempt_sequence
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        attempts_fingerprint
            .include(attempt)
            .map_err(ExecutionStoreError::from)?;
        new_attempt_count = new_attempt_count
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        attempt_patches.insert(key, (sequence, attempt.clone()));
    }

    let mut result_patches = BTreeMap::new();
    let mut completed_fingerprint = aggregate.completed_fingerprint;
    for mutation in commit.results() {
        let completed = mutation.completed();
        if completed.invocation().run_id() != run_id
            || completed.result_ref().value() != mutation.result().result_ref().handle()
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::LineageConflict,
            ));
        }
        let key = completed.invocation().id();
        let attempt_key = (key, completed.attempt_number());
        let attempt = attempt_patches
            .get(&attempt_key)
            .map(|(_, attempt)| attempt)
            .or_else(|| aggregate.attempts.get(&attempt_key))
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::LineageConflict))?;
        if attempt.invocation() != completed.invocation()
            || attempt.state() != super::AttemptRecordState::Completed
            || attempt.manifest() != completed.manifest()
            || attempt.recovery_mode() != completed.recovery_mode()
            || mutation.result().schema_digest() != attempt.manifest().schema_digest()
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
            Some(existing) if existing == &value => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::ResultConflict,
                ))
            }
            None => {}
        }
        match result_patches.get(&key) {
            Some(existing) if existing == &value => continue,
            Some(_) => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::ResultConflict,
                ))
            }
            None => {}
        }
        completed_fingerprint
            .include(completed)
            .map_err(ExecutionStoreError::from)?;
        result_patches.insert(key, value);
    }

    for key in &attempt_transition_keys {
        let attempt = attempt_patches
            .get(key)
            .map(|(_, attempt)| attempt)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::HistoryConflict))?;
        match attempt.state() {
            super::AttemptRecordState::Completed => {
                if !result_patches
                    .get(&attempt.invocation().id())
                    .is_some_and(|stored| {
                        stored.completed.invocation() == attempt.invocation()
                            && stored.completed.attempt_number() == attempt.attempt_number()
                            && stored.completed.manifest() == attempt.manifest()
                            && stored.completed.recovery_mode() == attempt.recovery_mode()
                    })
                {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::LineageConflict,
                    ));
                }
            }
            super::AttemptRecordState::Uncertain => {
                let exact_recovery = commit.command().recovery_record().is_some_and(|recovery| {
                    let pause = recovery.pause();
                    pause.invocation() == attempt.invocation()
                        && pause.attempt_number() == attempt.attempt_number()
                        && pause.manifest() == attempt.manifest()
                        && pause.manifest().recovery_mode() == attempt.recovery_mode()
                });
                if !exact_recovery {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::LineageConflict,
                    ));
                }
            }
            super::AttemptRecordState::Dispatching => {
                let pending = aggregate.stored.run().pending_approval();
                let exact_resume = commit.command().kind()
                    == super::RuntimeCommandKind::ResumeApproval
                    && commit.approval().is_some()
                    && aggregate.stored.run().state() == RunState::WaitingForApproval
                    && commit.target_run().state() == RunState::Running
                    && pending.is_some_and(|request| {
                        request.logical_invocation_id == attempt.invocation().id()
                            && request.canonical_argument_digest
                                == attempt.invocation().canonical_argument_digest()
                            && request.capability_id == attempt.invocation().capability_id()
                            && request.manifest_version == attempt.invocation().manifest_version()
                    });
                if !exact_resume {
                    return Err(ExecutionStoreError::new(
                        ExecutionStoreErrorCode::LineageConflict,
                    ));
                }
            }
            _ => {
                return Err(ExecutionStoreError::new(
                    ExecutionStoreErrorCode::HistoryConflict,
                ));
            }
        }
    }

    if let CheckpointMutation::Replace(checkpoint) = commit.checkpoint_mutation() {
        let expected_attempts = aggregate
            .attempts
            .len()
            .checked_add(new_attempt_count)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        let expected_completed = aggregate
            .results
            .len()
            .checked_add(result_patches.len())
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
        if checkpoint.attempts().len() != expected_attempts
            || checkpoint.completed_invocations().len() != expected_completed
            || checkpoint.attempts_fingerprint() != attempts_fingerprint
            || checkpoint.completed_fingerprint() != completed_fingerprint
            || checkpoint.cursor().is_some_and(|cursor| {
                !aggregate.steps.contains_key(cursor.logical_step_id())
                    && !step_patches.contains_key(cursor.logical_step_id())
            })
        {
            return Err(ExecutionStoreError::new(
                ExecutionStoreErrorCode::CheckpointConflict,
            ));
        }
    }

    let aggregate_changed = aggregate.stored.run() != commit.target_run()
        || !commit.events().is_empty()
        || !step_patches.is_empty()
        || !attempt_patches.is_empty()
        || !result_patches.is_empty();
    if aggregate_changed && matches!(commit.checkpoint_mutation(), CheckpointMutation::Unchanged) {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::CheckpointConflict,
        ));
    }
    if commit.target_run().state().is_terminal()
        && !matches!(commit.checkpoint_mutation(), CheckpointMutation::Clear)
    {
        return Err(ExecutionStoreError::new(
            ExecutionStoreErrorCode::CheckpointConflict,
        ));
    }

    let checkpoint_version = match commit.checkpoint_mutation() {
        CheckpointMutation::Unchanged => aggregate.checkpoint_version,
        CheckpointMutation::Replace(_) => aggregate
            .checkpoint_version
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?,
        CheckpointMutation::Clear if aggregate.checkpoint.is_some() => aggregate
            .checkpoint_version
            .checked_add(1)
            .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?,
        CheckpointMutation::Clear => aggregate.checkpoint_version,
    };
    let checkpoint = match commit.checkpoint_mutation() {
        CheckpointMutation::Unchanged => aggregate.checkpoint.clone(),
        CheckpointMutation::Replace(_) => None,
        CheckpointMutation::Clear => None,
    };
    let run_version = aggregate
        .stored
        .run_version()
        .checked_add(1)
        .ok_or_else(|| ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow))?;
    let mut stored = StoredRun::new(
        owner_id,
        commit.target_run().clone(),
        run_version,
        aggregate.stored.session_version(),
    )?;
    let mut lease = aggregate.lease.clone();
    let mut session_update = None;
    let mut release_serial_claim = false;
    if commit.target_run().state().is_terminal() {
        lease = None;
        if state
            .serial_claims
            .get(&(owner_id, commit.target_run().session_id()))
            == Some(&run_id)
        {
            let mut session = session.clone();
            session.version = session.version.checked_add(1).ok_or_else(|| {
                ExecutionStoreError::new(ExecutionStoreErrorCode::ArithmeticOverflow)
            })?;
            stored = StoredRun::new(
                owner_id,
                commit.target_run().clone(),
                run_version,
                session.version,
            )?;
            session_update = Some(session);
            release_serial_claim = true;
        }
    }
    let receipt = CommandReceipt::accepted(commit.command()).map_err(ExecutionStoreError::from)?;
    let grant_consumption = commit
        .approval()
        .and_then(|approval| approval.grant_consumption())
        .or_else(|| {
            commit
                .dispatch_grant()
                .map(|dispatch| dispatch.consumption())
        })
        .cloned();
    Ok(CommitPatch {
        stored,
        lease,
        checkpoint_version,
        checkpoint,
        events: commit.events().to_vec(),
        steps: step_patches
            .into_iter()
            .map(|(key, (sequence, step))| (key, sequence, step))
            .collect(),
        next_step_sequence,
        attempts: attempt_patches
            .into_iter()
            .map(|(key, (sequence, attempt))| (key, sequence, attempt))
            .collect(),
        next_attempt_sequence,
        attempts_fingerprint,
        results: result_patches.into_iter().collect(),
        completed_fingerprint,
        authoritative_grant_update,
        grant_consumption_key,
        session_update,
        release_serial_claim,
        receipt,
        grant_consumption,
    })
}

fn apply_commit_patch(
    state: &mut State,
    owner_id: Uuid,
    run_id: Uuid,
    patch: CommitPatch,
    outcome: ExecutionCommitOutcome,
) {
    let command_key = (owner_id, patch.receipt.command_id());
    let session_id = patch.stored.run().session_id();
    {
        let aggregate = state
            .runs
            .get_mut(&(owner_id, run_id))
            .expect("validated run aggregate must remain present under the owner lock");
        aggregate.stored = patch.stored;
        aggregate.lease = patch.lease;
        aggregate.checkpoint_version = patch.checkpoint_version;
        aggregate.checkpoint = patch.checkpoint;
        aggregate.events.extend(patch.events);
        for (key, sequence, step) in patch.steps {
            aggregate.step_order.insert(sequence, key.clone());
            aggregate.steps.insert(key, step);
        }
        aggregate.next_step_sequence = patch.next_step_sequence;
        for (key, sequence, attempt) in patch.attempts {
            aggregate.attempt_order.insert(sequence, key);
            aggregate.attempts.insert(key, attempt);
        }
        aggregate.next_attempt_sequence = patch.next_attempt_sequence;
        aggregate.attempts_fingerprint = patch.attempts_fingerprint;
        for (key, result) in patch.results {
            aggregate.results.insert(key, result);
        }
        aggregate.completed_fingerprint = patch.completed_fingerprint;
    }
    if let Some(authoritative) = patch.authoritative_grant_update {
        state.authoritative_grants.insert(
            (owner_id, authoritative.authority_key_encoded().to_owned()),
            authoritative,
        );
    }
    if let Some(key) = patch.grant_consumption_key {
        state.grant_consumptions.insert(key);
    }
    if let Some(session) = patch.session_update {
        state.sessions.insert((owner_id, session_id), session);
    }
    if patch.release_serial_claim {
        state.serial_claims.remove(&(owner_id, session_id));
    }
    state.receipts.insert(command_key, patch.receipt);
    state.outcomes.insert(command_key, outcome);
}

fn valid_command_transition(
    current: &super::Run,
    target: &super::Run,
    command: &super::RuntimeCommand,
    approval: Option<&super::ApprovalGrantMutation>,
    dispatch_grant: Option<&super::DispatchGrantMutation>,
    attempts: &[super::InvocationAttemptRecord],
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
    if attempts
        .iter()
        .any(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
        && !matches!(
            command.kind(),
            super::RuntimeCommandKind::PrepareDispatch | super::RuntimeCommandKind::ResumeApproval
        )
    {
        return false;
    }
    match command.kind() {
        super::RuntimeCommandKind::Start => {
            approval.is_none()
                && current.state() == RunState::Queued
                && target.state() == RunState::Running
        }
        super::RuntimeCommandKind::RecordProgress => {
            approval.is_none()
                && dispatch_grant.is_none()
                && attempts
                    .iter()
                    .all(|attempt| attempt.state() != super::AttemptRecordState::Dispatching)
                && current.state() == RunState::Running
                && current == target
        }
        super::RuntimeCommandKind::PrepareDispatch => {
            approval.is_none()
                && attempts
                    .iter()
                    .filter(|attempt| attempt.state() == super::AttemptRecordState::Dispatching)
                    .count()
                    == 1
                && current.state() == RunState::Running
                && current == target
        }
        super::RuntimeCommandKind::RequestApproval => {
            approval.is_none()
                && command.approval_request().is_some_and(|request| {
                    current
                        .wait_for_approval(request.clone())
                        .is_ok_and(|expected| &expected == target)
                })
        }
        super::RuntimeCommandKind::RequireRecovery => {
            approval.is_none()
                && command.recovery_record().is_some_and(|recovery| {
                    let pause = recovery.pause();
                    attempts.iter().any(|attempt| {
                        attempt.invocation() == pause.invocation()
                            && attempt.attempt_number() == pause.attempt_number()
                            && attempt.state() == super::AttemptRecordState::Uncertain
                            && attempt.manifest() == pause.manifest()
                            && attempt.recovery_mode() == pause.manifest().recovery_mode()
                    }) && current
                        .require_recovery(recovery.clone())
                        .is_ok_and(|expected| &expected == target)
                })
        }
        super::RuntimeCommandKind::Pause => {
            approval.is_none()
                && command.pause_reason().is_some_and(|reason| {
                    current
                        .transition(RunState::Paused, Some(reason))
                        .is_ok_and(|expected| &expected == target)
                })
        }
        super::RuntimeCommandKind::ResumeApproval => current
            .apply_resume_command(command, approval.map(|mutation| mutation.claim()), None)
            .is_ok_and(|outcome| outcome.run() == target),
        super::RuntimeCommandKind::ResumeRecovery => {
            approval.is_none()
                && current.state() == RunState::RecoveryRequired
                && current.recovery_pause().is_some_and(|pause| {
                    command.recovery_binding().is_some_and(|binding| {
                        current.recovery_binding() == Some(binding)
                            && pause.matches_resume_binding(binding)
                            && attempts.iter().any(|attempt| {
                                attempt.invocation().id() == binding.logical_invocation_id()
                                    && attempt.attempt_number() == binding.retry_attempt_number()
                                    && attempt.state() == super::AttemptRecordState::Pending
                                    && attempt.manifest().id() == binding.manifest_id()
                                    && attempt.manifest().version() == binding.manifest_version()
                                    && attempt.manifest().schema_digest()
                                        == binding.manifest_digest()
                                    && attempt.recovery_mode() == binding.recovery_mode()
                            })
                    })
                })
                && target.state() == RunState::Running
                && target.pause_reason().is_none()
                && target.pending_approval().is_none()
                && target.recovery_pause().is_none()
        }
        super::RuntimeCommandKind::Complete => {
            approval.is_none()
                && current
                    .transition(RunState::Completed, None)
                    .is_ok_and(|expected| &expected == target)
        }
        super::RuntimeCommandKind::Fail => {
            approval.is_none()
                && match current.state() {
                    RunState::Running => current
                        .transition(RunState::Failed, None)
                        .is_ok_and(|expected| &expected == target),
                    RunState::RecoveryRequired => current
                        .resolve_recovery_terminal(super::RecoveryTerminalResolution::Fail)
                        .is_ok_and(|expected| expected.run() == target),
                    _ => false,
                }
        }
        super::RuntimeCommandKind::Cancel => {
            approval.is_none()
                && match current.state() {
                    RunState::Running => current
                        .transition(RunState::Cancelled, None)
                        .is_ok_and(|expected| &expected == target),
                    RunState::RecoveryRequired => current
                        .resolve_recovery_terminal(super::RecoveryTerminalResolution::Cancel)
                        .is_ok_and(|expected| expected.run() == target),
                    _ => false,
                }
        }
    }
}
