//! Session runs (spec §4.2): accept a message into a session, list a
//! session's runs, and read one run.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use serde::Deserialize;
use utoipa::ToSchema;

use super::contracts::{ErrorBody, RunEnvelope, RunResponse, RunsEnvelope};
use super::http::{json_response, read_limited_body, request_query};
use super::jobs::{authorize, no_store};
use super::sessions::rejected;
use super::{parse_json_body, ApiError, AppState};
use crate::agent_runs::{
    AcceptRun, AcceptedRun, QueuedRunStart, SessionRunMode, SESSION_CANNOT_SEND,
};
use crate::runs::{RunSource, MAX_RUN_ATTACHMENTS, MAX_RUN_INPUT_TEXT_BYTES};
use crate::sessions::{is_valid_session_id, SessionKind};

const DEFAULT_RUN_PAGE: usize = 20;
const MAX_RUN_PAGE: usize = 50;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
/// Bodies this route reads, or the daemon-wide limit when that is larger: JSON
/// escapes a control character as six bytes (`\u0001`), so a 32 KiB text can
/// take 192 KiB, and the text check must answer, not the body limit (audit M29).
const MAX_RUN_REQUEST_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum StartRunMode {
    /// Wait behind the session's earlier messages.
    #[default]
    Queue,
    /// Join the session's active run before its next model call.
    Steer,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StartRunRequest {
    #[serde(default)]
    text: String,
    #[serde(default)]
    attachment_ids: Vec<String>,
    #[serde(default)]
    skill: Option<String>,
    #[serde(default)]
    mode: StartRunMode,
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let mut values = headers.get_all("idempotency-key").iter();
    let Some(value) = values.next() else {
        return Err(ApiError::bad_request_static(
            "Idempotency-Key header is required",
        ));
    };
    let single = values.next().is_none();
    match value.to_str() {
        Ok(key)
            if single
                && !key.is_empty()
                && key.len() <= MAX_IDEMPOTENCY_KEY_BYTES
                && key.bytes().all(|byte| matches!(byte, 0x21..=0x7e)) =>
        {
            Ok(key.to_string())
        }
        _ => Err(ApiError::bad_request_static(
            "Idempotency-Key header is invalid",
        )),
    }
}

async fn start_body(state: &AppState, request: Request) -> Result<StartRunRequest, Response> {
    let limit = state.config.max_request_bytes.max(MAX_RUN_REQUEST_BYTES);
    let bytes = read_limited_body(request, limit).await.map_err(no_store)?;
    parse_json_body(bytes).map_err(|error: ApiError| no_store(error.into_response()))
}

fn validate_input(input: &StartRunRequest) -> Result<(), ApiError> {
    if input.text.trim().is_empty() && input.attachment_ids.is_empty() {
        return Err(ApiError::bad_request_static(
            "text or attachments are required",
        ));
    }
    if input.text.len() > MAX_RUN_INPUT_TEXT_BYTES {
        return Err(ApiError::bad_request_static("text must be at most 32 KiB"));
    }
    if input.attachment_ids.len() > MAX_RUN_ATTACHMENTS {
        return Err(ApiError::bad_request_static(
            "at most 10 attachments are allowed per message",
        ));
    }
    if !input.attachment_ids.is_empty() {
        // Attachments arrive in M9; until then no id is known.
        return Err(ApiError::bad_request_static("unknown attachment ids"));
    }
    if input.skill.is_some() {
        // Skills arrive in M5.
        return Err(ApiError::bad_request_static("unknown skill"));
    }
    Ok(())
}

fn run_limit(uri: &Uri) -> Result<usize, ApiError> {
    let params =
        request_query(uri).map_err(|()| ApiError::bad_request_static("malformed query"))?;
    match params.get("limit").map(String::as_str) {
        None | Some("") => Ok(DEFAULT_RUN_PAGE),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_RUN_PAGE).contains(limit))
            .ok_or_else(|| {
                ApiError::bad_request(format!("limit must be between 1 and {MAX_RUN_PAGE}"))
            }),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/sessions/{session_id}/runs", tag = "runs",
    params(
        ("agent_id" = String, Path),
        ("session_id" = String, Path, description = "Percent-encoded session id"),
        ("Idempotency-Key" = String, Header, description = "1–128 visible ASCII characters. The same key within 24 hours returns the original run and creates nothing, but only while its run is still in the ledger: at most 24 hours and, once the history store holds it, among the agent's newest 50 finished runs")
    ),
    request_body = StartRunRequest,
    responses(
        (status = 200, description = "The key was used for this message within 24 hours: the original run, nothing created", body = RunEnvelope),
        (status = 202, description = "Accepted: the queued run", body = RunEnvelope),
        (status = 400, description = "Missing or invalid key, empty text with no attachments, text over 32 KiB, over 10 or unknown attachments, unknown skill, steer on a kind that cannot steer, or a body over 256 KiB", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 409, description = "The kind cannot receive messages (a job or helper session, or a Telegram session without its connector), the agent is a helper, or the key was used for a different message", body = ErrorBody),
        (status = 429, description = "Eight messages are already waiting for this companion", body = ErrorBody),
        (status = 503, description = "The control plane could not be saved", body = ErrorBody)
    ))]
pub(super) async fn start_session_run(
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
    let idempotency_key = match idempotency_key(request.headers()) {
        Ok(key) => key,
        Err(error) => return rejected(error),
    };
    let input = match start_body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    if let Err(error) = validate_input(&input) {
        return rejected(error);
    }
    // Which flow runs the message; `accept_run` re-checks the session under
    // the control-plane transaction.
    let target = {
        let guard = state.daemon.read().await;
        guard.sessions.get(&agent_id, &session_id).map(|record| {
            let connector = (record.kind == SessionKind::Telegram).then(|| {
                guard
                    .connectors
                    .values()
                    .find(|connector| {
                        connector.is_active()
                            && connector.agent_id == agent_id
                            && connector.room_id == record.room_id()
                    })
                    .map(|connector| connector.id.clone())
            });
            (record.room_id().to_string(), connector)
        })
    };
    let Some((room_id, connector)) = target else {
        return rejected(ApiError::not_found());
    };
    let (source, source_ref, start): (RunSource, Option<String>, QueuedRunStart) = match connector {
        // A Telegram session's message is the connector's owner turn (spec §4.2).
        Some(Some(connector_id)) => {
            let manager = state.connector_manager.clone();
            let (agent, text, key, connector) = (
                agent_id.clone(),
                input.text.clone(),
                idempotency_key.clone(),
                connector_id.clone(),
            );
            let start: QueuedRunStart =
                Box::new(move |run_id| -> BoxFuture<'static, Result<(), String>> {
                    Box::pin(async move {
                        manager
                            .send_from_owner_accepted(agent, connector, text, key, run_id)
                            .await
                            .map_err(|error| error.to_string())
                    })
                });
            (RunSource::Telegram, Some(connector_id), start)
        }
        Some(None) => return rejected(ApiError::conflict(SESSION_CANNOT_SEND)),
        None => (
            RunSource::Web,
            None,
            state.agent_runs.web_start(
                agent_id.clone(),
                room_id,
                input.text.clone(),
                idempotency_key.clone(),
            ),
        ),
    };
    let mode = match input.mode {
        StartRunMode::Queue => SessionRunMode::Queue,
        StartRunMode::Steer => SessionRunMode::Steer,
    };
    let accepted = state
        .agent_runs
        .accept_run(
            AcceptRun {
                agent_id,
                session_id,
                text: input.text,
                idempotency_key,
                mode,
                source,
                source_ref,
            },
            start,
        )
        .await;
    match accepted {
        Ok(AcceptedRun::Created(record)) => no_store(json_response(
            StatusCode::ACCEPTED,
            &RunEnvelope::of(&record),
        )),
        Ok(AcceptedRun::Replayed(record)) => {
            no_store(json_response(StatusCode::OK, &RunEnvelope::of(&record)))
        }
        Err(error) => rejected(error),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions/{session_id}/runs", tag = "runs",
    params(
        ("agent_id" = String, Path),
        ("session_id" = String, Path, description = "Percent-encoded session id"),
        ("limit" = Option<usize>, Query, description = "1–50, default 20")
    ),
    responses(
        (status = 200, description = "The session's runs the ledger holds, newest first", body = RunsEnvelope),
        (status = 400, description = "Invalid limit", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody)
    ))]
pub(super) async fn list_session_runs(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let limit = match run_limit(request.uri()) {
        Ok(limit) => limit,
        Err(error) => return rejected(error),
    };
    let guard = state.daemon.read().await;
    if !guard.agents.contains_key(&agent_id) || guard.sessions.get(&agent_id, &session_id).is_none()
    {
        return rejected(ApiError::not_found());
    }
    let runs = guard
        .runs
        .for_session(&agent_id, &session_id)
        .into_iter()
        .take(limit)
        .map(|record| RunResponse::from(&guard.with_live_tools(record.clone())))
        .collect();
    no_store(json_response(StatusCode::OK, &RunsEnvelope { runs }))
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/runs/{run_id}", tag = "runs",
    params(("agent_id" = String, Path), ("run_id" = String, Path)),
    responses(
        (status = 200, description = "The run, from the ledger or, once pruned, the history store", body = RunEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or run not found", body = ErrorBody),
        (status = 503, description = "The history store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn get_run(
    State(state): State<AppState>,
    Path((agent_id, run_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let (known, history) = {
        let guard = state.daemon.read().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        (
            guard
                .runs
                .get(&run_id)
                .filter(|record| record.agent_id == agent_id)
                .map(|record| guard.with_live_tools(record.clone())),
            guard.history.clone(),
        )
    };
    let record = match known {
        Some(record) => record,
        // Read outside the state lock (M2 lock rule).
        None => match history.store().get_run(&run_id).await {
            Ok(Some(record)) if record.agent_id == agent_id => record,
            Ok(_) => return rejected(ApiError::not_found()),
            Err(error) => return rejected(ApiError::service_unavailable(error.message())),
        },
    };
    no_store(json_response(StatusCode::OK, &RunEnvelope::of(&record)))
}
