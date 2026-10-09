//! The log tail (spec §11): a bounded, redacted list and a live stream. Both
//! routes require the local owner and answer `Cache-Control: no-store`,
//! errors included.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::{self, Stream};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use super::contracts::{ErrorBody, LogLineResponse, LogsEnvelope};
use super::http::{json_response, request_query};
use super::jobs::{authorize, no_store};
use super::sessions::rejected;
use super::{ApiError, AppState};
use crate::live;
use crate::logs::{LogBuffer, LogFilter, LogLevel, LogLine, StreamGuard};

pub(super) const LOGS_LEVEL_INVALID: &str = "level must be one of error, warn, info, debug, trace";
pub(super) const LOGS_LIMIT_INVALID: &str = "limit must be from 1 to 1000";
pub(super) const LOGS_AFTER_INVALID: &str = "after must be a whole number";
pub(super) const LOGS_QUERY_TOO_LONG: &str = "q must be at most 200 characters";
pub(super) const LOGS_TOO_MANY_STREAMS: &str = "Too many log streams are open";

pub(super) const DEFAULT_LOGS_LIMIT: usize = 200;
pub(super) const MAX_LOGS_LIMIT: usize = 1_000;
pub(super) const MAX_LOG_STREAMS: usize = 8;
pub(super) const MAX_LOG_QUERY_CHARS: usize = 200;

type Params = HashMap<String, String>;

struct LogQuery {
    filter: LogFilter,
    after: Option<u64>,
    params: Params,
}

/// `level`, `q`, and `after`, validated in that order.
fn parse_query(request: &Request) -> Result<LogQuery, ApiError> {
    let params = request_query(request.uri())
        .map_err(|()| ApiError::bad_request_static("malformed query"))?;
    let level = params
        .get("level")
        .map(|level| {
            LogLevel::parse(level).ok_or_else(|| ApiError::bad_request_static(LOGS_LEVEL_INVALID))
        })
        .transpose()?;
    let query = params.get("q").map(String::as_str);
    if query.is_some_and(|query| query.chars().count() > MAX_LOG_QUERY_CHARS) {
        return Err(ApiError::bad_request_static(LOGS_QUERY_TOO_LONG));
    }
    let after = params
        .get("after")
        .map(|after| {
            after
                .parse::<u64>()
                .map_err(|_| ApiError::bad_request_static(LOGS_AFTER_INVALID))
        })
        .transpose()?;
    Ok(LogQuery {
        filter: LogFilter::new(level, query),
        after,
        params,
    })
}

fn limit(params: &Params) -> Result<usize, ApiError> {
    match params.get("limit") {
        None => Ok(DEFAULT_LOGS_LIMIT),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_LOGS_LIMIT).contains(limit))
            .ok_or_else(|| ApiError::bad_request_static(LOGS_LIMIT_INVALID)),
    }
}

#[utoipa::path(get, path = "/api/logs", tag = "logs",
    params(
        ("level" = Option<String>, Query, description = "The lowest level to show: error, warn, info, debug, or trace"),
        ("q" = Option<String>, Query, description = "Case-insensitive text in the message or target; at most 200 characters"),
        ("after" = Option<u64>, Query, description = "Only lines with a higher seq, oldest first"),
        ("limit" = Option<usize>, Query, description = "1-1000, default 200")
    ),
    responses(
        (status = 200, description = "Redacted log lines, oldest first, and the newest seq", body = LogsEnvelope),
        (status = 400, description = "An invalid query", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody)
    ))]
pub(super) async fn list_logs(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let parsed = parse_query(&request).and_then(|query| {
        let limit = limit(&query.params)?;
        Ok((query, limit))
    });
    let (query, limit) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return rejected(error),
    };
    let lines = state.logs.lines(&query.filter, query.after, limit);
    no_store(json_response(
        StatusCode::OK,
        &LogsEnvelope {
            lines: lines.iter().map(LogLineResponse::from).collect(),
            newest_seq: state.logs.newest_seq(),
        },
    ))
}

#[utoipa::path(get, path = "/api/logs/stream", tag = "logs",
    params(
        ("level" = Option<String>, Query, description = "The lowest level to send"),
        ("q" = Option<String>, Query, description = "Case-insensitive text in the message or target; at most 200 characters"),
        ("after" = Option<u64>, Query, description = "Send the buffered lines after this seq first; without it, only new lines")
    ),
    responses(
        (status = 200, description = "Server-Sent Events: `log` (one log line) and `resync` (`{\"newestSeq\": n}`, when this stream fell behind; refetch the list after your newest seq)", content_type = "text/event-stream"),
        (status = 400, description = "An invalid query", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 429, description = "Eight log streams are already open", body = ErrorBody)
    ))]
pub(super) async fn stream_logs(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let query = match parse_query(&request) {
        Ok(query) => query,
        Err(error) => return rejected(error),
    };
    let Some(guard) = state.logs.try_open_stream(MAX_LOG_STREAMS) else {
        return rejected(ApiError::too_many_requests(LOGS_TOO_MANY_STREAMS));
    };
    // Without `after`, only lines pushed from now on are sent. A line pushed
    // between this read and the subscription is in the backlog and also on
    // the receiver; the stream drops the repeat by seq.
    let after = query.after.unwrap_or_else(|| state.logs.newest_seq());
    let mut response = Sse::new(log_events(
        Arc::clone(&state.logs),
        query.filter,
        after,
        guard,
    ))
    .keep_alive(KeepAlive::new().interval(Duration::from_secs(live::EVENT_KEEP_ALIVE_SECS)))
    .into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    no_store(response)
}

fn log_event(line: &LogLine) -> Event {
    Event::default()
        .event("log")
        .data(serde_json::to_string(&LogLineResponse::from(line)).unwrap_or_default())
}

fn resync_event(newest_seq: u64) -> Event {
    Event::default()
        .event("resync")
        .data(serde_json::json!({ "newestSeq": newest_seq }).to_string())
}

struct StreamState {
    pending: VecDeque<Arc<LogLine>>,
    receiver: tokio::sync::broadcast::Receiver<Arc<LogLine>>,
    last: u64,
    buffer: Arc<LogBuffer>,
    filter: LogFilter,
    /// Held for the life of the body, so a closed connection frees its slot.
    _guard: StreamGuard,
}

/// The backlog after `after` (taken with the receiver under one lock hold, so
/// nothing falls between them), then live lines; every line once, by seq.
/// No lock is held across an `.await`.
fn log_events(
    buffer: Arc<LogBuffer>,
    filter: LogFilter,
    after: u64,
    guard: StreamGuard,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let (backlog, receiver) = buffer.subscribe_after(after);
    let state = StreamState {
        pending: VecDeque::from(backlog),
        receiver,
        last: after,
        buffer,
        filter,
        _guard: guard,
    };
    stream::unfold(state, |mut state| async move {
        loop {
            while let Some(line) = state.pending.pop_front() {
                if line.seq <= state.last {
                    continue;
                }
                state.last = line.seq;
                if state.filter.matches(&line) {
                    return Some((Ok(log_event(&line)), state));
                }
            }
            match state.receiver.recv().await {
                Ok(line) => state.pending.push_back(line),
                Err(RecvError::Lagged(_)) => {
                    // Lines up to `last` are the client's to refetch. The
                    // receiver's backlog is moved to `pending` (older lines
                    // are skipped there, newer ones still sent), so a line
                    // pushed meanwhile is not lost and the next push does not
                    // overflow the channel again.
                    state.last = state.buffer.newest_seq();
                    loop {
                        match state.receiver.try_recv() {
                            Ok(line) => state.pending.push_back(line),
                            Err(TryRecvError::Lagged(_)) => {
                                state.last = state.buffer.newest_seq();
                            }
                            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                        }
                    }
                    return Some((Ok(resync_event(state.last)), state));
                }
                Err(RecvError::Closed) => return None,
            }
        }
    })
}
