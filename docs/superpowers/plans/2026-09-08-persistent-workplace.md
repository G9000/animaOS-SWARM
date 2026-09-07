# Persistent Workplace Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development for bounded implementation tasks and review. User has approved execution; proceed without an additional design approval gate.

**Goal:** Deliver a grounded agent-workplace scaffold with runtime fixes, honest capability discovery, and a usable web entry point.

**Architecture:** Keep reusable contracts in existing core packages, execution in the Rust daemon, transport in SDK, and presentation in web. Extend existing capability and memory infrastructure instead of duplicating it.

**Tech Stack:** Rust, Axum, existing anima-core/anima-memory, TypeScript SDK, React, Nx/Bun.

## Tasks

- [x] Audit runtime persistence, schedules, messaging, authority, memory, and tools; write `docs/design/2026-09-08-runtime-foundation-audit.md` with exact code evidence and gaps.
- [x] Fix bounded, demonstrated persistence/authority/communication defects under `hosts/rust-daemon/src`, with regression tests. Memory rejection/atomic replacement defects fixed; existing authority/communication bounds preserved. General durable workflow execution remains unwired.
- [x] Add read-only owner-authorized capability discovery at `/api/capabilities`, based on the actual daemon tool registry; distinguish registered native tools from planned modules. Add SDK types and method. Reuse existing core manifest boundary in extension documentation.
- [x] Rework `apps/web/src/components/WorkspaceDashboard.tsx`, onboarding presentation, and workspace navigation into an outcome-focused workplace. Add a capability view consuming the daemon inventory. Preserve existing setup/resume and permissions behavior. This is a presentation increment, not replacement of every screen.
- [x] Document the extension contract and memory decision, including concrete missing work and startup acceptance scenario.
- [x] Review integrated changes and execute `bun x nx run rust-daemon:test --skipNxCache`, web test/build/typecheck, SDK test/build/typecheck, and `git diff --check`. Final Rust pass: 997 passed, 5 ignored. Web: 336 full-suite tests, then 4 focused capabilities tests after the final UI adjustment; SDK: 37 tests. Builds/typechecks passed. Independent review resolved its one postcommit persistence finding. See audit/handoff for scope and platform limits.

## Bounded verification policy

Test meaningful behaviors: authorization before inventory access, inventory/registered-tool agreement, delegated authority limits, restart state semantics, UI error/offline handling, onboarding resume. One red/green cycle per regression; one integrated pass, then rerun only checks justified by new fixes. No endless polishing or repeated unchanged suites.
