use super::*;

#[tokio::test]
async fn capability_inventory_matches_registered_tools_without_granting_access() {
    let state = Arc::new(RwLock::new(DaemonState::new()));
    let expected = state.read().await.tool_registry.tool_names();
    let app = router(state.clone(), DaemonConfig::default());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/capabilities")
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let actual: Vec<_> = value["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["persistence"]["controlPlane"], "ephemeral");
    assert_eq!(value["persistence"]["memory"], "ephemeral");
    assert_eq!(value["persistence"]["executionJournal"], false);
    assert!(value["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["status"] == "planned"));
    assert!(state.read().await.list_agents().is_empty());
    let search = value["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "exa_search")
        .unwrap();
    assert!(!search["requirements"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn capability_inventory_rejects_untrusted_origins() {
    let app = router(
        Arc::new(RwLock::new(DaemonState::new())),
        DaemonConfig::default(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/capabilities")
                .header("host", "127.0.0.1:8080")
                .header("origin", "https://untrusted.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
