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
    assert!(
        body["defaultRoot"]
            .as_str()
            .expect("defaultRoot is a string")
            .len()
            > 0
    );
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
    assert!(
        !missing.exists(),
        "validate-only must not create the folder"
    );

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

fn bootstrap_body(root: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "workspace": {
            "rootPath": root,
            "companyName": "Northwind Research",
            "mission": "Continuous equity research",
            "values": ["cite sources"]
        },
        "agent": {
            "name": "Anima",
            "presetId": "chief-of-staff",
            "bio": "A vigilant chief of staff.",
            "adjectives": ["vigilant", "concise", "proactive"],
            "style": "brief, numbered",
            "system": "You are Anima, chief of staff at Northwind Research...",
            "provider": "deterministic",
            "model": "deterministic",
            "tools": ["memory_search", "memory_add", "recent_memories", "get_current_time", "calculate", "read_file"]
        }
    })
}

#[tokio::test]
async fn bootstrap_creates_workspace_agency_file_and_agent() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-boot-{}", std::process::id()));
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &bootstrap_body(&root).to_string(),
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status, 201, "body: {body}");
    assert_eq!(body["workspace"]["companyName"], "Northwind Research");
    assert_eq!(
        body["agent"]["state"]["config"]["bio"],
        "A vigilant chief of staff."
    );
    assert_eq!(
        body["agent"]["state"]["config"]["adjectives"],
        serde_json::json!(["vigilant", "concise", "proactive"])
    );

    // agency.yaml exists at the root and describes a single orchestrator.
    let yaml_path = root.join("agency.yaml");
    assert!(yaml_path.is_file());
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(&yaml_path).unwrap()).unwrap();
    assert_eq!(yaml["name"], "Northwind Research");
    assert_eq!(yaml["orchestrator"]["name"], "Anima");
    assert_eq!(yaml["strategy"], "supervisor");

    // Workspace config is live.
    let (_, workspace) = send_json_request(&app, "GET", "/api/workspace", "").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], true);

    // The agent's tools were resolved to canonical descriptors.
    let tools = body["agent"]["state"]["config"]["tools"]
        .as_array()
        .unwrap();
    let read_file = tools.iter().find(|t| t["name"] == "read_file").unwrap();
    assert!(read_file["description"].as_str().unwrap().len() > 0);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn bootstrap_rejects_unknown_tools_without_side_effects() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-boot-bad-{}", std::process::id()));
    let mut body = bootstrap_body(&root);
    body["agent"]["tools"] = serde_json::json!(["definitely_not_a_tool"]);
    let (status, _) =
        send_json_request(&app, "POST", "/api/workspace/bootstrap", &body.to_string()).await;
    assert_eq!(status, 400);
    assert!(
        !root.join("agency.yaml").exists(),
        "failed bootstrap must not write agency.yaml"
    );
    let (_, workspace) = send_json_request(&app, "GET", "/api/workspace", "").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], false);
    let (_, agents) = send_json_request(&app, "GET", "/api/agents", "").await;
    let agents: serde_json::Value = serde_json::from_str(&agents).unwrap();
    assert_eq!(agents["agents"].as_array().unwrap().len(), 0);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn bootstrap_rejects_empty_system() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-boot-sys-{}", std::process::id()));
    let mut body = bootstrap_body(&root);
    body["agent"]["system"] = serde_json::json!("  ");
    let (status, _) =
        send_json_request(&app, "POST", "/api/workspace/bootstrap", &body.to_string()).await;
    assert_eq!(status, 400);
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn bootstrap_rolls_back_when_agency_yaml_write_fails() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-boot-iofail-{}", std::process::id()));
    // Force the agency.yaml write to fail: pre-create the target path as a
    // directory so std::fs::write cannot open it as a file.
    std::fs::create_dir_all(root.join("agency.yaml")).expect("agency.yaml dir placeholder");

    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &bootstrap_body(&root).to_string(),
    )
    .await;
    assert_eq!(status, 503, "body: {body}");

    // Full rollback: no agent, no live workspace configuration.
    let (_, agents) = send_json_request(&app, "GET", "/api/agents", "").await;
    let agents: serde_json::Value = serde_json::from_str(&agents).unwrap();
    assert_eq!(
        agents["agents"].as_array().unwrap().len(),
        0,
        "agent must be rolled back"
    );
    let (_, workspace) = send_json_request(&app, "GET", "/api/workspace", "").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(
        workspace["configured"], false,
        "workspace must be rolled back"
    );

    std::fs::remove_dir_all(&root).ok();
}
