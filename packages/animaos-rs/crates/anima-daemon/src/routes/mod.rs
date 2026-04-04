mod agents;
mod api;
mod health;
mod memories;
mod swarms;

use std::collections::HashMap;

use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::routing::any;
use axum::Router;

use crate::app::{DaemonConfig, SharedDaemonState};
use crate::json::escape_json;

const NOT_FOUND_JSON: &str = "{\"error\":\"not found\"}";

#[derive(Clone)]
struct AppState {
    daemon: SharedDaemonState,
    config: DaemonConfig,
}

pub(crate) struct Response {
    pub(crate) status_line: &'static str,
    pub(crate) body: String,
}

impl Response {
    pub(crate) fn json(status_line: &'static str, body: String) -> Self {
        Self { status_line, body }
    }

    pub(crate) fn error(status_line: &'static str, message: &'static str) -> Self {
        Self::json(
            status_line,
            format!("{{\"error\":\"{}\"}}", escape_json(message)),
        )
    }
}

pub(crate) fn router(state: SharedDaemonState, config: DaemonConfig) -> Router {
    let app_state = AppState {
        daemon: state,
        config,
    };

    Router::new()
        .route("/health", any(health_entry))
        .route("/api/health", any(health_entry))
        .route("/api/memories", any(memories_collection_entry))
        .route("/api/memories/search", any(memories_search_entry))
        .route("/api/memories/recent", any(memories_recent_entry))
        .route("/api/agents", any(agents_collection_entry))
        .route("/api/agents/:agent_id", any(agent_detail_entry))
        .route("/api/agents/:agent_id/run", any(agent_run_entry))
        .route(
            "/api/agents/:agent_id/memories/recent",
            any(agent_recent_memories_entry),
        )
        .route("/api/swarms", any(swarms_collection_entry))
        .route("/api/swarms/:swarm_id", any(swarm_detail_entry))
        .route("/api/swarms/:swarm_id/run", any(swarm_run_entry))
        .route("/api/swarms/:swarm_id/events", any(swarm_events_entry))
        .fallback(not_found)
        .with_state(app_state)
}

async fn health_entry(method: Method) -> AxumResponse {
    match method {
        Method::GET => json_response(health::handle_health()),
        _ => not_found().await,
    }
}

async fn memories_collection_entry(
    method: Method,
    State(state): State<AppState>,
    request: Request,
) -> AxumResponse {
    match method {
        Method::POST => match read_limited_body(request, state.config.max_request_bytes).await {
            Ok(body) => json_response(memories::handle_create_memory(body, &state.daemon)),
            Err(response) => response,
        },
        _ => not_found().await,
    }
}

async fn memories_search_entry(
    method: Method,
    State(state): State<AppState>,
    uri: Uri,
) -> AxumResponse {
    match method {
        Method::GET => match request_query(&uri) {
            Ok(query) => json_response(memories::handle_search_memories(query, &state.daemon)),
            Err(()) => bad_request(),
        },
        _ => not_found().await,
    }
}

async fn memories_recent_entry(
    method: Method,
    State(state): State<AppState>,
    uri: Uri,
) -> AxumResponse {
    match method {
        Method::GET => match request_query(&uri) {
            Ok(query) => json_response(memories::handle_recent_memories(query, &state.daemon)),
            Err(()) => bad_request(),
        },
        _ => not_found().await,
    }
}

async fn agents_collection_entry(
    method: Method,
    State(state): State<AppState>,
    request: Request,
) -> AxumResponse {
    match method {
        Method::GET => json_response(agents::handle_list_agents(&state.daemon)),
        Method::POST => match read_limited_body(request, state.config.max_request_bytes).await {
            Ok(body) => json_response(agents::handle_create_agent(body, &state.daemon)),
            Err(response) => response,
        },
        _ => not_found().await,
    }
}

async fn agent_detail_entry(
    method: Method,
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> AxumResponse {
    match method {
        Method::GET => json_response(agents::handle_get_agent(&agent_id, &state.daemon)),
        _ => not_found().await,
    }
}

async fn agent_run_entry(
    method: Method,
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> AxumResponse {
    match method {
        Method::POST => match read_limited_body(request, state.config.max_request_bytes).await {
            Ok(body) => {
                json_response(agents::handle_run_agent(&agent_id, body, &state.daemon).await)
            }
            Err(response) => response,
        },
        _ => not_found().await,
    }
}

async fn agent_recent_memories_entry(
    method: Method,
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    uri: Uri,
) -> AxumResponse {
    match method {
        Method::GET => match request_query(&uri) {
            Ok(query) => json_response(agents::handle_recent_agent_memories(
                &agent_id,
                query,
                &state.daemon,
            )),
            Err(()) => bad_request(),
        },
        _ => not_found().await,
    }
}

async fn swarms_collection_entry(
    method: Method,
    State(state): State<AppState>,
    request: Request,
) -> AxumResponse {
    match method {
        Method::POST => match read_limited_body(request, state.config.max_request_bytes).await {
            Ok(body) => json_response(swarms::handle_create_swarm(body, &state.daemon).await),
            Err(response) => response,
        },
        _ => not_found().await,
    }
}

async fn swarm_detail_entry(
    method: Method,
    State(state): State<AppState>,
    Path(swarm_id): Path<String>,
) -> AxumResponse {
    match method {
        Method::GET => json_response(swarms::handle_get_swarm(&swarm_id, &state.daemon)),
        _ => not_found().await,
    }
}

async fn swarm_run_entry(
    method: Method,
    State(state): State<AppState>,
    Path(swarm_id): Path<String>,
    request: Request,
) -> AxumResponse {
    match method {
        Method::POST => match read_limited_body(request, state.config.max_request_bytes).await {
            Ok(body) => {
                json_response(swarms::handle_run_swarm(&swarm_id, body, &state.daemon).await)
            }
            Err(response) => response,
        },
        _ => not_found().await,
    }
}

async fn swarm_events_entry(
    method: Method,
    State(state): State<AppState>,
    Path(swarm_id): Path<String>,
) -> AxumResponse {
    match method {
        Method::GET => swarms::handle_subscribe_swarm_events(&swarm_id, &state.daemon),
        _ => not_found().await,
    }
}

async fn read_limited_body(request: Request, limit: usize) -> Result<Vec<u8>, AxumResponse> {
    to_bytes(request.into_body(), limit)
        .await
        .map(|body| body.to_vec())
        .map_err(|_| bad_request())
}

async fn not_found() -> AxumResponse {
    json_response(Response::json(
        "HTTP/1.1 404 Not Found",
        NOT_FOUND_JSON.to_string(),
    ))
}

fn bad_request() -> AxumResponse {
    json_response(Response::error(
        "HTTP/1.1 400 Bad Request",
        "malformed request",
    ))
}

fn json_response(response: Response) -> AxumResponse {
    (
        status_code(response.status_line),
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        response.body,
    )
        .into_response()
}

fn status_code(status_line: &str) -> StatusCode {
    match status_line {
        "HTTP/1.1 200 OK" => StatusCode::OK,
        "HTTP/1.1 201 Created" => StatusCode::CREATED,
        "HTTP/1.1 400 Bad Request" => StatusCode::BAD_REQUEST,
        "HTTP/1.1 404 Not Found" => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn request_query(uri: &Uri) -> Result<HashMap<String, String>, ()> {
    parse_query_string(uri.query().unwrap_or_default())
}

fn parse_query_string(query: &str) -> Result<HashMap<String, String>, ()> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key)?, percent_decode(value)?);
    }
    Ok(params)
}

fn percent_decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(());
                }
                decoded.push((hex_value(bytes[index + 1])? << 4) | hex_value(bytes[index + 2])?);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded).map_err(|_| ())
}

fn hex_value(byte: u8) -> Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(()),
    }
}
