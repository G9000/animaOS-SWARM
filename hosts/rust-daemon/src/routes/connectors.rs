use anima_core::Message;
use axum::extract::{Path, Request as AxumRequest, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::Response as AxumResponse;

use crate::connectors::credentials::TelegramBotToken;
use crate::connectors::runtime::{ConnectorManagerError, ConnectorRuntimeStatus};
use crate::connectors::TelegramConnectorRecord;

use super::contracts::{
    ConnectorErrorBody, ConnectorMessagePageQuery, ConnectorMessageRequest,
    ConnectorMessageResponse, ConnectorMessageSendResponse, ConnectorMessagesEnvelope,
    DeleteResponse, TelegramConnectorEnvelope, TelegramConnectorResponse,
    TelegramConnectorsEnvelope, TelegramCredentialRequest,
};
use super::http::{json_response, read_limited_body, request_query};
use super::{parse_json_body, AppState};

#[utoipa::path(
    get,
    path = "/api/agents/{agent_id}/connectors",
    tag = "connectors",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    responses(
        (status = 200, description = "Safe connector summaries", body = TelegramConnectorsEnvelope),
        (status = 404, description = "Agent not found", body = ConnectorErrorBody)
    )
)]
pub(super) async fn list_connectors(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> AxumResponse {
    let records = {
        let daemon = state.daemon.read().await;
        if daemon.get_agent(&agent_id).is_none() {
            return error_response(StatusCode::NOT_FOUND, "not_found", "not found");
        }
        let mut records = daemon
            .connectors
            .values()
            .filter(|connector| connector.agent_id == agent_id && connector.deleted_at_ms.is_none())
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        records
    };
    let mut connectors = Vec::with_capacity(records.len());
    for record in records {
        connectors.push(connector_response(&state, &record).await);
    }
    no_store(json_response(
        StatusCode::OK,
        &TelegramConnectorsEnvelope { connectors },
    ))
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/connectors/telegram",
    tag = "connectors",
    params(("agent_id" = String, Path, description = "Agent identifier")),
    request_body = TelegramCredentialRequest,
    responses(
        (status = 201, description = "Telegram connector created", body = TelegramConnectorEnvelope),
        (status = 400, description = "Invalid credential request", body = ConnectorErrorBody),
        (status = 422, description = "Telegram rejected the bot token", body = ConnectorErrorBody),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Agent not found", body = ConnectorErrorBody),
        (status = 409, description = "Agent already has a Telegram connector", body = ConnectorErrorBody),
        (status = 503, description = "Telegram, vault, or persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn create_telegram_connector(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if !state.local_owner.authorize(request.headers()) {
        return local_owner_error();
    }
    let token = match credential_from_request(request, &state).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    match state.connector_manager.create(agent_id, token).await {
        Ok(record) => {
            let connector = connector_response(&state, &record).await;
            no_store(json_response(
                StatusCode::CREATED,
                &TelegramConnectorEnvelope { connector },
            ))
        }
        Err(error) => manager_error(error),
    }
}

#[utoipa::path(
    put,
    path = "/api/agents/{agent_id}/connectors/{connector_id}/credential",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier")
    ),
    request_body = TelegramCredentialRequest,
    responses(
        (status = 200, description = "Telegram credential replaced", body = TelegramConnectorEnvelope),
        (status = 400, description = "Invalid credential request", body = ConnectorErrorBody),
        (status = 422, description = "Telegram rejected the bot token", body = ConnectorErrorBody),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody),
        (status = 503, description = "Telegram, vault, or persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn replace_telegram_credential(
    State(state): State<AppState>,
    Path((agent_id, connector_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if !state.local_owner.authorize(request.headers()) {
        return local_owner_error();
    }
    let token = match credential_from_request(request, &state).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    if owned_connector(&state, &agent_id, &connector_id)
        .await
        .is_err()
    {
        return not_found_error();
    }
    match state
        .connector_manager
        .replace_token(connector_id, token)
        .await
    {
        Ok(record) => connector_envelope(&state, &record).await,
        Err(error) => manager_error(error),
    }
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/connectors/{connector_id}/pairings/{chat_id}/approve",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier"),
        ("chat_id" = String, Path, description = "Pending Telegram chat identifier")
    ),
    responses(
        (status = 200, description = "Pending chat approved", body = TelegramConnectorEnvelope),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody),
        (status = 409, description = "Pairing candidate missing or changed", body = ConnectorErrorBody),
        (status = 503, description = "Persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn approve_telegram_pairing(
    State(state): State<AppState>,
    Path((agent_id, connector_id, chat_id)): Path<(String, String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if !state.local_owner.authorize(request.headers()) {
        return local_owner_error();
    }
    if chat_id.is_empty() || chat_id.len() > 64 {
        return invalid_request("chat identifier is invalid");
    }
    if owned_connector(&state, &agent_id, &connector_id)
        .await
        .is_err()
    {
        return not_found_error();
    }
    match state
        .connector_manager
        .approve_pending_chat(connector_id, Some(chat_id))
        .await
    {
        Ok(record) => connector_envelope(&state, &record).await,
        Err(error) => manager_error(error),
    }
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/connectors/{connector_id}/restart",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier")
    ),
    responses(
        (status = 200, description = "Connector worker restarted", body = TelegramConnectorEnvelope),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody),
        (status = 503, description = "Vault or worker unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn restart_telegram_connector(
    State(state): State<AppState>,
    Path((agent_id, connector_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if !state.local_owner.authorize(request.headers()) {
        return local_owner_error();
    }
    if owned_connector(&state, &agent_id, &connector_id)
        .await
        .is_err()
    {
        return not_found_error();
    }
    match state.connector_manager.restart(connector_id.clone()).await {
        Ok(()) => match owned_connector(&state, &agent_id, &connector_id).await {
            Ok(record) => connector_envelope(&state, &record).await,
            Err(()) => not_found_error(),
        },
        Err(error) => manager_error(error),
    }
}

#[utoipa::path(
    delete,
    path = "/api/agents/{agent_id}/connectors/{connector_id}",
    tag = "connectors",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier")
    ),
    responses(
        (status = 200, description = "Connector deleted", body = DeleteResponse),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody),
        (status = 503, description = "Vault or persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn delete_telegram_connector(
    State(state): State<AppState>,
    Path((agent_id, connector_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if !state.local_owner.authorize(request.headers()) {
        return local_owner_error();
    }
    if owned_connector(&state, &agent_id, &connector_id)
        .await
        .is_err()
    {
        return not_found_error();
    }
    match state.connector_manager.delete(connector_id).await {
        Ok(()) => no_store(json_response(
            StatusCode::OK,
            &DeleteResponse { deleted: true },
        )),
        Err(error) => manager_error(error),
    }
}

#[utoipa::path(
    get,
    path = "/api/agents/{agent_id}/connectors/{connector_id}/messages",
    tag = "connector-thread",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier"),
        ConnectorMessagePageQuery
    ),
    responses(
        (status = 200, description = "Paginated connector-room messages", body = ConnectorMessagesEnvelope),
        (status = 400, description = "Invalid page cursor or limit", body = ConnectorErrorBody),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody)
    )
)]
pub(super) async fn list_connector_messages(
    State(state): State<AppState>,
    Path((agent_id, connector_id)): Path<(String, String)>,
    uri: Uri,
) -> AxumResponse {
    let connector = match owned_connector(&state, &agent_id, &connector_id).await {
        Ok(connector) => connector,
        Err(()) => return not_found_error(),
    };
    let query = match request_query(&uri)
        .map_err(|_| "malformed query")
        .and_then(|query| ConnectorMessagePageQuery::from_query_map(&query))
        .and_then(ConnectorMessagePageQuery::validated)
    {
        Ok(query) => query,
        Err(message) => return invalid_request(message),
    };
    let snapshot = match state.daemon.read().await.get_agent(&agent_id) {
        Some(snapshot) => snapshot,
        None => return not_found_error(),
    };
    let messages = snapshot
        .messages
        .into_iter()
        .filter(|message| message.room_id == connector.room_id)
        .collect::<Vec<_>>();
    let page = match paginate_messages(messages, query.0.as_deref(), query.1) {
        Ok(page) => page,
        Err(()) => return invalid_request("before message was not found"),
    };
    no_store(json_response(StatusCode::OK, &page))
}

#[utoipa::path(
    post,
    path = "/api/agents/{agent_id}/connectors/{connector_id}/messages",
    tag = "connector-thread",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("connector_id" = String, Path, description = "Connector identifier")
    ),
    request_body = ConnectorMessageRequest,
    responses(
        (status = 200, description = "Agent turn committed with optional Telegram delivery", body = ConnectorMessageSendResponse),
        (status = 400, description = "Invalid text", body = ConnectorErrorBody),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 404, description = "Connector not found for agent", body = ConnectorErrorBody),
        (status = 429, description = "Agent or connector is busy", body = ConnectorErrorBody),
        (status = 503, description = "Agent run or persistence unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn send_connector_message(
    State(state): State<AppState>,
    Path((agent_id, connector_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if !state.local_owner.authorize(request.headers()) {
        return local_owner_error();
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid_request("malformed request"),
    };
    let text = match parse_json_body::<ConnectorMessageRequest>(body).and_then(|request| {
        request
            .into_text()
            .map_err(super::ApiError::bad_request_static)
    }) {
        Ok(text) => text,
        Err(_) => return invalid_request("request body is invalid"),
    };
    let room_id = match owned_connector(&state, &agent_id, &connector_id).await {
        Ok(connector) => connector.room_id,
        Err(()) => return not_found_error(),
    };
    match state
        .connector_manager
        .send_from_owner(agent_id, connector_id, text)
        .await
    {
        Ok((run, delivery_queued)) => no_store(json_response(
            StatusCode::OK,
            &ConnectorMessageSendResponse::from_run(run, &room_id, delivery_queued),
        )),
        Err(error) => manager_error(error),
    }
}

async fn credential_from_request(
    request: AxumRequest,
    state: &AppState,
) -> Result<TelegramBotToken, AxumResponse> {
    let body = read_limited_body(request, state.config.max_request_bytes)
        .await
        .map_err(|_| invalid_request("malformed request"))?;
    let value = parse_json_body::<TelegramCredentialRequest>(body)
        .map_err(|_| invalid_request("request body is invalid"))?
        .into_token()
        .map_err(|_| invalid_token())?;
    TelegramBotToken::parse(value).map_err(|_| invalid_token())
}

async fn owned_connector(
    state: &AppState,
    agent_id: &str,
    connector_id: &str,
) -> Result<TelegramConnectorRecord, ()> {
    let daemon = state.daemon.read().await;
    if daemon.get_agent(agent_id).is_none() {
        return Err(());
    }
    daemon
        .connectors
        .get(connector_id)
        .filter(|connector| connector.agent_id == agent_id && connector.deleted_at_ms.is_none())
        .cloned()
        .ok_or(())
}

async fn connector_response(
    state: &AppState,
    record: &TelegramConnectorRecord,
) -> TelegramConnectorResponse {
    let status = state
        .connector_manager
        .status(&record.id)
        .await
        .unwrap_or_else(|| {
            if record.approved_chat.is_some() {
                ConnectorRuntimeStatus::Ready
            } else {
                ConnectorRuntimeStatus::Pairing
            }
        });
    TelegramConnectorResponse::from_record(record, status)
}

async fn connector_envelope(state: &AppState, record: &TelegramConnectorRecord) -> AxumResponse {
    let connector = connector_response(state, record).await;
    no_store(json_response(
        StatusCode::OK,
        &TelegramConnectorEnvelope { connector },
    ))
}

fn paginate_messages(
    messages: Vec<Message>,
    before: Option<&str>,
    limit: usize,
) -> Result<ConnectorMessagesEnvelope, ()> {
    let end = match before {
        Some(before) => messages
            .iter()
            .position(|message| message.id == before)
            .ok_or(())?,
        None => messages.len(),
    };
    let start = end.saturating_sub(limit);
    let page = messages[start..end]
        .iter()
        .map(ConnectorMessageResponse::from)
        .collect::<Vec<_>>();
    let next_before = (start > 0)
        .then(|| page.first().map(|message| message.id.clone()))
        .flatten();
    Ok(ConnectorMessagesEnvelope {
        messages: page,
        next_before,
    })
}

fn manager_error(error: ConnectorManagerError) -> AxumResponse {
    match error {
        ConnectorManagerError::AgentNotFound | ConnectorManagerError::ConnectorNotFound => {
            not_found_error()
        }
        ConnectorManagerError::AgentAlreadyConnected => error_response(
            StatusCode::CONFLICT,
            "connector_already_exists",
            "agent already has an active Telegram connector",
        ),
        ConnectorManagerError::PairingNotFound => error_response(
            StatusCode::CONFLICT,
            "connector_pairing_not_found",
            "pairing candidate was not found",
        ),
        ConnectorManagerError::InvalidToken => invalid_token(),
        ConnectorManagerError::Transport => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "telegram_unavailable",
            "Telegram is unavailable",
        ),
        ConnectorManagerError::Credential => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "credential_unavailable",
            "credential vault is unavailable",
        ),
        ConnectorManagerError::CredentialStateUncertain => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "credential_reconciliation_required",
            "credential state requires reconciliation",
        ),
        ConnectorManagerError::Persistence => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "persistence_unavailable",
            "connector state persistence is unavailable",
        ),
        ConnectorManagerError::Backpressure => error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "connector is busy",
        ),
        ConnectorManagerError::ConflictingUpdate => error_response(
            StatusCode::CONFLICT,
            "connector_conflict",
            "connector state changed",
        ),
        ConnectorManagerError::WorkerStopped => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "connector_unavailable",
            "connector worker is unavailable",
        ),
    }
}

fn invalid_request(message: &str) -> AxumResponse {
    error_response(StatusCode::BAD_REQUEST, "invalid_request", message)
}

fn invalid_token() -> AxumResponse {
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "connector_token_invalid",
        "Telegram bot token is invalid",
    )
}

fn local_owner_error() -> AxumResponse {
    error_response(
        StatusCode::FORBIDDEN,
        "local_owner_required",
        "local owner authorization required",
    )
}

fn not_found_error() -> AxumResponse {
    error_response(StatusCode::NOT_FOUND, "not_found", "not found")
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
