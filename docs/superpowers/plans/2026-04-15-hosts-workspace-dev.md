# Hosts Workspace Dev Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the Rust daemon workspace under `hosts/`, make it a first-class Nx project, and add one `workspace-dev` orchestration flow that starts the selected host plus `apps/ui`.

**Architecture:** Keep backend host metadata in one TypeScript registry under a new root-level `tools/workspace-dev` project. Represent each host as an explicit Nx project via `project.json`, then let `workspace-dev` spawn the selected host target and the UI with host-derived environment variables. Update root scripts, tests, and high-signal docs/config so the new `hosts/` boundary is real instead of aspirational.

**Tech Stack:** Nx 22, Bun workspaces, TypeScript, Vitest, Cargo, Axum/Tokio Rust workspace

---

### Task 1: Add The Workspace Dev Tooling Project

**Files:**
- Create: `tools/workspace-dev/package.json`
- Create: `tools/workspace-dev/tsconfig.json`
- Create: `tools/workspace-dev/tsconfig.lib.json`
- Create: `tools/workspace-dev/tsconfig.spec.json`
- Create: `tools/workspace-dev/vitest.config.mts`
- Create: `tools/workspace-dev/src/hosts.ts`
- Create: `tools/workspace-dev/src/main.ts`
- Create: `tools/workspace-dev/src/process.ts`
- Create: `tools/workspace-dev/src/placeholder-host.ts`
- Create: `tools/workspace-dev/src/hosts.spec.ts`
- Create: `tools/workspace-dev/src/main.spec.ts`
- Modify: `package.json`
- Modify: `tsconfig.json`

- [ ] **Step 1: Write the failing registry tests**

Add tests in `tools/workspace-dev/src/hosts.spec.ts` that prove:
- the registry accepts `rust`, `elixir`, and `python`
- `rust` resolves to a ready host definition with project name `rust-daemon`
- `elixir` and `python` resolve to placeholder definitions
- unknown hosts produce a clear error

- [ ] **Step 2: Run the registry test to verify it fails**

Run: `bun x nx test workspace-dev --runInBand`
Expected: FAIL because `workspace-dev` and its registry module do not exist yet

- [ ] **Step 3: Write the minimal tooling project**

Create the new `tools/workspace-dev` TS/Vitest project, add it to root workspaces/references, and implement:
- `hosts.ts` for central host metadata
- `process.ts` for child-process lifecycle helpers
- `main.ts` for parsing `--host`, deriving `UI_BACKEND_ORIGIN`, and spawning the host + UI
- `placeholder-host.ts` for explicit placeholder failures used by future hosts

Define the `workspace-dev:dev` Nx target in `tools/workspace-dev/package.json` using `nx:run-commands` with `forwardAllArgs`.

- [ ] **Step 4: Run the registry test to verify it passes**

Run: `bun x nx test workspace-dev --runInBand`
Expected: PASS for the new registry tests

- [ ] **Step 5: Add a focused orchestration behavior test**

Add `tools/workspace-dev/src/main.spec.ts` to verify the command-building layer:
- chooses the selected host project
- injects `UI_BACKEND_ORIGIN`
- rejects placeholder hosts with intentional messaging

- [ ] **Step 6: Run the tooling test suite**

Run: `bun x nx test workspace-dev --runInBand`
Expected: PASS with both registry and orchestration tests green

- [ ] **Step 7: Commit**

```bash
git add package.json tsconfig.json tools/workspace-dev
git commit -m "feat: add workspace dev orchestration project"
```

### Task 2: Move The Rust Workspace Under Hosts And Add Nx Targets

**Files:**
- Create: `hosts/rust-daemon/project.json`
- Modify: `package.json`
- Modify: `README.md`
- Modify: `packages/sdk/tests/daemon-integration.spec.ts`
- Modify: `docs/SDK_USAGE.md`
- Modify: `docs/PRD.md`
- Modify: `docs/package-maturity-scorecard.md`
- Modify: `packages/core/README.md`
- Modify: `packages/memory/README.md`
- Move: `packages/animaos-rs` -> `hosts/rust-daemon`

- [ ] **Step 1: Write the failing path-reference test**

Update `packages/sdk/tests/daemon-integration.spec.ts` first so it expects the daemon manifest at `hosts/rust-daemon/Cargo.toml`.

- [ ] **Step 2: Run the targeted SDK integration test to verify it fails**

Run: `bun x nx test @animaOS-SWARM/sdk --runInBand --testPathPattern=daemon-integration`
Expected: FAIL because the manifest path still points at the old location

- [ ] **Step 3: Move the Rust workspace and create its Nx project**

Move the full Rust workspace to `hosts/rust-daemon`, then add `hosts/rust-daemon/project.json` with thin Nx `dev`, `build`, `test`, and `lint` targets backed by Cargo commands.

Update root scripts so:
- `bun run daemon` points at `hosts/rust-daemon/Cargo.toml`
- `bun dev --host <name>` routes through `workspace-dev`

- [ ] **Step 4: Sweep high-signal repo references**

Update the live code/docs/config references that should track the new canonical host location:
- SDK integration test manifest path
- root README
- SDK usage doc
- PRD summary
- package maturity scorecard
- package README files that describe Rust as canonical runtime

Leave historical plan/spec artifacts alone unless they would break tooling or current docs navigation.

- [ ] **Step 5: Verify the Rust host targets**

Run: `bun x nx run rust-daemon:build`
Expected: PASS

Run: `bun x nx run rust-daemon:test`
Expected: PASS

- [ ] **Step 6: Verify the updated SDK path assumption**

Run: `bun x nx test @animaOS-SWARM/sdk --runInBand --testPathPattern=daemon-integration`
Expected: PASS or, if the suite is intentionally slow/flaky in this environment, fail for runtime reasons other than missing manifest path

- [ ] **Step 7: Commit**

```bash
git add hosts/rust-daemon package.json README.md packages/sdk/tests/daemon-integration.spec.ts docs/SDK_USAGE.md docs/PRD.md docs/package-maturity-scorecard.md packages/core/README.md packages/memory/README.md
git commit -m "refactor: move rust daemon workspace under hosts"
```

### Task 3: Add Placeholder Host Projects And Central Host Wiring

**Files:**
- Create: `hosts/elixir-phoenix/project.json`
- Create: `hosts/elixir-phoenix/README.md`
- Create: `hosts/python-service/project.json`
- Create: `hosts/python-service/README.md`
- Modify: `tools/workspace-dev/src/hosts.ts`

- [ ] **Step 1: Write the failing placeholder-host assertion**

Extend `tools/workspace-dev/src/hosts.spec.ts` or `main.spec.ts` so the suite expects:
- `elixir-phoenix:dev` to fail intentionally
- `python-service:dev` to fail intentionally
- `workspace-dev --host elixir` and `workspace-dev --host python` to surface the placeholder status clearly

- [ ] **Step 2: Run the tooling test suite to verify it fails**

Run: `bun x nx test workspace-dev --runInBand`
Expected: FAIL because the placeholder project targets do not exist yet

- [ ] **Step 3: Create placeholder host projects**

Add `project.json` + `README.md` for `hosts/elixir-phoenix` and `hosts/python-service`.

Each project should expose `dev`, `build`, `test`, and `lint` targets. At minimum, `dev` must fail intentionally through the shared placeholder script with a message that the host is registered but not implemented.

- [ ] **Step 4: Re-run the tooling tests**

Run: `bun x nx test workspace-dev --runInBand`
Expected: PASS

- [ ] **Step 5: Verify intentional placeholder failures**

Run: `bun x nx run elixir-phoenix:dev`
Expected: FAIL with a clear placeholder message

Run: `bun x nx run python-service:dev`
Expected: FAIL with a clear placeholder message

- [ ] **Step 6: Commit**

```bash
git add hosts/elixir-phoenix hosts/python-service tools/workspace-dev/src/hosts.ts tools/workspace-dev/src/hosts.spec.ts tools/workspace-dev/src/main.spec.ts
git commit -m "feat: add placeholder host projects"
```

### Task 4: Wire The UI And E2E Flows To The Selected Host Contract

**Files:**
- Modify: `apps/ui/vite.config.mts`
- Modify: `apps/ui-e2e/playwright.live.config.ts`
- Modify: `README.md`

- [ ] **Step 1: Write the failing UI config test or assertion**

Add/extend the `workspace-dev` tests so they assert the UI command always receives `UI_BACKEND_ORIGIN` from host metadata instead of hardcoded `localhost:3000` assumptions.

- [ ] **Step 2: Run the tooling tests to verify the new assertion fails**

Run: `bun x nx test workspace-dev --runInBand`
Expected: FAIL until the orchestration and live config match the registry contract

- [ ] **Step 3: Tighten the live dev wiring**

Update:
- `apps/ui/vite.config.mts` to keep `UI_BACKEND_ORIGIN` authoritative while leaving a sensible local fallback only for non-orchestrated runs
- `apps/ui-e2e/playwright.live.config.ts` so the live browser flow uses the Rust host project or host-derived origin consistently with the new contract
- README usage examples to advertise `bun dev --host rust`

- [ ] **Step 4: Re-run the tooling tests**

Run: `bun x nx test workspace-dev --runInBand`
Expected: PASS

- [ ] **Step 5: Run the live UI build/test smoke checks**

Run: `bun x nx build @animaOS-SWARM/ui`
Expected: PASS

Run: `bun x nx test @animaOS-SWARM/ui-e2e --runInBand`
Expected: PASS, or if the environment cannot run browsers, fail for browser/runtime reasons rather than configuration drift

- [ ] **Step 6: Commit**

```bash
git add apps/ui/vite.config.mts apps/ui-e2e/playwright.live.config.ts README.md tools/workspace-dev/src/main.spec.ts
git commit -m "feat: wire ui dev flow through host registry"
```

### Task 5: End-To-End Verification For The New Workspace Entry Points

**Files:**
- Modify: `package.json` only if command ergonomics still need adjustment
- Modify: `tools/workspace-dev/src/main.ts` only if verification exposes gaps

- [ ] **Step 1: Verify the Nx-native entrypoint**

Run: `bun x nx run workspace-dev:dev -- --host rust`
Expected: host process starts, UI process starts, command stays attached until interrupted

- [ ] **Step 2: Verify the Bun entrypoint**

Run: `bun dev --host rust`
Expected: same behavior as the Nx-native entrypoint

- [ ] **Step 3: Verify unsupported host selection stays intentional**

Run: `bun dev --host elixir`
Expected: FAIL with a clear placeholder message

Run: `bun dev --host python`
Expected: FAIL with a clear placeholder message

- [ ] **Step 4: Run focused final verification**

Run: `bun x nx test workspace-dev --runInBand`
Expected: PASS

Run: `bun x nx run rust-daemon:test`
Expected: PASS

Run: `bun x nx build @animaOS-SWARM/ui`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add package.json tools/workspace-dev/src/main.ts
git commit -m "feat: add unified workspace dev entrypoint"
```
