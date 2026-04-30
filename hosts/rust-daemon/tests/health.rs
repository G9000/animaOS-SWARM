use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use anima_daemon::{app, serve, DaemonConfig};
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tokio::net::TcpListener;
use tower::util::ServiceExt;

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
    std::thread::sleep(Duration::from_millis(25));

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
    let mut stream = TcpStream::connect(addr).expect("client connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout configured");
    stream.write_all(request).expect("request written");

    read_http_response(&mut stream)
}

fn read_http_response(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];

    loop {
        let bytes_read = stream.read(&mut chunk).expect("response read");
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
