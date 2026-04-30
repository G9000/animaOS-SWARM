use anima_daemon::{app as daemon_app, app_with_config, DaemonConfig};
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
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

fn parse_json_body(body: &str) -> Value {
    serde_json::from_str(body).expect("response body should be valid JSON")
}

fn extract_json_string_field(body: &str, field: &str) -> String {
    parse_json_body(body)
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("response should include string field {field}"))
        .to_string()
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
async fn create_memory_accepts_scope_and_session_metadata() {
    let app = test_app();
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"room scoped note","importance":0.8,"scope":"room","roomId":"room-1","worldId":"world-1","sessionId":"session-1"}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories", body).await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"scope\":\"room\""));
    assert!(response.contains("\"roomId\":\"room-1\""));
    assert!(response.contains("\"worldId\":\"world-1\""));
    assert!(response.contains("\"sessionId\":\"session-1\""));
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
async fn search_alias_matches_memory_search_endpoint() {
    let app = test_app();
    let create_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"search alias should find this memory","importance":0.9,"tags":["search"]}"#;

    let (create_status, _) = send_json_request(&app, "POST", "/api/memories", create_body).await;
    let (search_status, search_response) =
        send_empty_request(&app, "GET", "/api/search?q=search%20alias").await;

    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(search_status, StatusCode::OK);
    assert!(search_response.contains("\"results\":"));
    assert!(search_response.contains("\"content\":\"search alias should find this memory\""));
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
async fn search_memories_filters_by_scope_and_room() {
    let app = test_app();
    let room_a_body = r#"{"agentId":"a1","agentName":"researcher","type":"fact","content":"shared planning topic","importance":0.8,"scope":"room","roomId":"room-a"}"#;
    let room_b_body = r#"{"agentId":"a1","agentName":"researcher","type":"fact","content":"shared planning topic","importance":0.8,"scope":"room","roomId":"room-b"}"#;

    let (room_a_status, _) = send_json_request(&app, "POST", "/api/memories", room_a_body).await;
    let (room_b_status, _) = send_json_request(&app, "POST", "/api/memories", room_b_body).await;
    let (search_status, search_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/search?q=planning%20topic&scope=room&roomId=room-a",
    )
    .await;

    assert_eq!(room_a_status, StatusCode::CREATED);
    assert_eq!(room_b_status, StatusCode::CREATED);
    assert_eq!(search_status, StatusCode::OK);
    assert!(search_response.contains("\"roomId\":\"room-a\""));
    assert!(!search_response.contains("\"roomId\":\"room-b\""));
}

#[tokio::test]
async fn recent_memories_filters_by_session_id() {
    let app = test_app();
    let first_body = r#"{"agentId":"a1","agentName":"researcher","type":"fact","content":"first session note","importance":0.8,"sessionId":"session-a"}"#;
    let second_body = r#"{"agentId":"a1","agentName":"researcher","type":"fact","content":"second session note","importance":0.8,"sessionId":"session-b"}"#;

    let (first_status, _) = send_json_request(&app, "POST", "/api/memories", first_body).await;
    let (second_status, _) = send_json_request(&app, "POST", "/api/memories", second_body).await;
    let (recent_status, recent_response) =
        send_empty_request(&app, "GET", "/api/memories/recent?sessionId=session-a").await;

    assert_eq!(first_status, StatusCode::CREATED);
    assert_eq!(second_status, StatusCode::CREATED);
    assert_eq!(recent_status, StatusCode::OK);
    assert!(recent_response.contains("\"content\":\"first session note\""));
    assert!(!recent_response.contains("\"content\":\"second session note\""));
}

#[tokio::test]
async fn create_and_list_memory_entities_round_trip() {
    let app = test_app();
    let body = r#"{"kind":"user","id":"user-1","name":"Leo","aliases":["operator"],"summary":"Primary operator"}"#;

    let (create_status, create_response) =
        send_json_request(&app, "POST", "/api/memories/entities", body).await;
    let (list_status, list_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/entities?kind=user&alias=operator",
    )
    .await;

    assert_eq!(create_status, StatusCode::CREATED);
    assert!(create_response.contains("\"kind\":\"user\""));
    assert!(create_response.contains("\"id\":\"user-1\""));
    assert_eq!(list_status, StatusCode::OK);
    let response = parse_json_body(&list_response);
    let entities = response["entities"]
        .as_array()
        .expect("entities should be an array");
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0]["id"], "user-1");
    assert_eq!(entities[0]["aliases"][0], "operator");
}

#[tokio::test]
async fn create_memory_entity_rejects_invalid_kind() {
    let app = test_app();
    let body = r#"{"kind":"person","id":"user-1","name":"Leo"}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories/entities", body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("endpoint kind must be one of agent, user, system, external"));
}

#[tokio::test]
async fn add_evaluated_memory_ignores_low_value_without_persisting() {
    let app = test_app();
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"ok","importance":0.05}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories/evaluated", body).await;
    let (recent_status, recent_response) =
        send_empty_request(&app, "GET", "/api/memories/recent?agentId=agent-1").await;

    assert_eq!(status, StatusCode::OK);
    let outcome = parse_json_body(&response);
    assert_eq!(outcome["evaluation"]["decision"], "ignore");
    assert!(outcome["memory"].is_null());
    assert_eq!(recent_status, StatusCode::OK);
    assert!(!recent_response.contains("\"content\":\"ok\""));
}

#[tokio::test]
async fn add_evaluated_memory_merges_duplicate_without_appending() {
    let app = test_app();
    let original_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"duplicate marker exact","importance":0.4}"#;
    let duplicate_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":" duplicate   marker exact ","importance":0.9}"#;

    let (create_status, create_response) =
        send_json_request(&app, "POST", "/api/memories", original_body).await;
    let original_id = extract_json_string_field(&create_response, "id");
    let (duplicate_status, duplicate_response) =
        send_json_request(&app, "POST", "/api/memories/evaluated", duplicate_body).await;
    let (_, recent_response) =
        send_empty_request(&app, "GET", "/api/memories/recent?agentId=agent-1&limit=5").await;

    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(duplicate_status, StatusCode::OK);
    let outcome = parse_json_body(&duplicate_response);
    assert_eq!(outcome["evaluation"]["decision"], "merge");
    assert_eq!(
        outcome["evaluation"]["duplicateMemoryId"].as_str(),
        Some(original_id.as_str())
    );
    assert!(outcome["memory"].is_null());
    assert_eq!(recent_response.matches("duplicate marker exact").count(), 1);
}

#[tokio::test]
async fn recall_memories_uses_relationship_evidence_without_recent_fallback() {
    let app = test_app();
    let memory_body = r#"{"agentId":"planner","agentName":"Planner","type":"fact","content":"Launch rollback rehearsal should happen before release","importance":0.8,"worldId":"world-1"}"#;

    let (memory_status, memory_response) =
        send_json_request(&app, "POST", "/api/memories", memory_body).await;
    let memory_id = extract_json_string_field(&memory_response, "id");
    let relationship_body = format!(
        "{{\"sourceAgentId\":\"planner\",\"sourceAgentName\":\"Planner\",\"targetKind\":\"user\",\"targetAgentId\":\"user-1\",\"targetAgentName\":\"Leo\",\"relationshipType\":\"responds_to\",\"strength\":0.9,\"confidence\":0.8,\"evidenceMemoryIds\":[\"{memory_id}\"],\"worldId\":\"world-1\"}}"
    );
    let (relationship_status, _) = send_json_request(
        &app,
        "POST",
        "/api/memories/relationships",
        &relationship_body,
    )
    .await;
    let (recall_status, recall_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/recall?q=no-keyword-match&entityId=user-1&worldId=world-1&recentLimit=0&limit=5",
    )
    .await;

    assert_eq!(memory_status, StatusCode::CREATED);
    assert_eq!(relationship_status, StatusCode::CREATED);
    assert_eq!(recall_status, StatusCode::OK);
    let response = parse_json_body(&recall_response);
    let results = response["results"]
        .as_array()
        .expect("results should be an array");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]["memory"]["id"].as_str(),
        Some(memory_id.as_str())
    );
    assert_eq!(results[0]["lexicalScore"].as_f64(), Some(0.0));
    assert_eq!(results[0]["recencyScore"].as_f64(), Some(0.0));
    assert!(results[0]["relationshipScore"].as_f64().unwrap() > 0.7);
}

#[tokio::test]
async fn recall_memories_rejects_missing_query() {
    let app = test_app();
    let (status, response) = send_empty_request(&app, "GET", "/api/memories/recall").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("q query parameter is required"));
}

#[tokio::test]
async fn create_agent_relationship_returns_relationship_edge() {
    let app = test_app();
    let body = r#"{"sourceAgentId":"planner","sourceAgentName":"Planner","targetAgentId":"critic","targetAgentName":"Critic","relationshipType":"collaborates_with","summary":"Critic pressure-tests launch plans.","strength":0.8,"confidence":0.7,"evidenceMemoryIds":["mem-1"],"worldId":"world-1"}"#;

    let (status, response) =
        send_json_request(&app, "POST", "/api/memories/relationships", body).await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"sourceKind\":\"agent\""));
    assert!(response.contains("\"sourceAgentId\":\"planner\""));
    assert!(response.contains("\"targetKind\":\"agent\""));
    assert!(response.contains("\"targetAgentId\":\"critic\""));
    assert!(response.contains("\"relationshipType\":\"collaborates_with\""));
    assert!(response.contains("\"evidenceMemoryIds\":[\"mem-1\"]"));
}

#[tokio::test]
async fn create_agent_relationship_rejects_invalid_endpoint_kind() {
    let app = test_app();
    let body = r#"{"sourceKind":"person","sourceAgentId":"planner","sourceAgentName":"Planner","targetAgentId":"critic","targetAgentName":"Critic","relationshipType":"collaborates_with"}"#;

    let (status, response) =
        send_json_request(&app, "POST", "/api/memories/relationships", body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("endpoint kind must be one of agent, user, system, external"));
}

#[tokio::test]
async fn list_agent_relationships_rejects_invalid_endpoint_kind() {
    let app = test_app();
    let (status, response) =
        send_empty_request(&app, "GET", "/api/memories/relationships?targetKind=person").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(response.contains("endpoint kind must be one of agent, user, system, external"));
}

#[tokio::test]
async fn create_agent_user_relationship_returns_user_endpoint_edge() {
    let app = test_app();
    let body = r#"{"sourceAgentId":"planner","sourceAgentName":"Planner","targetKind":"user","targetAgentId":"user-1","targetAgentName":"Leo","relationshipType":"responds_to","strength":0.6,"confidence":0.8,"evidenceMemoryIds":["mem-1"],"worldId":"world-1"}"#;

    let (status, response) =
        send_json_request(&app, "POST", "/api/memories/relationships", body).await;
    let (list_status, list_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/relationships?entityId=user-1&targetKind=user",
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("\"targetKind\":\"user\""));
    assert!(response.contains("\"targetAgentId\":\"user-1\""));
    assert_eq!(list_status, StatusCode::OK);
    assert!(list_response.contains("\"relationshipType\":\"responds_to\""));
}

#[tokio::test]
async fn list_agent_relationships_filters_by_agent_and_world() {
    let app = test_app();
    let first_body = r#"{"sourceAgentId":"planner","sourceAgentName":"Planner","targetAgentId":"critic","targetAgentName":"Critic","relationshipType":"collaborates_with","strength":0.8,"confidence":0.7,"worldId":"world-1"}"#;
    let second_body = r#"{"sourceAgentId":"writer","sourceAgentName":"Writer","targetAgentId":"researcher","targetAgentName":"Researcher","relationshipType":"hands_off_to","strength":0.8,"confidence":0.7,"worldId":"world-2"}"#;

    let (first_status, _) =
        send_json_request(&app, "POST", "/api/memories/relationships", first_body).await;
    let (second_status, _) =
        send_json_request(&app, "POST", "/api/memories/relationships", second_body).await;
    let (list_status, list_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/relationships?agentId=critic&worldId=world-1",
    )
    .await;

    assert_eq!(first_status, StatusCode::CREATED);
    assert_eq!(second_status, StatusCode::CREATED);
    assert_eq!(list_status, StatusCode::OK);
    assert!(list_response.contains("\"relationships\":"));
    assert!(list_response.contains("\"targetAgentId\":\"critic\""));
    assert!(!list_response.contains("\"targetAgentId\":\"researcher\""));
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
        ..DaemonConfig::default()
    });
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"body is larger than the configured max","importance":0.8}"#;

    let (status, response) = send_json_request(&app, "POST", "/api/memories", body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response, "{\"error\":\"malformed request\"}");
}
