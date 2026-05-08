use std::convert::Infallible;

use anima_core::{Content, TaskResult};
use anima_swarm::SwarmState;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use futures::stream;

use super::super::contracts::{SwarmEventResponse, SwarmStateResponse, TaskResultResponse};
use crate::events::{EventFanout, EventSubscriber};

pub(super) fn subscribe_swarm_events_response(
    subscriber: EventSubscriber,
) -> axum::response::Response {
    let stream = stream::unfold(subscriber, |mut subscriber| async move {
        loop {
            match subscriber.recv().await {
                Ok(message) => {
                    let event = Event::default().event(message.event).data(message.data);
                    return Some((Ok::<Event, Infallible>(event), subscriber));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    // Surface the gap so the client can resync (refetch the
                    // swarm state). Without this the consumer would silently
                    // see a hole in the event sequence.
                    let event = Event::default()
                        .event("swarm:lagged")
                        .data(format!("{{\"missed\":{missed}}}"));
                    return Some((Ok::<Event, Infallible>(event), subscriber));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

pub(super) fn publish_swarm_event(
    global_event_fanout: &EventFanout,
    swarm_event_fanout: Option<&EventFanout>,
    swarm_id: &str,
    event: &str,
    snapshot: &SwarmState,
    result: Option<&TaskResult<Content>>,
) {
    let payload = super::super::serialize_json(&SwarmEventResponse {
        swarm_id: swarm_id.to_string(),
        state: SwarmStateResponse::from(snapshot),
        result: result.map(TaskResultResponse::from),
    });

    global_event_fanout.publish(event.to_string(), payload.clone());
    if let Some(fanout) = swarm_event_fanout {
        fanout.publish(event.to_string(), payload);
    }
}
