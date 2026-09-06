use super::{http::json_response, AppState};
use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    response::Response,
};

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
enum Operation {
    Status,
    Login,
    Cancel,
    Disconnect,
}
async fn handle(state: AppState, request: Request, operation: Operation) -> Response {
    let authorized = match operation {
        Operation::Status => state.local_owner.authorize_read(request.headers()),
        _ => state.local_owner.authorize(request.headers()),
    };
    if authorized.is_err() {
        return no_store(json_response(
            StatusCode::FORBIDDEN,
            &serde_json::json!({"error":"Local owner authorization required"}),
        ));
    }
    let service = state.daemon.read().await.chatgpt_auth.clone();
    let result = match operation {
        Operation::Login => service.login().await,
        Operation::Cancel => service.cancel(false).await,
        Operation::Disconnect => service.cancel(true).await,
        Operation::Status => service.status().await,
    };
    no_store(match result {
        Ok(status) => json_response(StatusCode::OK, &status),
        Err(error) => json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({"error":error}),
        ),
    })
}
#[utoipa::path(get, path = "/api/providers/chatgpt/status", tag = "providers", responses((status = 200, description = "Redacted subscription status", body = crate::chatgpt_auth::Status), (status = 403, description = "Local owner required"), (status = 503, description = "Authorization or vault unavailable")))]
pub(super) async fn status(State(state): State<AppState>, request: Request) -> Response {
    handle(state, request, Operation::Status).await
}
#[utoipa::path(post, path = "/api/providers/chatgpt/login", tag = "providers", responses((status = 200, description = "Redacted subscription status", body = crate::chatgpt_auth::Status), (status = 403, description = "Local owner required"), (status = 503, description = "Authorization or vault unavailable")))]
pub(super) async fn login(State(state): State<AppState>, request: Request) -> Response {
    handle(state, request, Operation::Login).await
}
#[utoipa::path(delete, path = "/api/providers/chatgpt/login", tag = "providers", responses((status = 200, description = "Redacted subscription status", body = crate::chatgpt_auth::Status), (status = 403, description = "Local owner required"), (status = 503, description = "Authorization or vault unavailable")))]
pub(super) async fn cancel(State(state): State<AppState>, request: Request) -> Response {
    handle(state, request, Operation::Cancel).await
}
#[utoipa::path(delete, path = "/api/providers/chatgpt", tag = "providers", responses((status = 200, description = "Redacted subscription status", body = crate::chatgpt_auth::Status), (status = 403, description = "Local owner required"), (status = 503, description = "Authorization or vault unavailable")))]
pub(super) async fn disconnect(State(state): State<AppState>, request: Request) -> Response {
    handle(state, request, Operation::Disconnect).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn account_routes_require_owner_and_do_not_cache_rejections() {
        for (method, path) in [
            ("GET", "/api/providers/chatgpt/status"),
            ("POST", "/api/providers/chatgpt/login"),
            ("DELETE", "/api/providers/chatgpt/login"),
            ("DELETE", "/api/providers/chatgpt"),
        ] {
            let response = crate::app()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("host", "127.0.0.1:8080")
                        .header("origin", "https://attacker.example")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
    }
    #[tokio::test]
    async fn deterministic_account_status_and_login_are_isolated() {
        let app = crate::app();
        for (method, path, expected) in [
            ("GET", "/api/providers/chatgpt/status", StatusCode::OK),
            (
                "POST",
                "/api/providers/chatgpt/login",
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            ("DELETE", "/api/providers/chatgpt", StatusCode::OK),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("host", "127.0.0.1:8080")
                        .header("origin", "http://localhost:4200")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()["cache-control"], "no-store");
            let body = to_bytes(response.into_body(), 8192).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if expected == StatusCode::OK {
                assert_eq!(json["connected"], false);
                assert_eq!(json["accountId"], serde_json::Value::Null);
            }
        }
    }
    #[tokio::test]
    async fn browser_same_origin_read_uses_referrer_without_origin() {
        let response = crate::app()
            .oneshot(
                Request::builder()
                    .uri("/api/providers/chatgpt/status")
                    .header("host", "127.0.0.1:8080")
                    .header("sec-fetch-site", "same-origin")
                    .header("referer", "http://localhost:4200/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}
