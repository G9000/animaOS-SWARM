use anima_daemon::{app as daemon_app, app_with_config, DaemonConfig};
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
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

#[tokio::test]
async fn create_memory_returns_created_memory() {
    let app = test_app();
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"Rust daemon memory endpoint created","importance":0.8,"tags":["rust","memory"]}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories", body).await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"content\":\"Rust daemon memory endpoint created\""));
    assert!(response.contains("\"type\":\"fact\""));
}

#[tokio::test]
async fn create_memory_rejects_missing_required_fields() {
    let app = test_app();
    let body =
        r#"{"agentId":"agent-1","type":"fact","content":"missing agentName","importance":0.8}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories", body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("\"error\":\"agentName is required\""));
}

#[tokio::test]
async fn search_memories_returns_created_memory() {
    let app = test_app();
    let create_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"BM25 search should find this memory","importance":0.9,"tags":["search"]}"#;

    let (create_status, _) = send_json_request(&app, "POST", "/api/memories", create_body).await;
    let (search_status, search_response) =
        send_empty_request(&app, "GET", "/api/memories/search?q=BM25%20search").await;

    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(search_status, StatusCode::OK);
    assert!(search_response.contains("\"results\":"));
    assert!(search_response.contains("\"content\":\"BM25 search should find this memory\""));
}

#[tokio::test]
async fn search_memories_rejects_missing_query() {
    let app = test_app();
    let (status, response) = send_empty_request(&app, "GET", "/api/memories/search").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("\"error\":\"q query parameter is required\""));
}

#[tokio::test]
async fn recent_memories_returns_newest_first() {
    let app = test_app();
    let oldest_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"oldest","importance":0.4}"#;
    let newest_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"newest","importance":0.7}"#;

    let (first_status, _) = send_json_request(&app, "POST", "/api/memories", oldest_body).await;
    let (second_status, _) = send_json_request(&app, "POST", "/api/memories", newest_body).await;
    let (recent_status, recent_response) =
        send_empty_request(&app, "GET", "/api/memories/recent").await;

    assert_eq!(first_status, StatusCode::CREATED);
    assert_eq!(second_status, StatusCode::CREATED);
    assert_eq!(recent_status, StatusCode::OK);

    let newest_index = recent_response
        .find("\"content\":\"newest\"")
        .expect("recent response should contain newest");
    let oldest_index = recent_response
        .find("\"content\":\"oldest\"")
        .expect("recent response should contain oldest");
    assert!(newest_index < oldest_index);
}

#[tokio::test]
async fn search_memories_applies_agent_name_filter() {
    let app = test_app();
    let researcher_body = r#"{"agentId":"a1","agentName":"researcher","type":"fact","content":"shared topic from researcher","importance":0.8}"#;
    let writer_body = r#"{"agentId":"a2","agentName":"writer","type":"fact","content":"shared topic from writer","importance":0.8}"#;

    let (researcher_status, _) =
        send_json_request(&app, "POST", "/api/memories", researcher_body).await;
    let (writer_status, _) = send_json_request(&app, "POST", "/api/memories", writer_body).await;
    let (search_status, search_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/search?q=shared%20topic&agentName=writer",
    )
    .await;

    assert_eq!(researcher_status, StatusCode::CREATED);
    assert_eq!(writer_status, StatusCode::CREATED);
    assert_eq!(search_status, StatusCode::OK);
    assert!(search_response.contains("\"agentName\":\"writer\""));
    assert!(!search_response.contains("\"agentName\":\"researcher\""));
}

#[tokio::test]
async fn malformed_query_returns_bad_request_and_app_keeps_serving() {
    let app = test_app();

    let (bad_status, bad_response) =
        send_empty_request(&app, "GET", "/api/memories/search?q=%GG").await;
    let (good_status, good_response) = send_empty_request(&app, "GET", "/health").await;

    assert_eq!(bad_status, StatusCode::BAD_REQUEST);
    assert!(bad_response.contains("\"error\":\"malformed request\""));
    assert_eq!(good_status, StatusCode::OK);
    assert_eq!(good_response, "{\"status\":\"ok\"}");
}

#[tokio::test]
async fn create_memory_escapes_control_characters_in_json_response() {
    let app = test_app();
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"has\bbackspace\fpage","importance":0.8}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories", body).await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\\u0008"));
    assert!(response.contains("\\u000c"));
}

#[tokio::test]
async fn create_memory_accepts_surrogate_pair_unicode_escape() {
    let app = test_app();
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"launch \ud83d\ude80","importance":0.8}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories", body).await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"content\":\"launch 🚀\""));
}

#[tokio::test]
async fn create_memory_rejects_unescaped_newline_in_json_string() {
    let app = test_app();
    let body = format!(
        "{{\"agentId\":\"agent-1\",\"agentName\":\"researcher\",\"type\":\"fact\",\"content\":\"bad{}json\",\"importance\":0.8}}",
        '\n'
    );

    let (status, response) = send_json_request(&app, "POST", "/api/memories", &body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("\"error\":\"request body must be valid JSON\""));
}

#[tokio::test]
async fn app_with_config_enforces_max_request_bytes() {
    let app = app_with_config(DaemonConfig {
        max_request_bytes: 32,
    });
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"body is larger than the configured max","importance":0.8}"#;

    let (status, _) = send_json_request(&app, "POST", "/api/memories", body).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
