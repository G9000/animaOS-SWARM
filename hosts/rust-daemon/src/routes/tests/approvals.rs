use super::*;
use std::collections::BTreeMap;

use anima_core::primitives::now_millis;
use anima_core::{DataValue, ToolCall};
use serde_json::{json, Value};

use crate::agent_runs::test_support::{companion_config, remember_call, ScriptedModel, Step};
use crate::approvals::{
    ApprovalDecisionKind, ApprovalPolicy, ApprovalRequest, ApprovalResolution, ApprovalStatus,
    PendingApprovalStart, PolicyAction, ResolvedBy, RiskClass, APPROVAL_ALREADY_RESOLVED,
    APPROVAL_NOTE_TOO_LONG, APPROVAL_REVISION_STALE, HELPERS_USE_COMPANION_APPROVALS,
    MATCHER_KIND_NOT_FOR_TOOL, MATCHER_VALUE_INVALID, MAX_APPROVAL_RULES_PER_AGENT,
    READ_TOOLS_NEED_NO_RULE, TOO_MANY_RULES, UNKNOWN_RULE_TOOL,
};
use crate::runs::RunStatus;
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

#[test]
fn the_openapi_document_lists_every_approval_route() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    for path in [
        "/api/approvals",
        "/api/approvals/{approval_id}/decision",
        "/api/agents/{agent_id}/approval-policy",
        "/api/agents/{agent_id}/approval-rules",
        "/api/agents/{agent_id}/approval-rules/{rule_id}",
    ] {
        assert!(document.paths.paths.contains_key(path), "{path}");
    }
}

#[tokio::test]
async fn every_approval_route_requires_the_owner() {
    let (state, agent) = daemon();
    let app = router(state, DaemonConfig::default());
    let policy = json!({"write": "allow", "exec": "ask", "network": "allow", "delegate": "allow"});
    for (method, uri, body) in [
        (
            "POST",
            "/api/approvals/apr_1/decision".to_string(),
            Some(json!({"decision": "allow_once", "revision": 1})),
        ),
        ("GET", format!("/api/agents/{agent}/approval-policy"), None),
        (
            "PUT",
            format!("/api/agents/{agent}/approval-policy"),
            Some(policy),
        ),
        ("GET", format!("/api/agents/{agent}/approval-rules"), None),
        (
            "POST",
            format!("/api/agents/{agent}/approval-rules"),
            Some(json!({"tool": "bash", "matcher": {"kind": "any"}})),
        ),
        (
            "DELETE",
            format!("/api/agents/{agent}/approval-rules/rule_1"),
            None,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, &uri, "https://untrusted.example", body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn a_decision_resolves_the_waiting_call_and_repeats_idempotently() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Saved"]),
    ]);
    let mut daemon = DaemonState::with_model_adapter(model);
    let mut config = companion_config("companion");
    config.tools = Some(
        crate::tools::ToolRegistry::new()
            .resolve_descriptors(["memory_add"])
            .unwrap(),
    );
    let agent = daemon.create_agent(config).unwrap().state.id;
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        "chat:plans",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Plans".into(),
        TitleSource::Owner,
        1,
    ));
    daemon.approvals.set_policy(
        &agent,
        ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
    );
    let state = Arc::new(RwLock::new(daemon));
    let app = router(state.clone(), DaemonConfig::default());

    let mut start = request(
        "POST",
        &format!("/api/agents/{agent}/sessions/chat%3Aplans/runs"),
        OWNER_ORIGIN,
        Some(json!({"text": "remember the plan"})),
    );
    start
        .headers_mut()
        .insert("idempotency-key", "key-1".parse().unwrap());
    let accepted = app.clone().oneshot(start).await.unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let run_id = json_body(accepted).await["run"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut approval = None;
    for _ in 0..500 {
        approval = state
            .read()
            .await
            .approvals
            .pending()
            .first()
            .map(|found| (*found).clone());
        if approval.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let approval = approval.expect("the run asks");
    let decide = |body: Value| {
        request(
            "POST",
            &format!("/api/approvals/{}/decision", approval.id),
            OWNER_ORIGIN,
            Some(body),
        )
    };

    let first = app
        .clone()
        .oneshot(decide(json!({"decision": "allow_once", "revision": 1})))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["cache-control"], "no-store");
    let first = json_body(first).await;
    assert_eq!(first["approval"]["status"], "allowed");
    assert_eq!(first["approval"]["revision"], 2);
    let replay = app
        .clone()
        .oneshot(decide(json!({"decision": "allow_once", "revision": 1})))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await, first);
    let other = app
        .clone()
        .oneshot(decide(json!({"decision": "deny", "revision": 2})))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(other).await["error"], APPROVAL_ALREADY_RESOLVED);

    for _ in 0..500 {
        if state
            .read()
            .await
            .runs
            .get(&run_id)
            .is_some_and(|run| run.status == RunStatus::Completed)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the allowed run never completed");
}

#[tokio::test]
async fn a_decision_needs_the_current_revision_a_short_note_and_a_fitting_matcher() {
    let (state, agent) = daemon();
    let waiting = pending(&agent, "chat:plans", "run_a", 10);
    state.write().await.approvals.insert(waiting.clone());
    let app = router(state.clone(), DaemonConfig::default());
    let uri = format!("/api/approvals/{}/decision", waiting.id);
    for (body, status, message) in [
        (
            json!({"decision": "allow_once", "revision": 2}),
            StatusCode::CONFLICT,
            APPROVAL_REVISION_STALE,
        ),
        (
            json!({"decision": "deny", "note": "x".repeat(1_001), "revision": 1}),
            StatusCode::BAD_REQUEST,
            APPROVAL_NOTE_TOO_LONG,
        ),
        (
            json!({"decision": "allow_always", "matcher": {"kind": "path_glob", "value": "**"}, "revision": 1}),
            StatusCode::BAD_REQUEST,
            MATCHER_KIND_NOT_FOR_TOOL,
        ),
        (
            json!({"decision": "allow_once", "revision": 1, "extra": true}),
            StatusCode::BAD_REQUEST,
            "request body must be valid JSON",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request("POST", &uri, OWNER_ORIGIN, Some(body.clone())))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{body}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message, "{body}");
    }
    assert!(state
        .read()
        .await
        .approvals
        .get(&waiting.id)
        .unwrap()
        .is_pending());
    let missing = app
        .oneshot(request(
            "POST",
            "/api/approvals/apr_missing/decision",
            OWNER_ORIGIN,
            Some(json!({"decision": "allow_once", "revision": 1})),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_expired_or_mirrored_approval_answers_from_its_record() {
    let (state, agent) = daemon();
    let mut expired = pending(&agent, "chat:plans", "run_a", 10);
    expired.resolve(
        ApprovalStatus::Expired,
        ApprovalResolution {
            decision: None,
            note: None,
            matcher: None,
            rule_id: None,
            resolved_by: ResolvedBy::Restart,
            resolved_at_ms: 20,
        },
    );
    let mirrored = decided(&agent, "chat:plans", 30, "done");
    let mut expired_mirrored = pending(&agent, "chat:plans", "run_b", 40);
    expired_mirrored.resolve(
        ApprovalStatus::Expired,
        ApprovalResolution {
            decision: None,
            note: None,
            matcher: None,
            rule_id: None,
            resolved_by: ResolvedBy::Restart,
            resolved_at_ms: 50,
        },
    );
    state.write().await.approvals.insert(expired.clone());
    let history = state.read().await.history.clone();
    history
        .store()
        .upsert_approvals(&[mirrored.clone(), expired_mirrored.clone()])
        .await
        .unwrap();
    let app = router(state, DaemonConfig::default());
    let decide = |id: &str, body: Value| {
        request(
            "POST",
            &format!("/api/approvals/{id}/decision"),
            OWNER_ORIGIN,
            Some(body),
        )
    };

    let late = app
        .clone()
        .oneshot(decide(
            &expired.id,
            json!({"decision": "allow_once", "revision": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(late).await["error"], APPROVAL_ALREADY_RESOLVED);

    let late_mirrored = app
        .clone()
        .oneshot(decide(
            &expired_mirrored.id,
            json!({"decision": "allow_once", "revision": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(late_mirrored.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(late_mirrored).await["error"],
        APPROVAL_ALREADY_RESOLVED
    );

    let replay = app
        .clone()
        .oneshot(decide(
            &mirrored.id,
            json!({"decision": "allow_once", "revision": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(
        replay.status(),
        StatusCode::OK,
        "answered from the history store"
    );
    assert_eq!(
        json_body(replay).await["approval"]["id"],
        mirrored.id.as_str()
    );
    let other = app
        .oneshot(decide(
            &mirrored.id,
            json!({"decision": "deny", "revision": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_policy_round_trips_and_is_saved() {
    let (state, agent) = daemon();
    let app = router(state.clone(), DaemonConfig::default());
    let uri = format!("/api/agents/{agent}/approval-policy");

    let initial = json_body(
        app.clone()
            .oneshot(request("GET", &uri, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        initial["policy"],
        json!({"write": "allow", "exec": "ask", "network": "allow", "delegate": "allow"})
    );

    let strict = json!({"write": "ask", "exec": "deny", "network": "allow", "delegate": "ask"});
    let put = app
        .clone()
        .oneshot(request("PUT", &uri, OWNER_ORIGIN, Some(strict.clone())))
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(put.headers()["cache-control"], "no-store");
    assert_eq!(json_body(put).await["policy"], strict);
    let read = json_body(
        app.clone()
            .oneshot(request("GET", &uri, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(read["policy"], strict);
    let saved = state.read().await.control_plane_snapshot();
    assert_eq!(saved.approval_policies[0].agent_id, agent);
    assert_eq!(saved.approval_policies[0].policy.exec, PolicyAction::Deny);

    let partial = app
        .clone()
        .oneshot(request(
            "PUT",
            &uri,
            OWNER_ORIGIN,
            Some(json!({"write": "ask"})),
        ))
        .await
        .unwrap();
    assert_eq!(partial.status(), StatusCode::BAD_REQUEST);
    let unknown = app
        .oneshot(request(
            "GET",
            "/api/agents/agent-missing/approval-policy",
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_helper_has_no_policy_or_rules_of_its_own() {
    let (state, agent) = daemon();
    let helper = {
        let mut guard = state.write().await;
        let mut config = test_config("helper");
        let settings = config.settings.as_mut().unwrap();
        settings
            .additional
            .insert("workspaceRole".into(), DataValue::String("helper".into()));
        settings
            .additional
            .insert("parentAgentId".into(), DataValue::String(agent.clone()));
        guard.approvals.set_policy(
            &agent,
            ApprovalPolicy::default().with(RiskClass::Network, PolicyAction::Deny),
        );
        guard.create_agent(config).unwrap().state.id
    };
    let app = router(state, DaemonConfig::default());

    let read = json_body(
        app.clone()
            .oneshot(request(
                "GET",
                &format!("/api/agents/{helper}/approval-policy"),
                OWNER_ORIGIN,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(read["policy"]["network"], "deny", "the companion's policy");
    for (method, uri, body) in [
        (
            "PUT",
            format!("/api/agents/{helper}/approval-policy"),
            json!({"write": "allow", "exec": "allow", "network": "allow", "delegate": "allow"}),
        ),
        (
            "POST",
            format!("/api/agents/{helper}/approval-rules"),
            json!({"tool": "memory_add", "matcher": {"kind": "any"}}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, &uri, OWNER_ORIGIN, Some(body)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{method} {uri}");
        assert_eq!(
            json_body(response).await["error"],
            HELPERS_USE_COMPANION_APPROVALS
        );
    }
}

#[tokio::test]
async fn rules_are_created_listed_and_deleted() {
    let (state, agent) = daemon();
    let app = router(state, DaemonConfig::default());
    let rules = format!("/api/agents/{agent}/approval-rules");
    let body =
        json!({"tool": "bash", "matcher": {"kind": "command_prefix", "value": "  git   status "}});

    let created = app
        .clone()
        .oneshot(request("POST", &rules, OWNER_ORIGIN, Some(body.clone())))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = json_body(created).await["rule"].clone();
    assert_eq!(created["tool"], "bash");
    assert_eq!(
        created["matcher"],
        json!({"kind": "command_prefix", "value": "git status"})
    );
    assert_eq!(created["fromApprovalId"], Value::Null);
    let again = app
        .clone()
        .oneshot(request("POST", &rules, OWNER_ORIGIN, Some(body)))
        .await
        .unwrap();
    assert_eq!(
        again.status(),
        StatusCode::OK,
        "an identical rule is returned"
    );
    assert_eq!(json_body(again).await["rule"]["id"], created["id"]);

    let listed = json_body(
        app.clone()
            .oneshot(request("GET", &rules, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["rules"].as_array().unwrap().len(), 1);
    let tools = listed["tools"].as_array().unwrap();
    assert!(tools.contains(&json!({
        "name": "bash",
        "class": "exec",
        "matcherKinds": ["command_prefix", "any"]
    })));
    assert!(!tools.iter().any(|tool| tool["class"] == "read"));

    let delete = format!("{rules}/{}", created["id"].as_str().unwrap());
    let deleted = app
        .clone()
        .oneshot(request("DELETE", &delete, OWNER_ORIGIN, None))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(json_body(deleted).await, json!({"deleted": true}));
    let gone = app
        .oneshot(request("DELETE", &delete, OWNER_ORIGIN, None))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rules_refuse_unknown_and_read_tools_bad_matchers_and_the_101st() {
    let (state, agent) = daemon();
    let app = router(state, DaemonConfig::default());
    let rules = format!("/api/agents/{agent}/approval-rules");
    for (body, status, message) in [
        (
            json!({"tool": "teleport", "matcher": {"kind": "any"}}),
            StatusCode::BAD_REQUEST,
            UNKNOWN_RULE_TOOL,
        ),
        (
            json!({"tool": "calculate", "matcher": {"kind": "any"}}),
            StatusCode::BAD_REQUEST,
            READ_TOOLS_NEED_NO_RULE,
        ),
        (
            json!({"tool": "bash", "matcher": {"kind": "path_glob", "value": "**"}}),
            StatusCode::BAD_REQUEST,
            MATCHER_KIND_NOT_FOR_TOOL,
        ),
        (
            json!({"tool": "bash", "matcher": {"kind": "command_prefix", "value": "git; rm"}}),
            StatusCode::BAD_REQUEST,
            MATCHER_VALUE_INVALID,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request("POST", &rules, OWNER_ORIGIN, Some(body.clone())))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{body}");
        assert_eq!(json_body(response).await["error"], message, "{body}");
    }
    for index in 0..MAX_APPROVAL_RULES_PER_AGENT {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &rules,
                OWNER_ORIGIN,
                Some(json!({"tool": "web_fetch", "matcher": {"kind": "domain", "value": format!("d{index}.example")}})),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }
    let full = app
        .oneshot(request(
            "POST",
            &rules,
            OWNER_ORIGIN,
            Some(json!({"tool": "web_fetch", "matcher": {"kind": "any"}})),
        ))
        .await
        .unwrap();
    assert_eq!(full.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(full).await["error"], TOO_MANY_RULES);
}

/// A path a JSON control-plane save cannot write to: a directory where the
/// store expects a file.
fn invalid_snapshot_directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "anima-approval-route-invalid-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[tokio::test]
async fn a_failed_save_reverts_a_policy_change_a_new_rule_and_a_removed_rule() {
    use crate::control_plane_store::ControlPlaneStoreConfig;

    let (state, agent) = daemon();
    let app = router(state.clone(), DaemonConfig::default());
    let policy = format!("/api/agents/{agent}/approval-policy");
    let rules = format!("/api/agents/{agent}/approval-rules");
    let kept =
        json!({"tool": "bash", "matcher": {"kind": "command_prefix", "value": "git status"}});
    let created = app
        .clone()
        .oneshot(request("POST", &rules, OWNER_ORIGIN, Some(kept)))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let kept_id = json_body(created).await["rule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let old_policy =
        json!({"write": "allow", "exec": "ask", "network": "allow", "delegate": "allow"});

    let invalid_path = invalid_snapshot_directory();
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid_path.clone())));

    let strict = json!({"write": "ask", "exec": "deny", "network": "deny", "delegate": "ask"});
    let new_rule =
        json!({"tool": "bash", "matcher": {"kind": "command_prefix", "value": "git log"}});
    for refused in [
        request("PUT", &policy, OWNER_ORIGIN, Some(strict)),
        request("POST", &rules, OWNER_ORIGIN, Some(new_rule)),
        request("DELETE", &format!("{rules}/{kept_id}"), OWNER_ORIGIN, None),
    ] {
        let response = app.clone().oneshot(refused).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    let read = json_body(
        app.clone()
            .oneshot(request("GET", &policy, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(read["policy"], old_policy, "the old policy stays");
    let listed = json_body(
        app.oneshot(request("GET", &rules, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    let listed = listed["rules"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "the new rule was not added");
    assert_eq!(listed[0]["id"], kept_id, "the deleted rule is still there");
    let _ = std::fs::remove_dir_all(invalid_path);
}
