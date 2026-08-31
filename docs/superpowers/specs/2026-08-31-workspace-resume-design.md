# Workspace Resume Design

**Date:** 2026-08-31
**Status:** Approved design (pre-plan)
**Builds on:** `2026-08-30-workspace-first-onboarding-design.md`

## Problem

The workspace-first onboarding flow assumes a fresh workspace. A user who already has one — a folder containing an `anima.yaml` written by a previous install, another machine, or a prior bootstrap — currently hits a wall:

- `POST /api/workspace/bootstrap` returns 409 when `anima.yaml` already exists at the chosen root (`hosts/rust-daemon/src/routes/workspace.rs:89-95`).
- There is no path to adopt an existing workspace: the user must either pick a different folder (abandoning their setup) or re-create everything from scratch (and still be blocked by the existing file).

The `anima.yaml` at the workspace root is already the durable source of truth: bootstrap writes it with the company settings, orchestrator profile, and (via the `agents:` array the CLI loader already supports, `packages/cli/src/agency/loader.ts:73-86`) worker agents. Resume makes the daemon trust that file.

## Decisions (from brainstorming)

- **Entry point:** the onboarding Workspace step (step 0) — an "Already have a workspace?" affordance. Not daemon startup auto-detect, not a CLI command (both remain possible follow-ups).
- **Restore scope:** everything in the file — workspace config, orchestrator, and all workers in `agents:`. The yaml is the full source of truth for the workspace.
- **Unavailable provider/model:** resume anyway and warn visibly. The agent restores even if its provider isn't configured on this machine; the console surfaces a clear warning. No forced re-pick of provider/model.
- **Mechanics:** two new endpoints — read-only inspect + atomic resume (Approach 1). Bootstrap is not overloaded; the web client never parses yaml itself.

## Design

### 1. Daemon: yaml parsing (Rust)

`AgencyYamlConfig` / `AgencyYamlAgent` in `hosts/rust-daemon/src/routes/agencies.rs` currently derive `Serialize` only. Add `Deserialize` (`serde_yaml` is already a dependency, `hosts/rust-daemon/Cargo.toml:28`).

Add one shared helper, e.g. `load_agency_yaml(path: &Path) -> Result<AgencyYamlConfig, ApiError>`:

- Reads and parses the file.
- Validates the same invariants the CLI loader enforces: truthy `name`; `orchestrator` present with truthy `name`, `bio`, `system`; `agents` defaults to `[]`.
- Returns 400-mapped errors with clear messages for: file missing, unreadable, invalid YAML, missing required fields.

This is the single parse path reused by both new endpoints. The daemon never writes the yaml during resume.

### 2. Daemon: `GET /api/workspace/inspect?rootPath=...`

Read-only; never mutates disk or control-plane state. Follows the existing route-handler pattern (`*_entry` wrapper + `handle_*` in a routes submodule + `#[utoipa::path]` + OpenAPI registration).

Response:

- Folder missing, or no `anima.yaml` inside → `200 { "found": false }`. The UI treats this as "no workspace here" and continues normal onboarding.
- Valid file → `200`:

```json
{
  "found": true,
  "companyName": "Northwind Research",
  "mission": "Continuous equity research",
  "values": ["cite sources"],
  "orchestrator": { "name": "Anima", "bio": "...", "provider": "moonshot", "model": "kimi-k2" },
  "workers": [{ "name": "Scout", "provider": "moonshot", "model": "kimi-k2" }],
  "providerAvailable": true
}
```

- File exists but invalid/unparseable/missing required fields → `400` with a clear message.

`providerAvailable` reflects whether the orchestrator's provider is configured on this machine (same source as the providers catalog). It is advisory: it powers the UI warning only and never blocks resume. (If providers differ per worker, per-agent availability may be added later; v1 reports the orchestrator's.)

Contracts live in `hosts/rust-daemon/src/contracts/workspace.rs`, camelCase serde, matching the existing workspace contracts.

### 3. Daemon: `POST /api/workspace/resume {rootPath}`

Adoption, modeled on bootstrap's discipline:

1. **Validate everything before side effects.** Parse + validate the yaml (shared helper). Resolve and validate all agents' tools — unknown tools rejected pre-mutation, same as bootstrap's `create_agent` → `resolve_agent_tools` path.
2. **Conflict rules:**
   - Workspace configured for a *different* root → `409` naming the configured root.
   - Workspace configured for the *same* root → idempotent re-resume: skip agents whose names already exist, restore missing ones. This doubles as the recovery path after deleting the last agent (closing the seam the onboarding fix `11a0bd2` left: the createAgent re-hire path restores only the orchestrator, without its bio/adjectives/style — resume restores the full profile from the yaml).
   - Not configured → fresh adopt.
3. **Commit inside one control-plane transaction:** set workspace config from the yaml (company name, mission, values, root path), create the orchestrator, create each worker, persist.
4. **Rollback:** any failure after the first mutation (agent creation failure, persist failure) → full rollback: created agents removed, workspace config cleared, best-effort rollback persist — same shared rollback helper pattern as bootstrap. Nothing is half-adopted.
5. Success → `201 { workspace, orchestrator, workers }` (web client types mirror bootstrap's response shape plus `workers`).

The yaml file itself is never modified, renamed, or deleted by resume.

### 4. Web: Workspace step resume path

- Step 0 (`WorkspaceStep`) gains a quiet "Already have a workspace? Point to it" toggle/link below the folder field.
- Activating it swaps the folder Verify affordance for **Inspect**: calls `GET /api/workspace/inspect?rootPath=...` (with the same request-id/stale-response discipline the verify action uses).
- `found: false` → inline neutral note ("No workspace file found here — continue below to set up fresh") and the normal flow is unchanged.
- `found: true` → the wizard collapses to a **resume card** replacing the step body:
  - Company name + mission + workspace folder (truncated, `title` for full path — same pattern as ReviewStep).
  - Agent roster: orchestrator and workers with provider/model; warning icon + copy "provider not configured on this machine — agent will resume offline" when `providerAvailable` is false.
  - Primary button "Resume workspace" → `POST /api/workspace/resume` → `onCreated(orchestrator)` → console.
  - Secondary "Set up fresh instead" link → returns to the normal onboarding flow (draft preserved).
- `400` from inspect → inline error, nothing else changes.

Header copy when the resume card is showing: keep the kicker; title becomes "Resume your workspace" (or equivalent) so the mode change is explicit.

### 5. Error handling

| Case | Behavior |
|---|---|
| Folder missing / no `anima.yaml` | inspect `found: false`; normal onboarding continues |
| Malformed or incomplete yaml | inspect 400; inline error; no disk/state mutation |
| Resume on different configured root | 409; inline conflict naming the configured root |
| Unknown tool in any yaml agent | 400 pre-mutation; no side effects |
| Resume failure mid-adopt | full rollback; error shown on the resume card; folder + store untouched |
| Daemon restart after resume | normal startup restore (agents now in the control-plane store) |

### 6. Testing

**Rust integration tests** (`hosts/rust-daemon/tests/`):

- inspect: `found: false` for missing folder / missing file; valid file returns the full preview; malformed yaml → 400; missing orchestrator fields → 400.
- resume: written yaml (orchestrator + 2 workers) → 201, agents listed via `GET /api/agents`, workspace configured, yaml file untouched.
- resume with unknown tool → 400, zero side effects.
- 409 on different configured root.
- Idempotent same-root re-resume: delete one worker (or start from configured-but-zero-agents), resume → only missing agents restored.
- Rollback on forced persist failure; store ends clean.
- Restart persistence: resume, restart daemon with same control-plane file, agents + workspace survive.

**Web tests:**

- WorkspaceStep renders the resume affordance; inspect `found: false` → neutral note + normal flow intact.
- inspect `found: true` → resume card with company/mission/roster; provider warning shown when `providerAvailable: false`.
- Resume submits once (in-flight guard), calls `onCreated` with the orchestrator.
- inspect 400 → inline error.
- "Set up fresh instead" returns to the normal flow with the draft intact.

**Fixtures:** one valid `anima.yaml` (orchestrator + 2 workers) and one malformed file, shared where practical.

## Out of scope (documented follow-ups)

- Daemon startup auto-detect of `ANIMAOS_WORKSPACE_ROOT` pointing at an `anima.yaml` folder.
- CLI `adopt`/`import` command.
- Editing workspace config after adoption (Settings remains read-only).
- Per-worker provider availability in inspect (v1 reports the orchestrator's).
- Merging/reconciling a yaml whose contents conflict with already-persisted per-agent state (resume restores from the file; drift detection is a follow-up).
