use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::schedules::{
    ScheduleOutcomeStatus, ScheduleTarget, ScheduleTrigger, ScheduledPromptRecord,
};

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub(crate) enum ScheduleTriggerRequest {
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

impl From<ScheduleTriggerRequest> for ScheduleTrigger {
    fn from(value: ScheduleTriggerRequest) -> Self {
        match value {
            ScheduleTriggerRequest::Interval { interval_ms } => Self::Interval { interval_ms },
            ScheduleTriggerRequest::Daily {
                hour,
                minute,
                time_zone,
            } => Self::Daily {
                hour,
                minute,
                time_zone,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub(crate) enum ScheduleTargetRequest {
    Workspace,
    Connector {
        #[serde(rename = "connectorId")]
        connector_id: String,
    },
}

impl From<ScheduleTargetRequest> for ScheduleTarget {
    fn from(value: ScheduleTargetRequest) -> Self {
        match value {
            ScheduleTargetRequest::Workspace => Self::Workspace,
            ScheduleTargetRequest::Connector { connector_id } => Self::Connector { connector_id },
        }
    }
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScheduleCreateRequest {
    pub(crate) prompt: String,
    pub(crate) trigger: ScheduleTriggerRequest,
    pub(crate) target: ScheduleTargetRequest,
    pub(crate) enabled: Option<bool>,
    pub(crate) import_idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScheduleUpdateRequest {
    pub(crate) prompt: Option<String>,
    pub(crate) trigger: Option<ScheduleTriggerRequest>,
    pub(crate) target: Option<ScheduleTargetRequest>,
    pub(crate) enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LegacyScheduleImportRequest {
    pub(crate) schedules: Vec<LegacyScheduleItemRequest>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LegacyScheduleItemRequest {
    pub(crate) id: String,
    pub(crate) prompt: String,
    pub(crate) interval_secs: u64,
    pub(crate) created_at_ms: u64,
    pub(crate) last_run_at_ms: Option<u64>,
    pub(crate) target: Option<ScheduleTargetRequest>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum ScheduleTriggerResponse {
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

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum ScheduleTargetResponse {
    Workspace,
    Connector {
        #[serde(rename = "connectorId")]
        connector_id: String,
    },
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleOutcomeResponse {
    pub(crate) status: String,
    pub(crate) occurred_at_ms: u64,
    pub(crate) error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleResponse {
    pub(crate) id: String,
    pub(crate) import_idempotency_key: Option<String>,
    pub(crate) agent_id: String,
    pub(crate) prompt: String,
    pub(crate) trigger: ScheduleTriggerResponse,
    pub(crate) enabled: bool,
    pub(crate) target: ScheduleTargetResponse,
    pub(crate) next_due_at_ms: u64,
    pub(crate) last_fired_at_ms: Option<u64>,
    pub(crate) last_outcome: Option<ScheduleOutcomeResponse>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

impl From<ScheduledPromptRecord> for ScheduleResponse {
    fn from(value: ScheduledPromptRecord) -> Self {
        let trigger = match value.trigger {
            ScheduleTrigger::Interval { interval_ms } => {
                ScheduleTriggerResponse::Interval { interval_ms }
            }
            ScheduleTrigger::Daily {
                hour,
                minute,
                time_zone,
            } => ScheduleTriggerResponse::Daily {
                hour,
                minute,
                time_zone,
            },
        };
        let target = match value.target {
            ScheduleTarget::Workspace => ScheduleTargetResponse::Workspace,
            ScheduleTarget::Connector { connector_id } => {
                ScheduleTargetResponse::Connector { connector_id }
            }
        };
        let last_outcome = value.last_safe_outcome.map(|item| ScheduleOutcomeResponse {
            status: match item.status {
                ScheduleOutcomeStatus::Silent => "silent",
                ScheduleOutcomeStatus::Spoke => "spoke",
                ScheduleOutcomeStatus::Failed => "error",
            }
            .into(),
            occurred_at_ms: item.occurred_at_ms,
            error_code: item.error_code,
        });
        Self {
            id: value.id,
            import_idempotency_key: value.import_idempotency_key,
            agent_id: value.agent_id,
            prompt: value.prompt,
            trigger,
            enabled: value.enabled,
            target,
            next_due_at_ms: value.next_due_at_ms,
            last_fired_at_ms: value.last_fired.map(|item| item.fired_at_ms),
            last_outcome,
            created_at_ms: value.created_at_ms,
            updated_at_ms: value.updated_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ScheduleEnvelope {
    pub(crate) schedule: ScheduleResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SchedulesEnvelope {
    pub(crate) schedules: Vec<ScheduleResponse>,
}
