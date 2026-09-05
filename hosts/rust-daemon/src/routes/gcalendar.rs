use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response as AxumResponse};

use crate::connectors::gcalendar::CalendarError;

use super::contracts::{
    CalendarConnectEnvelope, CalendarConnectorResponse, CalendarStatusEnvelope,
    CalendarWriteEnvelope, CalendarWriteResponse, CalendarWritesEnvelope, ConnectorErrorBody,
    DeleteResponse,
};
use super::http::{json_response, request_query, LocalOwnerRejection};
use super::AppState;

#[utoipa::path(
    get,
    path = "/api/agents/{agent_id}/connectors/gcalendar",
    tag = "connectors",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    responses(
        (status = 200, description = "Google Calendar connection status", body = CalendarStatusEnvelope),
        (status = 404, description = "Agent not found", body = ConnectorErrorBody)
    )
)]
pub(super) async fn get_gcalendar_connector(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> AxumResponse {
    {
        let daemon = state.daemon.read().await;
        if daemon.get_agent(&agent_id).is_none() {
            return not_found_error();
        }
    }
    let record = state.calendar.connector_for_agent(&agent_id).await;
    no_store(json_response(
        StatusCode::OK,
        &CalendarStatusEnvelope {
            connector: record.as_ref().map(CalendarConnectorResponse::from_record),
            configured: state.calendar.oauth_configured().await.unwrap_or(false),
        },
    ))
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/connectors/gcalendar",
    tag = "connectors",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    responses(
        (status = 201, description = "OAuth pairing started; open the consent URL", body = CalendarConnectEnvelope),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Agent not found", body = ConnectorErrorBody),
        (status = 409, description = "Agent already has a Google Calendar connector", body = ConnectorErrorBody),
        (status = 503, description = "Google OAuth is not configured or persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn connect_gcalendar(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: axum::extract::Request,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    match state.calendar.begin_connect(&agent_id).await {
        Ok((record, consent_url)) => no_store(json_response(
            StatusCode::CREATED,
            &CalendarConnectEnvelope {
                connector: CalendarConnectorResponse::from_record(&record),
                consent_url,
            },
        )),
        Err(error) => calendar_error(error),
    }
}

/// Browser-facing OAuth redirect target. The `state` nonce is the CSRF guard;
/// this endpoint intentionally does not require local-owner headers because
/// it is reached via Google's redirect.
#[utoipa::path(
    get,
    path = "/api/connectors/gcalendar/callback",
    tag = "connectors",
    responses(
        (status = 200, description = "OAuth completed; the tab can be closed"),
        (status = 400, description = "Missing or invalid OAuth parameters"),
        (status = 503, description = "Google, vault, or persistence unavailable")
    )
)]
pub(super) async fn gcalendar_oauth_callback(
    State(state): State<AppState>,
    uri: axum::http::Uri,
) -> AxumResponse {
    let query = match request_query(&uri) {
        Ok(query) => query,
        Err(_) => return callback_page(StatusCode::BAD_REQUEST, "Malformed OAuth callback."),
    };
    let code = query.get("code").cloned().unwrap_or_default();
    let nonce = query.get("state").cloned().unwrap_or_default();
    if code.is_empty() || nonce.is_empty() {
        return callback_page(
            StatusCode::BAD_REQUEST,
            "Google authorization was not completed. Try connecting again from Connectors.",
        );
    }
    match state.calendar.complete_connect(&nonce, &code).await {
        Ok(_) => callback_page(
            StatusCode::OK,
            "Google Calendar connected. You can close this tab and return to AnimaOS.",
        ),
        Err(CalendarError::PairingNotFound) => callback_page(
            StatusCode::BAD_REQUEST,
            "This connection attempt expired. Start a new one from Connectors.",
        ),
        Err(_) => callback_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Google Calendar could not be connected right now. Try again from Connectors.",
        ),
    }
}

#[utoipa::path(
    delete,
    path = "/api/agents/{agent_id}/connectors/gcalendar/{connector_id}",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier")
    ),
    responses(
        (status = 200, description = "Google Calendar connector deleted", body = DeleteResponse),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody),
        (status = 503, description = "Vault or persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn delete_gcalendar_connector(
    State(state): State<AppState>,
    Path((agent_id, connector_id)): Path<(String, String)>,
    request: axum::extract::Request,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    match state.calendar.disconnect(&agent_id, &connector_id).await {
        Ok(()) => no_store(json_response(
            StatusCode::OK,
            &DeleteResponse { deleted: true },
        )),
        Err(error) => calendar_error(error),
    }
}

#[utoipa::path(
    get,
    path = "/api/agents/{agent_id}/connectors/gcalendar/{connector_id}/writes",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier")
    ),
    responses(
        (status = 200, description = "Pending and resolved calendar writes", body = CalendarWritesEnvelope),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody)
    )
)]
pub(super) async fn list_calendar_writes(
    State(state): State<AppState>,
    Path((agent_id, connector_id)): Path<(String, String)>,
) -> AxumResponse {
    match state.calendar.list_writes(&agent_id, &connector_id).await {
        Ok(writes) => no_store(json_response(
            StatusCode::OK,
            &CalendarWritesEnvelope {
                writes: writes
                    .iter()
                    .map(CalendarWriteResponse::from_record)
                    .collect(),
            },
        )),
        Err(error) => calendar_error(error),
    }
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/connectors/gcalendar/{connector_id}/writes/{write_id}/approve",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier"),
        ("write_id" = String, Path, description = "Pending write identifier")
    ),
    responses(
        (status = 200, description = "Calendar write applied or failed upstream", body = CalendarWriteEnvelope),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Write or connector not found", body = ConnectorErrorBody),
        (status = 409, description = "Write is no longer pending", body = ConnectorErrorBody),
        (status = 503, description = "Google, vault, or persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn approve_calendar_write(
    State(state): State<AppState>,
    Path((agent_id, connector_id, write_id)): Path<(String, String, String)>,
    request: axum::extract::Request,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    match state
        .calendar
        .approve_write(&agent_id, &connector_id, &write_id)
        .await
    {
        Ok(write) => no_store(json_response(
            StatusCode::OK,
            &CalendarWriteEnvelope {
                write: CalendarWriteResponse::from_record(&write),
            },
        )),
        Err(error) => calendar_error(error),
    }
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/connectors/gcalendar/{connector_id}/writes/{write_id}/reject",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier"),
        ("write_id" = String, Path, description = "Pending write identifier")
    ),
    responses(
        (status = 200, description = "Calendar write rejected", body = CalendarWriteEnvelope),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Write or connector not found", body = ConnectorErrorBody),
        (status = 409, description = "Write is no longer pending", body = ConnectorErrorBody)
    )
)]
pub(super) async fn reject_calendar_write(
    State(state): State<AppState>,
    Path((agent_id, connector_id, write_id)): Path<(String, String, String)>,
    request: axum::extract::Request,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    match state
        .calendar
        .reject_write(&agent_id, &connector_id, &write_id)
        .await
    {
        Ok(write) => no_store(json_response(
            StatusCode::OK,
            &CalendarWriteEnvelope {
                write: CalendarWriteResponse::from_record(&write),
            },
        )),
        Err(error) => calendar_error(error),
    }
}

fn callback_page(status: StatusCode, message: &str) -> AxumResponse {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut response = Html(format!(
        "<!doctype html><html><body style=\"font-family:system-ui;background:#0b0d10;color:#e6e9ee;display:flex;min-height:100vh;align-items:center;justify-content:center\"><p>{escaped}</p></body></html>"
    ))
    .into_response();
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn calendar_error(error: CalendarError) -> AxumResponse {
    match error {
        CalendarError::AgentNotFound | CalendarError::ConnectorNotFound => not_found_error(),
        CalendarError::AlreadyConnected => error_response(
            StatusCode::CONFLICT,
            "connector_already_exists",
            "agent already has an active Google Calendar connector",
        ),
        CalendarError::Unconfigured => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "calendar_oauth_unconfigured",
            "Google OAuth is not configured on this daemon",
        ),
        CalendarError::PairingNotFound => error_response(
            StatusCode::CONFLICT,
            "connector_pairing_not_found",
            "pairing attempt was not found or expired",
        ),
        CalendarError::NotConnected => error_response(
            StatusCode::CONFLICT,
            "calendar_not_connected",
            "Google Calendar is not connected for this agent",
        ),
        CalendarError::ReauthRequired => error_response(
            StatusCode::CONFLICT,
            "calendar_reauth_required",
            "Google Calendar access must be reauthorized",
        ),
        CalendarError::WriteNotPending => error_response(
            StatusCode::CONFLICT,
            "calendar_write_not_pending",
            "calendar write is no longer pending",
        ),
        CalendarError::InvalidDraft => error_response(
            StatusCode::BAD_REQUEST,
            "calendar_draft_invalid",
            "calendar event draft is invalid",
        ),
        CalendarError::Transport => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "connector_upstream_unavailable",
            "Google Calendar is unavailable",
        ),
        CalendarError::Credential => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "connector_credential_store_unavailable",
            "credential vault is unavailable",
        ),
        CalendarError::Persistence => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "connector_persistence_unavailable",
            "connector state persistence is unavailable",
        ),
        CalendarError::Conflict => error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "calendar_write_backlog",
            "too many pending calendar writes await approval",
        ),
    }
}

fn local_owner_error(rejection: LocalOwnerRejection) -> AxumResponse {
    match rejection {
        LocalOwnerRejection::LocalAdminRequired => error_response(
            StatusCode::FORBIDDEN,
            "connector_local_admin_required",
            "local connector administration authorization is required",
        ),
        LocalOwnerRejection::OriginRejected => error_response(
            StatusCode::FORBIDDEN,
            "connector_origin_rejected",
            "browser origin is not approved for connector administration",
        ),
    }
}

fn not_found_error() -> AxumResponse {
    error_response(
        StatusCode::NOT_FOUND,
        "connector_not_found",
        "connector was not found",
    )
}

fn error_response(status: StatusCode, code: &str, message: &str) -> AxumResponse {
    no_store(json_response(
        status,
        &ConnectorErrorBody {
            code: code.to_string(),
            error: message.to_string(),
        },
    ))
}

fn no_store(mut response: AxumResponse) -> AxumResponse {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
