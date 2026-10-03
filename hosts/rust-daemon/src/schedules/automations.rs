//! Automations (spec §9): what M6 adds to a scheduled prompt (a name,
//! active hours, who made it, a preset, and counters). This module also holds
//! their limits and strings, the heartbeat preset, and `AutomationService`,
//! through which the owner's routes and the companion's tools change them.
#![allow(dead_code)] // M6 Task 8 uses every item.

use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::timing::{
    normalized_active_hours, normalized_trigger, upcoming_fires, ActiveHours, ActiveWindow,
    ACTIVE_HOURS_NOT_FOR_ONCE,
};
use super::{
    next_due, next_schedule_id, validate_prompt, validate_target, validate_trigger, ScheduleError,
    ScheduleOutcomeStatus, ScheduleTarget, ScheduleTrigger, ScheduledPromptRecord,
};
use crate::app::SharedDaemonState;
use crate::skills::{has_variation_selector_run, is_hidden_in_one_line, is_smuggling_character};

/// The longest name, in characters (spec §9.1).
pub(crate) const MAX_AUTOMATION_NAME_CHARS: usize = 80;

/// Automations per agent (spec §9.3, §16).
pub(crate) const MAX_AUTOMATIONS_PER_AGENT: usize = 20;
/// A companion-made automation's fires are at least this far apart (spec §9.3).
pub(crate) const MIN_AGENT_AUTOMATION_GAP_MS: u64 = 5 * 60 * 1000;
/// Fires checked for that minimum (spec §9.3).
pub(crate) const AGENT_GAP_CHECKED_FIRES: usize = 10;
/// History entries shown (spec §9.1, §16).
pub(crate) const MAX_AUTOMATION_HISTORY_SHOWN: usize = 50;
/// Fire times a preview lists (spec §9.2).
pub(crate) const PREVIEW_FIRES: usize = 3;
/// The heartbeat preset (spec §9.2): every 30 minutes, 08:00 to 22:00 local.
pub(crate) const HEARTBEAT_INTERVAL_MS: u64 = 30 * 60 * 1000;
pub(crate) const HEARTBEAT_START: &str = "08:00";
pub(crate) const HEARTBEAT_END: &str = "22:00";
pub(crate) const HEARTBEAT_NAME: &str = "Heartbeat";
/// The preset's editable prompt; the scheduler's check-in suffix adds the
/// `CHECKIN_OK` instruction.
pub(crate) const HEARTBEAT_PROMPT: &str = "Review my open tasks, goals, and recent messages, and tell me briefly about anything that needs my attention.";

pub(crate) const TOO_MANY_AUTOMATIONS: &str =
    "This companion already has 20 automations; delete one first";
pub(crate) const AGENT_AUTOMATION_TOO_FREQUENT: &str =
    "Automations you create must run at least 5 minutes apart";
pub(crate) const AUTOMATION_NAME_INVALID: &str = "name must be 1–80 characters on one line";
pub(crate) const AUTOMATION_TEXT_HIDDEN: &str =
    "Automation text must not contain invisible tag or direction-override characters";
pub(crate) const HEARTBEAT_NEEDS_TIME_ZONE: &str = "timeZone is required for the heartbeat preset";
pub(crate) const AUTOMATION_ALREADY_RUNNING: &str = "This automation is already running";
pub(crate) const TOO_MANY_RUNNING_AUTOMATIONS: &str =
    "Too many automations are running; try again shortly";
/// The existing update literal, now named.
const NOTHING_TO_UPDATE: &str = "at least one field is required";

/// Who made an automation (spec §9.1). A companion's carries its tool
/// call, so the web can show the notice card beside it (spec §15.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum AutomationCreator {
    #[default]
    Owner,
    #[serde(rename_all = "camelCase")]
    Agent {
        agent_id: String,
        session_id: String,
        run_id: String,
        tool_call_id: String,
    },
}

/// A preset the automation was made from; a label that survives edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutomationPreset {
    Heartbeat,
}

impl AutomationPreset {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Heartbeat => "heartbeat",
        }
    }
}

/// How an automation's occurrences ended (spec §9.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationCounters {
    pub(crate) runs: u64,
    pub(crate) failures: u64,
    pub(crate) consecutive_failures: u64,
}

impl AutomationCounters {
    /// Counts one recorded outcome: a failure adds to both failure counts,
    /// a silent or spoken reply ends a failure streak, a stop changes neither.
    pub(crate) fn record(&mut self, status: &ScheduleOutcomeStatus) {
        self.runs = self.runs.saturating_add(1);
        match status {
            ScheduleOutcomeStatus::Failed => {
                self.failures = self.failures.saturating_add(1);
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            }
            ScheduleOutcomeStatus::Silent | ScheduleOutcomeStatus::Spoke => {
                self.consecutive_failures = 0;
            }
            ScheduleOutcomeStatus::Stopped => {}
        }
    }
}

/// A name from the prompt's first non-blank line, at most 80 characters.
pub(crate) fn default_name(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Automation");
    line.chars()
        .filter(|character| !character.is_control())
        .take(MAX_AUTOMATION_NAME_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The name clients show: the stored one, or the prompt's for records saved
/// before M6.
pub(crate) fn display_name(record: &ScheduledPromptRecord) -> String {
    if record.name.trim().is_empty() {
        default_name(&record.prompt)
    } else {
        record.name.clone()
    }
}

/// Restore validation of the M6 fields (the trigger has its own).
pub(crate) fn validate_stored_automation(record: &ScheduledPromptRecord) -> Result<(), String> {
    if record.name.chars().count() > MAX_AUTOMATION_NAME_CHARS
        || record.name.chars().any(char::is_control)
    {
        return Err("has an invalid name".into());
    }
    if let Some(hours) = &record.active_hours {
        if matches!(record.trigger, ScheduleTrigger::Once { .. }) {
            return Err(format!(
                "has invalid active hours: {ACTIVE_HOURS_NOT_FOR_ONCE}"
            ));
        }
        ActiveWindow::parse(hours)
            .map_err(|problem| format!("has invalid active hours: {problem}"))?;
    }
    if let AutomationCreator::Agent {
        agent_id,
        session_id,
        run_id,
        tool_call_id,
    } = &record.created_by
    {
        if [agent_id, session_id, run_id, tool_call_id]
            .iter()
            .any(|value| value.trim().is_empty())
        {
            return Err("has an invalid creator".into());
        }
    }
    let counters = &record.counters;
    if counters.failures > counters.runs || counters.consecutive_failures > counters.failures {
        return Err("has inconsistent counters".into());
    }
    Ok(())
}

/// A new automation, as the owner's routes or the companion's tools ask for it.
#[derive(Clone, Debug)]
pub(crate) struct AutomationInput {
    pub(crate) agent_id: String,
    /// `None`: from the prompt's first line.
    pub(crate) name: Option<String>,
    pub(crate) prompt: String,
    pub(crate) trigger: ScheduleTrigger,
    pub(crate) active_hours: Option<ActiveHours>,
    pub(crate) target: ScheduleTarget,
    pub(crate) enabled: bool,
    pub(crate) preset: Option<AutomationPreset>,
    pub(crate) created_by: AutomationCreator,
    /// A legacy browser import's key: such a create is idempotent and exempt
    /// from the per-agent limit.
    pub(crate) import_idempotency_key: Option<String>,
    pub(crate) explicit_next_due_at_ms: Option<u64>,
    pub(crate) created_at_override_ms: Option<u64>,
}

impl AutomationInput {
    /// An enabled automation the owner makes, with nothing else set.
    pub(crate) fn owner(
        agent_id: String,
        prompt: String,
        trigger: ScheduleTrigger,
        target: ScheduleTarget,
    ) -> Self {
        Self {
            agent_id,
            name: None,
            prompt,
            trigger,
            active_hours: None,
            target,
            enabled: true,
            preset: None,
            created_by: AutomationCreator::Owner,
            import_idempotency_key: None,
            explicit_next_due_at_ms: None,
            created_at_override_ms: None,
        }
    }
}

/// An edit; `active_hours: Some(None)` clears them.
#[derive(Clone, Debug, Default)]
pub(crate) struct AutomationPatch {
    pub(crate) name: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) trigger: Option<ScheduleTrigger>,
    pub(crate) active_hours: Option<Option<ActiveHours>>,
    pub(crate) target: Option<ScheduleTarget>,
    pub(crate) enabled: Option<bool>,
}

impl AutomationPatch {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.prompt.is_none()
            && self.trigger.is_none()
            && self.active_hours.is_none()
            && self.target.is_none()
            && self.enabled.is_none()
    }
}

/// The heartbeat preset (spec §9.2) in the owner's `time_zone`; the caller
/// may replace any field before creating it.
pub(crate) fn heartbeat_input(
    agent_id: String,
    time_zone: &str,
    target: ScheduleTarget,
) -> Result<AutomationInput, ScheduleError> {
    let time_zone = time_zone.trim();
    if time_zone.is_empty() {
        return Err(ScheduleError::Invalid(HEARTBEAT_NEEDS_TIME_ZONE));
    }
    super::timing::parse_time_zone(time_zone).map_err(ScheduleError::Rejected)?;
    let mut input = AutomationInput::owner(
        agent_id,
        HEARTBEAT_PROMPT.into(),
        ScheduleTrigger::Interval {
            interval_ms: HEARTBEAT_INTERVAL_MS,
        },
        target,
    );
    input.name = Some(HEARTBEAT_NAME.into());
    input.preset = Some(AutomationPreset::Heartbeat);
    input.active_hours = Some(ActiveHours {
        start: HEARTBEAT_START.into(),
        end: HEARTBEAT_END.into(),
        days: (0..=6).collect(),
        time_zone: time_zone.into(),
    });
    Ok(input)
}

/// The next `PREVIEW_FIRES` fire times (spec §9.2): the browser never
/// computes schedules itself.
pub(crate) fn preview(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    now_ms: u64,
) -> Result<Vec<u64>, ScheduleError> {
    validate_trigger(trigger)?;
    let trigger = normalized_trigger(trigger.clone());
    let hours = checked_hours(active_hours.cloned(), &trigger)?;
    let window = hours
        .as_ref()
        .map(ActiveWindow::parse)
        .transpose()
        .map_err(ScheduleError::Rejected)?;
    upcoming_fires(&trigger, window.as_ref(), now_ms, PREVIEW_FIRES)
        .map_err(ScheduleError::Rejected)
}

fn checked_prompt(prompt: &str) -> Result<(), ScheduleError> {
    validate_prompt(prompt)?;
    if prompt.chars().any(is_smuggling_character) {
        return Err(ScheduleError::Invalid(AUTOMATION_TEXT_HIDDEN));
    }
    Ok(())
}

fn checked_name(name: &str) -> Result<String, ScheduleError> {
    if name.chars().any(is_hidden_in_one_line) || has_variation_selector_run(name) {
        return Err(ScheduleError::Invalid(AUTOMATION_TEXT_HIDDEN));
    }
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_AUTOMATION_NAME_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err(ScheduleError::Invalid(AUTOMATION_NAME_INVALID));
    }
    Ok(trimmed.to_string())
}

/// A name from the prompt's first line with any invisible character left
/// out, so a name nobody wrote never fails the name check.
fn derived_name(prompt: &str) -> String {
    let visible = default_name(prompt)
        .chars()
        .filter(|character| !is_hidden_in_one_line(*character))
        .collect::<String>();
    let visible = if has_variation_selector_run(&visible) {
        visible
            .chars()
            .filter(|character| !matches!(*character as u32, 0xFE00..=0xFE0F))
            .collect()
    } else {
        visible
    };
    match visible.trim() {
        "" => "Automation".to_string(),
        name => name.to_string(),
    }
}

fn checked_hours(
    hours: Option<ActiveHours>,
    trigger: &ScheduleTrigger,
) -> Result<Option<ActiveHours>, ScheduleError> {
    let Some(hours) = hours else {
        return Ok(None);
    };
    if matches!(trigger, ScheduleTrigger::Once { .. }) {
        return Err(ScheduleError::Rejected(ACTIVE_HOURS_NOT_FOR_ONCE.into()));
    }
    normalized_active_hours(hours)
        .map(Some)
        .map_err(ScheduleError::Rejected)
}

/// Spec §9.3: a companion's automation fires at least 5 minutes apart over
/// its next 10 fires.
fn check_agent_gap(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    now_ms: u64,
) -> Result<(), ScheduleError> {
    let window = active_hours
        .map(ActiveWindow::parse)
        .transpose()
        .map_err(ScheduleError::Rejected)?;
    let fires = upcoming_fires(trigger, window.as_ref(), now_ms, AGENT_GAP_CHECKED_FIRES)
        .map_err(ScheduleError::Rejected)?;
    if fires
        .windows(2)
        .any(|pair| pair[1].saturating_sub(pair[0]) < MIN_AGENT_AUTOMATION_GAP_MS)
    {
        return Err(ScheduleError::Invalid(AGENT_AUTOMATION_TOO_FREQUENT));
    }
    Ok(())
}

/// The owner's and the companion's changes to automations (spec §9). Each
/// runs in its own task holding the control-plane transaction (a dropped
/// request never leaves an unsaved change in memory), saves, puts the
/// previous state back when the save fails, and then announces
/// `automation.updated`.
///
/// Lock order: the scheduler's `jobs` mutex, then the transaction, then the
/// state lock, then leaf mutexes. This service never takes `jobs`.
#[derive(Clone)]
pub(crate) struct AutomationService {
    state: SharedDaemonState,
    transactions: Arc<Mutex<()>>,
}

impl AutomationService {
    pub(crate) fn new(state: SharedDaemonState, transactions: Arc<Mutex<()>>) -> Self {
        Self {
            state,
            transactions,
        }
    }

    /// Runs `work` in its own task under the control-plane transaction, so
    /// that dropping the caller never leaves a change unsaved in memory.
    async fn locked<T, F, Fut>(&self, work: F) -> Result<T, ScheduleError>
    where
        T: Send + 'static,
        F: FnOnce(AutomationService) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ScheduleError>> + Send + 'static,
    {
        let this = self.clone();
        tokio::spawn(async move {
            let _transaction = Arc::clone(&this.transactions).lock_owned().await;
            work(this).await
        })
        .await
        .unwrap_or(Err(ScheduleError::Persistence))
    }

    /// The agent's automations, oldest first.
    pub(crate) async fn list(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ScheduledPromptRecord>, ScheduleError> {
        let state = self.state.read().await;
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

    pub(crate) async fn get(
        &self,
        agent_id: &str,
        schedule_id: &str,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        let state = self.state.read().await;
        if state.get_agent(agent_id).is_none() {
            return Err(ScheduleError::AgentNotFound);
        }
        state
            .schedules
            .get(schedule_id)
            .filter(|item| item.agent_id == agent_id)
            .cloned()
            .ok_or(ScheduleError::NotFound)
    }

    /// Creates an automation; `(record, false)` when a legacy import's key
    /// already made it.
    pub(crate) async fn create(
        &self,
        input: AutomationInput,
        now_ms: u64,
    ) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        checked_prompt(&input.prompt)?;
        let name = match &input.name {
            Some(name) => checked_name(name)?,
            None => derived_name(&input.prompt),
        };
        validate_trigger(&input.trigger)?;
        let trigger = normalized_trigger(input.trigger);
        let active_hours = checked_hours(input.active_hours, &trigger)?;
        let created_at_ms = input.created_at_override_ms.unwrap_or(now_ms);
        if created_at_ms == 0 || created_at_ms > now_ms.saturating_add(300_000) {
            return Err(ScheduleError::Invalid("createdAtMs is invalid"));
        }
        let next_due_at_ms = match input.explicit_next_due_at_ms {
            Some(value) if value > 0 => value,
            Some(_) => return Err(ScheduleError::Invalid("next due time is invalid")),
            None => next_due(&trigger, active_hours.as_ref(), now_ms)?,
        };
        if input
            .import_idempotency_key
            .as_ref()
            .is_some_and(|key| key.trim().is_empty() || key.len() > 256)
        {
            return Err(ScheduleError::Invalid("import idempotency key is invalid"));
        }
        if matches!(input.created_by, AutomationCreator::Agent { .. }) {
            check_agent_gap(&trigger, active_hours.as_ref(), now_ms)?;
        }
        let record = ScheduledPromptRecord {
            id: String::new(),
            import_idempotency_key: input.import_idempotency_key,
            agent_id: input.agent_id,
            prompt: input.prompt,
            trigger,
            enabled: input.enabled,
            target: input.target,
            next_due_at_ms,
            last_fired: None,
            last_safe_outcome: None,
            created_at_ms,
            updated_at_ms: now_ms.max(created_at_ms),
            name,
            active_hours,
            created_by: input.created_by,
            preset: input.preset,
            counters: AutomationCounters::default(),
        };
        self.locked(move |service| async move { service.insert(record, now_ms).await })
            .await
    }

    async fn insert(
        &self,
        mut record: ScheduledPromptRecord,
        now_ms: u64,
    ) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        let persist = {
            let mut state = self.state.write().await;
            if state.get_agent(&record.agent_id).is_none() {
                return Err(ScheduleError::AgentNotFound);
            }
            if let Some(key) = record.import_idempotency_key.as_ref() {
                if let Some(existing) = state
                    .schedules
                    .values()
                    .find(|item| {
                        item.agent_id == record.agent_id
                            && item.import_idempotency_key.as_ref() == Some(key)
                    })
                    .cloned()
                {
                    return Ok((existing, false));
                }
            }
            validate_target(&state, &record.agent_id, &record.target, record.enabled)?;
            let owned = state
                .schedules
                .values()
                .filter(|item| item.agent_id == record.agent_id)
                .count();
            if record.import_idempotency_key.is_none() && owned >= MAX_AUTOMATIONS_PER_AGENT {
                return Err(ScheduleError::Conflict(TOO_MANY_AUTOMATIONS));
            }
            record.id = loop {
                let candidate = next_schedule_id(now_ms);
                if !state.schedules.contains_key(&candidate) {
                    break candidate;
                }
            };
            state.schedules.insert(record.id.clone(), record.clone());
            state.control_plane_persist_request()
        };
        if persist.save().await.is_err() {
            self.state.write().await.schedules.remove(&record.id);
            return Err(ScheduleError::Persistence);
        }
        self.state
            .read()
            .await
            .publish_automation_updated(&record.agent_id, &record.id, false);
        Ok((record, true))
    }

    /// Changes an automation. The due time is computed again when the
    /// trigger or the active hours change, or when it is turned back on.
    pub(crate) async fn update(
        &self,
        agent_id: &str,
        schedule_id: &str,
        patch: AutomationPatch,
        now_ms: u64,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        if patch.is_empty() {
            return Err(ScheduleError::Invalid(NOTHING_TO_UPDATE));
        }
        if let Some(prompt) = &patch.prompt {
            checked_prompt(prompt)?;
        }
        let name = patch.name.as_deref().map(checked_name).transpose()?;
        if let Some(trigger) = &patch.trigger {
            validate_trigger(trigger)?;
        }
        let (agent_id, schedule_id) = (agent_id.to_string(), schedule_id.to_string());
        self.locked(move |service| async move {
            let (updated, previous, persist) = {
                let mut state = service.state.write().await;
                let previous = state
                    .schedules
                    .get(&schedule_id)
                    .filter(|item| item.agent_id == agent_id)
                    .cloned()
                    .ok_or(ScheduleError::NotFound)?;
                let mut updated = previous.clone();
                if let Some(name) = name {
                    updated.name = name;
                }
                if let Some(prompt) = patch.prompt {
                    updated.prompt = prompt;
                }
                if let Some(target) = patch.target {
                    updated.target = target;
                }
                let was_enabled = updated.enabled;
                if let Some(enabled) = patch.enabled {
                    updated.enabled = enabled;
                }
                let retimed = patch.trigger.is_some()
                    || patch.active_hours.is_some()
                    || (!was_enabled && updated.enabled);
                if let Some(trigger) = patch.trigger {
                    updated.trigger = normalized_trigger(trigger);
                }
                if let Some(hours) = patch.active_hours {
                    updated.active_hours = hours;
                }
                // Also catches a new one-time trigger on an automation that
                // keeps its old active hours.
                updated.active_hours = checked_hours(updated.active_hours, &updated.trigger)?;
                if retimed {
                    updated.next_due_at_ms =
                        next_due(&updated.trigger, updated.active_hours.as_ref(), now_ms)?;
                }
                validate_target(&state, &agent_id, &updated.target, updated.enabled)?;
                updated.updated_at_ms = now_ms.max(updated.created_at_ms);
                state.schedules.insert(schedule_id.clone(), updated.clone());
                (updated, previous, state.control_plane_persist_request())
            };
            if persist.save().await.is_err() {
                service
                    .state
                    .write()
                    .await
                    .schedules
                    .insert(schedule_id, previous);
                return Err(ScheduleError::Persistence);
            }
            service.state.read().await.publish_automation_updated(
                &updated.agent_id,
                &updated.id,
                false,
            );
            Ok(updated)
        })
        .await
    }

    /// Turns an automation off; `(record, false)` when it already was.
    pub(crate) async fn pause(
        &self,
        agent_id: &str,
        schedule_id: &str,
        now_ms: u64,
    ) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        let current = self
            .get(agent_id, schedule_id)
            .await
            .map_err(|error| match error {
                ScheduleError::AgentNotFound => ScheduleError::NotFound,
                other => other,
            })?;
        if !current.enabled {
            return Ok((current, false));
        }
        let patch = AutomationPatch {
            enabled: Some(false),
            ..AutomationPatch::default()
        };
        Ok((
            self.update(agent_id, schedule_id, patch, now_ms).await?,
            true,
        ))
    }

    pub(crate) async fn delete(
        &self,
        agent_id: &str,
        schedule_id: &str,
    ) -> Result<(), ScheduleError> {
        let (agent_id, schedule_id) = (agent_id.to_string(), schedule_id.to_string());
        self.locked(move |service| async move {
            let (removed, persist) = {
                let mut state = service.state.write().await;
                if !state
                    .schedules
                    .get(&schedule_id)
                    .is_some_and(|item| item.agent_id == agent_id)
                {
                    return Err(ScheduleError::NotFound);
                }
                let removed = state.schedules.remove(&schedule_id).expect("checked");
                (removed, state.control_plane_persist_request())
            };
            if persist.save().await.is_err() {
                service
                    .state
                    .write()
                    .await
                    .schedules
                    .insert(schedule_id, removed);
                return Err(ScheduleError::Persistence);
            }
            service
                .state
                .read()
                .await
                .publish_automation_updated(&agent_id, &schedule_id, true);
            Ok(())
        })
        .await
    }
}

/// An enabled, never-fired, owner-made workspace automation repeating every
/// 60 seconds, for tests.
#[cfg(test)]
pub(crate) fn test_automation(agent_id: &str, id: &str) -> ScheduledPromptRecord {
    ScheduledPromptRecord {
        id: id.into(),
        import_idempotency_key: None,
        agent_id: agent_id.into(),
        prompt: "Check status".into(),
        trigger: ScheduleTrigger::Interval {
            interval_ms: 60_000,
        },
        enabled: true,
        target: super::ScheduleTarget::Workspace,
        next_due_at_ms: 70_000,
        last_fired: None,
        last_safe_outcome: None,
        created_at_ms: 10_000,
        updated_at_ms: 10_000,
        name: "Check status".into(),
        active_hours: None,
        created_by: AutomationCreator::Owner,
        preset: None,
        counters: AutomationCounters::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedules::{ActiveHours, ScheduleTrigger};

    #[test]
    fn creators_presets_and_counters_serialize_in_camel_case() {
        assert_eq!(MAX_AUTOMATION_NAME_CHARS, 80);
        assert_eq!(
            serde_json::to_value(AutomationCreator::Owner).unwrap(),
            serde_json::json!({"kind": "owner"})
        );
        assert_eq!(
            serde_json::to_value(AutomationCreator::Agent {
                agent_id: "agent-1".into(),
                session_id: "chat:1".into(),
                run_id: "run_1".into(),
                tool_call_id: "call-1".into(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "agent",
                "agentId": "agent-1",
                "sessionId": "chat:1",
                "runId": "run_1",
                "toolCallId": "call-1"
            })
        );
        assert_eq!(
            serde_json::to_value(AutomationPreset::Heartbeat).unwrap(),
            "heartbeat"
        );
        assert_eq!(AutomationPreset::Heartbeat.as_str(), "heartbeat");
        assert_eq!(
            serde_json::to_value(AutomationCounters {
                runs: 3,
                failures: 2,
                consecutive_failures: 1,
            })
            .unwrap(),
            serde_json::json!({"runs": 3, "failures": 2, "consecutiveFailures": 1})
        );
        let legacy: ScheduledPromptRecord = serde_json::from_value({
            let mut value = serde_json::to_value(test_automation("agent-1", "s1")).unwrap();
            let object = value.as_object_mut().unwrap();
            for key in ["name", "activeHours", "createdBy", "preset", "counters"] {
                object.remove(key);
            }
            value
        })
        .unwrap();
        assert_eq!(legacy.name, "");
        assert_eq!(legacy.created_by, AutomationCreator::Owner);
        assert_eq!(legacy.counters, AutomationCounters::default());
    }

    #[test]
    fn counters_follow_each_outcome() {
        let mut counters = AutomationCounters::default();
        counters.record(&ScheduleOutcomeStatus::Failed);
        counters.record(&ScheduleOutcomeStatus::Failed);
        assert_eq!(
            counters,
            AutomationCounters {
                runs: 2,
                failures: 2,
                consecutive_failures: 2
            }
        );
        counters.record(&ScheduleOutcomeStatus::Stopped);
        assert_eq!(counters.consecutive_failures, 2, "a stop is not a success");
        counters.record(&ScheduleOutcomeStatus::Silent);
        assert_eq!(
            counters,
            AutomationCounters {
                runs: 4,
                failures: 2,
                consecutive_failures: 0
            }
        );
        counters.record(&ScheduleOutcomeStatus::Spoke);
        assert_eq!(counters.runs, 5);
    }

    #[test]
    fn a_missing_name_comes_from_the_prompts_first_line() {
        assert_eq!(
            default_name("\n  Check the inbox \nthen more"),
            "Check the inbox"
        );
        assert_eq!(default_name(&"x".repeat(200)).chars().count(), 80);
        assert_eq!(default_name(" \n "), "Automation");
        let mut record = test_automation("agent-1", "s1");
        record.name = String::new();
        record.prompt = "Water the plants".into();
        assert_eq!(display_name(&record), "Water the plants");
        record.name = "Plants".into();
        assert_eq!(display_name(&record), "Plants");
    }

    #[test]
    fn stored_automation_fields_are_validated() {
        let good = test_automation("agent-1", "s1");
        assert_eq!(validate_stored_automation(&good), Ok(()));
        let mut long_name = good.clone();
        long_name.name = "x".repeat(81);
        let mut control_name = good.clone();
        control_name.name = "two\nlines".into();
        let mut bad_hours = good.clone();
        bad_hours.active_hours = Some(ActiveHours {
            start: "08:00".into(),
            end: "08:00".into(),
            days: vec![1],
            time_zone: "UTC".into(),
        });
        let mut once_hours = good.clone();
        once_hours.trigger = ScheduleTrigger::Once { at_ms: 5 };
        once_hours.active_hours = Some(ActiveHours {
            start: "08:00".into(),
            end: "22:00".into(),
            days: vec![1],
            time_zone: "UTC".into(),
        });
        let mut empty_creator = good.clone();
        empty_creator.created_by = AutomationCreator::Agent {
            agent_id: "agent-1".into(),
            session_id: " ".into(),
            run_id: "run_1".into(),
            tool_call_id: "call-1".into(),
        };
        let mut counters = good.clone();
        counters.counters = AutomationCounters {
            runs: 1,
            failures: 2,
            consecutive_failures: 0,
        };
        for bad in [
            long_name,
            control_name,
            bad_hours,
            once_hours,
            empty_creator,
            counters,
        ] {
            assert!(validate_stored_automation(&bad).is_err(), "{bad:?}");
        }
    }

    use std::sync::Arc;

    use tokio::sync::{Mutex, RwLock};

    use crate::agent_runs::test_support::{companion_config, next_event};
    use crate::app::SharedDaemonState;
    use crate::schedules::timing::{ONCE_NOT_IN_FUTURE, TIME_ZONE_INVALID};
    use crate::schedules::{ScheduleError, ScheduleTarget};
    use crate::sessions::test_support::within;
    use crate::state::DaemonState;

    /// 2026-01-05 08:00 UTC, a Monday.
    const NOW: u64 = 1_767_600_000_000;
    const MINUTE: u64 = 60_000;

    fn service() -> (AutomationService, SharedDaemonState, String) {
        let mut daemon = DaemonState::new();
        let agent = daemon
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let state = Arc::new(RwLock::new(daemon));
        (
            AutomationService::new(Arc::clone(&state), Arc::new(Mutex::new(()))),
            state,
            agent,
        )
    }

    fn every(agent: &str, minutes: u64) -> AutomationInput {
        AutomationInput::owner(
            agent.into(),
            "Check status".into(),
            ScheduleTrigger::Interval {
                interval_ms: minutes * MINUTE,
            },
            ScheduleTarget::Workspace,
        )
    }

    fn as_agent(mut input: AutomationInput, agent: &str) -> AutomationInput {
        input.created_by = AutomationCreator::Agent {
            agent_id: agent.into(),
            session_id: "chat:1".into(),
            run_id: "run_1".into(),
            tool_call_id: "call-1".into(),
        };
        input
    }

    fn invalid_snapshot_directory() -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("anima-automation-invalid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_limits_and_strings_are_the_specs() {
        assert_eq!(MAX_AUTOMATIONS_PER_AGENT, 20);
        assert_eq!(MIN_AGENT_AUTOMATION_GAP_MS, 5 * 60 * 1000);
        assert_eq!(AGENT_GAP_CHECKED_FIRES, 10);
        assert_eq!(MAX_AUTOMATION_HISTORY_SHOWN, 50);
        assert_eq!(PREVIEW_FIRES, 3);
        assert_eq!(HEARTBEAT_INTERVAL_MS, 30 * 60 * 1000);
        assert_eq!((HEARTBEAT_START, HEARTBEAT_END), ("08:00", "22:00"));
        assert_eq!(HEARTBEAT_NAME, "Heartbeat");
        assert_eq!(
            HEARTBEAT_PROMPT,
            "Review my open tasks, goals, and recent messages, and tell me briefly about anything that needs my attention."
        );
        assert_eq!(
            TOO_MANY_AUTOMATIONS,
            "This companion already has 20 automations; delete one first"
        );
        assert_eq!(
            AGENT_AUTOMATION_TOO_FREQUENT,
            "Automations you create must run at least 5 minutes apart"
        );
        assert_eq!(
            AUTOMATION_NAME_INVALID,
            "name must be 1–80 characters on one line"
        );
        assert_eq!(
            AUTOMATION_TEXT_HIDDEN,
            "Automation text must not contain invisible tag or direction-override characters"
        );
        assert_eq!(
            HEARTBEAT_NEEDS_TIME_ZONE,
            "timeZone is required for the heartbeat preset"
        );
    }

    #[tokio::test]
    async fn create_names_validates_saves_and_announces() {
        let (service, state, agent) = service();
        let mut stream = state.read().await.live.subscribe(&agent).unwrap();
        let input = AutomationInput::owner(
            agent.clone(),
            "Water the plants\nand the herbs".into(),
            ScheduleTrigger::Cron {
                expression: "0 9 * * *".into(),
                time_zone: "UTC".into(),
            },
            ScheduleTarget::Workspace,
        );

        let (record, created) = service.create(input, NOW).await.unwrap();

        assert!(created);
        assert_eq!(record.name, "Water the plants");
        assert_eq!(
            record.next_due_at_ms,
            NOW + 60 * MINUTE,
            "09:00 the same day"
        );
        assert_eq!(record.created_by, AutomationCreator::Owner);
        assert_eq!(state.read().await.schedules[&record.id], record);
        let event = next_event(&mut stream).await.to_json(1);
        assert_eq!(event["type"], "automation.updated");
        assert_eq!(event["scheduleId"], record.id.as_str());
        assert_eq!(event["deleted"], false);
        assert_eq!(service.list(&agent).await.unwrap(), vec![record.clone()]);
        assert_eq!(service.get(&agent, &record.id).await.unwrap(), record);
        assert_eq!(
            service.get("someone-else", &record.id).await,
            Err(ScheduleError::AgentNotFound)
        );
    }

    #[tokio::test]
    async fn the_twenty_first_automation_is_refused() {
        let (service, _, agent) = service();
        for _ in 0..MAX_AUTOMATIONS_PER_AGENT {
            service.create(every(&agent, 60), NOW).await.unwrap();
        }
        assert_eq!(
            service.create(every(&agent, 60), NOW).await,
            Err(ScheduleError::Conflict(TOO_MANY_AUTOMATIONS))
        );
        let mut legacy = every(&agent, 60);
        legacy.import_idempotency_key = Some("legacy:agent:1".into());
        assert!(
            service.create(legacy, NOW).await.is_ok(),
            "a legacy import is exempt"
        );
    }

    #[tokio::test]
    async fn agent_automations_must_be_five_minutes_apart() {
        let (service, _, agent) = service();
        assert_eq!(
            service
                .create(as_agent(every(&agent, 4), &agent), NOW)
                .await,
            Err(ScheduleError::Invalid(AGENT_AUTOMATION_TOO_FREQUENT))
        );
        let mut cron = as_agent(every(&agent, 60), &agent);
        cron.trigger = ScheduleTrigger::Cron {
            expression: "*/4 * * * *".into(),
            time_zone: "UTC".into(),
        };
        assert_eq!(
            service.create(cron, NOW).await,
            Err(ScheduleError::Invalid(AGENT_AUTOMATION_TOO_FREQUENT))
        );
        assert!(service
            .create(as_agent(every(&agent, 5), &agent), NOW)
            .await
            .is_ok());
        assert!(
            service.create(every(&agent, 1), NOW).await.is_ok(),
            "the owner has no minimum"
        );
        let mut once = as_agent(every(&agent, 60), &agent);
        once.trigger = ScheduleTrigger::Once {
            at_ms: NOW + MINUTE,
        };
        assert!(
            service.create(once, NOW).await.is_ok(),
            "one fire has no gap"
        );
    }

    #[tokio::test]
    async fn hidden_text_and_bad_names_are_refused() {
        let (service, state, agent) = service();
        let mut hidden_prompt = every(&agent, 60);
        hidden_prompt.prompt = "Check \u{202E}status".into();
        let mut hidden_name = every(&agent, 60);
        hidden_name.name = Some("Check\u{200B}".into());
        let mut two_lines = every(&agent, 60);
        two_lines.name = Some("Check\nstatus".into());
        let mut long_name = every(&agent, 60);
        long_name.name = Some("x".repeat(81));
        let mut blank_name = every(&agent, 60);
        blank_name.name = Some("  ".into());
        for (input, problem) in [
            (hidden_prompt, AUTOMATION_TEXT_HIDDEN),
            (hidden_name, AUTOMATION_TEXT_HIDDEN),
            (two_lines, AUTOMATION_NAME_INVALID),
            (long_name, AUTOMATION_NAME_INVALID),
            (blank_name, AUTOMATION_NAME_INVALID),
        ] {
            assert_eq!(
                service.create(input, NOW).await,
                Err(ScheduleError::Invalid(problem))
            );
        }
        assert!(state.read().await.schedules.is_empty());
    }

    #[tokio::test]
    async fn active_hours_are_stored_in_order_and_a_once_refuses_them() {
        let (service, _, agent) = service();
        let mut input = every(&agent, 30);
        input.active_hours = Some(ActiveHours {
            start: "09:00".into(),
            end: "17:00".into(),
            days: vec![5, 1],
            time_zone: "UTC".into(),
        });
        let (record, _) = service.create(input, NOW).await.unwrap();
        assert_eq!(record.active_hours.as_ref().unwrap().days, vec![1, 5]);
        assert_eq!(
            record.next_due_at_ms,
            NOW + 60 * MINUTE,
            "the window opens at 09:00"
        );

        let mut once = every(&agent, 30);
        once.trigger = ScheduleTrigger::Once {
            at_ms: NOW + MINUTE,
        };
        once.active_hours = record.active_hours.clone();
        assert_eq!(
            service.create(once, NOW).await,
            Err(ScheduleError::Rejected(ACTIVE_HOURS_NOT_FOR_ONCE.into()))
        );
        let mut past = every(&agent, 30);
        past.trigger = ScheduleTrigger::Once { at_ms: NOW };
        assert_eq!(
            service.create(past, NOW).await,
            Err(ScheduleError::Rejected(ONCE_NOT_IN_FUTURE.into()))
        );
    }

    #[tokio::test]
    async fn an_update_recomputes_timing_only_when_it_must() {
        let (service, _, agent) = service();
        let (record, _) = service.create(every(&agent, 60), NOW).await.unwrap();
        let renamed = service
            .update(
                &agent,
                &record.id,
                AutomationPatch {
                    name: Some(" Status ".into()),
                    ..AutomationPatch::default()
                },
                NOW + MINUTE,
            )
            .await
            .unwrap();
        assert_eq!(renamed.name, "Status");
        assert_eq!(renamed.next_due_at_ms, record.next_due_at_ms);

        let windowed = service
            .update(
                &agent,
                &record.id,
                AutomationPatch {
                    active_hours: Some(Some(ActiveHours {
                        start: "12:00".into(),
                        end: "13:00".into(),
                        days: vec![1],
                        time_zone: "UTC".into(),
                    })),
                    ..AutomationPatch::default()
                },
                NOW,
            )
            .await
            .unwrap();
        assert_eq!(windowed.next_due_at_ms, NOW + 4 * 60 * MINUTE, "noon");
        let cleared = service
            .update(
                &agent,
                &record.id,
                AutomationPatch {
                    active_hours: Some(None),
                    ..AutomationPatch::default()
                },
                NOW,
            )
            .await
            .unwrap();
        assert_eq!(cleared.active_hours, None);
        assert_eq!(cleared.next_due_at_ms, NOW + 60 * MINUTE);

        let mut once = every(&agent, 60);
        once.trigger = ScheduleTrigger::Once {
            at_ms: NOW + MINUTE,
        };
        once.enabled = false;
        let (once, _) = service.create(once, NOW).await.unwrap();
        assert_eq!(
            service
                .update(
                    &agent,
                    &once.id,
                    AutomationPatch {
                        enabled: Some(true),
                        ..AutomationPatch::default()
                    },
                    NOW + 2 * MINUTE,
                )
                .await,
            Err(ScheduleError::Rejected(ONCE_NOT_IN_FUTURE.into())),
            "a one-time automation whose time passed needs a new time"
        );
        assert_eq!(
            service
                .update(&agent, &record.id, AutomationPatch::default(), NOW)
                .await,
            Err(ScheduleError::Invalid("at least one field is required"))
        );
        assert_eq!(
            service
                .update(
                    "someone-else",
                    &record.id,
                    AutomationPatch {
                        name: Some("Theirs".into()),
                        ..AutomationPatch::default()
                    },
                    NOW,
                )
                .await,
            Err(ScheduleError::NotFound),
            "another agent's automation is not found"
        );
    }

    #[tokio::test]
    async fn pause_is_idempotent_and_delete_announces() {
        let (service, state, agent) = service();
        let (record, _) = service.create(every(&agent, 60), NOW).await.unwrap();
        let (paused, changed) = service.pause(&agent, &record.id, NOW).await.unwrap();
        assert!(changed && !paused.enabled);
        let (_, changed) = service.pause(&agent, &record.id, NOW).await.unwrap();
        assert!(!changed, "already paused: nothing saved");
        assert_eq!(
            service.pause(&agent, "missing", NOW).await,
            Err(ScheduleError::NotFound)
        );

        let mut stream = state.read().await.live.subscribe(&agent).unwrap();
        service.delete(&agent, &record.id).await.unwrap();
        let event = next_event(&mut stream).await.to_json(1);
        assert_eq!(event["deleted"], true);
        assert!(state.read().await.schedules.is_empty());
        assert_eq!(
            service.delete(&agent, &record.id).await,
            Err(ScheduleError::NotFound)
        );
    }

    #[tokio::test]
    async fn a_failed_save_puts_everything_back() {
        use crate::control_plane_store::ControlPlaneStoreConfig;
        let (service, state, agent) = service();
        let (kept, _) = service.create(every(&agent, 60), NOW).await.unwrap();
        let invalid = invalid_snapshot_directory();
        state
            .write()
            .await
            .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid.clone())));

        assert_eq!(
            service.create(every(&agent, 60), NOW).await,
            Err(ScheduleError::Persistence)
        );
        assert_eq!(
            service
                .update(
                    &agent,
                    &kept.id,
                    AutomationPatch {
                        prompt: Some("Changed".into()),
                        ..AutomationPatch::default()
                    },
                    NOW,
                )
                .await,
            Err(ScheduleError::Persistence)
        );
        assert_eq!(
            service.delete(&agent, &kept.id).await,
            Err(ScheduleError::Persistence)
        );
        let guard = state.read().await;
        assert_eq!(guard.schedules.len(), 1);
        assert_eq!(guard.schedules[&kept.id], kept);
        drop(guard);
        let _ = std::fs::remove_dir_all(invalid);
    }

    #[tokio::test]
    async fn a_dropped_request_still_saves_its_automation() {
        let (service, state, agent) = service();
        let transactions = Arc::clone(&service.transactions);
        let held = transactions.lock().await;
        let mut request = Box::pin(service.create(every(&agent, 60), NOW));
        // One poll spawns the change, which then waits for the transaction.
        tokio::select! {
            biased;
            _ = &mut request => panic!("the change cannot finish while the transaction is held"),
            _ = std::future::ready(()) => {}
        }
        drop(request);
        drop(held);

        within("the dropped create to save", async {
            while state.read().await.schedules.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    #[tokio::test]
    async fn the_heartbeat_preset_runs_every_30_minutes_from_8_to_22_local() {
        let (service, _, agent) = service();
        let input = heartbeat_input(
            agent.clone(),
            "Asia/Kuala_Lumpur",
            ScheduleTarget::Workspace,
        )
        .unwrap();
        let (record, _) = service.create(input, NOW).await.unwrap();
        assert_eq!(record.preset, Some(AutomationPreset::Heartbeat));
        assert_eq!(record.name, HEARTBEAT_NAME);
        assert_eq!(record.prompt, HEARTBEAT_PROMPT);
        assert_eq!(
            record.trigger,
            ScheduleTrigger::Interval {
                interval_ms: HEARTBEAT_INTERVAL_MS
            }
        );
        assert_eq!(
            record.active_hours,
            Some(ActiveHours {
                start: HEARTBEAT_START.into(),
                end: HEARTBEAT_END.into(),
                days: vec![0, 1, 2, 3, 4, 5, 6],
                time_zone: "Asia/Kuala_Lumpur".into(),
            })
        );
        // 08:00 UTC is 16:00 in Kuala Lumpur: inside the window.
        assert_eq!(record.next_due_at_ms, NOW + 30 * MINUTE);
        assert_eq!(
            heartbeat_input(agent.clone(), " ", ScheduleTarget::Workspace).err(),
            Some(ScheduleError::Invalid(HEARTBEAT_NEEDS_TIME_ZONE))
        );
        assert_eq!(
            heartbeat_input(agent, "Mars/Base", ScheduleTarget::Workspace).err(),
            Some(ScheduleError::Rejected(TIME_ZONE_INVALID.into()))
        );
    }

    #[test]
    fn previews_list_the_next_three_fires() {
        let trigger = ScheduleTrigger::Interval {
            interval_ms: 30 * MINUTE,
        };
        assert_eq!(
            preview(&trigger, None, NOW),
            Ok(vec![
                NOW + 30 * MINUTE,
                NOW + 60 * MINUTE,
                NOW + 90 * MINUTE
            ])
        );
        assert_eq!(
            preview(&ScheduleTrigger::Interval { interval_ms: 0 }, None, NOW),
            Err(ScheduleError::Rejected(
                crate::schedules::timing::INTERVAL_INVALID.into()
            ))
        );
    }

    #[tokio::test]
    async fn a_derived_name_leaves_invisible_characters_out() {
        let (service, _, agent) = service();
        let mut input = every(&agent, 60);
        input.prompt = "Check\u{200B} status\u{FE0F}\u{FE0F}".into();
        let (record, _) = service.create(input, NOW).await.unwrap();
        assert_eq!(record.name, "Check status");
    }

    #[tokio::test]
    async fn a_one_time_trigger_cannot_keep_active_hours() {
        let (service, state, agent) = service();
        let mut input = every(&agent, 30);
        input.active_hours = Some(ActiveHours {
            start: "09:00".into(),
            end: "17:00".into(),
            days: vec![1],
            time_zone: "UTC".into(),
        });
        let (record, _) = service.create(input, NOW).await.unwrap();
        assert_eq!(
            service
                .update(
                    &agent,
                    &record.id,
                    AutomationPatch {
                        trigger: Some(ScheduleTrigger::Once {
                            at_ms: NOW + MINUTE
                        }),
                        ..AutomationPatch::default()
                    },
                    NOW,
                )
                .await,
            Err(ScheduleError::Rejected(ACTIVE_HOURS_NOT_FOR_ONCE.into()))
        );
        assert_eq!(state.read().await.schedules[&record.id], record);
    }
}
