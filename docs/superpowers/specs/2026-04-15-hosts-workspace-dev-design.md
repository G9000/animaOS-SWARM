# Core Ports, Hosts, And Unified Dev Design

**Date:** 2026-04-18
**Status:** Approved

---

## Goal

Restructure the repository so the host-agnostic engine ports live under `packages/`, runnable backend implementations live under `hosts/`, and local development starts the selected host plus the web UI through one consistent command.

This revision replaces the earlier incorrect assumption that the full Rust workspace should move under `hosts/`. That would have mixed reusable engine code with runnable backend processes. The correct boundary is:

- reusable core ports in `packages/`
- runnable hosts in `hosts/`

---

## Decision Summary

The repository will adopt this ownership model:

- `packages/core-rust` contains the Rust core port and its reusable engine crates
- `packages/core-ts` contains the TypeScript core port
- `hosts/*` contains runnable backend implementations only
- `apps/*` contains operator-facing or end-user-facing application surfaces
- `packages/*` continues to contain reusable SDKs, libraries, and shared code

The first restructuring pass will:

- rehome the reusable Rust crates into `packages/core-rust`
- rehome the Rust daemon into `hosts/rust-daemon`
- rename or reframe the current TypeScript core package as `packages/core-ts`
- add placeholder host directories for Elixir and Python
- add one host-selecting dev entrypoint: `bun dev --host <name>`
- add an Nx-native orchestration target for the same flow

The old TypeScript server is not removed in this pass. It stays in place while the repo boundaries are cleaned up.

---

## Repository Shape

The repo should converge on this shape:

```text
packages/
  core-rust/
  core-ts/
  sdk/
  cli/
  memory/
  swarm/
  tools/
  ...

hosts/
  rust-daemon/
  elixir-phoenix/
  python-service/

apps/
  ui/
  tui/
  server/        # kept for now, follow-up decision later
```

Interpretation:

- `packages/core-rust` and `packages/core-ts` are sibling engine ports
- `hosts/*` are runnable wrappers around a core port
- `apps/*` are user-facing surfaces, not backend ownership boundaries
- `packages/*` are reusable and importable, not deployment units

This matches the architectural intent: the core is host-agnostic, while hosts are deployment-specific wrappers around it.

---

## Boundary Rules

### `packages/core-rust`

`packages/core-rust` owns reusable Rust engine code only.

Examples:

- `anima-core`
- `anima-memory`
- `anima-swarm`
- shared Rust traits, primitives, and runtime libraries

It must not be treated as a backend process boundary.

### `packages/core-ts`

`packages/core-ts` is the TypeScript port of the core.

It remains a reusable package boundary, not a host. It can contain TypeScript-side core/runtime concepts, contracts, and library code. It does not need to imply a TypeScript backend host exists.

### `hosts/*`

`hosts/*` are runnable backend implementations.

Examples:

- `hosts/rust-daemon`
- `hosts/elixir-phoenix`
- `hosts/python-service`

Each host wraps one core port and exposes runtime capabilities over whatever transport it chooses. The host is the backend process boundary.

That means:

- reusable engine code does not belong in `hosts/*`
- deployment-specific code does not belong in `packages/core-*`

---

## Rust Layout

The Rust side should be split between reusable crates and the runnable daemon.

Target shape:

```text
packages/core-rust/
  crates/
    anima-core/
    anima-memory/
    anima-swarm/

hosts/rust-daemon/
  Cargo.toml
  src/
  tests/
```

To support this cleanly, the repository should adopt a repo-root Cargo workspace so Rust packages and Rust hosts can be siblings without awkward cross-directory ownership.

Expected Rust workspace membership:

- `packages/core-rust/crates/anima-core`
- `packages/core-rust/crates/anima-memory`
- `packages/core-rust/crates/anima-swarm`
- `hosts/rust-daemon`

This is the cleanest way to express:

- reusable Rust engine crates in `packages/`
- runnable Rust backend in `hosts/`

without pretending one owns the other.

---

## Host Model

The host model is simple:

- core is agnostic
- host is specific

Each host is a runnable wrapper around a core port.

Examples:

- `hosts/rust-daemon` wraps `packages/core-rust`
- a future TypeScript host, if ever needed, would still live under `hosts/`, not under `packages/core-ts`
- `hosts/elixir-phoenix` and `hosts/python-service` are peer host implementations, not alternate homes for the core itself

This keeps the architecture honest:

- the core is portable
- the host is replaceable
- the repo structure reflects that separation

---

## Nx Project Contract

Every runnable host must be a first-class Nx project and should expose a consistent target contract where practical:

- `dev`
- `build`
- `test`
- `lint`

The internals will differ by ecosystem, but the target names should stay aligned so workspace tooling remains predictable.

### `hosts/rust-daemon`

`hosts/rust-daemon` becomes an Nx project with targets that wrap Cargo commands.

Expected shape:

- `dev` -> `cargo run -p anima-daemon`
- `build` -> `cargo build -p anima-daemon`
- `test` -> `cargo test -p anima-daemon`
- `lint` -> `cargo fmt --check` and/or `cargo clippy` in a follow-up

### `hosts/elixir-phoenix`

Future Nx targets should wrap Mix/Phoenix commands.

### `hosts/python-service`

Future Nx targets should wrap Python tooling.

The goal is one workspace-facing contract for runnable backends.

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

`workspace-dev` is the proposed workspace orchestration project name. The exact name can change during implementation, but the orchestration role should exist as a dedicated Nx project.

### What `dev` Starts

For this restructuring track, the unified dev command starts:

- the selected host
- `apps/ui`
- any env wiring needed for `apps/ui` to target that host

It does not auto-launch the TUI. `apps/tui` remains a manual client.

### UI Wiring Rule

`apps/ui` must not hardcode a backend assumption like `localhost:3000` in the normal workspace dev flow.

Instead, the orchestrator supplies:

- `UI_BACKEND_ORIGIN`
- any future host-specific event origin vars if needed

The workspace dev layer owns host selection and host-to-UI wiring.

---

## Host Registry

Host selection must come from one central registry rather than from scattered script logic.

That registry should define:

- host key: `rust`, `elixir`, `python`
- Nx project name
- backend base URL/port
- implementation status
- optional host-specific env

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

This keeps host orchestration data-driven and prevents backend assumptions from leaking into UI config, docs, tests, and scripts.

---

## Migration Plan

### Phase 1: Correct The Repo Boundaries

- move reusable Rust engine crates from the current Rust workspace into `packages/core-rust`
- move the Rust daemon into `hosts/rust-daemon`
- create a repo-root Cargo workspace covering both `packages/core-rust` and `hosts/rust-daemon`
- rename or reframe the current TypeScript core package as `packages/core-ts`
- update scripts, docs, tests, and path references

Exit criteria:

- reusable Rust code lives only in `packages/core-rust`
- the runnable Rust backend lives only in `hosts/rust-daemon`
- `packages/core-ts` exists as the TypeScript core port

### Phase 2: Add First-Class Host Projects

- define `hosts/rust-daemon` as an Nx project
- create placeholder Nx host projects for `hosts/elixir-phoenix` and `hosts/python-service`
- register all hosts in the host registry

Exit criteria:

- all hosts are discoverable through one host registry
- Rust host is runnable through Nx
- Elixir and Python hosts have intentional placeholders rather than missing-path failures

### Phase 3: Add Unified Dev Orchestration

- create the `workspace-dev` orchestration project
- add `bun dev --host <name>`
- wire `apps/ui` to the selected host through env

Exit criteria:

- `bun dev --host rust` starts the selected host plus UI
- `nx run workspace-dev:dev -- --host rust` does the same
- host selection is controlled by the workspace dev layer rather than hardcoded backend assumptions

### Phase 4: Leave `apps/server` Alone For Now

The existing TypeScript server is not part of this restructuring decision.

For this pass:

- do not remove it
- do not move it
- do not use it to define the core-vs-host boundary

It can be evaluated later once the repo layout is corrected.

---

## Risks

Main risks:

- path churn from splitting the current Rust workspace into `packages/core-rust` and `hosts/rust-daemon`
- broken Cargo path references during the workspace transition
- broken docs and tests that still point at `packages/animaos-rs`
- accidental mixing of `core-ts` with host responsibilities
- workspace dev flow staying coupled to old server assumptions

Mitigations:

- move paths and workspace wiring in one coordinated change
- adopt one Cargo workspace at the repo root
- add one host registry instead of hardcoded conditionals
- keep `apps/server` out of scope for this pass to avoid mixing structural cleanup with backend removal

---

## Success Criteria

This restructuring is successful when:

- `packages/core-rust` is the clear home for reusable Rust engine code
- `packages/core-ts` is the clear home for the TypeScript core port
- `hosts/*` is the clear home for runnable backend implementations
- `hosts/rust-daemon` is a first-class Nx project
- the workspace has one stable dev entrypoint: `bun dev --host <name>`
- the workspace has one Nx-native orchestration target for the same flow
- the old TypeScript server is no longer distorting the core-vs-host boundary
- adding Elixir or Python hosts later does not require another repo-level reorganization

---

## Explicit Non-Goals

This restructuring does not require:

- removing `apps/server`
- deciding the final fate of the old TypeScript backend
- changing the ADR boundary of the core
- auto-launching the TUI during `dev`
- adding a gateway above the selected host

It only requires that the repository reflect the architectural truth:

- core ports are reusable packages
- hosts are runnable backends
