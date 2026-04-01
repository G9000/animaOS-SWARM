use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use anima_daemon::{Daemon, DaemonConfig};

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

fn spawn_daemon_with_config(
    config: DaemonConfig,
    request_limit: usize,
) -> (SocketAddr, JoinHandle<()>) {
    let daemon = Daemon::bind_with_config("127.0.0.1:0", config).expect("daemon binds");
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
    send_request_with_timeout(addr, request, Duration::from_millis(500))
}

fn send_request_with_timeout(addr: SocketAddr, request: &str, timeout: Duration) -> String {
    let mut stream = TcpStream::connect(addr).expect("client connects");
    stream
        .set_read_timeout(Some(timeout))
        .expect("read timeout configured");
    stream
        .write_all(request.as_bytes())
        .expect("request written");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response read");
    response
}

fn send_split_request(addr: SocketAddr, headers: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("client connects");
    stream
        .write_all(headers.as_bytes())
        .expect("headers written");
    thread::sleep(Duration::from_millis(10));
    stream.write_all(body.as_bytes()).expect("body written");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response read");
    response
}

fn send_split_request_with_delay(
    addr: SocketAddr,
    headers: &str,
    body: &str,
    delay: Duration,
) -> String {
    let mut stream = TcpStream::connect(addr).expect("client connects");
    stream
        .write_all(headers.as_bytes())
        .expect("headers written");
    thread::sleep(delay);
    stream.write_all(body.as_bytes()).expect("body written");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response read");
    response
}

#[test]
fn create_memory_returns_created_memory() {
    let (addr, server) = spawn_daemon(1);
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"Rust daemon memory endpoint created","importance":0.8,"tags":["rust","memory"]}"#;
    let request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_request(addr, &request);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"content\":\"Rust daemon memory endpoint created\""),
        "response missing created memory content: {response}"
    );
    assert!(
        response.contains("\"type\":\"fact\""),
        "response missing created memory type: {response}"
    );
}

#[test]
fn create_memory_rejects_missing_required_fields() {
    let (addr, server) = spawn_daemon(1);
    let body =
        r#"{"agentId":"agent-1","type":"fact","content":"missing agentName","importance":0.8}"#;
    let request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_request(addr, &request);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"error\":\"agentName is required\""),
        "expected validation error in response: {response}"
    );
}

#[test]
fn search_memories_returns_created_memory() {
    let (addr, server) = spawn_daemon(2);
    let create_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"BM25 search should find this memory","importance":0.9,"tags":["search"]}"#;
    let create_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        create_body.len(),
        create_body
    );
    let search_request = "GET /api/memories/search?q=BM25%20search HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let create_response = send_request(addr, &create_request);
    let search_response = send_request(addr, search_request);
    server.join().expect("server thread joins");

    assert!(
        create_response.starts_with("HTTP/1.1 201 Created"),
        "unexpected create response: {create_response}"
    );
    assert!(
        search_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected search response: {search_response}"
    );
    assert!(
        search_response.contains("\"results\":"),
        "search response missing results array: {search_response}"
    );
    assert!(
        search_response.contains("\"content\":\"BM25 search should find this memory\""),
        "search response missing created memory: {search_response}"
    );
}

#[test]
fn search_memories_rejects_missing_query() {
    let (addr, server) = spawn_daemon(1);
    let response = send_request(
        addr,
        "GET /api/memories/search HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\"error\":\"q query parameter is required\""),
        "expected missing q error in response: {response}"
    );
}

#[test]
fn recent_memories_returns_newest_first() {
    let (addr, server) = spawn_daemon(3);
    let oldest_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"oldest","importance":0.4}"#;
    let newest_body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"newest","importance":0.7}"#;
    let oldest_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        oldest_body.len(),
        oldest_body
    );
    let newest_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        newest_body.len(),
        newest_body
    );
    let recent_request =
        "GET /api/memories/recent HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first_response = send_request(addr, &oldest_request);
    thread::sleep(Duration::from_millis(10));
    let second_response = send_request(addr, &newest_request);
    let recent_response = send_request(addr, recent_request);
    server.join().expect("server thread joins");

    assert!(
        first_response.starts_with("HTTP/1.1 201 Created"),
        "unexpected first create response: {first_response}"
    );
    assert!(
        second_response.starts_with("HTTP/1.1 201 Created"),
        "unexpected second create response: {second_response}"
    );
    assert!(
        recent_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected recent response: {recent_response}"
    );

    let newest_index = recent_response
        .find("\"content\":\"newest\"")
        .expect("recent response should contain newest");
    let oldest_index = recent_response
        .find("\"content\":\"oldest\"")
        .expect("recent response should contain oldest");
    assert!(
        newest_index < oldest_index,
        "expected newest memory before oldest in response: {recent_response}"
    );
}

#[test]
fn search_memories_applies_agent_name_filter() {
    let (addr, server) = spawn_daemon(3);
    let researcher_body = r#"{"agentId":"a1","agentName":"researcher","type":"fact","content":"shared topic from researcher","importance":0.8}"#;
    let writer_body = r#"{"agentId":"a2","agentName":"writer","type":"fact","content":"shared topic from writer","importance":0.8}"#;
    let researcher_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        researcher_body.len(),
        researcher_body
    );
    let writer_request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        writer_body.len(),
        writer_body
    );
    let search_request = "GET /api/memories/search?q=shared%20topic&agentName=writer HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let researcher_response = send_request(addr, &researcher_request);
    let writer_response = send_request(addr, &writer_request);
    let search_response = send_request(addr, search_request);
    server.join().expect("server thread joins");

    assert!(
        researcher_response.starts_with("HTTP/1.1 201 Created"),
        "unexpected researcher create response: {researcher_response}"
    );
    assert!(
        writer_response.starts_with("HTTP/1.1 201 Created"),
        "unexpected writer create response: {writer_response}"
    );
    assert!(
        search_response.starts_with("HTTP/1.1 200 OK"),
        "unexpected filtered search response: {search_response}"
    );
    assert!(
        search_response.contains("\"agentName\":\"writer\""),
        "expected writer result in filtered search: {search_response}"
    );
    assert!(
        !search_response.contains("\"agentName\":\"researcher\""),
        "filtered search should exclude researcher result: {search_response}"
    );
}

#[test]
fn malformed_request_returns_bad_request_and_daemon_keeps_serving() {
    let (addr, server) = spawn_daemon(2);

    let bad_response = send_request(
        addr,
        "GET /api/memories/search?q=%GG HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    let good_response = send_request(
        addr,
        "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    server.join().expect("server thread joins");

    assert!(
        bad_response.starts_with("HTTP/1.1 400 Bad Request"),
        "unexpected malformed response: {bad_response}"
    );
    assert!(
        bad_response.contains("\"error\":\"malformed request\""),
        "expected malformed request error in response: {bad_response}"
    );
    assert!(
        good_response.starts_with("HTTP/1.1 200 OK"),
        "daemon should still serve valid requests after malformed input: {good_response}"
    );
}

#[test]
fn create_memory_accepts_lowercase_content_length_with_split_body() {
    let (addr, server) = spawn_daemon(1);
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"lowercase header body","importance":0.6}"#;
    let headers = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\ncontent-length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );

    let response = send_split_request(addr, &headers, body);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "unexpected response for lowercase content-length: {response}"
    );
    assert!(
        response.contains("\"content\":\"lowercase header body\""),
        "response missing created memory content: {response}"
    );
}

#[test]
fn create_memory_escapes_control_characters_in_json_response() {
    let (addr, server) = spawn_daemon(1);
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"has\bbackspace\fpage","importance":0.8}"#;
    let request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_request(addr, &request);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("\\u0008"),
        "response should escape backspace as unicode: {response}"
    );
    assert!(
        response.contains("\\u000c"),
        "response should escape form-feed as unicode: {response}"
    );
}

#[test]
fn partial_request_times_out_and_daemon_keeps_serving() {
    let (addr, server) = spawn_daemon(2);
    let partial_body =
        r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"partial"#;
    let headers = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        partial_body.len() + 32
    );
    let mut stalled_stream = TcpStream::connect(addr).expect("client connects");
    stalled_stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("read timeout configured");
    stalled_stream
        .write_all(headers.as_bytes())
        .expect("headers written");
    stalled_stream
        .write_all(partial_body.as_bytes())
        .expect("partial body written");

    thread::sleep(Duration::from_millis(250));

    let health_response = send_request_with_timeout(
        addr,
        "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        Duration::from_millis(500),
    );
    let mut stalled_response = String::new();
    stalled_stream
        .read_to_string(&mut stalled_response)
        .expect("stalled response read");
    server.join().expect("server thread joins");

    assert!(
        stalled_response.starts_with("HTTP/1.1 400 Bad Request"),
        "partial request should get a client error: {stalled_response}"
    );
    assert!(
        health_response.starts_with("HTTP/1.1 200 OK"),
        "daemon should keep serving after partial request timeout: {health_response}"
    );
}

#[test]
fn create_memory_accepts_surrogate_pair_unicode_escape() {
    let (addr, server) = spawn_daemon(1);
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"launch \ud83d\ude80","importance":0.8}"#;
    let request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_request(addr, &request);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "valid surrogate pair escape should be accepted: {response}"
    );
    assert!(
        response.contains("\"content\":\"launch 🚀\""),
        "response should decode surrogate pair into utf-8: {response}"
    );
}

#[test]
fn create_memory_rejects_unescaped_newline_in_json_string() {
    let (addr, server) = spawn_daemon(1);
    let body = format!(
        "{{\"agentId\":\"agent-1\",\"agentName\":\"researcher\",\"type\":\"fact\",\"content\":\"bad{}json\",\"importance\":0.8}}",
        '\n'
    );
    let request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_request(addr, &request);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "unescaped newline should be rejected as invalid JSON: {response}"
    );
    assert!(
        response.contains("\"error\":\"request body must be valid JSON\""),
        "expected json validation error in response: {response}"
    );
}

#[test]
fn create_memory_accepts_larger_request_when_size_limit_is_configured() {
    let (addr, server) = spawn_daemon_with_config(
        DaemonConfig {
            max_request_bytes: 256 * 1024,
            ..DaemonConfig::default()
        },
        1,
    );
    let content = "x".repeat(70 * 1024);
    let body = format!(
        "{{\"agentId\":\"agent-1\",\"agentName\":\"researcher\",\"type\":\"fact\",\"content\":\"{}\",\"importance\":0.8}}",
        content
    );
    let request = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_request(addr, &request);
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "configured daemon should accept larger request bodies: {response}"
    );
}

#[test]
fn create_memory_accepts_slower_split_body_when_timeout_is_configured() {
    let (addr, server) = spawn_daemon_with_config(
        DaemonConfig {
            request_read_timeout: Duration::from_millis(750),
            ..DaemonConfig::default()
        },
        1,
    );
    let body = r#"{"agentId":"agent-1","agentName":"researcher","type":"fact","content":"slow but valid body","importance":0.8}"#;
    let headers = format!(
        "POST /api/memories HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );

    let response = send_split_request_with_delay(addr, &headers, body, Duration::from_millis(300));
    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 201 Created"),
        "configured daemon should accept slower valid clients: {response}"
    );
}
