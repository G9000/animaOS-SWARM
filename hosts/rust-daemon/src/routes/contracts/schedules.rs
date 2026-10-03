use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use crate::schedules::{
    display_name, is_running, ActiveHours, AutomationCreator, ScheduleFireRecord,
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
    Cron {
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    Once {
        #[serde(rename = "atMs")]
        at_ms: u64,
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
            ScheduleTriggerRequest::Cron {
                expression,
                time_zone,
            } => Self::Cron {
                expression,
                time_zone,
            },
            ScheduleTriggerRequest::Once { at_ms } => Self::Once { at_ms },
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

/// When an automation may fire (spec §9.1): `HH:MM` wall times, days with
/// 0 for Sunday, and the time zone they are read in.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActiveHoursBody {
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) days: Vec<u8>,
    pub(crate) time_zone: String,
}

impl From<ActiveHoursBody> for ActiveHours {
    fn from(value: ActiveHoursBody) -> Self {
        Self {
            start: value.start,
            end: value.end,
            days: value.days,
            time_zone: value.time_zone,
        }
    }
}

impl From<ActiveHours> for ActiveHoursBody {
    fn from(value: ActiveHours) -> Self {
        Self {
            start: value.start,
            end: value.end,
            days: value.days,
            time_zone: value.time_zone,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PresetRequest {
    Heartbeat,
}

/// `Some(None)` for an explicit `null`, so a PATCH can clear a field.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Without `preset`, `prompt` and `trigger` are required; with
/// `preset: "heartbeat"`, `timeZone` is, and any field named here replaces
/// the preset's.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScheduleCreateRequest {
    pub(crate) prompt: Option<String>,
    pub(crate) trigger: Option<ScheduleTriggerRequest>,
    /// Defaults to the automation's own thread (`workspace`).
    pub(crate) target: Option<ScheduleTargetRequest>,
    pub(crate) enabled: Option<bool>,
    pub(crate) import_idempotency_key: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) active_hours: Option<ActiveHoursBody>,
    pub(crate) preset: Option<PresetRequest>,
    /// The owner's IANA time zone; read only with a preset.
    pub(crate) time_zone: Option<String>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScheduleUpdateRequest {
    pub(crate) prompt: Option<String>,
    pub(crate) trigger: Option<ScheduleTriggerRequest>,
    pub(crate) target: Option<ScheduleTargetRequest>,
    pub(crate) enabled: Option<bool>,
    pub(crate) name: Option<String>,
    /// `null` clears the active hours.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<ActiveHoursBody>)]
    pub(crate) active_hours: Option<Option<ActiveHoursBody>>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SchedulePreviewRequest {
    pub(crate) trigger: ScheduleTriggerRequest,
    pub(crate) active_hours: Option<ActiveHoursBody>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SchedulePreviewResponse {
    /// The next fire times, oldest first (one for a one-time automation).
    pub(crate) next_runs: Vec<u64>,
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
    Cron {
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    Once {
        #[serde(rename = "atMs")]
        at_ms: u64,
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
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum AutomationCreatorResponse {
    Owner,
    #[serde(rename_all = "camelCase")]
    Agent {
        agent_id: String,
        session_id: String,
        run_id: String,
        tool_call_id: String,
    },
}

impl From<AutomationCreator> for AutomationCreatorResponse {
    fn from(value: AutomationCreator) -> Self {
        match value {
            AutomationCreator::Owner => Self::Owner,
            AutomationCreator::Agent {
                agent_id,
                session_id,
                run_id,
                tool_call_id,
            } => Self::Agent {
                agent_id,
                session_id,
                run_id,
                tool_call_id,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationCountersResponse {
    pub(crate) runs: u64,
    pub(crate) failures: u64,
    pub(crate) consecutive_failures: u64,
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
    pub(crate) name: String,
    pub(crate) active_hours: Option<ActiveHoursBody>,
    pub(crate) created_by: AutomationCreatorResponse,
    /// `heartbeat` or null.
    pub(crate) preset: Option<String>,
    pub(crate) counters: AutomationCountersResponse,
    /// Its latest occurrence has no outcome yet.
    pub(crate) running: bool,
}

impl From<ScheduledPromptRecord> for ScheduleResponse {
    fn from(value: ScheduledPromptRecord) -> Self {
        let name = display_name(&value);
        let running = is_running(&value);
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
            ScheduleTrigger::Cron {
                expression,
                time_zone,
            } => ScheduleTriggerResponse::Cron {
                expression,
                time_zone,
            },
            ScheduleTrigger::Once { at_ms } => ScheduleTriggerResponse::Once { at_ms },
        };
        let target = match value.target {
            ScheduleTarget::Workspace => ScheduleTargetResponse::Workspace,
            ScheduleTarget::Connector { connector_id } => {
                ScheduleTargetResponse::Connector { connector_id }
            }
        };
        let last_outcome = value.last_safe_outcome.map(|item| ScheduleOutcomeResponse {
            status: item.status.contract_name().into(),
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
            name,
            active_hours: value.active_hours.map(Into::into),
            created_by: value.created_by.into(),
            preset: value.preset.map(|preset| preset.as_str().to_string()),
            counters: AutomationCountersResponse {
                runs: value.counters.runs,
                failures: value.counters.failures,
                consecutive_failures: value.counters.consecutive_failures,
            },
            running,
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

/// One occurrence (spec §9.1). `outcome` is `silent`, `spoke`, `failed`, or
/// `stopped` (the schedule's `lastOutcome.status` says `error` for a failure).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleRunResponse {
    pub(crate) id: String,
    pub(crate) schedule_id: String,
    pub(crate) agent_id: String,
    pub(crate) fired_at_ms: u64,
    pub(crate) finished_at_ms: u64,
    pub(crate) outcome: String,
    pub(crate) run_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) manual: bool,
}

impl From<ScheduleFireRecord> for ScheduleRunResponse {
    fn from(value: ScheduleFireRecord) -> Self {
        let outcome = match value.outcome {
            ScheduleOutcomeStatus::Silent => "silent",
            ScheduleOutcomeStatus::Spoke => "spoke",
            ScheduleOutcomeStatus::Failed => "failed",
            ScheduleOutcomeStatus::Stopped => "stopped",
        };
        Self {
            id: value.id,
            schedule_id: value.schedule_id,
            agent_id: value.agent_id,
            fired_at_ms: value.fired_at_ms,
            finished_at_ms: value.finished_at_ms,
            outcome: outcome.into(),
            run_id: value.run_id,
            session_id: value.session_id,
            error_code: value.error_code,
            manual: value.manual,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ScheduleRunsEnvelope {
    pub(crate) runs: Vec<ScheduleRunResponse>,
}
