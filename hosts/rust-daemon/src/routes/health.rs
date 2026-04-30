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
    let (database_configured, background_process_count) = {
        let guard = state.read().await;
        (guard.database_configured(), guard.background_process_count())
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

    ReadinessResponse {
        status: if issues.is_empty() {
            "ready".to_string()
        } else {
            "not_ready".to_string()
        },
        control_plane_durability: "ephemeral".to_string(),
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
    ) = {
        let guard = state.read().await;
        (
            guard.agent_count(),
            guard.swarm_count(),
            guard.swarm_snapshot_count(),
            guard.database_configured(),
            guard.background_process_count(),
            guard.memory_handle(),
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
        "anima_daemon_control_plane_durability_info{mode=\"ephemeral\"} 1".to_string(),
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
