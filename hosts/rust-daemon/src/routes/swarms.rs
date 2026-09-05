pub(super) mod events;

use anima_swarm::SwarmStatus;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use tokio::sync::OwnedSemaphorePermit;
use tracing::warn;

use self::events::{publish_swarm_event, subscribe_swarm_events_response};
use super::contracts::{
    SwarmCreateRequest, SwarmEnvelope, SwarmRunEnvelope, SwarmStateResponse, SwarmsEnvelope,
    TaskRequest, TaskResultResponse,
};
use super::ApiError;
use crate::agent_runs::AgentRunCoordinator;
use crate::app::SharedDaemonState;

pub(crate) async fn handle_create_swarm(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<SwarmEnvelope, ApiError> {
    let request: SwarmCreateRequest = super::parse_json_body(body)?;
    let config = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let (coordinator, event_stream, global_event_fanout) = {
        let guard = state.read().await;
        let (coordinator, event_stream) =
            guard.build_swarm(config).map_err(ApiError::bad_request)?;
        (coordinator, event_stream, guard.event_fanout())
    };
    let registered_event_stream = event_stream.clone();

    if let Err(message) = coordinator.start().await {
        return Err(ApiError::bad_request(message));
    }

    let (snapshot, persist_request) = {
        let mut guard = state.write().await;
        let snapshot = guard.register_swarm(coordinator, event_stream);
        (snapshot, guard.control_plane_persist_request())
    };
    persist_request
        .save()
        .await
        .map_err(|error| ApiError::service_unavailable(error.to_string()))?;
    publish_swarm_event(
        &global_event_fanout,
        Some(&registered_event_stream),
        &snapshot.id,
        "swarm:created",
        &snapshot,
        None,
    );

    Ok(SwarmEnvelope {
        swarm: SwarmStateResponse::from(&snapshot),
    })
}

pub(crate) async fn handle_list_swarms(
    state: &SharedDaemonState,
) -> Result<SwarmsEnvelope, ApiError> {
    let snapshots = {
        let guard = state.read().await;
        guard.list_swarms()
    };

    Ok(SwarmsEnvelope {
        swarms: snapshots.iter().map(SwarmStateResponse::from).collect(),
    })
}

pub(crate) async fn handle_get_swarm(
    swarm_id: &str,
    state: &SharedDaemonState,
) -> Result<SwarmEnvelope, ApiError> {
    let snapshot = {
        let guard = state.read().await;
        guard.get_swarm(swarm_id)
    };

    match snapshot {
        Some(snapshot) => Ok(SwarmEnvelope {
            swarm: SwarmStateResponse::from(&snapshot),
        }),
        None => Err(ApiError::not_found()),
    }
}

pub(crate) async fn handle_run_swarm(
    swarm_id: &str,
    body: Vec<u8>,
    state: &SharedDaemonState,
    agent_runs: &AgentRunCoordinator,
    permit: OwnedSemaphorePermit,
) -> Result<SwarmRunEnvelope, ApiError> {
    let request: TaskRequest = super::parse_json_body(body)?;
    let content = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let state = state.clone();
    let agent_runs = agent_runs.clone();
    let swarm_id = swarm_id.to_owned();
    // The HTTP waiter may time out or disconnect; the admitted worker still
    // owns execution, final persistence, cleanup, and its concurrency permit.
    tokio::spawn(async move {
        let _permit = permit;
        let result = run_swarm_owned(&swarm_id, content, &state, &agent_runs).await;
        if result.is_err() {
            warn!(%swarm_id, "owned swarm run failed to execute or commit");
        }
        result
    })
    .await
    .map_err(|error| {
        warn!(%error, "swarm run worker stopped unexpectedly");
        ApiError::service_unavailable("swarm run worker stopped unexpectedly")
    })?
}

async fn run_swarm_owned(
    swarm_id: &str,
    content: anima_core::Content,
    state: &SharedDaemonState,
    agent_runs: &AgentRunCoordinator,
) -> Result<SwarmRunEnvelope, ApiError> {
    let run_lock = state
        .read()
        .await
        .swarm_run_locks
        .get(swarm_id)
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    // Core dispatch serialization ends before the host's durable final commit.
    // Keep same-swarm requests serialized through that commit and publication.
    let _run_guard = run_lock.lock_owned().await;

    let (coordinator, global_event_fanout, swarm_event_fanout) = {
        let guard = state.read().await;
        (
            guard.get_swarm_coordinator(swarm_id),
            guard.event_fanout(),
            guard.swarm_event_fanout(swarm_id),
        )
    };

    let Some(coordinator) = coordinator else {
        return Err(ApiError::not_found());
    };

    let running_transaction = agent_runs.control_plane_transaction().await;
    let (previous_snapshot, persist_request) = {
        let mut running_snapshot = coordinator.get_state();
        running_snapshot.status = SwarmStatus::Running;
        running_snapshot.started_at = Some(anima_core::primitives::now_millis());
        running_snapshot.completed_at = None;
        running_snapshot.token_usage = Default::default();
        let mut guard = state.write().await;
        let previous_snapshot = guard.swarm_snapshots.get(swarm_id).cloned();
        guard.store_swarm_snapshot(running_snapshot);
        (previous_snapshot, guard.control_plane_persist_request())
    };
    if let Err(error) = persist_request.save().await {
        if let Some(previous_snapshot) = previous_snapshot {
            state.write().await.store_swarm_snapshot(previous_snapshot);
        }
        return Err(ApiError::service_unavailable(error.to_string()));
    }
    drop(running_transaction);

    let running_swarm_id = swarm_id.to_string();
    let running_global_event_fanout = global_event_fanout.clone();
    let running_swarm_event_fanout = swarm_event_fanout.clone();
    let result = coordinator
        .dispatch_content_with_running_hook(content, move |snapshot| {
            publish_swarm_event(
                &running_global_event_fanout,
                running_swarm_event_fanout.as_ref(),
                &running_swarm_id,
                "swarm:running",
                &snapshot,
                None,
            );
        })
        .await;
    let snapshot = coordinator.get_state();
    let final_transaction = agent_runs.control_plane_transaction().await;
    let (previous_snapshot, persist_request) = {
        let mut guard = state.write().await;
        let previous_snapshot = guard.swarm_snapshots.get(swarm_id).cloned();
        guard.store_swarm_snapshot(snapshot.clone());
        (previous_snapshot, guard.control_plane_persist_request())
    };
    if let Err(error) = persist_request.save().await {
        if let Some(previous_snapshot) = previous_snapshot {
            state.write().await.store_swarm_snapshot(previous_snapshot);
        }
        return Err(ApiError::service_unavailable(error.to_string()));
    }
    drop(final_transaction);

    publish_swarm_event(
        &global_event_fanout,
        swarm_event_fanout.as_ref(),
        swarm_id,
        "swarm:completed",
        &snapshot,
        Some(&result),
    );

    Ok(SwarmRunEnvelope {
        swarm: SwarmStateResponse::from(&snapshot),
        result: TaskResultResponse::from(&result),
    })
}

pub(crate) async fn handle_subscribe_swarm_events(
    swarm_id: &str,
    state: &SharedDaemonState,
) -> axum::response::Response {
    let subscriber = {
        let guard = state.read().await;
        guard.subscribe_to_swarm_events(swarm_id)
    };

    let Some(subscriber) = subscriber else {
        return (
            StatusCode::NOT_FOUND,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            super::serialize_json(&super::contracts::ErrorBody {
                error: "not found".to_string(),
            }),
        )
            .into_response();
    };

    subscribe_swarm_events_response(subscriber)
}
