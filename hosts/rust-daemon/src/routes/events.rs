//! The agent event stream (spec §6): one SSE connection per companion.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::HeaderValue;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::{self, Stream};
use serde_json::Value;

use super::jobs::{authorize, no_store};
use super::sessions::rejected;
use super::{ApiError, AppState};
use crate::live::{self, LiveDelivery, LiveSubscription};

const TOO_MANY_STREAMS: &str = "Too many event streams are open for this agent";

#[utoipa::path(get, path = "/api/agents/{agent_id}/events", tag = "runs",
    params(("agent_id" = String, Path)),
    responses(
        (status = 200, description = "Server-Sent Events: `stream.snapshot` first, then the session, run, step, message, and tool events of the agent and its helpers; `stream.resync` when this stream fell behind", content_type = "text/event-stream"),
        (status = 403, description = "Local owner required", body = super::contracts::ErrorBody),
        (status = 404, description = "Agent not found", body = super::contracts::ErrorBody),
        (status = 429, description = "Sixteen streams are already open for this agent", body = super::contracts::ErrorBody)
    ))]
pub(super) async fn agent_events(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let (subscription, runs) = {
        let guard = state.daemon.read().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        // Subscribed before the snapshot is taken, so nothing published after
        // it is missed; the client drops the overlap by offset and id.
        let Ok(subscription) = guard.live.subscribe(&agent_id) else {
            return rejected(ApiError::too_many_requests(TOO_MANY_STREAMS));
        };
        (subscription, guard.live_snapshot_runs(&agent_id))
    };
    let snapshot = live::snapshot_json(&agent_id, 1, &runs);
    let mut response = Sse::new(event_stream(agent_id, snapshot, subscription))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(live::EVENT_KEEP_ALIVE_SECS)))
        .into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    no_store(response)
}

fn sse_event(value: &Value) -> Event {
    Event::default()
        .id(value["seq"].as_u64().unwrap_or_default().to_string())
        .event(value["type"].as_str().unwrap_or("message"))
        .data(value.to_string())
}

struct StreamCursor {
    agent_id: String,
    seq: u64,
    snapshot: Option<Value>,
    subscription: LiveSubscription,
}

fn event_stream(
    agent_id: String,
    snapshot: Value,
    subscription: LiveSubscription,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let cursor = StreamCursor {
        agent_id,
        seq: 1,
        snapshot: Some(snapshot),
        subscription,
    };
    stream::unfold(cursor, |mut cursor| async move {
        if let Some(snapshot) = cursor.snapshot.take() {
            return Some((Ok(sse_event(&snapshot)), cursor));
        }
        let delivery = cursor.subscription.next().await?;
        cursor.seq += 1;
        let value = match delivery {
            LiveDelivery::Event(event) => event.to_json(cursor.seq),
            LiveDelivery::Lagged(missed) => live::resync_json(&cursor.agent_id, cursor.seq, missed),
        };
        Some((Ok(sse_event(&value)), cursor))
    })
}
