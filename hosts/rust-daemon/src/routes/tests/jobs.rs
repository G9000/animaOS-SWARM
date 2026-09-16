use super::*;

#[tokio::test]
async fn goal_routes_authorize_before_body_and_lookup() {
    let app = router(
        Arc::new(RwLock::new(DaemonState::new())),
        DaemonConfig::default(),
    );
    for (method, path) in [
        ("GET", "/api/goals"),
        ("POST", "/api/goals"),
        ("GET", "/api/goals/missing/jobs"),
        ("POST", "/api/goals/missing/status"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", "127.0.0.1:8080")
                    .header("origin", "https://untrusted.example")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn job_supervision_http_requires_owner_revision_and_persists_decisions() {
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
    let path = std::env::temp_dir().join(format!(
        "anima-supervision-http-{}.json",
        uuid::Uuid::new_v4()
    ));
    let store = ControlPlaneStoreConfig::Json(path.clone());
    let mut daemon = DaemonState::new();
    daemon.set_control_plane_store(Some(store.clone()));
    let agent = daemon
        .create_agent(test_config("supervision-http"))
        .unwrap()
        .state
        .id;
    let state = Arc::new(RwLock::new(daemon));
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/jobs");
    let request = |uri: &str, value: serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "127.0.0.1:8080")
            .header("origin", "http://localhost:4200")
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap()
    };
    let response = app
        .clone()
        .oneshot(request(
            &base,
            serde_json::json!({
                "title":"Proposal", "prompt":"Prepare a brief", "requestKey":"proposal",
                "maxAttempts":1, "requiresApproval":true
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let job: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(job["status"], "awaiting_approval");
    assert_eq!(job["maxAttempts"], 1);
    let id = job["id"].as_str().unwrap();
    let approve = format!("{base}/{id}/approve");
    let response = app
        .clone()
        .oneshot(request(&approve, serde_json::json!({"revision":99})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let response = app
        .clone()
        .oneshot(request(
            &approve,
            serde_json::json!({"revision":job["revision"]}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let saved = load_control_plane_snapshot(&store).await.unwrap().unwrap();
    assert_eq!(saved.jobs[0].status, crate::jobs::AgentJobStatus::Queued);
    // Model execution is covered by the service suite. Seed its settled result
    // here to verify the owner's HTTP review contract independently.
    let revision = {
        let mut state = state.write().await;
        let current = state.jobs.get_mut(id).unwrap();
        current.status = crate::jobs::AgentJobStatus::Completed;
        current.attempt = 1;
        current.started_at_ms = Some(current.updated_at_ms);
        current.finished_at_ms = Some(current.updated_at_ms);
        current.result = Some("Saved brief".into());
        current.revision
    };
    let review = format!("{base}/{id}/review");
    let response = app
        .clone()
        .oneshot(request(
            &review,
            serde_json::json!({
                "revision":revision,"decision":"changes_requested","note":""
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(request(
            &review,
            serde_json::json!({
                "revision":revision,"decision":"accepted","note":"Ready"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let output: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["attempts"][0]["result"], "Saved brief");
    assert_eq!(output["attempts"][0]["review"]["decision"], "accepted");
    let response = app.oneshot(request(&review, serde_json::json!({"revision":revision,"decision":"changes_requested","note":"Changed mind"}))).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let saved = load_control_plane_snapshot(&store).await.unwrap().unwrap();
    let saved_json = serde_json::to_value(&saved.jobs[0]).unwrap();
    assert_eq!(saved_json["attempts"][0]["review"]["note"], "Ready");
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn job_http_contract_persists_deduplicates_and_checks_revisions() {
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
    let path = std::env::temp_dir().join(format!("anima-job-http-{}.json", uuid::Uuid::new_v4()));
    let config = ControlPlaneStoreConfig::Json(path.clone());
    let mut daemon = DaemonState::new();
    daemon.set_control_plane_store(Some(config.clone()));
    let agent = daemon
        .create_agent(test_config("job-http"))
        .unwrap()
        .state
        .id;
    let app = router(Arc::new(RwLock::new(daemon)), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/jobs");
    let request = |method: &str, uri: &str, body: serde_json::Value| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "127.0.0.1:8080")
            .header("origin", "http://localhost:4200")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let payload = serde_json::json!({"title":"Brief", "prompt":"Write brief", "requestKey":"one"});
    let response = app
        .clone()
        .oneshot(request("POST", &base, payload.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let job: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(job["status"], "queued");
    let response = app
        .clone()
        .oneshot(request("POST", &base, payload))
        .await
        .unwrap();
    let duplicate: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(duplicate["id"], job["id"]);
    let saved = load_control_plane_snapshot(&config).await.unwrap().unwrap();
    assert_eq!(saved.jobs.len(), 1);
    let cancel = format!("{base}/{}/cancel", job["id"].as_str().unwrap());
    let stale = app
        .clone()
        .oneshot(request(
            "POST",
            &cancel,
            serde_json::json!({"revision":999}),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &cancel,
            serde_json::json!({"revision":job["revision"]}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(request("GET", &base, serde_json::Value::Null))
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(list["jobs"][0]["status"], "cancelled");
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn job_routes_authorize_before_parsing_or_agent_lookup() {
    let app = router(
        Arc::new(RwLock::new(DaemonState::new())),
        DaemonConfig::default(),
    );
    for (method, path) in [
        ("GET", "/api/agents/missing/jobs"),
        ("POST", "/api/agents/missing/jobs"),
        ("POST", "/api/agents/missing/jobs/missing/retry"),
        ("POST", "/api/agents/missing/jobs/missing/cancel"),
        ("POST", "/api/agents/missing/jobs/missing/approve"),
        ("POST", "/api/agents/missing/jobs/missing/review"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", "127.0.0.1:8080")
                    .header("origin", "https://untrusted.example")
                    .body(Body::from("not valid json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn job_queue_requires_persistence_and_does_not_invoke_an_agent() {
    let mut daemon = DaemonState::new();
    let agent = daemon.create_agent(test_config("job-owner")).unwrap();
    let state = Arc::new(RwLock::new(daemon));
    let app = router(state.clone(), DaemonConfig::default());
    let response = app.oneshot(Request::builder().method("POST")
        .uri(format!("/api/agents/{}/jobs", agent.state.id))
        .header("host", "127.0.0.1:8080").header("origin", "http://localhost:4200")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"title":"First brief","prompt":"Prepare a product brief","requestKey":"first-brief"}"#)).unwrap())
        .await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        state
            .read()
            .await
            .get_agent(&agent.state.id)
            .unwrap()
            .messages
            .len(),
        0
    );
}
