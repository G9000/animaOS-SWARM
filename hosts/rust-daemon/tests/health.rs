#[allow(dead_code)]
mod support;

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use anima_daemon::{app, app_with_configured_persistence, serve, DaemonConfig};
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tokio::net::TcpListener;
use tower::util::ServiceExt;

use support::use_temp_workspace_root;

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

#[tokio::test]
async fn health_endpoint_returns_ok_json() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .expect("content-type header exists"),
        "application/json"
    );

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    assert_eq!(
        std::str::from_utf8(&body).expect("body is utf-8"),
        "{\"status\":\"ok\"}"
    );
}

#[tokio::test]
async fn readiness_endpoint_returns_ready_json() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/ready")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .expect("content-type header exists"),
        "application/json"
    );

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let body = std::str::from_utf8(&body).expect("body is utf-8");
    assert!(body.contains("\"status\":\"ready\""), "{body}");
    assert!(
        body.contains("\"controlPlaneDurability\":\"ephemeral\""),
        "{body}"
    );
    assert!(body.contains("\"persistenceMode\":\"memory\""), "{body}");
}

#[tokio::test]
async fn metrics_endpoint_returns_prometheus_text() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .expect("content-type header exists")
        .to_str()
        .expect("content-type header is utf-8");
    assert!(content_type.starts_with("text/plain"), "{content_type}");

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let body = std::str::from_utf8(&body).expect("body is utf-8");
    assert!(body.contains("anima_daemon_ready 1"), "{body}");
    assert!(body.contains("anima_daemon_agents"), "{body}");
    assert!(body.contains("anima_daemon_memories"), "{body}");
    assert!(
        body.contains("anima_daemon_control_plane_durability_info{mode=\"ephemeral\"} 1"),
        "{body}"
    );
}

#[tokio::test]
async fn readiness_and_metrics_report_json_control_plane_store() {
    let workspace = use_temp_workspace_root("control-plane-health");
    let control_plane_path = workspace.path().join("control-plane.json");
    let _guard = EnvVarGuard::set("ANIMAOS_RS_CONTROL_PLANE_FILE", &control_plane_path);
    let app = app_with_configured_persistence(DaemonConfig::default())
        .await
        .expect("app configures persistence");

    let readiness = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/ready")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");
    let metrics = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    let readiness_body = to_bytes(readiness.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let readiness_body = std::str::from_utf8(&readiness_body).expect("body is utf-8");
    assert!(
        readiness_body.contains("\"controlPlaneDurability\":\"json\""),
        "{readiness_body}"
    );

    let metrics_body = to_bytes(metrics.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let metrics_body = std::str::from_utf8(&metrics_body).expect("body is utf-8");
    assert!(
        metrics_body.contains("anima_daemon_control_plane_durability_info{mode=\"json\"} 1"),
        "{metrics_body}"
    );
}

#[tokio::test]
async fn health_endpoint_propagates_request_id_header() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/health")
                .header("x-request-id", "test-request-id")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-request-id")
            .expect("x-request-id header exists"),
        "test-request-id"
    );
}

#[tokio::test]
async fn health_endpoint_returns_not_found_json_for_wrong_method() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn openapi_endpoint_returns_spec_json() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/openapi.json")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .expect("content-type header exists")
        .to_str()
        .expect("content-type header is utf-8");
    assert!(
        content_type.starts_with("application/json"),
        "{content_type}"
    );

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let body = std::str::from_utf8(&body).expect("body is utf-8");
    assert!(body.contains("\"openapi\""), "{body}");
    assert!(body.contains("\"/api/health\""), "{body}");
}

#[tokio::test]
async fn docs_endpoint_returns_html() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/docs/")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("app responds");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .expect("content-type header exists")
        .to_str()
        .expect("content-type header is utf-8");
    assert!(content_type.starts_with("text/html"), "{content_type}");

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let body = std::str::from_utf8(&body).expect("body is utf-8");
    assert!(
        body.contains("Scalar") || body.contains("api-reference"),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_exposes_health_and_error_paths_over_real_http() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("listener has local addr");
    let server = tokio::spawn(async move { serve(listener, DaemonConfig::default()).await });

    let health_response = send_raw_http(
        addr,
        b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    let malformed_response = send_raw_http(
        addr,
        b"GET /api/memories/search?q=%GG HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );

    server.abort();
    let _ = server.await;

    assert!(
        health_response.starts_with("HTTP/1.1 200 OK"),
        "{health_response:?}"
    );
    assert!(
        health_response.contains("{\"status\":\"ok\"}"),
        "{health_response:?}"
    );
    assert!(
        malformed_response.starts_with("HTTP/1.1 400 Bad Request"),
        "{malformed_response:?}"
    );
    assert!(
        malformed_response.contains("{\"error\":\"malformed request\"}"),
        "{malformed_response:?}"
    );
}

fn send_raw_http(addr: SocketAddr, request: &[u8]) -> String {
    let mut stream = connect_with_retry(addr);
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout configured");
    stream.write_all(request).expect("request written");

    read_http_response(&mut stream)
}

fn connect_with_retry(addr: SocketAddr) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match TcpStream::connect(addr) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
                let _ = error;
            }
            Err(error) => panic!("client connects: {error}"),
        }
    }
}

fn read_http_response(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];

    loop {
        let bytes_read = match stream.read(&mut chunk) {
            Ok(bytes_read) => bytes_read,
            Err(error) if error.kind() == ErrorKind::ConnectionReset && !buffer.is_empty() => {
                break;
            }
            Err(error) => panic!("response read: {error}"),
        };
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);

        if let Some(header_end) = find_sequence(&buffer, b"\r\n\r\n") {
            let header_end = header_end + 4;
            let content_length = parse_content_length(&buffer[..header_end]);
            if buffer.len() >= header_end + content_length {
                break;
            }
        }
    }

    String::from_utf8(buffer).expect("response is utf-8")
}

fn parse_content_length(header: &[u8]) -> usize {
    let header = std::str::from_utf8(header).expect("response header is utf-8");
    for line in header.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse::<usize>()
                .expect("content length is valid");
        }
    }
    0
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
