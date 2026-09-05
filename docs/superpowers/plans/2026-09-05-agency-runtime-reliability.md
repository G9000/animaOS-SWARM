# Agency Runtime Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. User selected inline implementation. Steps use checkbox syntax for tracking.

**Goal:** Deliver the first tested reliability phase of the approved persistent-agency direction.

**Architecture:** Keep strategy-neutral admission/accounting in anima-swarm. Wrap daemon swarm model calls to publish live usage and check the shared budget. Use a bounded daemon scheduler task set with per-agent exclusion and conservative interruption reconciliation.

**Tech Stack:** Rust, Tokio, existing ModelAdapter/CoordinatorAgentShell interfaces, existing control-plane snapshot persistence, Bun/Nx.

---

### Task 1: Core swarm accounting and admission

Files: `packages/core-rust/crates/anima-swarm/src/coordinator.rs`, `src/types.rs`, `tests/coordinator.rs`, `README.md`.

- [x] Write regression tests: completed prewarmed manager+worker total is 14 rather than 6; zero budget executes no shell; consumed budget prevents a later shell; a new dispatch resets usage.
- [x] Run `bun x nx run core-rust:test --skipNxCache` and confirm the regressions fail for the expected behavior.
- [x] Add a weak-reference budget-check callback to factory contexts, guard coordinator-owned run references, and preserve sealed completed totals. Saturate aggregate addition. Keep callbacks outside registry/state locks.
- [x] Rerun tests and document observed-budget/overshoot semantics.

### Task 2: Daemon model-boundary budget checks

Files: `hosts/rust-daemon/src/state/swarm_runtime.rs` and focused tests in that module.

- [x] Add deterministic adapter tests proving an exhausted shared budget blocks a follow-up provider request and usage is published immediately after a response.
- [x] Verify failures through the daemon test target.
- [x] Wrap the existing adapter with the context budget checker and the shell's shared usage counter. Preserve reset, provider identity, streaming semantics when applicable, and errors.
- [x] Rerun the tests; do not call a paid provider.

### Task 3: Bounded background scheduler and interrupted occurrences

Files: `hosts/rust-daemon/src/schedules.rs`, scheduler tests, and restart initialization only if necessary.

- [x] Add tests with gated model adapters proving unrelated agents progress, same-agent jobs do not overlap, and the task bound is respected.
- [x] Add restore tests: unresolved latest claims are disabled and marked `schedule_run_interrupted`; completed and never-claimed schedules are unchanged. No uncertain actions are replayed.
- [x] Verify these regressions fail before changing scheduler behavior.
- [x] Maintain a bounded task set, select one due schedule per available agent, persist claims before spawning, and continue scanning while jobs execute. Drain admitted work on shutdown. Report tick errors instead of silently discarding them.
- [x] Clear the old outcome on claim; reconcile unresolved occurrences under the existing control-plane transaction and rollback if persistence fails. Keep startup admission closed and retry reconciliation until its persistence succeeds. Add a persistence-failure test proving no model/tool execution while this gate is closed. Retain per-agent exclusion through the owned run's final durable commit.
- [x] Rerun all relevant tests.

### Task 4: Review and verification

- [x] Run `$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; bun x nx run rust-daemon:test --skipNxCache` (includes core tests).
- [x] Run `bun x nx run rust-daemon:lint --skipNxCache`; fix only touched-file formatting if unrelated baseline formatting fails.
- [x] Review changes independently and resolve substantive findings.
- [x] Record tested scope and remaining mission-engine integration. Leave all work inline and uncommitted; do not restart the live daemon or modify its persisted workspace.

## Initial phase verification and delivery

- Spec/plan and code independently reviewed; no remaining substantive findings.
- Final `rust-daemon:test --skipNxCache` passed, including core-rust dependency: 854 tests passed, four environment-dependent tests ignored.
- `core-rust:lint --skipNxCache`, targeted rustfmt checks on every touched Rust file, and `git diff --check` passed.
- Global `rust-daemon:lint` still fails on pre-existing formatting in untouched app/calendar/tool files. Those files were not reformatted.
- Fixed pre-existing calendar test-only compilation fixtures and registry expectations so the daemon suite could run. Production calendar behavior was not changed.
- Added regressions for cleanup-time and stale-poll accounting races, streaming preservation, legacy outcomes, and orphaned claims after runtime storage failure. Reconciliation now runs before every admission scan and excludes active jobs.
- Existing UI work preserved. No real provider calls, live workspace changes, daemon restart, commits, or pushes.

## Remaining approved direction

This delivery completes the reliability phase only. Persistent mission/task dependency modeling, a durable host ExecutionStore adapter, integration with DurableAgentEngine, cooperative mission pause/cancel/resume, and mission-control UI remain unimplemented. Interrupted legacy runs are paused/reported, not checkpoint-resumed.

## Follow-up: Rust core, SDK, and daemon assurance

User requested cross-layer verification after the initial phase. Changes remain inline and uncommitted; the live daemon and UI work were preserved.

- [x] Add `anima-schedule` to core-rust build/test/lint targets (29 previously omitted tests).
- [x] Reproduce and fix swarm cancellation at HTTP timeout/disconnect. An owned worker retains global admission and a per-swarm lock through durable final commit and completion publication.
- [x] Reproduce and fix pre-run persistence taking an Idle coordinator snapshot instead of the intended Running marker. Persist stored snapshots; roll back to the previous stored state on commit failure.
- [x] Test timeout, aborted caller, pre-execution Running persistence/restart, initial save failure, queued same-swarm dispatch, and failed final commit.
- [x] Fix SDK SSE abort-listener cleanup on connection/reader acquisition failure, with red/green regressions.
- [x] Correct SDK response types for server-owned descriptors, nested custom settings, messages, and nullable results. Add a CLI runtime-result adapter and repair stale test fixtures exposed by consumer typechecking.
- [x] Separate SDK integration compilation/startup deadlines; run the daemon binary directly in a temporary workspace with an allowlisted environment and local provider stub. Preserve host-owned provider credentials rather than permitting per-agent credential overrides.
- [x] Fix the real-HTTP test's cross-test persistence/environment race; five consecutive health-suite runs passed.
- [x] Resolve pre-existing global Rust formatting failures (format-only calendar/app/tool edits).
- [x] Independent follow-up review found no new substantive ownership/persistence issues.

Fresh validation:

- `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache`: **889 passed**, four ignored (three Postgres tests and one model-download test). Core includes all six reusable crates; daemon includes six new swarm reliability regressions.
- `bun x nx run-many -t build lint -p core-rust rust-daemon --skipNxCache`: passed with the same isolated Cargo target directory.
- `bun x nx run-many -t test build typecheck -p @animaOS-SWARM/cli @animaOS-SWARM/sdk --skipNxCache`: passed, **15 SDK** and **111 CLI** tests, including real-daemon HTTP/SSE integration and compile-time contract assertions.
- `git diff --check`: passed.

Remaining limits: interrupted interactive work is not checkpoint-resumed or drained on daemon shutdown; persisted Running intent supports conservative interrupted recovery. Existing dead-code and transitive `nom` future-compatibility warnings remain. No scratchboard version exists in this checkout; this plan records completion instead.
