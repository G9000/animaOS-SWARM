use super::*;
use anima_core::{DataValue, MessageRole};

use crate::history::conformance::history_message;
use crate::sessions::test_support::{message, seed_messages};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

const OWNER_ORIGIN: &str = "http://localhost:4200";

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
    // SEARCH_ROW_LIMIT (500) matches must not crowd an older session's only
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
async fn deleting_a_chat_removes_its_record_messages_and_history_rows() {
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
    let history = state.read().await.history.clone();
    history
        .flush_once(&state, &tokio::sync::Mutex::new(()), 10)
        .await
        .unwrap();
    let session = format!("/api/agents/{agent}/sessions/chat%3Aplans");

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
    }
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
    let response = app
        .clone()
        .oneshot(get(&session, OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
