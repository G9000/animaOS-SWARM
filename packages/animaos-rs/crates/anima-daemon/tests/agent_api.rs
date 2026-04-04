use anima_daemon::app as daemon_app;
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

async fn create_agent(app: &Router, body: &str) -> (StatusCode, String) {
    send_json_request(app, "POST", "/api/agents", body).await
}

async fn run_agent(app: &Router, agent_id: &str, body: &str) -> (StatusCode, String) {
    send_json_request(app, "POST", &format!("/api/agents/{agent_id}/run"), body).await
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
async fn create_agent_returns_runtime_snapshot() {
    let app = test_app();
    let (status, response) = create_agent(
        &app,
        r#"{"name":"researcher","model":"gpt-5.4","bio":"Finds answers quickly","provider":"openai","knowledge":["Rust","TypeScript"],"settings":{"temperature":0.2,"maxTokens":2048}}"#,
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"name\":\"researcher\""));
    assert!(response.contains("\"status\":\"idle\""));
    assert!(response.contains("\"messageCount\":0"));
    assert!(response.contains("\"eventCount\":1"));
    assert!(response.contains("\"lastTask\":null"));
}

#[tokio::test]
async fn list_agents_includes_created_runtime() {
    let app = test_app();
    let (create_status, create_response) = create_agent(
        &app,
        r#"{"name":"writer","model":"gpt-5.4-mini","topics":["docs"]}"#,
    )
    .await;
    let (list_status, list_response) = send_empty_request(&app, "GET", "/api/agents").await;

    assert_eq!(create_status, StatusCode::CREATED);
    assert!(create_response.contains("\"name\":\"writer\""));
    assert_eq!(list_status, StatusCode::OK);
    assert!(list_response.contains("\"agents\":["));
    assert!(list_response.contains("\"name\":\"writer\""));
}

#[tokio::test]
async fn get_agent_returns_runtime_snapshot() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"planner","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");
    let (status, response) =
        send_empty_request(&app, "GET", &format!("/api/agents/{agent_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains(&format!("\"id\":\"{agent_id}\"")));
    assert!(response.contains("\"name\":\"planner\""));
}

#[tokio::test]
async fn get_agent_returns_not_found_for_unknown_runtime() {
    let app = test_app();
    let (status, response) = send_empty_request(&app, "GET", "/api/agents/agent-missing").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response.contains("\"error\":\"not found\""));
}

#[tokio::test]
async fn agent_recent_memories_filters_by_runtime_agent_id() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"reviewer","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let matching_memory = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"fact\",\"content\":\"reviewer memory\",\"importance\":0.8}}"
    );
    let other_memory = r#"{"agentId":"agent-other","agentName":"writer","type":"fact","content":"writer memory","importance":0.8}"#;

    let _ = send_json_request(&app, "POST", "/api/memories", &matching_memory).await;
    let _ = send_json_request(&app, "POST", "/api/memories", other_memory).await;
    let (status, response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/agents/{agent_id}/memories/recent"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"content\":\"reviewer memory\""));
    assert!(!response.contains("\"content\":\"writer memory\""));
}

#[tokio::test]
async fn create_agent_rejects_missing_required_fields() {
    let app = test_app();
    let (status, response) = create_agent(&app, r#"{"name":"broken-agent"}"#).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("\"error\":\"model is required\""));
}

#[tokio::test]
async fn create_agent_rejects_unknown_tools() {
    let app = test_app();
    let (status, response) = create_agent(
        &app,
        r#"{"name":"broken-agent","model":"gpt-5.4","tools":["missing_tool"]}"#,
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("\"error\":\"unknown tool: missing_tool\""));
}

#[tokio::test]
async fn create_agent_accepts_object_tools_and_plugins() {
    let app = test_app();
    let (status, response) = create_agent(
        &app,
        r#"{"name":"builder","model":"gpt-5.4","tools":[{"name":"memory_search","description":"Search docs","parameters":{"query":{"type":"string"}}}],"plugins":[{"name":"notes","description":"Workspace notes"}]}"#,
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"tools\":[{\"name\":\"memory_search\""));
    assert!(response.contains("\"plugins\":[{\"name\":\"notes\""));
}

#[tokio::test]
async fn run_agent_executes_memory_add_tool_round_trip() {
    let app = test_app();
    let (_, create_response) = create_agent(
        &app,
        r#"{"name":"reviewer","model":"gpt-5.4","tools":[{"name":"memory_add","description":"Store a memory","parameters":{"content":{"type":"string"},"type":{"type":"string"},"importance":{"type":"number"}}}]}"#,
    )
    .await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (run_status, run_response) =
        run_agent(&app, &agent_id, r#"{"text":"remember ship the patch"}"#).await;
    let (_, recent_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/agents/{agent_id}/memories/recent"),
    )
    .await;

    assert_eq!(run_status, StatusCode::OK);
    assert!(run_response.contains("\"messageCount\":4"));
    assert!(run_response.contains("stored memory"));
    assert!(recent_response.contains("\"content\":\"ship the patch\""));
    assert!(recent_response.contains("\"type\":\"fact\""));
    assert!(recent_response.contains("\"tool-memory-add\""));
}

#[tokio::test]
async fn run_agent_executes_recent_memories_tool_round_trip() {
    let app = test_app();
    let (_, create_response) = create_agent(
        &app,
        r#"{"name":"reviewer","model":"gpt-5.4","tools":[{"name":"recent_memories","description":"List recent memories","parameters":{"limit":{"type":"number"}}}]}"#,
    )
    .await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let older_memory = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"fact\",\"content\":\"older memory\",\"importance\":0.8}}"
    );
    let newer_memory = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"observation\",\"content\":\"newer memory\",\"importance\":0.8}}"
    );

    let _ = send_json_request(&app, "POST", "/api/memories", &older_memory).await;
    let _ = send_json_request(&app, "POST", "/api/memories", &newer_memory).await;
    let (status, response) = run_agent(&app, &agent_id, r#"{"text":"recent 2"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"messageCount\":4"));
    assert!(response.contains("newer memory"));
    assert!(response.contains("older memory"));
    assert!(response.find("newer memory") < response.find("older memory"));
}

#[tokio::test]
async fn run_agent_returns_task_result_and_completed_runtime_state() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");
    let (status, response) =
        run_agent(&app, &agent_id, r#"{"text":"Summarize the latest task"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"status\":\"completed\""));
    assert!(response.contains("\"status\":\"success\""));
    assert!(response.contains("\"totalTokens\":"));
    assert!(response.contains("\"text\":\"operator handled task: Summarize the latest task\""));
}

#[tokio::test]
async fn run_agent_uses_recent_memory_provider_context() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let memory_body = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"operator\",\"type\":\"fact\",\"content\":\"provider context memory\",\"importance\":0.8}}"
    );
    let _ = send_json_request(&app, "POST", "/api/memories", &memory_body).await;

    let (status, response) = run_agent(&app, &agent_id, r#"{"text":"recall context"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("operator recalled context: provider context memory"));
}

#[tokio::test]
async fn run_agent_reuses_async_runtime_state_across_provider_and_tool_paths() {
    let app = test_app();
    let (_, create_response) = create_agent(
        &app,
        r#"{"name":"operator","model":"gpt-5.4","tools":[{"name":"recent_memories","description":"List recent memories","parameters":{"limit":{"type":"number"}}}]}"#,
    )
    .await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let memory_body = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"operator\",\"type\":\"fact\",\"content\":\"provider context memory\",\"importance\":0.8}}"
    );
    let _ = send_json_request(&app, "POST", "/api/memories", &memory_body).await;

    let (_, provider_run) = run_agent(&app, &agent_id, r#"{"text":"recall context"}"#).await;
    let (_, tool_run) = run_agent(&app, &agent_id, r#"{"text":"recent 1"}"#).await;
    let (_, recent_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/agents/{agent_id}/memories/recent?limit=3"),
    )
    .await;

    assert!(provider_run.contains("operator recalled context: provider context memory"));
    assert!(tool_run.contains("provider context memory"));
    assert!(recent_response.contains("\"type\":\"reflection\""));
}

#[tokio::test]
async fn run_agent_persists_reflection_memory_from_evaluator() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (run_status, run_response) =
        run_agent(&app, &agent_id, r#"{"text":"Reflect on response"}"#).await;
    let (_, recent_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/agents/{agent_id}/memories/recent?limit=3"),
    )
    .await;

    assert_eq!(run_status, StatusCode::OK);
    assert!(run_response.contains("\"status\":\"success\""));
    assert!(recent_response.contains("\"type\":\"reflection\""));
    assert!(
        recent_response.contains("evaluated response: operator handled task: Reflect on response")
    );
}

#[tokio::test]
async fn get_agent_reflects_runtime_context_after_run() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"analyst","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");
    let _ = run_agent(&app, &agent_id, r#"{"text":"Inspect memory state"}"#).await;
    let (status, response) =
        send_empty_request(&app, "GET", &format!("/api/agents/{agent_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"messageCount\":2"));
    assert!(response.contains("\"eventCount\":8"));
    assert!(!response.contains("\"promptTokens\":0,\"completionTokens\":0,\"totalTokens\":0"));
    assert!(response.contains("\"lastTask\":{"));
}

#[tokio::test]
async fn run_agent_returns_not_found_for_unknown_runtime() {
    let app = test_app();
    let (status, response) = run_agent(&app, "agent-missing", r#"{"text":"missing agent"}"#).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response.contains("\"error\":\"not found\""));
}

#[tokio::test]
async fn run_agent_rejects_missing_text() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");
    let (status, response) = run_agent(&app, &agent_id, r#"{"metadata":{"source":"test"}}"#).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("\"error\":\"text is required\""));
}

#[tokio::test]
async fn run_agent_persists_task_result_memory() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (run_status, _) = run_agent(&app, &agent_id, r#"{"text":"Produce final answer"}"#).await;
    let (recent_status, recent_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/agents/{agent_id}/memories/recent"),
    )
    .await;

    assert_eq!(run_status, StatusCode::OK);
    assert_eq!(recent_status, StatusCode::OK);
    assert!(recent_response.contains("\"type\":\"task_result\""));
    assert!(recent_response.contains("\"content\":\"operator handled task: Produce final answer\""));
}

#[tokio::test]
async fn run_agent_executes_memory_search_tool_round_trip() {
    let app = test_app();
    let (_, create_response) = create_agent(
        &app,
        r#"{"name":"reviewer","model":"gpt-5.4","tools":[{"name":"memory_search","description":"Search memories","parameters":{"query":{"type":"string"}}}]}"#,
    )
    .await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let memory_body = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"fact\",\"content\":\"remembered prior answer\",\"importance\":0.8}}"
    );
    let _ = send_json_request(&app, "POST", "/api/memories", &memory_body).await;

    let (status, response) = run_agent(&app, &agent_id, r#"{"text":"remembered"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"messageCount\":4"));
    assert!(response.contains("remembered prior answer"));
}

#[tokio::test]
async fn run_agent_does_not_reuse_previous_tool_result_between_runs() {
    let app = test_app();
    let (_, create_response) = create_agent(
        &app,
        r#"{"name":"reviewer","model":"gpt-5.4","tools":[{"name":"memory_search","description":"Search memories","parameters":{"query":{"type":"string"}}}]}"#,
    )
    .await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let alpha_memory = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"fact\",\"content\":\"alpha prior answer\",\"importance\":0.8}}"
    );
    let beta_memory = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"fact\",\"content\":\"beta prior answer\",\"importance\":0.8}}"
    );

    let _ = send_json_request(&app, "POST", "/api/memories", &alpha_memory).await;
    let (_, first_run) = run_agent(&app, &agent_id, r#"{"text":"alpha"}"#).await;
    let _ = send_json_request(&app, "POST", "/api/memories", &beta_memory).await;
    let (_, second_run) = run_agent(&app, &agent_id, r#"{"text":"beta"}"#).await;

    assert!(first_run.contains("alpha prior answer"));
    assert!(second_run.contains("beta prior answer"));
    assert!(!second_run.contains("alpha prior answer"));
}
