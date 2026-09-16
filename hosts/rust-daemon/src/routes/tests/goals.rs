use super::*;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    value: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn goal_http_budget_reservations_status_and_persistence() {
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
    let path = std::env::temp_dir().join(format!("anima-goals-http-{}.json", uuid::Uuid::new_v4()));
    let store = ControlPlaneStoreConfig::Json(path.clone());
    let mut daemon = DaemonState::new();
    daemon.set_control_plane_store(Some(store.clone()));
    let agent = daemon
        .create_agent(test_config("goal-http"))
        .unwrap()
        .state
        .id;
    let app = router(Arc::new(RwLock::new(daemon)), DaemonConfig::default());
    let input = serde_json::json!({"title":"Ship a brief","objective":"Create an accepted brief","requestKey":"goal","maxAttempts":1});
    let (status, goal) = call(&app, "POST", "/api/goals", input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(goal["remainingAttempts"], 1);
    let (_, repeated) = call(&app, "POST", "/api/goals", input).await;
    assert_eq!(goal["id"], repeated["id"]);
    let id = goal["id"].as_str().unwrap();
    let jobs_uri = format!("/api/agents/{agent}/jobs");
    let (_, job) = call(
        &app,
        "POST",
        &jobs_uri,
        serde_json::json!({"title":"Write","prompt":"Write brief","requestKey":"job1","goalId":id}),
    )
    .await;
    assert_eq!(job["goalId"], id);
    let (status, _) = call(
        &app,
        "POST",
        &jobs_uri,
        serde_json::json!({"title":"Write","prompt":"Write brief","requestKey":"job2","goalId":id}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, goals) = call(&app, "GET", "/api/goals", serde_json::Value::Null).await;
    assert_eq!(goals["goals"][0]["reservedAttempts"], 1);
    assert_eq!(goals["goals"][0]["consumedAttempts"], 0);
    let status_uri = format!("/api/goals/{id}/status");
    assert_eq!(
        call(
            &app,
            "POST",
            &status_uri,
            serde_json::json!({"revision":999,"status":"paused"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, paused) = call(
        &app,
        "POST",
        &status_uri,
        serde_json::json!({"revision":goal["revision"],"status":"paused"}),
    )
    .await;
    assert_eq!(paused["status"], "paused");
    assert_eq!(
        call(
            &app,
            "POST",
            &status_uri,
            serde_json::json!({"revision":paused["revision"],"status":"completed"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, linked) = call(
        &app,
        "GET",
        &format!("/api/goals/{id}/jobs"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(linked["jobs"][0]["id"], job["id"]);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{jobs_uri}/{}/cancel", job["id"].as_str().unwrap()),
            serde_json::json!({"revision":job["revision"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, goals) = call(&app, "GET", "/api/goals", serde_json::Value::Null).await;
    assert_eq!(goals["goals"][0]["remainingAttempts"], 1);
    let snapshot = load_control_plane_snapshot(&store).await.unwrap().unwrap();
    let mut restored = DaemonState::new();
    restored.restore_control_plane_snapshot(snapshot).unwrap();
    assert_eq!(restored.goals.len(), 1);
    std::fs::remove_file(path).unwrap();
}
