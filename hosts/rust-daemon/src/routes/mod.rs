mod agencies;
mod agents;
mod connectors;
mod contracts;
mod health;
mod http;
mod memories;
mod schedules;
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

use crate::agent_runs::AgentRunCoordinator;
use crate::app::{DaemonConfig, SharedDaemonState};
use crate::connectors::runtime::{ConnectorManager, ConnectorManagerError};
use crate::schedules::SchedulerService;

use self::contracts::{
    AgencyCreateRequest, AgencyCreateResponse, AgencyGenerateRequest, AgencyGenerateResponse,
    AgentConfigRequest, AgentEnvelope, AgentRecentMemoriesQuery, AgentRelationshipCreateRequest,
    AgentRelationshipQuery, AgentRelationshipResponse, AgentRelationshipsEnvelope,
    AgentUpdateRequest, AgentsEnvelope, DeleteResponse, ErrorBody, HealthResponse,
    MemoriesEnvelope, MemoryCreateRequest, MemoryEntitiesEnvelope, MemoryEntityCreateRequest,
    MemoryEntityQuery, MemoryEntityResponse, MemoryEvaluationOutcomeResponse,
    MemoryEvaluationRequest, MemoryEvaluationResponse, MemoryEvidenceTraceResponse,
    MemoryReadinessResponse, MemoryRecallEnvelope, MemoryRecallQuery, MemoryResponse,
    MemoryRetentionReportResponse, MemoryRetentionRequest, MemorySearchEnvelope, MemorySearchQuery,
    ProviderResponse, ProvidersEnvelope, ReadinessResponse, RecentMemoriesQuery,
    SwarmCreateRequest, SwarmEnvelope, SwarmRunEnvelope, SwarmsEnvelope, TaskRequest,
};
pub(crate) use self::contracts::{
    AgentRunEnvelope, AgentRuntimeSnapshotResponse, TaskResultResponse,
};
pub(crate) use self::http::configured_bind_is_loopback;
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
        update_agent_entry,
        delete_agent_entry,
        run_agent_entry,
        agent_recent_memories_entry,
        list_swarms_entry,
        create_swarm_entry,
        get_swarm_entry,
        run_swarm_entry,
        swarm_events_entry,
        list_providers_entry,
        connectors::list_connectors,
        connectors::create_telegram_connector,
        connectors::replace_telegram_credential,
        connectors::approve_telegram_pairing,
        connectors::restart_telegram_connector,
        connectors::delete_telegram_connector,
        connectors::list_connector_messages,
        connectors::send_connector_message,
        schedules::list_schedules,
        schedules::create_schedule,
        schedules::update_schedule,
        schedules::delete_schedule,
        schedules::import_legacy_schedules,
    ),
    tags(
        (name = "health", description = "Daemon health endpoints"),
        (name = "agencies", description = "Agency generation and team drafting"),
        (name = "agents", description = "Agent management and execution"),
        (name = "memories", description = "Memory storage and search"),
        (name = "swarms", description = "Swarm creation, execution, and streaming"),
        (name = "providers", description = "Model provider catalog"),
        (name = "connectors", description = "Agent-scoped connector administration"),
        (name = "connector-thread", description = "Dedicated connector-room messages"),
        (name = "schedules", description = "Daemon-backed scheduled prompts"),
    )
)]
struct ApiDoc;

#[derive(Clone)]
struct AppState {
    daemon: SharedDaemonState,
    config: DaemonConfig,
    run_limiter: Arc<Semaphore>,
    agent_runs: AgentRunCoordinator,
    connector_manager: ConnectorManager,
    scheduler: SchedulerService,
    local_owner: self::http::LocalOwnerPolicy,
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

#[allow(dead_code)] // Compatibility constructor used by focused route tests.
pub(crate) fn router_with_services(
    state: SharedDaemonState,
    config: DaemonConfig,
    run_limiter: Arc<Semaphore>,
    agent_runs: AgentRunCoordinator,
    connector_manager: ConnectorManager,
    bind_is_loopback: bool,
) -> Router {
    let scheduler = SchedulerService::new(
        Arc::clone(&state),
        agent_runs.clone(),
        connector_manager.clone(),
    );
    router_with_all_services(
        state,
        config,
        run_limiter,
        agent_runs,
        connector_manager,
        scheduler,
        bind_is_loopback,
    )
}

pub(crate) fn router_with_all_services(
    state: SharedDaemonState,
    config: DaemonConfig,
    run_limiter: Arc<Semaphore>,
    agent_runs: AgentRunCoordinator,
    connector_manager: ConnectorManager,
    scheduler: SchedulerService,
    bind_is_loopback: bool,
) -> Router {
    router_with_services_with_policies(
        state,
        config,
        run_limiter,
        agent_runs,
        connector_manager,
        scheduler,
        self::http::LocalOwnerPolicy::from_env(bind_is_loopback),
        self::http::ApiKeyPolicy::from_env(),
    )
}

fn router_with_services_with_policies(
    state: SharedDaemonState,
    config: DaemonConfig,
    run_limiter: Arc<Semaphore>,
    agent_runs: AgentRunCoordinator,
    connector_manager: ConnectorManager,
    scheduler: SchedulerService,
    local_owner: self::http::LocalOwnerPolicy,
    api_key: self::http::ApiKeyPolicy,
) -> Router {
    let app_state = AppState {
        daemon: Arc::clone(&state),
        config,
        run_limiter: Arc::clone(&run_limiter),
        agent_runs,
        connector_manager,
        scheduler,
        local_owner,
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
            get(get_agent_entry)
                .patch(update_agent_entry)
                .delete(delete_agent_entry),
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
        .route(
            "/api/agents/{agent_id}/schedules",
            get(schedules::list_schedules).post(schedules::create_schedule),
        )
        .route(
            "/api/agents/{agent_id}/schedules/import",
            axum::routing::post(schedules::import_legacy_schedules),
        )
        .route(
            "/api/agents/{agent_id}/schedules/{schedule_id}",
            axum::routing::patch(schedules::update_schedule).delete(schedules::delete_schedule),
        )
        .route(
            "/api/agents/{agent_id}/connectors",
            get(connectors::list_connectors),
        )
        .route(
            "/api/agents/{agent_id}/connectors/telegram",
            axum::routing::post(connectors::create_telegram_connector),
        )
        .route(
            "/api/agents/{agent_id}/connectors/{connector_id}/credential",
            axum::routing::put(connectors::replace_telegram_credential),
        )
        .route(
            "/api/agents/{agent_id}/connectors/{connector_id}/pairings/{chat_id}/approve",
            axum::routing::post(connectors::approve_telegram_pairing),
        )
        .route(
            "/api/agents/{agent_id}/connectors/{connector_id}/restart",
            axum::routing::post(connectors::restart_telegram_connector),
        )
        .route(
            "/api/agents/{agent_id}/connectors/{connector_id}",
            axum::routing::delete(connectors::delete_telegram_connector),
        )
        .route(
            "/api/agents/{agent_id}/connectors/{connector_id}/messages",
            get(connectors::list_connector_messages),
        )
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
        .route(
            "/api/agents/{agent_id}/connectors/{connector_id}/messages",
            axum::routing::post(connectors::send_connector_message),
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
        .layer(axum::middleware::from_fn_with_state(
            api_key,
            self::http::enforce_api_key,
        ))
        .fallback(not_found_entry)
        .layer(request_middleware)
        .with_state(app_state)
}

#[cfg(test)]
pub(crate) fn router(state: SharedDaemonState, config: DaemonConfig) -> Router {
    use crate::connectors::credentials::InMemoryCredentialStore;
    use crate::connectors::telegram::TelegramClient;

    let run_limiter = Arc::new(Semaphore::new(config.max_concurrent_runs));
    let agent_runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&run_limiter));
    let connector_manager = ConnectorManager::new(
        Arc::clone(&state),
        agent_runs.clone(),
        Arc::new(InMemoryCredentialStore::default()),
        Arc::new(TelegramClient::new().expect("test Telegram client should configure")),
    );
    router_with_services(
        state,
        config,
        run_limiter,
        agent_runs,
        connector_manager,
        configured_bind_is_loopback(),
    )
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
        Ok(body) => {
            let _transaction = state.agent_runs.control_plane_transaction().await;
            match agents::handle_create_agent(body, &state.daemon).await {
                Ok(response) => json_response(StatusCode::CREATED, &response),
                Err(error) => error.into_response(),
            }
        }
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
    responses(
        (status = 200, description = "Agent deleted", body = DeleteResponse),
        (status = 403, description = "Local owner authorization required", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn delete_agent_entry(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        let message = match rejection {
            self::http::LocalOwnerRejection::LocalAdminRequired => {
                "local owner authorization required"
            }
            self::http::LocalOwnerRejection::OriginRejected => "browser origin is not approved",
        };
        return json_response(
            StatusCode::FORBIDDEN,
            &ErrorBody {
                error: message.to_string(),
            },
        );
    }
    match state.connector_manager.delete_agent(agent_id).await {
        Ok(()) => json_response(StatusCode::OK, &DeleteResponse { deleted: true }),
        Err(ConnectorManagerError::AgentNotFound) => ApiError::not_found().into_response(),
        Err(error) => ApiError::service_unavailable(error.to_string()).into_response(),
    }
}

#[utoipa::path(
    patch,
    path = "/api/agents/{agent_id}",
    tag = "agents",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    request_body = AgentUpdateRequest,
    responses(
        (status = 200, description = "Agent updated", body = AgentEnvelope),
        (status = 400, description = "Invalid request", body = ErrorBody),
        (status = 404, description = "Not found", body = ErrorBody)
    )
)]
async fn update_agent_entry(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => {
            let _transaction = state.agent_runs.control_plane_transaction().await;
            match agents::handle_update_agent(&agent_id, body, &state.daemon).await {
                Ok(response) => json_response(StatusCode::OK, &response),
                Err(error) => error.into_response(),
            }
        }
        Err(response) => response,
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
    let permit = match state.agent_runs.try_admit() {
        Ok(permit) => permit,
        Err(error) => return error.into_response(),
    };

    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => {
            match agents::handle_run_agent(&agent_id, body, &state.agent_runs, permit).await {
                Ok(response) => json_response(StatusCode::OK, &response),
                Err(error) => error.into_response(),
            }
        }
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
        Ok(body) => {
            let _transaction = state.agent_runs.control_plane_transaction().await;
            match swarms::handle_create_swarm(body, &state.daemon).await {
                Ok(response) => json_response(StatusCode::CREATED, &response),
                Err(error) => error.into_response(),
            }
        }
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
        Ok(body) => {
            match swarms::handle_run_swarm(&swarm_id, body, &state.daemon, &state.agent_runs).await
            {
                Ok(response) => json_response(StatusCode::OK, &response),
                Err(error) => error.into_response(),
            }
        }
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
    use super::{router, router_with_services, router_with_services_with_policies};
    use crate::agent_runs::AgentRunCoordinator;
    use crate::app::DaemonConfig;
    use crate::connectors::credentials::{
        ConnectorCredentialStore, CredentialStoreError, InMemoryCredentialStore, TelegramBotToken,
    };
    use crate::connectors::runtime::{ConnectorManager, TelegramTransport};
    use crate::connectors::telegram::{
        TelegramClient, TelegramSentMessage, TelegramTransportError, TelegramUpdateBatch,
    };
    use crate::connectors::{
        OutboundDeliveryState, TelegramBotIdentity, TelegramChatKind, TelegramChatMetadata,
        TelegramConnectorRecord, TelegramPendingPairing,
    };
    use crate::routes::http::{ApiKeyPolicy, LocalOwnerPolicy};
    use crate::state::DaemonState;
    use anima_core::{
        AgentConfig, AgentSettings, Content, ModelAdapter, ModelGenerateRequest,
        ModelGenerateResponse, ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;
    use tokio::sync::{RwLock, Semaphore};
    use tower::util::ServiceExt;

    struct SlowModelAdapter {
        delay: Duration,
        calls: AtomicUsize,
    }

    struct GateFirstModelAdapter {
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
        calls: AtomicUsize,
    }

    struct TransactionalSwarmModelAdapter {
        context: StdMutex<Option<(AgentRunCoordinator, Arc<RwLock<DaemonState>>)>>,
    }

    struct CountingModelAdapter {
        calls: Arc<AtomicUsize>,
    }

    #[derive(Default)]
    struct CountingCredentialStore {
        calls: AtomicUsize,
        fail_put: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl ConnectorCredentialStore for CountingCredentialStore {
        async fn load(
            &self,
            _connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        async fn put(
            &self,
            _connector_id: &str,
            _token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_put.swap(false, Ordering::SeqCst) {
                return Err(CredentialStoreError::BackendUnavailable);
            }
            Ok(())
        }

        async fn delete(&self, _connector_id: &str) -> Result<(), CredentialStoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Default)]
    struct CountingTelegramTransport {
        calls: AtomicUsize,
        send_calls: AtomicUsize,
        get_me_error: StdMutex<Option<TelegramTransportError>>,
    }

    #[async_trait]
    impl TelegramTransport for CountingTelegramTransport {
        async fn get_me(
            &self,
            _token: &TelegramBotToken,
        ) -> Result<TelegramBotIdentity, TelegramTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self
                .get_me_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                return Err(error);
            }
            Ok(TelegramBotIdentity {
                id: "counting-bot".into(),
                username: None,
                display_name: None,
            })
        }

        async fn get_updates(
            &self,
            _token: &TelegramBotToken,
            offset: i64,
        ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(TelegramUpdateBatch {
                updates: Vec::new(),
                next_update_id: offset,
            })
        }

        async fn send_message(
            &self,
            _token: &TelegramBotToken,
            _chat_id: &str,
            _text: &str,
        ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.send_calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![TelegramSentMessage {
                message_id: "counting-message".into(),
                chat: TelegramChatMetadata {
                    id: "1".into(),
                    kind: TelegramChatKind::Private,
                    title: None,
                    username: None,
                },
            }])
        }
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
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                tokio::time::sleep(self.delay).await;
            }

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

    #[async_trait]
    impl ModelAdapter for CountingModelAdapter {
        fn provider(&self) -> &str {
            "counting"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(model_response(config))
        }
    }

    #[async_trait]
    impl ModelAdapter for GateFirstModelAdapter {
        fn provider(&self) -> &str {
            "gate-first"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .map_err(|_| "release gate closed".to_string())?
                    .forget();
            }
            Ok(model_response(config))
        }
    }

    #[async_trait]
    impl ModelAdapter for TransactionalSwarmModelAdapter {
        fn provider(&self) -> &str {
            "transactional-swarm"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            let context = self
                .context
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some((runs, state)) = context {
                let _transaction = runs.control_plane_transaction().await;
                let persist = {
                    let mut state = state.write().await;
                    state
                        .create_agent(test_config("created-from-swarm-model"))
                        .map_err(|error| error.to_string())?;
                    state.control_plane_persist_request()
                };
                persist.save().await.map_err(|error| error.to_string())?;
            }
            Ok(model_response(config))
        }
    }

    fn model_response(config: &AgentConfig) -> ModelGenerateResponse {
        ModelGenerateResponse {
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
        }
    }

    async fn create_test_swarm(app: &axum::Router, state: &Arc<RwLock<DaemonState>>) -> String {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/swarms")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{
                            "strategy":"round-robin",
                            "manager":{"name":"manager","model":"gpt-5.4"},
                            "workers":[{"name":"worker-a","model":"gpt-5.4"}],
                            "maxTurns":1
                        }"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        state.read().await.list_swarms()[0].id.clone()
    }

    fn custom_router(
        state: Arc<RwLock<DaemonState>>,
        runs: AgentRunCoordinator,
        config: DaemonConfig,
    ) -> axum::Router {
        let limiter = Arc::new(Semaphore::new(config.max_concurrent_runs));
        let connectors = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(TelegramClient::new().unwrap()),
        );
        router_with_services(state, config, limiter, runs, connectors, true)
    }

    #[tokio::test]
    async fn connector_owner_guard_precedes_vault_and_telegram_side_effects() {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(CountingModelAdapter {
            calls: Arc::clone(&model_calls),
        }));
        let agent_id = daemon
            .create_agent(test_config("guarded"))
            .unwrap()
            .state
            .id;
        let connector_id = "telegram-guarded".to_string();
        daemon.connectors.insert(
            connector_id.clone(),
            TelegramConnectorRecord {
                id: connector_id.clone(),
                agent_id: agent_id.clone(),
                room_id: "telegram-room-guarded".into(),
                bot: TelegramBotIdentity {
                    id: "counting-bot".into(),
                    username: Some("guard_bot".into()),
                    display_name: Some("Guard Bot".into()),
                },
                approved_chat: None,
                pending_pairing: Some(TelegramPendingPairing {
                    chat: TelegramChatMetadata {
                        id: "candidate-chat".into(),
                        kind: TelegramChatKind::Private,
                        title: None,
                        username: Some("candidate".into()),
                    },
                    requested_at_ms: 1,
                }),
                next_update_id: 0,
                enabled: true,
                deleted_at_ms: None,
                created_at_ms: 1,
                updated_at_ms: 1,
            },
        );
        let state = Arc::new(RwLock::new(daemon));
        let limiter = Arc::new(Semaphore::new(4));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
        let credentials = Arc::new(CountingCredentialStore::default());
        let transport = Arc::new(CountingTelegramTransport::default());
        let manager = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            credentials.clone(),
            transport.clone(),
        );
        let app = router_with_services(
            Arc::clone(&state),
            DaemonConfig::default(),
            limiter,
            runs,
            manager,
            true,
        );
        let create_path = format!("/api/agents/{agent_id}/connectors/telegram");
        let connector_path = format!("/api/agents/{agent_id}/connectors/{connector_id}");
        let mut requests = vec![
            Request::builder()
                .method("POST")
                .uri(&create_path)
                .header("host", "127.0.0.1:8080")
                .header("origin", "https://attacker.example")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"botToken":"42:must-not-reach-side-effects"}"#,
                ))
                .unwrap(),
            Request::builder()
                .method("PUT")
                .uri(format!("{connector_path}/credential"))
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("forwarded", "for=127.0.0.1")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"botToken":"42:must-not-reach-side-effects"}"#,
                ))
                .unwrap(),
            Request::builder()
                .method("POST")
                .uri(format!("{connector_path}/pairings/candidate-chat/approve"))
                .header("host", "127.0.0.1:8080")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("POST")
                .uri(format!("{connector_path}/restart"))
                .header("host", "127.0.0.1:8080")
                .header("origin", "https://attacker.example")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri(&connector_path)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("x-forwarded-for", "127.0.0.1")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("POST")
                .uri(format!("{connector_path}/messages"))
                .header("host", "127.0.0.1:8080")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"must not run"}"#))
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/agents/{agent_id}"))
                .header("host", "127.0.0.1:8080")
                .header("origin", "https://attacker.example")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/agents/{agent_id}"))
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("x-forwarded-host", "attacker.example")
                .body(Body::empty())
                .unwrap(),
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/agents/{agent_id}"))
                .header("host", "127.0.0.1:8080")
                .body(Body::empty())
                .unwrap(),
        ];

        let baseline_connector = state.read().await.connectors[&connector_id].clone();
        let baseline_agent = state.read().await.get_agent(&agent_id).unwrap();
        for request in requests.drain(..) {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        assert_eq!(credentials.calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
        assert_eq!(model_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            state.read().await.connectors[&connector_id],
            baseline_connector
        );
        assert_eq!(
            state.read().await.get_agent(&agent_id).unwrap(),
            baseline_agent
        );
        assert!(state.read().await.outbound.is_empty());
        assert!(state.read().await.inbound.is_empty());

        let remote_limiter = Arc::new(Semaphore::new(4));
        let remote_runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&remote_limiter));
        let remote_manager = ConnectorManager::new(
            Arc::clone(&state),
            remote_runs.clone(),
            credentials.clone(),
            transport.clone(),
        );
        let remote = router_with_services(
            Arc::clone(&state),
            DaemonConfig::default(),
            remote_limiter,
            remote_runs,
            remote_manager,
            false,
        );
        let response = remote
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/agents/{agent_id}"))
                    .header("host", "127.0.0.1:8080")
                    .header("origin", "http://localhost:4200")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(credentials.calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
        assert_eq!(model_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            state.read().await.connectors[&connector_id],
            baseline_connector
        );
        assert_eq!(
            state.read().await.get_agent(&agent_id).unwrap(),
            baseline_agent
        );
    }

    #[tokio::test]
    async fn api_key_and_local_owner_credentials_are_independent() {
        let mut daemon = DaemonState::new();
        let mut agent_ids = Vec::new();
        for name in [
            "combined-credentials",
            "wrong-local-owner",
            "wrong-global-key",
            "authorization-api-key",
            "x-api-key",
            "wrong-authorization-valid-x-api-key",
        ] {
            agent_ids.push(daemon.create_agent(test_config(name)).unwrap().state.id);
        }
        let state = Arc::new(RwLock::new(daemon));
        let limiter = Arc::new(Semaphore::new(4));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
        let manager = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(CountingTelegramTransport::default()),
        );
        let scheduler = crate::schedules::SchedulerService::new(
            Arc::clone(&state),
            runs.clone(),
            manager.clone(),
        );
        let app = router_with_services_with_policies(
            Arc::clone(&state),
            DaemonConfig::default(),
            limiter,
            runs,
            manager,
            scheduler,
            LocalOwnerPolicy::for_test(true, Some("local-admin")),
            ApiKeyPolicy::for_test(Some("global-api")),
        );

        let cases = [
            (
                &agent_ids[0],
                Some("Bearer local-admin"),
                Some("global-api"),
                None,
                StatusCode::OK,
            ),
            (
                &agent_ids[1],
                Some("Bearer wrong-local"),
                Some("global-api"),
                None,
                StatusCode::FORBIDDEN,
            ),
            (
                &agent_ids[2],
                Some("Bearer local-admin"),
                Some("wrong-global"),
                None,
                StatusCode::UNAUTHORIZED,
            ),
            (
                &agent_ids[3],
                Some("Bearer global-api"),
                None,
                Some("http://localhost:4200"),
                StatusCode::OK,
            ),
            (
                &agent_ids[4],
                None,
                Some("global-api"),
                Some("http://localhost:4200"),
                StatusCode::OK,
            ),
            (
                &agent_ids[5],
                Some("Bearer wrong-global"),
                Some("global-api"),
                Some("http://localhost:4200"),
                StatusCode::OK,
            ),
        ];

        for (agent_id, authorization, x_api_key, origin, expected) in cases {
            let mut request = Request::builder()
                .method("DELETE")
                .uri(format!("/api/agents/{agent_id}"))
                .header("host", "127.0.0.1:8080");
            if let Some(authorization) = authorization {
                request = request.header("authorization", authorization);
            }
            if let Some(x_api_key) = x_api_key {
                request = request.header("x-api-key", x_api_key);
            }
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "agent {agent_id}");
        }

        let guard = state.read().await;
        assert!(guard.get_agent(&agent_ids[0]).is_none());
        assert!(guard.get_agent(&agent_ids[1]).is_some());
        assert!(guard.get_agent(&agent_ids[2]).is_some());
        assert!(guard.get_agent(&agent_ids[3]).is_none());
        assert!(guard.get_agent(&agent_ids[4]).is_none());
        assert!(guard.get_agent(&agent_ids[5]).is_none());
    }

    #[tokio::test]
    async fn timed_out_owner_send_retry_singleflights_and_delivers_once() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let adapter = Arc::new(GateFirstModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            calls: AtomicUsize::new(0),
        });
        let mut daemon = DaemonState::with_model_adapter(adapter.clone());
        let agent_id = daemon
            .create_agent(test_config("timeout-idempotency"))
            .unwrap()
            .state
            .id;
        let connector_id = "telegram-timeout-idempotency".to_string();
        let room_id = "telegram-room-timeout-idempotency".to_string();
        daemon.connectors.insert(
            connector_id.clone(),
            TelegramConnectorRecord {
                id: connector_id.clone(),
                agent_id: agent_id.clone(),
                room_id: room_id.clone(),
                bot: TelegramBotIdentity {
                    id: "timeout-bot".into(),
                    username: Some("timeout_bot".into()),
                    display_name: None,
                },
                approved_chat: Some(TelegramChatMetadata {
                    id: "timeout-chat".into(),
                    kind: TelegramChatKind::Private,
                    title: None,
                    username: None,
                }),
                pending_pairing: None,
                next_update_id: 0,
                enabled: true,
                deleted_at_ms: None,
                created_at_ms: 1,
                updated_at_ms: 1,
            },
        );
        let state = Arc::new(RwLock::new(daemon));
        let limiter = Arc::new(Semaphore::new(4));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
        let credentials = Arc::new(InMemoryCredentialStore::default());
        credentials
            .put(
                &connector_id,
                TelegramBotToken::parse("42:timeout-owner-send").unwrap(),
            )
            .await
            .unwrap();
        let transport = Arc::new(CountingTelegramTransport::default());
        let manager = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            credentials,
            transport.clone(),
        );
        let mut config = DaemonConfig::default();
        config.request_timeout = Duration::from_millis(30);
        let app = router_with_services(
            Arc::clone(&state),
            config,
            limiter,
            runs,
            manager.clone(),
            true,
        );
        let uri = format!("/api/agents/{agent_id}/connectors/{connector_id}/messages");
        let request = || {
            Request::builder()
                .method("POST")
                .uri(&uri)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("idempotency-key", "timeout-owner-key")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"timeout owner turn"}"#))
                .unwrap()
        };

        let first = {
            let app = app.clone();
            let request = request();
            tokio::spawn(async move { app.oneshot(request).await.unwrap() })
        };
        entered.acquire().await.unwrap().forget();
        assert_eq!(first.await.unwrap().status(), StatusCode::REQUEST_TIMEOUT);

        let retry = {
            let app = app.clone();
            let request = request();
            tokio::spawn(async move { app.oneshot(request).await.unwrap() })
        };
        tokio::task::yield_now().await;
        release.add_permits(1);
        assert_eq!(retry.await.unwrap().status(), StatusCode::OK);

        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        let guard = state.read().await;
        let snapshot = guard.get_agent(&agent_id).unwrap();
        assert_eq!(
            snapshot
                .messages
                .iter()
                .filter(|message| message.room_id == room_id)
                .count(),
            2
        );
        assert_eq!(
            guard
                .outbound
                .values()
                .filter(|outbound| outbound.connector_id == connector_id)
                .count(),
            1
        );
        drop(guard);

        assert!(manager
            .deliver_pending_once(connector_id.clone())
            .await
            .unwrap());
        assert_eq!(transport.send_calls.load(Ordering::SeqCst), 1);
        assert!(state.read().await.outbound.values().any(|outbound| {
            outbound.connector_id == connector_id
                && outbound.delivery_state == OutboundDeliveryState::Delivered
        }));
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn connector_create_distinguishes_rejected_tokens_from_transient_telegram_failures() {
        for (error, expected_status, expected_code) in [
            (
                TelegramTransportError::UpstreamApi { code: Some(401) },
                StatusCode::UNPROCESSABLE_ENTITY,
                "connector_token_invalid",
            ),
            (
                TelegramTransportError::Transport,
                StatusCode::SERVICE_UNAVAILABLE,
                "connector_upstream_unavailable",
            ),
        ] {
            let mut daemon = DaemonState::new();
            let agent_id = daemon
                .create_agent(test_config("token-validation"))
                .unwrap()
                .state
                .id;
            let state = Arc::new(RwLock::new(daemon));
            let limiter = Arc::new(Semaphore::new(4));
            let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
            let credentials = Arc::new(CountingCredentialStore::default());
            let transport = Arc::new(CountingTelegramTransport {
                calls: AtomicUsize::new(0),
                send_calls: AtomicUsize::new(0),
                get_me_error: StdMutex::new(Some(error)),
            });
            let manager = ConnectorManager::new(
                Arc::clone(&state),
                runs.clone(),
                credentials.clone(),
                transport.clone(),
            );
            let app = router_with_services(
                Arc::clone(&state),
                DaemonConfig::default(),
                limiter,
                runs,
                manager,
                true,
            );
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/agents/{agent_id}/connectors/telegram"))
                        .header("host", "127.0.0.1:8080")
                        .header("origin", "http://localhost:4200")
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"botToken":"42:rejected-or-down"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected_status);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["code"], expected_code);
            assert_eq!(credentials.calls.load(Ordering::SeqCst), 0);
            assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
            assert!(state.read().await.connectors.is_empty());
        }
    }

    #[tokio::test]
    async fn connector_create_maps_credential_store_failure_to_stable_public_error() {
        let mut daemon = DaemonState::new();
        let agent_id = daemon
            .create_agent(test_config("credential-store-failure"))
            .unwrap()
            .state
            .id;
        let state = Arc::new(RwLock::new(daemon));
        let limiter = Arc::new(Semaphore::new(4));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
        let credentials = Arc::new(CountingCredentialStore {
            calls: AtomicUsize::new(0),
            fail_put: std::sync::atomic::AtomicBool::new(true),
        });
        let transport = Arc::new(CountingTelegramTransport::default());
        let manager = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            credentials.clone(),
            transport.clone(),
        );
        let app = router_with_services(
            Arc::clone(&state),
            DaemonConfig::default(),
            limiter,
            runs,
            manager,
            true,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/agents/{agent_id}/connectors/telegram"))
                    .header("host", "127.0.0.1:8080")
                    .header("origin", "http://localhost:4200")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"botToken":"42:telegram-secret-sentinel"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["code"], "connector_credential_store_unavailable");
        assert!(!text.contains("telegram-secret-sentinel"));
        assert!(credentials.calls.load(Ordering::SeqCst) >= 1);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert!(state.read().await.connectors.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn swarm_model_callback_can_publish_through_the_shared_transaction_gate() {
        let adapter = Arc::new(TransactionalSwarmModelAdapter {
            context: StdMutex::new(None),
        });
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(
            adapter.clone(),
        )));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
        *adapter
            .context
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((runs.clone(), Arc::clone(&state)));
        let app = custom_router(
            Arc::clone(&state),
            runs,
            DaemonConfig {
                request_timeout: Duration::from_millis(500),
                ..DaemonConfig::default()
            },
        );
        let swarm_id = create_test_swarm(&app, &state).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/swarms/{swarm_id}/run"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"mutate during dispatch"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(state
            .read()
            .await
            .list_agents()
            .iter()
            .any(|agent| agent.state.config.name == "created-from-swarm-model"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn another_control_plane_publisher_progresses_during_swarm_dispatch() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            GateFirstModelAdapter {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
                calls: AtomicUsize::new(0),
            },
        ))));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
        let app = custom_router(
            Arc::clone(&state),
            runs,
            DaemonConfig {
                request_timeout: Duration::from_secs(2),
                ..DaemonConfig::default()
            },
        );
        let swarm_id = create_test_swarm(&app, &state).await;
        let running_app = app.clone();
        let running = tokio::spawn(async move {
            running_app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/swarms/{swarm_id}/run"))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"text":"hold dispatch"}"#))
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        entered.acquire().await.unwrap().forget();

        let publisher = tokio::time::timeout(
            Duration::from_millis(100),
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/agents")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"parallel-publisher","model":"gpt-5.4"}"#,
                    ))
                    .unwrap(),
            ),
        )
        .await;
        release.add_permits(1);
        let running_response = running.await.unwrap();

        assert_eq!(
            publisher
                .expect("publisher must not wait for swarm dispatch")
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(running_response.status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_routes_respect_request_timeout() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            SlowModelAdapter {
                delay: Duration::from_millis(50),
                calls: AtomicUsize::new(0),
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
            Arc::clone(&state),
            DaemonConfig {
                request_timeout: Duration::from_millis(10),
                ..DaemonConfig::default()
            },
        );

        let response = app
            .clone()
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

        for _ in 0..100 {
            if state
                .read()
                .await
                .get_agent(&agent_id)
                .is_some_and(|snapshot| snapshot.state.status.as_str() == "completed")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let retry = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/agents/{agent_id}/run"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"run after timeout"}"#))
                    .expect("retry request builds"),
            )
            .await
            .expect("retry response should be returned");
        assert_eq!(retry.status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_routes_reject_before_parsing_when_concurrency_limit_is_exhausted() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            SlowModelAdapter {
                delay: Duration::from_millis(75),
                calls: AtomicUsize::new(0),
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
            .body(Body::from(r#"{"text":"#))
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

    #[tokio::test(flavor = "multi_thread")]
    async fn malformed_run_body_releases_early_admission_permit() {
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            SlowModelAdapter {
                delay: Duration::ZERO,
                calls: AtomicUsize::new(0),
            },
        ))));
        let agent_id = state
            .write()
            .await
            .create_agent(test_config("operator"))
            .expect("agent should be created")
            .state
            .id;
        let app = router(
            state,
            DaemonConfig {
                max_concurrent_runs: 1,
                ..DaemonConfig::default()
            },
        );

        let malformed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/agents/{agent_id}/run"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"#))
                    .expect("malformed request builds"),
            )
            .await
            .expect("malformed response should be returned");
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

        let valid = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/agents/{agent_id}/run"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"permit was released"}"#))
                    .expect("valid request builds"),
            )
            .await
            .expect("valid response should be returned");
        assert_eq!(valid.status(), StatusCode::OK);
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
