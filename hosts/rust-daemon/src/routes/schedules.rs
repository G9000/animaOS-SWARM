use axum::extract::{Path, Request as AxumRequest, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response as AxumResponse;

use crate::schedules::{legacy_next_due_at_ms, ScheduleError, ScheduleTarget, ScheduleTrigger};

use super::contracts::{
    ConnectorErrorBody, DeleteResponse, LegacyScheduleImportRequest, ScheduleCreateRequest,
    ScheduleEnvelope, ScheduleResponse, ScheduleUpdateRequest, SchedulesEnvelope,
};
use super::http::{json_response, read_limited_body, LocalOwnerRejection};
use super::{parse_json_body, AppState};

#[utoipa::path(get, path = "/api/agents/{agent_id}/schedules", tag = "schedules", params(("agent_id" = String, Path)), responses((status = 200, body = SchedulesEnvelope), (status = 404, body = ConnectorErrorBody)))]
pub(super) async fn list_schedules(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> AxumResponse {
    match state.scheduler.list(&agent_id).await {
        Ok(items) => json_response(
            StatusCode::OK,
            &SchedulesEnvelope {
                schedules: items.into_iter().map(Into::into).collect(),
            },
        ),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/schedules", tag = "schedules", params(("agent_id" = String, Path)), request_body = ScheduleCreateRequest, responses((status = 201, body = ScheduleEnvelope), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody)))]
pub(super) async fn create_schedule(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid("malformed request"),
    };
    let request = match parse_json_body::<ScheduleCreateRequest>(body) {
        Ok(request) => request,
        Err(_) => return invalid("request body is invalid"),
    };
    match state
        .scheduler
        .create(
            agent_id,
            request.prompt,
            request.trigger.into(),
            request.target.into(),
            request.enabled.unwrap_or(true),
            request.import_idempotency_key,
            None,
            None,
        )
        .await
    {
        Ok((record, created)) => no_store(json_response(
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            &ScheduleEnvelope {
                schedule: record.into(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(patch, path = "/api/agents/{agent_id}/schedules/{schedule_id}", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path)), request_body = ScheduleUpdateRequest, responses((status = 200, body = ScheduleEnvelope), (status = 400, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody)))]
pub(super) async fn update_schedule(
    State(state): State<AppState>,
    Path((agent_id, schedule_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid("malformed request"),
    };
    let request = match parse_json_body::<ScheduleUpdateRequest>(body) {
        Ok(request) => request,
        Err(_) => return invalid("request body is invalid"),
    };
    match state
        .scheduler
        .update(
            &agent_id,
            &schedule_id,
            request.prompt,
            request.trigger.map(Into::into),
            request.target.map(Into::into),
            request.enabled,
        )
        .await
    {
        Ok(record) => no_store(json_response(
            StatusCode::OK,
            &ScheduleEnvelope {
                schedule: record.into(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(delete, path = "/api/agents/{agent_id}/schedules/{schedule_id}", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path)), responses((status = 200, body = DeleteResponse), (status = 404, body = ConnectorErrorBody)))]
pub(super) async fn delete_schedule(
    State(state): State<AppState>,
    Path((agent_id, schedule_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    match state.scheduler.delete(&agent_id, &schedule_id).await {
        Ok(()) => no_store(json_response(
            StatusCode::OK,
            &DeleteResponse { deleted: true },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/schedules/import", tag = "schedules", params(("agent_id" = String, Path)), request_body = LegacyScheduleImportRequest, responses((status = 200, body = SchedulesEnvelope), (status = 400, body = ConnectorErrorBody)))]
pub(super) async fn import_legacy_schedules(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid("malformed request"),
    };
    let request = match parse_json_body::<LegacyScheduleImportRequest>(body) {
        Ok(request) => request,
        Err(_) => return invalid("request body is invalid"),
    };
    if request.schedules.len() > 100 {
        return invalid("too many legacy schedules");
    }
    let mut schedules = Vec::with_capacity(request.schedules.len());
    for item in request.schedules {
        if item.id.trim().is_empty() || item.id.len() > 256 {
            return invalid("legacy schedule id is invalid");
        }
        let next_due = match legacy_next_due_at_ms(
            item.created_at_ms,
            item.last_run_at_ms,
            item.interval_secs,
        ) {
            Ok(value) => value,
            Err(error) => return schedule_error(error),
        };
        let interval_ms = match item.interval_secs.checked_mul(1_000) {
            Some(value) => value,
            None => return invalid("legacy schedule timing overflow"),
        };
        let result = state
            .scheduler
            .create(
                agent_id.clone(),
                item.prompt,
                ScheduleTrigger::Interval { interval_ms },
                item.target
                    .map(Into::into)
                    .unwrap_or(ScheduleTarget::Workspace),
                true,
                Some(format!("legacy:{}:{}", agent_id, item.id)),
                Some(next_due),
                Some(item.created_at_ms),
            )
            .await;
        match result {
            Ok((record, _)) => schedules.push(ScheduleResponse::from(record)),
            Err(error) => return schedule_error(error),
        }
    }
    no_store(json_response(
        StatusCode::OK,
        &SchedulesEnvelope { schedules },
    ))
}

fn schedule_error(error: ScheduleError) -> AxumResponse {
    match error {
        ScheduleError::AgentNotFound | ScheduleError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            "schedule_not_found",
            "schedule was not found",
        ),
        ScheduleError::Invalid(message) => {
            error_response(StatusCode::BAD_REQUEST, "schedule_invalid", message)
        }
        ScheduleError::TargetUnavailable => error_response(
            StatusCode::CONFLICT,
            "schedule_target_unavailable",
            "schedule target is unavailable",
        ),
        ScheduleError::Persistence => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "schedule_persistence_unavailable",
            "schedule persistence is unavailable",
        ),
    }
}

fn invalid(message: &str) -> AxumResponse {
    error_response(StatusCode::BAD_REQUEST, "schedule_invalid", message)
}

fn local_owner_error(rejection: LocalOwnerRejection) -> AxumResponse {
    match rejection {
        LocalOwnerRejection::LocalAdminRequired => error_response(
            StatusCode::FORBIDDEN,
            "connector_local_admin_required",
            "local schedule administration authorization is required",
        ),
        LocalOwnerRejection::OriginRejected => error_response(
            StatusCode::FORBIDDEN,
            "connector_origin_rejected",
            "browser origin is not approved for schedule administration",
        ),
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> AxumResponse {
    no_store(json_response(
        status,
        &ConnectorErrorBody {
            code: code.into(),
            error: message.into(),
        },
    ))
}

fn no_store(mut response: AxumResponse) -> AxumResponse {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
