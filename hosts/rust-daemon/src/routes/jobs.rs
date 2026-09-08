use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{json_response, parse_json_body, read_limited_body, ApiError, AppState};
use crate::jobs::{AgentJobRecord, JobError, JobReviewDecision};

#[derive(Serialize, ToSchema)]
pub(super) struct JobsResponse {
    pub(super) jobs: Vec<AgentJobRecord>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateJobRequest {
    title: String,
    prompt: String,
    request_key: String,
    #[serde(default = "default_max_attempts")]
    max_attempts: u32,
    #[serde(default)]
    requires_approval: bool,
    #[serde(default)]
    goal_id: Option<String>,
}

fn default_max_attempts() -> u32 {
    3
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewJobRequest {
    revision: u64,
    decision: JobReviewDecision,
    #[serde(default)]
    note: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CancelJobRequest {
    revision: u64,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RetryJobRequest {
    revision: u64,
    #[serde(default)]
    acknowledge_uncertain: bool,
}

pub(super) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) fn authorize(state: &AppState, request: &Request, read: bool) -> Result<(), Response> {
    let result = if read {
        state.local_owner.authorize_read(request.headers())
    } else {
        state.local_owner.authorize(request.headers())
    };
    result.map_err(|_| {
        no_store(
            ApiError {
                status: StatusCode::FORBIDDEN,
                message: "local owner authorization required".into(),
            }
            .into_response(),
        )
    })
}

pub(super) fn error_response(error: JobError) -> Response {
    let error = match error {
        JobError::NotFound => ApiError::not_found(),
        JobError::Validation(message) => ApiError::bad_request(message),
        JobError::Conflict(message) => ApiError::conflict(message),
        JobError::Unavailable(message) => ApiError::service_unavailable(message),
    };
    no_store(error.into_response())
}

fn job_response(result: Result<AgentJobRecord, JobError>) -> Response {
    match result {
        Ok(job) => no_store(json_response(StatusCode::OK, &job)),
        Err(error) => error_response(error),
    }
}

pub(super) async fn body<T: serde::de::DeserializeOwned>(
    state: &AppState,
    request: Request,
) -> Result<T, Response> {
    let bytes = read_limited_body(request, state.config.max_request_bytes)
        .await
        .map_err(no_store)?;
    parse_json_body(bytes).map_err(|error| no_store(error.into_response()))
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/jobs", tag = "jobs",
    params(("agent_id" = String, Path)),
    responses((status = 200, description = "Agent-owned job history", body = JobsResponse), (status = 403, description = "Local owner required")))]
pub(super) async fn list_jobs(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    match state.jobs.list(&agent_id).await {
        Ok(jobs) => no_store(json_response(StatusCode::OK, &JobsResponse { jobs })),
        Err(error) => error_response(error),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/jobs", tag = "jobs",
    params(("agent_id" = String, Path)), request_body = CreateJobRequest,
    responses((status = 200, description = "Saved job; an identical request key returns the same job", body = AgentJobRecord), (status = 409, description = "Persistence missing, capacity, or request-key conflict"), (status = 403, description = "Local owner required")))]
pub(super) async fn create_job(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: CreateJobRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    job_response(
        state
            .jobs
            .create_with_goal(
                &agent_id,
                &input.title,
                &input.prompt,
                &input.request_key,
                input.max_attempts,
                input.requires_approval,
                input.goal_id.as_deref(),
            )
            .await,
    )
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/jobs/{job_id}/approve", tag = "jobs",
    params(("agent_id" = String, Path), ("job_id" = String, Path)), request_body = CancelJobRequest,
    responses((status = 200, description = "Owner approved this attempt for dispatch", body = AgentJobRecord), (status = 409, description = "Stale revision, invalid state, or capacity"), (status = 403, description = "Local owner required")))]
pub(super) async fn approve_job(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: CancelJobRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    job_response(state.jobs.approve(&agent_id, &job_id, input.revision).await)
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/jobs/{job_id}/review", tag = "jobs",
    params(("agent_id" = String, Path), ("job_id" = String, Path)), request_body = ReviewJobRequest,
    responses((status = 200, description = "Owner decision saved against the latest completed output", body = AgentJobRecord), (status = 400, description = "Invalid review feedback"), (status = 409, description = "Stale revision or already reviewed output"), (status = 403, description = "Local owner required")))]
pub(super) async fn review_job(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: ReviewJobRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    job_response(
        state
            .jobs
            .review_output(
                &agent_id,
                &job_id,
                input.revision,
                input.decision,
                &input.note,
            )
            .await,
    )
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/jobs/{job_id}/cancel", tag = "jobs",
    params(("agent_id" = String, Path), ("job_id" = String, Path)), request_body = CancelJobRequest,
    responses((status = 200, description = "Queued job cancelled", body = AgentJobRecord), (status = 409, description = "Stale revision or job already dispatched"), (status = 403, description = "Local owner required")))]
pub(super) async fn cancel_job(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: CancelJobRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    job_response(state.jobs.cancel(&agent_id, &job_id, input.revision).await)
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/jobs/{job_id}/retry", tag = "jobs",
    params(("agent_id" = String, Path), ("job_id" = String, Path)), request_body = RetryJobRequest,
    responses((status = 200, description = "Explicit new attempt queued", body = AgentJobRecord), (status = 409, description = "Stale revision, invalid state, acknowledgement missing, or retry limit"), (status = 403, description = "Local owner required")))]
pub(super) async fn retry_job(
    State(state): State<AppState>,
    Path((agent_id, job_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: RetryJobRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    job_response(
        state
            .jobs
            .retry(
                &agent_id,
                &job_id,
                input.revision,
                input.acknowledge_uncertain,
            )
            .await,
    )
}
