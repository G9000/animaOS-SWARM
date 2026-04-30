mod agents;
mod contracts;
mod health;
mod http;
mod memories;
mod swarms;

use axum::extract::{Path, Request as AxumRequest, State};
use axum::http::{StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response as AxumResponse};
use axum::routing::get;
use axum::Router;
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tower_http::LatencyUnit;
use tracing::Level;
use utoipa::OpenApi;
use utoipa_scalar::Scalar;

use crate::app::{DaemonConfig, SharedDaemonState};

use self::contracts::{
    AgentConfigRequest, AgentEnvelope, AgentRecentMemoriesQuery, AgentRunEnvelope, AgentsEnvelope,
    DeleteResponse, ErrorBody, HealthResponse, MemoriesEnvelope, MemoryCreateRequest,
    MemoryResponse, MemorySearchEnvelope, MemorySearchQuery, RecentMemoriesQuery,
    SwarmCreateRequest, SwarmEnvelope, SwarmRunEnvelope, SwarmsEnvelope, TaskRequest,
};
use self::http::{json_response, make_http_span, read_limited_body, request_query};
pub(super) use self::http::{parse_json_body, serialize_json};

#[derive(OpenApi)]
#[openapi(
    paths(
        api_health_entry,
        create_memory_entry,
        memories_search_entry,
        search_alias_entry,
        memories_recent_entry,
        list_agents_entry,
        create_agent_entry,
        get_agent_entry,
        delete_agent_entry,
        run_agent_entry,
        agent_recent_memories_entry,
        list_swarms_entry,
        create_swarm_entry,
        get_swarm_entry,
        run_swarm_entry,
        swarm_events_entry
    ),
    tags(
        (name = "health", description = "Daemon health endpoints"),
        (name = "agents", description = "Agent management and execution"),
        (name = "memories", description = "Memory storage and search"),
        (name = "swarms", description = "Swarm creation, execution, and streaming")
    )
)]
struct ApiDoc;

#[derive(Clone)]
struct AppState {
    daemon: SharedDaemonState,
    config: DaemonConfig,
}

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub(crate) fn bad_request_static(message: &'static str) -> Self {
        Self::bad_request(message)
    }

    pub(crate) fn malformed_request() -> Self {
        Self::bad_request_static("malformed request")
    }

    pub(crate) fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: "not found".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> AxumResponse {
        json_response(
            self.status,
            &ErrorBody {
                error: self.message,
            },
        )
    }
}

pub(crate) fn router(state: SharedDaemonState, config: DaemonConfig) -> Router {
    let app_state = AppState {
        daemon: state,
        config,
    };
    let request_middleware = ServiceBuilder::new()
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_http_span)
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .latency_unit(LatencyUnit::Millis),
                ),
        );
    let timed_routes = Router::new()
        .route("/openapi.json", get(openapi_entry))
        .route("/docs", get(docs_entry))
        .route("/docs/", get(docs_entry))
        .route("/health", get(health_entry))
        .route("/api/health", get(api_health_entry))
        .route("/api/memories", axum::routing::post(create_memory_entry))
        .route("/api/memories/search", get(memories_search_entry))
        .route("/api/search", get(search_alias_entry))
        .route("/api/memories/recent", get(memories_recent_entry))
        .route(
            "/api/agents",
            get(list_agents_entry).post(create_agent_entry),
        )
        .route(
            "/api/agents/{agent_id}",
            get(get_agent_entry).delete(delete_agent_entry),
        )
        .route(
            "/api/agents/{agent_id}/memories/recent",
            get(agent_recent_memories_entry),
        )
        .route(
            "/api/swarms",
            get(list_swarms_entry).post(create_swarm_entry),
        )
        .route("/api/swarms/{swarm_id}", get(get_swarm_entry))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            config.request_timeout,
        ));
    let run_routes = Router::new()
        .route(
            "/api/agents/{agent_id}/run",
            axum::routing::post(run_agent_entry),
        )
        .route("/api/swarms/{swarm_id}/run", axum::routing::post(run_swarm_entry));

    Router::new()
        .merge(timed_routes)
        .merge(run_routes)
        .route("/api/swarms/{swarm_id}/events", get(swarm_events_entry))
        .fallback(not_found_entry)
        .layer(request_middleware)
        .with_state(app_state)
}

async fn health_entry() -> AxumResponse {
    json_response(StatusCode::OK, &health::handle_health())
}

async fn openapi_entry() -> AxumResponse {
    json_response(StatusCode::OK, &ApiDoc::openapi())
}

async fn docs_entry() -> Html<String> {
    Html(Scalar::new(ApiDoc::openapi()).to_html())
}

#[utoipa::path(
    get,
    path = "/api/health",
    tag = "health",
    responses((status = 200, description = "Daemon is alive", body = HealthResponse))
)]
async fn api_health_entry() -> AxumResponse {
    json_response(StatusCode::OK, &health::handle_health())
}

#[utoipa::path(
    post,
    path = "/api/memories",
    tag = "memories",
    request_body = MemoryCreateRequest,
    responses(
        (status = 201, description = "Memory created", body = MemoryResponse),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn create_memory_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match memories::handle_create_memory(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::CREATED, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/memories/search",
    tag = "memories",
    params(MemorySearchQuery),
    responses(
        (status = 200, description = "Matching memories", body = MemorySearchEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn memories_search_entry(State(state): State<AppState>, uri: Uri) -> AxumResponse {
    handle_memory_search(uri, &state.daemon).await
}

#[utoipa::path(
    get,
    path = "/api/search",
    tag = "memories",
    params(MemorySearchQuery),
    responses(
        (status = 200, description = "Matching memories", body = MemorySearchEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn search_alias_entry(State(state): State<AppState>, uri: Uri) -> AxumResponse {
    handle_memory_search(uri, &state.daemon).await
}

#[utoipa::path(
    get,
    path = "/api/memories/recent",
    tag = "memories",
    params(RecentMemoriesQuery),
    responses(
        (status = 200, description = "Recent memories", body = MemoriesEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn memories_recent_entry(State(state): State<AppState>, uri: Uri) -> AxumResponse {
    let query = match request_query(&uri) {
        Ok(query) => match RecentMemoriesQuery::from_query_map(&query) {
            Ok(query) => query,
            Err(message) => return ApiError::bad_request_static(message).into_response(),
        },
        Err(()) => return ApiError::malformed_request().into_response(),
    };

    match memories::handle_recent_memories(query, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/agents",
    tag = "agents",
    responses((status = 200, description = "List agents", body = AgentsEnvelope))
)]
async fn list_agents_entry(State(state): State<AppState>) -> AxumResponse {
    match agents::handle_list_agents(&state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/agents",
    tag = "agents",
    request_body = AgentConfigRequest,
    responses(
        (status = 201, description = "Agent created", body = AgentEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn create_agent_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match agents::handle_create_agent(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::CREATED, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/agents/{agent_id}",
    tag = "agents",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    responses(
        (status = 200, description = "Agent snapshot", body = AgentEnvelope),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn get_agent_entry(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> AxumResponse {
    match agents::handle_get_agent(&agent_id, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    delete,
    path = "/api/agents/{agent_id}",
    tag = "agents",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    responses((status = 200, description = "Agent deleted", body = DeleteResponse))
)]
async fn delete_agent_entry(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> AxumResponse {
    match agents::handle_delete_agent(&agent_id, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/run",
    tag = "agents",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    request_body = TaskRequest,
    responses(
        (status = 200, description = "Task result", body = AgentRunEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn run_agent_entry(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match agents::handle_run_agent(&agent_id, body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/agents/{agent_id}/memories/recent",
    tag = "agents",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        AgentRecentMemoriesQuery
    ),
    responses(
        (status = 200, description = "Recent agent memories", body = MemoriesEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn agent_recent_memories_entry(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    uri: Uri,
) -> AxumResponse {
    let query = match request_query(&uri) {
        Ok(query) => match AgentRecentMemoriesQuery::from_query_map(&query) {
            Ok(query) => query,
            Err(message) => return ApiError::bad_request_static(message).into_response(),
        },
        Err(()) => return ApiError::malformed_request().into_response(),
    };

    match agents::handle_recent_agent_memories(&agent_id, query, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/swarms",
    tag = "swarms",
    responses((status = 200, description = "List swarms", body = SwarmsEnvelope))
)]
async fn list_swarms_entry(State(state): State<AppState>) -> AxumResponse {
    match swarms::handle_list_swarms(&state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/swarms",
    tag = "swarms",
    request_body = SwarmCreateRequest,
    responses(
        (status = 201, description = "Swarm created", body = SwarmEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn create_swarm_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match swarms::handle_create_swarm(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::CREATED, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/swarms/{swarm_id}",
    tag = "swarms",
    params(("swarm_id" = String, Path, description = "Swarm identifier")),
    responses(
        (status = 200, description = "Swarm snapshot", body = SwarmEnvelope),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn get_swarm_entry(
    State(state): State<AppState>,
    Path(swarm_id): Path<String>,
) -> AxumResponse {
    match swarms::handle_get_swarm(&swarm_id, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/swarms/{swarm_id}/run",
    tag = "swarms",
    params(("swarm_id" = String, Path, description = "Swarm identifier")),
    request_body = TaskRequest,
    responses(
        (status = 200, description = "Swarm task result", body = SwarmRunEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn run_swarm_entry(
    State(state): State<AppState>,
    Path(swarm_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match swarms::handle_run_swarm(&swarm_id, body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/swarms/{swarm_id}/events",
    tag = "swarms",
    params(("swarm_id" = String, Path, description = "Swarm identifier")),
    responses(
        (status = 200, description = "Server-sent events stream", content_type = "text/event-stream"),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn swarm_events_entry(
    State(state): State<AppState>,
    Path(swarm_id): Path<String>,
) -> AxumResponse {
    swarms::handle_subscribe_swarm_events(&swarm_id, &state.daemon).await
}

async fn not_found_entry() -> AxumResponse {
    ApiError::not_found().into_response()
}

async fn handle_memory_search(uri: Uri, state: &SharedDaemonState) -> AxumResponse {
    let query = match request_query(&uri) {
        Ok(query) => match MemorySearchQuery::from_query_map(&query) {
            Ok(query) => query,
            Err(message) => return ApiError::bad_request_static(message).into_response(),
        },
        Err(()) => return ApiError::malformed_request().into_response(),
    };

    match memories::handle_search_memories(query, state).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}
