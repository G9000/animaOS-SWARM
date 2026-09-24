//! Session routes (spec §3.3). Every answer is owner-authorized and `no-store`.

use std::collections::HashMap;

use anima_core::primitives::now_millis;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tracing::warn;

use super::contracts::{
    DeleteResponse, ErrorBody, SessionCreateRequest, SessionEnvelope, SessionMessagesEnvelope,
    SessionResponse, SessionUpdateRequest, SessionsEnvelope,
};
use super::http::{json_response, read_limited_body, request_query};
use super::jobs::{authorize, body, no_store};
use super::{parse_json_body, ApiError, AppState};
use crate::history::HistoryDeletion;
use crate::sessions::views::{
    self, MessagePageError, MessagePageRequest, SessionCursor, SessionListQuery,
    DEFAULT_MESSAGE_PAGE, DEFAULT_SESSION_PAGE, MAX_MESSAGE_PAGE, MAX_SEARCH_QUERY_CHARS,
    MAX_SESSION_PAGE,
};
use crate::sessions::{
    clean_owner_title, is_valid_session_id, new_chat_session_id, SessionKind, SessionOrigin,
    SessionRecord, TitleSource, DEFAULT_CHAT_TITLE,
};

const TOO_MANY_NEW_CHATS: &str = "Too many new chats; try again in a minute";
const SESSION_RUN_IN_PROGRESS: &str = "A run in this session is still in progress";
/// Helpers have no chat of their own (spec §3.1); they only run through the
/// companion that delegates to them.
const HELPER_SESSION_CONFLICT: &str = "Helpers must run through their owning companion";

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

async fn session_response(
    state: &AppState,
    agent_id: &str,
    session_id: &str,
    status: StatusCode,
) -> Response {
    match views::session_view(&state.daemon, agent_id, session_id).await {
        Some(view) => no_store(json_response(
            status,
            &SessionEnvelope {
                session: SessionResponse::from(&view),
            },
        )),
        None => rejected(ApiError::not_found()),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/sessions", tag = "sessions",
    params(("agent_id" = String, Path)), request_body = SessionCreateRequest,
    responses(
        (status = 201, description = "A new chat session", body = SessionEnvelope),
        (status = 400, description = "Invalid title", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody),
        (status = 409, description = "The agent is a helper; helpers run only through their companion", body = ErrorBody),
        (status = 429, description = "More than 60 new sessions this minute", body = ErrorBody),
        (status = 503, description = "The control plane could not be saved", body = ErrorBody)
    ))]
pub(super) async fn create_session(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let bytes = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(bytes) => bytes,
        Err(response) => return no_store(response),
    };
    let input = if bytes.iter().all(u8::is_ascii_whitespace) {
        SessionCreateRequest::default()
    } else {
        match parse_json_body::<SessionCreateRequest>(bytes) {
            Ok(input) => input,
            Err(error) => return rejected(error),
        }
    };
    let owner_title = match input.title.as_deref().map(clean_owner_title).transpose() {
        Ok(title) => title,
        Err(message) => return rejected(ApiError::bad_request_static(message)),
    };
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (session_id, persist) = {
        let mut guard = state.daemon.write().await;
        match guard.agents.get(&agent_id) {
            None => return rejected(ApiError::not_found()),
            // Controller ruling 1 (M2 pre-flight audit): helpers run only
            // through the companion that delegates to them.
            Some(runtime) if crate::agent_runs::is_helper_config(runtime.config()) => {
                return rejected(ApiError::conflict(HELPER_SESSION_CONFLICT));
            }
            Some(_) => {}
        }
        let now_ms = now_millis();
        if !guard.session_limiter.try_acquire(&agent_id, now_ms) {
            return rejected(ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                message: TOO_MANY_NEW_CHATS.into(),
            });
        }
        let (title, title_source) = match owner_title {
            Some(title) => (title, TitleSource::Owner),
            None => (DEFAULT_CHAT_TITLE.to_string(), TitleSource::FirstMessage),
        };
        let record = SessionRecord::new(
            &agent_id,
            &new_chat_session_id(),
            SessionKind::Chat,
            SessionOrigin::Web,
            title,
            title_source,
            now_ms,
        );
        let session_id = record.id.clone();
        guard.sessions.insert(record);
        (session_id, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state
            .daemon
            .write()
            .await
            .sessions
            .remove(&agent_id, &session_id);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    session_response(&state, &agent_id, &session_id, StatusCode::CREATED).await
}

#[utoipa::path(patch, path = "/api/agents/{agent_id}/sessions/{session_id}", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    request_body = SessionUpdateRequest,
    responses(
        (status = 200, description = "The updated session", body = SessionEnvelope),
        (status = 400, description = "Invalid or empty update", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 409, description = "This session cannot be renamed", body = ErrorBody),
        (status = 503, description = "The control plane could not be saved", body = ErrorBody)
    ))]
pub(super) async fn update_session(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    let input: SessionUpdateRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    if input.title.is_none() && input.archived.is_none() && input.last_read_at_ms.is_none() {
        return rejected(ApiError::bad_request_static(
            "at least one of title, archived, or lastReadAtMs is required",
        ));
    }
    let title = match input.title.as_deref().map(clean_owner_title).transpose() {
        Ok(title) => title,
        Err(message) => return rejected(ApiError::bad_request_static(message)),
    };
    if input
        .last_read_at_ms
        .is_some_and(|read_at| read_at > now_millis())
    {
        return rejected(ApiError::bad_request_static(
            "lastReadAtMs must not be in the future",
        ));
    }
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (previous, persist) = {
        let mut guard = state.daemon.write().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        let schedule_exists = guard
            .sessions
            .get(&agent_id, &session_id)
            .is_some_and(|record| views::automation_exists(&guard, record));
        let Some(record) = guard.sessions.get_mut(&agent_id, &session_id) else {
            return rejected(ApiError::not_found());
        };
        if title.is_some() && !record.capabilities(schedule_exists).rename {
            return rejected(ApiError::conflict("This session cannot be renamed"));
        }
        let previous = record.clone();
        if let Some(title) = title {
            record.title = title;
            record.title_source = TitleSource::Owner;
        }
        if let Some(archived) = input.archived {
            record.archived = archived;
        }
        if let Some(read_at) = input.last_read_at_ms {
            record.last_read_at_ms = Some(read_at);
        }
        (previous, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state.daemon.write().await.sessions.insert(previous);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    session_response(&state, &agent_id, &session_id, StatusCode::OK).await
}

#[utoipa::path(delete, path = "/api/agents/{agent_id}/sessions/{session_id}", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    responses(
        (status = 200, description = "Deleted: the record, hot and mirrored messages, and attachment records; memories are kept", body = DeleteResponse),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 409, description = "The kind does not allow deletion, or a run in the session is active", body = ErrorBody),
        (status = 503, description = "The control plane could not be saved", body = ErrorBody)
    ))]
pub(super) async fn delete_session(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    let room_id = {
        let guard = state.daemon.read().await;
        let record = match guard.sessions.get(&agent_id, &session_id) {
            Some(record) if guard.agents.contains_key(&agent_id) => record,
            _ => return rejected(ApiError::not_found()),
        };
        if !record
            .capabilities(views::automation_exists(&guard, record))
            .delete
        {
            return rejected(ApiError::conflict("This session cannot be deleted"));
        }
        record.room_id().to_string()
    };
    // Runs for this room wait behind the reservation, so none starts meanwhile.
    let Some(reservation) = state.agent_runs.try_reserve_room(&agent_id, &room_id) else {
        return rejected(ApiError::conflict(SESSION_RUN_IN_PROGRESS));
    };
    let transaction = state.agent_runs.control_plane_transaction().await;
    // Recorded in the same save as the removal below (Controller ruling 2, M2
    // pre-flight audit; Task 6's durable-deletion protocol): a crash right
    // after a successful save still replays this deletion on restart.
    let deletion = HistoryDeletion::session(&agent_id, &session_id);
    let (previous_agent, record, removed_runs, removed_ids, persist) = {
        let mut guard = state.daemon.write().await;
        if guard.runs.active_count_for_session(&agent_id, &session_id) > 0 {
            return rejected(ApiError::conflict(SESSION_RUN_IN_PROGRESS));
        }
        let Some(runtime) = guard.agents.get_mut(&agent_id) else {
            return rejected(ApiError::not_found());
        };
        let previous_agent = runtime.snapshot();
        let removed_ids = runtime
            .retain_messages(|message| message.room_id != room_id)
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        let Some(record) = guard.sessions.remove(&agent_id, &session_id) else {
            if let Err(error) = guard.restore_removed_agent(previous_agent) {
                warn!(agent_id = %agent_id, error = %error, "could not restore an agent after a session vanished");
            }
            return rejected(ApiError::not_found());
        };
        let removed_runs = guard
            .runs
            .remove_terminal_for_session(&agent_id, &session_id);
        guard.record_history_deletion(deletion.clone());
        (
            previous_agent,
            record,
            removed_runs,
            removed_ids,
            guard.control_plane_persist_request(),
        )
    };
    if let Err(error) = persist.save().await {
        let mut guard = state.daemon.write().await;
        if let Err(restore_error) = guard.restore_removed_agent(previous_agent) {
            warn!(agent_id = %agent_id, error = %restore_error, "could not restore an agent after a failed session delete");
        }
        guard.sessions.insert(record);
        for run in removed_runs {
            guard.runs.insert(run);
        }
        guard.clear_history_deletion(&deletion);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    // Durable now: the history rows may go (spec §3.3).
    let history = state.daemon.read().await.history.clone();
    history.enqueue_session_deletion(&agent_id, &session_id);
    history.forget_mirrored(removed_ids.iter().map(String::as_str));
    drop(transaction);
    drop(reservation);
    no_store(json_response(
        StatusCode::OK,
        &DeleteResponse { deleted: true },
    ))
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions/{session_id}/export", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    responses(
        (status = 200, description = "The visible transcript, including messages kept only in the history store", body = String, content_type = "text/markdown"),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 503, description = "The history store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn export_session(
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
    match views::full_transcript(&state.daemon, &agent_id, &session_id).await {
        Ok((record, agent_name, messages)) => {
            let markdown = views::transcript_markdown(&record, &agent_name, &messages);
            let mut response = (StatusCode::OK, markdown).into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/markdown; charset=utf-8"),
            );
            if let Ok(disposition) = HeaderValue::from_str(&format!(
                "attachment; filename=\"{}.md\"",
                views::export_file_stem(&record.title)
            )) {
                headers.insert(header::CONTENT_DISPOSITION, disposition);
            }
            no_store(response)
        }
        Err(MessagePageError::Unavailable) => rejected(ApiError::service_unavailable(
            "history store is unavailable",
        )),
        Err(MessagePageError::NotFound | MessagePageError::BeforeNotFound) => {
            rejected(ApiError::not_found())
        }
    }
}
