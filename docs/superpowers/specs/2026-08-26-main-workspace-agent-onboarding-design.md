# Main Workspace Agent Onboarding and Spatial Web UI

**Date:** 2026-08-26  
**Status:** Approved design  
**Primary surface:** `apps/web`  
**Runtime boundary:** `hosts/rust-daemon`

## Summary

Modernize the animaOS web console around one persistent main workspace agent. When the daemon has no agents, the app shows a four-step Guided Focus onboarding flow. The flow creates the first agent, chooses its model provider, and assigns an enforced workspace-tool profile. After creation, the app transitions into a calm, agent-first workspace using the Spatial Intelligence design direction with Neon Rose as the accent.

The daemon remains multi-agent. In this phase, the web app treats the oldest persisted agent as the main workspace agent and makes that rule explicit in one selector. The Agents surface can show additional existing agents, but creating or coordinating specialist agents is out of scope.

## Goals

- Give a first-time user a deliberate onboarding experience when no daemon agent exists.
- Create one main agent that can work within the configured daemon workspace root.
- Let the user choose an enforced workspace-access profile during onboarding.
- Replace the heavy sidebar and blue-purple aurora styling with a quieter, more modern agent-first shell.
- Preserve existing chat, settings, proactive check-ins, provider configuration, control-plane persistence, and automatic runtime memory behavior.
- Establish clear frontend boundaries for additional specialist agents later.

## Non-goals

- Memory onboarding, memory settings, or a new memory-management UI.
- Creating specialist agents or adding swarm orchestration to the web app.
- Adding an explicit `main` role to the daemon's persisted agent schema.
- Interactive approval prompts for individual tool calls.
- API-key entry or secret storage in the browser.
- A new runtime-activity API or full event timeline.
- Reworking the daemon's memory engine, model adapters, or workspace-root security.

## Main Agent Rule

The daemon already returns agents ordered by `created_at_ms`, then ID. The web app will select the first snapshot in that order as the main workspace agent.

- Zero agents: enter onboarding.
- One agent: that agent is main.
- Multiple agents: the oldest agent is main; all agents appear in the Agents surface.
- Deleting the main agent: the next oldest agent becomes main. Onboarding returns only when no agents remain.

This rule lives in a pure `selectMainAgent()` helper rather than being repeated across components. A future explicit daemon role can replace the helper without restructuring the shell.

## Onboarding Experience

The onboarding flow replaces the normal application shell while no agent exists. The previous behavior—rendering a sidebar next to a setup card—is removed.

### Step 1: Identity

- Agent name, defaulting to `Anima`.
- Optional operating instructions stored as the agent system prompt.
- Short copy explaining that this will be the main agent for the current daemon workspace.

### Step 2: Intelligence

- Provider cards sourced from `GET /api/providers`.
- Configured providers are selectable.
- Providers that require unavailable daemon configuration remain visible but disabled, with a concise setup explanation.
- Model choices use the existing provider suggestions and retain a custom model option.
- API keys never enter browser state.

### Step 3: Workspace Access

The user chooses one explicit tool profile. The chosen profile is sent as the agent's `tools` list and validated by the daemon.

All profiles include the existing memory and utility tools so workspace access does not silently disable normal agent memory behavior:

- `memory_search`
- `memory_add`
- `recent_memories`
- `get_current_time`
- `calculate`

Profiles then add these workspace capabilities:

#### Observe

- `read_file`
- `list_dir`
- `glob`
- `grep`
- `todo_read`

Observe can inspect the workspace but cannot write files or execute processes.

#### Collaborate (default)

Everything in Observe, plus:

- `write_file`
- `edit_file`
- `multi_edit`
- `todo_write`

Collaborate can change workspace files and maintain workspace todos, but cannot execute shell or background processes.

#### Operate

Everything in Collaborate, plus:

- `bash`
- `bg_start`
- `bg_output`
- `bg_stop`
- `bg_list`

Operate grants the daemon's full current workspace process surface. The UI states this clearly. These tools remain bounded by `ANIMAOS_WORKSPACE_ROOT` and existing daemon path checks.

Swarm-only tools and web tools are not included. Interactive per-command approval is not implied because the current blocking agent-run API does not expose that workflow.

### Step 4: Review

- Summarize agent name, provider/model, and access profile.
- Explain the highest-risk capability in the selected profile.
- Submit one `POST /api/agents` request.
- Keep the complete draft intact if creation fails.
- Transition using the created snapshot returned by the POST instead of requiring an immediate second fetch.

The draft is held in onboarding component state for the current page session. Successful creation clears it.

## Runtime and API Changes

### Agent creation

Extend the TypeScript daemon client so `createAgent` accepts `tools: string[]`. The Rust create contract already accepts tool names and validates them through the host tool registry.

### Agent updates

Users must be able to change access later without deleting the agent or losing its conversation. Extend PATCH support end to end:

- Add optional tools to `AgentConfigUpdate` in `anima-core`.
- Add optional tools to the daemon `AgentUpdateRequest` contract.
- Validate patched tool descriptors against `ToolRegistry` before mutating the runtime.
- Persist the updated agent config through the existing control-plane snapshot path.
- Return the updated snapshot in the existing agent envelope.
- Extend the TypeScript client and settings form to submit and derive access profiles.

Unknown tools return a stable bad-request response. A missing agent remains a not-found response. An invalid patch must not partially mutate the runtime.

### Agent view model

Expose the agent's configured tool names in the web view model. Derive Observe, Collaborate, or Operate only when the set exactly matches a known profile. If a pre-existing agent has a custom tool set, show `Custom access` and preserve it until the user deliberately selects a profile.

## Frontend Structure

### Application state

`ViewHarness` currently owns bootstrap, setup, chat, settings, and check-in orchestration. The redesign splits presentation from orchestration while preserving the existing daemon-backed behavior.

- `selectMainAgent()` selects the main snapshot.
- `useDaemonBootstrap()` owns health, provider, and agent loading plus polling.
- `OnboardingFlow` owns step navigation and draft state.
- `WorkspaceShell` owns responsive navigation and application chrome.
- Existing chat and check-in orchestration remains in a focused workspace controller during this phase.

### Onboarding components

- `OnboardingFlow`
- `IdentityStep`
- `ModelStep`
- `AccessStep`
- `ReviewStep`
- `OnboardingProgress`

Each step receives typed values and callbacks. Network submission belongs to the flow controller, not to individual step components.

### Workspace components

- `WorkspaceShell`: top navigation on desktop, bottom navigation on small screens.
- `AgentPresence`: orb, greeting, runtime status, and access badge.
- Existing `MessageList` and `Composer`: restyled inside the centered workspace canvas.
- `ActivityView`: preserves proactive check-ins and adds only summary information already available from the current agent snapshot.
- `AgentsView`: lists all daemon agents, badges the main agent, and presents additional agents as read-only entries for now.
- `SettingsPanel`: keeps identity/model editing and adds access-profile editing.

The shell has three destinations: Workspace, Activity, and Agents. Settings remains a contextual panel opened from the main-agent status control.

## Data Flow

1. On mount, request daemon health, agents, and providers.
2. Health begins as unknown; the UI must not claim the daemon is connected until a request succeeds.
3. If agents are empty, render `OnboardingFlow` without workspace navigation.
4. If agents exist, select the oldest as main and render `WorkspaceShell`.
5. On onboarding submit, map the selected access profile to its exact tool list and call `POST /api/agents`.
6. On success, adapt the returned snapshot and transition directly into Workspace.
7. Polling refreshes the agent collection and preserves the same main-agent selector rule.
8. When access changes in Settings, PATCH the exact tool list, refresh the returned view model, and preserve conversation history.

## Error Handling

- Bootstrap state is neutral while health is unknown.
- Daemon offline: show a focused offline state with retry and the supported daemon start command.
- Provider catalog unavailable: block the Intelligence step only and provide retry; do not discard Identity values.
- Unconfigured provider: disabled selection with daemon-environment guidance.
- Local validation: keep the user on the current step and focus the first invalid field.
- Creation failure: remain on Review and preserve all choices.
- Creation succeeds but later polling fails: keep the returned agent in the workspace and display offline status without returning to onboarding.
- Update failure: keep the settings panel open, retain unsaved values, and leave the agent's previous tool list intact.
- Concurrent appearance of additional agents: keep the oldest-agent main rule stable.

## Visual System

### Direction

Use the approved Spatial Intelligence direction: calm, cinematic, dimensional, and centered on the agent rather than on dashboard chrome.

### Color

- Base: near-black `#090A0F`.
- Surface: graphite `#17171D`.
- Primary accent: Neon Rose `#FF397F`.
- Healthy status: signal mint `#64DFAD`.
- Primary text: warm near-white.
- Secondary text and borders: neutral gray without purple tint.

Neon Rose is reserved for the agent presence, primary actions, keyboard focus, selected states, and restrained ambient glow. It is not used as a large flat background.

### Layout

- Remove the permanent wide sidebar.
- Use a slim translucent top shell on desktop.
- Center Workspace, Activity, and Agents navigation in the top shell.
- Move primary navigation to a bottom dock on small screens.
- Keep conversation content within a readable centered column.
- Use the agent orb as a stateful presence, not decoration on every screen.

### Motion and accessibility

- Use slow ambient glow and short state transitions.
- Avoid high-frequency pulsing and excessive continuous motion.
- Honor `prefers-reduced-motion` with static depth and near-instant transitions.
- Provide visible keyboard focus, semantic labels, step announcements, and sufficient contrast.
- Do not rely on color alone for daemon, agent, or permission status.

No generated bitmap asset is needed; the orb, glow, iconography, and depth are code-native visual elements.

## Responsive Behavior

- Onboarding uses a centered card on desktop and a full-width padded panel on small screens.
- The top navigation becomes a bottom dock on small screens.
- Long model/provider identifiers truncate with an accessible title.
- Status copy collapses before primary actions do.
- The composer stays reachable above mobile browser chrome and the on-screen keyboard.
- Settings remains a right drawer on desktop and a full-screen sheet on small screens.

## Testing Strategy

### Web unit and component tests

- Exact tool lists for Observe, Collaborate, and Operate.
- Exact-match profile derivation and custom-tool fallback.
- Oldest-agent main selection and empty-agent behavior.
- Step navigation and validation.
- Disabled unconfigured providers.
- Failed provider retry without losing prior values.
- Failed create retaining the Review draft.
- Successful create transitioning with the POST response.
- Initial health state never rendering a false connected indicator.
- Settings access changes and failed update preservation.

### Rust tests

- Agent config update applies tools and preserves other config fields.
- PATCH accepts known tool lists.
- PATCH rejects unknown tools before mutation.
- Updated tools survive control-plane snapshot persistence and restoration.
- Existing create, run, and update behavior remains intact.

### Workspace verification

- Run the focused web tests through the web Nx target.
- Run the web typecheck and production build through Nx.
- Run `bun x nx run rust-daemon:test --skipNxCache` for Rust host/core changes.
- Run focused browser checks against no-agent, offline, configured-agent, and settings-update states.
- Check desktop and small-screen layouts, keyboard navigation, focus visibility, reduced motion, and contrast.

## Acceptance Criteria

- With an online daemon and zero agents, the web app opens Guided Focus onboarding without the normal workspace shell.
- The user can complete Identity, Intelligence, Access, and Review and create exactly one agent.
- The created agent receives the exact tools for the selected profile.
- The daemon rejects unknown tools without partially changing an agent.
- A created or restored oldest agent opens as the main workspace agent.
- Existing additional agents appear in Agents without changing the main chat target.
- The user can change the main agent's access profile in Settings without losing messages.
- Offline and loading states never falsely report a connected daemon.
- Existing chat and proactive check-ins continue to work.
- The final UI uses the Spatial Intelligence layout and Neon Rose accent with no purple accent tokens.
- The experience remains usable on desktop and small screens, with keyboard navigation and reduced-motion support.
