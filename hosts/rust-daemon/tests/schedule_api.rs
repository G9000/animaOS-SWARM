use anima_daemon::app;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn create_agent(router: &axum::Router) -> String {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agents")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"scheduler","model":"gpt-5.4"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    json(response).await["agent"]["state"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn schedule_crud_is_agent_scoped_and_mutations_require_local_owner() {
    let router = app();
    let agent_id = create_agent(&router).await;
    let create_uri = format!("/api/agents/{agent_id}/schedules");
    let unguarded = router.clone().oneshot(Request::builder()
        .method("POST").uri(&create_uri).header("content-type", "application/json")
        .body(Body::from(r#"{"prompt":"Check status","trigger":{"type":"interval","intervalMs":60000},"target":{"type":"workspace"}}"#)).unwrap()).await.unwrap();
    assert_eq!(unguarded.status(), StatusCode::FORBIDDEN);

    let created = router.clone().oneshot(Request::builder()
        .method("POST").uri(&create_uri)
        .header("host", "127.0.0.1:8080").header("origin", "http://localhost:4200")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"prompt":"Check status","trigger":{"type":"interval","intervalMs":60000},"target":{"type":"workspace"}}"#)).unwrap()).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = json(created).await;
    let schedule_id = created["schedule"]["id"].as_str().unwrap();
    let initial_due = created["schedule"]["nextDueAtMs"].as_u64().unwrap();
    assert_eq!(created["schedule"]["agentId"], agent_id);
    assert!(created["schedule"]["nextDueAtMs"].as_u64().is_some());

    let listed = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&create_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(json(listed).await["schedules"].as_array().unwrap().len(), 1);

    let item_uri = format!("{create_uri}/{schedule_id}");
    let updated = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(&item_uri)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"prompt":"Check health"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = json(updated).await;
    assert_eq!(updated["schedule"]["prompt"], "Check health");
    assert_eq!(updated["schedule"]["nextDueAtMs"], initial_due);

    let disabled = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(&item_uri)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let disabled = json(disabled).await;
    assert_eq!(disabled["schedule"]["nextDueAtMs"], initial_due);
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let enabled = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(&item_uri)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        json(enabled).await["schedule"]["nextDueAtMs"]
            .as_u64()
            .unwrap()
            > initial_due
    );

    let deleted = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&item_uri)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
}

#[tokio::test]
async fn legacy_import_is_idempotent_and_preserves_browser_due_time() {
    let router = app();
    let agent_id = create_agent(&router).await;
    let uri = format!("/api/agents/{agent_id}/schedules/import");
    let request = || {
        Request::builder().method("POST").uri(&uri)
        .header("host", "127.0.0.1:8080").header("origin", "http://localhost:4200")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"schedules":[{"id":"legacy-one","prompt":"Check inbox","intervalSecs":60,"createdAtMs":1000,"lastRunAtMs":9000}]}"#)).unwrap()
    };

    let first = router.clone().oneshot(request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = json(first).await;
    assert_eq!(first["schedules"][0]["nextDueAtMs"], 69_000);
    let first_id = first["schedules"][0]["id"].as_str().unwrap().to_string();

    let retry = router.clone().oneshot(request()).await.unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(json(retry).await["schedules"][0]["id"], first_id);

    let list = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/agents/{agent_id}/schedules"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json(list).await["schedules"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn openapi_registers_schedule_crud_and_import_contracts() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let document = json(response).await;
    let paths = document["paths"].as_object().unwrap();
    assert!(paths["/api/agents/{agent_id}/schedules"]
        .get("get")
        .is_some());
    assert!(paths["/api/agents/{agent_id}/schedules"]
        .get("post")
        .is_some());
    assert!(paths["/api/agents/{agent_id}/schedules/{schedule_id}"]
        .get("patch")
        .is_some());
    assert!(paths["/api/agents/{agent_id}/schedules/{schedule_id}"]
        .get("delete")
        .is_some());
    assert!(paths["/api/agents/{agent_id}/schedules/import"]
        .get("post")
        .is_some());
    assert!(document["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag["name"] == "schedules"));
}
