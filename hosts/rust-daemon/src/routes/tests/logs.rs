use super::events::SseReader;
use super::*;

use serde_json::Value;
use tracing_subscriber::layer::SubscriberExt;

use crate::logs::{LogBuffer, LogLayer, LogLevel, LOG_REDACTED};
use crate::routes::logs::{
    DEFAULT_LOGS_LIMIT, LOGS_AFTER_INVALID, LOGS_LEVEL_INVALID, LOGS_LIMIT_INVALID,
    LOGS_QUERY_TOO_LONG, LOGS_TOO_MANY_STREAMS, MAX_LOGS_LIMIT, MAX_LOG_QUERY_CHARS,
    MAX_LOG_STREAMS,
};

const OWNER_ORIGIN: &str = "http://localhost:4200";

fn get(uri: &str, origin: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap()
}

fn app_with(buffer: &Arc<LogBuffer>) -> axum::Router {
    crate::routes::router_with_logs(
        Arc::new(RwLock::new(DaemonState::new())),
        DaemonConfig::default(),
        Arc::clone(buffer),
    )
}

async fn list(app: &axum::Router, query: &str) -> Value {
    let response = app
        .clone()
        .oneshot(get(&format!("/api/logs{query}"), OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn open_stream(app: &axum::Router, query: &str) -> SseReader {
    let response = app
        .clone()
        .oneshot(get(&format!("/api/logs/stream{query}"), OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    SseReader::new(response)
}

fn seqs(body: &Value) -> Vec<u64> {
    body["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["seq"].as_u64().unwrap())
        .collect()
}

/// Pushes `count` info lines named `line 1`, `line 2`, and so on.
fn push_numbered(buffer: &LogBuffer, count: u64) {
    for number in 1..=count {
        buffer.push(number, LogLevel::Info, "t", &format!("line {number}"));
    }
}

#[tokio::test]
async fn the_log_routes_refuse_a_non_owner() {
    let buffer = LogBuffer::new();
    let app = app_with(&buffer);
    for uri in ["/api/logs", "/api/logs/stream"] {
        let response = app
            .clone()
            .oneshot(get(uri, "https://untrusted.example"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store", "{uri}");
    }
}

#[tokio::test]
async fn list_returns_the_newest_limit_oldest_first() {
    let buffer = LogBuffer::new();
    push_numbered(&buffer, 5);
    let app = app_with(&buffer);

    let body = list(&app, "?limit=3").await;
    assert_eq!(seqs(&body), [3, 4, 5]);
    assert_eq!(body["newestSeq"], 5);
    let line = &body["lines"][2];
    assert_eq!(line["level"], "info");
    assert_eq!(line["target"], "t");
    assert_eq!(line["message"], "line 5");
    assert_eq!(line["at"], 5);

    assert_eq!(seqs(&list(&app, "").await), [1, 2, 3, 4, 5]);
    let empty = app_with(&LogBuffer::new());
    let body = list(&empty, "").await;
    assert_eq!(body["lines"], serde_json::json!([]));
    assert_eq!(body["newestSeq"], 0);
}

#[tokio::test]
async fn list_after_returns_only_newer_lines_up_to_the_limit() {
    let buffer = LogBuffer::new();
    push_numbered(&buffer, 6);
    let app = app_with(&buffer);

    assert_eq!(seqs(&list(&app, "?after=2&limit=2").await), [3, 4]);
    let past = list(&app, "?after=6").await;
    assert_eq!(seqs(&past), Vec::<u64>::new());
    assert_eq!(past["newestSeq"], 6);
}

#[tokio::test]
async fn list_filters_by_level_and_query() {
    let buffer = LogBuffer::new();
    buffer.push(1, LogLevel::Debug, "anima_daemon::runs", "alpha started");
    buffer.push(2, LogLevel::Info, "anima_daemon::runs", "beta started");
    buffer.push(3, LogLevel::Warn, "anima_daemon::logs", "Beta slow");
    buffer.push(4, LogLevel::Error, "anima_daemon::runs", "gamma failed");
    let app = app_with(&buffer);

    assert_eq!(seqs(&list(&app, "?level=warn").await), [3, 4]);
    assert_eq!(seqs(&list(&app, "?q=BETA").await), [2, 3]);
    assert_eq!(seqs(&list(&app, "?level=info&q=beta").await), [2, 3]);
    assert_eq!(seqs(&list(&app, "?q=anima_daemon::logs").await), [3]);
    assert_eq!(
        seqs(&list(&app, "?level=error&q=beta").await),
        Vec::<u64>::new()
    );
}

#[tokio::test]
async fn list_validates_its_query() {
    let app = app_with(&LogBuffer::new());
    let long_query = "a".repeat(MAX_LOG_QUERY_CHARS + 1);
    let cases = [
        ("/api/logs?level=loud".to_string(), LOGS_LEVEL_INVALID),
        ("/api/logs/stream?level=".to_string(), LOGS_LEVEL_INVALID),
        (format!("/api/logs?q={long_query}"), LOGS_QUERY_TOO_LONG),
        (
            format!("/api/logs/stream?q={long_query}"),
            LOGS_QUERY_TOO_LONG,
        ),
        ("/api/logs?after=-1".to_string(), LOGS_AFTER_INVALID),
        ("/api/logs?after=x".to_string(), LOGS_AFTER_INVALID),
        ("/api/logs/stream?after=1.5".to_string(), LOGS_AFTER_INVALID),
        ("/api/logs?limit=0".to_string(), LOGS_LIMIT_INVALID),
        ("/api/logs?limit=1001".to_string(), LOGS_LIMIT_INVALID),
        ("/api/logs?limit=many".to_string(), LOGS_LIMIT_INVALID),
    ];
    for (uri, message) in cases {
        let response = app.clone().oneshot(get(&uri, OWNER_ORIGIN)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store", "{uri}");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["error"], message, "{uri}");
    }
    // The edges are accepted: the longest query, limit 1 and 1,000, and the
    // stream ignores `limit`.
    let exact_query = "a".repeat(MAX_LOG_QUERY_CHARS);
    for uri in [
        format!("/api/logs?q={exact_query}"),
        "/api/logs?limit=1".to_string(),
        "/api/logs?limit=1000".to_string(),
        "/api/logs?after=0".to_string(),
    ] {
        let response = app.clone().oneshot(get(&uri, OWNER_ORIGIN)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn a_secret_logged_through_the_layer_never_reaches_the_response() {
    let buffer = LogBuffer::new();
    let app = app_with(&buffer);
    let mut stream = open_stream(&app, "").await;
    let subscriber = tracing_subscriber::registry().with(LogLayer::new(Arc::clone(&buffer)));
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!("call failed key=sk-ant-api03-AbCdEfGhIjKlMnOp12");
    });

    let secret = "sk-ant-api03-AbCdEfGhIjKlMnOp12";
    let event = stream.next().await;
    assert_eq!(event.event.as_deref(), Some("log"));
    assert!(!event.data.to_string().contains(secret), "{}", event.data);
    assert!(event.data["message"]
        .as_str()
        .unwrap()
        .contains(LOG_REDACTED));
    let body = list(&app, "").await;
    assert!(!body.to_string().contains(secret), "{body}");
    assert_eq!(seqs(&body), [1]);
}

#[tokio::test]
async fn a_stream_gets_the_backlog_then_live_lines_without_a_gap() {
    let buffer = LogBuffer::new();
    push_numbered(&buffer, 3);
    let app = app_with(&buffer);
    let mut stream = open_stream(&app, "?after=1").await;

    for (expected, text) in [(2, "line 2"), (3, "line 3")] {
        let event = stream.next().await;
        assert_eq!(event.event.as_deref(), Some("log"));
        assert_eq!(event.data["seq"], expected);
        assert_eq!(event.data["message"], text);
        assert_eq!(event.data["level"], "info");
    }
    buffer.push(4, LogLevel::Info, "t", "line 4");
    buffer.push(5, LogLevel::Info, "t", "line 5");
    // 3 was not sent twice: the next two events are 4 and 5, in order.
    assert_eq!(stream.next().await.data["seq"], 4);
    assert_eq!(stream.next().await.data["seq"], 5);
}

#[tokio::test]
async fn a_stream_without_after_sends_only_new_lines() {
    let buffer = LogBuffer::new();
    push_numbered(&buffer, 3);
    let app = app_with(&buffer);
    let mut stream = open_stream(&app, "").await;

    buffer.push(4, LogLevel::Info, "t", "line 4");
    let event = stream.next().await;
    assert_eq!(event.data["seq"], 4);
}

#[tokio::test]
async fn a_lagged_stream_sends_resync_then_continues() {
    let buffer = LogBuffer::with_limits(100, 2);
    let app = app_with(&buffer);
    let mut stream = open_stream(&app, "").await;
    push_numbered(&buffer, 6);

    let resync = stream.next().await;
    assert_eq!(resync.event.as_deref(), Some("resync"));
    assert_eq!(resync.data, serde_json::json!({ "newestSeq": 6 }));

    buffer.push(7, LogLevel::Info, "t", "line 7");
    let next = stream.next().await;
    assert_eq!(next.event.as_deref(), Some("log"));
    assert_eq!(next.data["seq"], 7);
}

#[tokio::test]
async fn the_stream_applies_its_level_and_query_filters() {
    let buffer = LogBuffer::new();
    buffer.push(1, LogLevel::Info, "t", "beta quiet");
    buffer.push(2, LogLevel::Warn, "t", "gamma loud");
    buffer.push(3, LogLevel::Warn, "t", "beta loud");
    let app = app_with(&buffer);
    let mut stream = open_stream(&app, "?after=0&level=warn&q=BETA").await;

    assert_eq!(stream.next().await.data["seq"], 3);
    buffer.push(4, LogLevel::Debug, "t", "beta debug");
    buffer.push(5, LogLevel::Error, "t", "delta");
    buffer.push(6, LogLevel::Error, "t", "beta failed");
    assert_eq!(stream.next().await.data["seq"], 6);
}

#[tokio::test]
async fn too_many_streams_answer_429_and_a_closed_stream_frees_its_slot() {
    let buffer = LogBuffer::new();
    let app = app_with(&buffer);
    let mut open = Vec::new();
    for _ in 0..MAX_LOG_STREAMS {
        let response = app
            .clone()
            .oneshot(get("/api/logs/stream", OWNER_ORIGIN))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }

    let refused = app
        .clone()
        .oneshot(get("/api/logs/stream", OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()["cache-control"], "no-store");
    let body: Value =
        serde_json::from_slice(&to_bytes(refused.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["error"], LOGS_TOO_MANY_STREAMS);

    // The list is not a stream and is not counted.
    assert_eq!(seqs(&list(&app, "").await), Vec::<u64>::new());

    drop(open.pop());
    let reopened = app
        .clone()
        .oneshot(get("/api/logs/stream", OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(reopened.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_rejected_query_does_not_take_a_stream_slot() {
    let buffer = LogBuffer::new();
    let app = app_with(&buffer);
    for _ in 0..(MAX_LOG_STREAMS * 2) {
        let response = app
            .clone()
            .oneshot(get("/api/logs/stream?level=loud", OWNER_ORIGIN))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = app
        .oneshot(get("/api/logs/stream", OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_stream_response_is_an_uncached_event_stream() {
    let app = app_with(&LogBuffer::new());
    let response = app
        .oneshot(get("/api/logs/stream", OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
}

#[test]
fn the_log_routes_are_in_the_openapi_document() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    for path in ["/api/logs", "/api/logs/stream"] {
        let operation = document.paths.paths[path]
            .get
            .as_ref()
            .unwrap_or_else(|| panic!("{path} is documented"));
        assert_eq!(operation.tags.as_deref(), Some(&["logs".to_string()][..]));
    }
    assert!(document
        .tags
        .unwrap_or_default()
        .iter()
        .any(|tag| tag.name == "logs"));
}

#[test]
fn constants() {
    assert_eq!(DEFAULT_LOGS_LIMIT, 200);
    assert_eq!(MAX_LOGS_LIMIT, 1_000);
    assert_eq!(MAX_LOG_STREAMS, 8);
    assert_eq!(MAX_LOG_QUERY_CHARS, 200);
    assert_eq!(
        LOGS_LEVEL_INVALID,
        "level must be one of error, warn, info, debug, trace"
    );
    assert_eq!(LOGS_LIMIT_INVALID, "limit must be from 1 to 1000");
    assert_eq!(LOGS_AFTER_INVALID, "after must be a whole number");
    assert_eq!(LOGS_QUERY_TOO_LONG, "q must be at most 200 characters");
    assert_eq!(LOGS_TOO_MANY_STREAMS, "Too many log streams are open");
}
