use serde::Serialize;
use utoipa::ToSchema;

use crate::connectors::gcalendar::{
    CalendarPendingWriteRecord, CalendarWriteOperation, CalendarWriteState,
    GoogleCalendarConnectorRecord,
};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarConnectorResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    #[serde(rename = "type")]
    pub(crate) connector_type: String,
    pub(crate) account_label: Option<String>,
    pub(crate) calendar_ids: Vec<String>,
    /// "pairing" while OAuth consent is outstanding, "reauthRequired" when
    /// Google rejected a token refresh, otherwise "active".
    pub(crate) status: String,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

impl CalendarConnectorResponse {
    pub(crate) fn from_record(value: &GoogleCalendarConnectorRecord) -> Self {
        let status = if value.pending_auth.is_some() {
            "pairing"
        } else if value.reauth_required {
            "reauthRequired"
        } else {
            "active"
        };
        Self {
            id: value.id.clone(),
            agent_id: value.agent_id.clone(),
            connector_type: "gcalendar".to_string(),
            account_label: value.account_label.clone(),
            calendar_ids: value.calendar_ids.clone(),
            status: status.to_string(),
            created_at_ms: value.created_at_ms,
            updated_at_ms: value.updated_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarStatusEnvelope {
    /// Null when the agent has no Google Calendar connector.
    pub(crate) connector: Option<CalendarConnectorResponse>,
    /// Whether the daemon has Google OAuth client credentials configured.
    pub(crate) configured: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarConnectEnvelope {
    pub(crate) connector: CalendarConnectorResponse,
    /// Google consent URL the owner must open to authorize access.
    pub(crate) consent_url: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarEventDraftResponse {
    pub(crate) calendar_id: String,
    pub(crate) event_id: Option<String>,
    pub(crate) title: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) location: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarWriteResponse {
    pub(crate) id: String,
    pub(crate) connector_id: String,
    pub(crate) operation: String,
    pub(crate) draft: CalendarEventDraftResponse,
    pub(crate) summary: String,
    pub(crate) state: String,
    pub(crate) error: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) resolved_at_ms: Option<u64>,
}

impl CalendarWriteResponse {
    pub(crate) fn from_record(value: &CalendarPendingWriteRecord) -> Self {
        Self {
            id: value.id.clone(),
            connector_id: value.connector_id.clone(),
            operation: match value.operation {
                CalendarWriteOperation::Create => "create",
                CalendarWriteOperation::Update => "update",
                CalendarWriteOperation::Delete => "delete",
            }
            .to_string(),
            draft: CalendarEventDraftResponse {
                calendar_id: value.draft.calendar_id.clone(),
                event_id: value.draft.event_id.clone(),
                title: value.draft.title.clone(),
                start: value.draft.start.clone(),
                end: value.draft.end.clone(),
                location: value.draft.location.clone(),
                description: value.draft.description.clone(),
            },
            summary: value.summary.clone(),
            state: match value.state {
                CalendarWriteState::Pending => "pending",
                CalendarWriteState::Applied => "applied",
                CalendarWriteState::Rejected => "rejected",
                CalendarWriteState::Failed => "failed",
            }
            .to_string(),
            error: value.error.clone(),
            created_at_ms: value.created_at_ms,
            resolved_at_ms: value.resolved_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CalendarWritesEnvelope {
    pub(crate) writes: Vec<CalendarWriteResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct CalendarWriteEnvelope {
    pub(crate) write: CalendarWriteResponse,
}
