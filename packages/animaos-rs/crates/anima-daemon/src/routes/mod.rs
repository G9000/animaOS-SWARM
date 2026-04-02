mod agents;
mod health;
mod memories;

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::routing::{get, post};
use axum::Router;

use crate::app::{DaemonConfig, SharedDaemonState};
use crate::http::Response;

const NOT_FOUND_JSON: &str = "{\"error\":\"not found\"}";

pub(crate) fn router(state: SharedDaemonState, config: DaemonConfig) -> Router {
    Router::new()
        .route("/health", get(handle_health))
        .route("/api/health", get(handle_health))
        .route("/api/memories", post(handle_create_memory))
        .route("/api/memories/search", get(handle_search_memories))
        .route("/api/memories/recent", get(handle_recent_memories))
        .route(
            "/api/agents",
            post(handle_create_agent).get(handle_list_agents),
        )
        .route("/api/agents/:agent_id", get(handle_get_agent))
        .route("/api/agents/:agent_id/run", post(handle_run_agent))
        .route(
            "/api/agents/:agent_id/memories/recent",
            get(handle_recent_agent_memories),
        )
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(config.max_request_bytes))
        .with_state(state)
}

async fn handle_health() -> AxumResponse {
    json_response(health::handle_health())
}

async fn handle_create_memory(State(state): State<SharedDaemonState>, body: Bytes) -> AxumResponse {
    json_response(memories::handle_create_memory(body.to_vec(), &state))
}

async fn handle_search_memories(State(state): State<SharedDaemonState>, uri: Uri) -> AxumResponse {
    match request_query(&uri) {
        Ok(query) => json_response(memories::handle_search_memories(query, &state)),
        Err(()) => bad_request(),
    }
}

async fn handle_recent_memories(State(state): State<SharedDaemonState>, uri: Uri) -> AxumResponse {
    match request_query(&uri) {
        Ok(query) => json_response(memories::handle_recent_memories(query, &state)),
        Err(()) => bad_request(),
    }
}

async fn handle_create_agent(State(state): State<SharedDaemonState>, body: Bytes) -> AxumResponse {
    json_response(agents::handle_create_agent(body.to_vec(), &state))
}

async fn handle_list_agents(State(state): State<SharedDaemonState>) -> AxumResponse {
    json_response(agents::handle_list_agents(&state))
}

async fn handle_get_agent(
    State(state): State<SharedDaemonState>,
    Path(agent_id): Path<String>,
) -> AxumResponse {
    json_response(agents::handle_get_agent(&agent_id, &state))
}

async fn handle_run_agent(
    State(state): State<SharedDaemonState>,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> AxumResponse {
    json_response(agents::handle_run_agent(&agent_id, body.to_vec(), &state).await)
}

async fn handle_recent_agent_memories(
    State(state): State<SharedDaemonState>,
    Path(agent_id): Path<String>,
    uri: Uri,
) -> AxumResponse {
    match request_query(&uri) {
        Ok(query) => json_response(agents::handle_recent_agent_memories(
            &agent_id, query, &state,
        )),
        Err(()) => bad_request(),
    }
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
