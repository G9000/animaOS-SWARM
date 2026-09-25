//! Accepted runs (spec §4.2–§4.3): acceptance under the control-plane
//! transaction with one save, then per-session execution in acceptance
//! order. The order is fixed where the run is accepted, not where a task
//! happens to be scheduled (M1 F15).

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};

use anima_core::primitives::now_millis;
use anima_core::{Content, DataValue};
use futures::future::BoxFuture;
use tracing::warn;

use super::{
    is_helper_config, AgentRunCoordinator, AgentRunRequest, RunRoom,
    HELPER_MUST_RUN_THROUGH_COMPANION, MAX_QUEUED_RUNS_PER_AGENT,
};
use crate::live::{run_status_event, LiveEventBody};
use crate::routes::ApiError;
use crate::runs::{
    RunError, RunRecord, RunSource, RunStart, RunStatus, IDEMPOTENCY_WINDOW_MS, RUN_FAILED,
    RUN_STOPPED, STOPPED_BY_OWNER,
};
use crate::sessions::{derived_title, SessionKind, TitleSource, DEFAULT_CHAT_TITLE};

pub(crate) const SESSION_CANNOT_SEND: &str = "This session cannot receive messages";
pub(crate) const SESSION_CANNOT_STEER: &str = "This session cannot be steered";
pub(crate) const IDEMPOTENCY_KEY_REUSED: &str =
    "Idempotency-Key was already used for a different message";
pub(crate) const QUEUE_FULL: &str =
    "This companion already has 8 queued messages; wait for one to start";
pub(crate) const RUN_NOT_QUEUED: &str = "This run is no longer waiting to start";
pub(crate) const RUN_STOPPED_BEFORE_START: &str = "This run was stopped before it started";
const RUN_ENDED_BEFORE_START: &str = "The run stopped unexpectedly before it started";

/// How a message joins its session (spec §4.2, §4.7).
#[allow(dead_code)] // The session runs route (next commit) builds `Queue`.
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
}

/// Starts an accepted run given its id and resolves once the run is over;
/// `Err` carries why it could not run.
pub(crate) type QueuedRunStart =
    Box<dyn FnOnce(String) -> BoxFuture<'static, Result<(), String>> + Send>;

pub(super) struct QueuedStart {
    run_id: String,
    /// The run's `createdAtMs`: when its message was accepted.
    accepted_at_ms: u64,
    start: QueuedRunStart,
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
    #[allow(dead_code)] // The session runs route (next commit) accepts messages.
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
            let model = runtime.config().model.clone();
            let provider = runtime.config().provider.clone();
            let Some(session) = guard.sessions.get(&request.agent_id, &request.session_id) else {
                return Err(ApiError::not_found());
            };
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
            if let Some(original) = guard.runs.find_by_idempotency_key(
                &request.agent_id,
                &request.idempotency_key,
                now_ms.saturating_sub(IDEMPOTENCY_WINDOW_MS),
            ) {
                return if original.session_id == request.session_id
                    && original.input.text == request.text
                {
                    Ok(AcceptedRun::Replayed(
                        guard.with_live_tools(original.clone()),
                    ))
                } else {
                    Err(ApiError::conflict(IDEMPOTENCY_KEY_REUSED))
                };
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
                now_ms,
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
            if let Some(title) = previous_title {
                if let Some(session) = guard.sessions.get_mut(&record.agent_id, &record.session_id)
                {
                    session.title = title;
                }
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

    /// The start of an accepted web message: the owner's turn in the
    /// session's room, run by this coordinator.
    #[allow(dead_code)] // The session runs route (next commit) starts web messages.
    pub(crate) fn web_start(
        &self,
        agent_id: String,
        room_id: String,
        text: String,
        idempotency_key: String,
    ) -> QueuedRunStart {
        let coordinator = self.clone();
        Box::new(
            move |run_id: String| -> BoxFuture<'static, Result<(), String>> {
                Box::pin(async move {
                    let request = AgentRunRequest {
                        agent_id,
                        content: Content {
                            text,
                            attachments: None,
                            metadata: Some(BTreeMap::from([(
                                "clientRequestId".to_string(),
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
                        .map_err(|error| error.message().to_string())
                })
            },
        )
    }

    /// Adds an accepted run to its session's queue by acceptance time, so a
    /// steer that becomes a queued message later (Task 9) keeps its place.
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
            let still_queued = self
                .state
                .read()
                .await
                .runs
                .get(&next.run_id)
                .is_some_and(|record| record.status == RunStatus::Queued);
            if !still_queued {
                // Stopped, or settled some other way, while it waited.
                self.state.read().await.live.runs().remove(&next.run_id);
                continue;
            }
            // Its own task, so a panic cannot take the session's queue with it.
            let failure = match tokio::spawn((next.start)(next.run_id.clone())).await {
                Ok(Ok(())) => None,
                Ok(Err(message)) => Some(message),
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
