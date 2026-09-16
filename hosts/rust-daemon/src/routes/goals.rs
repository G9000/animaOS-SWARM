use super::jobs::{authorize, body, error_response, no_store, JobsResponse};
use super::{json_response, AppState};
use crate::jobs::{GoalStatus, GoalView, JobError};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(super) struct GoalsResponse {
    goals: Vec<GoalView>,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateGoalRequest {
    title: String,
    objective: String,
    request_key: String,
    max_attempts: u32,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GoalStatusRequest {
    revision: u64,
    status: GoalStatus,
}

fn goal_response(result: Result<GoalView, JobError>) -> Response {
    match result {
        Ok(goal) => no_store(json_response(StatusCode::OK, &goal)),
        Err(error) => error_response(error),
    }
}

#[utoipa::path(get, path="/api/goals", tag="goals",
    responses((status=200, description="Saved goals and aggregate attempt budgets", body=GoalsResponse), (status=403, description="Local owner required")))]
pub(super) async fn list_goals(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    match state.jobs.list_goals().await {
        Ok(goals) => no_store(json_response(StatusCode::OK, &GoalsResponse { goals })),
        Err(error) => error_response(error),
    }
}

#[utoipa::path(post, path="/api/goals", tag="goals", request_body=CreateGoalRequest,
    responses((status=200, description="Saved goal; exact key replay returns existing goal", body=GoalView), (status=409, description="Persistence, capacity, or key conflict"), (status=403, description="Local owner required")))]
pub(super) async fn create_goal(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: CreateGoalRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    goal_response(
        state
            .jobs
            .create_goal(
                &input.title,
                &input.objective,
                &input.request_key,
                input.max_attempts,
            )
            .await,
    )
}

#[utoipa::path(post, path="/api/goals/{goal_id}/status", tag="goals", params(("goal_id"=String,Path)), request_body=GoalStatusRequest,
    responses((status=200, description="Status saved after revision and completion checks", body=GoalView), (status=409, description="Stale revision or invalid transition"), (status=403, description="Local owner required")))]
pub(super) async fn change_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: GoalStatusRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    goal_response(
        state
            .jobs
            .change_goal_status(&id, input.revision, input.status)
            .await,
    )
}

#[utoipa::path(get, path="/api/goals/{goal_id}/jobs", tag="goals", params(("goal_id"=String,Path)),
    responses((status=200, description="Linked assignments and saved outputs across agents", body=JobsResponse), (status=404, description="Goal not found"), (status=403, description="Local owner required")))]
pub(super) async fn goal_jobs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    match state.jobs.goal_jobs(&id).await {
        Ok(jobs) => no_store(json_response(StatusCode::OK, &JobsResponse { jobs })),
        Err(error) => error_response(error),
    }
}
