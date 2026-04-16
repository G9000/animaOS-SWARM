# Package And Surface Maturity Scorecard

Last updated: 2026-04-10

This scorecard tracks parity between the main TypeScript-facing packages and the currently active application/runtime surfaces:

- `@animaOS-SWARM/cli`
- `@animaOS-SWARM/core`
- `@animaOS-SWARM/memory`
- `@animaOS-SWARM/sdk`
- `@animaOS-SWARM/swarm`
- `@animaOS-SWARM/tools`
- `@animaOS-SWARM/tui`
- `@animaOS-SWARM/server`
- `@animaOS-SWARM/ui`
- `@animaOS-SWARM/ui-e2e`
- `hosts/rust-daemon`

The goal is not to make every surface identical. The goal is to hold each one to the same quality bar for its role.

## Target Bar

A surface is considered mature when it has all of the following:

1. Clear role statement in its README.
2. Public surface documented at a high level.
3. At least one quick usage example in the README.
4. Explicit build, static-validation, and test commands where the surface exposes them.
5. Direct tests for its important internal seams, not only end-to-end behavior.
6. At least one boundary validation path that reflects how consumers actually use it.
7. Fresh build and static-validation passes for the targets that surface actually owns.

For thin apps or harnesses, the direct seam can be a smoke test around the primary entrypoint rather than a deep internal unit suite.

## Current Snapshot

| Package  | Role clarity | README example | Direct seam tests | Consumer-boundary validation | Fresh build | Fresh static validation | Current validated tests | Maturity read  |
| -------- | ------------ | -------------- | ----------------- | ---------------------------- | ----------- | ----------------------- | ----------------------- | -------------- |
| `cli`    | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 59 tests / 6 files      | Parity bar met |
| `core`   | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 18 tests / 6 files      | Parity bar met |
| `memory` | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 87 tests / 6 files      | Parity bar met |
| `sdk`    | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 10 tests / 2 files      | Parity bar met |
| `swarm`  | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 30 tests / 2 files      | Parity bar met |
| `tools`  | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 58 tests / 2 files      | Parity bar met |
| `tui`    | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 102 tests / 12 files    | Parity bar met |

## App And Runtime Snapshot

| Surface      | Role clarity | README example | Direct seam tests | Consumer-boundary validation | Fresh build | Fresh static validation | Current validated tests | Maturity read  |
| ------------ | ------------ | -------------- | ----------------- | ---------------------------- | ----------- | ----------------------- | ----------------------- | -------------- |
| `server`     | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 8 tests / 2 files       | Parity bar met |
| `ui`         | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 7 tests / 3 files       | Parity bar met |
| `ui-e2e`     | Yes          | Yes            | Role-specific     | Yes                          | N/A         | Pass                    | 24 tests / 2 specs      | Parity bar met |
| `animaos-rs` | Yes          | Yes            | Yes               | Yes                          | Pass        | Pass                    | 166 tests / workspace   | Parity bar met |

## Notes By Package

### `cli`

Strengths:

- Clear operator-facing role as the `animaos` command package.
- Broad command coverage for `run`, `chat`, `launch`, `agents`, and agency generation.
- README now includes a quick command-line example and public-surface summary.

Remaining gap:

- Its remaining difference from other packages is role, not maturity bar. The CLI is the operator-facing command surface, so its boundary tests center on command parsing and daemon delegation rather than embedded API use.

### `core`

Strengths:

- Clear role as shared TypeScript contracts and support utilities.
- Direct tests for runtime helpers, daemon-health messaging, and builder helpers.
- README now documents scope and includes a quick example.

Remaining gap:

- Its remaining difference from `sdk` is role, not maturity bar. `core` is still a support package rather than a primary product surface.

### `memory`

Strengths:

- Clear role as the TypeScript memory utility layer.
- Strong direct test depth across ranking, storage, provider, evaluator, and manager behavior.
- README now documents scope and includes a quick usage example.

Remaining gap:

- Its remaining difference from other packages is role, not maturity bar. Production memory authority still lives in the Rust workspace.

### `sdk`

Strengths:

- Strongest package boundary.
- Real daemon integration tests validate health, memories, agents, swarm runs, and SSE events.
- Error model is explicit and externally consumable.

Remaining gap:

- Lowest remaining risk area is breadth of examples, not core capability.

### `swarm`

Strengths:

- Clear role as the TypeScript swarm orchestration utility layer.
- Strong coordinator coverage plus a package-root swarm helper smoke path.
- README now documents scope and includes a runnable coordinator example.

Remaining gap:

- Its remaining difference from `sdk` is role, not maturity bar. Canonical production execution still belongs to Rust.

### `tools`

Strengths:

- Clear role as the TypeScript tool execution and policy layer.
- Strong behavioral coverage around the built-in tool set, plus a root smoke path through exported registry and execution helpers.
- README now documents scope and includes a quick executor example.

Remaining gap:

- Its remaining difference from other packages is role, not maturity bar. Canonical production tool execution still belongs to Rust.

### `tui`

Strengths:

- Richest behavioral surface and deepest test suite.
- Direct tests now exist for extracted seams such as command derivation, displayed-agent derivation, and daemon-state control.
- README now states role, test command, and a quick embedding example.

Remaining gap:

- Its remaining difference from `sdk` is that the boundary validation is local consumer-style embedding rather than external daemon integration. That is appropriate for its role.

## Notes Beyond Packages

### `server`

Strengths:

- Clear role as the lightweight local HTTP app boundary.
- `/api/health` can now boot without provider credentials because model-adapter creation is deferred until execution paths need it.
- Route helpers now validate and narrow agent, swarm, and task payloads before execution, so the production build and HTTP boundary agree on accepted input.
- Direct smoke coverage now targets `createServer()`, the default `/api/health` path, and focused `/ws` event broadcasting for agent and swarm lifecycle updates.
- A deterministic mock model adapter can be selected for local boundary checks, so agent task execution is testable without external provider calls.
- README now documents how to build, serve, and validate the app.

Remaining gap:

- This remains a thin development-facing surface, not the canonical daemon boundary.

### `ui`

Strengths:

- Clear role as the current browser entrypoint.
- Direct render smoke coverage now locks the operator dashboard shell instead of the old starter page.
- The browser surface now exposes a real control grid for agents, swarms, search, and health while remaining usable in preview mode if the server is offline.
- A directly tested live-event reducer applies `/ws` agent and swarm events onto the browser snapshot without depending on full-page refreshes.
- Search result formatting now has a direct seam, so task-history hits and indexed document hits render with structured labels and plain-text excerpts instead of raw JSON fallback cards.
- README now documents how to build, serve, and validate the app.

Remaining gap:

- The next product-depth gap is richer operator workflows on top of the live event stream, not the absence of a browser UI.

### `ui-e2e`

Strengths:

- Clear role as the browser consumer-boundary harness for the web app.
- Playwright smoke now covers preview-mode shell rendering plus live websocket-driven agent creation, swarm creation, scoped agent task-output checks, structured swarm task completion output, and browser search workflows for task history, indexed documents, and multi-chunk document excerpts.
- README now documents the Nx e2e entrypoint.

Remaining gap:

- This surface is intentionally thin. Its job is boundary validation, not internal seam depth.

### `animaos-rs`

Strengths:

- Clear role as the canonical runtime workspace.
- Existing crate and integration tests already cover runtime, memory, swarm, and daemon behavior directly.
- README now documents workspace layout plus build, test, and quick daemon run commands.

Remaining gap:

- The remaining work here is roadmap depth and crate-level docs, not basic maturity posture.

## Recommended Next Parity Work

1. Keep the scorecard updated whenever a package or active app surface gains or loses a boundary, example, dedicated seam test, or build/static-validation target.
2. Expand README examples if a surface adds new major exported or operator-facing capabilities.
3. Treat future gaps as role-specific: daemon protocol depth belongs in `sdk`, operator workflow depth belongs in `tui`, browser workflow depth belongs in `ui` and `ui-e2e`, thin local HTTP app behavior belongs in `server`, and canonical execution/runtime depth belongs in `hosts/rust-daemon`.
