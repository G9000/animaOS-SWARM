//! Accepted runs (spec §4.2–§4.3): acceptance under the control-plane
//! transaction with one save, then per-session execution in acceptance
//! order. The order is fixed where the run is accepted, not where a task
//! happens to be scheduled (M1 F15). Steers (spec §4.7) join the session's
//! running run instead, saved with it, and whatever it leaves behind is
//! handed on when its execution ends.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use anima_core::primitives::now_millis;
use anima_core::{
    Content, DataValue, Message, MessageRole, RUN_ID_METADATA_KEY, STEER_METADATA_KEY,
};
use futures::future::BoxFuture;
use tokio::sync::OwnedMutexGuard;
use tracing::{error, warn};

use super::{
    is_helper_config, AgentRunCoordinator, AgentRunRequest, RunRoom,
    HELPER_MUST_RUN_THROUGH_COMPANION, MAX_QUEUED_RUNS_PER_AGENT,
};
use crate::live::{run_status_event, LiveEventBody};
use crate::routes::ApiError;
use crate::runs::{
    RunError, RunRecord, RunSource, RunStart, RunStatus, RunSteer, COMMIT_FAILED,
    IDEMPOTENCY_WINDOW_MS, QUEUE_FULL_BEFORE_START, QUEUE_FULL_BEFORE_START_MESSAGE, RUN_FAILED,
    RUN_STOPPED, STOPPED_BEFORE_START, STOPPED_BEFORE_START_MESSAGE, STOPPED_BY_OWNER,
};
use crate::sessions::{derived_title, SessionKind, TitleSource, DEFAULT_CHAT_TITLE};
use crate::state::{ControlPlanePersistRequest, DaemonState};

pub(crate) const SESSION_CANNOT_SEND: &str = "This session cannot receive messages";
pub(crate) const SESSION_CANNOT_STEER: &str = "This session cannot be steered";
pub(crate) const IDEMPOTENCY_KEY_REUSED: &str =
    "Idempotency-Key was already used for a different message";
pub(crate) const QUEUE_FULL: &str =
    "This companion already has 8 queued messages; wait for one to start";
pub(crate) const RUN_NOT_QUEUED: &str = "This run is no longer waiting to start";
pub(crate) const RUN_STOPPED_BEFORE_START: &str = "This run was stopped before it started";
const RUN_ENDED_BEFORE_START: &str = "The run stopped unexpectedly before it started";

/// Metadata an owner message's content carries: its idempotency key, and
/// for a steer when it was accepted.
pub(crate) const CLIENT_REQUEST_ID_METADATA_KEY: &str = "clientRequestId";
pub(crate) const ACCEPTED_AT_METADATA_KEY: &str = "acceptedAtMs";

fn metadata_text<'a>(content: &'a Content, key: &str) -> Option<&'a str> {
    match content.metadata.as_ref()?.get(key)? {
        DataValue::String(value) => Some(value),
        _ => None,
    }
}

/// When a steer was accepted, from its metadata.
fn accepted_at_ms(content: &Content) -> Option<u64> {
    match content.metadata.as_ref()?.get(ACCEPTED_AT_METADATA_KEY)? {
        DataValue::Number(at) if at.is_finite() && *at >= 0.0 => Some(*at as u64),
        _ => None,
    }
}

/// A steer as its run's inbox carries it: the text with its key and when it
/// was accepted, which the runtime keeps on the message it records.
fn steer_content(steer: &RunSteer) -> Content {
    Content {
        text: steer.text.clone(),
        attachments: None,
        metadata: Some(BTreeMap::from([
            (
                CLIENT_REQUEST_ID_METADATA_KEY.to_string(),
                DataValue::String(steer.idempotency_key.clone()),
            ),
            (
                ACCEPTED_AT_METADATA_KEY.to_string(),
                DataValue::Number(steer.accepted_at_ms as f64),
            ),
        ])),
    }
}

fn is_steer_message(message: &Message) -> bool {
    message.role == MessageRole::User
        && message
            .content
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get(STEER_METADATA_KEY))
            == Some(&DataValue::Bool(true))
}

/// The steers a run took into `messages`, its new transcript (spec §4.7).
pub(crate) fn steers_taken_in(messages: &[Message]) -> Vec<RunSteer> {
    messages
        .iter()
        .filter(|message| is_steer_message(message))
        .filter_map(|message| {
            Some(RunSteer {
                idempotency_key: metadata_text(&message.content, CLIENT_REQUEST_ID_METADATA_KEY)?
                    .to_string(),
                text: message.content.text.clone(),
                accepted_at_ms: accepted_at_ms(&message.content).unwrap_or(message.created_at_ms),
            })
        })
        .collect()
}

/// When messages and steers are accepted (spec §4.2, §4.7): the wall clock,
/// but always after the previous acceptance, so a clock stepping backwards
/// cannot reorder a session's messages (Task 7 review Minor 2). One per
/// coordinator, shared by its clones, and read under the control-plane
/// transaction.
#[derive(Clone, Default)]
pub(super) struct AcceptanceClock {
    last_ms: Arc<AtomicU64>,
    #[cfg(test)]
    wall: Arc<StdMutex<Option<TestWallClock>>>,
}

/// A test's stand-in for the wall clock.
#[cfg(test)]
type TestWallClock = Box<dyn FnMut() -> u64 + Send>;

impl AcceptanceClock {
    fn wall_ms(&self) -> u64 {
        #[cfg(test)]
        {
            let mut wall = self
                .wall
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(wall) = wall.as_mut() {
                return wall();
            }
        }
        now_millis()
    }

    /// The next acceptance time: later than every earlier one.
    fn next(&self) -> u64 {
        let wall_ms = self.wall_ms();
        let mut next = wall_ms;
        let _ = self
            .last_ms
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last_ms| {
                next = wall_ms.max(last_ms.saturating_add(1));
                Some(next)
            });
        next
    }
}

/// How a message joins its session (spec §4.2, §4.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRunMode {
    Queue,
    Steer,
}

/// A message to accept into a session.
#[derive(Clone, Debug)]
pub(crate) struct AcceptRun {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) text: String,
    pub(crate) idempotency_key: String,
    pub(crate) mode: SessionRunMode,
    /// `Web`, or `Telegram` for a Telegram session's owner turn.
    pub(crate) source: RunSource,
    pub(crate) source_ref: Option<String>,
}

/// What accepting a message did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AcceptedRun {
    /// A new queued run (202).
    Created(RunRecord),
    /// The key was used within 24 hours for this message: its run (200).
    Replayed(RunRecord),
    /// Joined the session's active run as a steer (202, spec §4.7): that run.
    Steered(RunRecord),
}

/// What a run's execution left in its steering (spec §4.7).
pub(super) struct SteerLeftovers {
    pub(super) run_id: String,
    pub(super) agent_id: String,
    pub(super) session_id: String,
    pub(super) room_id: String,
    /// What the run's closed inbox gave back.
    pub(super) unread: Vec<Content>,
    /// Keys of the steers its transcript took in: they stay on its record
    /// until its result is saved with them.
    pub(super) taken: HashSet<String>,
    /// Whether its control was cancelled (`cancel.is_cancelled()`), read as
    /// its execution ended. `hand_on_steers` also treats a stop saved on its
    /// record as one, read under the transaction.
    pub(super) stopped: bool,
}

/// Starts an accepted run given its id and resolves once the run is over;
/// `Err` says why it did not run.
pub(crate) type QueuedRunStart =
    Box<dyn FnOnce(String) -> BoxFuture<'static, Result<(), QueuedStartError>> + Send>;

/// Why an accepted run's start did not run it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QueuedStartError {
    /// It cannot run: its session queue settles it as `failed` with this
    /// message (or `cancelled` when it was stopped).
    Failed(String),
    /// What runs it is shutting down: it stays queued, unsaved, so the next
    /// start interrupts it as never started (spec §4.8).
    ShuttingDown,
}

pub(super) struct QueuedStart {
    run_id: String,
    /// The run's `createdAtMs`: when its message was accepted.
    accepted_at_ms: u64,
    start: QueuedRunStart,
}

/// The answer to a message whose key this agent used within the window
/// (spec §4.2): its run for the same session and text, else 409. It comes
/// before the session's capability checks, so a retried key keeps getting its
/// run after the session stopped taking messages (review Minor 6).
fn replayed(
    state: &DaemonState,
    agent_id: &str,
    session_id: &str,
    text: &str,
    idempotency_key: &str,
    now_ms: u64,
) -> Option<Result<AcceptedRun, ApiError>> {
    let original = state.runs.find_by_idempotency_key(
        agent_id,
        idempotency_key,
        now_ms.saturating_sub(IDEMPOTENCY_WINDOW_MS),
    )?;
    Some(
        if original.session_id == session_id && original.input.text == text {
            Ok(AcceptedRun::Replayed(
                state.with_live_tools(original.clone()),
            ))
        } else {
            Err(ApiError::conflict(IDEMPOTENCY_KEY_REUSED))
        },
    )
}

/// A reused key of a steer within the window (spec §4.2): the run it joined,
/// found in the ledger; a different session or text is a conflict. The run's transcript
/// is read only for a steer it already took in, and only its session's room
/// (audit M28).
fn steer_replay(
    state: &DaemonState,
    request: &AcceptRun,
    now_ms: u64,
) -> Result<Option<AcceptedRun>, ApiError> {
    let Some((run, pending)) = state.runs.find_steer(
        &request.agent_id,
        &request.idempotency_key,
        now_ms.saturating_sub(IDEMPOTENCY_WINDOW_MS),
    ) else {
        return Ok(None);
    };
    let same = run.session_id == request.session_id
        && match pending {
            Some(steer) => steer.text == request.text,
            // Its message may have left the hot tail; the key still names the run.
            None => recorded_steer_text(state, run, &request.idempotency_key)
                .is_none_or(|text| text == request.text),
        };
    if !same {
        return Err(ApiError::conflict(IDEMPOTENCY_KEY_REUSED));
    }
    Ok(Some(AcceptedRun::Replayed(
        state.with_live_tools(run.clone()),
    )))
}

/// The text of the steer `run` took in with `key`, from its session's room.
fn recorded_steer_text<'a>(state: &'a DaemonState, run: &RunRecord, key: &str) -> Option<&'a str> {
    let room_id = state
        .sessions
        .get(&run.agent_id, &run.session_id)?
        .room_id();
    state
        .agents
        .get(&run.agent_id)?
        .messages()
        .iter()
        .rev()
        .filter(|message| message.room_id == room_id)
        .find(|message| {
            is_steer_message(message)
                && metadata_text(&message.content, CLIENT_REQUEST_ID_METADATA_KEY) == Some(key)
                && metadata_text(&message.content, RUN_ID_METADATA_KEY) == Some(run.id.as_str())
        })
        .map(|message| message.content.text.as_str())
}

/// The session's running run a steer can join (spec §4.7): one whose stop
/// is not saved and whose inbox is still open, so its end hands the steer on.
fn joinable_run(state: &DaemonState, agent_id: &str, session_id: &str) -> Option<String> {
    state
        .runs
        .active_records()
        .into_iter()
        .filter(|record| {
            record.agent_id == agent_id
                && record.session_id == session_id
                && record.status.is_in_flight()
                && record.stop.is_none()
        })
        .find(|record| {
            state
                .live
                .runs()
                .control(&record.id)
                .is_some_and(|control| {
                    !control.cancel.is_cancelled() && !control.steering.is_closed()
                })
        })
        .map(|record| record.id.clone())
}

/// The steers `run_id` leaves behind, oldest first, taken off its record:
/// what its closed inbox gave back, and each saved steer its transcript did
/// not take in (one saved while the run was finishing found the inbox
/// closed, so only the record holds it).
fn take_leftover_steers(
    state: &mut DaemonState,
    run_id: &str,
    unread: Vec<Content>,
    taken: &HashSet<String>,
) -> Vec<Content> {
    let mut leftovers = unread;
    if let Some(record) = state.runs.get_mut(run_id) {
        let (kept, left): (Vec<_>, Vec<_>) = std::mem::take(&mut record.pending_steers)
            .into_iter()
            .partition(|steer| taken.contains(&steer.idempotency_key));
        record.pending_steers = kept;
        for steer in left {
            let unread = leftovers.iter().any(|content| {
                metadata_text(content, CLIENT_REQUEST_ID_METADATA_KEY)
                    == Some(steer.idempotency_key.as_str())
            });
            if !unread {
                leftovers.push(steer_content(&steer));
            }
        }
    }
    leftovers.sort_by_key(|content| accepted_at_ms(content).unwrap_or(u64::MAX));
    leftovers
}

/// Accepted runs of each session waiting for their turn, in acceptance
/// order. A session has an entry exactly while a drainer task works
/// through it.
pub(super) type SessionQueueMap = Arc<StdMutex<HashMap<(String, String), VecDeque<QueuedStart>>>>;

impl AgentRunCoordinator {
    /// Accepts a message into its session (spec §4.2): validates it, answers
    /// a reused key with its original run, and otherwise saves a `queued` run
    /// and appends it to the session's queue, all under the control-plane
    /// transaction, so acceptance order is execution order.
    pub(crate) async fn accept_run(
        &self,
        request: AcceptRun,
        start: QueuedRunStart,
    ) -> Result<AcceptedRun, ApiError> {
        let transaction = self.control_plane_transaction().await;
        let now_ms = now_millis();
        let (record, previous_title, persist) = {
            let mut guard = self.state.write().await;
            let Some(runtime) = guard.agents.get(&request.agent_id) else {
                return Err(ApiError::not_found());
            };
            if is_helper_config(runtime.config()) {
                return Err(ApiError::conflict(HELPER_MUST_RUN_THROUGH_COMPANION));
            }
            if self.is_being_deleted(&request.agent_id) {
                return Err(ApiError::conflict(super::AGENT_BEING_DELETED));
            }
            let model = runtime.config().model.clone();
            let provider = runtime.config().provider.clone();
            let Some(session) = guard.sessions.get(&request.agent_id, &request.session_id) else {
                return Err(ApiError::not_found());
            };
            if let Some(answer) = replayed(
                &guard,
                &request.agent_id,
                &request.session_id,
                &request.text,
                &request.idempotency_key,
                now_ms,
            ) {
                return answer;
            }
            if let Some(answer) = steer_replay(&guard, &request, now_ms)? {
                return Ok(answer);
            }
            let capabilities =
                session.capabilities(crate::sessions::views::automation_exists(&guard, session));
            let retitle = session.kind == SessionKind::Chat
                && session.title_source == TitleSource::FirstMessage
                && session.title == DEFAULT_CHAT_TITLE;
            if !capabilities.send {
                return Err(ApiError::conflict(SESSION_CANNOT_SEND));
            }
            if request.mode == SessionRunMode::Steer && !capabilities.steer {
                return Err(ApiError::bad_request_static(SESSION_CANNOT_STEER));
            }
            if request.mode == SessionRunMode::Steer {
                if let Some(run_id) = joinable_run(&guard, &request.agent_id, &request.session_id) {
                    // Saved with the run it joins (audit I3); nothing else is.
                    let steer = RunSteer {
                        idempotency_key: request.idempotency_key.clone(),
                        text: request.text.clone(),
                        accepted_at_ms: self.acceptance_clock.next(),
                    };
                    guard
                        .runs
                        .get_mut(&run_id)
                        .expect("the joinable run is in the ledger")
                        .pending_steers
                        .push(steer.clone());
                    let persist = guard.control_plane_persist_request();
                    drop(guard);
                    return self.join_steer(transaction, &run_id, steer, persist).await;
                }
                // Nothing to join, or the run is ending: the steer waits its
                // turn as a message.
            }
            if guard.runs.queued_count(&request.agent_id) >= MAX_QUEUED_RUNS_PER_AGENT {
                return Err(ApiError::too_many_requests(QUEUE_FULL));
            }
            let record = RunRecord::queued(
                RunStart {
                    agent_id: request.agent_id.clone(),
                    session_id: request.session_id.clone(),
                    source: request.source,
                    source_ref: request.source_ref.clone(),
                    idempotency_key: Some(request.idempotency_key.clone()),
                    text: request.text.clone(),
                    model,
                    provider,
                    parent_run_id: None,
                },
                self.acceptance_clock.next(),
            );
            // A new chat shows its first message's title from the moment it is
            // accepted, and keeps it if the run fails (M2 T17 Minor 14).
            let previous_title = retitle
                .then(|| derived_title(&request.text))
                .flatten()
                .and_then(|title| {
                    guard
                        .sessions
                        .get_mut(&request.agent_id, &request.session_id)
                        .map(|session| std::mem::replace(&mut session.title, title))
                });
            guard.runs.insert(record.clone());
            (
                record,
                previous_title,
                guard.control_plane_persist_request(),
            )
        };
        if let Err(error) = persist.save().await {
            let mut guard = self.state.write().await;
            guard.runs.remove(&record.id);
            let mut title_reverted = false;
            if let Some(title) = previous_title {
                if let Some(session) = guard.sessions.get_mut(&record.agent_id, &record.session_id)
                {
                    session.title = title;
                    title_reverted = true;
                }
            }
            // A stream opened during the save listed this queued run in its
            // snapshot; this ends it there. Published under the state lock,
            // after the removal, so every stream whose snapshot held the run
            // hears it (as for a failed start save).
            let mut failed = record;
            failed.finish(
                RunStatus::Failed,
                Some(RunError::new(COMMIT_FAILED, error.to_string())),
                now_millis(),
            );
            let parent = guard.live_parent_agent(&failed.agent_id, &failed.session_id);
            guard
                .live
                .publish(run_status_event(&failed), parent.as_deref());
            if title_reverted {
                guard.publish_session_event(
                    &failed.agent_id,
                    &failed.session_id,
                    LiveEventBody::SessionUpdated,
                );
            }
            return Err(ApiError::service_unavailable(error.to_string()));
        }
        {
            let guard = self.state.read().await;
            // Registered now, so a stop can reach the run while it waits. A
            // new run id gets a new control: no two runs share one (M3 Task 2).
            guard.live.runs().register(&record.id);
            let parent = guard.live_parent_agent(&record.agent_id, &record.session_id);
            guard
                .live
                .publish(run_status_event(&record), parent.as_deref());
            if previous_title.is_some() {
                guard.publish_session_event(
                    &record.agent_id,
                    &record.session_id,
                    LiveEventBody::SessionUpdated,
                );
            }
        }
        self.enqueue(&record, start);
        drop(transaction);
        Ok(AcceptedRun::Created(record))
    }

    /// Saves a steer with the run it joined, then hands it to that run's
    /// inbox (spec §4.7): durable before the run can read it. An inbox that
    /// closed meanwhile refuses it; the run's end, which waits for this
    /// transaction, then finds it on the record and hands it on.
    async fn join_steer(
        &self,
        transaction: OwnedMutexGuard<()>,
        run_id: &str,
        steer: RunSteer,
        persist: ControlPlanePersistRequest,
    ) -> Result<AcceptedRun, ApiError> {
        if let Err(error) = persist.save().await {
            let mut guard = self.state.write().await;
            if let Some(record) = guard.runs.get_mut(run_id) {
                record
                    .pending_steers
                    .retain(|pending| pending.idempotency_key != steer.idempotency_key);
            }
            return Err(ApiError::service_unavailable(error.to_string()));
        }
        let joined = {
            let guard = self.state.read().await;
            if let Some(control) = guard.live.runs().control(run_id) {
                let _ = control.steering.push(steer_content(&steer));
            }
            guard
                .runs
                .get(run_id)
                .map(|record| guard.with_live_tools(record.clone()))
        };
        drop(transaction);
        // Its record goes only under the transaction, held until here.
        joined
            .map(AcceptedRun::Steered)
            .ok_or_else(ApiError::not_found)
    }

    /// Hands on the steers a run's execution left behind (spec §4.7) with one
    /// save, in which each leaves the run's record and becomes a run of its
    /// own: after a stop, an `interrupted` run to send again (audit M8, a
    /// deliberate deviation from §4.7, which requeues them); otherwise a
    /// queued web run at its acceptance time, within the agent's cap of
    /// queued messages, and past the cap an `interrupted` run to send again
    /// (audit M9). Nothing unsaved is announced (spec §6).
    pub(super) async fn hand_on_steers(&self, left: SteerLeftovers) {
        let transaction = self.control_plane_transaction().await;
        let (records, parent, hub, persist) = {
            let mut guard = self.state.write().await;
            // Read under the transaction: a stop whose save was still in
            // progress when the run's execution ended counts as a stop.
            let stopped = left.stopped
                || guard
                    .runs
                    .get(&left.run_id)
                    .is_some_and(|record| record.stop.is_some());
            let steers = take_leftover_steers(&mut guard, &left.run_id, left.unread, &left.taken);
            if steers.is_empty() {
                return;
            }
            let Some(runtime) = guard.agents.get(&left.agent_id) else {
                return;
            };
            let model = runtime.config().model.clone();
            let provider = runtime.config().provider.clone();
            let now_ms = now_millis();
            let mut open_slots =
                MAX_QUEUED_RUNS_PER_AGENT.saturating_sub(guard.runs.queued_count(&left.agent_id));
            let records = steers
                .into_iter()
                .map(|content| {
                    let accepted_at_ms = accepted_at_ms(&content).unwrap_or(now_ms);
                    let idempotency_key =
                        metadata_text(&content, CLIENT_REQUEST_ID_METADATA_KEY).map(str::to_string);
                    let mut record = RunRecord::queued(
                        RunStart {
                            agent_id: left.agent_id.clone(),
                            session_id: left.session_id.clone(),
                            source: RunSource::Web,
                            source_ref: None,
                            idempotency_key,
                            text: content.text,
                            model: model.clone(),
                            provider: provider.clone(),
                            parent_run_id: None,
                        },
                        accepted_at_ms,
                    );
                    let refused = if stopped {
                        Some((STOPPED_BEFORE_START, STOPPED_BEFORE_START_MESSAGE))
                    } else if open_slots == 0 {
                        Some((QUEUE_FULL_BEFORE_START, QUEUE_FULL_BEFORE_START_MESSAGE))
                    } else {
                        open_slots -= 1;
                        None
                    };
                    if let Some((code, message)) = refused {
                        record.finish(
                            RunStatus::Interrupted,
                            Some(RunError::new(code, message)),
                            now_ms,
                        );
                    }
                    record
                })
                .collect::<Vec<_>>();
            for record in &records {
                guard.runs.insert(record.clone());
                if record.status == RunStatus::Queued {
                    // Registered now, so a stop reaches it while it waits.
                    guard.live.runs().register(&record.id);
                }
            }
            let parent = guard.live_parent_agent(&left.agent_id, &left.session_id);
            (
                records,
                parent,
                guard.live.clone(),
                guard.control_plane_persist_request(),
            )
        };
        let saved = match persist.save().await {
            Ok(()) => true,
            Err(error) => {
                // Kept for the next save (the run's own result save, right
                // after this); clients read them from the ledger once the
                // run's end refreshes it. The queued ones still run, and
                // their start save persists them. A restart first offers the
                // steers again from the run's saved record.
                error!(agent_id = %left.agent_id, session_id = %left.session_id, error = %error, "could not save the steers a run left behind");
                false
            }
        };
        for record in &records {
            if saved {
                hub.publish(run_status_event(record), parent.as_deref());
            }
            if record.status == RunStatus::Queued {
                let key = record
                    .idempotency_key
                    .clone()
                    .unwrap_or_else(|| record.id.clone());
                let start = self.web_start(
                    left.agent_id.clone(),
                    left.room_id.clone(),
                    record.input.text.clone(),
                    key,
                );
                self.enqueue(record, start);
            }
        }
        drop(transaction);
    }

    /// Makes the wall clock behind acceptance times read from `wall`.
    #[cfg(test)]
    pub(crate) fn set_wall_clock_for_test(&self, wall: impl FnMut() -> u64 + Send + 'static) {
        *self
            .acceptance_clock
            .wall
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(wall));
    }

    /// `accept_run`'s answer for a key already used, accepting nothing: for a
    /// caller that refuses a new message before it reaches `accept_run` (the
    /// route's Telegram session without its active connector).
    pub(crate) async fn replayed_run(
        &self,
        agent_id: &str,
        session_id: &str,
        text: &str,
        idempotency_key: &str,
    ) -> Option<Result<AcceptedRun, ApiError>> {
        let guard = self.state.read().await;
        replayed(
            &guard,
            agent_id,
            session_id,
            text,
            idempotency_key,
            now_millis(),
        )
    }

    /// The start of an accepted web message: the owner's turn in the
    /// session's room, run by this coordinator.
    pub(crate) fn web_start(
        &self,
        agent_id: String,
        room_id: String,
        text: String,
        idempotency_key: String,
    ) -> QueuedRunStart {
        let coordinator = self.clone();
        Box::new(
            move |run_id: String| -> BoxFuture<'static, Result<(), QueuedStartError>> {
                Box::pin(async move {
                    let request = AgentRunRequest {
                        agent_id,
                        content: Content {
                            text,
                            attachments: None,
                            metadata: Some(BTreeMap::from([(
                                CLIENT_REQUEST_ID_METADATA_KEY.to_string(),
                                DataValue::String(idempotency_key.clone()),
                            )])),
                        },
                        room: RunRoom::Stable(room_id),
                        idempotency_key: Some(idempotency_key),
                        source: RunSource::Web,
                        source_ref: None,
                        parent: None,
                    };
                    coordinator
                        .run_accepted(request, run_id)
                        .await
                        .map(|_| ())
                        .map_err(|error| QueuedStartError::Failed(error.message().to_string()))
                })
            },
        )
    }

    /// Adds an accepted run to its session's queue by acceptance time, so a
    /// steer that becomes a queued message later keeps its place.
    fn enqueue(&self, record: &RunRecord, start: QueuedRunStart) {
        let key = (record.agent_id.clone(), record.session_id.clone());
        let next = QueuedStart {
            run_id: record.id.clone(),
            accepted_at_ms: record.created_at_ms,
            start,
        };
        let spawn_drainer = {
            let mut queues = self
                .session_queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match queues.entry(key.clone()) {
                Entry::Occupied(mut queue) => {
                    let queue = queue.get_mut();
                    let position = queue
                        .iter()
                        .position(|item| item.accepted_at_ms > next.accepted_at_ms)
                        .unwrap_or(queue.len());
                    queue.insert(position, next);
                    false
                }
                Entry::Vacant(slot) => {
                    slot.insert(VecDeque::from([next]));
                    true
                }
            }
        };
        if spawn_drainer {
            let coordinator = self.clone();
            tokio::spawn(async move { coordinator.drain_session(key).await });
        }
    }

    /// Whether a drainer is still working through this session's queue.
    #[cfg(test)]
    pub(crate) fn has_session_queue(&self, agent_id: &str, session_id: &str) -> bool {
        self.session_queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&(agent_id.to_string(), session_id.to_string()))
    }

    /// Starts a session's accepted runs one after another, in acceptance order.
    async fn drain_session(&self, key: (String, String)) {
        loop {
            let next = {
                let mut queues = self
                    .session_queues
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(queue) = queues.get_mut(&key) else {
                    return;
                };
                match queue.pop_front() {
                    Some(next) => next,
                    None => {
                        queues.remove(&key);
                        return;
                    }
                }
            };
            // Looked at under the control-plane transaction, so a stop or an
            // agent deletion whose save is still in flight is never acted on:
            // were that save to fail and be put back, a run dropped here
            // would stay queued with nothing left to start it.
            let still_queued = {
                let _transaction = self.control_plane_transaction().await;
                let guard = self.state.read().await;
                let queued = guard
                    .runs
                    .get(&next.run_id)
                    .is_some_and(|record| record.status == RunStatus::Queued);
                if !queued {
                    // Stopped, or settled some other way, while it waited.
                    guard.live.runs().remove(&next.run_id);
                }
                queued
            };
            if !still_queued {
                continue;
            }
            // Its own task, so a panic cannot take the session's queue with it.
            let failure = match tokio::spawn((next.start)(next.run_id.clone())).await {
                Ok(Ok(())) => None,
                // Left as it is: settling or saving it now would turn a run
                // that never started into a failure (review Minor 4).
                Ok(Err(QueuedStartError::ShuttingDown)) => None,
                Ok(Err(QueuedStartError::Failed(message))) => Some(message),
                Err(_) => Some(RUN_ENDED_BEFORE_START.to_string()),
            };
            if let Some(message) = failure {
                let stopped = self
                    .state
                    .read()
                    .await
                    .live
                    .runs()
                    .control(&next.run_id)
                    .is_some_and(|control| control.cancel.is_cancelled());
                let (status, error) = if stopped {
                    (
                        RunStatus::Cancelled,
                        RunError::new(RUN_STOPPED, STOPPED_BY_OWNER),
                    )
                } else {
                    (RunStatus::Failed, RunError::new(RUN_FAILED, message))
                };
                self.settle_unstarted(&next.run_id, status, error).await;
            }
        }
    }

    /// Finishes an accepted run that never started (stopped while it waited,
    /// or refused when its turn came), saves that, and announces it. A run
    /// that already left `queued` keeps its state; a settled one only loses
    /// its registered control.
    pub(crate) async fn settle_unstarted(&self, run_id: &str, status: RunStatus, error: RunError) {
        let transaction = self.control_plane_transaction().await;
        let (record, parent, hub, persist) = {
            let mut guard = self.state.write().await;
            match guard.runs.get(run_id).map(|record| record.status) {
                Some(RunStatus::Queued) => {}
                // It started after all; its run owns its control.
                Some(current) if !current.is_terminal() => return,
                _ => {
                    // Already settled (a stop marks a queued run cancelled
                    // itself) or gone: only its control is left to forget.
                    guard.live.runs().remove(run_id);
                    return;
                }
            }
            let record = guard
                .runs
                .get_mut(run_id)
                .expect("the run is queued, checked above");
            record.finish(status, Some(error), now_millis());
            let record = record.clone();
            // It will never run: its control goes with the queued state.
            guard.live.runs().remove(&record.id);
            let parent = guard.live_parent_agent(&record.agent_id, &record.session_id);
            (
                record,
                parent,
                guard.live.clone(),
                guard.control_plane_persist_request(),
            )
        };
        if let Err(error) = persist.save().await {
            // Settled in memory; the next save persists it, and a restart
            // before that interrupts it as never started.
            warn!(run_id = %record.id, error = %error, "could not save an unstarted run's outcome");
        }
        drop(transaction);
        hub.publish(run_status_event(&record), parent.as_deref());
    }
}
