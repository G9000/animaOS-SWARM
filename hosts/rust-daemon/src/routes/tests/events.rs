use super::*;
use crate::live::{
    LiveEvent, LiveEventBody, LiveHub, LiveToolView, MAX_EVENT_SUBSCRIBERS_PER_AGENT,
};
use crate::runs::{RunRecord, RunSource, RunStart, RunStatus};
use http_body_util::BodyExt;

const OWNER_ORIGIN: &str = "http://localhost:4200";

fn events_request(agent_id: &str, origin: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/agents/{agent_id}/events"))
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap()
}

/// One parsed SSE block.
struct SseEvent {
    id: Option<String>,
    event: Option<String>,
    data: serde_json::Value,
}

/// Reads a text/event-stream body one event at a time, skipping keep-alives.
struct SseReader {
    body: Body,
    buffer: String,
}

impl SseReader {
    fn new(response: axum::response::Response) -> Self {
        Self {
            body: response.into_body(),
            buffer: String::new(),
        }
    }

    async fn next(&mut self) -> SseEvent {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let mut id = None;
                let mut event = None;
                let mut data = Vec::new();
                for line in block.lines() {
                    if let Some(value) = line.strip_prefix("id:") {
                        id = Some(value.trim().to_string());
                    } else if let Some(value) = line.strip_prefix("event:") {
                        event = Some(value.trim().to_string());
                    } else if let Some(value) = line.strip_prefix("data:") {
                        data.push(value.trim_start().to_string());
                    }
                }
                if data.is_empty() {
                    continue;
                }
                return SseEvent {
                    id,
                    event,
                    data: serde_json::from_str(&data.join("\n")).unwrap(),
                };
            }
            let frame = tokio::time::timeout(Duration::from_secs(5), self.body.frame())
                .await
                .expect("an event arrives within five seconds")
                .expect("the stream stays open")
                .unwrap();
            if let Ok(bytes) = frame.into_data() {
                self.buffer.push_str(std::str::from_utf8(&bytes).unwrap());
            }
        }
    }
}

fn state_with_agent() -> (Arc<RwLock<DaemonState>>, String) {
    let mut daemon = DaemonState::new();
    let agent = daemon
        .create_agent(test_config("companion"))
        .unwrap()
        .state
        .id;
    (Arc::new(RwLock::new(daemon)), agent)
}

fn running(agent_id: &str, session_id: &str) -> RunRecord {
    RunRecord::running(
        RunStart {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            source: RunSource::Web,
            source_ref: None,
            idempotency_key: None,
            text: "draft the plan".into(),
            model: "gpt-5.4".into(),
            provider: Some("openai".into()),
            parent_run_id: None,
        },
        1,
    )
}

#[test]
fn the_openapi_document_lists_the_event_stream_under_runs() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    let stream = document.paths.paths["/api/agents/{agent_id}/events"]
        .get
        .as_ref()
        .expect("the stream is documented");
    assert_eq!(stream.tags.as_deref(), Some(&["runs".to_string()][..]));
    assert!(document
        .tags
        .unwrap_or_default()
        .iter()
        .any(|tag| tag.name == "runs"));
}

#[tokio::test]
async fn the_event_stream_requires_the_owner_and_is_never_cached() {
    let (state, agent) = state_with_agent();
    let app = router(state, DaemonConfig::default());

    let refused = app
        .clone()
        .oneshot(events_request(&agent, "https://untrusted.example"))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["cache-control"], "no-store");

    let missing = app
        .clone()
        .oneshot(events_request("missing", OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.headers()["cache-control"], "no-store");

    let open = app
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::OK);
    assert_eq!(open.headers()["cache-control"], "no-store");
    assert_eq!(open.headers()["x-accel-buffering"], "no");
    assert!(open.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
}

#[tokio::test]
async fn the_first_event_is_a_snapshot_of_the_active_runs() {
    let (state, agent) = state_with_agent();
    let active = running(&agent, "chat:a");
    let mut done = running(&agent, "chat:b");
    done.finish(RunStatus::Completed, None, 2);
    {
        let mut guard = state.write().await;
        guard.runs.insert(active.clone());
        guard.runs.insert(done);
        let runs = guard.live.runs();
        runs.register(&active.id);
        runs.start_step(&active.id, &format!("{}:1", active.id));
        runs.append_text(&active.id, "Here is");
        runs.tool_started(
            &active.id,
            LiveToolView {
                step_id: format!("{}:1", active.id),
                tool_call_id: "call-1".into(),
                name: "calculate".into(),
                arguments_preview: "{\"expression\":\"1+2\"}".into(),
                arguments_truncated: false,
                status: "running".into(),
                duration_ms: None,
                result_preview: None,
                truncated: false,
            },
        );
    }
    let app = router(state, DaemonConfig::default());

    let response = app
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    let mut reader = SseReader::new(response);
    let snapshot = reader.next().await;

    assert_eq!(snapshot.id.as_deref(), Some("1"));
    assert_eq!(snapshot.event.as_deref(), Some("stream.snapshot"));
    assert_eq!(snapshot.data["type"], "stream.snapshot");
    assert_eq!(snapshot.data["seq"], 1);
    assert_eq!(snapshot.data["agentId"], agent.as_str());
    let runs = snapshot.data["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "terminal runs are not active");
    assert_eq!(runs[0]["run"]["id"], active.id.as_str());
    assert_eq!(runs[0]["run"]["status"], "running");
    assert_eq!(runs[0]["stepId"], format!("{}:1", active.id));
    assert_eq!(runs[0]["text"], "Here is");
    assert_eq!(runs[0]["textOffset"], 0);
    assert_eq!(runs[0]["tools"][0]["name"], "calculate");
    assert_eq!(snapshot.data["approvals"], serde_json::json!([]));
}

#[tokio::test]
async fn the_snapshot_covers_helper_and_delegated_runs_oldest_first() {
    use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
    use anima_core::DataValue;

    let (state, companion) = state_with_agent();
    let expected = {
        let mut guard = state.write().await;
        let mut helper = test_config("helper");
        let settings = helper.settings.as_mut().unwrap();
        settings
            .additional
            .insert("workspaceRole".into(), DataValue::String("helper".into()));
        settings
            .additional
            .insert("parentAgentId".into(), DataValue::String(companion.clone()));
        let helper = guard.create_agent(helper).unwrap().state.id;
        let peer = guard.create_agent(test_config("peer")).unwrap().state.id;
        let mut delegated = SessionRecord::new(
            &peer,
            "room-7",
            SessionKind::Helper,
            SessionOrigin::Delegation,
            "Draft a plan".into(),
            TitleSource::System,
            1,
        );
        delegated.parent_agent_id = Some(companion.clone());
        let delegated_session = delegated.id.clone();
        guard.sessions.insert(delegated);

        let mut own = running(&companion, "chat:a");
        own.created_at_ms = 30;
        let mut helper_run = running(&helper, "room-3");
        helper_run.created_at_ms = 10;
        let mut delegated_run = running(&peer, &delegated_session);
        delegated_run.created_at_ms = 20;
        let peers_own_chat = running(&peer, "chat:own");
        let expected = [&helper_run.id, &delegated_run.id, &own.id].map(String::clone);
        for run in [own, helper_run, delegated_run, peers_own_chat] {
            guard.runs.insert(run);
        }
        expected
    };
    let app = router(state, DaemonConfig::default());

    let response = app
        .oneshot(events_request(&companion, OWNER_ORIGIN))
        .await
        .unwrap();
    let snapshot = SseReader::new(response).next().await;

    let ids = snapshot.data["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["run"]["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        ids, expected,
        "the agent's, its helper's, and a delegated session's runs, oldest first"
    );
}

#[tokio::test]
async fn live_events_follow_the_snapshot_with_increasing_sequence_numbers() {
    let (state, agent) = state_with_agent();
    let hub = state.read().await.live.clone();
    let app = router(state, DaemonConfig::default());
    let response = app
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    let mut reader = SseReader::new(response);
    assert_eq!(reader.next().await.data["type"], "stream.snapshot");

    hub.publish(
        LiveEvent::new(&agent, LiveEventBody::SessionCreated).session("chat:new"),
        None,
    );
    hub.publish(
        LiveEvent::new(
            &agent,
            LiveEventBody::StepDelta {
                step_id: "run_1:1".into(),
                offset: 0,
                text: "Hel".into(),
            },
        )
        .session("chat:new")
        .run("run_1"),
        None,
    );

    let created = reader.next().await;
    assert_eq!(created.id.as_deref(), Some("2"));
    assert_eq!(created.event.as_deref(), Some("session.created"));
    assert_eq!(created.data["sessionId"], "chat:new");
    let delta = reader.next().await;
    assert_eq!(delta.id.as_deref(), Some("3"));
    assert_eq!(delta.data["seq"], 3);
    assert_eq!(delta.data["text"], "Hel");
    assert_eq!(delta.data["runId"], "run_1");
}

#[tokio::test]
async fn a_reconnect_with_last_event_id_starts_again_from_a_snapshot() {
    let (state, agent) = state_with_agent();
    let hub = state.read().await.live.clone();
    let app = router(state, DaemonConfig::default());
    let mut request = events_request(&agent, OWNER_ORIGIN);
    request
        .headers_mut()
        .insert("last-event-id", "99".parse().unwrap());

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut reader = SseReader::new(response);
    let snapshot = reader.next().await;
    assert_eq!(snapshot.id.as_deref(), Some("1"), "there is no replay");
    assert_eq!(snapshot.event.as_deref(), Some("stream.snapshot"));
    assert_eq!(snapshot.data["seq"], 1);

    hub.publish(LiveEvent::new(&agent, LiveEventBody::SessionUpdated), None);
    let next = reader.next().await;
    assert_eq!(next.id.as_deref(), Some("2"), "numbering starts over");
    assert_eq!(next.data["seq"], 2);
}

#[tokio::test]
async fn a_helpers_events_reach_its_companions_stream() {
    let (state, companion) = state_with_agent();
    let hub = state.read().await.live.clone();
    let app = router(state, DaemonConfig::default());
    let response = app
        .oneshot(events_request(&companion, OWNER_ORIGIN))
        .await
        .unwrap();
    let mut reader = SseReader::new(response);
    reader.next().await;

    hub.publish(
        LiveEvent::new("helper-1", LiveEventBody::SessionUpdated).session("room-9"),
        Some(&companion),
    );

    let event = reader.next().await;
    assert_eq!(event.data["type"], "session.updated");
    assert_eq!(event.data["agentId"], "helper-1");
}

#[tokio::test]
async fn the_seventeenth_stream_of_an_agent_is_refused() {
    let (state, agent) = state_with_agent();
    let hub = state.read().await.live.clone();
    let app = router(state, DaemonConfig::default());
    let held: Vec<_> = (0..MAX_EVENT_SUBSCRIBERS_PER_AGENT)
        .map(|_| hub.subscribe(&agent).unwrap())
        .collect();

    let refused = app
        .clone()
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()["cache-control"], "no-store");
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(refused.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(
        body["error"],
        "Too many event streams are open for this agent"
    );

    drop(held);
    let open = app
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_open_stream_ends_when_the_hub_closes() {
    let (state, agent) = state_with_agent();
    let hub = state.read().await.live.clone();
    let app = router(state, DaemonConfig::default());
    let response = app
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    let mut reader = SseReader::new(response);
    assert_eq!(reader.next().await.data["type"], "stream.snapshot");

    // Graceful shutdown closes the hub first (`app::serve_with_state`).
    hub.close();

    let end = tokio::time::timeout(Duration::from_secs(5), reader.body.frame())
        .await
        .expect("the stream ends, so graceful shutdown can finish");
    assert!(end.is_none(), "nothing follows the close");
    assert_eq!(hub.subscribers(&agent), 0, "the stream released its slot");
}

#[tokio::test]
async fn a_lagging_stream_is_told_to_resync_and_keeps_going() {
    let (state, agent) = state_with_agent();
    state.write().await.set_live_hub(LiveHub::new(2));
    let hub = state.read().await.live.clone();
    let app = router(state, DaemonConfig::default());
    let response = app
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    let mut reader = SseReader::new(response);
    assert_eq!(reader.next().await.data["type"], "stream.snapshot");

    for n in 0..5u64 {
        hub.publish(
            LiveEvent::new(
                &agent,
                LiveEventBody::StepDelta {
                    step_id: "run_1:1".into(),
                    offset: n,
                    text: "x".into(),
                },
            ),
            None,
        );
    }

    let resync = reader.next().await;
    assert_eq!(resync.data["type"], "stream.resync");
    assert_eq!(resync.data["missed"], 3);
    assert_eq!(resync.data["seq"], 2);
    let next = reader.next().await;
    assert_eq!(next.data["offset"], 3);
    assert_eq!(next.data["seq"], 3);
}
