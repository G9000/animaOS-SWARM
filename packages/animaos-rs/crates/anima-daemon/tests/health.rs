use anima_daemon::app;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
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
