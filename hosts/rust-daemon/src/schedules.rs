use serde::{Deserialize, Serialize};

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ScheduleOutcomeStatus {
    Succeeded,
    Failed,
}

fn default_enabled() -> bool {
    true
}
