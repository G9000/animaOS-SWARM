#[allow(dead_code)]
mod support;

use anima_daemon::{app_with_configured_persistence, DaemonConfig};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use support::{send_empty_request, send_json_request, send_request, test_app};

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
    assert_eq!(body["workers"], serde_json::json!([]));
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
async fn inspect_rejects_yaml_without_mission_or_description() {
    let root = support::use_temp_workspace_root("inspect-no-mission");
    let yaml = VALID_AGENCY_YAML
        .replace("description: Continuous equity research\n", "")
        .replace("mission: Continuous equity research\n", "");
    std::fs::write(root.path().join("anima.yaml"), yaml).expect("yaml writes");
    let app = test_app();
    let (status, body) = send_empty_request(&app, "GET", &inspect_uri(root.path())).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("mission or description is required"),
        "{body}"
    );
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

const RESUME_AGENCY_YAML: &str = r#"name: Northwind Research
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
  - name: Scribe
    bio: A scribe.
    system: You are Scribe.
"#;

#[tokio::test]
async fn resume_adopts_workspace_with_orchestrator_and_workers() {
    let root = support::use_temp_workspace_root("resume-adopt");
    let yaml_path = root.path().join("anima.yaml");
    std::fs::write(&yaml_path, RESUME_AGENCY_YAML).expect("yaml writes");
    let yaml_before = std::fs::read(&yaml_path).expect("yaml reads");
    let app = test_app();

    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/resume",
        &serde_json::json!({ "rootPath": root.path() }).to_string(),
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&body).expect("body is json");
    assert_eq!(status, 201, "body: {body}");
    assert_eq!(
        body["workspace"]["companyName"], "Northwind Research",
        "{body}"
    );
    assert_eq!(body["orchestrator"]["state"]["name"], "Anima", "{body}");
    assert_eq!(
        body["skipped"],
        serde_json::json!([]),
        "fresh adopt without collisions skips nothing: {body}"
    );
    let workers = body["workers"].as_array().expect("workers is an array");
    assert_eq!(workers.len(), 2, "{body}");
    assert_eq!(workers[0]["state"]["name"], "Scout", "{body}");
    assert_eq!(workers[1]["state"]["name"], "Scribe", "{body}");

    // Workspace config is live and the whole roster exists.
    let (_, workspace) = send_json_request(&app, "GET", "/api/workspace", "").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], true, "{workspace}");
    let (_, agents) = send_json_request(&app, "GET", "/api/agents", "").await;
    let agents: serde_json::Value = serde_json::from_str(&agents).unwrap();
    assert_eq!(agents["agents"].as_array().unwrap().len(), 3, "{agents}");

    // Resume never owns the yaml: it must be byte-identical afterwards.
    let yaml_after = std::fs::read(&yaml_path).expect("yaml reads");
    assert_eq!(yaml_before, yaml_after, "resume must not modify anima.yaml");
}

#[tokio::test]
async fn resume_rejects_unknown_tool_without_side_effects() {
    let root = support::use_temp_workspace_root("resume-badtool");
    // The WORKER carries the unknown tool, so the orchestrator is created
    // first and the mid-batch failure must roll it back.
    let yaml = RESUME_AGENCY_YAML.replace(
        "  - name: Scout\n    bio: A scout.\n    system: You are Scout.\n",
        "  - name: Scout\n    bio: A scout.\n    system: You are Scout.\n    tools: [not_a_real_tool]\n",
    );
    assert!(yaml.contains("not_a_real_tool"), "fixture edit applied");
    std::fs::write(root.path().join("anima.yaml"), yaml).expect("yaml writes");
    let app = test_app();

    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/resume",
        &serde_json::json!({ "rootPath": root.path() }).to_string(),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("Scout"),
        "error names the failing agent: {body}"
    );

    // The orchestrator was created before the worker failed, so an empty
    // roster proves the in-guard batch rollback ran.
    let (_, agents) = send_json_request(&app, "GET", "/api/agents", "").await;
    let agents: serde_json::Value = serde_json::from_str(&agents).unwrap();
    assert_eq!(
        agents["agents"].as_array().unwrap().len(),
        0,
        "no agents may be created on failure: {agents}"
    );
    let (_, workspace) = send_json_request(&app, "GET", "/api/workspace", "").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], false, "{workspace}");
}

#[tokio::test]
async fn resume_without_yaml_returns_400() {
    let root = support::use_temp_workspace_root("resume-noyaml");
    let app = test_app();
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/resume",
        &serde_json::json!({ "rootPath": root.path() }).to_string(),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("anima.yaml"), "{body}");
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

// --- Task 4: resume conflict, idempotency, and rollback semantics ----------

/// Two-agent fixture (orchestrator "Anima" + worker "Scout") for the
/// idempotency tests.
const RESUME_RESTORE_YAML: &str = r#"name: Northwind Research
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

/// A roster whose names deliberately differ from the bootstrap agent, so a
/// different-root 409 cannot be confused with a name collision.
const OTHER_ROOT_YAML: &str = r#"name: Contoso
description: Sales
mission: Sales
model: kimi-k2
strategy: supervisor
orchestrator:
  name: Atlas
  bio: A tireless lead.
  system: You are Atlas.
agents:
  - name: Scout
    system: You are Scout.
"#;

async fn resume_workspace(app: &axum::Router, root: &std::path::Path) -> (StatusCode, String) {
    send_json_request(
        app,
        "POST",
        "/api/workspace/resume",
        &serde_json::json!({ "rootPath": root }).to_string(),
    )
    .await
}

async fn delete_agent(app: &axum::Router, agent_id: &str) -> (StatusCode, String) {
    send_request(
        app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/agents/{agent_id}"))
            .header("host", "127.0.0.1:8080")
            .header("origin", "http://localhost:4200")
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

/// The live roster as (id, name) pairs, sorted by name for stable assertions.
fn roster(body: &str) -> Vec<(String, String)> {
    let body: serde_json::Value = serde_json::from_str(body).expect("body is json");
    let mut agents = body["agents"]
        .as_array()
        .expect("agents is an array")
        .iter()
        .map(|agent| {
            (
                agent["state"]["id"].as_str().expect("id").to_string(),
                agent["state"]["name"].as_str().expect("name").to_string(),
            )
        })
        .collect::<Vec<_>>();
    agents.sort_by(|left, right| left.1.cmp(&right.1));
    agents
}

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
async fn resume_conflicts_when_configured_for_different_root() {
    let guard = support::use_temp_workspace_root("resume-diff-root");
    let root_a = guard.path().join("a");
    let root_b = guard.path().join("b");
    std::fs::create_dir_all(&root_b).expect("root b created");
    std::fs::write(root_b.join("anima.yaml"), OTHER_ROOT_YAML).expect("yaml writes");
    let app = test_app();

    // Bootstrap configures the daemon for root A.
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &bootstrap_body(&root_a).to_string(),
    )
    .await;
    assert_eq!(status, 201, "body: {body}");

    // Resuming root B is a conflict that names root A.
    let (status, body) = resume_workspace(&app, &root_b).await;
    assert_eq!(status, 409, "{body}");
    let canonical_a = root_a.canonicalize().expect("root a canonicalizes");
    let error: serde_json::Value = serde_json::from_str(&body).expect("error body is json");
    let message = error["error"].as_str().expect("error message");
    assert!(
        message.contains(&canonical_a.display().to_string()),
        "conflict names the configured root: {body}"
    );

    // Nothing from root B's yaml was created: only the bootstrap agent exists.
    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(
        roster(&agents)
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["Anima"],
        "{agents}"
    );
}

#[tokio::test]
async fn resume_same_root_restores_missing_agents_only() {
    let root = support::use_temp_workspace_root("resume-restore");
    std::fs::write(root.path().join("anima.yaml"), RESUME_RESTORE_YAML).expect("yaml writes");
    let app = test_app();

    let (status, body) = resume_workspace(&app, root.path()).await;
    assert_eq!(status, 201, "{body}");
    let first: serde_json::Value = serde_json::from_str(&body).expect("body is json");
    let orchestrator_id = first["orchestrator"]["state"]["id"]
        .as_str()
        .expect("orchestrator id")
        .to_string();
    let scout_id = first["workers"][0]["state"]["id"]
        .as_str()
        .expect("worker id")
        .to_string();

    // The worker disappears (crash, owner cleanup, ...); a re-resume of the
    // same root must restore exactly it.
    let (status, body) = delete_agent(&app, &scout_id).await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = resume_workspace(&app, root.path()).await;
    assert_eq!(status, 201, "{body}");
    let second: serde_json::Value = serde_json::from_str(&body).expect("body is json");

    // The response reports the PRE-EXISTING orchestrator, not a new one.
    assert_eq!(
        second["orchestrator"]["state"]["id"].as_str(),
        Some(orchestrator_id.as_str()),
        "{second}"
    );
    assert_eq!(
        second["skipped"],
        serde_json::json!(["Anima"]),
        "the kept orchestrator is reported as skipped: {second}"
    );
    let workers = second["workers"].as_array().expect("workers is an array");
    assert_eq!(workers.len(), 1, "{second}");
    assert_eq!(workers[0]["state"]["name"], "Scout", "{second}");
    assert_ne!(
        workers[0]["state"]["id"].as_str(),
        Some(scout_id.as_str()),
        "the restored worker is a fresh agent: {second}"
    );

    // The orchestrator was not duplicated and the roster is whole again.
    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    let roster = roster(&agents);
    assert_eq!(roster.len(), 2, "{agents}");
    assert_eq!(
        roster.iter().filter(|(_, name)| name == "Anima").count(),
        1,
        "orchestrator must not be duplicated: {agents}"
    );

    let (_, workspace) = send_empty_request(&app, "GET", "/api/workspace").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], true, "{workspace}");
}

#[tokio::test]
async fn resume_fresh_adopt_skips_persisted_agent_name_collisions() {
    let root = support::use_temp_workspace_root("resume-adopt-collision");
    std::fs::write(root.path().join("anima.yaml"), RESUME_RESTORE_YAML).expect("yaml writes");
    let app = test_app();

    // A standalone agent that happens to share the orchestrator's name.
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/agents",
        r#"{"name":"Anima","model":"gpt-5.4"}"#,
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let existing_id = support::extract_json_string_field(&body, "id");

    let (status, body) = resume_workspace(&app, root.path()).await;
    assert_eq!(status, 201, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).expect("body is json");

    // The persisted agent is kept and reported as the orchestrator; only the
    // missing worker is created.
    assert_eq!(
        body["orchestrator"]["state"]["id"].as_str(),
        Some(existing_id.as_str()),
        "{body}"
    );
    assert_eq!(
        body["skipped"],
        serde_json::json!(["Anima"]),
        "the name collision is reported as skipped: {body}"
    );
    let workers = body["workers"].as_array().expect("workers is an array");
    assert_eq!(workers.len(), 1, "{body}");
    assert_eq!(workers[0]["state"]["name"], "Scout", "{body}");

    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    let roster = roster(&agents);
    assert_eq!(roster.len(), 2, "{agents}");
    assert!(
        roster
            .iter()
            .any(|(id, name)| id == &existing_id && name == "Anima"),
        "pre-existing agent kept with its id: {agents}"
    );

    let (_, workspace) = send_empty_request(&app, "GET", "/api/workspace").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], true, "{workspace}");
}

#[tokio::test]
async fn resume_all_agents_exist_returns_409() {
    let root = support::use_temp_workspace_root("resume-nothing-to-do");
    std::fs::write(root.path().join("anima.yaml"), RESUME_RESTORE_YAML).expect("yaml writes");
    let app = test_app();

    let (status, body) = resume_workspace(&app, root.path()).await;
    assert_eq!(status, 201, "{body}");

    // A second resume with the full roster already live is a meaningful
    // "nothing to do" conflict rather than a silent 200.
    let (status, body) = resume_workspace(&app, root.path()).await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body.contains("all agents from anima.yaml already exist"),
        "{body}"
    );

    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 2, "{agents}");
}

#[tokio::test]
async fn resume_rolls_back_when_persist_fails() {
    let workspace = support::use_temp_workspace_root("resume-rollback");
    let control_plane_path = workspace.path().join("control-plane.json");
    // Serialized against the other workspace tests by the root lock held in
    // `workspace`.
    let _guard = EnvVarGuard::set("ANIMAOS_RS_CONTROL_PLANE_FILE", &control_plane_path);
    let adopt_root = workspace.path().join("adopt");
    std::fs::create_dir_all(&adopt_root).expect("adopt root created");
    std::fs::write(adopt_root.join("anima.yaml"), RESUME_RESTORE_YAML).expect("yaml writes");
    let app = app_with_configured_persistence(DaemonConfig::default())
        .await
        .expect("app configures persistence");

    // Force the next control-plane save to fail: replace the snapshot file
    // with a directory so the atomic rename cannot replace it (mirrors the
    // bootstrap rollback test's anima.yaml technique).
    std::fs::remove_file(&control_plane_path).expect("startup snapshot exists");
    std::fs::create_dir(&control_plane_path).expect("sabotage directory created");

    let (status, body) = resume_workspace(&app, &adopt_root).await;
    assert_eq!(status, 503, "{body}");

    // Full rollback: no agents, no live workspace configuration.
    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(
        roster(&agents).len(),
        0,
        "agents must be rolled back: {agents}"
    );
    let (_, workspace_body) = send_empty_request(&app, "GET", "/api/workspace").await;
    let workspace_body: serde_json::Value = serde_json::from_str(&workspace_body).unwrap();
    assert_eq!(
        workspace_body["configured"], false,
        "workspace must be rolled back: {workspace_body}"
    );
}

#[tokio::test]
async fn resume_rejects_yaml_internal_duplicate_names() {
    let root = support::use_temp_workspace_root("resume-dup-names");
    let yaml = RESUME_RESTORE_YAML.replace("  - name: Scout\n", "  - name: Anima\n");
    assert!(yaml.contains("  - name: Anima\n"), "fixture edit applied");
    std::fs::write(root.path().join("anima.yaml"), yaml).expect("yaml writes");
    let app = test_app();

    let (status, body) = resume_workspace(&app, root.path()).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("duplicate agent name 'Anima'"),
        "error names the duplicate: {body}"
    );

    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 0, "{agents}");
    let (_, workspace) = send_empty_request(&app, "GET", "/api/workspace").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], false, "{workspace}");
}

#[tokio::test]
async fn resume_rejects_whitespace_worker_system() {
    let root = support::use_temp_workspace_root("resume-blank-system");
    let yaml = RESUME_RESTORE_YAML.replace("    system: You are Scout.\n", "    system: \" \"\n");
    assert!(yaml.contains("system: \" \""), "fixture edit applied");
    std::fs::write(root.path().join("anima.yaml"), yaml).expect("yaml writes");
    let app = test_app();

    let (status, body) = resume_workspace(&app, root.path()).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("Scout"), "error names the agent: {body}");

    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 0, "{agents}");
    let (_, workspace) = send_empty_request(&app, "GET", "/api/workspace").await;
    let workspace: serde_json::Value = serde_json::from_str(&workspace).unwrap();
    assert_eq!(workspace["configured"], false, "{workspace}");
}

#[tokio::test]
async fn resumed_agents_survive_restart() {
    let workspace = support::use_temp_workspace_root("resume-restart");
    let control_plane_path = workspace.path().join("control-plane.json");
    let _guard = EnvVarGuard::set("ANIMAOS_RS_CONTROL_PLANE_FILE", &control_plane_path);
    std::fs::write(workspace.path().join("anima.yaml"), RESUME_RESTORE_YAML).expect("yaml writes");

    let first_app = app_with_configured_persistence(DaemonConfig::default())
        .await
        .expect("first app should configure persistence");
    let (status, body) = resume_workspace(&first_app, workspace.path()).await;
    assert_eq!(status, 201, "{body}");
    drop(first_app);

    // Respawn against the same control-plane file (mirrors
    // control_plane_store_recovers_agents_and_swarms_after_restart).
    let second_app = app_with_configured_persistence(DaemonConfig::default())
        .await
        .expect("second app should configure persistence");
    let (_, agents) = send_empty_request(&second_app, "GET", "/api/agents").await;
    assert_eq!(
        roster(&agents)
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["Anima", "Scout"],
        "resumed roster restored after restart: {agents}"
    );
    let (_, workspace_body) = send_empty_request(&second_app, "GET", "/api/workspace").await;
    let workspace_body: serde_json::Value = serde_json::from_str(&workspace_body).unwrap();
    assert_eq!(workspace_body["configured"], true, "{workspace_body}");
    let canonical_root = workspace.path().canonicalize().expect("root canonicalizes");
    assert_eq!(
        workspace_body["workspace"]["rootPath"].as_str(),
        Some(canonical_root.display().to_string().as_str()),
        "{workspace_body}"
    );
}

fn team_bootstrap_body(root: &std::path::Path) -> serde_json::Value {
    let mut body = bootstrap_body(root);
    let mut worker = body["agent"].clone();
    worker["name"] = serde_json::json!("Researcher");
    worker["system"] = serde_json::json!("Research and cite evidence.");
    body["workers"] = serde_json::json!([worker]);
    body
}

#[tokio::test]
async fn bootstrap_team_persists_yaml_and_can_resume() {
    let root = support::use_temp_workspace_root("bootstrap-team");
    let app = test_app();
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &team_bootstrap_body(root.path()).to_string(),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["workers"][0]["state"]["name"], "Researcher");
    assert_eq!(
        body["agent"]["state"]["config"]["settings"]["additional"]["workspaceRole"],
        "lead"
    );
    assert!(
        body["workers"][0]["state"]["config"]["settings"]["additional"]["workspaceRole"].is_null()
    );
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(root.path().join("anima.yaml")).unwrap())
            .unwrap();
    assert_eq!(yaml["agents"][0]["name"], "Researcher");
    let fresh = test_app();
    let (status, body) = resume_workspace(&fresh, root.path()).await;
    assert_eq!(status, 201, "{body}");
    let resumed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        resumed["orchestrator"]["state"]["config"]["settings"]["additional"]["workspaceRole"],
        "lead"
    );
    let (_, agents) = send_empty_request(&fresh, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 2);
}

#[tokio::test]
async fn bootstrap_team_validates_all_workers_before_mutation() {
    let root = support::use_temp_workspace_root("bootstrap-team-invalid");
    for (field, value) in [
        ("name", serde_json::json!(" anima ")),
        ("name", serde_json::json!(" ")),
        ("system", serde_json::json!(" ")),
        ("bio", serde_json::json!(" ")),
        ("model", serde_json::json!(" ")),
        ("provider", serde_json::json!("other-provider")),
        ("tools", serde_json::json!([])),
        ("presetId", serde_json::json!("missing-preset")),
        ("tools", serde_json::json!(["missing_tool"])),
    ] {
        let app = test_app();
        let candidate = root.path().join(field);
        let mut request = team_bootstrap_body(&candidate);
        request["workers"][0][field] = value;
        let (status, body) = send_json_request(
            &app,
            "POST",
            "/api/workspace/bootstrap",
            &request.to_string(),
        )
        .await;
        assert_eq!(status, 400, "{field}: {body}");
        assert!(!candidate.exists(), "invalid team must not create root");
        let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
        assert_eq!(roster(&agents).len(), 0);
    }
}

#[tokio::test]
async fn bootstrap_team_rolls_back_every_agent_on_yaml_failure() {
    let root = support::use_temp_workspace_root("bootstrap-team-rollback");
    std::fs::create_dir(root.path().join("anima.yaml")).unwrap();
    let app = test_app();
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &team_bootstrap_body(root.path()).to_string(),
    )
    .await;
    assert_eq!(status, 503, "{body}");
    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 0);
}

#[tokio::test]
async fn bootstrap_team_survives_restart_and_persist_failure_rolls_back() {
    let workspace = support::use_temp_workspace_root("bootstrap-team-persist");
    let control_plane_path = workspace.path().join("control-plane.json");
    let _guard = EnvVarGuard::set("ANIMAOS_RS_CONTROL_PLANE_FILE", &control_plane_path);
    let root = workspace.path().join("team");
    let app = app_with_configured_persistence(DaemonConfig::default())
        .await
        .unwrap();
    std::fs::remove_file(&control_plane_path).unwrap();
    std::fs::create_dir(&control_plane_path).unwrap();
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &team_bootstrap_body(&root).to_string(),
    )
    .await;
    assert_eq!(status, 503, "{body}");
    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 0);
    assert!(!root.join("anima.yaml").exists());
    let (_, body) = send_empty_request(&app, "GET", "/api/workspace").await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["configured"],
        false
    );
    std::fs::remove_dir(&control_plane_path).unwrap();
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &team_bootstrap_body(&root).to_string(),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    drop(app);
    let restored = app_with_configured_persistence(DaemonConfig::default())
        .await
        .unwrap();
    let (_, agents) = send_empty_request(&restored, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 2, "{agents}");
    assert!(roster(&agents).iter().any(|(_, name)| name == "Researcher"));
    let agents: serde_json::Value = serde_json::from_str(&agents).unwrap();
    let lead = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["state"]["name"] == "Anima")
        .unwrap();
    assert_eq!(
        lead["state"]["config"]["settings"]["additional"]["workspaceRole"],
        "lead"
    );
}

#[tokio::test]
async fn bootstrap_team_limit_and_concurrent_requests() {
    let root = support::use_temp_workspace_root("bootstrap-team-race");
    let app = test_app();
    let mut request = team_bootstrap_body(root.path());
    let worker = request["workers"][0].clone();
    request["workers"] = serde_json::Value::Array(
        (0..10)
            .map(|i| {
                let mut worker = worker.clone();
                worker["name"] = serde_json::json!(format!("Worker {i}"));
                worker
            })
            .collect(),
    );
    let (status, body) = send_json_request(
        &app,
        "POST",
        "/api/workspace/bootstrap",
        &request.to_string(),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    request["workers"].as_array_mut().unwrap().pop();
    let request = request.to_string();
    let (first, second) = tokio::join!(
        send_json_request(&app, "POST", "/api/workspace/bootstrap", &request),
        send_json_request(&app, "POST", "/api/workspace/bootstrap", &request)
    );
    let mut statuses = [first.0, second.0];
    statuses.sort();
    assert_eq!(statuses, [201, 409], "{first:?} {second:?}");
    let (_, agents) = send_empty_request(&app, "GET", "/api/agents").await;
    assert_eq!(roster(&agents).len(), 10);
}
