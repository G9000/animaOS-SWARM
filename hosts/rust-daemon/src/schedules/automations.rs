//! Automations (spec §9): what M6 adds to a scheduled prompt (a name,
//! active hours, who made it, a preset, and counters). Task 5 adds the
//! limits, the strings, the heartbeat preset, and `AutomationService`.
#![allow(dead_code)] // M6 Task 8 uses every item.

use serde::{Deserialize, Serialize};

use super::timing::{ActiveWindow, ACTIVE_HOURS_NOT_FOR_ONCE};
use super::{ScheduleOutcomeStatus, ScheduleTrigger, ScheduledPromptRecord};

/// The longest name, in characters (spec §9.1).
pub(crate) const MAX_AUTOMATION_NAME_CHARS: usize = 80;

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
}
