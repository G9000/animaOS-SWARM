use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anima_core::{Content, DataValue, TaskStatus};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
use crate::app::SharedDaemonState;
use crate::connectors::runtime::{ConnectorManager, ConnectorRuntimeStatus};
use crate::connectors::{OutboundDeliveryState, TelegramOutboundRecord};
use crate::routes::ApiError;
use crate::runs::{RunOutcome, RunSource, RunStatus};
use crate::state::OutcomeUndo;

pub(crate) mod cron;
pub(crate) mod timing;
pub(crate) use timing::ActiveHours;
pub(crate) mod automations;
pub(crate) mod history;
#[cfg(test)]
pub(crate) use automations::test_automation;
pub(crate) use automations::{
    display_name, heartbeat_input, preview, validate_stored_automation, AutomationCounters,
    AutomationCreator, AutomationInput, AutomationPatch, AutomationPreset, AutomationService,
    AUTOMATION_ALREADY_RUNNING, AUTOMATION_HISTORY_UNAVAILABLE, MAX_AUTOMATION_HISTORY_SHOWN,
    PROMPT_AND_TRIGGER_REQUIRED, TOO_MANY_RUNNING_AUTOMATIONS,
};
#[cfg(test)]
pub(crate) use automations::{
    AGENT_AUTOMATION_TOO_FREQUENT, HEARTBEAT_NEEDS_TIME_ZONE, MAX_AUTOMATIONS_PER_AGENT,
    TOO_MANY_AUTOMATIONS,
};
pub(crate) use history::{FireLog, ScheduleFireRecord};

const CHECKIN_SENTINEL: &str = "CHECKIN_OK";
const CHECKIN_SUFFIX: &str = "(This is a scheduled check-in. If you have nothing worth saying right now, reply with exactly CHECKIN_OK and nothing else.)";
const MAX_PROMPT_BYTES: usize = 32 * 1024;
const WORKER_TICK: Duration = Duration::from_millis(250);
const MAX_ACTIVE_SCHEDULES: usize = 8;
static NEXT_SCHEDULE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduledPromptRecord {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) import_idempotency_key: Option<String>,
    pub(crate) agent_id: String,
    pub(crate) prompt: String,
    pub(crate) trigger: ScheduleTrigger,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    pub(crate) target: ScheduleTarget,
    pub(crate) next_due_at_ms: u64,
    #[serde(default)]
    pub(crate) last_fired: Option<ScheduleLastFired>,
    #[serde(default)]
    pub(crate) last_safe_outcome: Option<ScheduleSafeOutcome>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    /// Spec §9.1. Empty for records saved before M6 (`display_name` reads the
    /// prompt's first line for those).
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) active_hours: Option<ActiveHours>,
    #[serde(default)]
    pub(crate) created_by: AutomationCreator,
    #[serde(default)]
    pub(crate) preset: Option<AutomationPreset>,
    #[serde(default)]
    pub(crate) counters: AutomationCounters,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ScheduleTrigger {
    Interval {
        #[serde(rename = "intervalMs")]
        interval_ms: u64,
    },
    Daily {
        hour: u8,
        minute: u8,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Five-field cron on `time_zone`'s wall clock (spec §9.1).
    Cron {
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Fires once; the claim turns the automation off (spec §9.1).
    Once {
        #[serde(rename = "atMs")]
        at_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ScheduleTarget {
    Workspace,
    Connector {
        #[serde(rename = "connectorId")]
        connector_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleLastFired {
    pub(crate) fired_at_ms: u64,
    pub(crate) run_idempotency_key: String,
    /// Run now (spec §9.2) fired it, not the trigger.
    #[serde(default)]
    pub(crate) manual: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleSafeOutcome {
    pub(crate) status: ScheduleOutcomeStatus,
    pub(crate) occurred_at_ms: u64,
    #[serde(default)]
    pub(crate) error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ScheduleOutcomeStatus {
    Silent,
    Spoke,
    Failed,
    /// The owner stopped the run; the schedule stays enabled (spec §4.6).
    Stopped,
}

impl ScheduleOutcomeStatus {
    /// The name clients see (`error` for a failure, as before M3).
    pub(crate) const fn contract_name(&self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Spoke => "spoke",
            Self::Failed => "error",
            Self::Stopped => "stopped",
        }
    }
}

/// A check-in run's outcome (spec §4.6, §9.2).
pub(crate) fn checkin_outcome_status(outcome: &RunOutcome) -> ScheduleOutcomeStatus {
    if outcome.status == RunStatus::Cancelled {
        ScheduleOutcomeStatus::Stopped
    } else if outcome.result.status == TaskStatus::Error {
        ScheduleOutcomeStatus::Failed
    } else if outcome
        .result
        .data
        .as_ref()
        .is_some_and(|content| is_silent_checkin_reply(&content.text))
    {
        ScheduleOutcomeStatus::Silent
    } else {
        ScheduleOutcomeStatus::Spoke
    }
}

/// The error code a check-in outcome records.
pub(crate) fn checkin_error_code(status: &ScheduleOutcomeStatus) -> Option<String> {
    match status {
        ScheduleOutcomeStatus::Failed => Some("schedule_run_failed".into()),
        ScheduleOutcomeStatus::Stopped => Some("schedule_run_stopped".into()),
        ScheduleOutcomeStatus::Silent | ScheduleOutcomeStatus::Spoke => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleError {
    AgentNotFound,
    NotFound,
    Invalid(&'static str),
    /// A trigger or active hours the daemon refuses, with a built message (400).
    Rejected(String),
    /// The change conflicts with the automation's state or a limit (409).
    Conflict(&'static str),
    /// The scheduler is at its admission cap (429).
    Busy(&'static str),
    /// The history store could not be read (503).
    HistoryUnavailable,
    TargetUnavailable,
    Persistence,
}

#[derive(Clone)]
pub(crate) struct SchedulerService {
    inner: Arc<SchedulerInner>,
    worker: Arc<Mutex<Option<SchedulerWorker>>>,
}

struct SchedulerInner {
    state: SharedDaemonState,
    runs: AgentRunCoordinator,
    connectors: ConnectorManager,
    // One live run per automation (spec §4.3); a job owns its entry until the
    // detached agent run and durable commit finish.
    //
    // Lock order (controller ruling 4): `jobs` comes before the control-plane
    // transaction. The tick and Run now take `jobs` first, then the
    // transaction; nothing may take `jobs` while holding the transaction, or
    // the two would deadlock.
    jobs: Mutex<BTreeMap<String, JoinHandle<()>>>,
}

struct SchedulerWorker {
    cancel: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl SchedulerService {
    pub(crate) fn new(
        state: SharedDaemonState,
        runs: AgentRunCoordinator,
        connectors: ConnectorManager,
    ) -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                state,
                runs,
                connectors,
                jobs: Mutex::new(BTreeMap::new()),
            }),
            worker: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) async fn start(&self) {
        let mut worker = self.worker.lock().await;
        if worker.is_some() {
            return;
        }
        let (cancel, mut cancelled) = watch::channel(false);
        let inner = Arc::clone(&self.inner);
        let join = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancelled.changed() => {
                        if *cancelled.borrow() { break; }
                    }
                    _ = tokio::time::sleep(WORKER_TICK) => {
                        if let Err(error) = SchedulerService::tick_inner(&inner, now_ms()).await {
                            tracing::warn!(?error, "schedule recovery or admission failed; retrying next tick");
                        }
                    }
                }
            }
            drain_jobs(&inner).await;
        });
        *worker = Some(SchedulerWorker { cancel, join });
    }

    pub(crate) async fn shutdown(&self) {
        let mut slot = self.worker.lock().await;
        if let Some(worker) = slot.take() {
            let _ = worker.cancel.send(true);
            let _ = worker.join.await;
        }
    }

    /// Owner and companion changes to automations (spec §9).
    pub(crate) fn automations(&self) -> AutomationService {
        AutomationService::new(
            Arc::clone(&self.inner.state),
            self.inner.runs.control_plane_transactions(),
        )
    }

    pub(crate) async fn list(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ScheduledPromptRecord>, ScheduleError> {
        self.automations().list(agent_id).await
    }

    pub(crate) async fn create(
        &self,
        agent_id: String,
        prompt: String,
        trigger: ScheduleTrigger,
        target: ScheduleTarget,
        enabled: bool,
        import_idempotency_key: Option<String>,
        explicit_next_due_at_ms: Option<u64>,
        created_at_override_ms: Option<u64>,
    ) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        let mut input = AutomationInput::owner(agent_id, prompt, trigger, target);
        input.enabled = enabled;
        input.import_idempotency_key = import_idempotency_key;
        input.explicit_next_due_at_ms = explicit_next_due_at_ms;
        input.created_at_override_ms = created_at_override_ms;
        self.automations().create(input, now_ms()).await
    }

    #[cfg(test)]
    pub(crate) async fn update(
        &self,
        agent_id: &str,
        schedule_id: &str,
        prompt: Option<String>,
        trigger: Option<ScheduleTrigger>,
        target: Option<ScheduleTarget>,
        enabled: Option<bool>,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        let patch = AutomationPatch {
            prompt,
            trigger,
            target,
            enabled,
            ..AutomationPatch::default()
        };
        self.automations()
            .update(agent_id, schedule_id, patch, now_ms())
            .await
    }

    pub(crate) async fn delete(
        &self,
        agent_id: &str,
        schedule_id: &str,
    ) -> Result<(), ScheduleError> {
        self.automations().delete(agent_id, schedule_id).await
    }

    #[cfg(test)]
    pub(crate) async fn tick_at(&self, now: u64) -> Result<usize, ScheduleError> {
        let result = Self::tick_inner(&self.inner, now).await;
        drain_jobs(&self.inner).await;
        result
    }

    /// Awaits every job (tests drive Run now without the worker loop).
    #[cfg(test)]
    pub(crate) async fn drain(&self) {
        drain_jobs(&self.inner).await;
    }

    /// Fires an automation now (spec §9.2). Its due time and switch stay,
    /// one run per automation and the scheduler's admission cap still hold,
    /// and the fire is recorded as manual. The claim and the job's start run
    /// in their own task, so a dropped request still finishes them.
    pub(crate) async fn run_now(
        &self,
        agent_id: &str,
        schedule_id: &str,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        let inner = Arc::clone(&self.inner);
        let (agent_id, schedule_id) = (agent_id.to_string(), schedule_id.to_string());
        tokio::spawn(async move { run_now_inner(&inner, &agent_id, &schedule_id, now_ms()).await })
            .await
            .unwrap_or(Err(ScheduleError::Persistence))
    }

    async fn tick_inner(inner: &Arc<SchedulerInner>, now: u64) -> Result<usize, ScheduleError> {
        let mut jobs = inner.jobs.lock().await;
        reap_finished(&mut jobs).await;
        // Reconcile only jobs with no live owner, including failures after startup.
        // Persistence failure closes admission for this tick; the next tick retries.
        let active_schedules = jobs.keys().cloned().collect();
        reconcile_interrupted(inner, now, &active_schedules).await?;
        let due_ids = {
            let state = inner.state.read().await;
            let mut ids = state
                .schedules
                .values()
                .filter(|item| {
                    item.enabled && item.next_due_at_ms <= now && !unresolved_occurrence(item)
                })
                .map(|item| (item.next_due_at_ms, item.id.clone()))
                .collect::<Vec<_>>();
            ids.sort();
            ids
        };
        let mut claimed = 0;
        for (_, id) in due_ids {
            if jobs.len() >= MAX_ACTIVE_SCHEDULES {
                break;
            }
            if jobs.contains_key(&id) {
                continue;
            }
            // The claim is durable before the run waits for its room, slot, and
            // permit. Two automations sharing a room (one connector's chat, say)
            // are both claimed and one may wait on the room lock; a restart in
            // that window auto-disables the waiting occurrence under the
            // interrupted-schedule rule. Accepted for M1.
            if let Some(record) = claim_due(inner, &id, now).await? {
                claimed += 1;
                let inner = inner.clone();
                jobs.insert(
                    id,
                    tokio::spawn(async move {
                        execute_claimed(&inner, record, now).await;
                    }),
                );
            }
        }
        Ok(claimed)
    }
}

async fn drain_jobs(inner: &Arc<SchedulerInner>) {
    let mut jobs = inner.jobs.lock().await;
    for (_, job) in std::mem::take(&mut *jobs) {
        if let Err(error) = job.await {
            tracing::warn!(?error, "scheduled worker stopped during shutdown");
        }
    }
}

/// Awaits the jobs that finished, so their automations can run again.
async fn reap_finished(jobs: &mut BTreeMap<String, JoinHandle<()>>) {
    let finished = jobs
        .iter()
        .filter(|(_, job)| job.is_finished())
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in finished {
        if let Some(job) = jobs.remove(&id) {
            if let Err(error) = job.await {
                tracing::warn!(?error, "scheduled worker stopped unexpectedly");
            }
        }
    }
}

async fn run_now_inner(
    inner: &Arc<SchedulerInner>,
    agent_id: &str,
    schedule_id: &str,
    now: u64,
) -> Result<ScheduledPromptRecord, ScheduleError> {
    let owned = inner
        .state
        .read()
        .await
        .schedules
        .get(schedule_id)
        .is_some_and(|record| record.agent_id == agent_id);
    if !owned {
        return Err(ScheduleError::NotFound);
    }
    // The scheduler's lock order (controller ruling 4): `jobs` first, then
    // the control-plane transaction (in `claim_manual`). Nothing takes `jobs`
    // while holding the transaction.
    let mut jobs = inner.jobs.lock().await;
    reap_finished(&mut jobs).await;
    if jobs.contains_key(schedule_id) {
        return Err(ScheduleError::Conflict(AUTOMATION_ALREADY_RUNNING));
    }
    if jobs.len() >= MAX_ACTIVE_SCHEDULES {
        return Err(ScheduleError::Busy(TOO_MANY_RUNNING_AUTOMATIONS));
    }
    let record = claim_manual(inner, agent_id, schedule_id, now).await?;
    let job = {
        let (inner, record) = (Arc::clone(inner), record.clone());
        tokio::spawn(async move { execute_claimed(&inner, record, now).await })
    };
    jobs.insert(schedule_id.to_string(), job);
    Ok(record)
}

/// Saves a manual occurrence's claim: `lastFired` (marked manual) and no
/// outcome yet; the due time and the switch stay.
async fn claim_manual(
    inner: &Arc<SchedulerInner>,
    agent_id: &str,
    schedule_id: &str,
    now: u64,
) -> Result<ScheduledPromptRecord, ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (claimed, previous, persist) = {
        let mut state = inner.state.write().await;
        let previous = state
            .schedules
            .get(schedule_id)
            .filter(|record| record.agent_id == agent_id)
            .cloned()
            .ok_or(ScheduleError::NotFound)?;
        if unresolved_occurrence(&previous) {
            return Err(ScheduleError::Conflict(AUTOMATION_ALREADY_RUNNING));
        }
        let mut claimed = previous.clone();
        claimed.last_fired = Some(ScheduleLastFired {
            fired_at_ms: now,
            run_idempotency_key: format!("schedule:{}:manual:{now}", claimed.id),
            manual: true,
        });
        claimed.last_safe_outcome = None;
        claimed.updated_at_ms = now.max(claimed.created_at_ms).max(previous.updated_at_ms);
        state
            .schedules
            .insert(schedule_id.to_string(), claimed.clone());
        (claimed, previous, state.control_plane_persist_request())
    };
    if persist.save().await.is_err() {
        inner
            .state
            .write()
            .await
            .schedules
            .insert(schedule_id.to_string(), previous);
        return Err(ScheduleError::Persistence);
    }
    inner
        .state
        .read()
        .await
        .publish_automation_updated(agent_id, schedule_id, false);
    Ok(claimed)
}

fn unresolved_occurrence(record: &ScheduledPromptRecord) -> bool {
    record.last_fired.as_ref().is_some_and(|fired| {
        record
            .last_safe_outcome
            .as_ref()
            .is_none_or(|outcome| outcome.occurred_at_ms < fired.fired_at_ms)
    })
}

/// Its latest occurrence has no outcome yet: it is running, or a restart
/// interrupted it and the next tick will record that.
pub(crate) fn is_running(record: &ScheduledPromptRecord) -> bool {
    unresolved_occurrence(record)
}

async fn reconcile_interrupted(
    inner: &Arc<SchedulerInner>,
    now: u64,
    active_schedules: &BTreeSet<String>,
) -> Result<(), ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (changes, persist) = {
        let mut state = inner.state.write().await;
        // Occurrences whose run the owner stopped before a restart could
        // record its outcome: the saved stop survives the restart on the
        // interrupted run, and a stop keeps the schedule enabled (spec §4.6,
        // audit M24).
        let unresolved = state
            .schedules
            .values()
            .filter(|s| !active_schedules.contains(&s.id) && unresolved_occurrence(s))
            .map(|s| {
                let fired = s
                    .last_fired
                    .as_ref()
                    .expect("an unresolved occurrence fired");
                let run = state
                    .runs
                    .find_by_idempotency_key(&s.agent_id, &fired.run_idempotency_key, 0)
                    .filter(|run| {
                        run.source == RunSource::Schedule
                            && run.source_ref.as_deref() == Some(s.id.as_str())
                    });
                (
                    s.id.clone(),
                    s.agent_id.clone(),
                    fired.fired_at_ms,
                    run.is_some_and(|run| run.stop.is_some()),
                    run.map(|run| (run.id.clone(), run.session_id.clone())),
                )
            })
            .collect::<Vec<_>>();
        if unresolved.is_empty() {
            return Ok(());
        }
        let mut changes = Vec::new();
        for (id, agent_id, fired_at_ms, stopped, run) in unresolved {
            let previous = state.schedules[&id].clone();
            let occurred_at_ms = now.max(fired_at_ms);
            let outcome = if stopped {
                let status = ScheduleOutcomeStatus::Stopped;
                ScheduleSafeOutcome {
                    error_code: checkin_error_code(&status),
                    status,
                    occurred_at_ms,
                }
            } else {
                ScheduleSafeOutcome {
                    status: ScheduleOutcomeStatus::Failed,
                    occurred_at_ms,
                    error_code: Some("schedule_run_interrupted".into()),
                }
            };
            // Spec §9.1: the outcome, the counters, and a fire record.
            let undo = state.record_automation_outcome(&id, outcome, run, now);
            let schedule = state.schedules.get_mut(&id).expect("just recorded");
            if !stopped {
                schedule.enabled = false;
            }
            schedule.updated_at_ms = now.max(schedule.updated_at_ms);
            changes.push((previous, undo, agent_id));
        }
        (changes, state.control_plane_persist_request())
    };
    if persist.save().await.is_err() {
        // Controller ruling 3: exactly the previous records, counters, and
        // fire log. Undo in reverse, so each fire log change unwinds in turn.
        let mut state = inner.state.write().await;
        for (previous, undo, _) in changes.into_iter().rev() {
            if let Some(undo) = undo {
                state.undo_automation_outcome(undo);
            }
            state.schedules.insert(previous.id.clone(), previous);
        }
        return Err(ScheduleError::Persistence);
    }
    let state = inner.state.read().await;
    for (previous, _, agent_id) in &changes {
        state.publish_automation_updated(agent_id, &previous.id, false);
    }
    Ok(())
}

async fn claim_due(
    inner: &Arc<SchedulerInner>,
    id: &str,
    now: u64,
) -> Result<Option<ScheduledPromptRecord>, ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (claim, claimed, previous, persist) = {
        let mut state = inner.state.write().await;
        let Some(previous) = state
            .schedules
            .get(id)
            .filter(|item| item.enabled && item.next_due_at_ms <= now)
            .cloned()
        else {
            return Ok(None);
        };
        let mut claimed = previous.clone();
        let claim = match next_due_after_claim(
            &claimed.trigger,
            claimed.active_hours.as_ref(),
            previous.next_due_at_ms,
            now,
        ) {
            Ok(next_due_at_ms) => {
                claimed.next_due_at_ms = next_due_at_ms;
                claimed.last_fired = Some(ScheduleLastFired {
                    fired_at_ms: now,
                    run_idempotency_key: format!("schedule:{}:{}", claimed.id, now),
                    manual: false,
                });
                claimed.last_safe_outcome = None;
                if matches!(claimed.trigger, ScheduleTrigger::Once { .. }) {
                    // Spec §9.1: a one-time automation fires once, then turns
                    // itself off.
                    claimed.enabled = false;
                }
                true
            }
            // A stored trigger or window that can never fire again (restore
            // only checks syntax) is turned off, so it cannot end the tick and
            // starve the other due automations.
            Err(ScheduleError::Rejected(error)) => {
                tracing::warn!(schedule_id = %claimed.id, %error, "an automation has no next fire time; it was turned off");
                claimed.enabled = false;
                false
            }
            Err(error) => return Err(error),
        };
        claimed.updated_at_ms = now.max(claimed.created_at_ms);
        state.schedules.insert(id.to_string(), claimed.clone());
        (
            claim,
            claimed,
            previous,
            state.control_plane_persist_request(),
        )
    };
    if persist.save().await.is_err() {
        inner
            .state
            .write()
            .await
            .schedules
            .insert(id.to_string(), previous);
        return Err(ScheduleError::Persistence);
    }
    inner
        .state
        .read()
        .await
        .publish_automation_updated(&claimed.agent_id, &claimed.id, false);
    Ok(claim.then_some(claimed))
}

async fn execute_claimed(inner: &Arc<SchedulerInner>, record: ScheduledPromptRecord, now: u64) {
    let room = match &record.target {
        // Spec §9.2: a workspace automation runs in its own `schedule:<id>` session.
        ScheduleTarget::Workspace => RunRoom::Stable(crate::sessions::schedule_room_id(&record.id)),
        ScheduleTarget::Connector { connector_id } => {
            let connector = {
                let state = inner.state.read().await;
                state
                    .connectors
                    .get(connector_id)
                    .filter(|connector| {
                        connector.agent_id == record.agent_id
                            && connector.is_active()
                            && connector.approved_chat.is_some()
                    })
                    .cloned()
            };
            let ready =
                inner.connectors.status(connector_id).await == Some(ConnectorRuntimeStatus::Ready);
            let Some(connector) = connector.filter(|_| ready) else {
                let _ = record_outcome(
                    inner,
                    &record.id,
                    ScheduleOutcomeStatus::Failed,
                    Some("schedule_target_unavailable"),
                    now,
                    None,
                )
                .await;
                return;
            };
            RunRoom::Stable(connector.room_id)
        }
    };
    let run_key = record
        .last_fired
        .as_ref()
        .map(|fired| fired.run_idempotency_key.clone());
    let schedule_id = record.id.clone();
    let agent_id = record.agent_id.clone();
    let target = record.target.clone();
    let request = AgentRunRequest {
        agent_id: record.agent_id.clone(),
        content: Content {
            text: wrap_checkin_prompt(&record.prompt),
            metadata: Some(BTreeMap::from([
                ("kind".into(), DataValue::String("checkin".into())),
                ("id".into(), DataValue::String(record.id.clone())),
            ])),
            attachments: None,
        },
        room,
        idempotency_key: run_key.clone(),
        source: RunSource::Schedule,
        source_ref: Some(record.id.clone()),
        parent: None,
    };
    // What the commit hook did, so the rollback undoes exactly that.
    let recorded = Arc::new(std::sync::Mutex::new(
        None::<(Option<OutcomeUndo>, Option<TelegramOutboundRecord>)>,
    ));
    let commit_recorded = Arc::clone(&recorded);
    let result = inner
        .runs
        .run_with_commit_waiting(
            request,
            move |state, outcome| {
                let result = &outcome.result;
                let status = checkin_outcome_status(outcome);
                if !state.schedules.contains_key(&schedule_id) {
                    return Err(ApiError::not_found());
                }
                let mut outbound = None;
                if status == ScheduleOutcomeStatus::Spoke {
                    if let ScheduleTarget::Connector { connector_id } = &target {
                        let connector = state
                            .connectors
                            .get(connector_id)
                            .filter(|item| item.is_active() && item.approved_chat.is_some())
                            .ok_or_else(ApiError::not_found)?;
                        let (Some(reply_id), Some(reply)) =
                            (outcome.reply_message_id.as_ref(), result.data.as_ref())
                        else {
                            return Err(ApiError::bad_request(
                                "agent produced no assistant message",
                            ));
                        };
                        if !is_silent_checkin_reply(&reply.text) {
                            let item = TelegramOutboundRecord {
                                id: format!(
                                    "telegram:{}:schedule:{}:{}",
                                    connector_id, schedule_id, reply_id
                                ),
                                connector_id: connector_id.clone(),
                                agent_id: connector.agent_id.clone(),
                                room_id: connector.room_id.clone(),
                                assistant_message_id: reply_id.clone(),
                                text: reply.text.clone(),
                                created_at_ms: now,
                                delivered_at_ms: None,
                                attempts: 0,
                                delivery_state: OutboundDeliveryState::Pending,
                                message_pruned: false,
                            };
                            state
                                .outbound
                                .entry(item.id.clone())
                                .or_insert_with(|| item.clone());
                            outbound = Some(item);
                        }
                    }
                }
                // Spec §9.1: the outcome, the counters, and a fire record,
                // in the commit's save.
                let undo = state.record_automation_outcome(
                    &schedule_id,
                    ScheduleSafeOutcome {
                        error_code: checkin_error_code(&status),
                        status,
                        occurred_at_ms: now,
                    },
                    Some((outcome.run_id.clone(), outcome.session_id.clone())),
                    now_ms(),
                );
                *commit_recorded.lock().unwrap_or_else(|p| p.into_inner()) = Some((undo, outbound));
                Ok(())
            },
            move |state| {
                let done = recorded.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some((undo, outbound)) = done {
                    if let Some(outbound) = outbound {
                        if state.outbound.get(&outbound.id) == Some(&outbound) {
                            state.outbound.remove(&outbound.id);
                        }
                    }
                    if let Some(undo) = undo {
                        state.undo_automation_outcome(undo);
                    }
                }
                Ok(())
            },
        )
        .await;
    if result.is_err() {
        let _ = record_outcome(
            inner,
            &record.id,
            ScheduleOutcomeStatus::Failed,
            Some("schedule_run_failed"),
            now,
            run_key.as_deref(),
        )
        .await;
    } else {
        inner
            .state
            .read()
            .await
            .publish_automation_updated(&agent_id, &record.id, false);
    }
}

/// Records an occurrence's outcome outside a run's commit (a run that did
/// not commit, an unavailable target): the outcome, the counters, and a fire
/// record naming the occurrence's run, if one started (`run_key`).
async fn record_outcome(
    inner: &Arc<SchedulerInner>,
    id: &str,
    status: ScheduleOutcomeStatus,
    error_code: Option<&str>,
    now: u64,
    run_key: Option<&str>,
) -> Result<(), ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (agent_id, undo, persist) = {
        let mut state = inner.state.write().await;
        let agent_id = state
            .schedules
            .get(id)
            .map(|record| record.agent_id.clone())
            .ok_or(ScheduleError::NotFound)?;
        let run = run_key
            .and_then(|key| state.runs.find_by_idempotency_key(&agent_id, key, 0))
            .map(|run| (run.id.clone(), run.session_id.clone()));
        let undo = state
            .record_automation_outcome(
                id,
                ScheduleSafeOutcome {
                    status,
                    occurred_at_ms: now,
                    error_code: error_code.map(str::to_string),
                },
                run,
                now_ms(),
            )
            .ok_or(ScheduleError::NotFound)?;
        (agent_id, undo, state.control_plane_persist_request())
    };
    if persist.save().await.is_err() {
        // Controller ruling 3: exactly the previous outcome, counters, and
        // fire log.
        inner.state.write().await.undo_automation_outcome(undo);
        return Err(ScheduleError::Persistence);
    }
    inner
        .state
        .read()
        .await
        .publish_automation_updated(&agent_id, id, false);
    Ok(())
}

#[cfg(test)]
pub(crate) fn next_due_at_ms(
    trigger: &ScheduleTrigger,
    from_ms: u64,
) -> Result<u64, ScheduleError> {
    next_due(trigger, None, from_ms)
}

/// The first fire of `trigger` after `from_ms` inside `active_hours`.
pub(crate) fn next_due(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    from_ms: u64,
) -> Result<u64, ScheduleError> {
    let window = active_window(active_hours)?;
    timing::next_fire_after(trigger, window.as_ref(), from_ms).map_err(ScheduleError::Rejected)
}

/// The due time after the occurrence due at `previous_due` was claimed.
pub(crate) fn next_due_after_claim(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    previous_due: u64,
    now: u64,
) -> Result<u64, ScheduleError> {
    let window = active_window(active_hours)?;
    timing::next_fire_after_claim(trigger, window.as_ref(), previous_due, now)
        .map_err(ScheduleError::Rejected)
}

fn active_window(
    active_hours: Option<&ActiveHours>,
) -> Result<Option<timing::ActiveWindow>, ScheduleError> {
    active_hours
        .map(timing::ActiveWindow::parse)
        .transpose()
        .map_err(ScheduleError::Rejected)
}

pub(crate) fn legacy_next_due_at_ms(
    created_at_ms: u64,
    last_run_at_ms: Option<u64>,
    interval_secs: u64,
) -> Result<u64, ScheduleError> {
    if created_at_ms == 0 || interval_secs == 0 {
        return Err(ScheduleError::Invalid("legacy schedule timing is invalid"));
    }
    let interval_ms = interval_secs
        .checked_mul(1_000)
        .ok_or(ScheduleError::Invalid("legacy schedule timing overflow"))?;
    last_run_at_ms
        .unwrap_or(created_at_ms)
        .checked_add(interval_ms)
        .ok_or(ScheduleError::Invalid("legacy schedule timing overflow"))
}

pub(crate) fn wrap_checkin_prompt(prompt: &str) -> String {
    format!("{}\n\n{}", prompt.trim(), CHECKIN_SUFFIX)
}
/// The owner's prompt inside a wrapped check-in input.
pub(crate) fn unwrap_checkin_prompt(text: &str) -> &str {
    text.strip_suffix(CHECKIN_SUFFIX)
        .map(str::trim_end)
        .unwrap_or(text)
        .trim()
}
/// Input the scheduler tagged as a check-in prompt.
pub(crate) fn is_checkin_content(content: &Content) -> bool {
    matches!(
        content.metadata.as_ref().and_then(|metadata| metadata.get("kind")),
        Some(DataValue::String(kind)) if kind == "checkin"
    )
}
pub(crate) fn is_silent_checkin_reply(reply: &str) -> bool {
    reply.trim() == CHECKIN_SENTINEL
}

fn validate_prompt(prompt: &str) -> Result<(), ScheduleError> {
    if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(ScheduleError::Invalid("prompt is invalid"));
    }
    Ok(())
}

fn validate_trigger(trigger: &ScheduleTrigger) -> Result<(), ScheduleError> {
    timing::validate_stored_trigger(trigger).map_err(ScheduleError::Rejected)
}

fn validate_target(
    state: &crate::state::DaemonState,
    agent_id: &str,
    target: &ScheduleTarget,
    enabled: bool,
) -> Result<(), ScheduleError> {
    if let ScheduleTarget::Connector { connector_id } = target {
        let connector = state
            .connectors
            .get(connector_id)
            .filter(|item| item.agent_id == agent_id && item.deleted_at_ms.is_none())
            .ok_or(ScheduleError::TargetUnavailable)?;
        if enabled && !connector.is_active() {
            return Err(ScheduleError::TargetUnavailable);
        }
    }
    Ok(())
}

fn next_schedule_id(now: u64) -> String {
    format!(
        "schedule-{now}-{}",
        NEXT_SCHEDULE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::credentials::{InMemoryCredentialStore, TelegramBotToken};
    use crate::connectors::runtime::TelegramTransport;
    use crate::connectors::telegram::{
        TelegramSentMessage, TelegramTransportError, TelegramUpdateBatch,
    };
    use crate::connectors::{
        TelegramBotIdentity, TelegramChatKind, TelegramChatMetadata, TelegramConnectorRecord,
    };
    use crate::state::DaemonState;
    use anima_core::{AgentConfig, AgentSettings, MessageRole};
    use async_trait::async_trait;
    use tokio::sync::{RwLock, Semaphore};

    pub(super) struct NoopTelegram;

    #[async_trait]
    impl TelegramTransport for NoopTelegram {
        async fn get_me(
            &self,
            _token: &TelegramBotToken,
        ) -> Result<TelegramBotIdentity, TelegramTransportError> {
            Ok(TelegramBotIdentity {
                id: "1".into(),
                username: Some("test_bot".into()),
                display_name: Some("Test Bot".into()),
            })
        }
        async fn get_updates(
            &self,
            _token: &TelegramBotToken,
            _offset: i64,
        ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
            std::future::pending().await
        }
        async fn send_message(
            &self,
            _token: &TelegramBotToken,
            _chat_id: &str,
            _text: &str,
        ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
            Ok(vec![])
        }
    }

    pub(super) fn service() -> (
        SchedulerService,
        SharedDaemonState,
        String,
        ConnectorManager,
    ) {
        service_with_daemon(DaemonState::new())
    }

    pub(super) fn service_with_daemon(
        mut daemon: DaemonState,
    ) -> (
        SchedulerService,
        SharedDaemonState,
        String,
        ConnectorManager,
    ) {
        let agent_id = daemon
            .create_agent(AgentConfig {
                name: "scheduler".into(),
                model: "gpt-5.4".into(),
                bio: None,
                lore: None,
                knowledge: None,
                topics: None,
                adjectives: None,
                style: None,
                provider: Some("openai".into()),
                system: None,
                tools: None,
                plugins: None,
                settings: Some(AgentSettings::default()),
            })
            .unwrap()
            .state
            .id;
        let state = Arc::new(RwLock::new(daemon));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(2)));
        let connectors = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(NoopTelegram),
        );
        (
            SchedulerService::new(Arc::clone(&state), runs, connectors.clone()),
            state,
            agent_id,
            connectors,
        )
    }

    pub(super) struct GatedModel {
        pub(super) entered: Arc<Semaphore>,
        pub(super) release: Arc<Semaphore>,
    }

    #[async_trait]
    impl anima_core::ModelAdapter for GatedModel {
        fn provider(&self) -> &str {
            "test"
        }
        async fn generate(
            &self,
            _: &AgentConfig,
            _: &anima_core::ModelGenerateRequest,
        ) -> Result<anima_core::ModelGenerateResponse, String> {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            Ok(anima_core::ModelGenerateResponse {
                content: Content {
                    text: CHECKIN_SENTINEL.into(),
                    ..Default::default()
                },
                tool_calls: None,
                usage: Default::default(),
                stop_reason: anima_core::ModelStopReason::End,
            })
        }
    }

    pub(super) async fn due_schedule(
        service: &SchedulerService,
        agent_id: &str,
    ) -> ScheduledPromptRecord {
        service
            .create(
                agent_id.into(),
                "Check status".into(),
                ScheduleTrigger::Interval {
                    interval_ms: 60_000,
                },
                ScheduleTarget::Workspace,
                true,
                None,
                Some(now_ms()),
                None,
            )
            .await
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn an_unfireable_stored_cron_is_disabled_and_does_not_starve_the_tick() {
        let (service, state, agent_id, _) = service();
        let now = now_ms();
        let (bad, _) = service
            .create(
                agent_id.clone(),
                "Never".into(),
                ScheduleTrigger::Cron {
                    expression: "0 0 31 2 *".into(),
                    time_zone: "UTC".into(),
                },
                ScheduleTarget::Workspace,
                true,
                None,
                Some(now - 10),
                None,
            )
            .await
            .unwrap();
        let good = due_schedule(&service, &agent_id).await;
        assert_eq!(service.tick_at(now_ms()).await.unwrap(), 1);
        let guard = state.read().await;
        assert!(!guard.schedules[&bad.id].enabled, "the bad one is off");
        assert!(guard.schedules[&bad.id].last_fired.is_none());
        assert!(guard.schedules[&good.id].last_fired.is_some());
        assert!(guard.schedules[&good.id].enabled);
    }

    #[tokio::test]
    async fn time_zones_are_stored_trimmed_on_create_and_update() {
        let (service, _state, agent_id, _) = service();
        let (record, _) = service
            .create(
                agent_id.clone(),
                "Daily".into(),
                ScheduleTrigger::Daily {
                    hour: 9,
                    minute: 0,
                    time_zone: " America/New_York ".into(),
                },
                ScheduleTarget::Workspace,
                true,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            record.trigger,
            ScheduleTrigger::Daily {
                hour: 9,
                minute: 0,
                time_zone: "America/New_York".into(),
            }
        );
        let updated = service
            .update(
                &agent_id,
                &record.id,
                None,
                Some(ScheduleTrigger::Cron {
                    expression: "0 9 * * *".into(),
                    time_zone: "	Europe/Paris ".into(),
                }),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            updated.trigger,
            ScheduleTrigger::Cron {
                expression: "0 9 * * *".into(),
                time_zone: "Europe/Paris".into(),
            }
        );
    }

    #[tokio::test]
    async fn scheduler_runs_other_automations_of_a_busy_agent_concurrently() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let daemon = DaemonState::with_model_adapter(Arc::new(GatedModel {
            entered: entered.clone(),
            release: release.clone(),
        }));
        let (mut service, state, first, _) = service_with_daemon(daemon);
        Arc::get_mut(&mut service.inner).unwrap().runs =
            AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(8)));
        let second = {
            let mut state = state.write().await;
            let mut config = state.get_agent(&first).unwrap().state.config;
            config.name = "second".into();
            state.create_agent(config).unwrap().state.id
        };
        let running = due_schedule(&service, &first).await;
        service.start().await;
        tokio::time::timeout(Duration::from_secs(3), entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        let running_fired = state.read().await.schedules[&running.id].last_fired.clone();

        // Another automation of the busy agent and one of another agent.
        let sibling = due_schedule(&service, &first).await;
        let other = due_schedule(&service, &second).await;
        let both_started =
            tokio::time::timeout(Duration::from_secs(3), entered.acquire_many(2)).await;
        let running_fired_later = state.read().await.schedules[&running.id].last_fired.clone();
        release.add_permits(10);
        service.shutdown().await;

        assert!(
            both_started.is_ok(),
            "one run per automation: a busy agent's other automation starts too"
        );
        assert_eq!(
            running_fired_later, running_fired,
            "a running automation is never claimed again while it runs"
        );
        for id in [&running.id, &sibling.id, &other.id] {
            assert!(
                state.read().await.schedules[id].last_safe_outcome.is_some(),
                "{id} should finish"
            );
        }
    }

    #[test]
    fn a_stopped_check_in_is_its_own_outcome_and_keeps_the_schedule() {
        let outcome = |status: RunStatus, result: anima_core::TaskResult<Content>| RunOutcome {
            run_id: "run_1".into(),
            session_id: "schedule:s".into(),
            reply_message_id: None,
            result,
            status,
        };
        let reply = |text: &str| {
            anima_core::TaskResult::success(
                Content {
                    text: text.into(),
                    ..Content::default()
                },
                1,
            )
        };
        let stopped = checkin_outcome_status(&outcome(
            RunStatus::Cancelled,
            anima_core::TaskResult::error("stopped", 1),
        ));
        assert_eq!(stopped, ScheduleOutcomeStatus::Stopped);
        assert_eq!(
            checkin_error_code(&stopped).as_deref(),
            Some("schedule_run_stopped")
        );
        assert_eq!(stopped.contract_name(), "stopped");
        assert_eq!(
            checkin_outcome_status(&outcome(
                RunStatus::Failed,
                anima_core::TaskResult::error("boom", 1)
            )),
            ScheduleOutcomeStatus::Failed
        );
        assert_eq!(
            checkin_outcome_status(&outcome(RunStatus::Completed, reply(CHECKIN_SENTINEL))),
            ScheduleOutcomeStatus::Silent
        );
        assert_eq!(
            checkin_outcome_status(&outcome(RunStatus::Completed, reply("Heads up"))),
            ScheduleOutcomeStatus::Spoke
        );
        assert_eq!(
            serde_json::to_value(ScheduleOutcomeStatus::Stopped).unwrap(),
            "stopped"
        );
        for (status, name) in [
            (ScheduleOutcomeStatus::Silent, "silent"),
            (ScheduleOutcomeStatus::Spoke, "spoke"),
            (ScheduleOutcomeStatus::Failed, "error"),
        ] {
            assert_eq!(status.contract_name(), name);
        }
        assert_eq!(
            checkin_error_code(&ScheduleOutcomeStatus::Failed).as_deref(),
            Some("schedule_run_failed")
        );
        assert_eq!(checkin_error_code(&ScheduleOutcomeStatus::Spoke), None);
    }

    /// The id of the agent's running check-in once it streamed `text`.
    async fn running_check_in(state: &SharedDaemonState, agent_id: &str, text: &str) -> String {
        for _ in 0..500 {
            {
                let guard = state.read().await;
                let found = guard
                    .runs
                    .active_records()
                    .into_iter()
                    .find(|record| {
                        record.agent_id == agent_id
                            && record.status == RunStatus::Running
                            && guard
                                .live
                                .runs()
                                .view(&record.id)
                                .is_some_and(|view| view.text == text)
                    })
                    .map(|record| record.id.clone());
                if let Some(run_id) = found {
                    return run_id;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no check-in streamed {text:?}");
    }

    /// Controller ruling (M3 pre-flight audit M24, spec §4.6): a check-in
    /// its owner stops records `stopped`, its schedule stays enabled, and it
    /// fires again at its next occurrence.
    #[tokio::test]
    async fn a_stopped_check_in_records_stopped_and_keeps_its_schedule() {
        use crate::agent_runs::test_support::{ScriptedModel, Step};
        let daemon = DaemonState::with_model_adapter(ScriptedModel::new(vec![
            Step::Hold(vec!["Checking"]),
            Step::Text(vec![CHECKIN_SENTINEL]),
        ]));
        let (service, state, agent_id, _) = service_with_daemon(daemon);
        let record = due_schedule(&service, &agent_id).await;
        let now = now_ms();
        let ticking = {
            let service = service.clone();
            tokio::spawn(async move { service.tick_at(now).await })
        };
        let run_id = running_check_in(&state, &agent_id, "Checking").await;

        let stopping = service
            .inner
            .runs
            .stop_run(&agent_id, &run_id)
            .await
            .unwrap();
        assert_eq!(stopping.source, RunSource::Schedule);
        assert_eq!(ticking.await.unwrap().unwrap(), 1);

        {
            let guard = state.read().await;
            let schedule = &guard.schedules[&record.id];
            assert!(schedule.enabled, "a stop is not a failure");
            let outcome = schedule.last_safe_outcome.as_ref().unwrap();
            assert_eq!(outcome.status, ScheduleOutcomeStatus::Stopped);
            assert_eq!(outcome.error_code.as_deref(), Some("schedule_run_stopped"));
            let run = guard.runs.get(&run_id).unwrap();
            assert_eq!(run.status, RunStatus::Cancelled);
            assert_eq!(run.error.as_ref().unwrap().code, "stopped");
        }
        // Later ticks leave the resolved occurrence alone.
        reconcile_interrupted(&service.inner, now_ms(), &BTreeSet::new())
            .await
            .unwrap();
        assert!(state.read().await.schedules[&record.id].enabled);

        let next = state.read().await.schedules[&record.id].next_due_at_ms;
        assert_eq!(service.tick_at(next).await.unwrap(), 1, "it fires again");
        assert_eq!(
            state.read().await.schedules[&record.id]
                .last_safe_outcome
                .as_ref()
                .unwrap()
                .status,
            ScheduleOutcomeStatus::Silent
        );
    }

    /// ...and a stop saved just before a restart, before the check-in could
    /// commit, keeps the schedule enabled too: the restarted daemon records
    /// the occurrence as `stopped` instead of disabling it as interrupted
    /// (audit M24).
    #[tokio::test]
    async fn a_check_in_stopped_just_before_a_restart_keeps_its_schedule() {
        use crate::agent_runs::test_support::{ScriptedModel, Step};
        let daemon =
            DaemonState::with_model_adapter(ScriptedModel::new(vec![Step::Hold(vec!["Checking"])]));
        let (service, state, agent_id, _) = service_with_daemon(daemon);
        let record = due_schedule(&service, &agent_id).await;
        let now = now_ms();
        let ticking = {
            let service = service.clone();
            tokio::spawn(async move { service.tick_at(now).await })
        };
        let run_id = running_check_in(&state, &agent_id, "Checking").await;
        // What the stop saves is what a crash right after its save leaves.
        let save_gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(false);
        let stopping = {
            let runs = service.inner.runs.clone();
            let (agent_id, run_id) = (agent_id.clone(), run_id.clone());
            tokio::spawn(async move { runs.stop_run(&agent_id, &run_id).await })
        };
        tokio::time::timeout(Duration::from_secs(5), save_gate.entered.acquire())
            .await
            .expect("the stop saves")
            .unwrap()
            .forget();
        let on_disk = state.read().await.control_plane_snapshot();
        save_gate.release.add_permits(1);
        stopping.await.unwrap().unwrap();
        ticking.await.unwrap().unwrap();

        let mut restarted = DaemonState::new();
        restarted.restore_control_plane_snapshot(on_disk).unwrap();
        {
            let interrupted = restarted.runs.get(&run_id).unwrap();
            assert_eq!(interrupted.status, RunStatus::Interrupted);
            assert_eq!(
                interrupted.error.as_ref().unwrap().code,
                "restart_during_run"
            );
            assert!(interrupted.stop.is_some(), "the saved stop survives");
        }
        let restarted = Arc::new(RwLock::new(restarted));
        let runs = AgentRunCoordinator::new(Arc::clone(&restarted), Arc::new(Semaphore::new(2)));
        let connectors = ConnectorManager::new(
            Arc::clone(&restarted),
            runs.clone(),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(NoopTelegram),
        );
        let service = SchedulerService::new(Arc::clone(&restarted), runs, connectors);
        reconcile_interrupted(&service.inner, now_ms(), &BTreeSet::new())
            .await
            .unwrap();

        let guard = restarted.read().await;
        let schedule = &guard.schedules[&record.id];
        assert!(
            schedule.enabled,
            "a stopped occurrence is not an interrupted one"
        );
        let outcome = schedule.last_safe_outcome.as_ref().unwrap();
        assert_eq!(outcome.status, ScheduleOutcomeStatus::Stopped);
        assert_eq!(outcome.error_code.as_deref(), Some("schedule_run_stopped"));
    }

    #[tokio::test]
    async fn restart_disables_unfinished_claim_without_replaying_it() {
        let (service, state, agent_id, _) = service();
        let record = due_schedule(&service, &agent_id).await;
        claim_due(&service.inner, &record.id, now_ms())
            .await
            .unwrap()
            .unwrap();
        service.start().await;
        let reconciled = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !state.read().await.schedules[&record.id].enabled {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        service.shutdown().await;
        assert!(
            reconciled.is_ok(),
            "interrupted occurrence must require review before another run"
        );
        let state = state.read().await;
        assert_eq!(
            state.schedules[&record.id]
                .last_safe_outcome
                .as_ref()
                .unwrap()
                .error_code
                .as_deref(),
            Some("schedule_run_interrupted")
        );
        assert!(state.get_agent(&agent_id).unwrap().messages.is_empty());
    }

    #[tokio::test]
    async fn claiming_new_occurrence_clears_previous_outcome() {
        let (service, state, agent_id, _) = service();
        let record = due_schedule(&service, &agent_id).await;
        state
            .write()
            .await
            .schedules
            .get_mut(&record.id)
            .unwrap()
            .last_safe_outcome = Some(ScheduleSafeOutcome {
            status: ScheduleOutcomeStatus::Spoke,
            occurred_at_ms: 1,
            error_code: None,
        });
        let claimed = claim_due(&service.inner, &record.id, now_ms())
            .await
            .unwrap()
            .unwrap();
        assert!(claimed.last_safe_outcome.is_none());
        assert!(state.read().await.schedules[&record.id]
            .last_safe_outcome
            .is_none());
    }

    #[tokio::test]
    async fn reconciliation_preserves_completed_and_never_claimed_schedules() {
        let (service, state, agent_id, _) = service();
        let completed = due_schedule(&service, &agent_id).await;
        let now = now_ms();
        claim_due(&service.inner, &completed.id, now).await.unwrap();
        record_outcome(
            &service.inner,
            &completed.id,
            ScheduleOutcomeStatus::Spoke,
            None,
            now,
            None,
        )
        .await
        .unwrap();
        let fresh = due_schedule(&service, &agent_id).await;
        let before = state.read().await.schedules.clone();
        reconcile_interrupted(&service.inner, now, &BTreeSet::new())
            .await
            .unwrap();
        assert_eq!(state.read().await.schedules, before);
        assert!(state.read().await.schedules[&fresh.id].enabled);
        // Legacy snapshots kept an outcome from an earlier occurrence.
        state
            .write()
            .await
            .schedules
            .get_mut(&completed.id)
            .unwrap()
            .last_safe_outcome
            .as_mut()
            .unwrap()
            .occurred_at_ms = now - 1;
        reconcile_interrupted(&service.inner, now, &BTreeSet::new())
            .await
            .unwrap();
        assert!(!state.read().await.schedules[&completed.id].enabled);
        assert!(state.read().await.schedules[&fresh.id].enabled);
    }

    #[tokio::test]
    async fn scheduler_bounds_admission_and_leaves_excess_work_unclaimed() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let daemon = DaemonState::with_model_adapter(Arc::new(GatedModel {
            entered: entered.clone(),
            release: release.clone(),
        }));
        let (mut service, state, first, _) = service_with_daemon(daemon);
        Arc::get_mut(&mut service.inner).unwrap().runs =
            AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(64)));
        due_schedule(&service, &first).await;
        due_schedule(&service, &first).await;
        for n in 0..10 {
            let agent_id = {
                let mut state = state.write().await;
                let mut config = state.get_agent(&first).unwrap().state.config;
                config.name = format!("worker-{n}");
                state.create_agent(config).unwrap().state.id
            };
            due_schedule(&service, &agent_id).await;
        }
        let tick = tokio::spawn(async move { service.tick_at(now_ms()).await });
        let concurrent =
            tokio::time::timeout(Duration::from_secs(3), entered.acquire_many(8)).await;
        let claimed = state
            .read()
            .await
            .schedules
            .values()
            .filter(|r| r.last_fired.is_some())
            .count();
        release.add_permits(30);
        let result = tick.await.unwrap().unwrap();
        assert!(
            concurrent.is_ok(),
            "eight independent schedules should be admitted concurrently"
        );
        assert_eq!(claimed, 8);
        assert_eq!(result, 8);
    }

    #[tokio::test]
    async fn failed_restart_reconciliation_keeps_all_admission_closed() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let daemon = DaemonState::with_model_adapter(Arc::new(GatedModel {
            entered: entered.clone(),
            release: release.clone(),
        }));
        let (service, state, agent_id, _) = service_with_daemon(daemon);
        let record = due_schedule(&service, &agent_id).await;
        claim_due(&service.inner, &record.id, now_ms())
            .await
            .unwrap();
        due_schedule(&service, &agent_id).await;
        let path = std::env::temp_dir().join(format!("anima-reconcile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(path.clone()),
        ));
        service.start().await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        let no_execution = entered.available_permits() == 0;
        release.add_permits(10);
        service.shutdown().await;
        std::fs::remove_dir(&path).unwrap();
        assert!(
            no_execution,
            "no work may execute until reconciliation is durably saved"
        );
        let state = state.read().await;
        assert!(
            state.schedules[&record.id].enabled,
            "failed persistence must roll back in-memory reconciliation"
        );
        assert!(state.schedules[&record.id].last_safe_outcome.is_none());
    }

    #[tokio::test]
    async fn orphaned_claim_is_reconciled_after_storage_recovers_without_reexecution() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let daemon = DaemonState::with_model_adapter(Arc::new(GatedModel {
            entered: entered.clone(),
            release: release.clone(),
        }));
        let (service, state, agent_id, _) = service_with_daemon(daemon);
        let record = due_schedule(&service, &agent_id).await;
        let run_service = service.clone();
        let running = tokio::spawn(async move { run_service.tick_at(now_ms()).await });
        tokio::time::timeout(Duration::from_secs(3), entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        let directory = std::env::temp_dir().join(format!("anima-orphan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(directory.clone()),
        ));
        release.add_permits(1);
        assert_eq!(running.await.unwrap().unwrap(), 1);
        assert!(state.read().await.schedules[&record.id]
            .last_safe_outcome
            .is_none());
        let path = directory.join("snapshot.json");
        let config = crate::control_plane_store::ControlPlaneStoreConfig::Json(path.clone());
        state
            .write()
            .await
            .set_control_plane_store(Some(config.clone()));
        assert_eq!(service.tick_at(now_ms()).await.unwrap(), 0);
        let repaired = state.read().await.schedules[&record.id].clone();
        let persisted = crate::control_plane_store::load_control_plane_snapshot(&config)
            .await
            .unwrap();
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::remove_dir(&directory).unwrap();
        assert!(
            !repaired.enabled,
            "storage recovery must surface the orphan as requiring review"
        );
        assert_eq!(
            repaired
                .last_safe_outcome
                .as_ref()
                .unwrap()
                .error_code
                .as_deref(),
            Some("schedule_run_interrupted")
        );
        assert_eq!(persisted.unwrap().schedules[0], repaired);
        assert_eq!(
            entered.available_permits(),
            0,
            "reconciliation must not run the provider again"
        );
    }

    #[test]
    fn interval_first_fires_after_a_full_interval() {
        assert_eq!(
            next_due_at_ms(
                &ScheduleTrigger::Interval {
                    interval_ms: 60_000
                },
                1_000,
            )
            .unwrap(),
            61_000
        );
    }

    #[test]
    fn utc_daily_uses_the_next_local_wall_clock_occurrence() {
        let trigger = ScheduleTrigger::Daily {
            hour: 9,
            minute: 30,
            time_zone: "UTC".into(),
        };
        assert_eq!(
            next_due_at_ms(&trigger, 8 * 3_600_000).unwrap(),
            9 * 3_600_000 + 30 * 60_000
        );
        assert_eq!(
            next_due_at_ms(&trigger, 10 * 3_600_000).unwrap(),
            86_400_000 + 9 * 3_600_000 + 30 * 60_000
        );
    }

    #[test]
    fn legacy_import_preserves_browser_due_time_and_rejects_overflow() {
        assert_eq!(legacy_next_due_at_ms(1_000, None, 60).unwrap(), 61_000);
        assert_eq!(
            legacy_next_due_at_ms(1_000, Some(9_000), 60).unwrap(),
            69_000
        );
        assert!(legacy_next_due_at_ms(u64::MAX, None, 1).is_err());
        assert!(legacy_next_due_at_ms(1, None, 0).is_err());
    }

    #[test]
    fn checkin_wrapper_and_silence_match_are_exact() {
        let wrapped = wrap_checkin_prompt("Check status");
        assert_eq!(wrapped, "Check status\n\n(This is a scheduled check-in. If you have nothing worth saying right now, reply with exactly CHECKIN_OK and nothing else.)");
        assert!(is_silent_checkin_reply("CHECKIN_OK"));
        assert!(is_silent_checkin_reply("  CHECKIN_OK  "));
        assert!(!is_silent_checkin_reply("CHECKIN_OK."));
    }

    #[tokio::test]
    async fn due_workspace_schedule_runs_in_its_stable_schedule_room() {
        let (service, state, agent_id, manager) = service();
        let (record, _) = service
            .create(
                agent_id.clone(),
                "Check status".into(),
                ScheduleTrigger::Interval { interval_ms: 1_000 },
                ScheduleTarget::Workspace,
                true,
                None,
                Some(2),
                Some(1),
            )
            .await
            .unwrap();
        assert_eq!(service.tick_at(2).await.unwrap(), 1);
        assert_eq!(
            service.tick_at(1_002).await.unwrap(),
            1,
            "the next occurrence fires too"
        );
        let guard = state.read().await;
        let schedule = &guard.schedules[&record.id];
        assert_eq!(schedule.next_due_at_ms, 2_002);
        assert_eq!(schedule.last_fired.as_ref().unwrap().fired_at_ms, 1_002);
        assert_eq!(
            schedule.last_safe_outcome.as_ref().unwrap().status,
            ScheduleOutcomeStatus::Spoke
        );
        let room = crate::sessions::schedule_room_id(&record.id);
        let snapshot = guard.get_agent(&agent_id).unwrap();
        assert_eq!(snapshot.messages.len(), 4);
        assert!(
            snapshot
                .messages
                .iter()
                .all(|message| message.room_id == room),
            "both occurrences share the automation's room"
        );
        let input = snapshot
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .unwrap();
        assert_eq!(
            input.content.metadata.as_ref().unwrap().get("kind"),
            Some(&DataValue::String("checkin".into()))
        );
        assert_eq!(
            input.content.metadata.as_ref().unwrap().get("id"),
            Some(&DataValue::String(record.id.clone()))
        );
        let session = guard
            .sessions
            .get(&agent_id, &room)
            .expect("the automation's room is a check-in session");
        assert_eq!(session.kind, crate::sessions::SessionKind::Checkin);
        assert_eq!(session.title, "Check-in · Check status");
        drop(guard);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn unavailable_connector_advances_and_records_reason_without_running_agent() {
        let (service, state, agent_id, manager) = service();
        let connector_id = "telegram-unavailable".to_string();
        state.write().await.connectors.insert(
            connector_id.clone(),
            TelegramConnectorRecord {
                id: connector_id.clone(),
                agent_id: agent_id.clone(),
                room_id: "telegram:unavailable".into(),
                bot: TelegramBotIdentity {
                    id: "1".into(),
                    username: None,
                    display_name: None,
                },
                approved_chat: Some(TelegramChatMetadata {
                    id: "2".into(),
                    kind: TelegramChatKind::Private,
                    title: None,
                    username: None,
                }),
                pending_pairing: None,
                next_update_id: 0,
                enabled: true,
                deleted_at_ms: None,
                created_at_ms: 1,
                updated_at_ms: 1,
            },
        );
        let (record, _) = service
            .create(
                agent_id.clone(),
                "Check status".into(),
                ScheduleTrigger::Interval { interval_ms: 1_000 },
                ScheduleTarget::Connector { connector_id },
                true,
                None,
                Some(2),
                Some(1),
            )
            .await
            .unwrap();
        assert_eq!(service.tick_at(2).await.unwrap(), 1);
        let guard = state.read().await;
        let schedule = &guard.schedules[&record.id];
        assert_eq!(schedule.next_due_at_ms, 1_002);
        assert_eq!(
            schedule
                .last_safe_outcome
                .as_ref()
                .unwrap()
                .error_code
                .as_deref(),
            Some("schedule_target_unavailable")
        );
        assert!(guard.get_agent(&agent_id).unwrap().messages.is_empty());
        assert!(guard.outbound.is_empty());
        drop(guard);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn ready_connector_schedule_uses_stable_room_and_queues_durable_delivery() {
        let (service, state, agent_id, manager) = service();
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:test-token").unwrap(),
            )
            .await
            .unwrap();
        state
            .write()
            .await
            .connectors
            .get_mut(&connector.id)
            .unwrap()
            .approved_chat = Some(TelegramChatMetadata {
            id: "2".into(),
            kind: TelegramChatKind::Private,
            title: None,
            username: None,
        });
        manager.restart(connector.id.clone()).await.unwrap();
        let (record, _) = service
            .create(
                agent_id.clone(),
                "Check status".into(),
                ScheduleTrigger::Interval { interval_ms: 1_000 },
                ScheduleTarget::Connector {
                    connector_id: connector.id.clone(),
                },
                true,
                None,
                Some(2),
                Some(1),
            )
            .await
            .unwrap();
        assert_eq!(service.tick_at(2).await.unwrap(), 1);
        let guard = state.read().await;
        assert_eq!(
            guard.schedules[&record.id]
                .last_safe_outcome
                .as_ref()
                .unwrap()
                .status,
            ScheduleOutcomeStatus::Spoke
        );
        assert!(guard
            .outbound
            .values()
            .any(|item| item.connector_id == connector.id && item.room_id == connector.room_id));
        let snapshot = guard.get_agent(&agent_id).unwrap();
        assert!(
            snapshot
                .messages
                .iter()
                .filter(|message| message.room_id == connector.room_id)
                .count()
                >= 2
        );
        drop(guard);
        manager.shutdown().await;
    }
}

#[cfg(test)]
mod run_now_tests;
