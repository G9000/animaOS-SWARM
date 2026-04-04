use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use anima_core::{Content, TaskResult};
use anima_swarm::{SwarmConfig, SwarmState, SwarmStatus, SwarmStrategy};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use futures::stream;

use super::api::{
    optional_u64, optional_usize, parse_agent_config, required_string, string_array_json,
    task_result_json, token_usage_json,
};
use super::Response;
use crate::json::{escape_json, JsonParser, JsonValue};
use crate::state::DaemonState;

pub(crate) async fn handle_create_swarm(
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let body = match std::str::from_utf8(&body) {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid UTF-8",
            )
        }
    };

    let object = match JsonParser::new(body).parse_object() {
        Ok(object) => object,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid JSON",
            )
        }
    };

    let config = match parse_swarm_config(&object) {
        Ok(config) => config,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let (coordinator, event_stream) = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        match guard.build_swarm(config) {
            Ok(built) => built,
            Err(message) => {
                return Response::json(
                    "HTTP/1.1 400 Bad Request",
                    format!("{{\"error\":\"{}\"}}", escape_json(&message)),
                )
            }
        }
    };

    if let Err(message) = coordinator.start().await {
        return Response::json(
            "HTTP/1.1 400 Bad Request",
            format!("{{\"error\":\"{}\"}}", escape_json(&message)),
        );
    }

    let snapshot = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.register_swarm(coordinator, event_stream)
    };
    publish_swarm_event(state, &snapshot.id, "swarm:created", &snapshot, None);

    Response::json(
        "HTTP/1.1 201 Created",
        format!("{{\"swarm\":{}}}", swarm_state_json(&snapshot)),
    )
}

pub(crate) fn handle_get_swarm(swarm_id: &str, state: &Arc<Mutex<DaemonState>>) -> Response {
    let snapshot = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_swarm(swarm_id)
    };

    match snapshot {
        Some(snapshot) => Response::json(
            "HTTP/1.1 200 OK",
            format!("{{\"swarm\":{}}}", swarm_state_json(&snapshot)),
        ),
        None => Response::error("HTTP/1.1 404 Not Found", "not found"),
    }
}

pub(crate) async fn handle_run_swarm(
    swarm_id: &str,
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let body = match std::str::from_utf8(&body) {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid UTF-8",
            )
        }
    };

    let object = match JsonParser::new(body).parse_object() {
        Ok(object) => object,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid JSON",
            )
        }
    };

    let task = match required_string(&object, "text") {
        Ok(task) => task,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let coordinator = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_swarm_coordinator(swarm_id)
    };

    let Some(coordinator) = coordinator else {
        return Response::error("HTTP/1.1 404 Not Found", "not found");
    };

    let daemon_state = Arc::clone(state);
    let running_swarm_id = swarm_id.to_string();
    let result = coordinator
        .dispatch_with_running_hook(task, move |snapshot| {
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

    Response::json(
        "HTTP/1.1 200 OK",
        format!(
            "{{\"swarm\":{},\"result\":{}}}",
            swarm_state_json(&snapshot),
            task_result_json(Some(&result))
        ),
    )
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
            "{\"error\":\"not found\"}",
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
    let payload = format!(
        "{{\"swarmId\":\"{}\",\"state\":{},\"result\":{}}}",
        escape_json(swarm_id),
        swarm_state_json(snapshot),
        task_result_json(result)
    );

    state
        .lock()
        .expect("daemon state mutex should not be poisoned")
        .publish_swarm_event(swarm_id, event, payload);
}

fn parse_swarm_config(object: &BTreeMap<String, JsonValue>) -> Result<SwarmConfig, &'static str> {
    let strategy = match object.get("strategy") {
        Some(JsonValue::String(value)) => parse_strategy(value)?,
        _ => return Err("strategy is required"),
    };

    let manager = match object.get("manager") {
        Some(JsonValue::Object(value)) => parse_agent_config(value)?,
        _ => return Err("manager is required"),
    };

    let workers = match object.get("workers") {
        Some(JsonValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                JsonValue::Object(object) => parse_agent_config(object),
                _ => Err("workers must contain objects"),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("workers are required"),
    };

    Ok(SwarmConfig {
        strategy,
        manager,
        workers,
        max_concurrent_agents: optional_usize(object.get("maxConcurrentAgents"))?,
        max_parallel_delegations: optional_usize(object.get("maxParallelDelegations"))?,
        max_turns: optional_usize(object.get("maxTurns"))?,
        token_budget: optional_u64(object.get("tokenBudget"))?,
    })
}

fn parse_strategy(value: &str) -> Result<SwarmStrategy, &'static str> {
    match value {
        "supervisor" => Ok(SwarmStrategy::Supervisor),
        "dynamic" => Ok(SwarmStrategy::Dynamic),
        "round-robin" => Ok(SwarmStrategy::RoundRobin),
        _ => Err("strategy must be supervisor, dynamic, or round-robin"),
    }
}

fn swarm_state_json(state: &SwarmState) -> String {
    format!(
        "{{\"id\":\"{}\",\"status\":\"{}\",\"agentIds\":{},\"results\":{},\"tokenUsage\":{},\"startedAt\":{},\"completedAt\":{}}}",
        escape_json(&state.id),
        swarm_status_str(state.status),
        string_array_json(&state.agent_ids),
        task_results_json(&state.results),
        token_usage_json(&state.token_usage),
        optional_u128_json(state.started_at),
        optional_u128_json(state.completed_at)
    )
}

fn swarm_status_str(status: SwarmStatus) -> &'static str {
    status.as_str()
}

fn task_results_json(results: &[TaskResult<Content>]) -> String {
    format!(
        "[{}]",
        results
            .iter()
            .map(|result| task_result_json(Some(result)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn optional_u128_json(value: Option<u128>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}
