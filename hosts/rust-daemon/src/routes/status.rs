//! The status aggregate (spec §11.3): one snapshot behind `GET /api/status`
//! and the richer `/metrics`, so the two cannot disagree. The snapshot clones
//! what it needs under the state read lock and drops the guard before it
//! awaits anything else.

use std::collections::BTreeMap;

use anima_core::primitives::now_millis;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;

use super::contracts::{
    connector_status_name, ErrorBody, StatusApprovals, StatusAutomations, StatusConnector,
    StatusEvents, StatusHistory, StatusLimits, StatusLogs, StatusProvider, StatusReadiness,
    StatusResponse, StatusRuns, StatusStorage,
};
use super::health::handle_readiness;
use super::http::json_response;
use super::jobs::{authorize, no_store};
use super::{provider_responses, AppState};
use crate::agent_runs::MAX_QUEUED_RUNS_PER_AGENT;
use crate::app::{DaemonConfig, SharedDaemonState};
use crate::connectors::runtime::{ConnectorManager, ConnectorRuntimeStatus};
use crate::logs::{redact, LogBuffer, LOG_BUFFER_LINES};

/// Connectors listed in the status response.
pub(crate) const MAX_STATUS_CONNECTORS: usize = 50;
/// The most characters of the history store's last error that are shown.
pub(crate) const MAX_STATUS_ERROR_CHARS: usize = 500;

/// Everything the status response and the metrics read.
pub(crate) struct StatusSnapshot {
    pub(crate) response: StatusResponse,
    /// Messages the pruner has moved out of the hot tail since start.
    pub(crate) pruned_messages: u64,
}

/// A store error is redacted first (it can echo a URL or a key), then cut.
fn shown_error(error: &str) -> String {
    redact(error).chars().take(MAX_STATUS_ERROR_CHARS).collect()
}

/// The full snapshot behind `GET /api/status`.
pub(crate) async fn collect(
    state: &SharedDaemonState,
    config: &DaemonConfig,
    logs: &LogBuffer,
    connector_manager: &ConnectorManager,
    started_at_ms: u64,
    now_ms: u64,
) -> StatusSnapshot {
    collect_parts(
        state,
        config,
        logs,
        Some(connector_manager),
        started_at_ms,
        now_ms,
    )
    .await
}

/// The snapshot `/metrics` reads: no metric comes from the providers or the
/// connectors, so an unauthenticated scrape reads no credential vault and
/// awaits no connector; both lists are left empty.
pub(crate) async fn collect_for_metrics(
    state: &SharedDaemonState,
    config: &DaemonConfig,
    logs: &LogBuffer,
    started_at_ms: u64,
    now_ms: u64,
) -> StatusSnapshot {
    collect_parts(state, config, logs, None, started_at_ms, now_ms).await
}

/// `connector_manager` is `None` for the metrics, which skip the providers
/// and the connectors.
async fn collect_parts(
    state: &SharedDaemonState,
    config: &DaemonConfig,
    logs: &LogBuffer,
    connector_manager: Option<&ConnectorManager>,
    started_at_ms: u64,
    now_ms: u64,
) -> StatusSnapshot {
    let readiness = handle_readiness(state, config).await;
    let (
        history,
        hub,
        run_counts,
        mut connector_records,
        automations,
        approvals_pending,
        control_plane_durability,
    ) = {
        let guard = state.read().await;
        let automations = guard.schedules.values().fold(
            StatusAutomations {
                total: 0,
                enabled: 0,
                failing: 0,
                failures_total: 0,
            },
            |mut totals, schedule| {
                totals.total += 1;
                totals.enabled += usize::from(schedule.enabled);
                totals.failing += usize::from(schedule.counters.consecutive_failures > 0);
                totals.failures_total = totals
                    .failures_total
                    .saturating_add(schedule.counters.failures);
                totals
            },
        );
        let connector_records = if connector_manager.is_some() {
            guard
                .connectors
                .values()
                .filter(|connector| connector.deleted_at_ms.is_none())
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        (
            guard.history.clone(),
            guard.live.clone(),
            guard.runs.status_counts(),
            connector_records,
            automations,
            guard.approvals.pending().len(),
            guard.control_plane_durability(),
        )
    };
    connector_records.sort_by(|left, right| {
        left.created_at_ms
            .cmp(&right.created_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    connector_records.truncate(MAX_STATUS_CONNECTORS);

    let mut connectors = Vec::with_capacity(connector_records.len());
    for record in &connector_records {
        let Some(connector_manager) = connector_manager else {
            break;
        };
        let status = connector_manager.status(&record.id).await.unwrap_or(
            if record.approved_chat.is_some() {
                ConnectorRuntimeStatus::Ready
            } else {
                ConnectorRuntimeStatus::Pairing
            },
        );
        connectors.push(StatusConnector {
            id: record.id.clone(),
            agent_id: record.agent_id.clone(),
            connector_type: "telegram".into(),
            status: connector_status_name(status).into(),
            enabled: record.enabled,
        });
    }
    let providers = match connector_manager {
        Some(_) => provider_responses(state)
            .await
            .into_iter()
            .map(|provider| StatusProvider {
                id: provider.id,
                label: provider.label,
                configured: provider.configured,
            })
            .collect(),
        None => Vec::new(),
    };

    let stats = history.stats();
    let by_status = run_counts
        .iter()
        .map(|(status, count)| ((*status).to_string(), *count))
        .collect::<BTreeMap<_, _>>();
    let count_of = |status: &str| run_counts.get(status).copied().unwrap_or(0);
    let response = StatusResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_revision: option_env!("ANIMAOS_BUILD_REVISION").map(str::to_string),
        started_at_ms,
        now_ms,
        uptime_seconds: now_ms.saturating_sub(started_at_ms) / 1_000,
        readiness: StatusReadiness {
            status: readiness.status,
            // An issue can quote the history store's last error.
            issues: readiness
                .issues
                .iter()
                .map(|issue| shown_error(issue))
                .collect(),
        },
        storage: StatusStorage {
            persistence_mode: readiness.persistence_mode,
            control_plane: if control_plane_durability == "ephemeral" {
                "memory".into()
            } else {
                control_plane_durability.clone()
            },
            control_plane_durability,
            history: StatusHistory {
                store: history.store().label().to_string(),
                ephemeral: history.is_ephemeral(),
                healthy: stats.failing_since_ms.is_none(),
                pending_flush: stats.pending,
                usage_queued: stats.usage_queued,
                last_error: stats.last_error.as_deref().map(shown_error),
                failing_since_ms: stats.failing_since_ms,
                flush_errors: stats.flush_errors,
            },
        },
        providers,
        connectors,
        automations,
        approvals: StatusApprovals {
            pending: approvals_pending,
        },
        runs: StatusRuns {
            running: count_of("running"),
            queued: count_of("queued"),
            by_status,
        },
        events: StatusEvents {
            subscribers: hub.total_subscribers(),
            lagged_events: hub.lagged_events(),
        },
        logs: StatusLogs {
            buffered: logs.buffered(),
            newest_seq: logs.newest_seq(),
        },
        limits: StatusLimits {
            max_request_bytes: config.max_request_bytes,
            max_concurrent_runs: config.max_concurrent_runs,
            max_runs_per_agent: config.max_runs_per_agent,
            queued_runs_per_agent: MAX_QUEUED_RUNS_PER_AGENT,
            max_background_processes: config.max_background_processes,
            log_buffer_lines: LOG_BUFFER_LINES,
            event_buffer: config.session_event_buffer,
        },
    };
    StatusSnapshot {
        response,
        pruned_messages: stats.pruned_messages,
    }
}

#[utoipa::path(get, path = "/api/status", tag = "status",
    responses(
        (status = 200, description = "One snapshot of the daemon's health, storage, providers, connectors, automations, approvals, runs, streams, logs, and limits", body = StatusResponse),
        (status = 403, description = "Local owner required", body = ErrorBody)
    ))]
pub(super) async fn get_status(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let snapshot = collect(
        &state.daemon,
        &state.config,
        &state.logs,
        &state.connector_manager,
        state.started_at_ms,
        now_millis(),
    )
    .await;
    no_store(json_response(StatusCode::OK, &snapshot.response))
}
