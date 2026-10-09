//! The status aggregate body (spec §11.3).

use std::collections::BTreeMap;

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusResponse {
    /// The daemon's crate version.
    pub(crate) version: String,
    /// The build revision when the build set `ANIMAOS_BUILD_REVISION`, else null.
    pub(crate) build_revision: Option<String>,
    pub(crate) started_at_ms: u64,
    pub(crate) now_ms: u64,
    pub(crate) uptime_seconds: u64,
    pub(crate) readiness: StatusReadiness,
    pub(crate) storage: StatusStorage,
    pub(crate) providers: Vec<StatusProvider>,
    /// At most 50, oldest first; never a credential.
    pub(crate) connectors: Vec<StatusConnector>,
    pub(crate) automations: StatusAutomations,
    pub(crate) approvals: StatusApprovals,
    pub(crate) runs: StatusRuns,
    pub(crate) events: StatusEvents,
    pub(crate) logs: StatusLogs,
    pub(crate) limits: StatusLimits,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusReadiness {
    /// `ready` or `not_ready`.
    pub(crate) status: String,
    pub(crate) issues: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusStorage {
    pub(crate) persistence_mode: String,
    /// `json`, `postgres`, or `memory`.
    pub(crate) control_plane: String,
    pub(crate) control_plane_durability: String,
    pub(crate) history: StatusHistory,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusHistory {
    /// `memory`, `sqlite`, or `postgres`.
    pub(crate) store: String,
    pub(crate) ephemeral: bool,
    /// False while the store's writes are failing.
    pub(crate) healthy: bool,
    pub(crate) pending_flush: usize,
    pub(crate) usage_queued: usize,
    /// Redacted and at most 500 characters.
    pub(crate) last_error: Option<String>,
    pub(crate) failing_since_ms: Option<u64>,
    pub(crate) flush_errors: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusProvider {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) configured: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusConnector {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    #[serde(rename = "type")]
    pub(crate) connector_type: String,
    pub(crate) status: String,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusAutomations {
    pub(crate) total: usize,
    pub(crate) enabled: usize,
    /// Automations whose latest runs failed in a row.
    pub(crate) failing: usize,
    pub(crate) failures_total: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusApprovals {
    pub(crate) pending: usize,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusRuns {
    pub(crate) running: usize,
    pub(crate) queued: usize,
    /// Runs the ledger still holds, which prunes old finished runs.
    pub(crate) by_status: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusEvents {
    pub(crate) subscribers: usize,
    pub(crate) lagged_events: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusLogs {
    pub(crate) buffered: usize,
    pub(crate) newest_seq: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusLimits {
    pub(crate) max_request_bytes: usize,
    pub(crate) max_concurrent_runs: usize,
    pub(crate) max_runs_per_agent: usize,
    pub(crate) queued_runs_per_agent: usize,
    pub(crate) max_background_processes: usize,
    pub(crate) log_buffer_lines: usize,
    pub(crate) event_buffer: usize,
}
