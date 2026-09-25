use crate::app::{DaemonConfig, PersistenceMode, SharedDaemonState};
use crate::routes::contracts::{HealthResponse, ReadinessResponse};

pub(crate) fn handle_health() -> HealthResponse {
    HealthResponse {
        status: "ok".to_string(),
    }
}

pub(crate) async fn handle_readiness(
    state: &SharedDaemonState,
    config: &DaemonConfig,
) -> ReadinessResponse {
    let (database_configured, background_process_count, control_plane_durability, history) = {
        let guard = state.read().await;
        (
            guard.database_configured(),
            guard.background_process_count(),
            guard.control_plane_durability(),
            guard.history.clone(),
        )
    };

    let mut issues = Vec::new();
    if matches!(config.persistence_mode, PersistenceMode::Postgres) && !database_configured {
        issues.push(
            "postgres persistence mode requires a configured database connection".to_string(),
        );
    }
    if let Err(error) = &background_process_count {
        issues.push(format!("background process manager unavailable: {error}"));
    }
    if let Some(issue) = history.readiness_issue(anima_core::primitives::now_millis()) {
        issues.push(issue);
    }

    ReadinessResponse {
        status: if issues.is_empty() {
            "ready".to_string()
        } else {
            "not_ready".to_string()
        },
        control_plane_durability,
        persistence_mode: config.persistence_mode.as_str().to_string(),
        database: if database_configured {
            "configured".to_string()
        } else if matches!(config.persistence_mode, PersistenceMode::Postgres) {
            "missing".to_string()
        } else {
            "disabled".to_string()
        },
        issues,
    }
}

pub(crate) async fn handle_metrics(state: &SharedDaemonState, config: &DaemonConfig) -> String {
    let (
        agent_count,
        swarm_count,
        swarm_snapshot_count,
        database_configured,
        background_process_count,
        memory_handle,
        control_plane_durability,
    ) = {
        let guard = state.read().await;
        (
            guard.agent_count(),
            guard.swarm_count(),
            guard.swarm_snapshot_count(),
            guard.database_configured(),
            guard.background_process_count(),
            guard.memory_handle(),
            guard.control_plane_durability(),
        )
    };

    let memory_count = memory_handle.read().await.size();
    let background_process_manager_healthy = background_process_count.is_ok();
    let running_background_processes = background_process_count.unwrap_or(0);
    let ready = background_process_manager_healthy
        && match config.persistence_mode {
            PersistenceMode::Memory => true,
            PersistenceMode::Postgres => database_configured,
        };

    [
        "# HELP anima_daemon_ready Whether the daemon is ready to serve traffic.".to_string(),
        "# TYPE anima_daemon_ready gauge".to_string(),
        format!("anima_daemon_ready {}", usize::from(ready)),
        "# HELP anima_daemon_agents Current in-memory agent runtime count.".to_string(),
        "# TYPE anima_daemon_agents gauge".to_string(),
        format!("anima_daemon_agents {}", agent_count),
        "# HELP anima_daemon_swarms Current in-memory swarm coordinator count.".to_string(),
        "# TYPE anima_daemon_swarms gauge".to_string(),
        format!("anima_daemon_swarms {}", swarm_count),
        "# HELP anima_daemon_swarm_snapshots Current stored swarm snapshot count.".to_string(),
        "# TYPE anima_daemon_swarm_snapshots gauge".to_string(),
        format!("anima_daemon_swarm_snapshots {}", swarm_snapshot_count),
        "# HELP anima_daemon_memories Current in-memory memory count.".to_string(),
        "# TYPE anima_daemon_memories gauge".to_string(),
        format!("anima_daemon_memories {}", memory_count),
        "# HELP anima_daemon_background_processes Current running background process count.".to_string(),
        "# TYPE anima_daemon_background_processes gauge".to_string(),
        format!(
            "anima_daemon_background_processes {}",
            running_background_processes
        ),
        "# HELP anima_daemon_background_process_manager_healthy Whether the background process manager is healthy.".to_string(),
        "# TYPE anima_daemon_background_process_manager_healthy gauge".to_string(),
        format!(
            "anima_daemon_background_process_manager_healthy {}",
            usize::from(background_process_manager_healthy)
        ),
        "# HELP anima_daemon_database_configured Whether a database adapter is configured.".to_string(),
        "# TYPE anima_daemon_database_configured gauge".to_string(),
        format!(
            "anima_daemon_database_configured {}",
            usize::from(database_configured)
        ),
        "# HELP anima_daemon_persistence_mode_info Current persistence mode.".to_string(),
        "# TYPE anima_daemon_persistence_mode_info gauge".to_string(),
        format!(
            "anima_daemon_persistence_mode_info{{mode=\"{}\"}} 1",
            config.persistence_mode.as_str()
        ),
        "# HELP anima_daemon_control_plane_durability_info Current control plane durability mode.".to_string(),
        "# TYPE anima_daemon_control_plane_durability_info gauge".to_string(),
        format!(
            "anima_daemon_control_plane_durability_info{{mode=\"{}\"}} 1",
            control_plane_durability
        ),
        "# HELP anima_daemon_max_request_bytes Configured max request bytes.".to_string(),
        "# TYPE anima_daemon_max_request_bytes gauge".to_string(),
        format!("anima_daemon_max_request_bytes {}", config.max_request_bytes),
        "# HELP anima_daemon_request_timeout_seconds Configured request timeout in seconds.".to_string(),
        "# TYPE anima_daemon_request_timeout_seconds gauge".to_string(),
        format!(
            "anima_daemon_request_timeout_seconds {}",
            config.request_timeout.as_secs_f64()
        ),
        "# HELP anima_daemon_max_concurrent_runs Configured max concurrent run requests.".to_string(),
        "# TYPE anima_daemon_max_concurrent_runs gauge".to_string(),
        format!(
            "anima_daemon_max_concurrent_runs {}",
            config.max_concurrent_runs
        ),
        "# HELP anima_daemon_max_background_processes Configured max running background processes.".to_string(),
        "# TYPE anima_daemon_max_background_processes gauge".to_string(),
        format!(
            "anima_daemon_max_background_processes {}",
            config.max_background_processes
        ),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::{HistoryService, HISTORY_READINESS_GRACE_MS};
    use crate::state::DaemonState;
    use anima_core::MessageRole;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn readiness_reports_a_history_store_that_has_failed_for_five_minutes() {
        let store = Arc::new(FlakyHistoryStore::new());
        store.set_failing(true);
        let history = HistoryService::new(store);
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let config = DaemonConfig::default();
        assert_eq!(handle_readiness(&state, &config).await.status, "ready");

        history.enqueue_committed(
            "agent-1",
            "chat:one",
            &[
                history_message("msg-1-1", "agent-1", "chat:one", MessageRole::User, "hi", 1)
                    .message,
            ],
        );
        let long_ago = anima_core::primitives::now_millis() - HISTORY_READINESS_GRACE_MS - 1_000;
        let transactions = tokio::sync::Mutex::new(());
        assert!(history
            .flush_once(&state, &transactions, long_ago)
            .await
            .is_err());

        let response = handle_readiness(&state, &config).await;
        assert_eq!(response.status, "not_ready");
        assert!(
            response
                .issues
                .iter()
                .any(|issue| issue.starts_with("history store writes have failed for 5 minutes")),
            "{:?}",
            response.issues
        );
    }
}
