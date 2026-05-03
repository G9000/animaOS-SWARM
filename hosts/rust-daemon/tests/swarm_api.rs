mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use futures::{pin_mut, StreamExt};
use serde_json::Value;
use tower::util::ServiceExt;

async fn create_swarm(app: &Router) -> (StatusCode, String) {
    create_swarm_with_body(
        app,
        r#"{
            "strategy":"round-robin",
            "manager":{"name":"manager","model":"gpt-5.4"},
            "workers":[{"name":"worker-a","model":"gpt-5.4"}],
            "maxTurns":2
        }"#,
    )
    .await
}

async fn create_swarm_with_body(app: &Router, body: &str) -> (StatusCode, String) {
    send_json_request(app, "POST", "/api/swarms", body).await
}

async fn run_swarm(app: &Router, swarm_id: &str, body: &str) -> (StatusCode, String) {
    send_json_request(app, "POST", &format!("/api/swarms/{swarm_id}/run"), body).await
}

use support::{
    extract_json_string_field, extract_json_u64_field, extract_sse_event_data, send_empty_request,
    send_json_request, test_app, use_temp_workspace_root,
};

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
async fn list_swarms_returns_registered_snapshots() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let (status, response) = send_empty_request(&app, "GET", "/api/swarms").await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"swarms\":["));
    assert!(response.contains(&format!("\"id\":\"{swarm_id}\"")));
    assert!(response.contains("\"status\":\"idle\""));
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
async fn run_swarm_accepts_task_field_alias() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let (status, response) = run_swarm(&app, &swarm_id, r#"{"task":"Coordinate the patch"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("\"status\":\"success\""));
    assert!(response.contains("[manager]: manager handled task: Coordinate the patch"));
}

#[tokio::test]
async fn send_message_can_target_configured_agent_name() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let (status, response) = run_swarm(
        &app,
        &swarm_id,
        r#"{"text":"send to worker-a: named handoff and recall inbox"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("sent message to worker-a"));
    assert!(response.contains("\"messages\":[{"));
    assert!(response.contains("\"content\":{\"text\":\"named handoff and recall inbox\""));
    assert!(response.contains("worker-a recalled swarm inbox:"));
    assert!(response.contains("named handoff and recall inbox"));

    let (relationships_status, relationships_response) = send_empty_request(
        &app,
        "GET",
        &format!(
            "/api/memories/relationships?relationshipType=hands_off_to&roomId={swarm_id}&limit=5"
        ),
    )
    .await;
    assert_eq!(relationships_status, StatusCode::OK);
    let relationships_json: Value =
        serde_json::from_str(&relationships_response).expect("relationships response is json");
    let relationships = relationships_json["relationships"]
        .as_array()
        .expect("relationships should be an array");
    let relationship = relationships
        .iter()
        .find(|relationship| {
            relationship["sourceAgentName"] == "manager"
                && relationship["targetAgentName"] == "worker-a"
                && relationship["relationshipType"] == "hands_off_to"
        })
        .expect("swarm handoff relationship should be persisted");
    assert_eq!(relationship["sourceKind"], "agent");
    assert_eq!(relationship["targetKind"], "agent");
    assert!(relationship["tags"]
        .as_array()
        .expect("relationship tags should be an array")
        .iter()
        .any(|tag| tag == "relation:handoff"));
    let relationship_id = relationship["id"]
        .as_str()
        .expect("relationship should include an id");
    let evidence_memory_id = relationship["evidenceMemoryIds"]
        .as_array()
        .expect("relationship should include evidence memory ids")
        .first()
        .and_then(|value| value.as_str())
        .expect("relationship should cite the message evidence memory");

    let (trace_status, trace_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/memories/{evidence_memory_id}/trace"),
    )
    .await;
    assert_eq!(trace_status, StatusCode::OK);
    let trace_json: Value = serde_json::from_str(&trace_response).expect("trace response is json");
    assert!(trace_json["memory"]["content"]
        .as_str()
        .expect("trace memory content should be a string")
        .contains("named handoff and recall inbox"));
    assert!(trace_json["relationships"]
        .as_array()
        .expect("trace relationships should be an array")
        .iter()
        .any(|trace_relationship| trace_relationship["id"] == relationship_id));
}

#[tokio::test]
async fn broadcast_message_persists_swarm_relationship() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let (status, response) = run_swarm(
        &app,
        &swarm_id,
        r#"{"text":"broadcast relationship map update and recall inbox"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(response.contains("broadcast message sent to swarm"));
    assert!(response.contains("worker-a recalled swarm inbox:"));
    assert!(response.contains("relationship map update and recall inbox"));

    let (relationships_status, relationships_response) = send_empty_request(
        &app,
        "GET",
        &format!(
            "/api/memories/relationships?relationshipType=broadcasts_to&roomId={swarm_id}&limit=5"
        ),
    )
    .await;
    assert_eq!(relationships_status, StatusCode::OK);
    let relationships_json: Value =
        serde_json::from_str(&relationships_response).expect("relationships response is json");
    let relationships = relationships_json["relationships"]
        .as_array()
        .expect("relationships should be an array");
    let relationship = relationships
        .iter()
        .find(|relationship| {
            relationship["sourceAgentName"] == "manager"
                && relationship["targetAgentId"] == swarm_id
                && relationship["relationshipType"] == "broadcasts_to"
        })
        .expect("swarm broadcast relationship should be persisted");
    assert_eq!(relationship["sourceKind"], "agent");
    assert_eq!(relationship["targetKind"], "system");
    assert!(relationship["tags"]
        .as_array()
        .expect("relationship tags should be an array")
        .iter()
        .any(|tag| tag == "relation:broadcast"));
    let relationship_id = relationship["id"]
        .as_str()
        .expect("relationship should include an id");
    let evidence_memory_id = relationship["evidenceMemoryIds"]
        .as_array()
        .expect("relationship should include evidence memory ids")
        .first()
        .and_then(|value| value.as_str())
        .expect("relationship should cite the broadcast evidence memory");

    let (trace_status, trace_response) = send_empty_request(
        &app,
        "GET",
        &format!("/api/memories/{evidence_memory_id}/trace"),
    )
    .await;
    assert_eq!(trace_status, StatusCode::OK);
    let trace_json: Value = serde_json::from_str(&trace_response).expect("trace response is json");
    assert!(trace_json["memory"]["content"]
        .as_str()
        .expect("trace memory content should be a string")
        .contains("relationship map update and recall inbox"));
    assert!(trace_json["relationships"]
        .as_array()
        .expect("trace relationships should be an array")
        .iter()
        .any(|trace_relationship| trace_relationship["id"] == relationship_id));
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
async fn swarm_event_stream_emits_live_agent_activity_and_lifecycle_events() {
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
                    && chunks.contains("event: task:started")
                    && chunks.contains("event: agent:tokens")
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
    assert!(chunks.contains("event: task:started"));
    assert!(chunks.contains("event: agent:tokens"));
    assert!(chunks.contains("event: swarm:completed"));
    assert!(chunks.contains(&format!("\"swarmId\":\"{swarm_id}\"")));
    assert!(chunks.contains("\"agentName\":\"worker-a\""));

    let running_data =
        extract_sse_event_data(&chunks, "swarm:running").expect("running event data exists");
    assert!(running_data.contains(&format!("\"swarmId\":\"{swarm_id}\"")));
    assert!(running_data.contains("\"status\":\"running\""));
    assert!(running_data.contains("\"result\":null"));

    let completed_data =
        extract_sse_event_data(&chunks, "swarm:completed").expect("completed event data exists");
    assert!(completed_data.contains(&format!("\"swarmId\":\"{swarm_id}\"")));
    assert!(completed_data.contains("\"result\":{"));
    assert!(completed_data.contains("\"status\":\"success\""));
}

#[tokio::test]
async fn swarm_event_stream_emits_swarm_messaging_tool_results() {
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

    let run_handle = {
        let app = app.clone();
        let swarm_id = swarm_id.clone();
        tokio::spawn(async move {
            run_swarm(
                &app,
                &swarm_id,
                r#"{"text":"send to worker-a: streamed handoff and recall inbox"}"#,
            )
            .await
        })
    };

    let stream = response.into_body().into_data_stream();
    pin_mut!(stream);

    let mut chunks = String::new();
    for _ in 0..256 {
        match futures::poll!(stream.next()) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                chunks.push_str(std::str::from_utf8(&bytes).expect("chunk should be utf-8"));
                if chunks.contains("event: swarm:message")
                    && chunks.contains("event: tool:after")
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
    assert!(run_response.contains("sent message to worker-a"));
    assert!(run_response.contains("worker-a recalled swarm inbox:"));
    assert!(run_response.contains("streamed handoff and recall inbox"));

    let swarm_message_data =
        extract_sse_event_data(&chunks, "swarm:message").expect("swarm message event data exists");
    assert!(swarm_message_data.contains(&format!("\"swarmId\":\"{swarm_id}\"")));
    assert!(swarm_message_data.contains("\"message\":{"));
    assert!(swarm_message_data.contains("\"from\":\"manager-"));
    assert!(swarm_message_data.contains("\"to\":\"worker-a-"));
    assert!(swarm_message_data.contains("\"text\":\"streamed handoff and recall inbox\""));

    let tool_after_data =
        extract_sse_event_data(&chunks, "tool:after").expect("tool after event data exists");
    assert!(tool_after_data.contains("\"toolName\":\"send_message\""));
    assert!(tool_after_data.contains("\"status\":\"success\""));
    assert!(tool_after_data.contains("\"result\":\"sent message to worker-a (worker-a-"));
}

#[tokio::test]
async fn swarm_event_stream_emits_tool_results() {
    let app = test_app();
    let (_, create_response) = create_swarm_with_body(
        &app,
        r#"{
            "strategy":"round-robin",
            "manager":{
                "name":"manager",
                "model":"gpt-5.4",
                "tools":[{
                    "name":"memory_add",
                    "description":"Store a memory",
                    "parameters":{
                        "content":{"type":"string"},
                        "type":{"type":"string"},
                        "importance":{"type":"number"}
                    }
                }]
            },
            "workers":[{"name":"worker-a","model":"gpt-5.4"}],
            "maxTurns":2
        }"#,
    )
    .await;
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

    let run_handle = {
        let app = app.clone();
        let swarm_id = swarm_id.clone();
        tokio::spawn(async move {
            run_swarm(&app, &swarm_id, r#"{"text":"remember ship the patch"}"#).await
        })
    };

    let stream = response.into_body().into_data_stream();
    pin_mut!(stream);

    let mut chunks = String::new();
    for _ in 0..256 {
        match futures::poll!(stream.next()) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                chunks.push_str(std::str::from_utf8(&bytes).expect("chunk should be utf-8"));
                if chunks.contains("event: tool:after") && chunks.contains("event: swarm:completed")
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
    assert!(run_response.contains("stored memory: ship the patch"));
    assert!(chunks.contains("event: tool:after"));

    let tool_after_data =
        extract_sse_event_data(&chunks, "tool:after").expect("tool after event data exists");
    assert!(tool_after_data.contains("\"toolName\":\"memory_add\""));
    assert!(tool_after_data.contains("\"status\":\"success\""));
    assert!(tool_after_data.contains("\"result\":\"stored memory: ship the patch\""));
}

#[tokio::test]
async fn swarm_event_stream_emits_todo_tool_results() {
    let workspace_root = use_temp_workspace_root("swarm-todo");
    let app = test_app();
    let (_, create_response) = create_swarm_with_body(
        &app,
        r#"{
            "strategy":"round-robin",
            "manager":{
                "name":"manager",
                "model":"gpt-5.4",
                "tools":[{
                    "name":"todo_write",
                    "description":"Write todos",
                    "parameters":{
                        "todos":{"type":"array"}
                    }
                }]
            },
            "workers":[{"name":"worker-a","model":"gpt-5.4"}],
            "maxTurns":2
        }"#,
    )
    .await;
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

    let run_handle = {
        let app = app.clone();
        let swarm_id = swarm_id.clone();
        tokio::spawn(
            async move { run_swarm(&app, &swarm_id, r#"{"text":"plan release patch"}"#).await },
        )
    };

    let stream = response.into_body().into_data_stream();
    pin_mut!(stream);

    let mut chunks = String::new();
    for _ in 0..256 {
        match futures::poll!(stream.next()) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                chunks.push_str(std::str::from_utf8(&bytes).expect("chunk should be utf-8"));
                if chunks.contains("event: tool:after") && chunks.contains("event: swarm:completed")
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
    assert!(run_response.contains("Todos updated (1 completed, 1 in progress, 1 pending)."));
    assert!(chunks.contains("event: tool:after"));

    let tool_after_data =
        extract_sse_event_data(&chunks, "tool:after").expect("tool after event data exists");
    assert!(tool_after_data.contains("\"toolName\":\"todo_write\""));
    assert!(tool_after_data.contains("\"status\":\"success\""));
    assert!(tool_after_data.contains("Todos updated (1 completed, 1 in progress, 1 pending)."));
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
async fn swarm_event_stream_emits_write_file_tool_results() {
    let workspace_root = use_temp_workspace_root("swarm-files");
    let app = test_app();
    let (_, create_response) = create_swarm_with_body(
        &app,
        r#"{
            "strategy":"round-robin",
            "manager":{
                "name":"manager",
                "model":"gpt-5.4",
                "tools":[{
                    "name":"write_file",
                    "description":"Write file",
                    "parameters":{
                        "file_path":{"type":"string"},
                        "content":{"type":"string"}
                    }
                }]
            },
            "workers":[{"name":"worker-a","model":"gpt-5.4"}],
            "maxTurns":2
        }"#,
    )
    .await;
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

    let run_handle = {
        let app = app.clone();
        let swarm_id = swarm_id.clone();
        tokio::spawn(async move {
            run_swarm(&app, &swarm_id, r#"{"text":"write file release patch"}"#).await
        })
    };

    let stream = response.into_body().into_data_stream();
    pin_mut!(stream);

    let mut chunks = String::new();
    for _ in 0..256 {
        match futures::poll!(stream.next()) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                chunks.push_str(std::str::from_utf8(&bytes).expect("chunk should be utf-8"));
                if chunks.contains("event: tool:after") && chunks.contains("event: swarm:completed")
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
    assert!(run_response.contains("Wrote 23 chars to notes/release-patch.txt"));
    assert!(chunks.contains("event: tool:after"));

    let tool_after_data =
        extract_sse_event_data(&chunks, "tool:after").expect("tool after event data exists");
    assert!(tool_after_data.contains("\"toolName\":\"write_file\""));
    assert!(tool_after_data.contains("\"status\":\"success\""));
    assert!(tool_after_data.contains("Wrote 23 chars to notes/release-patch.txt"));
    assert!(
        workspace_root
            .path()
            .join("notes")
            .join("release-patch.txt")
            .exists(),
        "write_file should persist the release patch file inside the temp workspace"
    );
}

#[tokio::test]
async fn repeated_swarm_runs_do_not_reuse_stale_runtime_context() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let (first_status, first_response) =
        run_swarm(&app, &swarm_id, r#"{"text":"Repeat the same task"}"#).await;
    let (second_status, second_response) =
        run_swarm(&app, &swarm_id, r#"{"text":"Repeat the same task"}"#).await;
    let first_total_tokens = extract_json_u64_field(&first_response, "totalTokens");
    let second_total_tokens = extract_json_u64_field(&second_response, "totalTokens");

    assert_eq!(first_status, StatusCode::OK);
    assert!(first_response.contains("\"status\":\"success\""));
    assert_eq!(second_status, StatusCode::OK);
    assert!(
        second_total_tokens == first_total_tokens,
        "pooled worker token usage should reset between runs; first={first_total_tokens}, second={second_total_tokens}, response={second_response}"
    );
}

#[tokio::test]
async fn second_run_running_event_clears_stale_token_usage() {
    let app = test_app();
    let (_, create_response) = create_swarm(&app).await;
    let swarm_id = extract_json_string_field(&create_response, "id");

    let (first_status, first_response) =
        run_swarm(&app, &swarm_id, r#"{"text":"Prime pooled worker state"}"#).await;
    let first_total_tokens = extract_json_u64_field(&first_response, "totalTokens");

    assert_eq!(first_status, StatusCode::OK);
    assert!(
        first_total_tokens > 0,
        "first run should consume tokens so stale usage is observable"
    );

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

    let run_handle = {
        let app = app.clone();
        let swarm_id = swarm_id.clone();
        tokio::spawn(async move {
            run_swarm(&app, &swarm_id, r#"{"text":"Prime pooled worker state"}"#).await
        })
    };

    let stream = response.into_body().into_data_stream();
    pin_mut!(stream);

    let mut chunks = String::new();
    for _ in 0..256 {
        match futures::poll!(stream.next()) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                chunks.push_str(std::str::from_utf8(&bytes).expect("chunk should be utf-8"));
                if chunks.contains("event: swarm:running") {
                    break;
                }
            }
            std::task::Poll::Ready(Some(Err(error))) => panic!("stream errored: {error}"),
            std::task::Poll::Ready(None) => break,
            std::task::Poll::Pending => tokio::task::yield_now().await,
        }
    }

    let (second_status, second_response) = run_handle.await.expect("run task should finish");
    let running_data =
        extract_sse_event_data(&chunks, "swarm:running").expect("running event data exists");
    let running_total_tokens = extract_json_u64_field(running_data, "totalTokens");

    assert_eq!(second_status, StatusCode::OK);
    assert!(second_response.contains("\"status\":\"success\""));
    assert_eq!(
        running_total_tokens, 0,
        "running event should not inherit prior token totals; first={first_total_tokens}, running={running_total_tokens}, payload={running_data}"
    );
}
