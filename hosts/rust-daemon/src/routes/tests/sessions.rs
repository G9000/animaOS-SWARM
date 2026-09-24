use super::*;
use anima_core::MessageRole;

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
