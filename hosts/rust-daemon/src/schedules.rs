use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anima_core::{Content, DataValue, MessageRole, TaskStatus};
use chrono::{LocalResult, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
use crate::app::SharedDaemonState;
use crate::connectors::runtime::{ConnectorManager, ConnectorRuntimeStatus};
use crate::connectors::{OutboundDeliveryState, TelegramOutboundRecord};
use crate::routes::ApiError;

const CHECKIN_SENTINEL: &str = "CHECKIN_OK";
const CHECKIN_SUFFIX: &str = "(This is a scheduled check-in. If you have nothing worth saying right now, reply with exactly CHECKIN_OK and nothing else.)";
const MAX_PROMPT_BYTES: usize = 32 * 1024;
const WORKER_TICK: Duration = Duration::from_millis(250);
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleError {
    AgentNotFound,
    NotFound,
    Invalid(&'static str),
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
                        let _ = SchedulerService::tick_inner(&inner, now_ms()).await;
                    }
                }
            }
        });
        *worker = Some(SchedulerWorker { cancel, join });
    }

    pub(crate) async fn shutdown(&self) {
        if let Some(worker) = self.worker.lock().await.take() {
            let _ = worker.cancel.send(true);
            let _ = worker.join.await;
        }
    }

    pub(crate) async fn list(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ScheduledPromptRecord>, ScheduleError> {
        let state = self.inner.state.read().await;
        if state.get_agent(agent_id).is_none() {
            return Err(ScheduleError::AgentNotFound);
        }
        let mut records = state
            .schedules
            .values()
            .filter(|item| item.agent_id == agent_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(records)
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
        validate_prompt(&prompt)?;
        validate_trigger(&trigger)?;
        let now = now_ms();
        let created_at_ms = created_at_override_ms.unwrap_or(now);
        if created_at_ms == 0 || created_at_ms > now.saturating_add(300_000) {
            return Err(ScheduleError::Invalid("createdAtMs is invalid"));
        }
        let next_due_at_ms = match explicit_next_due_at_ms {
            Some(value) if value > 0 => value,
            Some(_) => return Err(ScheduleError::Invalid("next due time is invalid")),
            None => next_due_at_ms(&trigger, now)?,
        };
        if import_idempotency_key
            .as_ref()
            .is_some_and(|key| key.trim().is_empty() || key.len() > 256)
        {
            return Err(ScheduleError::Invalid("import idempotency key is invalid"));
        }
        let _transaction = self.inner.runs.control_plane_transaction().await;
        let (record, previous, persist) = {
            let mut state = self.inner.state.write().await;
            if state.get_agent(&agent_id).is_none() {
                return Err(ScheduleError::AgentNotFound);
            }
            if let Some(key) = import_idempotency_key.as_ref() {
                if let Some(existing) = state
                    .schedules
                    .values()
                    .find(|item| {
                        item.agent_id == agent_id
                            && item.import_idempotency_key.as_ref() == Some(key)
                    })
                    .cloned()
                {
                    return Ok((existing, false));
                }
            }
            validate_target(&state, &agent_id, &target, enabled)?;
            let id = loop {
                let candidate = next_schedule_id(now);
                if !state.schedules.contains_key(&candidate) {
                    break candidate;
                }
            };
            let record = ScheduledPromptRecord {
                id: id.clone(),
                import_idempotency_key,
                agent_id,
                prompt,
                trigger,
                enabled,
                target,
                next_due_at_ms,
                last_fired: None,
                last_safe_outcome: None,
                created_at_ms,
                updated_at_ms: now.max(created_at_ms),
            };
            let previous = state.schedules.insert(id, record.clone());
            (record, previous, state.control_plane_persist_request())
        };
        if persist.save().await.is_err() {
            let mut state = self.inner.state.write().await;
            if let Some(previous) = previous {
                state.schedules.insert(record.id.clone(), previous);
            } else {
                state.schedules.remove(&record.id);
            }
            return Err(ScheduleError::Persistence);
        }
        Ok((record, true))
    }

    pub(crate) async fn update(
        &self,
        agent_id: &str,
        schedule_id: &str,
        prompt: Option<String>,
        trigger: Option<ScheduleTrigger>,
        target: Option<ScheduleTarget>,
        enabled: Option<bool>,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        if let Some(prompt) = &prompt {
            validate_prompt(prompt)?;
        }
        if let Some(trigger) = &trigger {
            validate_trigger(trigger)?;
        }
        if prompt.is_none() && trigger.is_none() && target.is_none() && enabled.is_none() {
            return Err(ScheduleError::Invalid("at least one field is required"));
        }
        let now = now_ms();
        let _transaction = self.inner.runs.control_plane_transaction().await;
        let (updated, previous, persist) = {
            let mut state = self.inner.state.write().await;
            let previous = state
                .schedules
                .get(schedule_id)
                .filter(|item| item.agent_id == agent_id)
                .cloned()
                .ok_or(ScheduleError::NotFound)?;
            let mut updated = previous.clone();
            if let Some(prompt) = prompt {
                updated.prompt = prompt;
            }
            if let Some(target) = target {
                updated.target = target;
            }
            let was_enabled = updated.enabled;
            if let Some(enabled) = enabled {
                updated.enabled = enabled;
            }
            let timing_reset = trigger.is_some() || (!was_enabled && updated.enabled);
            if let Some(trigger) = trigger {
                updated.trigger = trigger;
            }
            if timing_reset {
                updated.next_due_at_ms = next_due_at_ms(&updated.trigger, now)?;
            }
            validate_target(&state, agent_id, &updated.target, updated.enabled)?;
            updated.updated_at_ms = now.max(updated.created_at_ms);
            state
                .schedules
                .insert(schedule_id.to_string(), updated.clone());
            (updated, previous, state.control_plane_persist_request())
        };
        if persist.save().await.is_err() {
            self.inner
                .state
                .write()
                .await
                .schedules
                .insert(schedule_id.to_string(), previous);
            return Err(ScheduleError::Persistence);
        }
        Ok(updated)
    }

    pub(crate) async fn delete(
        &self,
        agent_id: &str,
        schedule_id: &str,
    ) -> Result<(), ScheduleError> {
        let _transaction = self.inner.runs.control_plane_transaction().await;
        let (removed, persist) = {
            let mut state = self.inner.state.write().await;
            if !state
                .schedules
                .get(schedule_id)
                .is_some_and(|item| item.agent_id == agent_id)
            {
                return Err(ScheduleError::NotFound);
            }
            let removed = state.schedules.remove(schedule_id).expect("checked");
            (removed, state.control_plane_persist_request())
        };
        if persist.save().await.is_err() {
            self.inner
                .state
                .write()
                .await
                .schedules
                .insert(schedule_id.to_string(), removed);
            return Err(ScheduleError::Persistence);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn tick_at(&self, now: u64) -> Result<usize, ScheduleError> {
        Self::tick_inner(&self.inner, now).await
    }

    async fn tick_inner(inner: &Arc<SchedulerInner>, now: u64) -> Result<usize, ScheduleError> {
        let due_ids = {
            let state = inner.state.read().await;
            let mut ids = state
                .schedules
                .values()
                .filter(|item| item.enabled && item.next_due_at_ms <= now)
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            ids.sort();
            ids
        };
        let mut claimed = 0;
        for id in due_ids {
            if let Some(record) = claim_due(inner, &id, now).await? {
                claimed += 1;
                execute_claimed(inner, record, now).await;
            }
        }
        Ok(claimed)
    }
}

async fn claim_due(
    inner: &Arc<SchedulerInner>,
    id: &str,
    now: u64,
) -> Result<Option<ScheduledPromptRecord>, ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (claimed, previous, persist) = {
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
        claimed.next_due_at_ms =
            next_due_after_claim(&claimed.trigger, previous.next_due_at_ms, now)?;
        claimed.last_fired = Some(ScheduleLastFired {
            fired_at_ms: now,
            run_idempotency_key: format!("schedule:{}:{}", claimed.id, now),
        });
        claimed.updated_at_ms = now.max(claimed.created_at_ms);
        state.schedules.insert(id.to_string(), claimed.clone());
        (claimed, previous, state.control_plane_persist_request())
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
    Ok(Some(claimed))
}

async fn execute_claimed(inner: &Arc<SchedulerInner>, record: ScheduledPromptRecord, now: u64) {
    let room = match &record.target {
        ScheduleTarget::Workspace => RunRoom::Generated,
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
                )
                .await;
                return;
            };
            RunRoom::Stable(connector.room_id)
        }
    };
    let schedule_id = record.id.clone();
    let target = record.target.clone();
    let rollback_schedule_id = schedule_id.clone();
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
        idempotency_key: record
            .last_fired
            .as_ref()
            .map(|item| item.run_idempotency_key.clone()),
    };
    let outcome = Arc::new(std::sync::Mutex::new(
        None::<(ScheduleSafeOutcome, Option<TelegramOutboundRecord>)>,
    ));
    let commit_outcome = Arc::clone(&outcome);
    let result = inner
        .runs
        .run_with_commit_waiting(
            request,
            move |state, snapshot, result| {
                let status = if result.status == TaskStatus::Error {
                    ScheduleOutcomeStatus::Failed
                } else if result
                    .data
                    .as_ref()
                    .is_some_and(|content| is_silent_checkin_reply(&content.text))
                {
                    ScheduleOutcomeStatus::Silent
                } else {
                    ScheduleOutcomeStatus::Spoke
                };
                let safe = ScheduleSafeOutcome {
                    status: status.clone(),
                    occurred_at_ms: now,
                    error_code: (status == ScheduleOutcomeStatus::Failed)
                        .then(|| "schedule_run_failed".into()),
                };
                let schedule = state
                    .schedules
                    .get_mut(&schedule_id)
                    .ok_or_else(ApiError::not_found)?;
                schedule.last_safe_outcome = Some(safe.clone());
                schedule.updated_at_ms = now.max(schedule.created_at_ms);
                let mut outbound = None;
                if status == ScheduleOutcomeStatus::Spoke {
                    if let ScheduleTarget::Connector { connector_id } = &target {
                        let connector = state
                            .connectors
                            .get(connector_id)
                            .filter(|item| item.is_active() && item.approved_chat.is_some())
                            .ok_or_else(ApiError::not_found)?;
                        let assistant = snapshot
                            .messages
                            .iter()
                            .rev()
                            .find(|message| {
                                message.room_id == connector.room_id
                                    && message.role == MessageRole::Assistant
                            })
                            .ok_or_else(|| {
                                ApiError::bad_request("agent produced no assistant message")
                            })?;
                        if !is_silent_checkin_reply(&assistant.content.text) {
                            let item = TelegramOutboundRecord {
                                id: format!(
                                    "telegram:{}:schedule:{}:{}",
                                    connector_id, schedule_id, assistant.id
                                ),
                                connector_id: connector_id.clone(),
                                agent_id: connector.agent_id.clone(),
                                room_id: connector.room_id.clone(),
                                assistant_message_id: assistant.id.clone(),
                                text: assistant.content.text.clone(),
                                created_at_ms: now,
                                delivered_at_ms: None,
                                attempts: 0,
                                delivery_state: OutboundDeliveryState::Pending,
                            };
                            state
                                .outbound
                                .entry(item.id.clone())
                                .or_insert_with(|| item.clone());
                            outbound = Some(item);
                        }
                    }
                }
                *commit_outcome.lock().unwrap_or_else(|p| p.into_inner()) = Some((safe, outbound));
                Ok(())
            },
            move |state, baseline| {
                if let Some((_, outbound)) =
                    outcome.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
                {
                    if let Some(outbound) = outbound {
                        if state.outbound.get(&outbound.id) == Some(outbound) {
                            state.outbound.remove(&outbound.id);
                        }
                    }
                }
                state
                    .rollback_agent_runtime(baseline)
                    .map(|_| ())
                    .map_err(ApiError::service_unavailable)?;
                if let Some(schedule) = state.schedules.get_mut(&rollback_schedule_id) {
                    schedule.last_safe_outcome = None;
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
        )
        .await;
    }
}

async fn record_outcome(
    inner: &Arc<SchedulerInner>,
    id: &str,
    status: ScheduleOutcomeStatus,
    error_code: Option<&str>,
    now: u64,
) -> Result<(), ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (previous, persist) = {
        let mut state = inner.state.write().await;
        let schedule = state.schedules.get_mut(id).ok_or(ScheduleError::NotFound)?;
        let previous = schedule.clone();
        schedule.last_safe_outcome = Some(ScheduleSafeOutcome {
            status,
            occurred_at_ms: now,
            error_code: error_code.map(str::to_string),
        });
        schedule.updated_at_ms = now.max(schedule.created_at_ms);
        (previous, state.control_plane_persist_request())
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
    Ok(())
}

pub(crate) fn next_due_at_ms(
    trigger: &ScheduleTrigger,
    from_ms: u64,
) -> Result<u64, ScheduleError> {
    validate_trigger(trigger)?;
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => from_ms
            .checked_add(*interval_ms)
            .ok_or(ScheduleError::Invalid("schedule timing overflow")),
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => next_daily_at_ms(*hour, *minute, time_zone, from_ms),
    }
}

fn next_due_after_claim(
    trigger: &ScheduleTrigger,
    previous_due: u64,
    now: u64,
) -> Result<u64, ScheduleError> {
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => {
            let elapsed = now.saturating_sub(previous_due);
            let steps = elapsed / *interval_ms + 1;
            previous_due
                .checked_add(
                    interval_ms
                        .checked_mul(steps)
                        .ok_or(ScheduleError::Invalid("schedule timing overflow"))?,
                )
                .ok_or(ScheduleError::Invalid("schedule timing overflow"))
        }
        ScheduleTrigger::Daily { .. } => next_due_at_ms(trigger, now),
    }
}

fn next_daily_at_ms(
    hour: u8,
    minute: u8,
    time_zone: &str,
    from_ms: u64,
) -> Result<u64, ScheduleError> {
    let tz: Tz = time_zone
        .parse()
        .map_err(|_| ScheduleError::Invalid("timeZone is invalid"))?;
    let from = Utc
        .timestamp_millis_opt(
            i64::try_from(from_ms)
                .map_err(|_| ScheduleError::Invalid("schedule timing overflow"))?,
        )
        .single()
        .ok_or(ScheduleError::Invalid("schedule timing is invalid"))?;
    let local = from.with_timezone(&tz);
    for day_offset in 0..=2 {
        let date = local
            .date_naive()
            .checked_add_days(chrono::Days::new(day_offset))
            .ok_or(ScheduleError::Invalid("schedule timing overflow"))?;
        let naive = date
            .and_hms_opt(u32::from(hour), u32::from(minute), 0)
            .ok_or(ScheduleError::Invalid("daily trigger is invalid"))?;
        let candidate = match tz.from_local_datetime(&naive) {
            LocalResult::Single(value) => value,
            LocalResult::Ambiguous(first, second) => first.min(second),
            LocalResult::None => continue,
        };
        let millis = u64::try_from(candidate.timestamp_millis())
            .map_err(|_| ScheduleError::Invalid("schedule timing is invalid"))?;
        if millis > from_ms {
            return Ok(millis);
        }
    }
    Err(ScheduleError::Invalid(
        "daily trigger has no next occurrence",
    ))
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
    let core_trigger = match trigger {
        ScheduleTrigger::Interval { interval_ms } => {
            if *interval_ms == 0 || interval_ms % 1_000 != 0 {
                return Err(ScheduleError::Invalid(
                    "intervalMs must be a positive whole number of seconds",
                ));
            }
            anima_schedule::ScheduleTrigger::Every {
                interval_secs: interval_ms / 1_000,
            }
        }
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => {
            let tz: Tz = time_zone
                .parse()
                .map_err(|_| ScheduleError::Invalid("timeZone is invalid"))?;
            let now = Utc::now();
            let offset_minutes = now.with_timezone(&tz).offset().fix().local_minus_utc() / 60;
            anima_schedule::ScheduleTrigger::DailyAt {
                hour: *hour,
                minute: *minute,
                tz_offset_minutes: offset_minutes,
            }
        }
    };
    anima_schedule::Scheduler::new(vec![anima_schedule::ScheduledJob {
        name: "validation".into(),
        agent_name: "agent".into(),
        prompt: "prompt".into(),
        trigger: core_trigger,
        enabled: true,
    }])
    .map_err(|_| ScheduleError::Invalid("trigger is invalid"))?;
    Ok(())
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
    use anima_core::{AgentConfig, AgentSettings};
    use async_trait::async_trait;
    use tokio::sync::{RwLock, Semaphore};

    struct NoopTelegram;

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

    fn service() -> (
        SchedulerService,
        SharedDaemonState,
        String,
        ConnectorManager,
    ) {
        let mut daemon = DaemonState::new();
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
    async fn due_workspace_schedule_claims_before_running_and_tags_the_generated_room() {
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
        let guard = state.read().await;
        let schedule = &guard.schedules[&record.id];
        assert_eq!(schedule.next_due_at_ms, 1_002);
        assert_eq!(schedule.last_fired.as_ref().unwrap().fired_at_ms, 2);
        assert_eq!(
            schedule.last_safe_outcome.as_ref().unwrap().status,
            ScheduleOutcomeStatus::Spoke
        );
        let snapshot = guard.get_agent(&agent_id).unwrap();
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
            Some(&DataValue::String(record.id))
        );
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
