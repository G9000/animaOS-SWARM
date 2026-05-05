mod support;

use anima_daemon::{app_with_configured_persistence, DaemonConfig};
use axum::http::StatusCode;
use axum::Router;
use serde_json::Value;
use support::{
    extract_json_string_field, send_empty_request, send_json_request, test_app,
    use_temp_workspace_root,
};

async fn create_agent(app: &Router, body: &str) -> (StatusCode, String) {
    send_json_request(app, "POST", "/api/agents", body).await
}

async fn run_agent(app: &Router, agent_id: &str, body: &str) -> (StatusCode, String) {
    send_json_request(app, "POST", &format!("/api/agents/{agent_id}/run"), body).await
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn extract_result_text(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("result")?
        .get("data")?
        .get("text")?
        .as_str()
        .map(ToOwned::to_owned)
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
async fn control_plane_store_recovers_agents_and_swarms_after_restart() {
    let workspace = use_temp_workspace_root("control-plane-restart");
    let control_plane_path = workspace.path().join("control-plane.json");
    let _guard = EnvVarGuard::set("ANIMAOS_RS_CONTROL_PLANE_FILE", &control_plane_path);

    let first_app = app_with_configured_persistence(DaemonConfig::default())
        .await
        .expect("first app should configure persistence");
    let (_, create_agent_response) =
        create_agent(&first_app, r#"{"name":"restored-agent","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_agent_response, "id");
    let (_, create_swarm_response) = send_json_request(
        &first_app,
        "POST",
        "/api/swarms",
        r#"{
            "strategy":"round-robin",
            "manager":{"name":"manager","model":"gpt-5.4"},
            "workers":[{"name":"worker-a","model":"gpt-5.4"}],
            "maxTurns":1
        }"#,
    )
    .await;
    let swarm_id = extract_json_string_field(&create_swarm_response, "id");

    let second_app = app_with_configured_persistence(DaemonConfig::default())
        .await
        .expect("second app should configure persistence");
    let (agent_status, agent_response) =
        send_empty_request(&second_app, "GET", &format!("/api/agents/{agent_id}")).await;
    let (swarm_status, swarm_response) =
        send_empty_request(&second_app, "GET", &format!("/api/swarms/{swarm_id}")).await;
    let (agent_run_status, agent_run_response) =
        run_agent(&second_app, &agent_id, r#"{"text":"still there?"}"#).await;
    let (swarm_run_status, swarm_run_response) = send_json_request(
        &second_app,
        "POST",
        &format!("/api/swarms/{swarm_id}/run"),
        r#"{"text":"continue after restart"}"#,
    )
    .await;

    assert_eq!(agent_status, StatusCode::OK);
    assert!(agent_response.contains("\"name\":\"restored-agent\""));
    assert_eq!(swarm_status, StatusCode::OK);
    assert!(swarm_response.contains(&format!("\"id\":\"{swarm_id}\"")));
    assert!(swarm_response.contains("\"status\":\"idle\""));
    assert_eq!(agent_run_status, StatusCode::OK);
    assert!(agent_run_response.contains("restored-agent handled task: still there?"));
    assert_eq!(swarm_run_status, StatusCode::OK);
    assert!(swarm_run_response.contains("continue after restart"));
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
async fn run_agent_accepts_task_field_alias() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (status, response) =
        run_agent(&app, &agent_id, r#"{"task":"Summarize the latest task"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"status\":\"success\""));
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

    let (run_status, run_response) = run_agent(
        &app,
        &agent_id,
        r#"{"text":"I prefer terse release summaries","metadata":{"userId":"user-42","userName":"Leo"}}"#,
    )
    .await;
    let (_, recent_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/agents/{agent_id}/memories/recent?limit=3"),
    )
    .await;
    let (relationships_status, relationships_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/relationships?entityId=user-42&targetKind=user",
    )
    .await;

    assert_eq!(run_status, StatusCode::OK);
    assert!(run_response.contains("\"status\":\"success\""));
    assert!(recent_response.contains("\"type\":\"reflection\""));
    assert!(recent_response
        .contains("evaluated response: operator handled task: I prefer terse release summaries"));
    assert!(recent_response.contains("user stated preference: I prefer terse release summaries"));
    assert_eq!(relationships_status, StatusCode::OK);
    let relationships_json: Value =
        serde_json::from_str(&relationships_response).expect("relationships response is json");
    let relationships = relationships_json["relationships"]
        .as_array()
        .expect("relationships should be an array");
    assert_eq!(relationships.len(), 1);
    let relationship = &relationships[0];
    assert_eq!(relationship["targetKind"], "user");
    assert_eq!(relationship["targetAgentId"], "user-42");
    assert_eq!(relationship["relationshipType"], "responds_to");
    assert!(relationship["tags"]
        .as_array()
        .expect("relationship tags should be an array")
        .iter()
        .any(|tag| tag == "relation:communication_preference"));
    let relationship_id = relationship["id"]
        .as_str()
        .expect("relationship should include id");
    let evidence_memory_ids = relationship["evidenceMemoryIds"]
        .as_array()
        .expect("relationship should include evidence memory ids");
    assert!(evidence_memory_ids.len() >= 2);
    let evidence_memory_id = evidence_memory_ids[0]
        .as_str()
        .expect("evidence memory id should be a string");

    let (recall_status, recall_response) = send_empty_request(
        &app,
        "GET",
        "/api/memories/recall?q=no-keyword-match&entityId=user-42&recentLimit=0&limit=5",
    )
    .await;
    assert_eq!(recall_status, StatusCode::OK);
    let recall_json: Value =
        serde_json::from_str(&recall_response).expect("recall response is json");
    let recall_results = recall_json["results"]
        .as_array()
        .expect("recall results should be an array");
    assert!(recall_results.iter().any(|result| {
        result["memory"]["id"] == evidence_memory_id
            && result["relationshipScore"].as_f64().unwrap_or_default() > 0.0
    }));

    let (trace_status, trace_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/memories/{evidence_memory_id}/trace"),
    )
    .await;
    assert_eq!(trace_status, StatusCode::OK);
    let trace_json: Value = serde_json::from_str(&trace_response).expect("trace response is json");
    assert!(trace_json["relationships"]
        .as_array()
        .expect("trace relationships should be an array")
        .iter()
        .any(|trace_relationship| trace_relationship["id"] == relationship_id));
    assert!(trace_json["entities"]
        .as_array()
        .expect("trace entities should be an array")
        .iter()
        .any(|entity| entity["id"] == "user-42"));
}

#[tokio::test]
async fn run_agent_without_user_metadata_does_not_create_user_relationship() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (run_status, _) = run_agent(&app, &agent_id, r#"{"text":"Reflect without user"}"#).await;
    let (recent_status, recent_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/agents/{agent_id}/memories/recent?limit=3"),
    )
    .await;
    let (relationships_status, relationships_response) =
        send_empty_request(&app, "GET", "/api/memories/relationships?targetKind=user").await;

    assert_eq!(run_status, StatusCode::OK);
    assert_eq!(recent_status, StatusCode::OK);
    assert!(recent_response.contains("\"type\":\"reflection\""));
    assert_eq!(relationships_status, StatusCode::OK);
    assert!(relationships_response.contains("\"relationships\":[]"));
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
    assert!(response.contains("\"messages\":[{"));
    assert!(response.contains("\"role\":\"user\""));
    assert!(response.contains("\"role\":\"assistant\""));
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
async fn delete_agent_removes_runtime_and_returns_deleted_flag() {
    let app = test_app();
    let (_, create_response) = create_agent(&app, r#"{"name":"operator","model":"gpt-5.4"}"#).await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (delete_status, delete_response) =
        send_empty_request(&app, "DELETE", &format!("/api/agents/{agent_id}")).await;
    let (get_status, get_response) =
        send_empty_request(&app, "GET", &format!("/api/agents/{agent_id}")).await;

    assert_eq!(delete_status, StatusCode::OK);
    assert!(delete_response.contains("\"deleted\":true"));
    assert_eq!(get_status, StatusCode::NOT_FOUND);
    assert!(get_response.contains("\"error\":\"not found\""));
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

    let first_text = extract_result_text(&first_run).expect("first run should return text");
    let second_text = extract_result_text(&second_run).expect("second run should return text");

    assert!(first_text.contains("alpha prior answer"));
    assert!(second_text.contains("beta prior answer"));
    assert!(!second_text.contains("alpha prior answer"));
}

#[tokio::test]
async fn run_agent_executes_todo_write_and_read_round_trip() {
    let workspace_root = use_temp_workspace_root("agent-todo");
    let app = test_app();
    let (_, create_response) = create_agent(
        &app,
        r#"{"name":"reviewer","model":"gpt-5.4","tools":[{"name":"todo_write","description":"Write todos","parameters":{"todos":{"type":"array"}}},{"name":"todo_read","description":"Read todos","parameters":{}}]}"#,
    )
    .await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (write_status, write_response) =
        run_agent(&app, &agent_id, r#"{"text":"plan release patch"}"#).await;
    let (read_status, read_response) = run_agent(&app, &agent_id, r#"{"text":"read todos"}"#).await;

    assert_eq!(write_status, StatusCode::OK);
    assert!(write_response.contains("Todos updated (1 completed, 1 in progress, 1 pending)."));
    assert_eq!(read_status, StatusCode::OK);
    assert!(read_response.contains("[x] 1. [completed] Inspect release patch"));
    assert!(read_response.contains("[>] 2. [in_progress] Implement release patch"));
    assert!(read_response.contains("[ ] 3. [pending] Validate release patch"));
    assert!(
        workspace_root
            .path()
            .join(".animaos-swarm")
            .join("todos.json")
            .exists(),
        "todo_write should persist the todo list inside the temp workspace"
    );
}

#[tokio::test]
async fn run_agent_executes_filesystem_tools_round_trip() {
    let workspace_root = use_temp_workspace_root("agent-files");
    let app = test_app();
    let (_, create_response) = create_agent(
        &app,
        r#"{"name":"reviewer","model":"gpt-5.4","tools":[{"name":"write_file","description":"Write file","parameters":{"file_path":{"type":"string"},"content":{"type":"string"}}},{"name":"read_file","description":"Read file","parameters":{"file_path":{"type":"string"}}},{"name":"list_dir","description":"List directory","parameters":{"path":{"type":"string"}}}]}"#,
    )
    .await;
    let agent_id = extract_json_string_field(&create_response, "id");

    let (write_status, write_response) =
        run_agent(&app, &agent_id, r#"{"text":"write file release patch"}"#).await;
    let (read_status, read_response) =
        run_agent(&app, &agent_id, r#"{"text":"read file release patch"}"#).await;
    let (list_status, list_response) = run_agent(&app, &agent_id, r#"{"text":"list notes"}"#).await;

    assert_eq!(write_status, StatusCode::OK);
    assert!(write_response.contains("Wrote 23 chars to notes/release-patch.txt"));
    assert_eq!(read_status, StatusCode::OK);
    assert!(read_response.contains("notes for release patch"));
    assert!(read_response.contains("1| notes for release patch"));
    assert_eq!(list_status, StatusCode::OK);
    assert!(list_response.contains("[file] release-patch.txt"));
    assert!(
        workspace_root
            .path()
            .join("notes")
            .join("release-patch.txt")
            .exists(),
        "write_file should persist the release patch file inside the temp workspace"
    );
}
