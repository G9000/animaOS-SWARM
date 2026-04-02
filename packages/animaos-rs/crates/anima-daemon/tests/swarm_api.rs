use anima_daemon::app as daemon_app;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use futures::{pin_mut, StreamExt};
use tower::util::ServiceExt;

fn test_app() -> Router {
    daemon_app()
}

async fn send_request(app: &Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.expect("app responds");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    (
        status,
        std::str::from_utf8(&body)
            .expect("body is utf-8")
            .to_string(),
    )
}

async fn send_json_request(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    send_request(
        app,
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request builds"),
    )
    .await
}

async fn send_empty_request(app: &Router, method: &str, uri: &str) -> (StatusCode, String) {
    send_request(
        app,
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

async fn create_swarm(app: &Router) -> (StatusCode, String) {
    send_json_request(
        app,
        "POST",
        "/api/swarms",
        r#"{
            "strategy":"round-robin",
            "manager":{"name":"manager","model":"gpt-5.4"},
            "workers":[{"name":"worker-a","model":"gpt-5.4"}],
            "maxTurns":2
        }"#,
    )
    .await
}

async fn run_swarm(app: &Router, swarm_id: &str, body: &str) -> (StatusCode, String) {
    send_json_request(app, "POST", &format!("/api/swarms/{swarm_id}/run"), body).await
}

fn extract_json_string_field(response: &str, field: &str) -> String {
    let needle = format!("\"{field}\":\"");
    let start = response
        .find(&needle)
        .map(|index| index + needle.len())
        .expect("field should exist");
    let rest = &response[start..];
    let end = rest.find('"').expect("field should terminate");
    rest[..end].to_string()
}

#[tokio::test]
async fn create_swarm_returns_created_idle_snapshot() {
    let app = test_app();

    let (status, response) = create_swarm(&app).await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"swarm\":{"));
    assert!(response.contains("\"status\":\"idle\""));
    assert!(response.contains("\"agentIds\":["));
}

#[tokio::test]
async fn run_swarm_returns_result_and_updates_swarm_state() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let (status, response) = run_swarm(&app, &swarm_id, r#"{"text":"Coordinate the patch"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains(&format!("\"id\":\"{swarm_id}\"")));
    assert!(response.contains("\"result\":{"));
    assert!(response.contains("\"status\":\"success\""));
    assert!(response.contains("[manager]: manager handled task: Coordinate the patch"));
    assert!(response.contains(
        "[worker-a]: worker-a handled task: Continue working on this task: Coordinate the patch"
    ));
}

#[tokio::test]
async fn get_swarm_returns_latest_state_snapshot() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let _ = run_swarm(&app, &swarm_id, r#"{"text":"Inspect the swarm state"}"#).await;
    let (status, response) =
        send_empty_request(&app, "GET", &format!("/api/swarms/{swarm_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains(&format!("\"id\":\"{swarm_id}\"")));
    assert!(response.contains("\"status\":\"idle\""));
    assert!(response.contains("\"results\":["));
    assert!(response.contains("Inspect the swarm state"));
}

#[tokio::test]
async fn swarm_event_stream_emits_running_and_completed_events() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/swarms/{swarm_id}/events"))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("event stream responds");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .expect("content-type should exist"),
        "text/event-stream"
    );

    let run_handle = {
        let app = app.clone();
        let swarm_id = swarm_id.clone();
        tokio::spawn(async move {
            run_swarm(&app, &swarm_id, r#"{"text":"Stream the swarm events"}"#).await
        })
    };

    let stream = response.into_body().into_data_stream();
    pin_mut!(stream);

    let mut chunks = String::new();
    for _ in 0..256 {
        match futures::poll!(stream.next()) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                chunks.push_str(std::str::from_utf8(&bytes).expect("chunk should be utf-8"));
                if chunks.contains("event: swarm:running")
                    && chunks.contains("event: swarm:completed")
                {
                    break;
                }
            }
            std::task::Poll::Ready(Some(Err(error))) => panic!("stream errored: {error}"),
            std::task::Poll::Ready(None) => break,
            std::task::Poll::Pending => tokio::task::yield_now().await,
        }
    }

    let (run_status, run_response) = run_handle.await.expect("run task should finish");

    assert_eq!(run_status, StatusCode::OK);
    assert!(run_response.contains("\"status\":\"success\""));
    assert!(chunks.contains("event: swarm:running"));
    assert!(chunks.contains("event: swarm:completed"));
    assert!(chunks.contains(&format!("\"swarmId\":\"{swarm_id}\"")));
}
