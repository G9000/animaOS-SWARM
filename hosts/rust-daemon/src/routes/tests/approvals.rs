use super::*;
use std::collections::BTreeMap;

use anima_core::primitives::now_millis;
use anima_core::{DataValue, ToolCall};
use serde_json::{json, Value};

use crate::approvals::{
    ApprovalDecisionKind, ApprovalRequest, ApprovalResolution, ApprovalStatus,
    PendingApprovalStart, ResolvedBy,
};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

const OWNER_ORIGIN: &str = "http://localhost:4200";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn request(method: &str, uri: &str, origin: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn remember() -> ToolCall {
    ToolCall {
        id: "call-1".into(),
        name: "memory_add".into(),
        args: BTreeMap::from([("content".to_string(), DataValue::String("the plan".into()))]),
    }
}

fn pending(agent: &str, session: &str, run: &str, at_ms: u64) -> ApprovalRequest {
    ApprovalRequest::pending(
        PendingApprovalStart {
            agent_id: agent,
            session_id: session,
            run_id: run,
            call: &remember(),
            timeout_ms: 60_000,
        },
        at_ms,
    )
}

fn decided(agent: &str, session: &str, at_ms: u64, note: &str) -> ApprovalRequest {
    let mut approval = pending(agent, session, "run_done", at_ms);
    approval.resolve(
        ApprovalStatus::Allowed,
        ApprovalResolution {
            decision: Some(ApprovalDecisionKind::AllowOnce),
            note: Some(note.into()),
            matcher: None,
            rule_id: None,
            resolved_by: ResolvedBy::Owner,
            resolved_at_ms: at_ms + 1,
        },
    );
    approval
}

/// A daemon whose agent has the chat `chat:plans`.
fn daemon() -> (Arc<RwLock<DaemonState>>, String) {
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
        TitleSource::Owner,
        1,
    ));
    (Arc::new(RwLock::new(daemon)), agent)
}

fn ids(body: &Value) -> Vec<String> {
    body["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|approval| approval["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn listing_approvals_requires_the_owner() {
    let (state, _) = daemon();
    let app = router(state, DaemonConfig::default());

    let refused = app
        .oneshot(request(
            "GET",
            "/api/approvals",
            "https://untrusted.example",
            None,
        ))
        .await
        .unwrap();

    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn pending_approvals_are_listed_oldest_first_and_filtered_by_agent() {
    let (state, agent) = daemon();
    let (later, earlier, other) = (
        pending(&agent, "chat:plans", "run_a", 20),
        pending(&agent, "chat:plans", "run_a", 10),
        pending("agent-other", "chat:x", "run_x", 5),
    );
    {
        let mut guard = state.write().await;
        for approval in [later.clone(), earlier.clone(), other.clone()] {
            guard.approvals.insert(approval);
        }
        guard
            .approvals
            .insert(decided(&agent, "chat:plans", 30, "done"));
    }
    let app = router(state, DaemonConfig::default());

    let mine = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/approvals?status=pending&agentId={agent}"),
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(mine.status(), StatusCode::OK);
    assert_eq!(mine.headers()["cache-control"], "no-store");
    let mine = json_body(mine).await;
    assert_eq!(ids(&mine), [earlier.id.clone(), later.id.clone()]);
    assert_eq!(mine["nextCursor"], Value::Null);
    assert_eq!(mine["approvals"][0]["matcherKinds"], json!(["any"]));

    let everyone = json_body(
        app.oneshot(request("GET", "/api/approvals", OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(ids(&everyone), [other.id, earlier.id, later.id]);
}

#[tokio::test]
async fn decided_approvals_merge_the_store_and_the_control_plane_within_thirty_days() {
    let (state, agent) = daemon();
    let now = now_millis();
    let old = decided(&agent, "chat:plans", now - 31 * DAY_MS, "too old");
    let recent = decided(&agent, "chat:plans", now - 2 * DAY_MS, "stored");
    let mut both = decided(&agent, "chat:plans", now - 3 * DAY_MS, "stored copy");
    let held = decided(&agent, "chat:plans", now - DAY_MS, "held");
    // Written outside the state lock, as the outbox writes.
    let history = state.read().await.history.clone();
    history
        .store()
        .upsert_approvals(&[old, recent.clone(), both.clone()])
        .await
        .unwrap();
    both.resolution.as_mut().unwrap().note = Some("control plane copy".into());
    {
        let mut guard = state.write().await;
        guard.approvals.insert(held.clone());
        guard.approvals.insert(both.clone());
    }
    let app = router(state, DaemonConfig::default());

    let first = json_body(
        app.clone()
            .oneshot(request(
                "GET",
                &format!("/api/approvals?status=decided&agentId={agent}&limit=2"),
                OWNER_ORIGIN,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(ids(&first), [held.id.clone(), recent.id.clone()]);
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    assert_eq!(cursor, format!("{}:{}", recent.created_at_ms, recent.id));

    let second = json_body(
        app.oneshot(request(
            "GET",
            &format!("/api/approvals?status=decided&agentId={agent}&limit=2&cursor={cursor}"),
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        ids(&second),
        [both.id.clone()],
        "the 31-day-old one is left out"
    );
    assert_eq!(
        second["approvals"][0]["resolution"]["note"],
        "control plane copy"
    );
    assert_eq!(second["nextCursor"], Value::Null);
}

#[tokio::test]
async fn invalid_approval_queries_are_rejected() {
    let (state, _) = daemon();
    let app = router(state, DaemonConfig::default());
    for (query, message) in [
        ("status=bogus", "status must be pending or decided"),
        ("status=decided&limit=0", "limit must be between 1 and 100"),
        (
            "status=decided&limit=101",
            "limit must be between 1 and 100",
        ),
        ("status=decided&cursor=nonsense", "cursor is not valid"),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "GET",
                &format!("/api/approvals?{query}"),
                OWNER_ORIGIN,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message, "{query}");
    }
}

#[tokio::test]
async fn a_session_counts_its_pending_approvals() {
    let (state, agent) = daemon();
    {
        let mut guard = state.write().await;
        guard
            .approvals
            .insert(pending(&agent, "chat:plans", "run_a", 10));
        guard
            .approvals
            .insert(decided(&agent, "chat:plans", 5, "done"));
    }
    let app = router(state, DaemonConfig::default());

    let body = json_body(
        app.oneshot(request(
            "GET",
            &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap(),
    )
    .await;

    assert_eq!(body["session"]["pendingApprovals"], 1);
}
