use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use anima_daemon::Daemon;

fn spawn_daemon(request_limit: usize) -> (SocketAddr, JoinHandle<()>) {
    let daemon = Daemon::bind("127.0.0.1:0").expect("daemon binds");
    let addr = daemon.local_addr().expect("daemon reports local addr");

    let server = thread::spawn(move || {
        daemon
            .serve_n(request_limit)
            .expect("daemon serves expected number of requests");
    });

    thread::sleep(Duration::from_millis(25));
    (addr, server)
}

fn send_request(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("client connects");
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("read timeout configured");
    stream
        .write_all(request.as_bytes())
        .expect("request written");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response read");
    response
}

fn create_agent(addr: SocketAddr, body: &str) -> String {
    let request = format!(
        "POST /api/agents HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    send_request(addr, &request)
}

fn run_agent(addr: SocketAddr, agent_id: &str, body: &str) -> String {
    let request = format!(
        "POST /api/agents/{agent_id}/run HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    send_request(addr, &request)
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

#[test]
fn create_agent_returns_runtime_snapshot() {
    let (addr, server) = spawn_daemon(1);
    let response = create_agent(
        addr,
        r#"{"name":"researcher","model":"gpt-5.4","bio":"Finds answers quickly","provider":"openai","knowledge":["Rust","TypeScript"],"settings":{"temperature":0.2,"maxTokens":2048}}"#,
    );
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"name\":\"researcher\""),
        "response missing agent name: {response}"
    );
    assert!(
        response.contains("\"status\":\"idle\""),
        "response missing idle status: {response}"
    );
    assert!(
        response.contains("\"messageCount\":0"),
        "response missing message count: {response}"
    );
    assert!(
        response.contains("\"eventCount\":1"),
        "response missing event count: {response}"
    );
    assert!(
        response.contains("\"lastTask\":null"),
        "response missing lastTask placeholder: {response}"
    );
}

#[test]
fn list_agents_includes_created_runtime() {
    let (addr, server) = spawn_daemon(2);
    let create_response = create_agent(
        addr,
        r#"{"name":"writer","model":"gpt-5.4-mini","topics":["docs"]}"#,
    );
    let list_response = send_request(
        addr,
        "GET /api/agents HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    server.join().expect("server thread joins");

    assert!(
        create_response.starts_with("HTTP/1.1 201 Created"),
        "unexpected create response: {create_response}"
    );
    assert!(
        list_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected list response: {list_response}"
    );
    assert!(
        list_response.contains("\"agents\":["),
        "list response missing agents array: {list_response}"
    );
    assert!(
        list_response.contains("\"name\":\"writer\""),
        "list response missing created runtime: {list_response}"
    );
}

#[test]
fn get_agent_returns_runtime_snapshot() {
    let (addr, server) = spawn_daemon(2);
    let create_response = create_agent(addr, r#"{"name":"planner","model":"gpt-5.4"}"#);
    let agent_id = extract_json_string_field(&create_response, "id");
    let get_response = send_request(
        addr,
        &format!(
            "GET /api/agents/{agent_id} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        ),
    );
    server.join().expect("server thread joins");

    assert!(
        get_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected get response: {get_response}"
    );
    assert!(
        get_response.contains(&format!("\"id\":\"{agent_id}\"")),
        "get response missing runtime id: {get_response}"
    );
    assert!(
        get_response.contains("\"name\":\"planner\""),
        "get response missing agent name: {get_response}"
    );
}

#[test]
fn get_agent_returns_not_found_for_unknown_runtime() {
    let (addr, server) = spawn_daemon(1);
    let response = send_request(
        addr,
        "GET /api/agents/agent-missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 404 Not Found"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"error\":\"not found\""),
        "missing not found payload: {response}"
    );
}

#[test]
fn agent_recent_memories_filters_by_runtime_agent_id() {
    let (addr, server) = spawn_daemon(4);
    let create_response = create_agent(addr, r#"{"name":"reviewer","model":"gpt-5.4"}"#);
    let agent_id = extract_json_string_field(&create_response, "id");

    let matching_memory = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"fact\",\"content\":\"reviewer memory\",\"importance\":0.8}}"
    );
    let matching_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        matching_memory.len(),
        matching_memory
    );
    let other_memory = r#"{"agentId":"agent-other","agentName":"writer","type":"fact","content":"writer memory","importance":0.8}"#;
    let other_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        other_memory.len(),
        other_memory
    );

    let _ = send_request(addr, &matching_request);
    let _ = send_request(addr, &other_request);
    let recent_response = send_request(
        addr,
        &format!(
            "GET /api/agents/{agent_id}/memories/recent HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        ),
    );
    server.join().expect("server thread joins");

    assert!(
        recent_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected recent response: {recent_response}"
    );
    assert!(
        recent_response.contains("\"content\":\"reviewer memory\""),
        "recent response missing matching memory: {recent_response}"
    );
    assert!(
        !recent_response.contains("\"content\":\"writer memory\""),
        "recent response should exclude other agent memory: {recent_response}"
    );
}

#[test]
fn create_agent_rejects_missing_required_fields() {
    let (addr, server) = spawn_daemon(1);
    let response = create_agent(addr, r#"{"name":"broken-agent"}"#);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"error\":\"model is required\""),
        "missing validation error: {response}"
    );
}

#[test]
fn create_agent_accepts_object_tools_and_plugins() {
    let (addr, server) = spawn_daemon(1);
    let response = create_agent(
        addr,
        r#"{"name":"builder","model":"gpt-5.4","tools":[{"name":"search","description":"Search docs","parameters":{"query":{"type":"string"}}}],"plugins":[{"name":"notes","description":"Workspace notes"}]}"#,
    );
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"tools\":[{\"name\":\"search\""),
        "response missing serialized tool object: {response}"
    );
    assert!(
        response.contains("\"plugins\":[{\"name\":\"notes\""),
        "response missing serialized plugin object: {response}"
    );
}

#[test]
fn run_agent_returns_task_result_and_completed_runtime_state() {
    let (addr, server) = spawn_daemon(2);
    let create_response = create_agent(addr, r#"{"name":"operator","model":"gpt-5.4"}"#);
    let agent_id = extract_json_string_field(&create_response, "id");
    let run_response = run_agent(addr, &agent_id, r#"{"text":"Summarize the latest task"}"#);
    server.join().expect("server thread joins");

    assert!(
        run_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected run response: {run_response}"
    );
    assert!(
        run_response.contains("\"status\":\"completed\""),
        "runtime state should be completed: {run_response}"
    );
    assert!(
        run_response.contains("\"status\":\"success\""),
        "task result should be success: {run_response}"
    );
    assert!(
        run_response.contains("\"totalTokens\":"),
        "run response should include token usage: {run_response}"
    );
    assert!(
        run_response.contains("\"text\":\"operator handled task: Summarize the latest task\""),
        "run response missing deterministic output: {run_response}"
    );
}

#[test]
fn get_agent_reflects_runtime_context_after_run() {
    let (addr, server) = spawn_daemon(3);
    let create_response = create_agent(addr, r#"{"name":"analyst","model":"gpt-5.4"}"#);
    let agent_id = extract_json_string_field(&create_response, "id");
    let _ = run_agent(addr, &agent_id, r#"{"text":"Inspect memory state"}"#);
    let get_response = send_request(
        addr,
        &format!(
            "GET /api/agents/{agent_id} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        ),
    );
    server.join().expect("server thread joins");

    assert!(
        get_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected get response: {get_response}"
    );
    assert!(
        get_response.contains("\"messageCount\":2"),
        "runtime should track user and assistant messages: {get_response}"
    );
    assert!(
        get_response.contains("\"eventCount\":8"),
        "runtime should track lifecycle and message events: {get_response}"
    );
    assert!(
        !get_response.contains("\"promptTokens\":0,\"completionTokens\":0,\"totalTokens\":0"),
        "runtime should update token usage after run: {get_response}"
    );
    assert!(
        get_response.contains("\"lastTask\":{"),
        "runtime should retain the last task result: {get_response}"
    );
}

#[test]
fn run_agent_returns_not_found_for_unknown_runtime() {
    let (addr, server) = spawn_daemon(1);
    let response = run_agent(addr, "agent-missing", r#"{"text":"missing agent"}"#);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 404 Not Found"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"error\":\"not found\""),
        "missing not found payload: {response}"
    );
}

#[test]
fn run_agent_rejects_missing_text() {
    let (addr, server) = spawn_daemon(2);
    let create_response = create_agent(addr, r#"{"name":"operator","model":"gpt-5.4"}"#);
    let agent_id = extract_json_string_field(&create_response, "id");
    let response = run_agent(addr, &agent_id, r#"{"metadata":{"source":"test"}}"#);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"error\":\"text is required\""),
        "missing validation error: {response}"
    );
}

#[test]
fn run_agent_persists_task_result_memory() {
    let (addr, server) = spawn_daemon(3);
    let create_response = create_agent(addr, r#"{"name":"operator","model":"gpt-5.4"}"#);
    let agent_id = extract_json_string_field(&create_response, "id");

    let run_response = run_agent(addr, &agent_id, r#"{"text":"Produce final answer"}"#);
    let recent_response = send_request(
        addr,
        &format!(
            "GET /api/agents/{agent_id}/memories/recent HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        ),
    );
    server.join().expect("server thread joins");

    assert!(
        run_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected run response: {run_response}"
    );
    assert!(
        recent_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected recent response: {recent_response}"
    );
    assert!(
        recent_response.contains("\"type\":\"task_result\""),
        "run should persist a task_result memory: {recent_response}"
    );
    assert!(
        recent_response.contains("\"content\":\"operator handled task: Produce final answer\""),
        "run output should be persisted as recent memory: {recent_response}"
    );
}

#[test]
fn run_agent_executes_memory_search_tool_round_trip() {
    let (addr, server) = spawn_daemon(3);
    let create_response = create_agent(
        addr,
        r#"{"name":"reviewer","model":"gpt-5.4","tools":[{"name":"memory_search","description":"Search memories","parameters":{"query":{"type":"string"}}}]}"#,
    );
    let agent_id = extract_json_string_field(&create_response, "id");

    let memory_body = format!(
        "{{\"agentId\":\"{agent_id}\",\"agentName\":\"reviewer\",\"type\":\"fact\",\"content\":\"remembered prior answer\",\"importance\":0.8}}"
    );
    let memory_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        memory_body.len(),
        memory_body
    );
    let _ = send_request(addr, &memory_request);

    let run_response = run_agent(addr, &agent_id, r#"{"text":"remembered"}"#);
    server.join().expect("server thread joins");

    assert!(
        run_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected run response: {run_response}"
    );
    assert!(
        run_response.contains("\"messageCount\":4"),
        "tool loop should add assistant and tool messages: {run_response}"
    );
    assert!(
        run_response.contains("remembered prior answer"),
        "tool-backed response should include memory search result: {run_response}"
    );
}
