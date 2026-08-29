# Workspace-First Onboarding and Agency Runtime Bridge

**Date:** 2026-08-30
**Status:** Approved design
**Primary surface:** `apps/web`
**Runtime boundary:** `hosts/rust-daemon`
**Builds on:** `docs/superpowers/specs/2026-08-26-main-workspace-agent-onboarding-design.md`

## Summary

Rework the Guided Focus onboarding from "create your main agent" into "set up your workspace": the user founds a company (name, mission, values, folder on disk), picks a model, then designs their main agent by choosing a personality preset and describing what they want in plain words — the daemon's model drafts the proper bio, traits, and instructions, which stay fully editable. Submission creates the workspace config, an `agency.yaml` (single-orchestrator agency), and the orchestrator as a runtime agent in one atomic daemon transaction.

The workspace becomes a first-class daemon concept: a persisted `WorkspaceConfig` owns the root path and company identity, and `ANIMAOS_WORKSPACE_ROOT` becomes only the initial default. The agency file format becomes the durable home for team identity so that hiring worker agents later is a natural extension of the same structure.

## Goals

- Onboarding defines the workspace before the agent: company name, mission, values, and the folder the daemon uses as workspace root.
- The main agent is the orchestrator of a real single-member agency, persisted as `agency.yaml` in the workspace root.
- Users never have to write a system prompt from scratch: personality presets plus a plain-language intent field feed a daemon-side profile generator; output is editable.
- Personality fields (`bio`, `adjectives`, `style`) flow end to end into `anima-core`'s existing `AgentConfig` — no core schema changes.
- One atomic bootstrap: any failure leaves no partial workspace config, no `agency.yaml`, and no agent.
- Existing chat, settings, check-ins, provider configuration, and the oldest-agent main rule keep working.

## Non-goals

- Hiring or generating worker agents during onboarding (fast-follow feature; the agency file structure is designed for it).
- Changing `team_size` semantics or the CLI agency generator.
- Interactive per-tool-call approval prompts.
- API-key entry in the browser.
- Multiple workspaces per daemon or workspace switching.
- Migrating pre-existing daemon agents into agencies.

## Onboarding Experience

The flow replaces the application shell while no agent exists, as today. The step order changes from Identity → Intelligence → Access → Review to:

### Step 1: Workspace (new)

- **Company name** (required). This becomes the agency name.
- **Mission** (required, one sentence). Stored in workspace config and `agency.yaml`; injected into the profile-generation prompt.
- **Office location** (required): absolute folder path on the daemon host. A Verify action (via `PUT /api/workspace` in validate-only mode) canonicalizes the path, confirms it exists or can be created, and reports which. On submit the folder is created if missing.
- **No partial persistence:** onboarding never calls the persisting form of `PUT /api/workspace`. Workspace config is only persisted inside the bootstrap transaction, so abandoning the flow midway leaves no daemon-side state.
- **Values** (optional, 3–5 short chips). Stored in workspace config and `agency.yaml`; injected into the profile-generation prompt.

### Step 2: Intelligence (existing `ModelStep`, moved earlier)

Unchanged in behavior: configured provider cards, model suggestions, custom model, retry. It now precedes the Agent step so profile generation uses the exact provider and model the agent will run on.

### Step 3: Agent (replaces `IdentityStep`)

- **Name** (required, defaults to `Anima`).
- **Personality preset** (required selection, default `chief-of-staff`): one of four cards — Chief of Staff, Calm Assistant, Senior Engineer, Creative Partner. Each preset ships a template profile (bio, adjectives, style, system-prompt template with workspace placeholders) in `apps/web/src/lib/agent-presets.ts`.
- **Intent** (free text): plain-language "what do you want this agent to do for you?" Rough wording is fine. Required to enable Generate, but not required to complete the step.
- **✨ Generate profile**: calls `POST /api/agents/generate-profile` with preset, intent, and the chosen provider/model. Returns `{ bio, adjectives, style, system }` with the workspace mission and values baked in. All returned fields render as editable inputs. Regenerate is available.
- **Fallback:** if the selected provider cannot generate (only the deterministic provider is configured, or generation fails), the preset's template profile fills the fields with the workspace name/mission substituted, and the step remains completable without generation.
- **Step requirement:** name, preset, and a non-empty `system` — sourced from generation, the preset template, or manual edits.

### Step 4: Access (existing `AccessStep`)

Unchanged: Observe / Collaborate (default) / Operate tool profiles from the prior design.

### Step 5: Review (upgraded `ReviewStep`)

- Summarizes workspace (company, mission, folder) and agent (name, preset, provider/model, access profile, bio preview).
- States the atomicity promise: one validated create; failure creates nothing.
- Submits a single `POST /api/workspace/bootstrap` request.
- Keeps the complete draft intact on failure, as today.

The draft is held in onboarding component state for the page session. Successful creation clears it.

## Daemon Changes

### Workspace config and persistence

- Add `WorkspaceConfig { root_path, company_name, mission, values: Vec<String> }` to the daemon control-plane snapshot.
- Bump the control-plane store version (3 → 4) with a migration: snapshots without a workspace config load as "not configured."
- `ANIMAOS_WORKSPACE_ROOT` becomes the initial default for the root path. When no workspace config exists, behavior is exactly as today (env var, then daemon current directory).
- Workspace tools resolve the root from daemon state (`WorkspaceConfig.root_path` when configured) instead of re-reading the environment per call. Canonicalization and escape checks (`ensure_path_within_workspace`) are unchanged.

### Endpoints

- `GET /api/workspace` — returns the workspace config plus a `configured: bool`. When unconfigured, also returns the effective default root (env var or launch directory) so onboarding can pre-fill the folder field.
- `PUT /api/workspace` — body `{ rootPath, companyName, mission, values }`. Validates the path is absolute and exists or is creatable, canonicalizes, creates the folder if missing, persists via the control-plane transaction path. A `validateOnly: true` mode performs validation without persisting (powers the Verify action).
- `POST /api/agents/generate-profile` — body `{ presetId, intent, provider, model }`. Builds a structured-output prompt embedding the workspace mission and values and the preset's style guidance; returns `{ bio, adjectives, style, system }`. Reuses the agency-generator's model-call pattern (`ModelGenerateRequest`, JSON-only system prompt). Unknown `presetId` → stable bad request. Unconfigured/deterministic provider → stable error the web app translates into the template-fallback path.
- `POST /api/workspace/bootstrap` — body `{ workspace: {...}, agent: { name, presetId, bio, adjectives, style, system, provider, model, tools } }`. One control-plane transaction:
  1. Validate workspace fields and agent fields (name non-empty, model non-empty, known tool slugs resolved to canonical registry descriptors as in the existing create path).
  2. Validate/canonicalize/create the root folder.
  3. Write `agency.yaml` at the root: single-orchestrator agency with company name, mission, values, strategy `supervisor`, and the orchestrator definition (name, bio, adjectives, style, system, model, tools).
  4. Create the runtime agent with the full personality fields on `AgentConfig` (`bio`, `adjectives`, `style`, `system`) plus the resolved canonical tool descriptors.
  5. Persist workspace config + agent in the control-plane snapshot.
  Any step failing rolls back all durable state: no `agency.yaml`, no agent, no workspace config. The only permitted side effect of a failed bootstrap is the empty root folder itself if validation created it. Returns `{ workspace, agent }` (the created snapshot) on success.
- Existing `POST /api/agents` and PATCH paths are untouched; bootstrap composes their validation and tool-resolution internals.

### Personality on runtime agents

`AgentConfig` in `anima-core` already carries `bio`, `lore`, `knowledge`, `topics`, `adjectives`, `style`. Bootstrap sets `bio`, `adjectives`, `style`, and `system`; `lore`/`knowledge`/`topics` remain unset for now. The agent view model exposes the new fields so the web app can display them.

## Web Changes

- `OnboardingFlow`: five steps with the new order; per-step validation unchanged in spirit (stay on step, focus first invalid field).
- New components: `WorkspaceStep`, `AgentStep` (preset cards, intent field, generate/regenerate, editable generated fields). `IdentityStep` is removed; `ModelStep`, `AccessStep`, `ReviewStep` are adjusted for order and summary content.
- New lib modules: `agent-presets.ts` (four presets with template profiles) and workspace client methods (`getWorkspace`, `putWorkspace`, `generateProfile`, `bootstrapWorkspace`) in the daemon API client.
- The personality preset selected determines the fallback template and the style guidance sent to the generator.
- After onboarding, the workspace shell header shows the company name; the mission appears in the workspace view. Settings gains a read-only workspace section (editing workspace config post-onboarding is a follow-up).
- `selectMainAgent()` and the oldest-agent rule are unchanged; the orchestrator is simply the first agent.

## Error Handling

- Folder invalid, not absolute, or uncreatable: stay on Workspace step, inline error, focus path field.
- Provider catalog unavailable: blocks Intelligence only, as today; Workspace draft is preserved.
- Generate-profile failure or unavailable provider: keep preset template content in the fields, show a non-blocking notice, allow continuing or retrying.
- Bootstrap failure: stay on Review with the full draft intact; nothing is created daemon-side (verified by `GET /api/workspace` + agent list remaining empty).
- Bootstrap succeeds but later polling fails: keep the created agent in the workspace and show offline status, as today.
- Concurrent agent appearance: oldest-agent main rule keeps the behavior stable.

## Testing Strategy

### Rust tests

- Control-plane store v3 → v4 migration: old snapshots load with no workspace config.
- Root resolution precedence: configured root > env var > current directory; tool escape checks still bind to the configured root.
- `PUT /api/workspace`: validates relative paths (reject), uncreatable paths (reject), creates missing folders, `validateOnly` does not persist.
- `generate-profile`: unknown preset rejected; deterministic provider returns the stable fallback error; a scripted adapter returns structured fields.
- `bootstrap`: success writes `agency.yaml` (parse and assert orchestrator fields), creates the agent with canonical tool descriptors, and persists workspace config; forced failure at agent creation leaves no `agency.yaml`, no config, and no agent; unknown tool slugs rejected before any side effect.
- Existing agency, agent, and tool tests remain green.

### Web tests

- Five-step order and validation; Workspace step blocks on empty name/mission/folder.
- Verify action success/failure states.
- Preset selection fills template fields; Generate replaces them; Regenerate issues a new request; fields remain editable after generation.
- Deterministic-only provider: generate control shows fallback notice, step completable.
- Bootstrap failure preserves the entire draft; success transitions with the POST response.
- Review renders workspace + agent summary.

### Workspace verification

- `bun x nx run rust-daemon:test --skipNxCache` (with `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon` on Windows when the daemon binary is locked).
- Web `test`, `typecheck`, and `build` targets through Nx.
- Browser checks: fresh daemon (no config), onboarding end to end against the deterministic provider (fallback path) and a configured provider (generation path), offline mid-flow, small-screen layout, keyboard navigation.

## Acceptance Criteria

- With a fresh daemon and zero agents, onboarding opens with the Workspace step first.
- The user can complete Workspace → Intelligence → Agent → Access → Review and create exactly one orchestrator agent plus a workspace config and `agency.yaml` in one atomic request.
- A failed bootstrap leaves no workspace config, no `agency.yaml`, and no agent.
- The generated profile reflects the workspace mission and values; all generated fields are editable before creation.
- With only the deterministic provider, onboarding completes via preset templates without generation.
- The created agent's `AgentConfig` carries `bio`, `adjectives`, `style`, and `system`; its tools are the exact profile list with canonical descriptors.
- Workspace tools operate under the configured root after bootstrap; `ANIMAOS_WORKSPACE_ROOT` still works as the pre-config default.
- Existing agents, chat, check-ins, and settings continue to work; the oldest agent remains main.
