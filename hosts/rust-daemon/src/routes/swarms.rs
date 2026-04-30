pub(super) mod events;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;

use super::contracts::{
    SwarmCreateRequest, SwarmEnvelope, SwarmRunEnvelope, SwarmStateResponse,
    SwarmsEnvelope, TaskRequest, TaskResultResponse,
};
use super::ApiError;
use crate::app::SharedDaemonState;
use self::events::{publish_swarm_event, subscribe_swarm_events_response};

pub(crate) async fn handle_create_swarm(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<SwarmEnvelope, ApiError> {
    let request: SwarmCreateRequest = super::parse_json_body(body)?;
    let config = request.into_domain().map_err(ApiError::bad_request_static)?;

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

    let snapshot = {
        let mut guard = state.write().await;
        guard.register_swarm(coordinator, event_stream)
    };
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
) -> Result<SwarmRunEnvelope, ApiError> {
    let request: TaskRequest = super::parse_json_body(body)?;
    let content = request.into_domain().map_err(ApiError::bad_request_static)?;

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

    let running_swarm_id = swarm_id.to_string();
    let running_global_event_fanout = global_event_fanout.clone();
    let running_swarm_event_fanout = swarm_event_fanout.clone();
    let result = coordinator
        .dispatch_with_running_hook(content.text.clone(), move |snapshot| {
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
    {
        let mut guard = state.write().await;
        guard.store_swarm_snapshot(snapshot.clone());
    }

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
