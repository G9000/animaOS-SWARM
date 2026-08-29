#[allow(dead_code)]
mod support;

use support::{send_json_request, test_app};

#[tokio::test]
async fn get_workspace_reports_unconfigured_with_default_root() {
    let app = test_app();
    let (status, body) = send_json_request(&app, "GET", "/api/workspace", "").await;
    assert_eq!(status, 200);

    let body: serde_json::Value = serde_json::from_str(&body).expect("body is json");
    assert_eq!(body["configured"], false);
    assert!(body["defaultRoot"]
        .as_str()
        .expect("defaultRoot is a string")
        .len()
        > 0);
    assert!(body["workspace"].is_null());
    assert!(body.get("rootPathExists").is_none());
}

#[tokio::test]
async fn openapi_spec_lists_workspace_endpoint() {
    let app = test_app();
    let (status, body) = send_json_request(&app, "GET", "/openapi.json", "").await;
    assert_eq!(status, 200);
    assert!(body.contains("\"/api/workspace\""), "{body}");
    assert!(body.contains("WorkspaceResponse"), "{body}");
}
