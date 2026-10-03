use anima_core::primitives::now_millis;
use axum::extract::{Path, Request as AxumRequest, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response as AxumResponse;

use crate::schedules::{
    heartbeat_input, legacy_next_due_at_ms, preview, AutomationInput, AutomationPatch,
    ScheduleError, ScheduleTarget, ScheduleTrigger, AUTOMATION_HISTORY_UNAVAILABLE,
    MAX_AUTOMATION_HISTORY_SHOWN, PROMPT_AND_TRIGGER_REQUIRED,
};

use super::contracts::{
    ConnectorErrorBody, DeleteResponse, LegacyScheduleImportRequest, PresetRequest,
    ScheduleCreateRequest, ScheduleEnvelope, SchedulePreviewRequest, SchedulePreviewResponse,
    ScheduleResponse, ScheduleRunsEnvelope, ScheduleUpdateRequest, SchedulesEnvelope,
};
use super::http::{json_response, read_limited_body, request_query, LocalOwnerRejection};
use super::{parse_json_body, AppState};

/// Spec §16: the history shows the latest 50.
pub(crate) const HISTORY_LIMIT_INVALID: &str = "limit must be from 1 to 50";

#[utoipa::path(get, path = "/api/agents/{agent_id}/schedules", tag = "schedules", params(("agent_id" = String, Path)), responses((status = 200, body = SchedulesEnvelope), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody)))]
pub(super) async fn list_schedules(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize_read(request.headers()) {
        return local_owner_error(rejection);
    }
    match state.scheduler.list(&agent_id).await {
        Ok(items) => no_store(json_response(
            StatusCode::OK,
            &SchedulesEnvelope {
                schedules: items.into_iter().map(Into::into).collect(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/schedules", tag = "schedules", params(("agent_id" = String, Path)), request_body = ScheduleCreateRequest, responses((status = 201, body = ScheduleEnvelope), (status = 200, body = ScheduleEnvelope), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 409, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
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
    let input = match automation_input(agent_id, request) {
        Ok(input) => input,
        Err(error) => return schedule_error(error),
    };
    match state
        .scheduler
        .automations()
        .create(input, now_millis())
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

/// A create request as the service's input: the heartbeat preset's fields
/// unless the request names its own (spec §9.2).
fn automation_input(
    agent_id: String,
    request: ScheduleCreateRequest,
) -> Result<AutomationInput, ScheduleError> {
    let target = request
        .target
        .map(Into::into)
        .unwrap_or(ScheduleTarget::Workspace);
    let mut input = match request.preset {
        Some(PresetRequest::Heartbeat) => heartbeat_input(
            agent_id,
            request.time_zone.as_deref().unwrap_or_default(),
            target,
        )?,
        None => {
            let (Some(prompt), Some(trigger)) = (request.prompt.clone(), request.trigger.clone())
            else {
                return Err(ScheduleError::Invalid(PROMPT_AND_TRIGGER_REQUIRED));
            };
            AutomationInput::owner(agent_id, prompt, ScheduleTrigger::from(trigger), target)
        }
    };
    if let Some(prompt) = request.prompt {
        input.prompt = prompt;
    }
    if let Some(trigger) = request.trigger {
        input.trigger = trigger.into();
    }
    if let Some(name) = request.name {
        input.name = Some(name);
    }
    if let Some(hours) = request.active_hours {
        input.active_hours = Some(hours.into());
    }
    if let Some(enabled) = request.enabled {
        input.enabled = enabled;
    }
    input.import_idempotency_key = request.import_idempotency_key;
    Ok(input)
}

#[utoipa::path(patch, path = "/api/agents/{agent_id}/schedules/{schedule_id}", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path)), request_body = ScheduleUpdateRequest, responses((status = 200, body = ScheduleEnvelope), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
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
    let patch = AutomationPatch {
        name: request.name,
        prompt: request.prompt,
        trigger: request.trigger.map(Into::into),
        active_hours: request.active_hours.map(|hours| hours.map(Into::into)),
        target: request.target.map(Into::into),
        enabled: request.enabled,
    };
    match state
        .scheduler
        .automations()
        .update(&agent_id, &schedule_id, patch, now_millis())
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

#[utoipa::path(post, path = "/api/agents/{agent_id}/schedules/{schedule_id}/run", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path)), responses((status = 202, body = ScheduleEnvelope), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 409, body = ConnectorErrorBody), (status = 429, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
pub(super) async fn run_schedule(
    State(state): State<AppState>,
    Path((agent_id, schedule_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    match state.scheduler.run_now(&agent_id, &schedule_id).await {
        Ok(record) => no_store(json_response(
            StatusCode::ACCEPTED,
            &ScheduleEnvelope {
                schedule: record.into(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/schedules/{schedule_id}/history", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path), ("limit" = Option<usize>, Query, description = "1 to 50, default 50")), responses((status = 200, body = ScheduleRunsEnvelope), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
pub(super) async fn schedule_history(
    State(state): State<AppState>,
    Path((agent_id, schedule_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize_read(request.headers()) {
        return local_owner_error(rejection);
    }
    let Ok(params) = request_query(request.uri()) else {
        return invalid("malformed query");
    };
    let limit = match params.get("limit").map(String::as_str) {
        None | Some("") => MAX_AUTOMATION_HISTORY_SHOWN,
        Some(value) => match value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_AUTOMATION_HISTORY_SHOWN).contains(limit))
        {
            Some(limit) => limit,
            None => return invalid(HISTORY_LIMIT_INVALID),
        },
    };
    match state
        .scheduler
        .automations()
        .history(&agent_id, &schedule_id, limit)
        .await
    {
        Ok(runs) => no_store(json_response(
            StatusCode::OK,
            &ScheduleRunsEnvelope {
                runs: runs.into_iter().map(Into::into).collect(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(post, path = "/api/schedules/preview", tag = "schedules", request_body = SchedulePreviewRequest, responses((status = 200, body = SchedulePreviewResponse), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody)))]
pub(super) async fn preview_schedule(
    State(state): State<AppState>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize_read(request.headers()) {
        return local_owner_error(rejection);
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid("malformed request"),
    };
    let request = match parse_json_body::<SchedulePreviewRequest>(body) {
        Ok(request) => request,
        Err(_) => return invalid("request body is invalid"),
    };
    let active_hours = request.active_hours.map(Into::into);
    match preview(&request.trigger.into(), active_hours.as_ref(), now_millis()) {
        Ok(next_runs) => no_store(json_response(
            StatusCode::OK,
            &SchedulePreviewResponse { next_runs },
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
        ScheduleError::Rejected(message) => {
            error_response(StatusCode::BAD_REQUEST, "schedule_invalid", &message)
        }
        ScheduleError::Conflict(message) => {
            error_response(StatusCode::CONFLICT, "schedule_conflict", message)
        }
        ScheduleError::Busy(message) => {
            error_response(StatusCode::TOO_MANY_REQUESTS, "schedule_busy", message)
        }
        ScheduleError::HistoryUnavailable => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "schedule_history_unavailable",
            AUTOMATION_HISTORY_UNAVAILABLE,
        ),
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
