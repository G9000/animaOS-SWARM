# Main Workspace Agent Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current single setup card/sidebar experience with Guided Focus onboarding and a Neon Rose Spatial Intelligence workspace centered on the oldest persisted daemon agent, including enforced and editable workspace-access profiles.

**Architecture:** Keep the daemon multi-agent and make the oldest-agent rule a pure frontend selector. Move daemon bootstrap/polling into a hook, let onboarding own its transient draft, and keep chat/check-in orchestration in the workspace controller. Make the Rust `ToolRegistry` the source of truth for both executable handlers and canonical model-facing descriptors so browser-supplied tool slugs are safely expanded on create and PATCH before runtime mutation or persistence.

**Tech Stack:** React 19, TypeScript, Vite, Vitest + Testing Library, Tailwind/CSS, Rust, Axum, Tokio, Nx, Bun.

---

## Task 1: Establish the web test harness and permission-domain primitives

**Files:**
- Modify: `apps/web/package.json`
- Modify: `apps/web/vitest.config.mts`
- Create: `apps/web/src/test/setup.ts`
- Create: `apps/web/src/lib/agent-access.ts`
- Create: `apps/web/src/lib/agent-access.test.ts`

- [ ] **Step 1: Add the focused frontend test dependencies**

Run from the workspace root:

```powershell
bun --cwd apps/web add --dev @testing-library/jest-dom @testing-library/react @testing-library/user-event
```

Expected: `apps/web/package.json` and `bun.lock` add the Testing Library packages without changing runtime dependencies.

- [ ] **Step 2: Configure Vitest for DOM component tests**

Set `environment: 'jsdom'` and `setupFiles: ['./src/test/setup.ts']` in `apps/web/vitest.config.mts`. In the setup file import `@testing-library/jest-dom/vitest` and call `afterEach(cleanup)` from Testing Library.

- [ ] **Step 3: Write failing tests for access profiles and the main-agent selector**

Cover these exact expectations in `agent-access.test.ts`:

```ts
expect(toolNamesForProfile('observe')).toEqual([
  'memory_search', 'memory_add', 'recent_memories', 'get_current_time', 'calculate',
  'read_file', 'list_dir', 'glob', 'grep', 'todo_read',
]);
expect(toolNamesForProfile('collaborate')).toEqual([
  ...toolNamesForProfile('observe'),
  'write_file', 'edit_file', 'multi_edit', 'todo_write',
]);
expect(toolNamesForProfile('operate')).toEqual([
  ...toolNamesForProfile('collaborate'),
  'bash', 'bg_start', 'bg_output', 'bg_stop', 'bg_list',
]);
expect(deriveAccessProfile([...toolNamesForProfile('observe')].reverse())).toBe('observe');
expect(deriveAccessProfile(['read_file'])).toBe('custom');
expect(selectMainAgent([])).toBeNull();
expect(selectMainAgent([newer, oldest])).toBe(oldest);
```

- [ ] **Step 4: Run the tests to verify the red state**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache
```

Expected: FAIL because `agent-access.ts` does not exist or does not export the required helpers.

- [ ] **Step 5: Implement immutable profiles and one selection rule**

Export:

```ts
export type AccessProfile = 'observe' | 'collaborate' | 'operate';
export type DerivedAccessProfile = AccessProfile | 'custom';
export const ACCESS_PROFILES = { /* label, summary, risk, tools */ } as const;
export function toolNamesForProfile(profile: AccessProfile): string[];
export function deriveAccessProfile(toolNames: readonly string[]): DerivedAccessProfile;
export function selectMainAgent(agents: readonly AgentDetail[]): AgentDetail | null;
```

`selectMainAgent` operates only on the adapted frontend shape (`AgentDetail.id` and `AgentDetail.created_at_ms`). Compare profile tools as sets for derivation, reject duplicates as custom, and sort a copy by `created_at_ms` then `id` so the helper remains correct even if a caller supplies unsorted adapted agents. Wire snapshots remain confined to the daemon client/bootstrap hook and use `snapshot.state.id` plus `snapshot.state.createdAtMs`.

- [ ] **Step 6: Run tests and commit**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache
git add apps/web/package.json apps/web/vitest.config.mts apps/web/src/test/setup.ts apps/web/src/lib/agent-access.ts apps/web/src/lib/agent-access.test.ts bun.lock
git commit -m "test(web): define workspace access profiles"
```

Expected: PASS.

## Task 2: Make the Rust tool registry own canonical descriptors

**Files:**
- Modify: `hosts/rust-daemon/src/tools.rs`
- Modify: `hosts/rust-daemon/src/tools/tests.rs`
- Modify: `hosts/rust-daemon/src/state/swarm_runtime.rs`

- [ ] **Step 1: Write failing registry tests**

Add tests proving that:

- `ToolRegistry::resolve_descriptors(["read_file", "write_file", "bash"])` returns the same order requested.
- Each returned descriptor has a non-empty description.
- `read_file` requires `file_path`, `write_file` requires `file_path` and `content`, and `bash` requires `command` in its JSON-style parameter schema.
- Unknown names produce `unknown tool '...'` without returning partial descriptors.
- `send_message` and `broadcast_message` use the same canonical descriptors already expected by the swarm runtime.

- [ ] **Step 2: Run the focused Rust test to verify failure**

```powershell
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; cargo test -p anima-daemon tools::tests::registry_resolves_canonical_descriptors -- --exact
```

Expected: FAIL because the registry currently stores handlers only.

- [ ] **Step 3: Replace handler-only registration with canonical definitions**

Store a registration containing both `ToolDescriptor` and `ToolHandler`. Add small schema helpers for object, string, integer, boolean, array, enum, optional properties, and required property lists so every existing registered tool has a usable canonical definition:

- memory: `memory_search`, `memory_add`, `recent_memories`
- web: `web_fetch`, `exa_search`
- utility: `get_current_time`, `calculate`
- filesystem: `read_file`, `list_dir`, `glob`, `grep`, `write_file`, `edit_file`, `multi_edit`
- todos: `todo_write`, `todo_read`
- process: `bash`, `bg_start`, `bg_output`, `bg_stop`, `bg_list`
- swarm: `send_message`, `broadcast_message`

Keep handler lookup behavior unchanged. Add `descriptor(name)` and `resolve_descriptors(names)` APIs. Resolution must clone daemon-owned descriptors and return an error on the first unknown slug.

- [ ] **Step 4: Consolidate swarm descriptors**

Have `state/swarm_runtime.rs` request the canonical swarm descriptors from the registry rather than maintaining separate schemas. This prevents drift between auto-injected swarm tools and request-resolved tools.

- [ ] **Step 5: Run focused and module tests**

```powershell
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; cargo test -p anima-daemon tools::tests -- --nocapture
```

Expected: PASS with existing handler tests unchanged.

- [ ] **Step 6: Commit**

```powershell
git add hosts/rust-daemon/src/tools.rs hosts/rust-daemon/src/tools/tests.rs hosts/rust-daemon/src/state/swarm_runtime.rs
git commit -m "feat(daemon): canonicalize registered tool descriptors"
```

## Task 3: Canonicalize agent tools on create and update

**Files:**
- Modify: `packages/core-rust/crates/anima-core/src/agent.rs`
- Modify: `packages/core-rust/crates/anima-core/src/runtime.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/agents.rs`
- Modify: `hosts/rust-daemon/src/routes/agents.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Modify: `hosts/rust-daemon/src/state.rs`
- Modify: `hosts/rust-daemon/tests/agent_api.rs`

- [ ] **Step 1: Add failing API tests for canonical create, transactional PATCH, and restoration**

Add integration tests that send name-only strings:

```json
{"name":"Anima","model":"deterministic","tools":["read_file","write_file","bash"]}
```

Assert the response snapshot stores all three tools with non-empty descriptions and required schemas. Then PATCH an existing agent with `{"tools":["read_file","grep"]}` and assert name, model, provider, system, and messages remain unchanged while canonical tools change.

Add a second PATCH with `{"name":"must-not-stick","tools":["not_registered"]}`. Assert HTTP 400 and fetch the agent again to prove neither name nor tools mutated.

Add a backwards-compatibility request using the detailed object form with forged `description`, `parametersSchema`, and `examples` for `read_file`. Assert creation succeeds but the response/runtime snapshot contains the registry-owned canonical description/schema/examples, never the request metadata.

In the same red phase, add a control-plane restoration test that creates an agent, PATCHes its tools, waits for the existing persistence completion path, recreates daemon state from the same temporary `ANIMAOS_RS_CONTROL_PLANE_FILE`, and asserts the restored snapshot still contains the daemon-owned descriptions and parameter schemas.

In the `routes/agents.rs` test module, add a capturing `ModelAdapter` that records the `AgentConfig` received by `generate`. Create an agent through the request/state path with `read_file`, `write_file`, and `bash`, run that agent once, and assert the captured adapter config includes the canonical required properties for all three tools. This exercises the complete create → runtime → agent-run → model-adapter boundary.

- [ ] **Step 2: Run the API tests to verify failure**

```powershell
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; cargo test -p anima-daemon --test agent_api agent_tools -- --nocapture
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; cargo test -p anima-daemon --test agent_api updated_agent_tools_survive_control_plane_restoration -- --exact
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; cargo test -p anima-daemon routes::agents::tests::run_agent_passes_canonical_tools_to_model_adapter -- --exact
```

Expected: FAIL because create keeps empty or request-owned descriptors, PATCH does not accept tools, and the adapter boundary does not receive canonical schemas.

- [ ] **Step 3: Extend the core update contract**

Add `tools: Option<Vec<ToolDescriptor>>` to `AgentConfigUpdate` and assign it in `AgentRuntime::update_config`. Update existing constructors/tests with `tools: None` where needed.

- [ ] **Step 4: Parse tools without trusting request metadata**

Add `tools: Option<Vec<ToolDescriptorRequest>>` to `AgentUpdateRequest`. Convert both string and detailed request forms to a list of names only; duplicate or unknown names should be rejected consistently. Do not retain request-supplied descriptions, schemas, or examples for registered host tools.

- [ ] **Step 5: Resolve before mutation in daemon state**

For create and update, call `ToolRegistry::resolve_descriptors` before constructing or mutating the runtime. Change `update_agent` to return a result capable of distinguishing unknown tool input from a missing agent. Build the complete `AgentConfigUpdate` only after validation succeeds, then persist via the existing control-plane snapshot path.

- [ ] **Step 6: Map errors at the route boundary**

Return the existing stable bad-request envelope for invalid/unknown tools and not-found for absent agents. Keep the response envelope shape unchanged on success.

- [ ] **Step 7: Run Rust verification and commit**

```powershell
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; bun x nx run rust-daemon:test --skipNxCache
git add packages/core-rust/crates/anima-core/src/agent.rs packages/core-rust/crates/anima-core/src/runtime.rs hosts/rust-daemon/src/routes/contracts/agents.rs hosts/rust-daemon/src/routes/agents.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/tests/agent_api.rs
git commit -m "feat(daemon): update agent workspace tools"
```

Expected: all non-ignored Rust host/core tests PASS.

## Task 4: Extend the web daemon model and bootstrap collection

**Files:**
- Modify: `apps/web/src/lib/types.ts`
- Modify: `apps/web/src/lib/daemon-api.ts`
- Create: `apps/web/src/lib/daemon-api.test.ts`
- Create: `apps/web/src/hooks/useDaemonBootstrap.ts`
- Create: `apps/web/src/hooks/useDaemonBootstrap.test.tsx`

- [ ] **Step 1: Write failing adapter and bootstrap tests**

Assert `toAgentDetail` maps `state.config.tools` to `toolNames` and preserves existing message filtering. Mock `daemon.health`, `listProviders`, and `listAgents` to prove:

- initial status is `unknown`, never `online`;
- successful bootstrap supplies the whole agent collection and selects no main agent itself;
- a failed health/agent request reports `offline` with retry;
- provider retry does not reset already-loaded agent state;
- polling retains the last known agents when a later request fails.
- `acceptAgentSnapshot(created)` inserts/replaces the POST snapshot immediately and that agent remains available after a subsequent polling failure.
- `acceptAgentSnapshot(updated)` replaces only the matching agent after PATCH while retaining other snapshots.
- `removeAgentSnapshot(id)` removes only the deleted agent so the next-oldest selector can promote deterministically.

- [ ] **Step 2: Run focused tests to verify failure**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=daemon-api.test.ts
bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=useDaemonBootstrap.test.tsx
```

Expected: FAIL because tool mapping and the hook do not exist.

- [ ] **Step 3: Extend wire and view-model types**

Represent daemon tools as descriptors on the wire but expose only `toolNames: string[]` on `AgentDetail`. Add `tools: string[]` to create input and optional `tools` to update input. Preserve current API error handling.

- [ ] **Step 4: Implement `useDaemonBootstrap`**

Return a small state object:

```ts
{
  connection: 'unknown' | 'online' | 'offline';
  loaded: boolean;
  agents: DaemonSnapshot[];
  providers: DaemonProvider[] | null;
  providersError: string | null;
  refreshAgents(): Promise<void>;
  retryProviders(): Promise<void>;
  acceptAgentSnapshot(snapshot: DaemonSnapshot): void;
  removeAgentSnapshot(id: string): void;
}
```

Use one initial bootstrap, five-second agent polling, and last-known-good data on polling failures. Keep health/provider failures distinct. `acceptAgentSnapshot` is the single POST/PATCH adoption path: replace a matching `state.id` or append a new snapshot, then sort by `state.createdAtMs` and `state.id`. `removeAgentSnapshot` is the successful DELETE path. Neither mutation performs a refetch.

- [ ] **Step 5: Run tests and commit**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache
git add apps/web/src/lib/types.ts apps/web/src/lib/daemon-api.ts apps/web/src/lib/daemon-api.test.ts apps/web/src/hooks/useDaemonBootstrap.ts apps/web/src/hooks/useDaemonBootstrap.test.tsx
git commit -m "feat(web): model daemon agents and workspace access"
```

## Task 5: Build the four-step Guided Focus onboarding flow

**Files:**
- Delete: `apps/web/src/components/SetupScreen.tsx`
- Create: `apps/web/src/components/onboarding/OnboardingFlow.tsx`
- Create: `apps/web/src/components/onboarding/OnboardingProgress.tsx`
- Create: `apps/web/src/components/onboarding/IdentityStep.tsx`
- Create: `apps/web/src/components/onboarding/ModelStep.tsx`
- Create: `apps/web/src/components/onboarding/AccessStep.tsx`
- Create: `apps/web/src/components/onboarding/ReviewStep.tsx`
- Create: `apps/web/src/components/onboarding/OnboardingFlow.test.tsx`

- [ ] **Step 1: Write failing user-flow tests**

Render the flow with provider fixtures and cover:

- Step 1 defaults the name to `Anima`, requires a non-empty name, and preserves optional instructions.
- Step 2 shows all daemon providers, disables unconfigured providers, and supports suggested/custom models.
- Provider retry leaves the Step 1 draft unchanged.
- Step 3 defaults to Collaborate and exposes Observe/Collaborate/Operate capability and risk copy.
- Step 4 summarizes the draft and submits the exact `toolNamesForProfile` result.
- A rejected create remains on Review with every choice intact and focuses/announces the error.
- A successful create calls `onCreated` with the returned snapshot, without requesting the agent list again.
- every step change updates a polite live announcement and `aria-current="step"`.
- invalid Next keeps the current step and focuses the first invalid labeled field.
- access choices include text capability/risk labels so their meaning does not depend on color.

- [ ] **Step 2: Run the component test to verify failure**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=OnboardingFlow.test.tsx
```

Expected: FAIL because the onboarding components do not exist.

- [ ] **Step 3: Implement typed draft ownership and validation**

`OnboardingFlow` owns:

```ts
type OnboardingDraft = {
  name: string;
  system: string;
  provider: string;
  model: string;
  customModel: string;
  access: AccessProfile;
};
```

Individual steps receive values/callbacks only. Keep network submission in the flow. Use semantic headings, field labels, a progress `<ol>`, `aria-current="step"`, and an alert/live region for errors.

- [ ] **Step 4: Implement provider fallback and review submission**

Configured providers are selectable; unavailable providers remain visible and disabled with environment guidance. The Review submit calls `daemon.createAgent` once with name/provider/resolved model/system/tools and passes `response.agent` to `onCreated`.

- [ ] **Step 5: Run tests and commit**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache
git add apps/web/src/components/onboarding apps/web/src/components/SetupScreen.tsx
git commit -m "feat(web): add guided main-agent onboarding"
```

## Task 6: Replace the sidebar with the spatial workspace shell

**Files:**
- Delete: `apps/web/src/components/Sidebar.tsx`
- Create: `apps/web/src/components/WorkspaceShell.tsx`
- Create: `apps/web/src/components/AgentPresence.tsx`
- Create: `apps/web/src/components/ActivityView.tsx`
- Create: `apps/web/src/components/AgentsView.tsx`
- Create: `apps/web/src/components/WorkspaceShell.test.tsx`
- Create: `apps/web/src/ViewHarness.test.tsx`
- Create: `apps/web-e2e/src/main-workspace-agent.spec.ts`
- Modify: `apps/web/src/components/ChatScreen.tsx`
- Modify: `apps/web/src/components/CheckinsView.tsx`
- Modify: `apps/web/src/ViewHarness.tsx`

- [ ] **Step 1: Characterize preserved controller behavior before refactoring**

In `ViewHarness.test.tsx`, mock the daemon client and use fake timers where needed. First capture these existing contracts as passing regression tests:

- sending chat calls `runAgent(mainAgent.id, text)`, refreshes visible messages, and does not target a later agent;
- a due proactive check-in calls `runAgent(mainAgent.id, wrappedPrompt, { kind: 'checkin', id })` and the Activity destination displays its outcome;
- saving identity/model/provider/system PATCHes the same agent and preserves returned messages;
- deleting an agent clears only that agent's local check-ins.

Run the test before structural edits; if a contract cannot be characterized without the new collection seam, add the smallest harness injection necessary and commit that seam separately before continuing.

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=ViewHarness.test.tsx
```

Expected: PASS for the current behavior characterization.

- [ ] **Step 2: Write failing shell and browser behavior tests**

Cover:

- unknown connection renders neutral connecting copy and no “connected” label;
- offline renders the focused retry state and `bun dev --host rust`;
- zero online agents renders only onboarding, without workspace navigation;
- agents render Workspace/Activity/Agents destinations;
- the oldest snapshot is the chat/settings target and receives the Main badge;
- additional agents render read-only in Agents;
- deleting the main agent promotes the next oldest; deleting the final agent returns to onboarding;
- small-screen markup exposes the same destinations through the bottom dock.
- daemon, agent, and permission status each include explicit text/icon labels so they remain understandable without color.

In `apps/web-e2e/src/main-workspace-agent.spec.ts`, use `page.route('**/api/**', ...)` with an in-memory agent array and provider fixtures. Add Chromium cases for zero-agent onboarding, POST failure/draft retention, successful POST transition, existing/multiple agents, PATCH failure preservation, offline bootstrap, a `390x844` viewport, `reducedMotion: 'reduce'`, keyboard-only step navigation, step announcement, first-invalid-field focus, and text/icon status labels.

- [ ] **Step 3: Run the new behavior tests to verify the red state**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=WorkspaceShell.test.tsx
bun x nx e2e @animaOS-SWARM/web-e2e --skipNxCache --project=chromium --grep="main workspace agent"
```

Expected: FAIL because the shell and multi-agent collection flow do not exist.

- [ ] **Step 4: Split orchestration from chrome in `ViewHarness`**

Use `useDaemonBootstrap`, adapt each wire `DaemonSnapshot` to `AgentDetail`, and call `selectMainAgent` only with that adapted `AgentDetail[]`. Keep chat, check-in scheduling, reset, and settings state targeted to that selected main agent. On onboarding success, call the wire-level `acceptAgentSnapshot(response.agent)` and enter Workspace immediately. On PATCH success use the same wire-level adoption function. On DELETE success call `removeAgentSnapshot(id)`. Preserve last-known-good snapshots when later polling fails.

- [ ] **Step 5: Implement the new shell and destinations**

`WorkspaceShell` owns responsive navigation for `workspace | activity | agents`. `AgentPresence` shows the code-native orb, agent/runtime status, and derived access badge. `ActivityView` preserves current check-ins and adds snapshot-derived token/message summaries only. `AgentsView` lists all agents, clearly badges the oldest as Main, and provides no specialist creation controls.

- [ ] **Step 6: Restyle chat presentation without changing chat behavior**

Keep `MessageList` and `Composer` APIs stable where practical. Place them in a centered readable canvas, keep the composer reachable on mobile, and remove sidebar-dependent offsets.

- [ ] **Step 7: Run tests and commit**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache
bun x nx e2e @animaOS-SWARM/web-e2e --skipNxCache --project=chromium --grep="main workspace agent"
git add apps/web/src/components/WorkspaceShell.tsx apps/web/src/components/AgentPresence.tsx apps/web/src/components/ActivityView.tsx apps/web/src/components/AgentsView.tsx apps/web/src/components/WorkspaceShell.test.tsx apps/web/src/components/ChatScreen.tsx apps/web/src/components/CheckinsView.tsx apps/web/src/components/Sidebar.tsx apps/web/src/ViewHarness.tsx apps/web/src/ViewHarness.test.tsx apps/web-e2e/src/main-workspace-agent.spec.ts
git commit -m "feat(web): center the workspace on the main agent"
```

## Task 7: Add access-profile editing to Settings

**Files:**
- Modify: `apps/web/src/components/SettingsPanel.tsx`
- Create: `apps/web/src/components/SettingsPanel.test.tsx`
- Modify: `apps/web/src/ViewHarness.tsx`

- [ ] **Step 1: Write failing Settings tests**

Cover exact profile derivation, `Custom access` for unmatched tools, a deliberate profile selection sending exact tool slugs, successful response replacement, and failed PATCH preserving the unsaved selection while keeping the prior agent tools visible outside the form.

- [ ] **Step 2: Run the focused test to verify failure**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=SettingsPanel.test.tsx
```

Expected: FAIL because Settings does not expose access controls or pass tools in its patch.

- [ ] **Step 3: Extend the settings draft and save contract**

Initialize from `deriveAccessProfile(agent.toolNames)`. For custom tools, show a non-editing `Custom access` state until the user deliberately chooses Observe, Collaborate, or Operate. Include `tools` only after that explicit selection. Keep the drawer open and draft values intact on errors.

- [ ] **Step 4: Update the workspace controller from the PATCH response**

Have `saveSettings` use `response.agent` to update local state immediately and then allow background refresh. Do not clear messages or recreate the agent.

- [ ] **Step 5: Run tests and commit**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache
git add apps/web/src/components/SettingsPanel.tsx apps/web/src/components/SettingsPanel.test.tsx apps/web/src/ViewHarness.tsx
git commit -m "feat(web): edit main-agent workspace access"
```

## Task 8: Apply the Neon Rose Spatial Intelligence visual system

**Files:**
- Modify: `apps/web/src/styles.css`
- Modify: `apps/web/src/components/ui-bits.tsx`
- Modify: `apps/web/src/components/icons.tsx`
- Modify: `apps/web/src/components/onboarding/OnboardingFlow.tsx`
- Modify: `apps/web/src/components/onboarding/OnboardingProgress.tsx`
- Modify: `apps/web/src/components/onboarding/IdentityStep.tsx`
- Modify: `apps/web/src/components/onboarding/ModelStep.tsx`
- Modify: `apps/web/src/components/onboarding/AccessStep.tsx`
- Modify: `apps/web/src/components/onboarding/ReviewStep.tsx`
- Modify: `apps/web/src/components/WorkspaceShell.tsx`
- Modify: `apps/web/src/components/AgentPresence.tsx`
- Modify: `apps/web/src/components/ActivityView.tsx`
- Modify: `apps/web/src/components/AgentsView.tsx`
- Modify: `apps/web/src/components/ChatScreen.tsx`
- Modify: `apps/web/src/components/CheckinsView.tsx`
- Modify: `apps/web/src/components/SettingsPanel.tsx`
- Create: `apps/web/src/visual-tokens.test.ts`

- [ ] **Step 1: Add a static token guard test**

In `apps/web/src/visual-tokens.test.ts`, read `styles.css` plus the listed component sources relative to `import.meta.url`. Assert the approved anchors exist—`#090A0F`, `#17171D`, `#FF397F`, `#64DFAD`—while prior accent classes/tokens such as `sky-400`, `purple-`, `violet-`, and purple/blue accent hex values are absent from those files.

- [ ] **Step 2: Run the guard to verify failure**

```powershell
bun x nx test @animaOS-SWARM/web --skipNxCache --testFile=visual-tokens.test.ts
```

Expected: FAIL while the current blue/sky styling remains.

- [ ] **Step 3: Define and apply the visual tokens**

Use CSS variables for near-black base, graphite surface, Neon Rose primary/focus/selected state, mint health, warm near-white text, and neutral borders. Reserve rose for presence, primary actions, focus, selected states, and restrained glow; do not use it as a large flat background.

- [ ] **Step 4: Implement depth, motion, and responsive behavior**

Add a slim translucent desktop top shell, centered spatial canvas, code-native layered orb/glow, mobile bottom dock, desktop settings drawer/mobile full-screen sheet, readable truncation with titles, safe-area-aware composer spacing, visible `:focus-visible`, and static/near-instant reduced-motion overrides.

- [ ] **Step 5: Run unit, type, build, and lint checks**

```powershell
bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/web --skipNxCache
bun x nx run @animaOS-SWARM/web:lint --skipNxCache
```

If the inferred web project has no `lint` target, record that fact and run the repository lint command scoped to `apps/web/src` using the existing workspace configuration instead of inventing an Nx target.

- [ ] **Step 6: Commit**

```powershell
git add apps/web/src
git commit -m "style(web): apply neon rose spatial workspace"
```

## Task 9: Full verification and browser QA

**Files:**
- Modify only files needed to fix verified regressions

- [ ] **Step 1: Run the complete relevant automated suite**

```powershell
$env:CI='1'; bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/web --skipNxCache
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; bun x nx run rust-daemon:test --skipNxCache
$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; bun x nx run rust-daemon:lint --skipNxCache
```

Expected: all relevant non-ignored tests, web typecheck/build, and Rust formatting PASS.

- [ ] **Step 2: Run deterministic mocked browser scenarios**

Run the Playwright route-fixture suite created in Task 6. Its in-memory API owns zero-agent, provider failure/retry, create failure, successful POST, multiple-agent, PATCH failure, and offline responses without touching user daemon data:

```powershell
bun x nx e2e @animaOS-SWARM/web-e2e --skipNxCache --project=chromium --grep="main workspace agent"
```

Verify the cases explicitly assert all four steps, disabled providers, Collaborate default, draft preservation, immediate Workspace transition, `aria-current="step"`/live announcement changes, first-invalid-field focus, keyboard-only operation, textual/icon status labels, `390x844` bottom dock/sheet behavior, and reduced-motion styles.

- [ ] **Step 3: Run one live disposable-daemon smoke path**

Create an isolated QA directory and point a standalone daemon at it; never delete or reuse the user's normal control-plane file:

```powershell
$qaRoot = Join-Path $env:TEMP 'animaos-main-agent-qa'
if (Test-Path -LiteralPath $qaRoot) { Move-Item -LiteralPath $qaRoot -Destination "$qaRoot-backup-$(Get-Date -Format yyyyMMddHHmmss)" }
New-Item -ItemType Directory -Path $qaRoot, (Join-Path $qaRoot 'workspace') | Out-Null
$env:ANIMAOS_RS_HOST='127.0.0.1'
$env:ANIMAOS_RS_PORT='18080'
$env:ANIMAOS_RS_CONTROL_PLANE_FILE=(Join-Path $qaRoot 'control-plane.json')
$env:ANIMAOS_RS_MEMORY_FILE=(Join-Path $qaRoot 'memory.json')
$env:ANIMAOS_RS_MEMORY_EMBEDDINGS='disabled'
$env:ANIMAOS_WORKSPACE_ROOT=(Join-Path $qaRoot 'workspace')
bun x nx run rust-daemon:dev
```

In a second terminal start the web proxy against the disposable daemon:

```powershell
$env:UI_BACKEND_ORIGIN='http://127.0.0.1:18080'
bun x nx run @animaOS-SWARM/web:dev
```

Use `http://localhost:4200`. Complete onboarding with the deterministic provider and send one chat message. In a third PowerShell terminal, run these exact live assertions:

```powershell
$apiRoot = 'http://127.0.0.1:18080/api'
$listed = Invoke-RestMethod -Method Get -Uri "$apiRoot/agents"
$main = $listed.agents | Sort-Object @{Expression={$_.state.createdAtMs}}, @{Expression={$_.state.id}} | Select-Object -First 1
if (-not $main) { throw 'onboarding did not persist the main agent' }
$mainId = $main.state.id
$mainMessageCount = $main.messageCount

$secondBody = @{
  name = 'Second agent'
  provider = 'deterministic'
  model = 'deterministic'
  tools = @('memory_search','memory_add','recent_memories','get_current_time','calculate','read_file','list_dir','glob','grep','todo_read')
} | ConvertTo-Json -Depth 5
$second = Invoke-RestMethod -Method Post -Uri "$apiRoot/agents" -ContentType 'application/json' -Body $secondBody
$secondId = $second.agent.state.id
if ($secondId -eq $mainId) { throw 'second agent did not receive a distinct id' }

$before = Invoke-RestMethod -Method Get -Uri "$apiRoot/agents/$mainId"
$beforeConfig = $before.agent.state.config | ConvertTo-Json -Depth 30 -Compress
$badBody = @{ name = 'must-not-stick'; tools = @('not_registered') } | ConvertTo-Json
$badStatus = 0
try {
  Invoke-RestMethod -Method Patch -Uri "$apiRoot/agents/$mainId" -ContentType 'application/json' -Body $badBody
} catch {
  $badStatus = [int]$_.Exception.Response.StatusCode
}
if ($badStatus -ne 400) { throw "unknown-tool PATCH returned $badStatus instead of 400" }

$after = Invoke-RestMethod -Method Get -Uri "$apiRoot/agents/$mainId"
$afterConfig = $after.agent.state.config | ConvertTo-Json -Depth 30 -Compress
if ($afterConfig -ne $beforeConfig) { throw 'invalid PATCH partially mutated main-agent config' }
if ($after.agent.messageCount -ne $mainMessageCount) { throw 'settings/API checks changed conversation history' }
```

Return to the browser and verify the second agent appears read-only while the first remains Main. Use Settings to make one valid access-profile change and confirm the prior chat remains. Stop both foreground processes after the smoke test; retain the QA directory for inspection.

- [ ] **Step 4: Test visual, accessible, and failure states**

At `1440x900` and `390x844`, verify top-shell/bottom-dock behavior, settings drawer/sheet, composer reachability, focus visibility, text contrast, title-based truncation, reduced motion, neutral unknown health, and focused offline recovery copy. Navigate the full flow using Tab/Shift+Tab/Enter only. Confirm step changes are announced, invalid Next focuses the first invalid field, and daemon/agent/permission meanings have text or icon labels independent of color. Confirm there is no purple or blue accent residue.

- [ ] **Step 5: Review the final diff and commit any QA fixes**

```powershell
git status --short
git diff --check
git diff --stat
```

Commit only intentional fixes with a focused message. Do not include generated build artifacts or unrelated workspace changes.
