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

#[tokio::test]
async fn put_workspace_rejects_relative_path() {
    let app = test_app();
    let (status, body) = send_json_request(
        &app,
        "PUT",
        "/api/workspace",
        &serde_json::json!({
            "rootPath": "relative/folder",
            "companyName": "Northwind",
            "mission": "Research"
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("absolute"), "{body}");
}

#[tokio::test]
async fn put_workspace_validate_only_does_not_persist() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-validate-{}", std::process::id()));
    let (status, body) = send_json_request(
        &app,
        "PUT",
        "/api/workspace",
        &serde_json::json!({
            "rootPath": root,
            "companyName": "Northwind",
            "mission": "Research",
            "validateOnly": true
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (_, body) = send_json_request(&app, "GET", "/api/workspace", "").await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["configured"], false, "{body}");
    assert!(!root.exists(), "validate-only must not create the folder");
}

#[tokio::test]
async fn put_workspace_persists_and_creates_folder() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-persist-{}", std::process::id()));
    let (status, body) = send_json_request(
        &app,
        "PUT",
        "/api/workspace",
        &serde_json::json!({
            "rootPath": root,
            "companyName": "Northwind",
            "mission": "Research",
            "values": ["cite sources"]
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(root.is_dir());
    let (_, body) = send_json_request(&app, "GET", "/api/workspace", "").await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["configured"], true, "{body}");
    assert_eq!(body["workspace"]["companyName"], "Northwind", "{body}");
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn put_workspace_validate_only_reports_root_path_exists() {
    let app = test_app();

    let missing = std::env::temp_dir().join(format!("anima-missing-{}", std::process::id()));
    let (status, body) = send_json_request(
        &app,
        "PUT",
        "/api/workspace",
        &serde_json::json!({
            "rootPath": missing,
            "companyName": "Northwind",
            "mission": "Research",
            "validateOnly": true
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["rootPathExists"], false, "{body}");
    assert!(!missing.exists(), "validate-only must not create the folder");

    let existing = std::env::temp_dir();
    let (status, body) = send_json_request(
        &app,
        "PUT",
        "/api/workspace",
        &serde_json::json!({
            "rootPath": existing,
            "companyName": "Northwind",
            "mission": "Research",
            "validateOnly": true
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["rootPathExists"], true, "{body}");
}

#[tokio::test]
async fn put_workspace_validate_only_reflects_current_configuration() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-configured-{}", std::process::id()));
    let (status, body) = send_json_request(
        &app,
        "PUT",
        "/api/workspace",
        &serde_json::json!({
            "rootPath": root,
            "companyName": "Northwind",
            "mission": "Research"
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let probe = std::env::temp_dir().join(format!("anima-probe-{}", std::process::id()));
    let (status, body) = send_json_request(
        &app,
        "PUT",
        "/api/workspace",
        &serde_json::json!({
            "rootPath": probe,
            "companyName": "Contoso",
            "mission": "Sales",
            "validateOnly": true
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        body["configured"], true,
        "validate-only must reflect the already-configured daemon: {body}"
    );
    assert!(!probe.exists(), "validate-only must not create the folder");

    std::fs::remove_dir_all(&root).ok();
}
