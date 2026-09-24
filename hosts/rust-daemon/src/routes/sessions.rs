//! Session routes (spec §3.3). Every answer is owner-authorized and `no-store`.

use std::collections::HashMap;

use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use super::contracts::{
    ErrorBody, SessionEnvelope, SessionMessagesEnvelope, SessionResponse, SessionsEnvelope,
};
use super::http::{json_response, request_query};
use super::jobs::{authorize, no_store};
use super::{ApiError, AppState};
use crate::sessions::views::{
    self, MessagePageError, MessagePageRequest, SessionCursor, SessionListQuery,
    DEFAULT_MESSAGE_PAGE, DEFAULT_SESSION_PAGE, MAX_MESSAGE_PAGE, MAX_SEARCH_QUERY_CHARS,
    MAX_SESSION_PAGE,
};
use crate::sessions::{is_valid_session_id, SessionKind};

type QueryParams = HashMap<String, String>;

pub(super) fn rejected(error: ApiError) -> Response {
    no_store(error.into_response())
}

fn params(uri: &Uri) -> Result<QueryParams, ApiError> {
    request_query(uri).map_err(|()| ApiError::bad_request_static("malformed query"))
}

fn flag(params: &QueryParams, name: &str, default: bool) -> Result<bool, ApiError> {
    match params.get(name).map(String::as_str) {
        None | Some("") => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(_) => Err(ApiError::bad_request(format!(
            "{name} must be true or false"
        ))),
    }
}

fn limit(params: &QueryParams, default: usize, max: usize) -> Result<usize, ApiError> {
    match params.get("limit").map(String::as_str) {
        None | Some("") => Ok(default),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=max).contains(limit))
            .ok_or_else(|| ApiError::bad_request(format!("limit must be between 1 and {max}"))),
    }
}

fn list_query(uri: &Uri) -> Result<SessionListQuery, ApiError> {
    let params = params(uri)?;
    let kind = match params.get("kind").map(String::as_str) {
        None | Some("") => None,
        Some(value) => Some(SessionKind::parse(value).ok_or_else(|| {
            ApiError::bad_request_static("kind must be one of chat, telegram, checkin, job, helper")
        })?),
    };
    let q = params
        .get("q")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if q.as_ref()
        .is_some_and(|q| q.chars().count() > MAX_SEARCH_QUERY_CHARS)
    {
        return Err(ApiError::bad_request(format!(
            "q must be at most {MAX_SEARCH_QUERY_CHARS} characters"
        )));
    }
    let cursor = match params.get("cursor").map(String::as_str) {
        None | Some("") => None,
        Some(value) => Some(
            SessionCursor::decode(value)
                .ok_or_else(|| ApiError::bad_request_static("cursor is invalid"))?,
        ),
    };
    Ok(SessionListQuery {
        kind,
        archived: flag(&params, "archived", false)?,
        q,
        cursor,
        limit: limit(&params, DEFAULT_SESSION_PAGE, MAX_SESSION_PAGE)?,
        include_helpers: flag(&params, "includeHelpers", true)?,
    })
}

fn message_query(uri: &Uri) -> Result<MessagePageRequest, ApiError> {
    let params = params(uri)?;
    Ok(MessagePageRequest {
        before: params
            .get("before")
            .filter(|value| !value.is_empty())
            .cloned(),
        limit: limit(&params, DEFAULT_MESSAGE_PAGE, MAX_MESSAGE_PAGE)?,
        include_hidden: flag(&params, "includeHidden", false)?,
    })
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions", tag = "sessions",
    params(
        ("agent_id" = String, Path),
        ("kind" = Option<String>, Query, description = "chat, telegram, checkin, job, or helper"),
        ("archived" = Option<bool>, Query, description = "true lists only archived sessions (default false)"),
        ("q" = Option<String>, Query, description = "Search titles and message text (up to 200 characters)"),
        ("cursor" = Option<String>, Query, description = "nextCursor of the previous page"),
        ("limit" = Option<usize>, Query, description = "1-200, default 50"),
        ("includeHelpers" = Option<bool>, Query, description = "Include helper and delegated sessions whose parentAgentId is this agent (default true)")
    ),
    responses(
        (status = 200, description = "Sessions, newest activity first", body = SessionsEnvelope),
        (status = 400, description = "Invalid query", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody)
    ))]
pub(super) async fn list_sessions(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let query = match list_query(request.uri()) {
        Ok(query) => query,
        Err(error) => return rejected(error),
    };
    match views::list_sessions(&state.daemon, &agent_id, &query).await {
        Some(page) => no_store(json_response(
            StatusCode::OK,
            &SessionsEnvelope::from(&page),
        )),
        None => rejected(ApiError::not_found()),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions/{session_id}", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    responses(
        (status = 200, description = "One session", body = SessionEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody)
    ))]
pub(super) async fn get_session(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    match views::session_view(&state.daemon, &agent_id, &session_id).await {
        Some(view) => no_store(json_response(
            StatusCode::OK,
            &SessionEnvelope {
                session: SessionResponse::from(&view),
            },
        )),
        None => rejected(ApiError::not_found()),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions/{session_id}/messages", tag = "sessions",
    params(
        ("agent_id" = String, Path),
        ("session_id" = String, Path, description = "Percent-encoded session id"),
        ("before" = Option<String>, Query, description = "Only messages older than this message id"),
        ("limit" = Option<usize>, Query, description = "1-200, default 50"),
        ("includeHidden" = Option<bool>, Query, description = "Include silent check-in turns (default false)")
    ),
    responses(
        (status = 200, description = "Messages oldest to newest within the page", body = SessionMessagesEnvelope),
        (status = 400, description = "Invalid query or unknown before message", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 503, description = "The page needs the history store and it cannot be read", body = ErrorBody)
    ))]
pub(super) async fn list_session_messages(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    let query = match message_query(request.uri()) {
        Ok(query) => query,
        Err(error) => return rejected(error),
    };
    match views::session_messages(&state.daemon, &agent_id, &session_id, &query).await {
        Ok(page) => no_store(json_response(
            StatusCode::OK,
            &SessionMessagesEnvelope::from(&page),
        )),
        Err(MessagePageError::NotFound) => rejected(ApiError::not_found()),
        Err(MessagePageError::BeforeNotFound) => {
            rejected(ApiError::bad_request_static("before message was not found"))
        }
        Err(MessagePageError::Unavailable) => rejected(ApiError::service_unavailable(
            "history store is unavailable",
        )),
    }
}
