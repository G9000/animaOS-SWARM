#[allow(dead_code)]
mod support;

use support::{send_empty_request, send_json_request, test_app};

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

    // anima.yaml exists at the root and describes a single orchestrator.
    let yaml_path = root.join("anima.yaml");
    assert!(yaml_path.is_file());
    assert!(
        !root.join("anima.yaml.tmp").exists(),
        "tmp file must be renamed away"
    );
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(&yaml_path).unwrap()).unwrap();
    assert_eq!(yaml["name"], "Northwind Research");
    assert_eq!(yaml["orchestrator"]["name"], "Anima");
    assert_eq!(yaml["orchestrator"]["bio"], "A vigilant chief of staff.");
    assert_eq!(yaml["provider"], "deterministic");
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
        !root.join("anima.yaml").exists(),
        "failed bootstrap must not write anima.yaml"
    );
    assert!(
        !root.join("anima.yaml.tmp").exists(),
        "failed bootstrap must not leave a tmp file"
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
async fn bootstrap_rejects_blank_bio() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-boot-bio-{}", std::process::id()));
    let mut body = bootstrap_body(&root);
    body["agent"]["bio"] = serde_json::json!("  ");
    let (status, _) =
        send_json_request(&app, "POST", "/api/workspace/bootstrap", &body.to_string()).await;
    assert_eq!(status, 400);
    assert!(
        !root.join("anima.yaml").exists(),
        "blank bio must not produce anima.yaml"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn bootstrap_conflicts_when_already_bootstrapped() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-boot-twice-{}", std::process::id()));
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &bootstrap_body(&root).to_string(),
    )
    .await;
    assert_eq!(status, 201, "body: {body}");

    // Second bootstrap (even against a different root) is rejected.
    let other_root =
        std::env::temp_dir().join(format!("anima-boot-twice-other-{}", std::process::id()));
    let (status, _) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &bootstrap_body(&other_root).to_string(),
    )
    .await;
    assert_eq!(status, 409);

    let (_, agents) = send_json_request(&app, "GET", "/api/agents", "").await;
    let agents: serde_json::Value = serde_json::from_str(&agents).unwrap();
    assert_eq!(agents["agents"].as_array().unwrap().len(), 1);

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&other_root).ok();
}

#[tokio::test]
async fn bootstrap_rolls_back_when_agency_yaml_write_fails() {
    let app = test_app();
    let root = std::env::temp_dir().join(format!("anima-boot-iofail-{}", std::process::id()));
    // Force the anima.yaml write to fail: pre-create the target path as a
    // directory so the atomic rename cannot replace it with a file.
    std::fs::create_dir_all(root.join("anima.yaml")).expect("anima.yaml dir placeholder");

    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &bootstrap_body(&root).to_string(),
    )
    .await;
    assert_eq!(status, 503, "body: {body}");

    // Rollback also cleans up the tmp file from the atomic write.
    assert!(
        !root.join("anima.yaml.tmp").exists(),
        "tmp file must be removed on rollback"
    );

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

fn inspect_uri(root: &std::path::Path) -> String {
    support::query_uri(
        "/api/workspace/inspect",
        "rootPath",
        root.to_str().expect("temp root is utf-8"),
    )
}

const VALID_AGENCY_YAML: &str = r#"name: Northwind Research
description: Continuous equity research
mission: Continuous equity research
values: [cite sources]
model: kimi-k2
provider: moonshot
strategy: supervisor
orchestrator:
  name: Anima
  bio: A vigilant chief of staff.
  system: You are Anima.
  model: kimi-k2
  tools: [read_file]
agents:
  - name: Scout
    bio: A scout.
    system: You are Scout.
"#;

#[tokio::test]
async fn inspect_returns_found_false_without_yaml() {
    let root = support::use_temp_workspace_root("inspect-empty");
    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", &inspect_uri(root.path())).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"found\":false"), "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).expect("body is json");
    assert!(body.get("companyName").is_none(), "{body}");
    assert!(body.get("orchestrator").is_none(), "{body}");
}

#[tokio::test]
async fn inspect_returns_preview_for_valid_yaml() {
    let root = support::use_temp_workspace_root("inspect-valid");
    std::fs::write(root.path().join("anima.yaml"), VALID_AGENCY_YAML).expect("yaml writes");
    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", &inspect_uri(root.path())).await;
    assert_eq!(status, 200, "{body}");

    let body: serde_json::Value = serde_json::from_str(&body).expect("body is json");
    assert_eq!(body["found"], true, "{body}");
    assert_eq!(body["companyName"], "Northwind Research", "{body}");
    assert_eq!(body["mission"], "Continuous equity research", "{body}");
    assert_eq!(
        body["values"],
        serde_json::json!(["cite sources"]),
        "{body}"
    );

    let orchestrator = &body["orchestrator"];
    assert_eq!(orchestrator["name"], "Anima", "{body}");
    assert_eq!(orchestrator["bio"], "A vigilant chief of staff.", "{body}");
    assert_eq!(orchestrator["provider"], "moonshot", "{body}");
    assert_eq!(orchestrator["model"], "kimi-k2", "{body}");

    let workers = body["workers"].as_array().expect("workers is an array");
    assert_eq!(workers.len(), 1, "{body}");
    assert_eq!(workers[0]["name"], "Scout", "{body}");
    assert_eq!(workers[0]["model"], "kimi-k2", "{body}");
    assert!(
        workers[0].get("bio").is_none(),
        "worker previews omit bio: {body}"
    );
    assert!(
        workers[0].get("system").is_none(),
        "worker previews omit system: {body}"
    );

    assert!(
        body["providerAvailable"].is_boolean(),
        "providerAvailable present: {body}"
    );
}

#[tokio::test]
async fn inspect_rejects_malformed_yaml() {
    let root = support::use_temp_workspace_root("inspect-malformed");
    std::fs::write(root.path().join("anima.yaml"), "{{ not yaml").expect("yaml writes");
    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", &inspect_uri(root.path())).await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn inspect_rejects_blank_orchestrator_bio() {
    let root = support::use_temp_workspace_root("inspect-badfields");
    let yaml = VALID_AGENCY_YAML.replace("  bio: A vigilant chief of staff.\n", "  bio: \"\"\n");
    std::fs::write(root.path().join("anima.yaml"), yaml).expect("yaml writes");
    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", &inspect_uri(root.path())).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("orchestrator.bio"), "{body}");
}

#[tokio::test]
async fn inspect_rejects_missing_root_path_param_with_json_error() {
    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", "/api/workspace/inspect").await;
    assert_eq!(status, 400, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).expect("error body is json");
    assert_eq!(body["error"], "rootPath is required", "{body}");
}

#[tokio::test]
async fn inspect_rejects_empty_root_path_param() {
    let app = test_app();
    let uri = support::query_uri("/api/workspace/inspect", "rootPath", "");
    let (status, body) = send_empty_request(&app, "GET", &uri).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("rootPath is required"), "{body}");
}

#[tokio::test]
async fn inspect_defaults_provider_to_openai_when_yaml_omits_provider() {
    let root = support::use_temp_workspace_root("inspect-default-provider");
    let yaml = VALID_AGENCY_YAML.replace("provider: moonshot\n", "");
    std::fs::write(root.path().join("anima.yaml"), yaml).expect("yaml writes");
    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", &inspect_uri(root.path())).await;
    assert_eq!(status, 200, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).expect("body is json");
    assert_eq!(body["found"], true, "{body}");
    assert_eq!(body["orchestrator"]["provider"], "openai", "{body}");
    assert_eq!(body["workers"][0]["provider"], "openai", "{body}");
    assert!(
        body["providerAvailable"].is_boolean(),
        "providerAvailable present: {body}"
    );
}

#[tokio::test]
async fn inspect_reports_provider_unavailable_without_moonshot_keys() {
    let root = support::use_temp_workspace_root("inspect-provider-keys");
    std::fs::write(root.path().join("anima.yaml"), VALID_AGENCY_YAML).expect("yaml writes");
    // Pin the environment: moonshot reports configured only when one of its
    // API-key env vars is set, so clear them all for a deterministic result.
    // Serialized with the other inspect tests via the workspace-root lock
    // held by `root`.
    let saved: Vec<(&'static str, Option<std::ffi::OsString>)> = [
        "MOONSHOT_API_KEY",
        "MOONSHOT_KEY",
        "MOONSHOT_TOKEN",
        "KIMI_API_KEY",
    ]
    .into_iter()
    .map(|name| {
        let previous = std::env::var_os(name);
        std::env::remove_var(name);
        (name, previous)
    })
    .collect();

    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", &inspect_uri(root.path())).await;

    for (name, previous) in saved {
        if let Some(previous) = previous {
            std::env::set_var(name, previous);
        }
    }

    assert_eq!(status, 200, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).expect("body is json");
    assert_eq!(body["found"], true, "{body}");
    assert_eq!(body["providerAvailable"], false, "{body}");
}
