use super::*;

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
