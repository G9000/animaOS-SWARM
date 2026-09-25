use super::*;
use anima_core::{DataValue, MessageRole};

use crate::history::conformance::history_message;
use crate::sessions::test_support::{message, seed_messages};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

const OWNER_ORIGIN: &str = "http://localhost:4200";

/// A model adapter that blocks in `generate` until released, signaling entry
/// first -- lets a test hold a run's room lock open (same pattern as
/// `routes::agents::tests::PendingModelAdapter`, not reusable across those
/// two test modules).
struct PendingModelAdapter {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

#[async_trait]
impl ModelAdapter for PendingModelAdapter {
    fn provider(&self) -> &str {
        "pending"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        self.entered.add_permits(1);
        self.release
            .acquire()
            .await
            .expect("release semaphore should remain open")
            .forget();
        Ok(ModelGenerateResponse {
            content: Content {
                text: format!("{} handled task: pending", config.name),
                attachments: None,
                metadata: None,
            },
            tool_calls: None,
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                ..TokenUsage::default()
            },
            stop_reason: ModelStopReason::End,
        })
    }
}

fn pending_adapter() -> (Arc<dyn ModelAdapter>, Arc<Semaphore>, Arc<Semaphore>) {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    (
        Arc::new(PendingModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }),
        entered,
        release,
    )
}

fn get(uri: &str, origin: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap()
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn app_with_session() -> (axum::Router, Arc<RwLock<DaemonState>>, String) {
    let mut daemon = DaemonState::new();
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
        "Plans".into(),
        TitleSource::FirstMessage,
        1,
    ));
    seed_messages(
        &mut daemon,
        &agent,
        vec![
            message(
                &agent,
                "m1",
                "chat:plans",
                MessageRole::User,
                "Plan the offsite",
                1,
            ),
            message(
                &agent,
                "m2",
                "chat:plans",
                MessageRole::Assistant,
                "Here is the plan",
                2,
            ),
        ],
    );
    let state = Arc::new(RwLock::new(daemon));
    (router(state.clone(), DaemonConfig::default()), state, agent)
}

#[tokio::test]
async fn session_reads_require_the_owner_and_are_never_cached() {
    let (app, _, agent) = app_with_session().await;
    for path in [
        format!("/api/agents/{agent}/sessions"),
        format!("/api/agents/{agent}/sessions/chat%3Aplans"),
        format!("/api/agents/{agent}/sessions/chat%3Aplans/messages"),
    ] {
        let response = app
            .clone()
            .oneshot(get(&path, "https://untrusted.example"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let response = app.clone().oneshot(get(&path, OWNER_ORIGIN)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn session_routes_list_one_session_and_page_its_messages() {
    let (app, _, agent) = app_with_session().await;
    let list = json(
        app.clone()
            .oneshot(get(
                &format!("/api/agents/{agent}/sessions?limit=10"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    let session = &list["sessions"][0];
    assert_eq!(session["id"], "chat:plans");
    assert_eq!(session["agentId"], agent.as_str());
    assert_eq!(session["roomId"], "chat:plans");
    assert_eq!(session["kind"], "chat");
    assert_eq!(session["origin"], "web");
    assert_eq!(session["titleSource"], "first_message");
    assert_eq!(session["messageCount"], 2);
    assert_eq!(session["preview"], "Here is the plan");
    assert_eq!(session["unread"], true);
    assert_eq!(session["activeRuns"], 0);
    assert_eq!(session["pendingApprovals"], 0);
    assert_eq!(session["capabilities"]["delete"], true);
    assert!(session.get("match").is_none());
    assert_eq!(list["nextCursor"], serde_json::Value::Null);

    let one = json(
        app.clone()
            .oneshot(get(
                &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(one["session"]["title"], "Plans");

    let page = json(
        app.clone()
            .oneshot(get(
                &format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?limit=1"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(page["messages"][0]["id"], "m2");
    assert_eq!(page["messages"][0]["role"], "assistant");
    assert_eq!(page["messages"][0]["text"], "Here is the plan");
    assert_eq!(page["nextBefore"], "m2");
    let older = json(
        app.clone()
            .oneshot(get(
                &format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?limit=1&before=m2"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(older["messages"][0]["id"], "m1");
    assert_eq!(older["nextBefore"], serde_json::Value::Null);

    for (path, status, error) in [
        (
            "/api/agents/missing/sessions".to_string(),
            StatusCode::NOT_FOUND,
            "not found",
        ),
        (
            format!("/api/agents/{agent}/sessions/chat%3Amissing"),
            StatusCode::NOT_FOUND,
            "not found",
        ),
        (
            format!("/api/agents/{agent}/sessions?limit=0"),
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 200",
        ),
        (
            format!("/api/agents/{agent}/sessions?kind=bogus"),
            StatusCode::BAD_REQUEST,
            "kind must be one of chat, telegram, checkin, job, helper",
        ),
        (
            format!("/api/agents/{agent}/sessions?archived=maybe"),
            StatusCode::BAD_REQUEST,
            "archived must be true or false",
        ),
        (
            format!("/api/agents/{agent}/sessions?cursor=%21"),
            StatusCode::BAD_REQUEST,
            "cursor is invalid",
        ),
        (
            format!("/api/agents/{agent}/sessions?q={}", "a".repeat(201)),
            StatusCode::BAD_REQUEST,
            "q must be at most 200 characters",
        ),
        (
            format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?before=nope"),
            StatusCode::BAD_REQUEST,
            "before message was not found",
        ),
        (
            format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?limit=201"),
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 200",
        ),
    ] {
        let response = app.clone().oneshot(get(&path, OWNER_ORIGIN)).await.unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store", "{path}");
        assert_eq!(json(response).await["error"], error, "{path}");
    }
}

#[tokio::test]
async fn search_finds_a_session_whose_only_match_is_older_than_five_hundred_matches_elsewhere() {
    // Controller ruling 2 (M2 pre-flight audit): session search ranks
    // sessions by their newest matching message, so a session with more than
    // SEARCH_SESSION_LIMIT (500) matches must not crowd an older session's only
    // match out of the results.
    let (app, state, agent) = app_with_session().await;
    let store = {
        let mut daemon = state.write().await;
        daemon.sessions.insert(SessionRecord::new(
            &agent,
            "chat:busy",
            SessionKind::Chat,
            SessionOrigin::Web,
            "Busy".into(),
            TitleSource::FirstMessage,
            2_000,
        ));
        daemon.sessions.insert(SessionRecord::new(
            &agent,
            "chat:quiet",
            SessionKind::Chat,
            SessionOrigin::Web,
            "Quiet".into(),
            TitleSource::FirstMessage,
            1,
        ));
        daemon.history.store()
    };
    let busy_rows = (0..=500u64)
        .map(|n| {
            history_message(
                &format!("busy-{n}"),
                &agent,
                "chat:busy",
                MessageRole::User,
                "deploy the build",
                1_000 + n,
            )
        })
        .collect::<Vec<_>>();
    store.upsert_messages(&busy_rows).await.unwrap();
    store
        .upsert_messages(&[history_message(
            "quiet-1",
            &agent,
            "chat:quiet",
            MessageRole::User,
            "deploy notes",
            1,
        )])
        .await
        .unwrap();

    let list = json(
        app.clone()
            .oneshot(get(
                &format!("/api/agents/{agent}/sessions?q=deploy"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    let ids = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["chat:busy", "chat:quiet"],
        "a session with more than 500 matches must not crowd out an older session's only match"
    );
}

#[tokio::test]
async fn listing_agents_as_summaries_omits_their_messages() {
    let (app, _, agent) = app_with_session().await;
    let summaries = json(
        app.clone()
            .oneshot(get("/api/agents?view=summary", OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    let summary = &summaries["agents"][0];
    assert_eq!(summary["state"]["id"], agent.as_str());
    assert_eq!(summary["messageCount"], 2);
    assert!(summary.get("messages").is_none());
    let full = json(
        app.clone()
            .oneshot(get("/api/agents", OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        full["agents"][0]["messages"].as_array().unwrap().len(),
        2,
        "the default response is unchanged"
    );
    // Controller ruling 1 (M2 pre-flight audit): an unrecognized `view` is
    // ignored, not rejected -- only `view=summary` changes the shape.
    let unrecognized = json(
        app.clone()
            .oneshot(get("/api/agents?view=full", OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        unrecognized["agents"][0]["messages"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "an unrecognized view falls back to the full response"
    );
    // Fix round 1 (M2 review): a malformed query string is not a rejection
    // either -- this route used to ignore the URI entirely.
    let malformed = json(
        app.clone()
            .oneshot(get("/api/agents?%zz=1", OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        malformed["agents"][0]["messages"].as_array().unwrap().len(),
        2,
        "a malformed query string falls back to the full response, not a rejection"
    );
}

#[test]
fn the_openapi_document_lists_the_session_read_routes() {
    use utoipa::OpenApi;

    let paths = crate::routes::ApiDoc::openapi().paths.paths;
    for path in [
        "/api/agents/{agent_id}/sessions",
        "/api/agents/{agent_id}/sessions/{session_id}",
        "/api/agents/{agent_id}/sessions/{session_id}/messages",
    ] {
        assert!(paths.contains_key(path), "{path}");
    }
}

fn send(method: &str, uri: &str, origin: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// A path a JSON control-plane save cannot write to: a directory where the
/// store expects a file (same trick as `connectors::runtime::tests::
/// invalid_snapshot_directory`, not reusable across those two test modules).
fn invalid_snapshot_directory(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "anima-session-route-{label}-invalid-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[tokio::test]
async fn session_mutations_and_export_require_the_owner() {
    let (app, _, agent) = app_with_session().await;
    let session = format!("/api/agents/{agent}/sessions/chat%3Aplans");
    for request in [
        send(
            "POST",
            &format!("/api/agents/{agent}/sessions"),
            "https://untrusted.example",
            serde_json::json!({}),
        ),
        send(
            "PATCH",
            &session,
            "https://untrusted.example",
            serde_json::json!({"archived": true}),
        ),
        send(
            "DELETE",
            &session,
            "https://untrusted.example",
            serde_json::Value::Null,
        ),
        get(&format!("{session}/export"), "https://untrusted.example"),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn creating_a_session_for_a_helper_agent_is_refused() {
    // Controller ruling 1 (M2 pre-flight audit): helpers have no chat of
    // their own; they only run through the companion that delegates to them.
    let (app, state, agent) = app_with_session().await;
    let helper_id = {
        let mut guard = state.write().await;
        let mut helper = test_config("helper");
        let settings = helper.settings.as_mut().unwrap();
        settings
            .additional
            .insert("workspaceRole".into(), DataValue::String("helper".into()));
        settings
            .additional
            .insert("parentAgentId".into(), DataValue::String(agent.clone()));
        guard.create_agent(helper).unwrap().state.id
    };

    let response = app
        .clone()
        .oneshot(send(
            "POST",
            &format!("/api/agents/{helper_id}/sessions"),
            OWNER_ORIGIN,
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        json(response).await["error"],
        "Helpers must run through their owning companion"
    );
}

#[tokio::test]
async fn creating_a_chat_saves_it_returns_201_and_is_rate_limited() {
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};

    let (app, state, agent) = app_with_session().await;
    let path = std::env::temp_dir().join(format!(
        "anima-session-create-{}.json",
        uuid::Uuid::new_v4()
    ));
    let store = ControlPlaneStoreConfig::Json(path.clone());
    state
        .write()
        .await
        .set_control_plane_store(Some(store.clone()));
    let create = format!("/api/agents/{agent}/sessions");

    let empty = Request::builder()
        .method("POST")
        .uri(&create)
        .header("host", "127.0.0.1:8080")
        .header("origin", OWNER_ORIGIN)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(empty).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let created = json(response).await;
    let id = created["session"]["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("chat:"), "{id}");
    assert_eq!(created["session"]["title"], "New chat");
    assert_eq!(created["session"]["titleSource"], "first_message");
    assert_eq!(created["session"]["kind"], "chat");
    let saved = load_control_plane_snapshot(&store).await.unwrap().unwrap();
    assert!(
        saved.sessions.iter().any(|session| session.id == id),
        "the new chat is durable"
    );

    let titled = json(
        app.clone()
            .oneshot(send(
                "POST",
                &create,
                OWNER_ORIGIN,
                serde_json::json!({"title": "  Trip   plans "}),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(titled["session"]["title"], "Trip plans");
    assert_eq!(titled["session"]["titleSource"], "owner");

    for (uri, body, status, error) in [
        (
            create.clone(),
            serde_json::json!({"title": ""}),
            StatusCode::BAD_REQUEST,
            "title must be 1 to 120 characters",
        ),
        (
            "/api/agents/missing/sessions".to_string(),
            serde_json::json!({}),
            StatusCode::NOT_FOUND,
            "not found",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(send("POST", &uri, OWNER_ORIGIN, body))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{uri}");
        assert_eq!(json(response).await["error"], error);
    }

    {
        let mut guard = state.write().await;
        let now = anima_core::primitives::now_millis();
        while guard.session_limiter.try_acquire(&agent, now) {}
    }
    let response = app
        .clone()
        .oneshot(send("POST", &create, OWNER_ORIGIN, serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        json(response).await["error"],
        "Too many new chats; try again in a minute"
    );
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn a_failed_save_leaves_no_new_chat_behind() {
    // Minor 4 (fix round 1, M2 review): create's rollback was untested.
    use crate::control_plane_store::ControlPlaneStoreConfig;

    let (app, state, agent) = app_with_session().await;
    let invalid_path = invalid_snapshot_directory("session-create");
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid_path.clone())));

    let response = app
        .clone()
        .oneshot(send(
            "POST",
            &format!("/api/agents/{agent}/sessions"),
            OWNER_ORIGIN,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");

    let ids = state
        .read()
        .await
        .sessions
        .records()
        .filter(|record| record.agent_id == agent)
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["chat:plans"],
        "the failed create's session was rolled back"
    );

    std::fs::remove_dir_all(invalid_path).unwrap();
}

#[tokio::test]
async fn patching_renames_archives_and_marks_a_session_read() {
    let (app, state, agent) = app_with_session().await;
    state.write().await.sessions.insert(SessionRecord::new(
        &agent,
        "job:job-1",
        SessionKind::Job,
        SessionOrigin::Job,
        "Job · Weekly report".into(),
        TitleSource::System,
        1,
    ));
    let chat = format!("/api/agents/{agent}/sessions/chat%3Aplans");
    let job = format!("/api/agents/{agent}/sessions/job%3Ajob-1");

    let renamed = json(
        app.clone()
            .oneshot(send(
                "PATCH",
                &chat,
                OWNER_ORIGIN,
                serde_json::json!({"title": "Offsite", "lastReadAtMs": 2}),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(renamed["session"]["title"], "Offsite");
    assert_eq!(renamed["session"]["titleSource"], "owner");
    assert_eq!(renamed["session"]["lastReadAtMs"], 2);
    assert_eq!(renamed["session"]["unread"], false);
    let archived = json(
        app.clone()
            .oneshot(send(
                "PATCH",
                &job,
                OWNER_ORIGIN,
                serde_json::json!({"archived": true}),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(archived["session"]["archived"], true);
    assert!(
        state
            .read()
            .await
            .sessions
            .get(&agent, "job:job-1")
            .unwrap()
            .archived
    );

    for (uri, body, status, error) in [
        (
            chat.clone(),
            serde_json::json!({}),
            StatusCode::BAD_REQUEST,
            "at least one of title, archived, or lastReadAtMs is required",
        ),
        (
            chat.clone(),
            serde_json::json!({"lastReadAtMs": u64::MAX}),
            StatusCode::BAD_REQUEST,
            "lastReadAtMs must not be in the future",
        ),
        (
            chat.clone(),
            serde_json::json!({"title": " "}),
            StatusCode::BAD_REQUEST,
            "title must be 1 to 120 characters",
        ),
        (
            job.clone(),
            serde_json::json!({"title": "Renamed"}),
            StatusCode::CONFLICT,
            "This session cannot be renamed",
        ),
        (
            format!("/api/agents/{agent}/sessions/chat%3Amissing"),
            serde_json::json!({"archived": true}),
            StatusCode::NOT_FOUND,
            "not found",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(send("PATCH", &uri, OWNER_ORIGIN, body))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json(response).await["error"], error, "{uri}");
    }
}

#[tokio::test]
async fn a_failed_save_restores_the_previous_record_on_patch() {
    // Minor 4 (fix round 1, M2 review): PATCH's rollback was untested.
    use crate::control_plane_store::ControlPlaneStoreConfig;

    let (app, state, agent) = app_with_session().await;
    let invalid_path = invalid_snapshot_directory("session-patch");
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid_path.clone())));

    let response = app
        .clone()
        .oneshot(send(
            "PATCH",
            &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
            OWNER_ORIGIN,
            serde_json::json!({"title": "Renamed", "archived": true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");

    let record = state
        .read()
        .await
        .sessions
        .get(&agent, "chat:plans")
        .unwrap()
        .clone();
    assert_eq!(
        record.title, "Plans",
        "the previous record is restored, not the failed rename"
    );
    assert_eq!(record.title_source, TitleSource::FirstMessage);
    assert!(!record.archived, "the failed archive did not stick either");

    std::fs::remove_dir_all(invalid_path).unwrap();
}

#[tokio::test]
async fn deleting_a_chat_removes_its_record_messages_and_history_rows() {
    // Important 2's success case (fix round 1, M2 review): a real JSON
    // control-plane store, so the *saved* snapshot -- not just in-memory
    // state -- is asserted to hold the session's `HistoryDeletion` and none
    // of its runs.
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};

    let (app, state, agent) = app_with_session().await;
    seed_messages(
        &mut *state.write().await,
        &agent,
        vec![message(
            &agent,
            "k1",
            "chat:keep",
            MessageRole::User,
            "keep me",
            3,
        )],
    );
    let run_id = {
        let mut guard = state.write().await;
        let mut run = crate::runs::RunRecord::running(
            crate::runs::RunStart {
                agent_id: agent.clone(),
                session_id: "chat:plans".into(),
                source: crate::runs::RunSource::Api,
                source_ref: None,
                idempotency_key: None,
                text: "done already".into(),
                model: "gpt-5.4".into(),
                provider: None,
                parent_run_id: None,
            },
            1,
        );
        run.finish(crate::runs::RunStatus::Completed, None, 1);
        let id = run.id.clone();
        guard.runs.insert(run);
        id
    };
    let history = state.read().await.history.clone();
    history
        .flush_once(&state, &tokio::sync::Mutex::new(()), 10)
        .await
        .unwrap();
    // Minor 7 (fix round 1, M2 review): confirm `chat:plans`'s rows are
    // really mirrored before deleting, not just that they are absent
    // afterward (which would also be true if they had never been mirrored).
    let mirrored_before_delete = history
        .store()
        .page_messages(&crate::history::MessagePageQuery {
            agent_id: agent.clone(),
            session_id: "chat:plans".into(),
            before: None,
            limit: 10,
            include_hidden: true,
        })
        .await
        .unwrap();
    assert_eq!(
        mirrored_before_delete.len(),
        2,
        "chat:plans's messages are mirrored before the delete"
    );
    let session = format!("/api/agents/{agent}/sessions/chat%3Aplans");
    let path = std::env::temp_dir().join(format!(
        "anima-session-delete-durable-{}.json",
        uuid::Uuid::new_v4()
    ));
    let store = ControlPlaneStoreConfig::Json(path.clone());
    state
        .write()
        .await
        .set_control_plane_store(Some(store.clone()));

    let response = app
        .clone()
        .oneshot(send(
            "DELETE",
            &session,
            OWNER_ORIGIN,
            serde_json::Value::Null,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(json(response).await["deleted"], true);
    {
        let guard = state.read().await;
        assert!(guard.sessions.get(&agent, "chat:plans").is_none());
        let rooms = guard
            .get_agent(&agent)
            .unwrap()
            .messages
            .into_iter()
            .map(|message| message.room_id)
            .collect::<Vec<_>>();
        assert_eq!(rooms, ["chat:keep"], "other rooms keep their messages");
        assert!(
            guard.runs.get(&run_id).is_none(),
            "the session's terminal run leaves the ledger"
        );
    }
    let saved = load_control_plane_snapshot(&store).await.unwrap().unwrap();
    assert!(
        saved.pending_history_deletions.iter().any(|deletion| {
            deletion.agent_id == agent && deletion.session_id.as_deref() == Some("chat:plans")
        }),
        "the saved snapshot records the session's deletion"
    );
    assert!(
        saved.runs.iter().all(|run| run.session_id != "chat:plans"),
        "the saved snapshot holds none of the session's runs"
    );

    history
        .flush_once(&state, &tokio::sync::Mutex::new(()), 11)
        .await
        .unwrap();
    let rows = history
        .store()
        .page_messages(&crate::history::MessagePageQuery {
            agent_id: agent.clone(),
            session_id: "chat:plans".into(),
            before: None,
            limit: 10,
            include_hidden: true,
        })
        .await
        .unwrap();
    assert!(rows.is_empty(), "the history rows are deleted too");
    assert_eq!(
        history.store().get_run(&run_id).await.unwrap(),
        None,
        "the deleted session's run is never mirrored"
    );
    assert!(state.read().await.pending_history_deletions.is_empty());
    let response = app
        .clone()
        .oneshot(get(&session, OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn a_failed_delete_save_restores_the_record_messages_and_runs() {
    // Important 2 (fix round 1, M2 review): the durable protocol and its
    // rollback were untested. If `clear_history_deletion` regressed, nothing
    // here would fail and a restart would replay a deletion of a session
    // that is, in fact, still there -- data loss.
    use crate::control_plane_store::ControlPlaneStoreConfig;

    let (app, state, agent) = app_with_session().await;
    let run_id = {
        let mut guard = state.write().await;
        let mut run = crate::runs::RunRecord::running(
            crate::runs::RunStart {
                agent_id: agent.clone(),
                session_id: "chat:plans".into(),
                source: crate::runs::RunSource::Api,
                source_ref: None,
                idempotency_key: None,
                text: "done already".into(),
                model: "gpt-5.4".into(),
                provider: None,
                parent_run_id: None,
            },
            1,
        );
        run.finish(crate::runs::RunStatus::Completed, None, 1);
        let id = run.id.clone();
        guard.runs.insert(run);
        id
    };
    let invalid_path = invalid_snapshot_directory("session-delete");
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid_path.clone())));

    let response = app
        .clone()
        .oneshot(send(
            "DELETE",
            &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
            OWNER_ORIGIN,
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");

    let guard = state.read().await;
    assert!(
        guard.sessions.get(&agent, "chat:plans").is_some(),
        "the record is restored"
    );
    let message_ids = guard
        .get_agent(&agent)
        .unwrap()
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        message_ids,
        ["m1", "m2"],
        "the room's hot messages are restored"
    );
    assert!(
        guard.runs.get(&run_id).is_some(),
        "the session's terminal run is restored"
    );
    assert!(
        guard.pending_history_deletions.is_empty(),
        "a failed save leaves no pending entry"
    );
    drop(guard);

    std::fs::remove_dir_all(invalid_path).unwrap();
}

#[tokio::test]
async fn deleting_is_refused_by_kind_and_while_a_run_is_active() {
    let (app, state, agent) = app_with_session().await;
    state.write().await.sessions.insert(SessionRecord::new(
        &agent,
        "telegram:bot",
        SessionKind::Telegram,
        SessionOrigin::Telegram,
        "Telegram · @bot".into(),
        TitleSource::System,
        1,
    ));
    state
        .write()
        .await
        .runs
        .insert(crate::runs::RunRecord::running(
            crate::runs::RunStart {
                agent_id: agent.clone(),
                session_id: "chat:plans".into(),
                source: crate::runs::RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "still working".into(),
                model: "gpt-5.4".into(),
                provider: None,
                parent_run_id: None,
            },
            1,
        ));

    for (uri, error) in [
        (
            format!("/api/agents/{agent}/sessions/telegram%3Abot"),
            "This session cannot be deleted",
        ),
        (
            format!("/api/agents/{agent}/sessions/chat%3Aplans"),
            "A run in this session is still in progress",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(send("DELETE", &uri, OWNER_ORIGIN, serde_json::Value::Null))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json(response).await["error"], error);
    }
    assert!(state
        .read()
        .await
        .sessions
        .get(&agent, "chat:plans")
        .is_some());
}

#[tokio::test]
async fn deleting_a_session_whose_room_a_run_holds_is_refused_then_succeeds() {
    // Minor 2 (fix round 1, M2 review): `try_reserve_room`'s refusal branch
    // was never exercised -- the test above only inserts a `RunRecord`
    // directly, without ever taking the room's real lock, so it only reaches
    // the ledger recheck, not `try_reserve_room` itself.
    let (adapter, entered, release) = pending_adapter();
    let mut daemon = DaemonState::with_model_adapter(adapter);
    let agent = daemon
        .create_agent(test_config("companion"))
        .unwrap()
        .state
        .id;
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        "chat:held",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Held".into(),
        TitleSource::FirstMessage,
        1,
    ));
    let state = Arc::new(RwLock::new(daemon));
    let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
    let app = custom_router(Arc::clone(&state), runs.clone(), DaemonConfig::default());

    let held = {
        let runs = runs.clone();
        let agent_id = agent.clone();
        tokio::spawn(async move {
            runs.run(crate::agent_runs::AgentRunRequest {
                agent_id,
                content: Content {
                    text: "hold the room".into(),
                    attachments: None,
                    metadata: None,
                },
                room: crate::agent_runs::RunRoom::Stable("chat:held".into()),
                idempotency_key: None,
                source: crate::runs::RunSource::Web,
                source_ref: None,
                parent: None,
            })
            .await
        })
    };
    entered.acquire().await.unwrap().forget();

    assert!(
        runs.try_reserve_room(&agent, "chat:held").is_none(),
        "the room's lock is held by the in-flight run"
    );
    let session = format!("/api/agents/{agent}/sessions/chat%3Aheld");
    let response = app
        .clone()
        .oneshot(send(
            "DELETE",
            &session,
            OWNER_ORIGIN,
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        json(response).await["error"],
        "A run in this session is still in progress"
    );

    release.add_permits(1);
    held.await
        .expect("the held run should join")
        .expect("the held run should finish");

    let response = app
        .oneshot(send(
            "DELETE",
            &session,
            OWNER_ORIGIN,
            serde_json::Value::Null,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the room is free once the run finishes"
    );
}

#[tokio::test]
async fn exporting_a_session_returns_its_full_markdown_transcript() {
    let (app, state, agent) = app_with_session().await;
    state
        .read()
        .await
        .history
        .store()
        .upsert_messages(&[crate::history::conformance::history_message(
            "m0",
            &agent,
            "chat:plans",
            MessageRole::User,
            "An archived question",
            0,
        )])
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(get(
            &format!("/api/agents/{agent}/sessions/chat%3Aplans/export"),
            OWNER_ORIGIN,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"plans.md\""
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.starts_with("# Plans\n"), "{body}");
    let archived = body.find("An archived question").unwrap();
    let question = body.find("Plan the offsite").unwrap();
    let answer = body.find("Here is the plan").unwrap();
    assert!(archived < question && question < answer, "{body}");
    assert!(body.contains("**You** · 1970-01-01 00:00 UTC"), "{body}");
    assert!(
        body.contains("**companion** · 1970-01-01 00:00 UTC"),
        "{body}"
    );
    let missing = app
        .clone()
        .oneshot(get(
            &format!("/api/agents/{agent}/sessions/chat%3Amissing/export"),
            OWNER_ORIGIN,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn exporting_a_session_keeps_messages_longer_than_the_search_index_cap_whole() {
    // Residual round R1 (M2 final fix wave re-review): search indexes only the
    // first MAX_INDEXED_TEXT_BYTES of a message, but the export promises the
    // full transcript, so longer messages (kept in the store or in the hot
    // tail) must come out whole.
    let (app, state, agent) = app_with_session().await;
    let filler = "x".repeat(crate::history::MAX_INDEXED_TEXT_BYTES);
    let stored_text = format!("stored alphaneedle {filler} stored omeganeedle");
    let hot_text = format!("hot alphaneedle {filler} hot omeganeedle");
    state
        .read()
        .await
        .history
        .store()
        .upsert_messages(&[history_message(
            "m0",
            &agent,
            "chat:plans",
            MessageRole::Tool,
            &stored_text,
            0,
        )])
        .await
        .unwrap();
    seed_messages(
        &mut *state.write().await,
        &agent,
        vec![message(
            &agent,
            "m3",
            "chat:plans",
            MessageRole::Assistant,
            &hot_text,
            3,
        )],
    );

    let response = app
        .clone()
        .oneshot(get(
            &format!("/api/agents/{agent}/sessions/chat%3Aplans/export"),
            OWNER_ORIGIN,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        body.contains(&stored_text),
        "a stored message over the index cap is exported whole"
    );
    assert!(
        body.contains(&hot_text),
        "a hot message over the index cap is exported whole"
    );
}

#[tokio::test]
async fn exporting_a_session_with_an_unreadable_store_answers_service_unavailable() {
    // Minor 3 (fix round 1, M2 review): the export's 503 string was unasserted.
    use crate::history::conformance::FlakyHistoryStore;
    use crate::history::HistoryService;

    let flaky = Arc::new(FlakyHistoryStore::new());
    let mut daemon = DaemonState::new();
    daemon.set_history(HistoryService::new(flaky.clone()));
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
        "Plans".into(),
        TitleSource::FirstMessage,
        1,
    ));
    let state = Arc::new(RwLock::new(daemon));
    let app = router(state, DaemonConfig::default());
    flaky.set_failing(true);

    let response = app
        .oneshot(get(
            &format!("/api/agents/{agent}/sessions/chat%3Aplans/export"),
            OWNER_ORIGIN,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        json(response).await["error"],
        "history store is unavailable"
    );
}

#[test]
fn the_openapi_document_lists_the_session_mutation_routes() {
    use utoipa::OpenApi;

    let paths = crate::routes::ApiDoc::openapi().paths.paths;
    assert!(paths["/api/agents/{agent_id}/sessions"].post.is_some());
    let session = &paths["/api/agents/{agent_id}/sessions/{session_id}"];
    assert!(session.patch.is_some() && session.delete.is_some());
    assert!(paths["/api/agents/{agent_id}/sessions/{session_id}/export"]
        .get
        .is_some());
}
