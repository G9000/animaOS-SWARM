use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use anima_core::{Content, TaskResult};
use anima_swarm::SwarmState;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use futures::stream;

use super::contracts::{
    SwarmCreateRequest, SwarmEnvelope, SwarmEventResponse, SwarmRunEnvelope, SwarmStateResponse,
    SwarmsEnvelope, TaskRequest, TaskResultResponse,
};
use super::ApiError;
use crate::state::DaemonState;

pub(crate) async fn handle_create_swarm(
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<SwarmEnvelope, ApiError> {
    let request: SwarmCreateRequest = super::parse_json_body(body)?;
    let config = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let (coordinator, event_stream) = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.build_swarm(config).map_err(ApiError::bad_request)?
    };

    if let Err(message) = coordinator.start().await {
        return Err(ApiError::bad_request(message));
    }

    let snapshot = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.register_swarm(coordinator, event_stream)
    };
    publish_swarm_event(state, &snapshot.id, "swarm:created", &snapshot, None);

    Ok(SwarmEnvelope {
        swarm: SwarmStateResponse::from(&snapshot),
    })
}

pub(crate) fn handle_list_swarms(
    state: &Arc<Mutex<DaemonState>>,
) -> Result<SwarmsEnvelope, ApiError> {
    let snapshots = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.list_swarms()
    };

    Ok(SwarmsEnvelope {
        swarms: snapshots.iter().map(SwarmStateResponse::from).collect(),
    })
}

pub(crate) fn handle_get_swarm(
    swarm_id: &str,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<SwarmEnvelope, ApiError> {
    let snapshot = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
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
    state: &Arc<Mutex<DaemonState>>,
) -> Result<SwarmRunEnvelope, ApiError> {
    let request: TaskRequest = super::parse_json_body(body)?;
    let content = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let coordinator = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_swarm_coordinator(swarm_id)
    };

    let Some(coordinator) = coordinator else {
        return Err(ApiError::not_found());
    };

    let daemon_state = Arc::clone(state);
    let running_swarm_id = swarm_id.to_string();
    let result = coordinator
        .dispatch_with_running_hook(content.text.clone(), move |snapshot| {
            publish_swarm_event(
                &daemon_state,
                &running_swarm_id,
                "swarm:running",
                &snapshot,
                None,
            );
        })
        .await;
    let snapshot = coordinator.get_state();
    {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.store_swarm_snapshot(snapshot.clone());
    }

    publish_swarm_event(state, swarm_id, "swarm:completed", &snapshot, Some(&result));

    Ok(SwarmRunEnvelope {
        swarm: SwarmStateResponse::from(&snapshot),
        result: TaskResultResponse::from(&result),
    })
}

pub(crate) fn handle_subscribe_swarm_events(
    swarm_id: &str,
    state: &Arc<Mutex<DaemonState>>,
) -> axum::response::Response {
    let subscriber = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
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

    let stream = stream::unfold(subscriber, |mut subscriber| async move {
        loop {
            match subscriber.recv().await {
                Ok(message) => {
                    let event = Event::default().event(message.event).data(message.data);
                    return Some((Ok::<Event, Infallible>(event), subscriber));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn publish_swarm_event(
    state: &Arc<Mutex<DaemonState>>,
    swarm_id: &str,
    event: &str,
    snapshot: &SwarmState,
    result: Option<&TaskResult<Content>>,
) {
    let payload = super::serialize_json(&SwarmEventResponse {
        swarm_id: swarm_id.to_string(),
        state: SwarmStateResponse::from(snapshot),
        result: result.map(TaskResultResponse::from),
    });

    state
        .lock()
        .expect("daemon state mutex should not be poisoned")
        .publish_swarm_event(swarm_id, event, payload);
}
