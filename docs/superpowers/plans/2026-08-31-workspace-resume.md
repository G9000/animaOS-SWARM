# Workspace Resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user point the daemon at an existing folder containing `anima.yaml` and resume it — restoring workspace config, orchestrator, and workers — instead of re-running onboarding.

**Architecture:** Two new daemon endpoints (read-only `GET /api/workspace/inspect` preview + atomic `POST /api/workspace/resume` adopt) reusing bootstrap's validation/transaction/rollback discipline. The web onboarding Workspace step gains an "Already have a workspace?" path that inspects, shows a resume card, and resumes with one click.

**Tech Stack:** Rust (axum, serde_yaml, utoipa) daemon; React + TypeScript + Tailwind web console; vitest.

**Spec:** `docs/superpowers/specs/2026-08-31-workspace-resume-design.md` (approved)

---

## Context for Implementers (read me first)

**Environment:**
- bun is NOT on PATH: use `"$HOME/.bun/bin/bun"`. cargo is NOT on PATH: `export PATH="$HOME/.cargo/bin:$HOME/.bun/bin:$PATH"` before any nx/cargo command.
- Rust gates: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache` and `... rust-daemon:lint --skipNxCache`.
- Web full suite: `bun x nx test @animaOS-SWARM/web --skipNxCache`. Focused web tests: `cd apps/web && bun x vitest run <file-or-substring>` (vitest 4 has NO `--testFile` flag).
- Web typecheck: `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache`; build: `bun x nx run @animaOS-SWARM/web:build --skipNxCache`.
- Commits are GPG-signed. The user may have unrelated uncommitted changes in the tree — NEVER `git add -A`/`git add .`; stage only the files each task lists.

**Key existing code:**
- `hosts/rust-daemon/src/routes/agencies.rs:35-139` — `AgencyYamlConfig` / `AgencyYamlAgent` (derive `Serialize` only; fields private). Bootstrap builds these via `single_orchestrator` / `orchestrator` constructors.
- `hosts/rust-daemon/src/routes/workspace.rs` — `handle_bootstrap_workspace` (validate-first, transaction, `rollback_bootstrap`), `validate_workspace_request`, `validate_root_path`, `config_response`, `default_root_label`. New handlers go in this file.
- `hosts/rust-daemon/src/routes/contracts/workspace.rs` — existing workspace contracts; new contracts go here.
- `hosts/rust-daemon/src/routes/mod.rs:279-283` — workspace route registration; `:1184-1240` — entry-point pattern (`*_entry` + `#[utoipa::path]`); `:1162-1180` — `list_providers_entry` using `provider_summaries()` (each summary has `id`, `configured`).
- `hosts/rust-daemon/src/state.rs:2049-2071` — `create_agent(AgentConfig)` canonicalizes tools via `resolve_agent_tools` (rejects unknown slugs before mutation). `:2154` — `list_agents() -> Vec<AgentRuntimeSnapshot>` (use `.state.name` for the name-skip rule).
- `hosts/rust-daemon/tests/workspace_api.rs` + `tests/support/mod.rs` — integration test patterns: `test_app()`, `use_temp_workspace_root(prefix)`, `send_json_request`, `extract_json_string_field`.
- `apps/web/src/lib/daemon-api.ts` — `getWorkspace`, `validateWorkspace`, `bootstrapWorkspace`, `AgentProfile` types. New client methods go here.
- `apps/web/src/components/onboarding/OnboardingFlow.tsx` — wizard orchestrator: draft state, `verifyRequestIdRef` stale-response guard, `verifyWorkspace`/`generateProfile`/`submit` async actions with in-flight + mounted guards, header copy. Mirror these patterns for inspect/resume.
- `apps/web/src/components/onboarding/WorkspaceStep.tsx` — controlled step 0 component.
- `packages/cli/src/agency/loader.ts:62-86` — the yaml invariants resume must enforce: truthy `name`; `orchestrator.name/bio/system` truthy; `agents` defaults to `[]`; missing `provider` defaults to `"openai"`.

---

## Phase 1 — Daemon: parse + inspect + resume

### Task 1: Deserialize support + `load_agency_yaml` helper

**Files:**
- Modify: `hosts/rust-daemon/src/routes/agencies.rs:35-139`

- [ ] **Step 1: Write the failing unit tests**

Add to `#[cfg(test)] mod tests` in `agencies.rs` (create the module if absent):

```rust
#[cfg(test)]
mod tests {
    use super::load_agency_yaml;

    fn write_yaml(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("anima.yaml");
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    #[test]
    fn parses_full_config_with_workers() {
        let dir = std::env::temp_dir().join(format!("anima-yaml-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = write_yaml(&dir, r#"
name: Northwind Research
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
"#);
        let config = load_agency_yaml(&path).expect("should parse");
        assert_eq!(config.name, "Northwind Research");
        assert_eq!(config.orchestrator.name, "Anima");
        assert_eq!(config.agents.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn defaults_agents_to_empty_when_missing() {
        let dir = std::env::temp_dir().join(format!("anima-yaml-min-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = write_yaml(&dir, "name: Co\nmodel: m\norchestrator:\n  name: A\n  bio: b\n  system: s\n");
        let config = load_agency_yaml(&path).expect("should parse");
        assert!(config.agents.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_missing_orchestrator_bio() {
        let dir = std::env::temp_dir().join(format!("anima-yaml-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = write_yaml(&dir, "name: Co\nmodel: m\norchestrator:\n  name: A\n  system: s\n");
        assert!(load_agency_yaml(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_invalid_yaml() {
        let dir = std::env::temp_dir().join(format!("anima-yaml-inv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = write_yaml(&dir, "{{{{ not yaml");
        assert!(load_agency_yaml(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$HOME/.bun/bin:$PATH" && CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`
Expected: FAIL — `load_agency_yaml` missing, fields not readable.

- [ ] **Step 3: Implement**

In `agencies.rs`:

1. Add `Deserialize` to both structs and `pub(crate)` to all fields of both structs:

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgencyYamlConfig {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) values: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) provider: String,
    #[serde(default)]
    pub(crate) strategy: String,
    // ... same treatment for max_parallel_delegations / orchestrator / agents:
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_parallel_delegations: Option<u64>,
    pub(crate) orchestrator: AgencyYamlAgent,
    #[serde(default)]
    pub(crate) agents: Vec<AgencyYamlAgent>,
}
```

`AgencyYamlAgent`: add `Deserialize`; all `Option` fields + `bio` get `#[serde(default)]` (keep existing `skip_serializing_if`); `name`/`system` stay required at the serde layer (blank values are rejected by validation below). Make all fields `pub(crate)`. Note: keeping worker `system` serde-required is deliberately stricter than the CLI loader (which defaults a missing worker system to `''`) — a system-less worker fails loudly with a clear 400 rather than resuming with empty instructions. Record that choice in a comment on the loader.

2. Add the shared loader at the end of the struct section:

```rust
/// Parse and validate an anima.yaml agency file. Enforces the same invariants
/// as the CLI loader (packages/cli/src/agency/loader.ts): truthy `name`, and
/// an orchestrator with truthy name/bio/system. `agents` defaults to empty.
pub(crate) fn load_agency_yaml(path: &Path) -> Result<AgencyYamlConfig, ApiError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| ApiError::bad_request(format!("could not read {}: {error}", path.display())))?;
    let config: AgencyYamlConfig = serde_yaml::from_str(&raw)
        .map_err(|error| ApiError::bad_request(format!("anima.yaml is not valid: {error}")))?;
    if config.name.trim().is_empty() {
        return Err(ApiError::bad_request_static("anima.yaml: name is required"));
    }
    if config.orchestrator.name.trim().is_empty()
        || config.orchestrator.bio.trim().is_empty()
        || config.orchestrator.system.trim().is_empty()
    {
        return Err(ApiError::bad_request_static(
            "anima.yaml: orchestrator must have name, bio, and system",
        ));
    }
    Ok(config)
}
```

(Import `Deserialize` from serde alongside `Serialize`.)

- [ ] **Step 4: Run tests to verify they pass** — same command; all four pass.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes/agencies.rs
git commit -m "feat(daemon): add anima.yaml parsing with CLI-loader invariants"
```

---

### Task 2: Inspect contracts + `GET /api/workspace/inspect`

**Files:**
- Modify: `hosts/rust-daemon/src/routes/contracts/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (entry + route + OpenAPI)
- Test: `hosts/rust-daemon/tests/workspace_api.rs`

- [ ] **Step 1: Write the failing integration tests**

In `tests/workspace_api.rs`, following the existing bootstrap test patterns (`test_app()`, `use_temp_workspace_root`, `send_json_request` / `send_empty_request` for GET-with-query):

```rust
#[tokio::test]
async fn inspect_returns_found_false_without_yaml() {
    let _root = use_temp_workspace_root("inspect-empty");
    let app = test_app();
    let (status, body) = send_empty_request(
        &app,
        &format!("/api/workspace/inspect?rootPath={}", url_encoded_root),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"found\":false"));
}

#[tokio::test]
async fn inspect_returns_preview_for_valid_yaml() {
    // write a valid anima.yaml (orchestrator + 1 worker) into the temp root first
    // assert: 200, "found":true, company name, orchestrator name, worker name,
    // providerAvailable present
}

#[tokio::test]
async fn inspect_rejects_malformed_yaml() {
    // write "{{ not yaml" as anima.yaml; expect 400
}

#[tokio::test]
async fn inspect_rejects_yaml_missing_orchestrator_fields() {
    // valid yaml, orchestrator without bio; expect 400
}
```

Before writing these tests, add a small helper to `tests/support/mod.rs` so query strings stay correct:

```rust
/// Build a GET URI with one query parameter, percent-encoding the value
/// (Windows paths contain `:` and `\`, which must be encoded).
pub(crate) fn query_uri(path: &str, param: &str, value: &str) -> String {
    let encoded: String = value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect();
    format!("{path}?{param}={encoded}")
}
```

Use `query_uri("/api/workspace/inspect", "rootPath", root.path().to_str().unwrap())` in every inspect/resume test instead of the `url_encoded_root` placeholder above. (Multi-byte chars: percent-encoding per `char` is fine for these tests since paths are ASCII temp dirs.)

- [ ] **Step 2: Run to verify fail** — route 404.

- [ ] **Step 3: Implement**

Contracts (`contracts/workspace.rs`):

```rust
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceInspectAgentPreview {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bio: Option<String>, // orchestrator only; workers omit bio/system
    pub(crate) provider: String,
    pub(crate) model: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceInspectResponse {
    pub(crate) found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) company_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) orchestrator: Option<WorkspaceInspectAgentPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workers: Option<Vec<WorkspaceInspectAgentPreview>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider_available: Option<bool>,
}
```

Handler (`workspace.rs`):

```rust
pub(crate) async fn handle_inspect_workspace(
    root_path: &str,
) -> Result<WorkspaceInspectResponse, ApiError> {
    let not_found = || WorkspaceInspectResponse {
        found: false,
        company_name: None,
        mission: None,
        values: None,
        orchestrator: None,
        workers: None,
        provider_available: None,
    };
    let trimmed = root_path.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request_static("rootPath is required"));
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_dir() {
        return Ok(not_found());
    }
    let yaml_path = candidate.join("anima.yaml");
    if !yaml_path.is_file() {
        return Ok(not_found());
    }
    let config = super::agencies::load_agency_yaml(&yaml_path)?;
    let effective_provider = |agent_provider: &str| {
        let provider = agent_provider.trim();
        if provider.is_empty() { "openai".to_string() } else { provider.to_string() }
    };
    let provider = effective_provider(&config.provider);
    // providerAvailable mirrors the providers catalog: unknown or
    // unconfigured providers (and the deterministic adapter) report false.
    let provider_available = provider_summaries()
        .into_iter()
        .find(|summary| summary.id == provider)
        .map(|summary| summary.configured && summary.id != "deterministic")
        .unwrap_or(false);
    let preview = |agent: &AgencyYamlAgent, include_bio: bool| WorkspaceInspectAgentPreview {
        name: agent.name.clone(),
        bio: if include_bio { Some(agent.bio.clone()) } else { None },
        provider: provider.clone(),
        model: agent.model.clone().unwrap_or_else(|| config.model.clone()),
    };
    Ok(WorkspaceInspectResponse {
        found: true,
        company_name: Some(config.name.clone()),
        mission: config.mission.clone().or(Some(config.description.clone())),
        values: config.values.clone(),
        orchestrator: Some(preview(&config.orchestrator, true)),
        workers: Some(config.agents.iter().map(|agent| preview(agent, false)).collect()),
        provider_available: Some(provider_available),
    })
}
```

(Import whatever `mod.rs` imports for `provider_summaries` — find its `use` path at the top of `mod.rs` and mirror it. If `provider_summaries` lives in a module not visible from `workspace.rs`, re-export it `pub(crate)` from its home module.)

Route registration (`mod.rs`): mirror the existing workspace entries —

```rust
#[utoipa::path(
    get,
    path = "/api/workspace/inspect",
    tag = "workspace",
    params(("rootPath" = String, Query, description = "Workspace root path to inspect")),
    responses(
        (status = 200, description = "Workspace inspection result", body = WorkspaceInspectResponse),
        (status = 400, description = "Invalid path or anima.yaml", body = ErrorBody)
    )
)]
async fn inspect_workspace_entry(
    Query(query): Query<WorkspaceInspectQuery>,
) -> AxumResponse {
    handle_result(workspace::handle_inspect_workspace(&query.root_path).await)
}
```

with `#[derive(Deserialize)] struct WorkspaceInspectQuery { #[serde(rename = "rootPath")] root_path: String }` near the other request structs. Register `.route("/api/workspace/inspect", get(inspect_workspace_entry))` next to the existing workspace routes and add the path to the OpenAPI paths list. (Check how existing entries convert handler results to responses — mirror the exact helper used by `get_workspace_entry`.)

- [ ] **Step 4: Run tests to verify pass** — full Rust gate command; new tests green.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes/contracts/workspace.rs hosts/rust-daemon/src/routes/workspace.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/tests/workspace_api.rs
git commit -m "feat(daemon): add read-only GET /api/workspace/inspect"
```

---

### Task 3: `POST /api/workspace/resume` — fresh adopt

**Files:**
- Modify: `hosts/rust-daemon/src/routes/contracts/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (entry + route + OpenAPI)
- Test: `hosts/rust-daemon/tests/workspace_api.rs`

- [ ] **Step 1: Write the failing integration tests**

```rust
#[tokio::test]
async fn resume_adopts_workspace_with_orchestrator_and_workers() {
    // temp root; write valid anima.yaml (orchestrator Anima + workers Scout, Scribe)
    // POST /api/workspace/resume {rootPath}
    // expect 201; body has workspace.companyName, orchestrator.name "Anima", workers len 2
    // GET /api/workspace -> configured:true with the workspace
    // GET /api/agents -> 3 agents
    // anima.yaml content UNCHANGED (read before and after, byte-compare)
}

#[tokio::test]
async fn resume_rejects_unknown_tool_without_side_effects() {
    // yaml with orchestrator tools: ["not_a_real_tool"]
    // expect 400; GET /api/agents empty; GET /api/workspace configured:false
}

#[tokio::test]
async fn resume_without_yaml_returns_400() {
    // empty temp root; expect 400 mentioning anima.yaml
}
```

- [ ] **Step 2: Run to verify fail** — route 404.

- [ ] **Step 3: Implement**

Contracts:

```rust
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceResumeRequest {
    pub(crate) root_path: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceResumeResponse {
    pub(crate) workspace: WorkspaceConfigResponse,
    pub(crate) orchestrator: AgentRuntimeSnapshotResponse,
    pub(crate) workers: Vec<AgentRuntimeSnapshotResponse>,
}
```

Handler (`workspace.rs`):

```rust
pub(crate) async fn handle_resume_workspace(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<WorkspaceResumeResponse, ApiError> {
    let request: WorkspaceResumeRequest = super::parse_json_body(body)?;

    // 1. Validate the folder WITHOUT creating it (validate_only=true): resume
    //    never creates directories; a missing folder/yaml is a 400.
    let root = validate_root_path(&request.root_path, true)?;
    let yaml_path = root.join("anima.yaml");
    if !yaml_path.is_file() {
        return Err(ApiError::bad_request(format!(
            "no anima.yaml found at {}",
            root.display()
        )));
    }
    let agency = super::agencies::load_agency_yaml(&yaml_path)?;

    // 2. Conflict + collision rules, resolved inside one read.
    let existing_names: std::collections::BTreeSet<String> = {
        let guard = state.read().await;
        if let Some(current) = &guard.workspace {
            if current.root_path != root {
                return Err(ApiError::conflict(format!(
                    "workspace is already configured for {}",
                    current.root_path.display()
                )));
            }
        }
        guard
            .list_agents()
            .iter()
            .map(|snapshot| snapshot.state.name.clone())
            .collect()
    };

    // 3. Build AgentConfigs for every yaml agent not already present (name-skip).
    //    Tool slugs are validated by create_agent -> resolve_agent_tools before
    //    any mutation, but we must know upfront whether ANY would fail, so we
    //    pre-validate by resolving against the registry. Mirror what
    //    create_agent does; if resolve_agent_tools is only callable on the
    //    guard, do step 3+4 inside one write guard and rely on create_agent's
    //    pre-mutation validation — create_agent validates tools BEFORE
    //    inserting (state.rs:2053), so a later failure leaves earlier agents
    //    created: therefore validate ALL agents' tools FIRST by calling
    //    resolve_agent_tools in a read pass if accessible, otherwise create
    //    orchestrator/workers in one write guard and roll back the whole batch
    //    on the first error (preferred — see step 4).
    let workspace_config = WorkspaceConfig {
        root_path: root.clone(),
        company_name: agency.name.trim().to_string(),
        mission: agency
            .mission
            .clone()
            .unwrap_or_else(|| agency.description.clone())
            .trim()
            .to_string(),
        values: agency.values.clone().unwrap_or_default(),
    };
    if workspace_config.mission.is_empty() {
        return Err(ApiError::bad_request_static(
            "anima.yaml: mission or description is required",
        ));
    }

    let to_agent_config = |agent: &AgencyYamlAgent, is_orchestrator: bool| -> Result<AgentConfig, ApiError> {
        let name = agent.name.trim().to_string();
        let model = agent
            .model
            .clone()
            .unwrap_or_else(|| agency.model.clone())
            .trim()
            .to_string();
        if model.is_empty() {
            return Err(ApiError::bad_request(format!(
                "anima.yaml: agent {name} has no model and the file sets no default"
            )));
        }
        let bio = agent.bio.trim().to_string();
        if is_orchestrator && bio.is_empty() {
            return Err(ApiError::bad_request_static(
                "anima.yaml: orchestrator bio is required",
            ));
        }
        Ok(AgentConfig {
            name,
            model,
            bio: if bio.is_empty() { None } else { Some(bio) },
            lore: agent.lore.clone(),
            knowledge: agent.knowledge.clone(),
            topics: agent.topics.clone(),
            adjectives: agent.adjectives.clone(),
            style: agent.style.clone(),
            provider: {
                let provider = agency.provider.trim().to_string();
                if provider.is_empty() { None } else { Some(provider) }
            },
            system: Some(agent.system.clone()),
            tools: agent.tools.as_ref().map(|tools| {
                tools
                    .iter()
                    .map(|tool| ToolDescriptor {
                        name: tool.clone(),
                        description: String::new(),
                        parameters_schema: BTreeMap::new(),
                        examples: None,
                    })
                    .collect::<Vec<_>>()
            }),
            plugins: None,
            settings: None,
        })
    };

    // 4. One write guard: create all missing agents, tracking created ids so a
    //    failure mid-batch rolls back the whole batch (create_agent validates
    //    tools before inserting, but an earlier agent in the batch would
    //    already exist — remove it on error).
    let previous_workspace = {
        let guard = state.read().await;
        guard.workspace.clone()
    };
    let mut created: Vec<(String, AgentRuntimeSnapshot)> = Vec::new();
    let mut persist_request = {
        let mut guard = state.write().await;
        let mut failed: Option<String> = None;
        for (agent, is_orchestrator) in std::iter::once((&agency.orchestrator, true))
            .chain(agency.agents.iter().map(|agent| (agent, false)))
        {
            if existing_names.contains(agent.name.trim()) {
                continue;
            }
            match to_agent_config(agent, is_orchestrator)
                .and_then(|config| guard.create_agent(config).map_err(ApiError::bad_request))
            {
                Ok(snapshot) => created.push((snapshot.state.id.clone(), snapshot)),
                Err(error) => {
                    failed = Some(error.to_string());
                    break;
                }
            }
        }
        if let Some(message) = failed {
            for (id, _) in &created {
                guard.remove_agent(id);
            }
            return Err(ApiError::bad_request(message));
        }
        if created.is_empty() {
            return Err(ApiError::conflict(
                "all agents from anima.yaml already exist",
            ));
        }
        guard.workspace = Some(workspace_config.clone());
        guard.control_plane_persist_request()
    };

    // 5. Persist; on failure roll back to the previous state (never None when
    //    a workspace was already configured for this root).
    if let Err(error) = persist_request.save().await {
        rollback_resume(state, &created, previous_workspace).await;
        return Err(ApiError::service_unavailable(error.to_string()));
    }

    let mut created_snapshots = created.into_iter().map(|(_, snapshot)| snapshot);
    let orchestrator = created_snapshots
        .next()
        .map(|snapshot| AgentRuntimeSnapshotResponse::from(&snapshot))
        // The orchestrator was skipped by the name-skip rule: report the
        // existing one. (Reachable only on same-root re-resume.)
        .unwrap_or_else(|| unreachable!("fresh adopt always creates the orchestrator"));
    Ok(WorkspaceResumeResponse {
        workspace: config_response(&workspace_config),
        orchestrator,
        workers: created_snapshots
            .map(|snapshot| AgentRuntimeSnapshotResponse::from(&snapshot))
            .collect(),
    })
}

/// Undo a partially applied resume: remove only the agents this resume
/// created and restore the previous workspace config (Some when re-resuming
/// an already-configured root, None for a fresh adopt). Never touches the
/// yaml file — resume does not own it.
async fn rollback_resume(
    state: &SharedDaemonState,
    created: &[(String, AgentRuntimeSnapshot)],
    previous_workspace: Option<WorkspaceConfig>,
) {
    let rollback_request = {
        let mut guard = state.write().await;
        for (id, _) in created {
            guard.remove_agent(id);
        }
        guard.workspace = previous_workspace;
        guard.control_plane_persist_request()
    };
    rollback_request.save().await.ok();
}
```

NOTE for the implementer: the reference above assumes `remove_agent(&str)` and `control_plane_persist_request()` signatures from `rollback_bootstrap` — mirror them exactly. The `unreachable!` for the skipped-orchestrator case is acceptable for fresh adopt; Task 4 refines same-root re-resume semantics including response shape when the orchestrator already exists — if simpler, have Task 3 return 409 when the orchestrator name already exists and let Task 4 implement the skip/restore behavior. Pick ONE, keep it consistent, and note the choice in your report.

Route entry (`mod.rs`): mirror `bootstrap_workspace_entry` — `post`, path `/api/workspace/resume`, 201 response with `WorkspaceResumeResponse`, 400/409/503 documented, register route + OpenAPI path. IMPORTANT: like every existing mutating entry, the wrapper must take the `control_plane_transaction()` guard before delegating to `handle_resume_workspace` (see how `bootstrap_workspace_entry` does it) — the spec's "one control-plane transaction" requirement depends on it; do not drop it when copying the snippet.

- [ ] **Step 4: Run tests to verify pass.**

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes/contracts/workspace.rs hosts/rust-daemon/src/routes/workspace.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/tests/workspace_api.rs
git commit -m "feat(daemon): add atomic POST /api/workspace/resume"
```

---

### Task 4: Resume conflict + idempotency + rollback tests

**Files:**
- Modify: `hosts/rust-daemon/src/routes/workspace.rs` (only if Task 3 chose the 409-on-existing-orchestrator simplification and this task implements skip/restore)
- Test: `hosts/rust-daemon/tests/workspace_api.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn resume_conflicts_when_configured_for_different_root() {
    // bootstrap workspace at root A (reuse the existing bootstrap test helper/pattern)
    // write anima.yaml at root B; POST resume {rootPath: B}
    // expect 409 naming root A; root B's agents not created
}

#[tokio::test]
async fn resume_same_root_restores_missing_agents_only() {
    // bootstrap at root A with orchestrator "Anima"
    // hand-edit/rewrite anima.yaml at A to add a worker "Scout" (or use a yaml
    //   that already had Scout and delete the worker via DELETE /api/agents/:id
    //   if that endpoint exists — check agent_api.rs tests for the pattern)
    // POST resume {rootPath: A}
    // expect 201; Anima NOT duplicated (still 1 agent named Anima); Scout created
}

#[tokio::test]
async fn resume_fresh_adopt_skips_persisted_agent_name_collisions() {
    // pre-create a standalone agent named "Anima" via POST /api/agents
    //   (see agent_api.rs tests for the exact payload)
    // write anima.yaml with orchestrator "Anima" + worker "Scout" at temp root
    // POST resume; expect 201; the pre-existing Anima kept (id unchanged);
    // Scout created; workspace configured
}

#[tokio::test]
async fn resume_rolls_back_when_persist_fails() {
    // follow the existing bootstrap rollback test's technique for forcing a
    //   persist failure (check how tests/workspace_api.rs forces it — likely a
    //   control-plane file path that cannot be written)
    // expect 503; GET /api/agents empty; GET /api/workspace configured:false
}

#[tokio::test]
async fn resumed_agents_survive_restart() {
    // only if the test harness supports respawning the app against the same
    // control-plane file (check how existing restart/persistence tests do it —
    // mirror that); otherwise cover persistence by asserting the control-plane
    // file contents directly
}
```

- [ ] **Step 2–4:** Run red → implement any missing pieces → run green. Specifically:
  - If Task 3 chose the 409-on-existing-orchestrator simplification, this task implements the name-skip/restore behavior.
  - If Task 3 kept the `unreachable!` orchestrator extraction, fix the response shape now: on same-root re-resume where the orchestrator was skipped, return the EXISTING orchestrator snapshot (look it up from `list_agents()` by name) rather than the first created worker — `resume_same_root_restores_missing_agents_only` exposes this bug.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/routes/workspace.rs hosts/rust-daemon/tests/workspace_api.rs
git commit -m "feat(daemon): resume conflict, idempotency, and rollback semantics"
```

---

### Task 5: Rust verification checkpoint

- [ ] `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache` — PASS
- [ ] `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:lint --skipNxCache` — PASS

No commit unless fixes were needed.

---

## Phase 2 — Web client

### Task 6: `inspectWorkspace` + `resumeWorkspace` client methods

**Files:**
- Modify: `apps/web/src/lib/daemon-api.ts`
- Test: `apps/web/src/lib/daemon-api.test.ts`

- [ ] **Step 1: Write the failing tests** — mirror the existing `validateWorkspace`/`bootstrapWorkspace` tests in `daemon-api.test.ts` (fetch mock pattern):

```ts
it('inspectWorkspace issues GET with encoded rootPath', async () => {
  mockFetchJson({ found: false });
  const result = await daemon.inspectWorkspace('C:\\anima');
  expect(result).toEqual({ found: false });
  expect(lastRequestUrl()).toContain('/api/workspace/inspect?rootPath=');
  expect(lastRequestUrl()).toContain(encodeURIComponent('C:\\anima'));
});

it('resumeWorkspace posts rootPath and returns the envelope', async () => {
  mockFetchJson({ workspace: {}, orchestrator: { state: { name: 'Anima' } }, workers: [] });
  const result = await daemon.resumeWorkspace('C:\\anima');
  expect(lastRequestMethod()).toBe('POST');
  expect(lastRequestBody()).toEqual({ rootPath: 'C:\\anima' });
  expect(result.orchestrator.state.name).toBe('Anima');
});
```

(Adapt `mockFetchJson`/`lastRequest*` to the actual helpers in the existing test file.)

- [ ] **Step 2: Run to verify fail** — `cd apps/web && bun x vitest run daemon-api`

- [ ] **Step 3: Implement** in `daemon-api.ts`, next to `bootstrapWorkspace`:

```ts
export interface WorkspaceInspectAgentPreview {
  name: string;
  bio?: string;
  provider: string;
  model: string;
}

export interface WorkspaceInspectFound {
  found: true;
  companyName: string;
  mission: string;
  values: string[];
  orchestrator: WorkspaceInspectAgentPreview;
  workers: WorkspaceInspectAgentPreview[];
  providerAvailable: boolean;
}

export type WorkspaceInspectResponse = { found: false } | WorkspaceInspectFound;

export interface WorkspaceResumeResponse {
  workspace: DaemonWorkspaceConfig;
  orchestrator: DaemonSnapshot;
  workers: DaemonSnapshot[];
}
```

And on the client object (mirror `bootstrapWorkspace`'s implementation; for `inspectWorkspace` use a GET with `?rootPath=${encodeURIComponent(rootPath)}` — check how existing GET methods are shaped):

```ts
inspectWorkspace(rootPath: string): Promise<WorkspaceInspectResponse>;
resumeWorkspace(rootPath: string): Promise<WorkspaceResumeResponse>;
```

(`DaemonSnapshot` = the existing agent snapshot type used by `bootstrapWorkspace`'s response; check the actual name in the file.)

- [ ] **Step 4: Run to verify pass.**

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/lib/daemon-api.ts apps/web/src/lib/daemon-api.test.ts
git commit -m "feat(web): add inspect/resume workspace client methods"
```

---

## Phase 3 — Web UI

### Task 7: `ResumeCard` component

**Files:**
- Create: `apps/web/src/components/onboarding/ResumeCard.tsx`
- Test: `apps/web/src/components/onboarding/ResumeCard.test.tsx`

- [ ] **Step 1: Write the failing tests**

```tsx
describe('ResumeCard', () => {
  it('renders company, mission, folder, and the agent roster', () => { /* company name, mission, root path (title attr), orchestrator name, both worker names */ });
  it('warns when the provider is unavailable', () => { /* providerAvailable: false → text matching /offline|not configured/i */ });
  it('omits the warning when the provider is available', () => { /* queryByText null */ });
  it('emits onResume and onSetupFresh', async () => { /* click both buttons */ });
  it('disables resume while resuming and shows errors', () => { /* resuming: true → disabled; resumeError: 'boom' → role=alert */ });
});
```

- [ ] **Step 2: Run to verify fail** — component missing.

- [ ] **Step 3: Implement** — controlled presentational component, conventions from `WorkspaceStep.tsx`/`ReviewStep.tsx` (`labelCls`, `field`, `rounded-2xl border border-line bg-white/[0.02]`, `text-ink`/`text-ink-2`/`text-ink-3`, aria-hidden decorative glyphs, `role="alert"` errors):

```tsx
import type { WorkspaceInspectFound } from '../../lib/daemon-api';

export interface ResumeCardProps {
  preview: WorkspaceInspectFound;
  rootPath: string;
  resuming: boolean;
  resumeError: string | null;
  onResume(): void;
  onSetupFresh(): void;
}
```

Structure: `<section aria-labelledby="onboarding-resume-heading">` with heading "Resume your workspace" + subcopy "We found an existing workspace here. Pick up where you left off."; a workspace block (company 🏢 aria-hidden + mission + truncated mono path with `title`); a roster block listing orchestrator first ("Main agent" tag) then workers, each `name — provider/model`; provider warning paragraph when `!preview.providerAvailable`: "The provider for these agents isn't configured on this machine — they'll resume offline until you add the key."; footer: primary "Resume workspace" button (accent styling from AgentStep's generate button, `disabled={resuming}`, label "Resuming…" while resuming) + secondary "Set up fresh instead" button (border styling); `role="alert"` error paragraph.

- [ ] **Step 4: Run to verify pass.**

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/components/onboarding/ResumeCard.tsx apps/web/src/components/onboarding/ResumeCard.test.tsx
git commit -m "feat(web): add resume card for existing workspaces"
```

---

### Task 8: WorkspaceStep "already have a workspace" affordance

**Files:**
- Modify: `apps/web/src/components/onboarding/WorkspaceStep.tsx`
- Modify: `apps/web/src/components/onboarding/WorkspaceStep.test.tsx`

- [ ] **Step 1: Write the failing tests**

```tsx
it('offers resume-with-existing mode', async () => {
  const props = setup({ onResumeModeChange: vi.fn() });
  await userEvent.click(screen.getByRole('button', { name: /already have a workspace/i }));
  expect(props.onResumeModeChange).toHaveBeenCalledWith(true);
});

it('inspects instead of verifying in resume mode', async () => {
  const props = setup({ resumeMode: true, onInspect: vi.fn() });
  await userEvent.click(screen.getByRole('button', { name: /inspect/i }));
  expect(props.onInspect).toHaveBeenCalled();
  expect(props.onVerify).not.toHaveBeenCalled();
});

it('resume mode offers a path back to fresh setup', async () => {
  const props = setup({ resumeMode: true });
  await userEvent.click(screen.getByRole('button', { name: /set up fresh|new workspace/i }));
  expect(props.onResumeModeChange).toHaveBeenCalledWith(false);
});
```

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3: Implement** — add three optional props (default false/no-op safe): `resumeMode?: boolean`, `onResumeModeChange?(mode: boolean): void`, `onInspect?(): void`. Behavior:
  - Below the Office location row, when `!resumeMode`: a quiet text button "Already have a workspace? Point to it" → `onResumeModeChange?.(true)`.
  - When `resumeMode`: the Verify button's label becomes `inspecting ? 'Inspecting…' : 'Inspect'` and its action is `onInspect` (add `inspecting?: boolean` prop or reuse `verifying` — reuse `verifying`, it already carries the disabled spinner semantics); mission/values fields stay (they'll be prefilled/overwritten by the yaml — simplest: hide them in resume mode since the yaml owns them; keep company name hidden too. Only the folder field matters in resume mode. Implement by conditionally rendering only the rootPath row + mode link.)
  - In resume mode under the folder row: "or set up a new workspace instead" text button → `onResumeModeChange?.(false)`.
- Keep all existing behavior byte-identical when the new props are omitted (existing 8 tests must pass unchanged).

- [ ] **Step 4: Run to verify pass** (focused file, then full suite).

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/components/onboarding/WorkspaceStep.tsx apps/web/src/components/onboarding/WorkspaceStep.test.tsx
git commit -m "feat(web): add existing-workspace affordance to workspace step"
```

---

### Task 9: OnboardingFlow resume wiring

**Files:**
- Modify: `apps/web/src/components/onboarding/OnboardingFlow.tsx`
- Modify: `apps/web/src/components/onboarding/OnboardingFlow.test.tsx`

- [ ] **Step 1: Write the failing tests** (append to `OnboardingFlow.test.tsx`; spy pattern already used there — `vi.spyOn(daemon, 'inspectWorkspace')` / `'resumeWorkspace'`):

1. Resume mode: clicking "Already have a workspace" → only the folder field + Inspect button remain.
2. Inspect `found: false` → inline note /no workspace/i (or the exact copy implemented), wizard unchanged.
3. Inspect `found: true` → ResumeCard renders with company/mission/roster from the preview; wizard steps hidden.
4. Resume submit → `daemon.resumeWorkspace` called once with the root path; `onCreated` called with the returned orchestrator snapshot.
5. Resume error → `role="alert"` on the card; a second submit works after fixing (or at least the error displays and state is intact).
6. "Set up fresh instead" from the card → back to the Workspace step normal flow, draft intact.
7. Stale inspect guard: start inspect with a deferred promise, edit the folder path, resolve → no card appears (mirror the existing stale-verify test).

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3: Implement** in `OnboardingFlow.tsx`:

- State: `resumeMode: boolean`, `inspectPreview: WorkspaceInspectFound | null`, `inspectNote: string | null` (for found:false / errors), `resuming: boolean`, `resumeError: string | null`, plus `inspectRequestIdRef` and `resumeInFlightRef`.
- `changeRootPath` already resets verify status — also reset `inspectPreview`/`inspectNote` and bump `inspectRequestIdRef`.
- `inspectWorkspace` action: guard empty path + in-flight; `const id = ++inspectRequestIdRef.current`; call `daemon.inspectWorkspace(draft.workspace.rootPath)`; on resolve bail if id stale or unmounted; `found:false` → `inspectNote = "No workspace file found here — set up fresh below."`; `found:true` → `inspectPreview = result`; error → `inspectNote = message`.
- Render: when `inspectPreview` is set, render `<ResumeCard>` INSTEAD of the current step body (keep the header; swap title to "Resume your workspace"), hiding Back/Next nav. `onSetupFresh` → clear `inspectPreview` + `resumeMode(false)`.
- `resumeWorkspace` action: `resumeInFlightRef` guard; `daemon.resumeWorkspace(rootPath)`; success → `mountedRef`-guarded `onCreated(response.orchestrator)`; error → `resumeError = message`; finally clear resuming (mounted-guarded).
- WorkspaceStep wiring: pass `resumeMode`, `onResumeModeChange` (entering resume mode also clears `verifyStatus`; leaving clears `inspectPreview`/`inspectNote`), `onInspect={inspectWorkspace}`.
- IMPORTANT: while `inspectPreview` is shown, `goNext`/`goBack` must be unreachable (nav hidden). Submit path (`bootstrapWorkspace`/`createAgent`) is untouched.

- [ ] **Step 4: Run to verify pass** — focused file, then full suite + typecheck.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/components/onboarding/OnboardingFlow.tsx apps/web/src/components/onboarding/OnboardingFlow.test.tsx
git commit -m "feat(web): resume an existing workspace from onboarding"
```

---

## Phase 4 — Final verification

### Task 10: Full verification

- [ ] Rust: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache` + `... rust-daemon:lint --skipNxCache` — PASS
- [ ] Web: `bun x nx test @animaOS-SWARM/web --skipNxCache` + `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache` + `bun x nx run @animaOS-SWARM/web:build --skipNxCache` — PASS
- [ ] Live E2E against a real daemon (fresh control-plane file under %TEMP%, `ANIMAOS_RS_PORT=8091` to isolate): write an `anima.yaml` (orchestrator + 1 worker) into a temp folder → `GET /api/workspace/inspect?rootPath=...` → 200 found:true with roster → `POST /api/workspace/resume` → 201 → `GET /api/agents` shows both agents → yaml byte-identical → restart daemon → agents + workspace survive. Kill the daemon and delete temp dirs afterward. (Send JSON bodies via `--data-binary @file` — Git Bash mangles inline backslash paths.)
- [ ] Docs-only commit if the plan needed amendments:

```bash
git add docs/superpowers/plans/2026-08-31-workspace-resume.md
git commit -m "docs: record workspace resume plan amendments" --allow-empty
```

---

## Out of Scope (from spec)

- Daemon startup auto-detect of `ANIMAOS_WORKSPACE_ROOT` with an existing `anima.yaml`.
- CLI adopt/import command.
- Per-worker provider availability in inspect (v1 reports the orchestrator's).
- Drift detection between yaml and persisted per-agent state.
