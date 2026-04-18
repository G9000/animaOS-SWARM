# Core Ports And Hosts Workspace Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure the repo so reusable engine ports live in `packages/core-rust` and `packages/core-ts`, runnable backends live in `hosts/*`, and the existing `workspace-dev` flow keeps starting the selected host plus `apps/ui`.

**Architecture:** The repo already contains a partial host-first migration: `hosts/rust-daemon` exists, `workspace-dev` exists, and `bun dev --host rust` already points at that host. This plan corrects the boundary without throwing that work away. Reusable Rust crates move out of `hosts/rust-daemon/crates/*` into `packages/core-rust/crates/*`; the daemon crate is flattened to `hosts/rust-daemon/`; the TS core folder becomes `packages/core-ts` while keeping the npm package name `@animaOS-SWARM/core` to avoid unnecessary import churn. This plan supersedes `docs/superpowers/plans/2026-04-15-hosts-workspace-dev.md`.

**Tech Stack:** Nx 22, Bun workspaces, TypeScript, Vitest, Cargo, Rust, Axum, Tokio

---

## File Structure

**New files:**
- `Cargo.toml` - repo-root Cargo workspace for Rust packages and hosts
- `packages/core-rust/README.md` - Rust core port overview
- `tools/workspace-dev/src/layout.spec.ts` - repo-layout regression tests for the new package/host boundary
- `hosts/elixir-phoenix/project.json` - explicit placeholder Nx project
- `hosts/python-service/project.json` - explicit placeholder Nx project

**Moved files/directories:**
- `packages/core` -> `packages/core-ts`
- `hosts/rust-daemon/crates/anima-core` -> `packages/core-rust/crates/anima-core`
- `hosts/rust-daemon/crates/anima-memory` -> `packages/core-rust/crates/anima-memory`
- `hosts/rust-daemon/crates/anima-swarm` -> `packages/core-rust/crates/anima-swarm`
- `hosts/rust-daemon/crates/anima-daemon/src` -> `hosts/rust-daemon/src`
- `hosts/rust-daemon/crates/anima-daemon/tests` -> `hosts/rust-daemon/tests`
- `hosts/rust-daemon/crates/anima-daemon/migrations` -> `hosts/rust-daemon/migrations`
- `hosts/rust-daemon/crates/anima-daemon/README.md` -> `hosts/rust-daemon/README.md`
- `hosts/rust-daemon/README.md` -> `packages/core-rust/README.md`

**Modified files:**
- `tsconfig.json` - point root TS references at `packages/core-ts`
- `apps/server/tsconfig.app.json` - point app references at `../../packages/core-ts/tsconfig.lib.json`
- `packages/core-ts/vitest.config.mts` - update cache dir path after the move
- `package.json` - keep `dev` and `daemon` scripts aligned with the corrected boundary if needed
- `.gitignore` - ignore repo-root `target/`
- `hosts/rust-daemon/Cargo.toml` - convert from Rust workspace manifest to a daemon package manifest and update dependencies to `../../packages/core-rust/crates/*`
- `hosts/rust-daemon/project.json` - run daemon package commands, not workspace-wide commands
- `hosts/rust-daemon/README.md` - daemon-specific host docs after the flatten
- `packages/core-rust/crates/anima-memory/Cargo.toml` - update `anima-core` path
- `packages/core-rust/crates/anima-swarm/Cargo.toml` - update `anima-core` and `anima-memory` paths
- `tools/workspace-dev/src/hosts.ts` - keep host registry correct for placeholder projects if paths or names drift
- `tools/workspace-dev/src/hosts.spec.ts` - keep registry expectations aligned
- `tools/workspace-dev/src/main.spec.ts` - keep orchestration expectations aligned
- `README.md` - describe `packages/core-rust`, `packages/core-ts`, and `hosts/rust-daemon` correctly
- `docs/SDK_USAGE.md` - point docs at the new core-vs-host split
- `docs/PRD.md` - fix package/host tree
- `docs/package-maturity-scorecard.md` - fix ownership language
- `packages/core-ts/README.md` - explain that this is the TS core port and not the canonical runtime host
- `packages/memory/README.md` - stop describing `hosts/rust-daemon` as the runtime core

---

## Preflight

**Important current-state note:** this repo currently has uncommitted edits under `hosts/rust-daemon/*`. Do not overwrite or discard them during execution. If the worktree is still dirty when implementation starts:

- either move to a clean worktree first
- or coordinate with the human before moving daemon files

This is not optional. The restructuring is file-move heavy and will conflict badly with parallel edits.

---

## Task 1: Rehome The TypeScript Core Folder As `packages/core-ts`

**Files:**
- Create: `tools/workspace-dev/src/layout.spec.ts`
- Move: `packages/core` -> `packages/core-ts`
- Modify: `tsconfig.json`
- Modify: `apps/server/tsconfig.app.json`
- Modify: `packages/core-ts/vitest.config.mts`

- [ ] **Step 1: Write the failing layout test for the TypeScript core path**

Create `tools/workspace-dev/src/layout.spec.ts`:

```ts
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

function repoPath(...segments: string[]) {
  return resolve(import.meta.dirname, '..', '..', '..', ...segments);
}

describe('repo layout', () => {
  it('stores the TypeScript core port in packages/core-ts', () => {
    expect(existsSync(repoPath('packages', 'core-ts', 'package.json'))).toBe(true);
    expect(existsSync(repoPath('packages', 'core', 'package.json'))).toBe(false);
  });
});
```

- [ ] **Step 2: Run the workspace-dev tests to verify the new path test fails**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: FAIL because `packages/core-ts/package.json` does not exist yet and `packages/core/package.json` still exists.

- [ ] **Step 3: Move the folder and update TypeScript references**

Make these exact changes:

- move `packages/core` to `packages/core-ts`
- in `tsconfig.json`, replace:

```json
{ "path": "./packages/core" }
```

with:

```json
{ "path": "./packages/core-ts" }
```

- in `apps/server/tsconfig.app.json`, replace:

```json
{ "path": "../../packages/core/tsconfig.lib.json" }
```

with:

```json
{ "path": "../../packages/core-ts/tsconfig.lib.json" }
```

- in `packages/core-ts/vitest.config.mts`, replace:

```ts
cacheDir: '../../node_modules/.vite/packages/core',
```

with:

```ts
cacheDir: '../../node_modules/.vite/packages/core-ts',
```

Do **not** change the npm package name in `packages/core-ts/package.json` yet. Keep:

```json
"name": "@animaOS-SWARM/core"
```

This pass is about folder boundaries, not workspace-wide import renames.

- [ ] **Step 4: Re-run the workspace-dev tests**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: PASS for the new `packages/core-ts` layout assertion.

- [ ] **Step 5: Run focused build checks for the moved TS core**

Run: `bun x nx build @animaOS-SWARM/core`  
Expected: PASS

Run: `bun x nx build @animaOS-SWARM/server`  
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add tsconfig.json apps/server/tsconfig.app.json packages/core-ts tools/workspace-dev/src/layout.spec.ts
git commit -m "refactor: rename ts core folder to core-ts"
```

### Task 2: Move Reusable Rust Crates Into `packages/core-rust`

**Files:**
- Create: `Cargo.toml`
- Modify: `.gitignore`
- Move: `hosts/rust-daemon/README.md` -> `packages/core-rust/README.md`
- Move: `hosts/rust-daemon/crates/anima-core` -> `packages/core-rust/crates/anima-core`
- Move: `hosts/rust-daemon/crates/anima-memory` -> `packages/core-rust/crates/anima-memory`
- Move: `hosts/rust-daemon/crates/anima-swarm` -> `packages/core-rust/crates/anima-swarm`
- Modify: `packages/core-rust/crates/anima-memory/Cargo.toml`
- Modify: `packages/core-rust/crates/anima-swarm/Cargo.toml`
- Modify: `tools/workspace-dev/src/layout.spec.ts`

- [ ] **Step 1: Extend the layout test to assert the new Rust core paths**

Append to `tools/workspace-dev/src/layout.spec.ts`:

```ts
it('stores reusable Rust crates under packages/core-rust', () => {
  expect(
    existsSync(repoPath('packages', 'core-rust', 'crates', 'anima-core', 'Cargo.toml'))
  ).toBe(true);
  expect(
    existsSync(repoPath('packages', 'core-rust', 'crates', 'anima-memory', 'Cargo.toml'))
  ).toBe(true);
  expect(
    existsSync(repoPath('packages', 'core-rust', 'crates', 'anima-swarm', 'Cargo.toml'))
  ).toBe(true);

  expect(
    existsSync(repoPath('hosts', 'rust-daemon', 'crates', 'anima-core', 'Cargo.toml'))
  ).toBe(false);
  expect(
    existsSync(repoPath('hosts', 'rust-daemon', 'crates', 'anima-memory', 'Cargo.toml'))
  ).toBe(false);
  expect(
    existsSync(repoPath('hosts', 'rust-daemon', 'crates', 'anima-swarm', 'Cargo.toml'))
  ).toBe(false);
});
```

- [ ] **Step 2: Run the workspace-dev tests to verify the Rust layout assertions fail**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: FAIL because the Rust core crates still live under `hosts/rust-daemon/crates/*`.

- [ ] **Step 3: Create the repo-root Cargo workspace and move the core crates**

Create repo-root `Cargo.toml` with:

```toml
[workspace]
members = [
    "packages/core-rust/crates/anima-core",
    "packages/core-rust/crates/anima-memory",
    "packages/core-rust/crates/anima-swarm",
    "hosts/rust-daemon/crates/anima-daemon",
]
resolver = "2"

[workspace.package]
edition = "2021"
license = "MIT"
version = "0.1.0"
authors = ["animaOS contributors"]

[workspace.dependencies]
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio", "migrate", "macros", "json", "chrono"], default-features = false }

[workspace.lints.rust]
unsafe_code = "forbid"
```

Then:

- move `hosts/rust-daemon/README.md` to `packages/core-rust/README.md`
- move the three reusable crates to `packages/core-rust/crates/*`
- update `packages/core-rust/crates/anima-memory/Cargo.toml`:

```toml
[dependencies]
anima-core = { path = "../anima-core" }
```

- update `packages/core-rust/crates/anima-swarm/Cargo.toml`:

```toml
[dependencies]
anima-core = { path = "../anima-core" }
anima-memory = { path = "../anima-memory" }
futures = "0.3"
tokio = { version = "1.48.0", features = ["sync"] }
```

- add `/target` to `.gitignore`

- [ ] **Step 4: Verify Cargo sees the new workspace**

Run: `cargo metadata --format-version 1`  
Expected: PASS with members including `packages/core-rust/crates/anima-core`, `anima-memory`, `anima-swarm`, and `hosts/rust-daemon/crates/anima-daemon`.

- [ ] **Step 5: Run focused Rust crate tests from the repo root**

Run: `cargo test -p anima-core`  
Expected: PASS

Run: `cargo test -p anima-memory`  
Expected: PASS

Run: `cargo test -p anima-swarm`  
Expected: PASS

- [ ] **Step 6: Re-run the workspace-dev tests**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: PASS for the new Rust core path assertions.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml .gitignore packages/core-rust tools/workspace-dev/src/layout.spec.ts
git commit -m "refactor: move reusable rust crates to core-rust"
```

### Task 3: Flatten The Daemon Crate To `hosts/rust-daemon`

**Files:**
- Move: `hosts/rust-daemon/crates/anima-daemon/README.md` -> `hosts/rust-daemon/README.md`
- Move: `hosts/rust-daemon/crates/anima-daemon/src` -> `hosts/rust-daemon/src`
- Move: `hosts/rust-daemon/crates/anima-daemon/tests` -> `hosts/rust-daemon/tests`
- Move: `hosts/rust-daemon/crates/anima-daemon/migrations` -> `hosts/rust-daemon/migrations`
- Modify: `hosts/rust-daemon/Cargo.toml`
- Modify: `hosts/rust-daemon/project.json`
- Modify: `tools/workspace-dev/src/layout.spec.ts`

- [ ] **Step 1: Extend the layout test to assert the flattened daemon host**

Append to `tools/workspace-dev/src/layout.spec.ts`:

```ts
it('stores the daemon package at hosts/rust-daemon', () => {
  expect(existsSync(repoPath('hosts', 'rust-daemon', 'Cargo.toml'))).toBe(true);
  expect(existsSync(repoPath('hosts', 'rust-daemon', 'src', 'main.rs'))).toBe(true);
  expect(existsSync(repoPath('hosts', 'rust-daemon', 'tests', 'health.rs'))).toBe(true);
  expect(existsSync(repoPath('hosts', 'rust-daemon', 'crates', 'anima-daemon'))).toBe(false);
});
```

- [ ] **Step 2: Run the workspace-dev tests to verify the flattened-daemon assertion fails**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: FAIL because the daemon crate is still nested in `hosts/rust-daemon/crates/anima-daemon`.

- [ ] **Step 3: Flatten the daemon package and rewrite its manifest**

Move the daemon crate contents up to `hosts/rust-daemon/`, then replace `hosts/rust-daemon/Cargo.toml` with:

```toml
[package]
name = "anima-daemon"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true

[dependencies]
anima-core = { path = "../../packages/core-rust/crates/anima-core" }
anima-memory = { path = "../../packages/core-rust/crates/anima-memory" }
anima-swarm = { path = "../../packages/core-rust/crates/anima-swarm" }
async-trait = "0.1"
axum = "0.8"
futures = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { workspace = true }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = ["request-id", "timeout", "trace"] }
tokio = { version = "1", features = ["macros", "net", "rt-multi-thread", "signal", "sync"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
utoipa = { version = "5.4.0", features = ["axum_extras"] }
utoipa-swagger-ui = { version = "9.0.2", features = ["axum", "vendored"] }

[dev-dependencies]
http-body-util = "0.1"

[lints]
workspace = true
```

Update the repo-root `Cargo.toml` workspace members to:

```toml
[workspace]
members = [
    "packages/core-rust/crates/anima-core",
    "packages/core-rust/crates/anima-memory",
    "packages/core-rust/crates/anima-swarm",
    "hosts/rust-daemon",
]
resolver = "2"
```

- [ ] **Step 4: Retarget the Nx host project to the daemon package**

Update `hosts/rust-daemon/project.json` targets to:

```json
{
  "$schema": "../../node_modules/nx/schemas/project-schema.json",
  "name": "rust-daemon",
  "root": "hosts/rust-daemon",
  "projectType": "application",
  "targets": {
    "dev": {
      "executor": "nx:run-commands",
      "options": {
        "cwd": ".",
        "command": "cargo run -p anima-daemon"
      }
    },
    "build": {
      "executor": "nx:run-commands",
      "options": {
        "cwd": ".",
        "command": "cargo build -p anima-daemon"
      }
    },
    "test": {
      "executor": "nx:run-commands",
      "options": {
        "cwd": ".",
        "command": "cargo test -p anima-daemon"
      }
    },
    "lint": {
      "executor": "nx:run-commands",
      "options": {
        "cwd": ".",
        "command": "cargo fmt --all --check"
      }
    }
  }
}
```

- [ ] **Step 5: Verify the daemon host works from the corrected boundary**

Run: `bun x nx run rust-daemon:build`  
Expected: PASS

Run: `bun x nx run rust-daemon:test`  
Expected: PASS

Run: `cargo test -p anima-daemon`  
Expected: PASS

- [ ] **Step 6: Re-run the workspace-dev tests**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: PASS for the flattened-daemon assertions.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml hosts/rust-daemon tools/workspace-dev/src/layout.spec.ts
git commit -m "refactor: flatten rust daemon host package"
```

### Task 4: Add Explicit Placeholder Nx Projects For Elixir And Python Hosts

**Files:**
- Create: `hosts/elixir-phoenix/project.json`
- Create: `hosts/python-service/project.json`
- Modify: `hosts/elixir-phoenix/README.md`
- Modify: `hosts/python-service/README.md`
- Modify: `tools/workspace-dev/src/layout.spec.ts`

- [ ] **Step 1: Extend the layout test to assert placeholder host projects exist**

Append to `tools/workspace-dev/src/layout.spec.ts`:

```ts
it('stores placeholder host projects for elixir and python', () => {
  expect(existsSync(repoPath('hosts', 'elixir-phoenix', 'project.json'))).toBe(true);
  expect(existsSync(repoPath('hosts', 'python-service', 'project.json'))).toBe(true);
});
```

- [ ] **Step 2: Run the workspace-dev tests to verify the placeholder-host assertion fails**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: FAIL because neither placeholder host directory has a `project.json` file yet.

- [ ] **Step 3: Create explicit placeholder Nx projects**

Create `hosts/elixir-phoenix/project.json`:

```json
{
  "$schema": "../../node_modules/nx/schemas/project-schema.json",
  "name": "elixir-phoenix",
  "root": "hosts/elixir-phoenix",
  "projectType": "application",
  "targets": {
    "dev": {
      "executor": "nx:run-commands",
      "options": {
        "command": "bun run tools/workspace-dev/src/placeholder-host.ts --host elixir"
      }
    },
    "build": {
      "executor": "nx:run-commands",
      "options": {
        "command": "bun run tools/workspace-dev/src/placeholder-host.ts --host elixir --mode build"
      }
    },
    "test": {
      "executor": "nx:run-commands",
      "options": {
        "command": "bun run tools/workspace-dev/src/placeholder-host.ts --host elixir --mode test"
      }
    },
    "lint": {
      "executor": "nx:run-commands",
      "options": {
        "command": "bun run tools/workspace-dev/src/placeholder-host.ts --host elixir --mode lint"
      }
    }
  }
}
```

Create `hosts/python-service/project.json` by copying the same shape and replacing `elixir` with `python`.

Update the placeholder README files so each one clearly says the host is registered in `workspace-dev` but not implemented yet.

- [ ] **Step 4: Re-run the workspace-dev tests**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: PASS

- [ ] **Step 5: Verify the placeholder hosts fail intentionally through Nx**

Run: `bun x nx run elixir-phoenix:dev`  
Expected: FAIL with a clear placeholder message

Run: `bun x nx run python-service:dev`  
Expected: FAIL with a clear placeholder message

- [ ] **Step 6: Commit**

```bash
git add hosts/elixir-phoenix hosts/python-service tools/workspace-dev/src/layout.spec.ts
git commit -m "feat: add explicit placeholder host projects"
```

### Task 5: Rewrite High-Signal Docs Around `core-rust`, `core-ts`, And `rust-daemon`

**Files:**
- Modify: `README.md`
- Modify: `docs/SDK_USAGE.md`
- Modify: `docs/PRD.md`
- Modify: `docs/package-maturity-scorecard.md`
- Modify: `packages/core-ts/README.md`
- Modify: `packages/memory/README.md`
- Modify: `hosts/rust-daemon/README.md`
- Modify: `packages/core-rust/README.md`

- [ ] **Step 1: Capture the stale wording that must disappear**

Run:

```bash
rg -n "canonical runtime lives in Rust under `hosts/rust-daemon`|packages/core/|canonical runtime core and daemon crates|hosts/rust-daemon is the canonical execution path for runtime, swarm, memory" README.md docs/SDK_USAGE.md docs/PRD.md docs/package-maturity-scorecard.md packages/core-ts/README.md packages/memory/README.md hosts/rust-daemon/README.md packages/core-rust/README.md
```

Expected: MATCHES showing the old core-vs-host wording and old `packages/core/` path language.

- [ ] **Step 2: Update the high-signal docs**

Make these content corrections:

- `README.md`
  - describe `packages/core-rust` as the reusable Rust runtime core
  - describe `hosts/rust-daemon` as the runnable Rust host
  - keep `packages/core-ts` as the TS core port
- `docs/SDK_USAGE.md`
  - replace links to `../hosts/rust-daemon/` as the runtime core with `../packages/core-rust/`
  - keep daemon operational commands under `hosts/rust-daemon`
- `docs/PRD.md` and `docs/package-maturity-scorecard.md`
  - separate reusable runtime ownership from host ownership
- `packages/core-ts/README.md`
  - explain that it is the TS core port under `packages/core-ts`
  - keep the package name `@animaOS-SWARM/core`
- `packages/core-rust/README.md`
  - explain that it contains Rust reusable runtime crates
- `hosts/rust-daemon/README.md`
  - keep only daemon-host-specific documentation
- `packages/memory/README.md`
  - stop calling `hosts/rust-daemon` the runtime core; describe it as one host that can use the reusable core

- [ ] **Step 3: Verify the stale wording is gone**

Run the same `rg` command from Step 1.  
Expected: either zero matches or only historical references outside the files listed above.

- [ ] **Step 4: Run focused smoke checks after the doc/config sweep**

Run: `bun x nx test workspace-dev --runInBand`  
Expected: PASS

Run: `bun x nx build @animaOS-SWARM/ui`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add README.md docs/SDK_USAGE.md docs/PRD.md docs/package-maturity-scorecard.md packages/core-rust/README.md packages/core-ts/README.md packages/memory/README.md hosts/rust-daemon/README.md
git commit -m "docs: align core and host ownership language"
```

### Task 6: Final Live Dev Validation

**Files:**
- Modify only if validation exposes a real bug:
  - `tools/workspace-dev/src/hosts.ts`
  - `tools/workspace-dev/src/main.ts`
  - `tools/workspace-dev/src/main.spec.ts`
  - `apps/ui/vite.config.mts`

- [ ] **Step 1: Run the full automated verification set**

Run:

```bash
bun x nx test workspace-dev --runInBand
bun x nx build @animaOS-SWARM/core
bun x nx build @animaOS-SWARM/server
bun x nx run rust-daemon:build
bun x nx run rust-daemon:test
cargo test -p anima-core
cargo test -p anima-memory
cargo test -p anima-swarm
cargo test -p anima-daemon
```

Expected: PASS

- [ ] **Step 2: Run the live dev smoke test**

Run: `bun dev --host rust`

Expected:
- one process starts `rust-daemon`
- one process starts `@animaOS-SWARM/ui`
- UI process receives `UI_BACKEND_ORIGIN=http://127.0.0.1:8080`
- stopping the launcher cleanly tears both down

- [ ] **Step 3: If the live smoke test exposes a bug, add the smallest failing test first**

If `workspace-dev` or UI wiring fails, add or extend a focused test in:
- `tools/workspace-dev/src/main.spec.ts`
- `tools/workspace-dev/src/hosts.spec.ts`

Then implement the smallest fix in:
- `tools/workspace-dev/src/main.ts`
- `tools/workspace-dev/src/hosts.ts`
- or `apps/ui/vite.config.mts`

- [ ] **Step 4: Re-run the affected verification**

Run the smallest relevant subset first, then re-run the full automated verification set from Step 1.  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add tools/workspace-dev apps/ui/vite.config.mts
git commit -m "fix: finalize workspace dev host orchestration"
```

---

## Manual Review Notes

- Leave historical plan/spec artifacts that mention `packages/animaos-rs` or `packages/core/` alone unless they are current navigation targets or actively misleading in high-signal docs.
- The old TypeScript server in `apps/server` is intentionally out of scope for this plan.
- If the implementation hits the existing dirty `hosts/rust-daemon/*` edits, stop and coordinate rather than force-moving files on top of them.
