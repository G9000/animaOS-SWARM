mod agencies;
mod agents;
mod contracts;
mod health;
mod http;
mod memories;
mod swarms;

use std::sync::Arc;

use axum::extract::{Path, Request as AxumRequest, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response as AxumResponse};
use axum::routing::get;
use axum::Router;
use tokio::sync::Semaphore;
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
    AgencyCreateRequest, AgencyCreateResponse, AgencyGenerateRequest, AgencyGenerateResponse,
    AgentConfigRequest, AgentEnvelope, AgentRecentMemoriesQuery, AgentRelationshipCreateRequest,
    AgentRelationshipQuery, AgentRelationshipResponse, AgentRelationshipsEnvelope,
    AgentRunEnvelope, AgentsEnvelope, DeleteResponse, ErrorBody, HealthResponse, MemoriesEnvelope,
    MemoryCreateRequest, MemoryEntitiesEnvelope, MemoryEntityCreateRequest, MemoryEntityQuery,
    MemoryEntityResponse, MemoryEvaluationOutcomeResponse, MemoryEvaluationRequest,
    MemoryEvaluationResponse, MemoryEvidenceTraceResponse, MemoryReadinessResponse,
    MemoryRecallEnvelope, MemoryRecallQuery, MemoryResponse, MemoryRetentionReportResponse,
    MemoryRetentionRequest, MemorySearchEnvelope, MemorySearchQuery, ProviderResponse,
    ProvidersEnvelope, ReadinessResponse, RecentMemoriesQuery, SwarmCreateRequest, SwarmEnvelope,
    SwarmRunEnvelope, SwarmsEnvelope, TaskRequest,
};
use self::http::{json_response, make_http_span, read_limited_body, request_query};
pub(super) use self::http::{parse_json_body, serialize_json};
use crate::runtime_model::provider_summaries;

#[derive(OpenApi)]
#[openapi(
    paths(
        api_health_entry,
        ready_entry,
        create_agency_entry,
        generate_agency_entry,
        create_memory_entry,
        memories_search_entry,
        search_alias_entry,
        memories_recent_entry,
        create_memory_entity_entry,
        list_memory_entities_entry,
        evaluate_memory_entry,
        add_evaluated_memory_entry,
        recall_memories_entry,
        memory_trace_entry,
        memory_readiness_entry,
        apply_memory_retention_entry,
        create_agent_relationship_entry,
        list_agent_relationships_entry,
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
        swarm_events_entry,
        list_providers_entry
    ),
    tags(
        (name = "health", description = "Daemon health endpoints"),
        (name = "agencies", description = "Agency generation and team drafting"),
        (name = "agents", description = "Agent management and execution"),
        (name = "memories", description = "Memory storage and search"),
        (name = "swarms", description = "Swarm creation, execution, and streaming"),
        (name = "providers", description = "Model provider catalog")
    )
)]
struct ApiDoc;

#[derive(Clone)]
struct AppState {
    daemon: SharedDaemonState,
    config: DaemonConfig,
    run_limiter: Arc<Semaphore>,
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

    pub(crate) fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
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
        run_limiter: Arc::new(Semaphore::new(config.max_concurrent_runs)),
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
        .route("/ready", get(ready_entry))
        .route("/metrics", get(metrics_entry))
        .route("/api/health", get(api_health_entry))
        .route("/api/ready", get(ready_entry))
        .route(
            "/api/agencies/create",
            axum::routing::post(create_agency_entry),
        )
        .route(
            "/api/agencies/generate",
            axum::routing::post(generate_agency_entry),
        )
        .route("/api/memories", axum::routing::post(create_memory_entry))
        .route("/api/memories/search", get(memories_search_entry))
        .route("/api/search", get(search_alias_entry))
        .route("/api/memories/recent", get(memories_recent_entry))
        .route(
            "/api/memories/entities",
            get(list_memory_entities_entry).post(create_memory_entity_entry),
        )
        .route(
            "/api/memories/evaluations",
            axum::routing::post(evaluate_memory_entry),
        )
        .route(
            "/api/memories/evaluated",
            axum::routing::post(add_evaluated_memory_entry),
        )
        .route("/api/memories/recall", get(recall_memories_entry))
        .route("/api/memories/readiness", get(memory_readiness_entry))
        .route(
            "/api/memories/retention",
            axum::routing::post(apply_memory_retention_entry),
        )
        .route("/api/memories/{memory_id}/trace", get(memory_trace_entry))
        .route(
            "/api/memories/relationships",
            get(list_agent_relationships_entry).post(create_agent_relationship_entry),
        )
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
        .route("/api/providers", get(list_providers_entry))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            config.request_timeout,
        ));
    let run_routes = Router::new()
        .route(
            "/api/agents/{agent_id}/run",
            axum::routing::post(run_agent_entry),
        )
        .route(
            "/api/swarms/{swarm_id}/run",
            axum::routing::post(run_swarm_entry),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            config.request_timeout,
        ));

    Router::new()
        .merge(timed_routes)
        .merge(run_routes)
        .route("/api/swarms/{swarm_id}/events", get(swarm_events_entry))
        // Auth gates everything mounted above this line; the middleware
        // exempts health/readiness/metrics/docs by path. When
        // ANIMAOS_RS_API_KEY is unset the daemon runs in trust-the-network
        // mode (fine for 127.0.0.1 dev only).
        .layer(axum::middleware::from_fn(self::http::enforce_api_key))
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
    get,
    path = "/api/ready",
    tag = "health",
    responses(
        (status = 200, description = "Daemon is ready", body = ReadinessResponse),
        (status = 503, description = "Daemon is not ready", body = ReadinessResponse)
    )
)]
async fn ready_entry(State(state): State<AppState>) -> AxumResponse {
    let response = health::handle_readiness(&state.daemon, &state.config).await;
    let status = if response.status == "ready" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    json_response(status, &response)
}

async fn metrics_entry(State(state): State<AppState>) -> AxumResponse {
    let body = health::handle_metrics(&state.daemon, &state.config).await;
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        body,
    )
        .into_response()
}

#[utoipa::path(
    post,
    path = "/api/agencies/create",
    tag = "agencies",
    request_body = AgencyCreateRequest,
    responses(
        (status = 201, description = "Agency workspace created", body = AgencyCreateResponse),
        (status = 400, description = "Invalid request, invalid model output, or invalid workspace path", body = ErrorBody)
    )
)]
async fn create_agency_entry(State(state): State<AppState>, request: AxumRequest) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match agencies::handle_create_agency(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::CREATED, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    post,
    path = "/api/agencies/generate",
    tag = "agencies",
    request_body = AgencyGenerateRequest,
    responses(
        (status = 200, description = "Generated agency draft", body = AgencyGenerateResponse),
        (status = 400, description = "Invalid request or model output", body = ErrorBody)
    )
)]
async fn generate_agency_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match agencies::handle_generate_agency(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
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
async fn create_memory_entry(State(state): State<AppState>, request: AxumRequest) -> AxumResponse {
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
    post,
    path = "/api/memories/entities",
    tag = "memories",
    request_body = MemoryEntityCreateRequest,
    responses(
        (status = 201, description = "Memory entity created or updated", body = MemoryEntityResponse),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn create_memory_entity_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match memories::handle_create_memory_entity(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::CREATED, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/memories/entities",
    tag = "memories",
    params(MemoryEntityQuery),
    responses(
        (status = 200, description = "Memory entities", body = MemoryEntitiesEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn list_memory_entities_entry(State(state): State<AppState>, uri: Uri) -> AxumResponse {
    let query = match request_query(&uri) {
        Ok(query) => match MemoryEntityQuery::from_query_map(&query) {
            Ok(query) => query,
            Err(message) => return ApiError::bad_request_static(message).into_response(),
        },
        Err(()) => return ApiError::malformed_request().into_response(),
    };

    match memories::handle_list_memory_entities(query, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/memories/evaluations",
    tag = "memories",
    request_body = MemoryEvaluationRequest,
    responses(
        (status = 200, description = "Memory evaluation", body = MemoryEvaluationResponse),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn evaluate_memory_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match memories::handle_evaluate_memory(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    post,
    path = "/api/memories/evaluated",
    tag = "memories",
    request_body = MemoryEvaluationRequest,
    responses(
        (status = 200, description = "Evaluated memory write outcome", body = MemoryEvaluationOutcomeResponse),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn add_evaluated_memory_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match memories::handle_add_evaluated_memory(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/memories/recall",
    tag = "memories",
    params(MemoryRecallQuery),
    responses(
        (status = 200, description = "Hybrid memory recall results", body = MemoryRecallEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn recall_memories_entry(State(state): State<AppState>, uri: Uri) -> AxumResponse {
    let query = match request_query(&uri) {
        Ok(query) => match MemoryRecallQuery::from_query_map(&query) {
            Ok(query) => query,
            Err(message) => return ApiError::bad_request_static(message).into_response(),
        },
        Err(()) => return ApiError::malformed_request().into_response(),
    };

    match memories::handle_recall_memories(query, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/memories/{memory_id}/trace",
    tag = "memories",
    params(("memory_id" = String, Path, description = "Memory ID to trace")),
    responses(
        (status = 200, description = "Memory evidence trace", body = MemoryEvidenceTraceResponse),
        (status = 404, description = "Memory not found", body = ErrorBody)
    )
)]
async fn memory_trace_entry(
    State(state): State<AppState>,
    Path(memory_id): Path<String>,
) -> AxumResponse {
    match memories::handle_memory_trace(memory_id, &state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    get,
    path = "/api/memories/readiness",
    tag = "memories",
    responses((status = 200, description = "Memory quality and embedding readiness", body = MemoryReadinessResponse))
)]
async fn memory_readiness_entry(State(state): State<AppState>) -> AxumResponse {
    match memories::handle_memory_readiness(&state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}

#[utoipa::path(
    post,
    path = "/api/memories/retention",
    tag = "memories",
    request_body = MemoryRetentionRequest,
    responses(
        (status = 200, description = "Memory retention report", body = MemoryRetentionReportResponse),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn apply_memory_retention_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match memories::handle_apply_memory_retention(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    post,
    path = "/api/memories/relationships",
    tag = "memories",
    request_body = AgentRelationshipCreateRequest,
    responses(
        (status = 201, description = "Agent relationship created or updated", body = AgentRelationshipResponse),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn create_agent_relationship_entry(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match memories::handle_create_agent_relationship(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::CREATED, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}

#[utoipa::path(
    get,
    path = "/api/memories/relationships",
    tag = "memories",
    params(AgentRelationshipQuery),
    responses(
        (status = 200, description = "Agent relationships", body = AgentRelationshipsEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody)
    )
)]
async fn list_agent_relationships_entry(State(state): State<AppState>, uri: Uri) -> AxumResponse {
    let query = match request_query(&uri) {
        Ok(query) => match AgentRelationshipQuery::from_query_map(&query) {
            Ok(query) => query,
            Err(message) => return ApiError::bad_request_static(message).into_response(),
        },
        Err(()) => return ApiError::malformed_request().into_response(),
    };

    match memories::handle_list_agent_relationships(query, &state.daemon).await {
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
async fn create_agent_entry(State(state): State<AppState>, request: AxumRequest) -> AxumResponse {
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
    let _permit = match state.run_limiter.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return ApiError::service_unavailable("too many concurrent run requests")
                .into_response();
        }
    };

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
async fn create_swarm_entry(State(state): State<AppState>, request: AxumRequest) -> AxumResponse {
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
    let _permit = match state.run_limiter.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return ApiError::service_unavailable("too many concurrent run requests")
                .into_response();
        }
    };

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

#[utoipa::path(
    get,
    path = "/api/providers",
    tag = "providers",
    responses((status = 200, description = "Supported model providers", body = ProvidersEnvelope))
)]
async fn list_providers_entry() -> AxumResponse {
    let providers = provider_summaries()
        .into_iter()
        .map(|summary| ProviderResponse {
            id: summary.id.to_string(),
            label: summary.label.to_string(),
            requires_key: summary.requires_key,
            configured: summary.configured,
            api_key_envs: summary.api_key_envs.iter().map(|s| s.to_string()).collect(),
        })
        .collect::<Vec<_>>();
    json_response(StatusCode::OK, &ProvidersEnvelope { providers })
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

#[cfg(test)]
mod tests {
    use super::router;
    use crate::app::DaemonConfig;
    use crate::state::DaemonState;
    use anima_core::{
        AgentConfig, AgentSettings, Content, ModelAdapter, ModelGenerateRequest,
        ModelGenerateResponse, ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;
    use tower::util::ServiceExt;

    struct SlowModelAdapter {
        delay: Duration,
    }

    #[async_trait]
    impl ModelAdapter for SlowModelAdapter {
        fn provider(&self) -> &str {
            "slow"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            tokio::time::sleep(self.delay).await;

            Ok(ModelGenerateResponse {
                content: Content {
                    text: format!("{} completed", config.name),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                stop_reason: ModelStopReason::End,
            })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_routes_respect_request_timeout() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            SlowModelAdapter {
                delay: Duration::from_millis(50),
            },
        ))));
        let agent_id = {
            let mut guard = state.write().await;
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        };
        let app = router(
            state,
            DaemonConfig {
                request_timeout: Duration::from_millis(10),
                ..DaemonConfig::default()
            },
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/agents/{agent_id}/run"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"run pending task"}"#))
                    .expect("request builds"),
            )
            .await
            .expect("app responds");

        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_routes_reject_when_concurrency_limit_is_exhausted() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            SlowModelAdapter {
                delay: Duration::from_millis(75),
            },
        ))));
        let (first_agent_id, second_agent_id) = {
            let mut guard = state.write().await;
            let first = guard
                .create_agent(test_config("operator-one"))
                .expect("first agent should be created")
                .state
                .id;
            let second = guard
                .create_agent(test_config("operator-two"))
                .expect("second agent should be created")
                .state
                .id;
            (first, second)
        };
        let app = router(
            state,
            DaemonConfig {
                max_concurrent_runs: 1,
                request_timeout: Duration::from_secs(1),
                ..DaemonConfig::default()
            },
        );

        let first_request = Request::builder()
            .method("POST")
            .uri(format!("/api/agents/{first_agent_id}/run"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"text":"first task"}"#))
            .expect("first request builds");
        let second_request = Request::builder()
            .method("POST")
            .uri(format!("/api/agents/{second_agent_id}/run"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"text":"second task"}"#))
            .expect("second request builds");

        let first_app = app.clone();
        let first = tokio::spawn(async move {
            first_app
                .oneshot(first_request)
                .await
                .expect("first response should be returned")
        });

        tokio::time::sleep(Duration::from_millis(10)).await;

        let second = app
            .oneshot(second_request)
            .await
            .expect("second response should be returned");

        assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(second.into_body(), usize::MAX)
            .await
            .expect("body reads");
        assert!(std::str::from_utf8(&body)
            .expect("body is utf-8")
            .contains("too many concurrent run requests"));

        let first = first.await.expect("first join succeeds");
        assert_eq!(first.status(), StatusCode::OK);
    }

    fn test_config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "gpt-5.4".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: Some("openai".into()),
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }
}
