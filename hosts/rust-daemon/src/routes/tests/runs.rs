use super::*;
use anima_core::DataValue;
use serde_json::json;

use crate::agent_runs::test_support::{events_until, Gate, ScriptedModel, Step};
use crate::runs::{RunRecord, RunStatus};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource, DEFAULT_CHAT_TITLE};

const OWNER_ORIGIN: &str = "http://localhost:4200";

fn start_request(
    agent: &str,
    session: &str,
    key: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/agents/{agent}/sessions/{}/runs",
            session.replace(':', "%3A")
        ))
        .header("host", "127.0.0.1:8080")
        .header("origin", OWNER_ORIGIN)
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn get_request(uri: &str, origin: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

/// A router over a daemon whose agent has the new chat `chat:plans`.
async fn app_with_chat(
    model: Arc<dyn ModelAdapter>,
) -> (axum::Router, Arc<RwLock<DaemonState>>, String) {
    let mut daemon = DaemonState::with_model_adapter(model);
    let agent = daemon
        .create_agent(test_config("companion"))
        .unwrap()
        .state
        .id;
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        "chat:plans",
        SessionKind::Chat,
        SessionOrigin::Web,
        DEFAULT_CHAT_TITLE.into(),
        TitleSource::FirstMessage,
        1,
    ));
    let state = Arc::new(RwLock::new(daemon));
    (router(state.clone(), DaemonConfig::default()), state, agent)
}

async fn wait_for(state: &Arc<RwLock<DaemonState>>, run_id: &str, status: RunStatus) -> RunRecord {
    for _ in 0..500 {
        if let Some(record) = state
            .read()
            .await
            .runs
            .get(run_id)
            .filter(|record| record.status == status)
        {
            return record.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {run_id} never became {status:?}");
}

fn telegram_connector(agent: &str, id: &str, room: &str) -> TelegramConnectorRecord {
    TelegramConnectorRecord {
        id: id.into(),
        agent_id: agent.into(),
        room_id: room.into(),
        bot: TelegramBotIdentity {
            id: "session-runs-bot".into(),
            username: Some("session_runs_bot".into()),
            display_name: None,
        },
        approved_chat: Some(TelegramChatMetadata {
            id: "session-runs-chat".into(),
            kind: TelegramChatKind::Private,
            title: None,
            username: None,
        }),
        pending_pairing: None,
        next_update_id: 0,
        enabled: true,
        deleted_at_ms: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

#[tokio::test]
async fn starting_a_run_checks_the_owner_the_key_and_the_body() {
    let (app, _, agent) = app_with_chat(ScriptedModel::new(vec![])).await;
    let untrusted = Request::builder()
        .method("POST")
        .uri(format!("/api/agents/{agent}/sessions/chat%3Aplans/runs"))
        .header("host", "127.0.0.1:8080")
        .header("origin", "https://untrusted.example")
        .header("idempotency-key", "key")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"text":"hi"}"#))
        .unwrap();
    let refused = app.clone().oneshot(untrusted).await.unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(refused).await["error"],
        "local owner authorization required"
    );

    let long_key = "k".repeat(129);
    let too_long = "x".repeat(32 * 1024 + 1);
    let eleven: Vec<String> = (0..11).map(|n| format!("att-{n}")).collect();
    for (key, body, message) in [
        (
            None,
            json!({"text": "hi"}),
            "Idempotency-Key header is required",
        ),
        (
            Some(long_key.as_str()),
            json!({"text": "hi"}),
            "Idempotency-Key header is invalid",
        ),
        (
            Some("bad key"),
            json!({"text": "hi"}),
            "Idempotency-Key header is invalid",
        ),
        (
            Some("k1"),
            json!({"text": "   "}),
            "text or attachments are required",
        ),
        (
            Some("k2"),
            json!({"text": too_long}),
            "text must be at most 32 KiB",
        ),
        (
            Some("k3"),
            json!({"text": "hi", "attachmentIds": eleven}),
            "at most 10 attachments are allowed per message",
        ),
        (
            Some("k4"),
            json!({"text": "hi", "attachmentIds": ["att-1"]}),
            "unknown attachment ids",
        ),
        (
            Some("k5"),
            json!({"text": "hi", "skill": "summarize"}),
            "unknown skill",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(start_request(&agent, "chat:plans", key, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{message}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message);
    }
    for (agent_id, session) in [(agent.as_str(), "chat:missing"), ("missing", "chat:plans")] {
        let response = app
            .clone()
            .oneshot(start_request(
                agent_id,
                session,
                Some("k6"),
                json!({"text": "hi"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

/// Controller ruling (M3 pre-flight audit M29): a 32 KiB text survives
/// worst-case JSON escaping (six bytes a character) under the route's own
/// body limit, so the text check answers, not the generic body limit.
#[tokio::test]
async fn a_fully_escaped_32_kib_text_is_checked_by_its_length_not_the_body_limit() {
    let (app, state, agent) = app_with_chat(ScriptedModel::new(vec![])).await;
    let escaped = "\u{1}".repeat(32 * 1024);
    let body = json!({ "text": escaped });
    assert!(
        body.to_string().len() > DaemonConfig::default().max_request_bytes,
        "the body is larger than the daemon-wide limit"
    );

    let accepted = app
        .clone()
        .oneshot(start_request(&agent, "chat:plans", Some("k-escaped"), body))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let run_id = json_body(accepted).await["run"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for(&state, &run_id, RunStatus::Completed).await;

    let over = app
        .clone()
        .oneshot(start_request(
            &agent,
            "chat:plans",
            Some("k-over"),
            json!({ "text": "\u{1}".repeat(32 * 1024 + 1) }),
        ))
        .await
        .unwrap();
    assert_eq!(over.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(over).await["error"],
        "text must be at most 32 KiB"
    );

    let oversized = app
        .oneshot(start_request(
            &agent,
            "chat:plans",
            Some("k-huge"),
            json!({ "text": "\u{1}".repeat(64 * 1024) }),
        ))
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::BAD_REQUEST);
    assert_eq!(oversized.headers()["cache-control"], "no-store");
    assert_eq!(json_body(oversized).await["error"], "malformed request");
}

#[tokio::test]
async fn a_message_is_accepted_as_a_queued_run_that_then_completes() {
    let (app, state, agent) = app_with_chat(ScriptedModel::new(vec![Step::Text(vec![
        "Here ",
        "is the plan",
    ])]))
    .await;

    let response = app
        .clone()
        .oneshot(start_request(
            &agent,
            "chat:plans",
            Some("key-1"),
            json!({"text": "Plan the offsite"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = json_body(response).await;
    assert_eq!(body["run"]["status"], "queued");
    assert_eq!(body["run"]["source"], "web");
    assert_eq!(body["run"]["sessionId"], "chat:plans");
    assert_eq!(body["run"]["input"]["text"], "Plan the offsite");
    assert!(body.get("steer").is_none());
    let run_id = body["run"]["id"].as_str().unwrap().to_string();

    let finished = wait_for(&state, &run_id, RunStatus::Completed).await;
    {
        let guard = state.read().await;
        let messages: Vec<_> = guard.agents[&agent]
            .messages()
            .iter()
            .filter(|message| message.room_id == "chat:plans")
            .collect();
        assert_eq!(messages.len(), 2);
        let metadata = messages[0].content.metadata.as_ref().unwrap();
        assert_eq!(
            metadata["clientRequestId"],
            DataValue::String("key-1".into())
        );
        assert_eq!(
            metadata["idempotencyKey"],
            DataValue::String("key-1".into())
        );
        assert_eq!(messages[1].content.text, "Here is the plan");
        assert_eq!(
            finished.reply_message_id.as_deref(),
            Some(messages[1].id.as_str())
        );
    }

    let read = app
        .oneshot(get_request(
            &format!("/api/agents/{agent}/runs/{run_id}"),
            OWNER_ORIGIN,
        ))
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(read.headers()["cache-control"], "no-store");
    let read = json_body(read).await;
    assert_eq!(read["run"]["status"], "completed");
    assert_eq!(
        read["run"]["replyMessageId"],
        finished.reply_message_id.unwrap().as_str()
    );
}

#[tokio::test]
async fn a_reused_key_returns_the_original_run_and_a_different_text_conflicts() {
    let gate = Gate::new();
    let (app, state, agent) = app_with_chat(ScriptedModel::gated(vec![], gate.clone())).await;
    let send =
        |text: &str| start_request(&agent, "chat:plans", Some("key-1"), json!({ "text": text }));

    let first = json_body(app.clone().oneshot(send("hello")).await.unwrap()).await;
    let run_id = first["run"]["id"].as_str().unwrap().to_string();
    gate.entered().await;

    let replay = app.clone().oneshot(send("hello")).await.unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.headers()["cache-control"], "no-store");
    let replay = json_body(replay).await;
    assert_eq!(replay["run"]["id"], run_id.as_str());
    assert_eq!(replay["run"]["status"], "running");

    let conflict = app.clone().oneshot(send("something else")).await.unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(conflict.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(conflict).await["error"],
        "Idempotency-Key was already used for a different message"
    );

    gate.release();
    wait_for(&state, &run_id, RunStatus::Completed).await;
    let after = app.clone().oneshot(send("hello")).await.unwrap();
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(json_body(after).await["run"]["status"], "completed");

    let listed = json_body(
        app.oneshot(get_request(
            &format!("/api/agents/{agent}/sessions/chat%3Aplans/runs"),
            OWNER_ORIGIN,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        listed["runs"].as_array().unwrap().len(),
        1,
        "a replay creates nothing"
    );
    assert_eq!(
        state.read().await.agents[&agent]
            .messages()
            .iter()
            .filter(|message| message.room_id == "chat:plans")
            .count(),
        2
    );
}

#[tokio::test]
async fn messages_in_one_session_run_in_acceptance_order() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![], gate.clone());
    let (app, state, agent) = app_with_chat(model.clone()).await;
    let mut run_ids = Vec::new();
    for (index, text) in ["first", "second", "third"].into_iter().enumerate() {
        let key = format!("key-{index}");
        let body = json_body(
            app.clone()
                .oneshot(start_request(
                    &agent,
                    "chat:plans",
                    Some(key.as_str()),
                    json!({ "text": text }),
                ))
                .await
                .unwrap(),
        )
        .await;
        run_ids.push(body["run"]["id"].as_str().unwrap().to_string());
    }
    for _ in 0..3 {
        gate.entered().await;
        gate.release();
    }
    for run_id in &run_ids {
        wait_for(&state, run_id, RunStatus::Completed).await;
    }
    let order: Vec<String> = model
        .requests()
        .iter()
        .map(|request| request.messages.last().unwrap().content.text.clone())
        .collect();
    assert_eq!(order, ["first", "second", "third"]);
}

#[tokio::test]
async fn a_ninth_waiting_message_is_refused_with_429() {
    let gate = Gate::new();
    let (app, state, agent) = app_with_chat(ScriptedModel::gated(vec![], gate.clone())).await;
    let send = |key: String| {
        start_request(
            &agent,
            "chat:plans",
            Some(key.as_str()),
            json!({ "text": key.clone() }),
        )
    };
    let mut run_ids = Vec::new();
    let first = json_body(app.clone().oneshot(send("key-0".into())).await.unwrap()).await;
    run_ids.push(first["run"]["id"].as_str().unwrap().to_string());
    gate.entered().await; // the first run is running, not queued
    for n in 1..=8 {
        let response = app.clone().oneshot(send(format!("key-{n}"))).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        run_ids.push(
            json_body(response).await["run"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(state.read().await.runs.queued_count(&agent), 8);

    let refused = app.clone().oneshot(send("key-9".into())).await.unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(refused).await["error"],
        "This companion already has 8 queued messages; wait for one to start"
    );

    for _ in 0..9 {
        gate.release();
    }
    wait_for(&state, run_ids.last().unwrap(), RunStatus::Completed).await;
}

/// Carry-forward (M2 T17 Minor 9): a message waiting behind another counts in
/// its session's `activeRuns`, so the web never declares it unconfirmed.
#[tokio::test]
async fn a_waiting_message_counts_as_an_active_run_of_its_session() {
    let gate = Gate::new();
    let (app, state, agent) = app_with_chat(ScriptedModel::gated(vec![], gate.clone())).await;
    let mut run_ids = Vec::new();
    for key in ["key-1", "key-2"] {
        let body = json_body(
            app.clone()
                .oneshot(start_request(
                    &agent,
                    "chat:plans",
                    Some(key),
                    json!({ "text": key }),
                ))
                .await
                .unwrap(),
        )
        .await;
        run_ids.push(body["run"]["id"].as_str().unwrap().to_string());
        if key == "key-1" {
            gate.entered().await;
        }
    }
    assert_eq!(
        state.read().await.runs.get(&run_ids[1]).unwrap().status,
        RunStatus::Queued
    );

    let session = json_body(
        app.clone()
            .oneshot(get_request(
                &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(session["session"]["activeRuns"], 2);

    gate.release();
    gate.entered().await;
    gate.release();
    wait_for(&state, &run_ids[1], RunStatus::Completed).await;
}

#[tokio::test]
async fn read_only_kinds_helpers_and_unsteerable_sessions_are_refused() {
    let mut daemon = DaemonState::with_model_adapter(ScriptedModel::new(vec![]));
    let agent = daemon
        .create_agent(test_config("companion"))
        .unwrap()
        .state
        .id;
    let mut helper_config = test_config("helper");
    let additional = &mut helper_config.settings.as_mut().unwrap().additional;
    additional.insert("workspaceRole".into(), DataValue::String("helper".into()));
    additional.insert("parentAgentId".into(), DataValue::String(agent.clone()));
    let helper = daemon.create_agent(helper_config).unwrap().state.id;
    daemon.connectors.insert(
        "telegram-refusals".into(),
        telegram_connector(&agent, "telegram-refusals", "telegram-room-refusals"),
    );
    for (owner, id, kind, origin) in [
        (&agent, "job:1", SessionKind::Job, SessionOrigin::Job),
        (
            &agent,
            "telegram-room-refusals",
            SessionKind::Telegram,
            SessionOrigin::Telegram,
        ),
        (
            &agent,
            "telegram-room-gone",
            SessionKind::Telegram,
            SessionOrigin::Telegram,
        ),
        (
            &helper,
            "room-9",
            SessionKind::Helper,
            SessionOrigin::Delegation,
        ),
    ] {
        daemon.sessions.insert(SessionRecord::new(
            owner,
            id,
            kind,
            origin,
            "Session".into(),
            TitleSource::System,
            1,
        ));
    }
    let app = router(Arc::new(RwLock::new(daemon)), DaemonConfig::default());

    let job = app
        .clone()
        .oneshot(start_request(
            &agent,
            "job:1",
            Some("k1"),
            json!({"text": "hi"}),
        ))
        .await
        .unwrap();
    assert_eq!(job.status(), StatusCode::CONFLICT);
    assert_eq!(job.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(job).await["error"],
        "This session cannot receive messages"
    );
    let steer = app
        .clone()
        .oneshot(start_request(
            &agent,
            "telegram-room-refusals",
            Some("k2"),
            json!({"text": "hi", "mode": "steer"}),
        ))
        .await
        .unwrap();
    assert_eq!(steer.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(steer).await["error"],
        "This session cannot be steered"
    );
    let disconnected = app
        .clone()
        .oneshot(start_request(
            &agent,
            "telegram-room-gone",
            Some("k4"),
            json!({"text": "hi"}),
        ))
        .await
        .unwrap();
    assert_eq!(disconnected.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(disconnected).await["error"],
        "This session cannot receive messages",
        "a Telegram session without its active connector"
    );
    let helper_run = app
        .oneshot(start_request(
            &helper,
            "room-9",
            Some("k3"),
            json!({"text": "hi"}),
        ))
        .await
        .unwrap();
    assert_eq!(helper_run.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(helper_run).await["error"],
        "Helpers must run through their owning companion"
    );
}

#[tokio::test]
async fn a_telegram_sessions_message_runs_as_the_connectors_owner_turn() {
    let mut daemon =
        DaemonState::with_model_adapter(ScriptedModel::new(vec![Step::Text(vec!["On it"])]));
    let agent = daemon
        .create_agent(test_config("companion"))
        .unwrap()
        .state
        .id;
    let connector_id = "telegram-session-runs";
    let room_id = "telegram-room-session-runs";
    daemon.connectors.insert(
        connector_id.into(),
        telegram_connector(&agent, connector_id, room_id),
    );
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        room_id,
        SessionKind::Telegram,
        SessionOrigin::Telegram,
        "Telegram".into(),
        TitleSource::System,
        1,
    ));
    let state = Arc::new(RwLock::new(daemon));
    let limiter = Arc::new(Semaphore::new(4));
    let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
    let manager = ConnectorManager::new(
        Arc::clone(&state),
        runs.clone(),
        Arc::new(InMemoryCredentialStore::default()),
        Arc::new(CountingTelegramTransport::default()),
    );
    let app = router_with_services(
        Arc::clone(&state),
        DaemonConfig::default(),
        limiter,
        runs,
        manager.clone(),
        true,
    );

    let response = app
        .oneshot(start_request(
            &agent,
            room_id,
            Some("tg-key"),
            json!({"text": "Remind me at 5"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = json_body(response).await;
    assert_eq!(body["run"]["source"], "telegram");
    assert_eq!(body["run"]["sourceRef"], connector_id);
    let finished = wait_for(
        &state,
        body["run"]["id"].as_str().unwrap(),
        RunStatus::Completed,
    )
    .await;
    {
        let guard = state.read().await;
        let outbound: Vec<_> = guard
            .outbound
            .values()
            .filter(|outbound| outbound.connector_id == connector_id)
            .collect();
        assert_eq!(outbound.len(), 1, "the reply is queued for delivery");
        assert_eq!(
            Some(outbound[0].assistant_message_id.as_str()),
            finished.reply_message_id.as_deref()
        );
    }
    manager.shutdown().await;
}

#[tokio::test]
async fn the_first_message_titles_a_new_chat_when_it_is_accepted() {
    let gate = Gate::new();
    let (app, state, agent) = app_with_chat(ScriptedModel::gated(vec![], gate.clone())).await;
    let hub = state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent).unwrap();

    let body = json_body(
        app.oneshot(start_request(
            &agent,
            "chat:plans",
            Some("key-1"),
            json!({"text": "Plan the offsite\nwith every detail"}),
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        state
            .read()
            .await
            .sessions
            .get(&agent, "chat:plans")
            .unwrap()
            .title,
        "Plan the offsite"
    );
    let events = events_until(&mut subscription, "session.updated").await;
    assert_eq!(events[0]["type"], "run.queued");
    assert_eq!(events[0]["run"]["status"], "queued");

    gate.entered().await;
    gate.release();
    wait_for(
        &state,
        body["run"]["id"].as_str().unwrap(),
        RunStatus::Completed,
    )
    .await;
    assert_eq!(
        state
            .read()
            .await
            .sessions
            .get(&agent, "chat:plans")
            .unwrap()
            .title,
        "Plan the offsite",
        "the commit keeps it"
    );
}

#[tokio::test]
async fn a_check_in_session_accepts_the_owners_reply() {
    let (app, state, agent) =
        app_with_chat(ScriptedModel::new(vec![Step::Text(vec!["Noted"])])).await;
    state.write().await.sessions.insert(SessionRecord::new(
        &agent,
        "schedule:daily",
        SessionKind::Checkin,
        SessionOrigin::Schedule,
        "Check-in".into(),
        TitleSource::System,
        1,
    ));

    let body = json_body(
        app.oneshot(start_request(
            &agent,
            "schedule:daily",
            Some("key-1"),
            json!({"text": "Done for today"}),
        ))
        .await
        .unwrap(),
    )
    .await;

    let finished = wait_for(
        &state,
        body["run"]["id"].as_str().unwrap(),
        RunStatus::Completed,
    )
    .await;
    assert_eq!(finished.session_id, "schedule:daily");
    assert!(state.read().await.agents[&agent]
        .messages()
        .iter()
        .any(|message| message.room_id == "schedule:daily" && message.content.text == "Noted"));
}

#[tokio::test]
async fn session_runs_are_listed_newest_first_and_read_from_the_ledger_or_history() {
    let (app, state, agent) = app_with_chat(ScriptedModel::new(vec![])).await;
    let mut ids = Vec::new();
    for n in 0..2 {
        let key = format!("key-{n}");
        let body = json_body(
            app.clone()
                .oneshot(start_request(
                    &agent,
                    "chat:plans",
                    Some(key.as_str()),
                    json!({ "text": format!("message {n}") }),
                ))
                .await
                .unwrap(),
        )
        .await;
        let id = body["run"]["id"].as_str().unwrap().to_string();
        wait_for(&state, &id, RunStatus::Completed).await;
        ids.push(id);
    }

    let runs_path = format!("/api/agents/{agent}/sessions/chat%3Aplans/runs");
    let listed = app
        .clone()
        .oneshot(get_request(&format!("{runs_path}?limit=1"), OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(listed.headers()["cache-control"], "no-store");
    let listed = json_body(listed).await;
    assert_eq!(listed["runs"].as_array().unwrap().len(), 1);
    assert_eq!(listed["runs"][0]["id"], ids[1].as_str());
    let all = json_body(
        app.clone()
            .oneshot(get_request(&runs_path, OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    let all: Vec<&str> = all["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["id"].as_str().unwrap())
        .collect();
    assert_eq!(all, [ids[1].as_str(), ids[0].as_str()], "newest first");
    for (limit, message) in [
        ("51", "limit must be between 1 and 50"),
        ("0", "limit must be between 1 and 50"),
        ("many", "limit must be between 1 and 50"),
    ] {
        let bad_limit = app
            .clone()
            .oneshot(get_request(
                &format!("{runs_path}?limit={limit}"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap();
        assert_eq!(bad_limit.status(), StatusCode::BAD_REQUEST);
        assert_eq!(bad_limit.headers()["cache-control"], "no-store");
        assert_eq!(json_body(bad_limit).await["error"], message);
    }
    for uri in [
        format!("/api/agents/{agent}/sessions/chat%3Amissing/runs"),
        format!("/api/agents/missing/sessions/chat%3Aplans/runs"),
    ] {
        let response = app
            .clone()
            .oneshot(get_request(&uri, OWNER_ORIGIN))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    // Controller ruling (M3 pre-flight audit M25).
    let refused = app
        .clone()
        .oneshot(get_request(&runs_path, "https://untrusted.example"))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["cache-control"], "no-store");

    // A run pruned from the ledger is still read from the history store.
    let (archived, history) = {
        let mut guard = state.write().await;
        (guard.runs.remove(&ids[0]).unwrap(), guard.history.clone())
    };
    history.store().upsert_runs(&[archived]).await.unwrap();
    let read = app
        .clone()
        .oneshot(get_request(
            &format!("/api/agents/{agent}/runs/{}", ids[0]),
            OWNER_ORIGIN,
        ))
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(read.headers()["cache-control"], "no-store");
    assert_eq!(json_body(read).await["run"]["id"], ids[0].as_str());

    for uri in [
        format!("/api/agents/{agent}/runs/run_missing"),
        format!("/api/agents/missing/runs/{}", ids[1]),
    ] {
        let response = app
            .clone()
            .oneshot(get_request(&uri, OWNER_ORIGIN))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let refused = app
        .oneshot(get_request(
            &format!("/api/agents/{agent}/runs/{}", ids[1]),
            "https://untrusted.example",
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["cache-control"], "no-store");
}

/// Another agent's run is not found through this agent, from the ledger or
/// the history store.
#[tokio::test]
async fn a_run_is_read_only_through_its_own_agent() {
    let (app, state, agent) = app_with_chat(ScriptedModel::new(vec![])).await;
    let other = state
        .write()
        .await
        .create_agent(test_config("other"))
        .unwrap()
        .state
        .id;
    let body = json_body(
        app.clone()
            .oneshot(start_request(
                &agent,
                "chat:plans",
                Some("key-1"),
                json!({"text": "hi"}),
            ))
            .await
            .unwrap(),
    )
    .await;
    let run_id = body["run"]["id"].as_str().unwrap().to_string();
    let finished = wait_for(&state, &run_id, RunStatus::Completed).await;

    let through_other = format!("/api/agents/{other}/runs/{run_id}");
    let response = app
        .clone()
        .oneshot(get_request(&through_other, OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let history = {
        let mut guard = state.write().await;
        guard.runs.remove(&run_id);
        guard.history.clone()
    };
    history.store().upsert_runs(&[finished]).await.unwrap();
    let response = app
        .oneshot(get_request(&through_other, OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn the_openapi_document_lists_the_session_run_routes_under_runs() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    let paths = &document.paths.paths;
    let runs = &paths["/api/agents/{agent_id}/sessions/{session_id}/runs"];
    let start = runs.post.as_ref().expect("starting a run is documented");
    let list = runs.get.as_ref().expect("listing runs is documented");
    let read = paths["/api/agents/{agent_id}/runs/{run_id}"]
        .get
        .as_ref()
        .expect("reading a run is documented");
    for operation in [start, list, read] {
        assert_eq!(operation.tags.as_deref(), Some(&["runs".to_string()][..]));
    }
    let start = serde_json::to_value(start).unwrap();
    for status in ["200", "202", "400", "403", "404", "409", "429", "503"] {
        assert!(
            start["responses"].get(status).is_some(),
            "start documents {status}"
        );
    }
    // Controller ruling (M3 pre-flight audit M1): the window is the ledger's.
    let key = start["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "Idempotency-Key")
        .expect("the key header is documented");
    let description = key["description"].as_str().unwrap();
    assert!(
        description.contains("while its run is still in the ledger"),
        "{description}"
    );
    assert!(description.contains("50 finished runs"), "{description}");
}
