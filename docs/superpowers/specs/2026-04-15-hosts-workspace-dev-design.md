# Hosts Workspace And Unified Dev Design

**Date:** 2026-04-15
**Status:** Approved

---

## Goal

Restructure the repository so backend implementations live under `hosts/` and become first-class Nx projects, with one consistent local development entrypoint that starts the selected host plus the web UI.

This design is about repository boundaries and developer workflow. It does not change the ADR decision that `anima-core` is host-agnostic. It changes how host implementations are organized and run inside this repo.

---

## Decision Summary

The repository will adopt this ownership model:

- `hosts/*` contains backend host implementations
- `apps/*` contains user-facing application surfaces
- `packages/*` contains reusable libraries, SDKs, and shared contracts

The first concrete migration is:

- move `packages/animaos-rs` to `hosts/rust-daemon`
- make `hosts/rust-daemon` a first-class Nx project
- add placeholder host projects for future Elixir and Python implementations
- add a workspace-level Nx orchestration project that starts the selected host plus `apps/ui`
- expose one human-friendly entrypoint: `bun dev --host <name>`

The target backend choices are:

- `rust`
- `elixir`
- `python`

Only Rust is expected to be fully implemented in phase 1. Elixir and Python are explicit repo-level citizens from the beginning so the structure does not need to be rethought later.

---

## Repository Shape

The repo should converge on this structure:

```text
hosts/
  rust-daemon/
  elixir-phoenix/
  python-service/

apps/
  ui/
  tui/

packages/
  sdk/
  cli/
  core/
  swarm/
  memory/
  tools/
  ...
```

Interpretation:

- `hosts/*` are backend process boundaries that expose runtime capabilities to clients
- `apps/*` are operator-facing or end-user-facing surfaces
- `packages/*` are imported and reused; they are not deployment boundaries

This gives the repository one honest place for Rust, Elixir, and Python backends without overloading `apps/` or hiding production backends inside `packages/`.

---

## Host Model

Each host is a backend implementation of the same broad role: expose runtime capabilities to the rest of the system.

Examples:

- `hosts/rust-daemon` -> Axum/Tokio host
- `hosts/elixir-phoenix` -> Phoenix/BEAM host
- `hosts/python-service` -> Python host

`hosts` is intentionally broader than "HTTP server" in the abstract, but in this repository it should be treated as the home for backend server/process implementations.

The host is the backend boundary. UI, TUI, SDK, and CLI should talk directly to the selected host. A second server above the host is not required by default.

An extra server or gateway is only justified later for specific concerns like:

- browser auth/session handling
- edge routing or caching
- multi-service aggregation
- public/private topology separation

Until one of those problems is concrete, the selected host is the backend.

---

## Nx Project Contract

Every host must be represented as an Nx project and should expose a consistent target contract where possible:

- `dev`
- `build`
- `test`
- `lint`

Not every language ecosystem will implement every target in exactly the same way, but the names should stay consistent so the workspace tooling stays predictable.

### Rust Host

`hosts/rust-daemon` becomes an Nx project even though it is built with Cargo.

Its Nx targets should be thin wrappers around Cargo commands, likely using `nx:run-commands`:

- `dev` -> `cargo run`
- `build` -> `cargo build`
- `test` -> `cargo test`
- `lint` -> `cargo fmt --check` and/or `cargo clippy` in a follow-up if desired

### Elixir Host

`hosts/elixir-phoenix` will eventually expose the same target names through Phoenix/Mix commands.

### Python Host

`hosts/python-service` will eventually expose the same target names through Python tooling.

The point is not identical internals. The point is one workspace-level interface.

---

## Unified Dev Flow

The developer-facing command surface becomes:

```bash
bun dev --host rust
bun dev --host elixir
bun dev --host python
```

and an Nx-native equivalent:

```bash
nx run workspace-dev:dev -- --host rust
nx run workspace-dev:dev -- --host elixir
nx run workspace-dev:dev -- --host python
```

`workspace-dev` is the proposed workspace orchestration project name. The exact project name can change during implementation, but the orchestration responsibility should exist as a dedicated Nx project rather than as ad hoc shell glue.

### What The Dev Command Starts

For phase 1, the unified dev command starts:

- the selected host
- `apps/ui`
- any env wiring required for `apps/ui` to target the selected host

It does not auto-launch the TUI. `apps/tui` remains a manual client.

### UI Wiring Rule

`apps/ui` must stop assuming a fixed backend like `localhost:3000` when launched through the workspace dev flow.

Instead, the dev orchestrator must supply the backend origin explicitly through environment variables such as:

- `UI_BACKEND_ORIGIN`
- any future event-stream or websocket origin variables if needed

The orchestrator owns the host-specific port map.

---

## Host Registry

The workspace dev orchestration must read from one central host registry rather than hardcoding conditional logic in multiple files.

That registry should define, at minimum:

- host key: `rust`, `elixir`, `python`
- Nx project name for that host
- backend base URL/port
- any host-specific env required for development
- implementation status

Example shape:

```ts
type HostKey = 'rust' | 'elixir' | 'python';

interface HostDefinition {
  projectName: string;
  baseUrl: string;
  status: 'ready' | 'placeholder';
  env?: Record<string, string>;
}
```

This makes host selection a data problem instead of spreading backend assumptions through scripts, UI config, docs, and tests.

---

## Migration Plan

### Phase 1: Introduce `hosts/` and Rust Host Parity

- move `packages/animaos-rs` to `hosts/rust-daemon`
- update Cargo manifest-path references, docs, scripts, and tests
- define `hosts/rust-daemon` as an Nx project
- add `dev`, `build`, and `test` targets for Rust
- add the workspace orchestration project
- add `bun dev --host rust`
- update `apps/ui` to read backend origin from orchestrated env

Exit criteria:

- `bun dev --host rust` starts the Rust host and UI together
- `nx run workspace-dev:dev -- --host rust` does the same
- Rust build/test commands still work directly in the moved host directory

### Phase 2: Add Future Host Stubs

- create `hosts/elixir-phoenix`
- create `hosts/python-service`
- register them in the host registry
- expose Nx `dev` targets even if initially placeholder-only

Exit criteria:

- the workspace accepts `--host elixir` and `--host python`
- unsupported hosts fail clearly and intentionally rather than through missing-path errors

### Phase 3: Resolve Existing TypeScript Server Boundary

`apps/server` should be handled as an explicit follow-up decision, not implicitly kept forever.

Possible outcomes:

- remove it once UI parity exists against the selected host API
- keep it only as a dev sandbox and rename it accordingly
- replace it with a narrower proxy/BFF only if a real browser-facing need appears

This prevents the repository from continuing to carry two overlapping backend stories by accident.

---

## Risks

Main risks:

- large path churn from moving `packages/animaos-rs`
- broken Cargo manifest references
- broken test fixtures and docs that mention old paths
- UI/backend assumptions remaining split across config and tests
- false Nx consistency if host projects are added without a real contract

Mitigations:

- do the path move and dev orchestration in one coordinated change
- centralize host metadata in one registry
- sweep hardcoded path and port references immediately after the move
- keep the target contract intentionally small: `dev`, `build`, `test`, `lint`

---

## Success Criteria

This design is successful when:

- `hosts/` is the clear home for backend implementations
- `apps/` remains the clear home for UI/TUI surfaces
- `packages/` remains the clear home for reusable libraries
- `hosts/rust-daemon` is a first-class Nx project
- the workspace has one stable development entrypoint: `bun dev --host <name>`
- the workspace has one Nx-native orchestration entrypoint for the same flow
- `apps/ui` can target whichever host is selected without hardcoded local backend assumptions
- adding an Elixir host no longer requires repo-level reorganization

---

## Explicit Non-Goals

This design does not require:

- changing the ADR boundary of `anima-core`
- automatically launching the TUI during `dev`
- introducing a gateway or proxy above the selected host
- making Elixir or Python fully implemented in the same change as the repo restructure

It only requires that the repository stop hiding backend implementations in misleading locations and expose one clean development flow for the chosen host.
