//! Run bodies (spec §4.1–§4.2, §4.6, §6).

use serde::Serialize;
use utoipa::ToSchema;

use super::shared::TokenUsageResponse;
use crate::runs::RunRecord;

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunInputResponse {
    pub(crate) text: String,
    pub(crate) attachment_ids: Vec<String>,
    pub(crate) skill: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunErrorResponse {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStopResponse {
    pub(crate) requested_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStepResponse {
    pub(crate) step_id: String,
    pub(crate) usage: TokenUsageResponse,
}

/// A ledger run (spec §4.1) as clients see it.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    /// `web`, `api`, `telegram`, `schedule`, `job`, `delegation`, or `peer`.
    pub(crate) source: String,
    pub(crate) source_ref: Option<String>,
    /// `queued`, `running`, `awaiting_approval`, `completed`, `failed`,
    /// `cancelled`, or `interrupted`.
    pub(crate) status: String,
    pub(crate) input: RunInputResponse,
    pub(crate) created_at_ms: u64,
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) error: Option<RunErrorResponse>,
    pub(crate) stop: Option<RunStopResponse>,
    pub(crate) tools_started: Vec<String>,
    pub(crate) steps: Vec<RunStepResponse>,
    pub(crate) usage: TokenUsageResponse,
    pub(crate) model: String,
    pub(crate) provider: Option<String>,
    pub(crate) parent_run_id: Option<String>,
    /// The committed final reply, once the run completed.
    pub(crate) reply_message_id: Option<String>,
}

impl From<&RunRecord> for RunResponse {
    fn from(record: &RunRecord) -> Self {
        Self {
            id: record.id.clone(),
            agent_id: record.agent_id.clone(),
            session_id: record.session_id.clone(),
            source: record.source.as_str().into(),
            source_ref: record.source_ref.clone(),
            status: record.status.as_str().into(),
            input: RunInputResponse {
                text: record.input.text.clone(),
                attachment_ids: record.input.attachment_ids.clone(),
                skill: record.input.skill.clone(),
            },
            created_at_ms: record.created_at_ms,
            started_at_ms: record.started_at_ms,
            finished_at_ms: record.finished_at_ms,
            error: record.error.as_ref().map(|error| RunErrorResponse {
                code: error.code.clone(),
                message: error.message.clone(),
            }),
            stop: record.stop.as_ref().map(|stop| RunStopResponse {
                requested_at_ms: stop.requested_at_ms,
            }),
            tools_started: record.tools_started.clone(),
            steps: record
                .steps
                .iter()
                .map(|step| RunStepResponse {
                    step_id: step.step_id.clone(),
                    usage: TokenUsageResponse::from(&step.usage),
                })
                .collect(),
            usage: TokenUsageResponse::from(&record.usage),
            model: record.model.clone(),
            provider: record.provider.clone(),
            parent_run_id: record.parent_run_id.clone(),
            reply_message_id: record.reply_message_id.clone(),
        }
    }
}

/// A steer that joined the session's active run (spec §4.2).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SteerStatusResponse {
    /// `pending` until the run's next model call drains it.
    pub(crate) status: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunEnvelope {
    pub(crate) run: RunResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) steer: Option<SteerStatusResponse>,
}

impl RunEnvelope {
    pub(crate) fn of(record: &RunRecord) -> Self {
        Self {
            run: RunResponse::from(record),
            steer: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunsEnvelope {
    pub(crate) runs: Vec<RunResponse>,
}
