# Single Companion and VPS Deployment Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development for the independent deployment task and test-driven-development for meaningful behavior changes.

**Goal:** Replace the swarm-first web experience with one personal companion and provide a protected, persistent Docker deployment for one owner on a VPS.

**Architecture:** Retain the existing Rust runtime and delegation engine. Make the main agent the sole top-level conversation identity. Keep tools, activity, schedules, and settings accessible without an operations console or team roster. Serve the existing web build with a reverse proxy alongside the daemon, using persistent storage and protected ingress. Preserve existing agents and all user data.

**Tech Stack:** React, TypeScript, Nx/Bun, existing Rust daemon, Docker Compose, reverse proxy.

## Scope and acceptance

- One primary companion, chat-first navigation, no Team/Operations/agent-switcher front door.
- Existing background delegation remains available; no claim of new autonomous orchestration unless runtime support is verified.
- Simpler responsive chat appearance, honest online/offline state, settings and tool/activity access.
- VPS package includes persistent daemon state, restart policy, authenticated ingress, health checks where supported, streaming proxy support, and setup/backup/recovery documentation.
- No customer billing, multi-tenant service, new channel integrations, deleted runtime data, or purchased/deployed infrastructure.
- Test UI behavior and production web build. Validate deployment configuration and Docker runtime when locally available. Explicitly distinguish static checks from live VPS validation.

## Tasks

- [x] Inspect shell/chat/onboarding and existing daemon deployment contracts.
- [x] Add failing tests for chat-first single-companion navigation and main-agent selection.
- [x] Simplify shell, onboarding copy, chat styling, and delegation presentation without deleting backend capabilities.
- [x] Package protected web + daemon VPS deployment and document installation, persistence, upgrades, backup and interrupted-task limits.
- [x] Verify web tests/build and deployment checks; review changes for regressions and unauthorized exposure.
- [x] Record exact checks, limitations, and handoff instructions.

## Validation commands

Discover resolved targets with `bun x nx show project @animaOS-SWARM/web --json` before running tests. Run the web test and build targets without cache. Deployment validation uses `docker compose --env-file <test-config> -f deploy/vps/compose.yaml config --quiet`, and an isolated local container smoke run. If Rust code is changed, run `bun x nx run rust-daemon:test --skipNxCache` with an isolated target when needed.

## Implementation notes

- Added runtime `spawn_helper` creation/reuse instead of requiring a preconfigured roster. Helpers stay behind one companion identity; current parent permissions are enforced, recursive delegation is blocked, and process tools are excluded until their lifetime can be safely cancelled.
- Helpers have a four-start allowance per parent run, four reusable slots, and bounded model/tool-turn settings. A cooperative timeout is not an external-effect rollback or crash checkpoint.
- Added `ANIMA_PUBLIC_BASE_URL` so cloud Google/Microsoft callback registration and token exchange use the same public HTTPS origin; local defaults remain unchanged.
- Setup does not silently select the mock adapter or a potentially absent local Ollama server. Keyless/test providers remain explicit choices.
- Existing agents and old reusable components are preserved. No production credentials, live model calls, remote VPS provisioning, billing, Discord, or WhatsApp implementation is included.

## Verification record

- Final web unit suite: 374 passed across 42 files. Web production build and type-check passed.
- Final combined Chromium browser run: 16/16 passed with simulated APIs, including desktop/mobile companion flows and the four migrated legacy specs. No tests disabled. Firefox/WebKit and live provider calls were not part of this run.
- Final required Rust suite: 1,037 passed, zero failed, five optional tests ignored.
- Gateway policy suite: five tests, 47 assertions passed. Compose synthetic-secret configuration validated without exposing real credentials.
- Both exact Dockerfiles built successfully. Clean Linux builds required generating the SDK before web compilation and Debian trixie for ONNX's glibc C23 symbols.
- Isolated container smoke passed readiness, real daemon API bootstrap, SQLite memory, actual Rust keyring dummy credential storage, authenticated proxy/static app delivery, restart restoration, and full container recreation against the same named volumes. No published ports, real credentials or model calls. Only labeled test containers/volumes removed; built images retained.
- Real provider consent/inference, public TLS, VPS reboot and off-site backup restore remain deployment acceptance checks, not claims from local tests.
