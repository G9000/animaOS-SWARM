# Workspace-First Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild Guided Focus onboarding as a 5-step workspace-first flow (Workspace → Intelligence → Agent → Access → Review) backed by new daemon workspace endpoints and an atomic bootstrap that creates workspace config + `anima.yaml` + orchestrator runtime agent.

**Architecture:** The daemon gains a persisted `WorkspaceConfig` in the control-plane snapshot (store v3 → v4) and owns workspace-root truth; `ANIMAOS_WORKSPACE_ROOT` becomes the initial default. New endpoints: `GET/PUT /api/workspace`, `POST /api/agents/generate-profile`, `POST /api/workspace/bootstrap`. The web app drives them from new `WorkspaceStep` / `AgentStep` components; profile generation drafts `bio`/`adjectives`/`style`/`system` from a preset + plain-language intent, with preset templates as the offline fallback.

**Tech Stack:** Rust (axum, tokio, serde, utoipa) in `hosts/rust-daemon`; React + TypeScript + Vitest + Testing Library in `apps/web`.

**Spec:** `docs/superpowers/specs/2026-08-30-workspace-first-onboarding-design.md`

---

## Key Facts Discovered During Planning (read first)

- `hosts/rust-daemon/src/routes/contracts/agents.rs` — `AgentConfigRequest` **already accepts** `bio`, `lore`, `knowledge`, `topics`, `adjectives`, `style`, `system`. No contract change is needed to carry personality on agent creation; only the web client needs to send them.
- `hosts/rust-daemon/src/state.rs:2018` — `DaemonState::create_agent` already calls `self.resolve_agent_tools(config.tools)?` (state.rs:2264), which expands name-only tool slugs into canonical registry descriptors and rejects unknown slugs. Bootstrap reuses this for free by building an `AgentConfig` with name-only descriptors.
- `hosts/rust-daemon/src/control_plane_store.rs:17` — `CONTROL_PLANE_STORE_VERSION = 3`. Loading rejects only versions **greater than** the current; `#[serde(default)]` on a new field gives free migration. Bump to 4.
- `hosts/rust-daemon/src/tools/workspace.rs:4` — `workspace_root_path(tool_name)` reads `ANIMAOS_WORKSPACE_ROOT` per call, falling back to `std::env::current_dir()`. Callers: `tools/filesystem.rs`, `tools/filesystem/search.rs`, `tools/todo.rs`, `routes/agencies.rs`. Do **not** use `std::env::set_var` at runtime (unsafe in edition 2024); thread the configured root through `ToolExecutionContext` instead.
- `hosts/rust-daemon/src/routes/agencies.rs:35-77` — `AgencyYamlConfig` / `AgencyYamlAgent` are private serde structs that already model exactly the agency file bootstrap must write. Make them `pub(crate)` and reuse them.
- Route handler pattern (`hosts/rust-daemon/src/routes/mod.rs:835`): an `async fn xxx_entry(State(state): State<AppState>, request: AxumRequest)` wrapper reads the limited body, takes `state.agent_runs.control_plane_transaction().await`, delegates to a `handle_xxx` function in a routes submodule, and maps `Ok` → `json_response(StatusCode::…)`, `Err` → `error.into_response()`. New endpoints follow this exactly, including a `#[utoipa::path]` annotation and registration in the OpenAPI doc (find the existing `create_agent_entry` annotation + the `openapi_entry` paths list and mirror them).
- Web test command: `bun x nx test @animaOS-SWARM/web --skipNxCache` (focus one file with `--testFile=Foo.test.tsx`).
- Rust test command (Windows, daemon binary may be locked): `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`. Rust integration tests live in `hosts/rust-daemon/tests/` and use `tests/support/mod.rs` helpers (`test_app`, `send_json_request`, `use_temp_workspace_root`).
- The onboarding draft lives entirely in `OnboardingFlow` component state; step components are controlled (typed values + callbacks in, no network calls).

## Design Refinement (supersedes one spec detail)

The spec says `generate-profile` embeds "the workspace mission and values" — but during onboarding the workspace config is deliberately **not yet persisted** (Verify is validate-only). Therefore `POST /api/agents/generate-profile` takes the workspace identity **in the request body** (`workspace: { companyName, mission, values }`), not from daemon state. Same for bootstrap, which carries both `workspace` and `agent` payloads.

---

## Phase 1 — Daemon: workspace state and persistence

### Task 1: `WorkspaceConfig` + control-plane store v4

**Files:**
- Modify: `hosts/rust-daemon/src/control_plane_store.rs` (struct + version)
- Modify: `hosts/rust-daemon/src/state.rs` (field, restore, persist assembly)
- Test: `hosts/rust-daemon/src/control_plane_store.rs` (`#[cfg(test)]` module at the bottom of the file)

- [ ] **Step 1: Write the failing tests**

Add to the test module in `control_plane_store.rs`:

```rust
#[test]
fn v3_snapshot_without_workspace_loads_as_unconfigured() {
    let json = r#"{
        "version": 3,
        "agents": [],
        "swarms": [],
        "connectors": [],
        "credentialCleanup": [],
        "inbound": [],
        "outbound": [],
        "schedules": []
    }"#;
    let snapshot: ControlPlaneSnapshot = serde_json::from_str(json).expect("v3 snapshot parses");
    assert!(snapshot.workspace.is_none());
}

#[test]
fn workspace_config_round_trips() {
    let config = WorkspaceConfig {
        root_path: PathBuf::from("C:\\workspaces\\northwind"),
        company_name: "Northwind Research".into(),
        mission: "Continuous equity research".into(),
        values: vec!["cite sources".into()],
    };
    let snapshot = ControlPlaneSnapshot {
        workspace: Some(config.clone()),
        ..Default::default()
    };
    let payload = serde_json::to_string(&snapshot).expect("serialize");
    let restored: ControlPlaneSnapshot = serde_json::from_str(&payload).expect("deserialize");
    assert_eq!(restored.workspace, Some(config));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: FAIL — `WorkspaceConfig` and `workspace` field do not exist.

- [ ] **Step 3: Implement**

In `control_plane_store.rs`:

```rust
const CONTROL_PLANE_STORE_VERSION: u32 = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceConfig {
    pub(crate) root_path: PathBuf,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    #[serde(default)]
    pub(crate) values: Vec<String>,
}
```

Add to `ControlPlaneSnapshot`:

```rust
    #[serde(default)]
    pub(crate) workspace: Option<WorkspaceConfig>,
```

In `state.rs`:
- Add field to `DaemonState` (next to `schedules`): `pub(crate) workspace: Option<WorkspaceConfig>,`
- Initialize to `None` in every `DaemonState` constructor (grep for `schedules: HashMap::new()` to find them all).
- In the control-plane restore path (grep `resolve_restored_agent_tools` at state.rs:2100 — the restore function containing it): add `state.workspace = snapshot.workspace.clone();` (adapt to the actual receiver shape).
- In the snapshot assembly used by `control_plane_persist_request` (grep `ControlPlaneSnapshot {` in state.rs): add `workspace: self.workspace.clone(),`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: PASS (the two new tests plus the full existing suite — watch for struct-literal compile errors at every `ControlPlaneSnapshot { ... }` construction site; add `workspace: None` or `..Default::default()` where needed).

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/state.rs
git commit -m "feat(daemon): persist workspace config in control plane v4"
```

---

### Task 2: Workspace root resolution from daemon state

**Files:**
- Modify: `hosts/rust-daemon/src/tools/workspace.rs:4-15`
- Modify: `hosts/rust-daemon/src/tools.rs` (`ToolExecutionContext`)
- Modify: `hosts/rust-daemon/src/tools/filesystem.rs`, `hosts/rust-daemon/src/tools/filesystem/search.rs`, `hosts/rust-daemon/src/tools/todo.rs` (callers)
- Modify: `hosts/rust-daemon/src/state.rs` (ToolExecutionContext construction sites — grep `ToolExecutionContext::new`)
- Test: `hosts/rust-daemon/src/tools/tests.rs`

- [ ] **Step 1: Write the failing test**

In `tools/tests.rs`:

```rust
#[test]
fn configured_workspace_root_overrides_env_var() {
    let configured = PathBuf::from("C:\\configured\\root");
    let resolved = workspace_root_path("read_file", Some(configured.as_path()))
        .expect("configured root resolves");
    assert_eq!(resolved, configured);
}

#[test]
fn missing_config_falls_back_to_env_or_cwd() {
    // With no override, behavior is unchanged from today.
    let resolved = workspace_root_path("read_file", None);
    assert!(resolved.is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: FAIL — `workspace_root_path` takes one argument today.

- [ ] **Step 3: Implement**

In `tools/workspace.rs`:

```rust
pub(crate) fn workspace_root_path(
    tool_name: &str,
    configured: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(root) = configured {
        return Ok(root.to_path_buf());
    }
    match std::env::var("ANIMAOS_WORKSPACE_ROOT") {
        // ...existing body unchanged...
    }
}
```

In `tools.rs`, add to `ToolExecutionContext`:

```rust
    pub(super) workspace_root: Option<PathBuf>,
```

(Add a `PathBuf` import and a corresponding `Option<PathBuf>` parameter to `ToolExecutionContext::new`.)

Update every tool handler that calls `workspace_root_path("…")` to pass the context's override, e.g. in `tools/filesystem/search.rs:17`:

```rust
let workspace_root = workspace_root_path("read_file", ctx_workspace_root(&context))?;
```

Add a small helper in `tools.rs`:

```rust
pub(crate) fn ctx_workspace_root(context: &ToolExecutionContext) -> Option<&Path> {
    context.workspace_root.as_deref()
}
```

In `state.rs`, at each `ToolExecutionContext::new(...)` call site, pass `self.workspace.as_ref().map(|w| w.root_path.clone())` as the new argument.

Leave `routes/agencies.rs` callers of `workspace_root_path("agency_create")` passing `None` for now — bootstrap (Task 6) handles agency-at-root writing; generic agency scaffolding keeps env behavior. Update the call sites to the two-arg form with `None`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: PASS. All existing workspace-tool tests still pass because `use_temp_workspace_root` sets the env var and no override is configured in those tests.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/tools hosts/rust-daemon/src/tools.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/routes/agencies.rs
git commit -m "feat(daemon): resolve workspace root from daemon state with env fallback"
```

---

## Phase 2 — Daemon: workspace endpoints

### Task 3: Workspace contracts + `GET /api/workspace`

**Files:**
- Create: `hosts/rust-daemon/src/routes/contracts/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/mod.rs` (re-export)
- Create: `hosts/rust-daemon/src/routes/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (route + entry)
- Test: `hosts/rust-daemon/tests/workspace_api.rs` (new; reuse helpers from `tests/support/mod.rs`)

- [ ] **Step 1: Write the failing test**

Create `hosts/rust-daemon/tests/workspace_api.rs` modeled on `tests/health.rs` (look at its imports and `test_app` usage):

```rust
mod support;

use support::{send_json_request, test_app};

#[tokio::test]
async fn get_workspace_reports_unconfigured_with_default_root() {
    let app = test_app().await;
    let (status, body) = send_json_request(&app, "GET", "/api/workspace", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["configured"], false);
    assert!(body["defaultRoot"].as_str().expect("defaultRoot").len() > 0);
    assert!(body["workspace"].is_null());
}
```

(Check `tests/support/mod.rs` for the exact `send_json_request` signature and adapt; register `mod support;` the same way existing test files do.)

- [ ] **Step 2: Run test to verify it fails**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: FAIL — 404, route does not exist.

- [ ] **Step 3: Implement**

Create `contracts/workspace.rs`:

```rust
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceConfigResponse {
    pub(crate) root_path: String,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    pub(crate) values: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceResponse {
    pub(crate) configured: bool,
    pub(crate) workspace: Option<WorkspaceConfigResponse>,
    /// Effective root the daemon would use without configuration
    /// (env var or launch directory). Lets onboarding pre-fill the field.
    pub(crate) default_root: String,
    /// Present only in validate-only responses: whether the validated root
    /// already exists on disk. Drives the "will be created" UI copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) root_path_exists: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceConfigRequest {
    pub(crate) root_path: String,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    #[serde(default)]
    pub(crate) values: Vec<String>,
    /// When true, validate only — do not persist or mutate daemon state.
    #[serde(default)]
    pub(crate) validate_only: bool,
}
```

Create `routes/workspace.rs`:

```rust
use std::path::{Path, PathBuf};

use super::contracts::{WorkspaceConfigRequest, WorkspaceConfigResponse, WorkspaceResponse};
use super::ApiError;
use crate::app::SharedDaemonState;
use crate::control_plane_store::WorkspaceConfig;

pub(crate) async fn handle_get_workspace(
    state: &SharedDaemonState,
) -> Result<WorkspaceResponse, ApiError> {
    let workspace = {
        let guard = state.read().await;
        guard.workspace.clone()
    };
    Ok(WorkspaceResponse {
        configured: workspace.is_some(),
        workspace: workspace.as_ref().map(config_response),
        default_root: default_root_label(),
        root_path_exists: None,
    })
}

fn config_response(config: &WorkspaceConfig) -> WorkspaceConfigResponse {
    WorkspaceConfigResponse {
        root_path: config.root_path.display().to_string(),
        company_name: config.company_name.clone(),
        mission: config.mission.clone(),
        values: config.values.clone(),
    }
}

fn default_root_label() -> String {
    match std::env::var("ANIMAOS_WORKSPACE_ROOT") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    }
}
```

Wire the route in `routes/mod.rs` next to `/api/agents`:

```rust
.route(
    "/api/workspace",
    get(get_workspace_entry).put(put_workspace_entry),
)
.route(
    "/api/workspace/bootstrap",
    axum::routing::post(bootstrap_workspace_entry),
)
```

(`put_workspace_entry` and `bootstrap_workspace_entry` arrive in Tasks 4 and 6 — add `get_workspace_entry` now, stub-free; comment out the other two route lines until then, or land Tasks 3–7 before running the full suite. Prefer landing route wiring per-task and only registering entries that exist.)

Entry wrapper (mirror `create_agent_entry` at mod.rs:835, minus the transaction mutex for a GET):

```rust
async fn get_workspace_entry(State(state): State<AppState>) -> AxumResponse {
    match workspace::handle_get_workspace(&state.daemon).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => error.into_response(),
    }
}
```

Add `mod workspace;` to `routes/mod.rs`, `mod workspace;` + re-exports to `contracts/mod.rs`, and the `#[utoipa::path]` annotation + OpenAPI registration mirroring `handle_get_agent`'s.

- [ ] **Step 4: Run test to verify it passes**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes hosts/rust-daemon/tests/workspace_api.rs
git commit -m "feat(daemon): add GET /api/workspace"
```

---

### Task 4: `PUT /api/workspace` with validate-only mode

**Files:**
- Modify: `hosts/rust-daemon/src/routes/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (entry)
- Test: `hosts/rust-daemon/tests/workspace_api.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/workspace_api.rs`:

```rust
#[tokio::test]
async fn put_workspace_rejects_relative_path() {
    let app = test_app().await;
    let (status, body) = send_json_request(&app, "PUT", "/api/workspace", Some(serde_json::json!({
        "rootPath": "relative/folder",
        "companyName": "Northwind",
        "mission": "Research"
    }))).await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap_or_default().contains("absolute"));
}

#[tokio::test]
async fn put_workspace_validate_only_does_not_persist() {
    let app = test_app().await;
    let root = std::env::temp_dir().join(format!("anima-validate-{}", std::process::id()));
    let (status, _) = send_json_request(&app, "PUT", "/api/workspace", Some(serde_json::json!({
        "rootPath": root,
        "companyName": "Northwind",
        "mission": "Research",
        "validateOnly": true
    }))).await;
    assert_eq!(status, 200);
    let (_, body) = send_json_request(&app, "GET", "/api/workspace", None).await;
    assert_eq!(body["configured"], false);
    assert!(!root.exists(), "validate-only must not create the folder");
}

#[tokio::test]
async fn put_workspace_persists_and_creates_folder() {
    let app = test_app().await;
    let root = std::env::temp_dir().join(format!("anima-persist-{}", std::process::id()));
    let (status, _) = send_json_request(&app, "PUT", "/api/workspace", Some(serde_json::json!({
        "rootPath": root,
        "companyName": "Northwind",
        "mission": "Research",
        "values": ["cite sources"]
    }))).await;
    assert_eq!(status, 200);
    assert!(root.is_dir());
    let (_, body) = send_json_request(&app, "GET", "/api/workspace", None).await;
    assert_eq!(body["configured"], true);
    assert_eq!(body["workspace"]["companyName"], "Northwind");
    std::fs::remove_dir_all(&root).ok();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: FAIL — 404/405 on PUT.

- [ ] **Step 3: Implement**

Add to `routes/workspace.rs`:

```rust
pub(crate) async fn handle_put_workspace(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<WorkspaceResponse, ApiError> {
    let request: WorkspaceConfigRequest = super::parse_json_body(body)?;
    let root_existed = Path::new(request.root_path.trim()).exists();
    let config = validate_workspace_request(&request)?;

    if request.validate_only {
        return Ok(WorkspaceResponse {
            configured: false,
            workspace: Some(config_response(&config)),
            default_root: default_root_label(),
            root_path_exists: Some(root_existed),
        });
    }

    let persist_request = {
        let mut guard = state.write().await;
        guard.workspace = Some(config);
        guard.control_plane_persist_request()
    };
    persist_request
        .save()
        .await
        .map_err(|error| ApiError::service_unavailable(error.to_string()))?;

    handle_get_workspace(state).await
}

fn validate_workspace_request(
    request: &WorkspaceConfigRequest,
) -> Result<WorkspaceConfig, ApiError> {
    let company = request.company_name.trim();
    if company.is_empty() {
        return Err(ApiError::bad_request_static("companyName is required"));
    }
    let mission = request.mission.trim();
    if mission.is_empty() {
        return Err(ApiError::bad_request_static("mission is required"));
    }
    let root = validate_root_path(&request.root_path, request.validate_only)?;
    let values = request
        .values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .take(5)
        .collect();
    Ok(WorkspaceConfig {
        root_path: root,
        company_name: company.to_string(),
        mission: mission.to_string(),
        values,
    })
}

fn validate_root_path(raw: &str, validate_only: bool) -> Result<PathBuf, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request_static("rootPath is required"));
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_absolute() {
        return Err(ApiError::bad_request_static(
            "rootPath must be an absolute path",
        ));
    }
    if candidate.exists() {
        if !candidate.is_dir() {
            return Err(ApiError::bad_request_static("rootPath is not a directory"));
        }
    } else {
        let ancestor = candidate
            .ancestors()
            .skip(1)
            .find(|path| path.exists())
            .ok_or_else(|| ApiError::bad_request_static("rootPath has no existing ancestor"))?;
        if !ancestor.is_dir() {
            return Err(ApiError::bad_request_static(
                "rootPath's nearest existing ancestor is not a directory",
            ));
        }
        if !validate_only {
            std::fs::create_dir_all(&candidate).map_err(|error| {
                ApiError::bad_request(format!("rootPath could not be created: {error}"))
            })?;
        }
    }
    if validate_only && !candidate.exists() {
        // Nothing to canonicalize yet; return as-is.
        return Ok(candidate);
    }
    candidate.canonicalize().map_err(|error| {
        ApiError::bad_request(format!("rootPath could not be resolved: {error}"))
    })
}
```

Add the `put_workspace_entry` wrapper in `routes/mod.rs` mirroring `create_agent_entry` (limited body read → `control_plane_transaction().await` guard → `handle_put_workspace` → 200 on success). Add the `#[utoipa::path]` annotation + OpenAPI registration.

Note: `ApiError::bad_request_static` exists (used in `routes/agents.rs:20`); confirm the exact constructor names before writing.

- [ ] **Step 4: Run tests to verify they pass**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes hosts/rust-daemon/tests/workspace_api.rs
git commit -m "feat(daemon): add PUT /api/workspace with validate-only mode"
```

---

## Phase 3 — Daemon: profile generation

### Task 5: `POST /api/agents/generate-profile`

**Files:**
- Create: `hosts/rust-daemon/src/routes/profile.rs` (presets + prompt builder + parser + handler)
- Modify: `hosts/rust-daemon/src/routes/contracts/agents.rs` (request/response contracts)
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (route + entry)
- Test: `hosts/rust-daemon/src/routes/profile.rs` (`#[cfg(test)]` module with a scripted `ModelAdapter` stub)

- [ ] **Step 1: Write the failing tests**

At the bottom of the new `routes/profile.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_presets_resolve() {
        for id in ["chief-of-staff", "calm-assistant", "senior-engineer", "creative-partner"] {
            assert!(profile_preset(id).is_some(), "preset {id} should exist");
        }
        assert!(profile_preset("nope").is_none());
    }

    #[test]
    fn prompt_embeds_workspace_identity_and_intent() {
        let prompt = build_profile_prompt(
            profile_preset("chief-of-staff").unwrap(),
            "watch my portfolio",
            &WorkspaceIdentity {
                company_name: "Northwind".into(),
                mission: "equity research".into(),
                values: vec!["cite sources".into()],
            },
        );
        assert!(prompt.contains("Northwind"));
        assert!(prompt.contains("equity research"));
        assert!(prompt.contains("cite sources"));
        assert!(prompt.contains("watch my portfolio"));
    }

    #[test]
    fn parses_structured_profile_from_model_output() {
        let output = r#"{"bio":"A calm operator.","adjectives":["calm","precise"],"style":"brief, numbered","system":"You are Anima..."}"#;
        let profile = parse_profile_output(output).expect("valid profile parses");
        assert_eq!(profile.bio, "A calm operator.");
        assert_eq!(profile.adjectives, vec!["calm", "precise"]);
        assert!(profile.system.contains("You are Anima"));
    }

    #[test]
    fn parse_rejects_non_json_output() {
        assert!(parse_profile_output("sorry, I cannot").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement**

Contracts in `contracts/agents.rs`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceIdentityRequest {
    pub(crate) company_name: String,
    pub(crate) mission: String,
    #[serde(default)]
    pub(crate) values: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateProfileRequest {
    pub(crate) preset_id: String,
    pub(crate) intent: String,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) workspace: WorkspaceIdentityRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentProfileResponse {
    pub(crate) bio: String,
    pub(crate) adjectives: Vec<String>,
    pub(crate) style: String,
    pub(crate) system: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentProfileEnvelope {
    pub(crate) profile: AgentProfileResponse,
}
```

`routes/profile.rs`:

```rust
use serde::{Deserialize, Serialize};

use anima_core::{Content, Message, MessageRole, ModelGenerateRequest};

use super::contracts::{AgentProfileEnvelope, AgentProfileResponse, GenerateProfileRequest};
use super::ApiError;
use crate::app::SharedDaemonState;
use crate::control_plane_store::WorkspaceConfig;

/// Stable error prefix the web client matches on to trigger the preset-template
/// fallback. Keep this exact string stable.
pub(crate) const PROFILE_GENERATION_UNAVAILABLE: &str = "PROFILE_GENERATION_UNAVAILABLE";

pub(crate) struct ProfilePreset {
    pub(crate) id: &'static str,
    pub(crate) style_guidance: &'static str,
}

pub(crate) const PROFILE_PRESETS: &[ProfilePreset] = &[
    ProfilePreset {
        id: "chief-of-staff",
        style_guidance: "Proactive, organized chief of staff. Briefs the owner first, anticipates needs, keeps crisp summaries. Never waits to be asked when something is clearly in scope.",
    },
    ProfilePreset {
        id: "calm-assistant",
        style_guidance: "Patient, thorough assistant. Asks before acting on anything ambiguous, explains reasoning, prefers correctness over speed.",
    },
    ProfilePreset {
        id: "senior-engineer",
        style_guidance: "Direct senior engineer. Code-first, minimal ceremony, flags risks plainly, no filler.",
    },
    ProfilePreset {
        id: "creative-partner",
        style_guidance: "Exploratory creative partner. Offers multiple angles, playful but grounded, generous with ideas.",
    },
];

pub(crate) fn profile_preset(id: &str) -> Option<&'static ProfilePreset> {
    PROFILE_PRESETS.iter().find(|preset| preset.id == id)
}

/// Subset of workspace identity carried in the generate-profile request body
/// (workspace config is not persisted until bootstrap).
pub(crate) struct WorkspaceIdentity {
    pub(crate) company_name: String,
    pub(crate) mission: String,
    pub(crate) values: Vec<String>,
}

pub(crate) fn build_profile_prompt(
    preset: &ProfilePreset,
    intent: &str,
    workspace: &WorkspaceIdentity,
) -> String {
    let values = if workspace.values.is_empty() {
        "none specified".to_string()
    } else {
        workspace.values.join(", ")
    };
    format!(
        "You are writing the profile for the main agent of a company.\n\
         Company: {}\nMission: {}\nValues: {}\n\
         Personality direction: {}\n\
         What the owner wants from this agent: {}\n\n\
         Respond with ONLY a JSON object with exactly these keys:\n\
         - \"bio\": one sentence of personality\n\
         - \"adjectives\": array of 3 short lowercase adjectives\n\
         - \"style\": one short line describing communication style\n\
         - \"system\": the full system prompt the agent should run with \
         (second person, starts with \"You are\", weaves in the mission and values, \
         concrete behavioral rules, 4-8 sentences)",
        workspace.company_name, workspace.mission, values, preset.style_guidance, intent
    )
}

pub(crate) fn parse_profile_output(output: &str) -> Result<AgentProfileResponse, String> {
    // Reuse the JSON-extraction approach from routes/agencies.rs if one exists
    // there (grep for `from_str` in agencies.rs); otherwise parse directly and,
    // on failure, retry with the first {...} span extracted from the text.
    let value: serde_json::Value = serde_json::from_str(output.trim())
        .or_else(|_| {
            let start = output.find('{').ok_or("no JSON object in output")?;
            let end = output.rfind('}').ok_or("no JSON object in output")?;
            serde_json::from_str(&output[start..=end])
        })
        .map_err(|error| format!("profile output was not valid JSON: {error}"))?;

    let bio = value["bio"].as_str().unwrap_or_default().trim().to_string();
    let system = value["system"].as_str().unwrap_or_default().trim().to_string();
    if bio.is_empty() || system.is_empty() {
        return Err("profile output missing bio or system".into());
    }
    let adjectives = value["adjectives"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
                .take(5)
                .collect()
        })
        .unwrap_or_default();
    let style = value["style"].as_str().unwrap_or_default().trim().to_string();
    Ok(AgentProfileResponse { bio, adjectives, style, system })
}

pub(crate) async fn handle_generate_profile(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<AgentProfileEnvelope, ApiError> {
    let request: GenerateProfileRequest = super::parse_json_body(body)?;
    let preset = profile_preset(request.preset_id.trim())
        .ok_or_else(|| ApiError::bad_request_static("unknown presetId"))?;
    let intent = request.intent.trim();
    if intent.is_empty() {
        return Err(ApiError::bad_request_static("intent is required"));
    }
    let provider = request.provider.as_deref().unwrap_or("").trim();

    let adapter = {
        let guard = state.read().await;
        // The deterministic adapter cannot generate real profiles; treat it
        // (and an explicitly unconfigured provider) as unavailable so the web
        // app falls back to preset templates.
        if provider.is_empty() || provider == "deterministic" {
            return Err(ApiError::bad_request_static(
                "PROFILE_GENERATION_UNAVAILABLE: no generative provider configured",
            ));
        }
        std::sync::Arc::clone(&guard.model_adapter)
    };

    let identity = WorkspaceIdentity {
        company_name: request.workspace.company_name.trim().to_string(),
        mission: request.workspace.mission.trim().to_string(),
        values: request.workspace.values.clone(),
    };
    let prompt = build_profile_prompt(preset, intent, &identity);
    // Adapter call pattern verified against routes/agencies.rs:272-293 —
    // generate takes (&AgentConfig, &ModelGenerateRequest).
    let generator_config = AgentConfig {
        name: "profile-generator".into(),
        model: request
            .model
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| "gpt-4o-mini".into()),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some(provider.to_string()),
        system: Some("You output only valid JSON objects.".into()),
        tools: None,
        plugins: None,
        settings: None,
    };
    let model_request = ModelGenerateRequest {
        system: "You output only valid JSON objects.".into(),
        messages: vec![Message {
            id: String::new(),
            agent_id: String::new(),
            room_id: String::new(),
            content: Content { text: prompt, attachments: None, metadata: None },
            role: MessageRole::User,
            created_at_ms: 0,
        }],
        temperature: Some(0.4),
        max_tokens: Some(1200),
    };

    let response = adapter
        .generate(&generator_config, &model_request)
        .await
        // Match the agencies.rs convention: model errors surface as bad_request.
        .map_err(|message| ApiError::bad_request(format!("profile model error: {message}")))?;
    let text = response.content.text;
    let profile = parse_profile_output(&text)
        .map_err(|error| ApiError::service_unavailable(format!("profile generation produced unusable output: {error}")))?;
    Ok(AgentProfileEnvelope { profile })
}
```

The imports at the top of `routes/profile.rs` need `AgentConfig` added: `use anima_core::{AgentConfig, Content, Message, MessageRole, ModelGenerateRequest};`. JSON extraction may reuse the agency parser's approach (grep `parse_agents_payload` in `routes/agencies.rs`); the inline fallback in `parse_profile_output` above is sufficient if that proves awkward to share.

Entry in `routes/mod.rs` (mirror `generate_agency_entry`: limited body → transaction guard not needed for a read-only model call, but keep the pattern if neighboring entries take it for provider access — follow the local convention):

```rust
.route(
    "/api/agents/generate-profile",
    axum::routing::post(generate_profile_entry),
)
```

```rust
async fn generate_profile_entry(State(state): State<AppState>, request: AxumRequest) -> AxumResponse {
    match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => match profile::handle_generate_profile(body, &state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Err(response) => response,
    }
}
```

Add `mod profile;`, the `#[utoipa::path]` annotation, and OpenAPI registration.

For the adapter-level test, follow the pattern agencies tests use for model stubbing (grep `ModelAdapter` in `hosts/rust-daemon/src/routes/agencies.rs` tests or `hosts/rust-daemon/tests/`); if none exists, define a minimal stub implementing `ModelAdapter` returning a fixed JSON string and drive `handle_generate_profile` directly against a `DaemonState` built with `DaemonState::new()` and the stub installed in `model_adapter`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes
git commit -m "feat(daemon): add POST /api/agents/generate-profile"
```

---

## Phase 4 — Daemon: atomic bootstrap

### Task 6: `POST /api/workspace/bootstrap`

**Files:**
- Modify: `hosts/rust-daemon/src/routes/agencies.rs:35-77` (make `AgencyYamlConfig` / `AgencyYamlAgent` `pub(crate)`)
- Modify: `hosts/rust-daemon/src/routes/contracts/workspace.rs` (bootstrap contracts)
- Modify: `hosts/rust-daemon/src/routes/workspace.rs` (handler)
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (entry)
- Test: `hosts/rust-daemon/tests/workspace_api.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/workspace_api.rs`:

```rust
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
    let app = test_app().await;
    let root = std::env::temp_dir().join(format!("anima-boot-{}", std::process::id()));
    let (status, body) = send_json_request(&app, "POST", "/api/workspace/bootstrap", Some(bootstrap_body(&root))).await;
    assert_eq!(status, 201, "body: {body}");
    assert_eq!(body["workspace"]["companyName"], "Northwind Research");
    assert_eq!(body["agent"]["state"]["config"]["bio"], "A vigilant chief of staff.");
    assert_eq!(body["agent"]["state"]["config"]["adjectives"], serde_json::json!(["vigilant", "concise", "proactive"]));

    // anima.yaml exists at the root and describes a single orchestrator.
    let yaml_path = root.join("anima.yaml");
    assert!(yaml_path.is_file());
    let yaml: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(&yaml_path).unwrap()).unwrap();
    assert_eq!(yaml["name"], "Northwind Research");
    assert_eq!(yaml["orchestrator"]["name"], "Anima");
    assert_eq!(yaml["strategy"], "supervisor");

    // Workspace config is live.
    let (_, workspace) = send_json_request(&app, "GET", "/api/workspace", None).await;
    assert_eq!(workspace["configured"], true);

    // The agent's tools were resolved to canonical descriptors.
    let tools = body["agent"]["state"]["config"]["tools"].as_array().unwrap();
    let read_file = tools.iter().find(|t| t["name"] == "read_file").unwrap();
    assert!(read_file["description"].as_str().unwrap().len() > 0);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn bootstrap_rejects_unknown_tools_without_side_effects() {
    let app = test_app().await;
    let root = std::env::temp_dir().join(format!("anima-boot-bad-{}", std::process::id()));
    let mut body = bootstrap_body(&root);
    body["agent"]["tools"] = serde_json::json!(["definitely_not_a_tool"]);
    let (status, _) = send_json_request(&app, "POST", "/api/workspace/bootstrap", Some(body)).await;
    assert_eq!(status, 400);
    assert!(!root.join("anima.yaml").exists(), "failed bootstrap must not write anima.yaml");
    let (_, workspace) = send_json_request(&app, "GET", "/api/workspace", None).await;
    assert_eq!(workspace["configured"], false);
    let (_, agents) = send_json_request(&app, "GET", "/api/agents", None).await;
    assert_eq!(agents["agents"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn bootstrap_rejects_empty_system() {
    let app = test_app().await;
    let root = std::env::temp_dir().join(format!("anima-boot-sys-{}", std::process::id()));
    let mut body = bootstrap_body(&root);
    body["agent"]["system"] = serde_json::json!("  ");
    let (status, _) = send_json_request(&app, "POST", "/api/workspace/bootstrap", Some(body)).await;
    assert_eq!(status, 400);
}
```

(Add `serde_yaml` to `hosts/rust-daemon` dev-dependencies if not already a dependency — agencies.rs serializes YAML, so check `Cargo.toml` first and reuse whatever YAML crate is already there.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: FAIL — 404.

- [ ] **Step 3: Implement**

Contracts in `contracts/workspace.rs`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootstrapAgentRequest {
    pub(crate) name: String,
    pub(crate) preset_id: String,
    pub(crate) bio: Option<String>,
    pub(crate) adjectives: Option<Vec<String>>,
    pub(crate) style: Option<String>,
    pub(crate) system: String,
    pub(crate) provider: Option<String>,
    pub(crate) model: String,
    pub(crate) tools: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceBootstrapRequest {
    pub(crate) workspace: WorkspaceConfigRequest,
    pub(crate) agent: BootstrapAgentRequest,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceBootstrapResponse {
    pub(crate) workspace: WorkspaceConfigResponse,
    pub(crate) agent: super::AgentRuntimeSnapshotResponse,
}
```

In `routes/agencies.rs`, change `struct AgencyYamlConfig` and `struct AgencyYamlAgent` to `pub(crate) struct` (fields stay private; add a `pub(crate) fn single_orchestrator(...)` constructor there so the yaml shape stays owned by the agencies module):

```rust
impl AgencyYamlConfig {
    pub(crate) fn single_orchestrator(
        name: String,
        mission: String,
        values: Vec<String>,
        provider: String,
        model: String,
        orchestrator: AgencyYamlAgent,
    ) -> Self {
        Self {
            name,
            description: mission.clone(),
            mission: Some(mission),
            values: if values.is_empty() { None } else { Some(values) },
            model,
            provider,
            strategy: "supervisor".into(),
            max_parallel_delegations: None,
            orchestrator,
            agents: Vec::new(),
        }
    }
}

impl AgencyYamlAgent {
    pub(crate) fn orchestrator(
        name: String,
        bio: String,
        style: Option<String>,
        system: String,
        model: Option<String>,
        tools: Option<Vec<String>>,
        adjectives: Option<Vec<String>>,
    ) -> Self {
        Self {
            name,
            position: Some("orchestrator".into()),
            bio,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives,
            style,
            system,
            model,
            tools,
            collaborates_with: None,
        }
    }
}
```

(Match the actual field names/types at agencies.rs:35-77 — adjust the constructor bodies to the real struct definitions.)

Handler in `routes/workspace.rs`:

```rust
use anima_core::{AgentConfig, ToolDescriptor};

use super::contracts::{
    AgentRuntimeSnapshotResponse, WorkspaceBootstrapRequest, WorkspaceBootstrapResponse,
    WorkspaceConfigResponse,
};
use super::profile::profile_preset;
use super::ApiError;
use crate::app::SharedDaemonState;
use crate::routes::agencies::{AgencyYamlAgent, AgencyYamlConfig};

pub(crate) async fn handle_bootstrap_workspace(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<WorkspaceBootstrapResponse, ApiError> {
    let request: WorkspaceBootstrapRequest = super::parse_json_body(body)?;

    // 1. Validate everything before any side effect.
    let mut workspace_request = request.workspace;
    workspace_request.validate_only = false;
    let workspace_config = validate_workspace_request(&workspace_request)?;
    profile_preset(request.agent.preset_id.trim())
        .ok_or_else(|| ApiError::bad_request_static("unknown presetId"))?;
    let agent_name = request.agent.name.trim().to_string();
    if agent_name.is_empty() {
        return Err(ApiError::bad_request_static("agent name is required"));
    }
    let system = request.agent.system.trim().to_string();
    if system.is_empty() {
        return Err(ApiError::bad_request_static("agent system is required"));
    }
    let model = request.agent.model.trim().to_string();
    if model.is_empty() {
        return Err(ApiError::bad_request_static("agent model is required"));
    }
    if request.agent.tools.is_empty() {
        return Err(ApiError::bad_request_static("agent tools are required"));
    }

    // 2. Build the runtime agent config. create_agent resolves the name-only
    //    tool slugs to canonical descriptors and rejects unknown slugs BEFORE
    //    mutating anything — so tool validation failures leave no side effects.
    let agent_config = AgentConfig {
        name: agent_name.clone(),
        model: model.clone(),
        bio: request.agent.bio.filter(|b| !b.trim().is_empty()),
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: request.agent.adjectives,
        style: request.agent.style.filter(|s| !s.trim().is_empty()),
        provider: request.agent.provider.filter(|p| !p.trim().is_empty()),
        system: Some(system.clone()),
        tools: Some(
            request
                .agent
                .tools
                .iter()
                .map(|name| ToolDescriptor {
                    name: name.clone(),
                    description: String::new(),
                    parameters: Default::default(),
                    examples: None,
                })
                .collect(),
        ),
        plugins: None,
        settings: None,
    };

    // 3. Create the agent + set workspace under one state write; persist once.
    //    If create_agent fails (e.g. unknown tool), nothing was persisted and
    //    no file was written.
    let (snapshot, persist_request) = {
        let mut guard = state.write().await;
        let snapshot = guard
            .create_agent(agent_config)
            .map_err(ApiError::bad_request)?;
        guard.workspace = Some(workspace_config.clone());
        (snapshot, guard.control_plane_persist_request())
    };

    // 4. Write anima.yaml only after the agent exists; on IO failure, roll
    //    back the in-memory agent + workspace before returning the error.
    let agency = AgencyYamlConfig::single_orchestrator(
        workspace_config.company_name.clone(),
        workspace_config.mission.clone(),
        workspace_config.values.clone(),
        request.agent.provider.clone().unwrap_or_default(),
        model,
        AgencyYamlAgent::orchestrator(
            agent_name,
            request.agent.bio.clone().unwrap_or_default(),
            request.agent.style.clone(),
            system,
            Some(snapshot.state.config.model.clone()),
            Some(request.agent.tools.clone()),
            request.agent.adjectives.clone(),
        ),
    );
    let yaml_path = workspace_config.root_path.join("anima.yaml");
    let yaml_body = serde_yaml::to_string(&agency)
        .map_err(|error| ApiError::bad_request(format!("anima.yaml could not be serialized: {error}")))?;
    if let Err(error) = std::fs::write(&yaml_path, yaml_body) {
        let rollback_persist = {
            let mut guard = state.write().await;
            guard.remove_agent(&snapshot.state.id);
            guard.workspace = None;
            guard.control_plane_persist_request()
        };
        rollback_persist.save().await.ok();
        return Err(ApiError::service_unavailable(format!(
            "anima.yaml could not be written: {error}"
        )));
    }

    persist_request
        .save()
        .await
        .map_err(|error| ApiError::service_unavailable(error.to_string()))?;

    Ok(WorkspaceBootstrapResponse {
        workspace: config_response(&workspace_config),
        agent: AgentRuntimeSnapshotResponse::from(&snapshot),
    })
}
```

Also confirm `DaemonState::remove_agent` is callable here (state.rs — used by `handle_delete_agent`; it is) and that `AgentConfig` fields match the real struct in `anima-core` (grep `pub struct AgentConfig` in `packages/core-rust/crates/anima-core`).

Entry in `routes/mod.rs` — mirror `create_agent_entry` exactly (limited body → `control_plane_transaction().await` → handler → 201 CREATED). Add `#[utoipa::path]` + OpenAPI registration.

- [ ] **Step 4: Run tests to verify they pass**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes hosts/rust-daemon/tests/workspace_api.rs hosts/rust-daemon/Cargo.toml Cargo.lock
git commit -m "feat(daemon): add atomic POST /api/workspace/bootstrap"
```

---

### Task 7: Rust verification checkpoint

- [ ] **Step 1: Full suite**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: PASS — entire suite including all new workspace tests.

- [ ] **Step 2: Lint**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:lint --skipNxCache`
Expected: PASS with no new warnings.

- [ ] **Step 3: Commit (only if lint fixes were needed)**

```bash
git add -A
git commit -m "chore(daemon): lint fixes for workspace endpoints"
```

---

## Phase 5 — Web: client and presets

### Task 8: Daemon client workspace/profile/bootstrap methods

**Files:**
- Modify: `apps/web/src/lib/daemon-api.ts`
- Test: `apps/web/src/lib/daemon-api.test.ts`

- [ ] **Step 1: Write the failing tests**

Append to `apps/web/src/lib/daemon-api.test.ts` (follow the file's existing fetch-mock pattern):

```ts
it('getWorkspace fetches /workspace', async () => {
  fetchMock.mockResolvedValueOnce(jsonResponse({
    configured: false,
    workspace: null,
    defaultRoot: 'C:\\anima',
  }));
  const result = await daemon.getWorkspace();
  expect(fetchMock).toHaveBeenCalledWith('/api/workspace', expect.anything());
  expect(result.configured).toBe(false);
  expect(result.defaultRoot).toBe('C:\\anima');
});

it('validateWorkspace PUTs with validateOnly', async () => {
  fetchMock.mockResolvedValueOnce(jsonResponse({ configured: false, workspace: null, defaultRoot: '' }));
  await daemon.validateWorkspace({
    rootPath: 'C:\\workspaces\\northwind',
    companyName: 'Northwind',
    mission: 'Research',
    values: ['cite sources'],
  });
  const [, init] = fetchMock.mock.calls.at(-1)!;
  expect(init?.method).toBe('PUT');
  expect(JSON.parse(String(init?.body))).toMatchObject({ validateOnly: true });
});

it('generateProfile POSTs preset, intent, model, and workspace identity', async () => {
  fetchMock.mockResolvedValueOnce(jsonResponse({
    profile: { bio: 'B', adjectives: ['a'], style: 's', system: 'S' },
  }));
  const result = await daemon.generateProfile({
    presetId: 'chief-of-staff',
    intent: 'watch my portfolio',
    provider: 'openai',
    model: 'gpt-5',
    workspace: { companyName: 'Northwind', mission: 'Research', values: [] },
  });
  expect(result.profile.system).toBe('S');
  const [, init] = fetchMock.mock.calls.at(-1)!;
  expect(JSON.parse(String(init?.body)).workspace.companyName).toBe('Northwind');
});

it('bootstrapWorkspace POSTs workspace and agent payloads', async () => {
  fetchMock.mockResolvedValueOnce(jsonResponse({ workspace: {}, agent: snapshotStub }));
  await daemon.bootstrapWorkspace({
    workspace: { rootPath: 'C:\\x', companyName: 'N', mission: 'M', values: [] },
    agent: {
      name: 'Anima', presetId: 'chief-of-staff', bio: 'b', adjectives: ['a'],
      style: 's', system: 'S', provider: 'openai', model: 'gpt-5',
      tools: ['read_file'],
    },
  });
  const [path, init] = fetchMock.mock.calls.at(-1)!;
  expect(path).toBe('/api/workspace/bootstrap');
  expect(init?.method).toBe('POST');
});
```

(Adapt `jsonResponse`, `fetchMock`, and `snapshotStub` names to what the test file already defines.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=daemon-api.test.ts`
Expected: FAIL — methods do not exist.

- [ ] **Step 3: Implement**

Add to `daemon-api.ts`:

```ts
export interface DaemonWorkspaceConfig {
  rootPath: string;
  companyName: string;
  mission: string;
  values: string[];
}

export interface DaemonWorkspaceState {
  configured: boolean;
  workspace: DaemonWorkspaceConfig | null;
  defaultRoot: string;
  /** Present only on validate-only responses: does the folder already exist? */
  rootPathExists?: boolean;
}

export interface WorkspaceConfigInput {
  rootPath: string;
  companyName: string;
  mission: string;
  values: string[];
}

export interface GenerateProfileInput {
  presetId: string;
  intent: string;
  provider: string;
  model: string;
  workspace: { companyName: string; mission: string; values: string[] };
}

export interface AgentProfile {
  bio: string;
  adjectives: string[];
  style: string;
  system: string;
}

export interface BootstrapWorkspaceInput {
  workspace: WorkspaceConfigInput;
  agent: {
    name: string;
    presetId: string;
    bio?: string;
    adjectives?: string[];
    style?: string;
    system: string;
    provider?: string;
    model: string;
    tools: string[];
  };
}

/** Error.message prefix returned by the daemon when no generative provider is available. */
export const PROFILE_GENERATION_UNAVAILABLE = 'PROFILE_GENERATION_UNAVAILABLE';
```

Extend `DaemonSnapshot.state.config` with the personality fields the daemon now returns (they already exist in `AgentConfigResponse`):

```ts
      bio?: string | null;
      adjectives?: string[] | null;
      style?: string | null;
```

Add to the `daemon` object:

```ts
  getWorkspace: () => request<DaemonWorkspaceState>('/workspace'),

  putWorkspace: (input: WorkspaceConfigInput) =>
    request<DaemonWorkspaceState>('/workspace', {
      method: 'PUT',
      body: JSON.stringify(input),
    }),

  validateWorkspace: (input: WorkspaceConfigInput) =>
    request<DaemonWorkspaceState>('/workspace', {
      method: 'PUT',
      body: JSON.stringify({ ...input, validateOnly: true }),
    }),

  generateProfile: (input: GenerateProfileInput) =>
    request<{ profile: AgentProfile }>('/agents/generate-profile', {
      method: 'POST',
      body: JSON.stringify(input),
    }),

  bootstrapWorkspace: (input: BootstrapWorkspaceInput) =>
    request<{ workspace: DaemonWorkspaceConfig; agent: DaemonSnapshot }>(
      '/workspace/bootstrap',
      { method: 'POST', body: JSON.stringify(input) },
    ),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=daemon-api.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/lib/daemon-api.ts apps/web/src/lib/daemon-api.test.ts
git commit -m "feat(web): add workspace, profile, and bootstrap daemon client methods"
```

---

### Task 9: Personality presets library

**Files:**
- Create: `apps/web/src/lib/agent-presets.ts`
- Test: `apps/web/src/lib/agent-presets.test.ts`

- [ ] **Step 1: Write the failing tests**

```ts
import { AGENT_PRESETS, presetById, presetTemplate } from './agent-presets';

describe('agent presets', () => {
  it('ships exactly the four daemon-known presets', () => {
    expect(AGENT_PRESETS.map((preset) => preset.id)).toEqual([
      'chief-of-staff',
      'calm-assistant',
      'senior-engineer',
      'creative-partner',
    ]);
  });

  it('every preset has a label, tagline, and icon', () => {
    for (const preset of AGENT_PRESETS) {
      expect(preset.label.length).toBeGreaterThan(0);
      expect(preset.tagline.length).toBeGreaterThan(0);
      expect(preset.icon.length).toBeGreaterThan(0);
    }
  });

  it('template embeds workspace company and mission', () => {
    const profile = presetTemplate('chief-of-staff', {
      companyName: 'Northwind Research',
      mission: 'Continuous equity research',
      agentName: 'Anima',
    });
    expect(profile.system).toContain('Northwind Research');
    expect(profile.system).toContain('Continuous equity research');
    expect(profile.system).toContain('Anima');
    expect(profile.bio.length).toBeGreaterThan(0);
    expect(profile.adjectives.length).toBe(3);
  });

  it('unknown preset id returns undefined', () => {
    expect(presetById('nope')).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=agent-presets.test.ts`
Expected: FAIL — module missing.

- [ ] **Step 3: Implement**

```ts
// Personality presets for the Agent onboarding step. Preset ids MUST match the
// daemon's PROFILE_PRESETS (hosts/rust-daemon/src/routes/profile.rs) — the id
// is the wire key for POST /api/agents/generate-profile. The templates are the
// offline fallback when no generative provider is configured.

export type PresetId =
  | 'chief-of-staff'
  | 'calm-assistant'
  | 'senior-engineer'
  | 'creative-partner';

export interface AgentPreset {
  id: PresetId;
  label: string;
  tagline: string;
  icon: string;
}

export interface PresetTemplateContext {
  companyName: string;
  mission: string;
  agentName: string;
}

export interface PresetProfile {
  bio: string;
  adjectives: string[];
  style: string;
  system: string;
}

export const AGENT_PRESETS: AgentPreset[] = [
  { id: 'chief-of-staff', label: 'Chief of Staff', tagline: 'Proactive, organized, briefs you first', icon: '🧭' },
  { id: 'calm-assistant', label: 'Calm Assistant', tagline: 'Patient, thorough, asks before acting', icon: '☕' },
  { id: 'senior-engineer', label: 'Senior Engineer', tagline: 'Direct, code-first, minimal ceremony', icon: '🔧' },
  { id: 'creative-partner', label: 'Creative Partner', tagline: 'Exploratory, playful, idea-rich', icon: '🎨' },
];

export function presetById(id: string): AgentPreset | undefined {
  return AGENT_PRESETS.find((preset) => preset.id === id);
}

export function presetTemplate(id: PresetId, context: PresetTemplateContext): PresetProfile {
  const { companyName, mission, agentName } = context;
  switch (id) {
    case 'chief-of-staff':
      return {
        bio: `A vigilant chief of staff at ${companyName} who turns noise into calm, actionable briefs.`,
        adjectives: ['vigilant', 'concise', 'proactive'],
        style: 'Brief, structured, leads with the most important thing.',
        system: [
          `You are ${agentName}, the chief of staff at ${companyName}.`,
          `The company mission: ${mission}.`,
          'Brief the owner proactively: lead with what matters, then context, then recommended action.',
          'When you notice something unusual inside your access level, investigate first, then report with evidence.',
          'Keep replies short unless the owner asks for depth. Never invent figures or sources.',
        ].join('\n'),
      };
    case 'calm-assistant':
      return {
        bio: `A patient assistant at ${companyName} who explains reasoning and never rushes.`,
        adjectives: ['patient', 'thorough', 'careful'],
        style: 'Warm, unhurried, explains before acting.',
        system: [
          `You are ${agentName}, a calm assistant at ${companyName}.`,
          `The company mission: ${mission}.`,
          'Ask before acting on anything ambiguous. Explain your reasoning in plain language.',
          'Prefer correctness over speed; double-check facts before presenting them.',
        ].join('\n'),
      };
    case 'senior-engineer':
      return {
        bio: `A direct senior engineer at ${companyName} who ships and flags risks plainly.`,
        adjectives: ['direct', 'precise', 'pragmatic'],
        style: 'Terse, code-first, no filler.',
        system: [
          `You are ${agentName}, a senior engineer at ${companyName}.`,
          `The company mission: ${mission}.`,
          'Go code-first: show the change, then one line of rationale. Flag risks plainly.',
          'No ceremony, no filler. If something is a bad idea, say so and say why.',
        ].join('\n'),
      };
    case 'creative-partner':
      return {
        bio: `An exploratory creative partner at ${companyName} who brings angles you did not ask for.`,
        adjectives: ['curious', 'playful', 'grounded'],
        style: 'Generous with ideas, always tied back to the goal.',
        system: [
          `You are ${agentName}, a creative partner at ${companyName}.`,
          `The company mission: ${mission}.`,
          'Offer multiple angles before converging. Stay playful but grounded in the mission.',
          'Every idea ends with a concrete next step.',
        ].join('\n'),
      };
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=agent-presets.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/lib/agent-presets.ts apps/web/src/lib/agent-presets.test.ts
git commit -m "feat(web): add personality preset library with offline templates"
```

---

## Phase 6 — Web: onboarding UI

### Task 10: Five-step progress indicator

**Files:**
- Modify: `apps/web/src/components/onboarding/OnboardingProgress.tsx`
- Modify: `apps/web/src/components/onboarding/OnboardingFlow.test.tsx` (step-label assertions land in Task 13; here only the component change)

- [ ] **Step 1: Update the steps**

```ts
export const ONBOARDING_STEPS = [
  'Workspace',
  'Intelligence',
  'Agent',
  'Access',
  'Review',
] as const;
```

Change the grid class from `grid-cols-4` to `grid-cols-5`.

- [ ] **Step 2: Verify existing suite impact**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=OnboardingFlow.test.tsx`
Expected: FAIL — flow tests reference the old step order. That is expected; Task 13 rewrites them. Do not commit yet; fold this change into the Task 13 commit if you prefer green-at-every-commit. (Recommended: keep going, commit with Task 13.)

---

### Task 11: `WorkspaceStep` component

**Files:**
- Create: `apps/web/src/components/onboarding/WorkspaceStep.tsx`
- Test: `apps/web/src/components/onboarding/WorkspaceStep.test.tsx`

- [ ] **Step 1: Write the failing tests**

```tsx
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { WorkspaceStep } from './WorkspaceStep';

function setup(overrides: Partial<Parameters<typeof WorkspaceStep>[0]> = {}) {
  const props = {
    companyName: '',
    mission: '',
    rootPath: 'C:\\anima',
    values: [] as string[],
    verifying: false,
    verifyStatus: null,
    onCompanyNameChange: vi.fn(),
    onMissionChange: vi.fn(),
    onRootPathChange: vi.fn(),
    onValuesChange: vi.fn(),
    onVerify: vi.fn(),
    companyInputRef: { current: null },
    ...overrides,
  };
  render(<WorkspaceStep {...props} />);
  return props;
}

describe('WorkspaceStep', () => {
  it('renders company, mission, folder, and values fields', () => {
    setup();
    expect(screen.getByLabelText(/company name/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/mission/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/office location/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/values/i)).toBeInTheDocument();
  });

  it('emits edits', async () => {
    const props = setup();
    await userEvent.type(screen.getByLabelText(/company name/i), 'N');
    expect(props.onCompanyNameChange).toHaveBeenCalled();
  });

  it('shows a verifying state and verify result', () => {
    setup({ verifying: true });
    expect(screen.getByRole('button', { name: /verifying/i })).toBeDisabled();
  });

  it('shows create-vs-existing result copy', () => {
    setup({ verifyStatus: { ok: true, willCreate: true } });
    expect(screen.getByText(/will be created/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=WorkspaceStep.test.tsx`
Expected: FAIL — component missing.

- [ ] **Step 3: Implement**

Controlled component; mirror `IdentityStep.tsx` styling (`labelCls`, `field` classes from `../ui-bits`):

```tsx
import type { RefObject } from 'react';

import { labelCls } from '../ui-bits';

export interface WorkspaceVerifyStatus {
  ok: boolean;
  willCreate?: boolean;
  message?: string;
}

export interface WorkspaceStepProps {
  companyName: string;
  mission: string;
  rootPath: string;
  values: string[];
  verifying: boolean;
  verifyStatus: WorkspaceVerifyStatus | null;
  onCompanyNameChange(value: string): void;
  onMissionChange(value: string): void;
  onRootPathChange(value: string): void;
  onValuesChange(values: string[]): void;
  onVerify(): void;
  companyInputRef: RefObject<HTMLInputElement | null>;
  validationErrorId?: string;
}

export function WorkspaceStep({
  companyName,
  mission,
  rootPath,
  values,
  verifying,
  verifyStatus,
  onCompanyNameChange,
  onMissionChange,
  onRootPathChange,
  onValuesChange,
  onVerify,
  companyInputRef,
  validationErrorId,
}: WorkspaceStepProps) {
  return (
    <section aria-labelledby="onboarding-workspace-heading" className="space-y-5">
      <div>
        <h2 id="onboarding-workspace-heading" className="font-display text-2xl font-semibold tracking-tight text-ink">
          Workspace
        </h2>
        <p className="mt-1 max-w-xl text-sm leading-relaxed text-ink-2">
          Name your company and pick the folder your agents will work in.
        </p>
      </div>

      <div>
        <label htmlFor="onboarding-company-name" className={labelCls}>Company name</label>
        <input
          ref={companyInputRef}
          id="onboarding-company-name"
          className="field"
          value={companyName}
          onChange={(event) => onCompanyNameChange(event.target.value)}
          autoComplete="off"
          aria-invalid={Boolean(validationErrorId)}
          aria-describedby={validationErrorId}
        />
      </div>

      <div>
        <label htmlFor="onboarding-mission" className={labelCls}>Mission (one sentence)</label>
        <input
          id="onboarding-mission"
          className="field"
          value={mission}
          onChange={(event) => onMissionChange(event.target.value)}
          autoComplete="off"
          placeholder="What is this company for?"
        />
      </div>

      <div>
        <label htmlFor="onboarding-root-path" className={labelCls}>Office location</label>
        <div className="flex gap-2">
          <input
            id="onboarding-root-path"
            className="field flex-1"
            value={rootPath}
            onChange={(event) => onRootPathChange(event.target.value)}
            autoComplete="off"
            spellCheck={false}
          />
          <button
            type="button"
            onClick={onVerify}
            disabled={verifying}
            className="rounded-xl border border-line bg-white/[0.02] px-4 py-2 text-sm font-medium text-ink-2 transition hover:border-line-strong hover:text-ink disabled:opacity-50"
          >
            {verifying ? 'Verifying…' : 'Verify'}
          </button>
        </div>
        {verifyStatus?.ok ? (
          <p className="mt-2 text-sm text-mint">
            ✓ {verifyStatus.willCreate ? 'Folder will be created' : 'Folder exists'} — the daemon will use this as the workspace root.
          </p>
        ) : null}
        {verifyStatus && !verifyStatus.ok ? (
          <p role="alert" className="mt-2 text-sm text-danger">{verifyStatus.message}</p>
        ) : null}
      </div>

      <div>
        <label htmlFor="onboarding-values" className={labelCls}>Values (optional, up to 5, comma-separated)</label>
        <input
          id="onboarding-values"
          className="field"
          value={values.join(', ')}
          onChange={(event) =>
            onValuesChange(
              event.target.value.split(',').map((value) => value.trim()).filter(Boolean).slice(0, 5),
            )
          }
          autoComplete="off"
          placeholder="cite sources, never invent numbers"
        />
      </div>
    </section>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=WorkspaceStep.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/components/onboarding/WorkspaceStep.tsx apps/web/src/components/onboarding/WorkspaceStep.test.tsx
git commit -m "feat(web): add onboarding workspace step"
```

---

### Task 12: `AgentStep` component (presets + intent + generate)

**Files:**
- Create: `apps/web/src/components/onboarding/AgentStep.tsx`
- Test: `apps/web/src/components/onboarding/AgentStep.test.tsx`

- [ ] **Step 1: Write the failing tests**

```tsx
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { AgentStep } from './AgentStep';

function setup(overrides: Partial<Parameters<typeof AgentStep>[0]> = {}) {
  const props = {
    name: 'Anima',
    presetId: 'chief-of-staff' as const,
    intent: '',
    bio: '',
    adjectives: [] as string[],
    style: '',
    system: '',
    generating: false,
    generateAvailable: true,
    generateError: null,
    onNameChange: vi.fn(),
    onPresetChange: vi.fn(),
    onIntentChange: vi.fn(),
    onBioChange: vi.fn(),
    onStyleChange: vi.fn(),
    onSystemChange: vi.fn(),
    onGenerate: vi.fn(),
    nameInputRef: { current: null },
    ...overrides,
  };
  render(<AgentStep {...props} />);
  return props;
}

describe('AgentStep', () => {
  it('renders the four preset cards', () => {
    setup();
    expect(screen.getByText('Chief of Staff')).toBeInTheDocument();
    expect(screen.getByText('Calm Assistant')).toBeInTheDocument();
    expect(screen.getByText('Senior Engineer')).toBeInTheDocument();
    expect(screen.getByText('Creative Partner')).toBeInTheDocument();
  });

  it('marks the selected preset', () => {
    setup({ presetId: 'senior-engineer' });
    expect(screen.getByRole('radio', { name: /senior engineer/i })).toHaveAttribute('aria-checked', 'true');
  });

  it('selecting a preset emits onPresetChange', async () => {
    const props = setup();
    await userEvent.click(screen.getByRole('radio', { name: /creative partner/i }));
    expect(props.onPresetChange).toHaveBeenCalledWith('creative-partner');
  });

  it('generate is disabled without intent', () => {
    setup({ intent: '' });
    expect(screen.getByRole('button', { name: /generate profile/i })).toBeDisabled();
  });

  it('generate click emits onGenerate when intent exists', async () => {
    const props = setup({ intent: 'watch my portfolio' });
    await userEvent.click(screen.getByRole('button', { name: /generate profile/i }));
    expect(props.onGenerate).toHaveBeenCalled();
  });

  it('shows fallback notice when generation is unavailable', () => {
    setup({ generateAvailable: false });
    expect(screen.getByText(/template/i)).toBeInTheDocument();
  });

  it('profile fields stay editable after generation', async () => {
    const props = setup({ bio: 'A bio', system: 'A system' });
    await userEvent.type(screen.getByLabelText(/^bio/i), '!');
    expect(props.onBioChange).toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText(/instructions/i), '!');
    expect(props.onSystemChange).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=AgentStep.test.tsx`
Expected: FAIL — component missing.

- [ ] **Step 3: Implement**

```tsx
import type { RefObject } from 'react';

import { AGENT_PRESETS, type PresetId } from '../../lib/agent-presets';
import { labelCls } from '../ui-bits';

export interface AgentStepProps {
  name: string;
  presetId: PresetId;
  intent: string;
  bio: string;
  adjectives: string[];
  style: string;
  system: string;
  generating: boolean;
  /** false when the daemon reported PROFILE_GENERATION_UNAVAILABLE */
  generateAvailable: boolean;
  generateError: string | null;
  onNameChange(value: string): void;
  onPresetChange(value: PresetId): void;
  onIntentChange(value: string): void;
  onBioChange(value: string): void;
  onStyleChange(value: string): void;
  onSystemChange(value: string): void;
  onGenerate(): void;
  nameInputRef: RefObject<HTMLInputElement | null>;
  validationErrorId?: string;
}

export function AgentStep({
  name,
  presetId,
  intent,
  bio,
  adjectives,
  style,
  system,
  generating,
  generateAvailable,
  generateError,
  onNameChange,
  onPresetChange,
  onIntentChange,
  onBioChange,
  onStyleChange,
  onSystemChange,
  onGenerate,
  nameInputRef,
  validationErrorId,
}: AgentStepProps) {
  return (
    <section aria-labelledby="onboarding-agent-heading" className="space-y-5">
      <div>
        <h2 id="onboarding-agent-heading" className="font-display text-2xl font-semibold tracking-tight text-ink">
          Agent
        </h2>
        <p className="mt-1 max-w-xl text-sm leading-relaxed text-ink-2">
          Pick a personality, describe what you want in plain words, and let the model write the proper profile.
        </p>
      </div>

      <div>
        <label htmlFor="onboarding-agent-name" className={labelCls}>Name</label>
        <input
          ref={nameInputRef}
          id="onboarding-agent-name"
          className="field"
          value={name}
          onChange={(event) => onNameChange(event.target.value)}
          autoComplete="off"
          aria-invalid={Boolean(validationErrorId)}
          aria-describedby={validationErrorId}
        />
      </div>

      <div role="radiogroup" aria-label="Personality preset" className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        {AGENT_PRESETS.map((preset) => {
          const selected = preset.id === presetId;
          return (
            <button
              key={preset.id}
              type="button"
              role="radio"
              aria-checked={selected}
              onClick={() => onPresetChange(preset.id)}
              className={`rounded-2xl border p-4 text-left transition ${
                selected
                  ? 'border-accent/60 bg-accent/[0.08]'
                  : 'border-line bg-white/[0.02] hover:border-line-strong'
              }`}
            >
              <span className="text-sm font-semibold text-ink">
                {preset.icon} {preset.label}
              </span>
              <span className="mt-1 block text-xs leading-relaxed text-ink-2">{preset.tagline}</span>
            </button>
          );
        })}
      </div>

      <div>
        <label htmlFor="onboarding-intent" className={labelCls}>
          What do you want {name.trim() || 'your agent'} to do for you?
        </label>
        <textarea
          id="onboarding-intent"
          className="field min-h-20 resize-y"
          value={intent}
          onChange={(event) => onIntentChange(event.target.value)}
          placeholder="Plain words are fine — rough is fine."
        />
        <div className="mt-2 flex items-center gap-3">
          <button
            type="button"
            onClick={onGenerate}
            disabled={generating || !intent.trim() || !generateAvailable}
            className="rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-abyss transition hover:bg-accent/90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {generating ? 'Generating…' : system ? '↻ Regenerate profile' : '✨ Generate profile'}
          </button>
          {!generateAvailable ? (
            <span className="text-xs text-ink-3">
              No generative provider configured — the preset template is filled in below; edit freely.
            </span>
          ) : null}
        </div>
        {generateError ? (
          <p role="alert" className="mt-2 text-sm text-danger">{generateError}</p>
        ) : null}
      </div>

      <div>
        <label htmlFor="onboarding-bio" className={labelCls}>Bio</label>
        <input
          id="onboarding-bio"
          className="field"
          value={bio}
          onChange={(event) => onBioChange(event.target.value)}
          autoComplete="off"
        />
      </div>

      {adjectives.length > 0 ? (
        <div aria-label="Traits" className="flex flex-wrap gap-1.5">
          {adjectives.map((adjective) => (
            <span key={adjective} className="rounded-full border border-line px-2.5 py-1 text-xs text-ink-2">
              {adjective}
            </span>
          ))}
        </div>
      ) : null}

      <div>
        <label htmlFor="onboarding-style" className={labelCls}>Style</label>
        <input
          id="onboarding-style"
          className="field"
          value={style}
          onChange={(event) => onStyleChange(event.target.value)}
          autoComplete="off"
        />
      </div>

      <div>
        <label htmlFor="onboarding-system" className={labelCls}>Instructions</label>
        <textarea
          id="onboarding-system"
          className="field min-h-32 resize-y"
          value={system}
          onChange={(event) => onSystemChange(event.target.value)}
          placeholder="Generated instructions appear here — edit anything."
        />
      </div>
    </section>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=AgentStep.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/components/onboarding/AgentStep.tsx apps/web/src/components/onboarding/AgentStep.test.tsx
git commit -m "feat(web): add onboarding agent step with presets and generation"
```

---

### Task 13: Rework `OnboardingFlow` to five steps with bootstrap submit

**Files:**
- Modify: `apps/web/src/components/onboarding/OnboardingFlow.tsx`
- Modify: `apps/web/src/components/onboarding/OnboardingFlow.test.tsx`
- Delete: `apps/web/src/components/onboarding/IdentityStep.tsx`

- [ ] **Step 1: Rewrite the tests first**

Replace `OnboardingFlow.test.tsx` with tests covering:

1. Renders Workspace step first (company name field visible).
2. Next blocked with empty company name → blocking error, focus company input.
3. Verify failure shows inline error and keeps draft.
4. Order: Workspace → Intelligence (model) → Agent → Access → Review; Back preserves all drafts.
5. Intelligence unchanged behavior: provider catalog error blocks only that step; workspace draft preserved (adapt existing test).
6. Agent step: selecting a preset fills template bio/system (fallback path); Generate calls `daemon.generateProfile` with preset/intent/provider/model/workspace; daemon `PROFILE_GENERATION_UNAVAILABLE` error switches to template fallback + notice.
7. Review: summary shows company, mission, folder, agent name, preset label, provider/model, access.
8. Submit calls `daemon.bootstrapWorkspace` once with the full payload (exact tool list from `toolNamesForProfile(access)`); on success calls `onCreated` with the returned agent snapshot.
9. Bootstrap failure stays on Review with the entire draft intact.
10. Empty custom model blocks at Intelligence (adapt existing).

Mock `daemon.getWorkspace`, `daemon.validateWorkspace`, `daemon.generateProfile`, `daemon.bootstrapWorkspace` in the `vi.mock('../lib/daemon-api')` block following the existing pattern in the file.

- [ ] **Step 2: Run tests to verify they fail**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=OnboardingFlow.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement**

Rework `OnboardingFlow.tsx`:

- New draft shape:

```ts
interface WorkspaceDraft {
  companyName: string;
  mission: string;
  rootPath: string;
  values: string[];
}

interface OnboardingDraft {
  workspace: WorkspaceDraft;
  name: string;
  presetId: PresetId;
  intent: string;
  bio: string;
  adjectives: string[];
  style: string;
  system: string;
  provider: string;
  model: string;
  customModel: string;
  access: AccessProfile;
}

const INITIAL_DRAFT: OnboardingDraft = {
  workspace: { companyName: '', mission: '', rootPath: '', values: [] },
  name: 'Anima',
  presetId: 'chief-of-staff',
  intent: '',
  bio: '',
  adjectives: [],
  style: '',
  system: '',
  provider: '',
  model: '',
  customModel: '',
  access: 'collaborate',
};
```

- On mount, call `daemon.getWorkspace()` and pre-fill `rootPath` from `defaultRoot` (failure is non-blocking — leave the field empty).
- Step indices: 0 Workspace, 1 Intelligence (`ModelStep`, unchanged props), 2 Agent, 3 Access, 4 Review.
- Validation per step:
  - 0: company name, mission, root path all non-empty.
  - 1: existing provider/model logic (keep the `intelligenceReady` machinery; renumber the `currentStep === 1` references accordingly — the model step is still index 1, so most conditions survive).
  - 2: name non-empty + `system` non-empty (from generation, template, or edits). If `system` is empty when leaving the step, fill it from `presetTemplate(presetId, { companyName, mission, agentName: name })` first, then proceed.
- Verify action: `daemon.validateWorkspace({ rootPath, companyName, mission, values })`; success → `verifyStatus = { ok: true, willCreate: response.rootPathExists === false }`. Failure → `verifyStatus = { ok: false, message }`. Any edit to the root path resets `verifyStatus` to null.
- Generate action: `daemon.generateProfile({ presetId, intent, provider: draft.provider, model: resolvedModel, workspace: { companyName, mission, values } })`; success → fill bio/adjectives/style/system. On error whose message starts with `PROFILE_GENERATION_UNAVAILABLE` → set `generateAvailable = false` and fill fields from `presetTemplate` (only fields still empty). Other errors → `generateError` message, keep existing field values.
- Preset change: set `presetId`; if the profile fields are untouched (still initial/empty), fill from the new preset's template.
- Submit (Review): build the bootstrap payload:

```ts
const response = await daemon.bootstrapWorkspace({
  workspace: {
    rootPath: draft.workspace.rootPath.trim(),
    companyName: draft.workspace.companyName.trim(),
    mission: draft.workspace.mission.trim(),
    values: draft.workspace.values,
  },
  agent: {
    name,
    presetId: draft.presetId,
    bio: draft.bio.trim() || undefined,
    adjectives: draft.adjectives.length ? draft.adjectives : undefined,
    style: draft.style.trim() || undefined,
    system: draft.system.trim(),
    provider: draft.provider || undefined,
    model: resolvedModel,
    tools: toolNamesForProfile(draft.access),
  },
});
onCreated(response.agent);
```

Keep the existing guards: `submitInFlightRef`, provider-catalog-changed redirect (now back to step 1), blocking errors, focus management (company input on step 0, name input on step 2, model select on step 1).

- Header copy: kicker `Guided Focus · Workspace`, title `Set up your workspace`, subtitle `Name your company, pick its folder, and hire your first agent.`

- Delete `IdentityStep.tsx` and its import.

- [ ] **Step 4: Run tests to verify they pass**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=OnboardingFlow.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/components/onboarding
git commit -m "feat(web): rebuild onboarding as workspace-first five-step flow"
```

---

### Task 14: Review step summary + progress commit

**Files:**
- Modify: `apps/web/src/components/onboarding/ReviewStep.tsx`
- Modify: `apps/web/src/components/onboarding/OnboardingProgress.tsx` (from Task 10)
- Test: cover the summary inside `OnboardingFlow.test.tsx` (Task 13 item 7)

- [ ] **Step 1: Extend `ReviewStep` props and summary**

Add `workspace: { companyName: string; mission: string; rootPath: string }`, `presetLabel: string`, and `bio: string` to `ReviewStepProps`. Render a workspace block above the existing agent summary:

```tsx
<div className="rounded-2xl border border-line bg-white/[0.02] p-4">
  <p className="text-sm font-semibold text-ink">🏢 {workspace.companyName}</p>
  <p className="mt-1 text-sm text-ink-2">{workspace.mission}</p>
  <p className="mt-1 truncate font-mono text-xs text-ink-3" title={workspace.rootPath}>
    {workspace.rootPath}
  </p>
</div>
```

Add a bio preview line under the agent name, and an atomicity note:

```tsx
<p className="text-xs leading-relaxed text-ink-3">
  Creates the workspace, the company file (anima.yaml), and your agent in one step — if anything fails, nothing is half-created.
</p>
```

- [ ] **Step 2: Run flow tests**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=OnboardingFlow.test.tsx`
Expected: PASS (including the Review summary assertions).

- [ ] **Step 3: Commit**

```bash
git add apps/web/src/components/onboarding/ReviewStep.tsx apps/web/src/components/onboarding/OnboardingProgress.tsx
git commit -m "feat(web): add workspace summary to review step and five-step progress"
```

---

## Phase 7 — Web: workspace identity in the shell

### Task 15: Company name in shell header + read-only workspace in Settings

**Files:**
- Modify: `apps/web/src/ViewHarness.tsx` (fetch workspace state alongside bootstrap)
- Modify: `apps/web/src/hooks/useDaemonBootstrap.ts` (expose workspace)
- Modify: `apps/web/src/components/WorkspaceShell.tsx` (header)
- Modify: `apps/web/src/components/SettingsPanel.tsx` (read-only workspace section)
- Test: `apps/web/src/hooks/useDaemonBootstrap.test.tsx`, `apps/web/src/components/WorkspaceShell.test.tsx`, `apps/web/src/components/SettingsPanel.test.tsx`

- [ ] **Step 1: Write the failing tests**

- `useDaemonBootstrap.test.tsx`: mock `daemon.getWorkspace`; assert the hook returns `workspace` state and tolerates `getWorkspace` rejection (workspace = null, no bootstrap failure).
- `WorkspaceShell.test.tsx`: with `workspace={{ companyName: 'Northwind Research', ... }}`, the header shows "Northwind Research"; with `workspace={null}`, the header renders exactly as today.
- `SettingsPanel.test.tsx`: workspace section renders company name, mission, and root path read-only; absent workspace → section hidden.

- [ ] **Step 2: Run tests to verify they fail**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=useDaemonBootstrap.test.tsx --testFile=WorkspaceShell.test.tsx --testFile=SettingsPanel.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement**

- `useDaemonBootstrap`: add `workspace: DaemonWorkspaceState | null` state; fetch `daemon.getWorkspace()` in the same bootstrap pass as providers/agents; on failure set null and continue. Include it in the polling refresh at the same cadence as agents (cheap single GET).
- `ViewHarness`: pass `workspace` through to `WorkspaceShell` and `SettingsPanel`; after onboarding `onCreated`, refresh workspace state (or accept the bootstrap response's workspace — simplest: call `getWorkspace()` once in the `onCreated` handler).
- `WorkspaceShell`: when `workspace?.configured`, render `{workspace.workspace.companyName}` next to the shell brand; no layout change otherwise.
- `SettingsPanel`: new "Workspace" section above identity settings, read-only rows (Company, Mission, Folder). Editing is a documented follow-up.

- [ ] **Step 4: Run tests to verify they pass**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache`
Expected: PASS — full web suite.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src
git commit -m "feat(web): surface workspace identity in shell and settings"
```

---

## Phase 8 — Final verification

### Task 16: Full workspace verification

- [ ] **Step 1: Rust suite + lint**

Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Run: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:lint --skipNxCache`
Expected: both PASS.

- [ ] **Step 2: Web suite + typecheck + build**

Run: `bun x nx test @animaOS-SWARM/web --skipNxCache`
Run: `bun x nx run @animaOS-SWARM/web:build --skipNxCache`
Expected: PASS (build performs the Vite production build and type checking).

- [ ] **Step 3: Manual browser pass**

Run `bun dev --host rust`, then with a fresh daemon data directory:
1. Onboarding opens on the Workspace step; complete all five steps against the deterministic provider (template fallback path) → agent created, `anima.yaml` exists at the chosen root, shell header shows the company name.
2. Repeat against a configured provider and use ✨ Generate (generation path); edit the generated instructions before creating.
3. Kill the daemon mid-flow; confirm the offline state never shows onboarding losing the draft; restart and confirm the created agent + workspace survive.
4. Small-screen width: stepper labels truncate, Verify/Generate buttons stay reachable.
5. Keyboard-only pass through all five steps; visible focus on preset cards and Verify/Generate buttons.

- [ ] **Step 4: Final commit (docs only if nothing changed)**

```bash
git add -A
git commit -m "chore: final verification for workspace-first onboarding" --allow-empty
```

---

## Out of Scope (documented follow-ups)

- Hire-a-worker flow that appends to `anima.yaml` and creates additional runtime agents.
- Editing workspace config after onboarding (Settings section is read-only for now).
- `generate-profile` model selection independent of the agent's model.
- Migrating pre-existing daemon agents into agencies.
