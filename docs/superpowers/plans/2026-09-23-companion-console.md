# Companion Console Implementation Plan (master)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the OpenClaw/Hermes-class companion console described in the spec: sessions, live runs, parallel sessions, approvals, skills, automations, memory editor, usage/logs/health, attachments, voice, and AI titles, on the Rust daemon and the web console.

**Architecture:** Daemon-owned contracts in `hosts/rust-daemon`, reusable engine logic in `packages/core-rust` (`anima-core`, `anima-model-adapters`, `anima-memory`), typed clients in `packages/sdk`, UI in `apps/web`. Current state lives in the control-plane snapshot with a bounded message tail; history lives in a new row-based history store fed by an outbox. One SSE stream per companion carries all live events.

**Tech Stack:** Rust (tokio, axum 0.8, serde, rusqlite 0.32 bundled with FTS5, sqlx Postgres, utoipa, tracing-subscriber, cap-std, croner), TypeScript (React 19, Vite, Tailwind v4, Vitest, Testing Library, Playwright), Nx with Bun.

**Spec:** `docs/superpowers/specs/2026-09-23-companion-console-design.md` (read it before any task; section numbers below refer to it).

## Global Constraints

- Follow `AGENTS.md`: reusable engine code stays in `packages/*`; `anima-core` gains no HTTP framework, DB driver, or host runtime dependency; use `bun x nx ...` targets; do not touch `apps/server`.
- Only new third-party dependency allowed: `croner` (cron parsing) in `hosts/rust-daemon`. Reuse existing `sha2`, `base64`, `uuid`, `chrono-tz`, `cap-std`, `rusqlite`, `sqlx`, `tracing-subscriber`. Any other new dependency requires stopping and asking.
- New env vars, exact names: `ANIMAOS_RS_MAX_RUNS_PER_AGENT` (default 3), `ANIMAOS_RS_SESSION_EVENT_BUFFER` (default 1024), `ANIMAOS_RS_HISTORY_SQLITE_FILE` (default `history.sqlite` beside `ANIMAOS_RS_CONTROL_PLANE_FILE`).
- Session ids match `^[A-Za-z0-9._:-]{1,200}$`. New chat ids are `chat:<uuid-v4>`. Run ids are `run_<uuid-v4>`.
- Every new daemon route: reads call `state.local_owner.authorize_read(headers)` and set `Cache-Control: no-store`; mutations call `state.local_owner.authorize(headers)`; every route gets a `#[utoipa::path]` entry.
- Limits (spec §16) are constants in code, named once, and covered by tests: 8 queued runs per agent; 3 concurrent runs per agent (helpers 1); 32 KiB run text; 10 attachments; 1,024-event buffer; 16 subscribers; 50 ms / 512-byte delta flush; 2 KiB previews; context 60% of window, 32,000 fallback, 4 images; hot tail newest 200 per session, 24 h; 500 retained events; approvals 30 min (15 min Telegram-started); skills 32 KiB body, 50 indexed, 10 pending drafts; automations 20 per agent, 5-minute agent minimum, 50 history shown; logs 2,000 lines of 4 KiB; uploads 10 MiB images, 1 MiB text, 25 MiB documents; titles 2–6 words, 60 characters.
- Existing route shapes, the CLI, and the TUI must keep working. Pre-existing memory routes keep their current authorization.
- Nothing streamed is retracted; restarts never replay side effects; history failures never lose data.
- Commits use the repo's conventional style (`feat(daemon): ...`, `fix(daemon): ...`, `feat(web): ...`, `test(...)`, `docs: ...`). Commit after each task's tests pass.
- Verification before claiming a milestone done: `bun x nx run rust-daemon:test --skipNxCache` for any Rust change; `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache` for TS changes (confirm names with `bun x nx show projects --json`); the web-e2e target once M10 adds specs.

## Execution protocol

- **Rolling-wave detail.** This master plan fixes milestones, tasks, files, interfaces, and acceptance. Each milestone's step-by-step plan (with complete test and implementation code) is written to `docs/superpowers/plans/2026-09-23-companion-console-m<N>.md` immediately before that milestone starts, against the code as it then exists, and linked in the status table below.
- **Per task:** a fresh implementer subagent works test-first from the milestone plan; a reviewer then checks spec compliance and code quality; fixes loop until clean; then commit.
- **Per milestone:** run the full relevant verification commands, update the status table, and continue to the next milestone without pausing for approval.

## Status

| Milestone | Detailed plan | Status |
|---|---|---|
| M0 Security and groundwork | `2026-09-23-companion-console-m0.md` | done (Nx rust-daemon:test 1,082 passed at 4e3eb7d) |
| M1 Run coordinator | `2026-09-23-companion-console-m1.md` | done (Nx rust-daemon:test 1,141 passed at ef5b6fe) |
| M2 Sessions | `2026-09-23-companion-console-m2.md` | planned (audit in progress) |
| M3 Live runs | (written before M3) | pending |
| M4 Approvals | (written before M4) | pending |
| M5 Skills | (written before M5) | pending |
| M6 Automations | (written before M6) | pending |
| M7 Memory | (written before M7) | pending |
| M8 Usage, logs, health | (written before M8) | pending |
| M9 Attachments and voice | (written before M9) | pending |
| M10 Deployment and docs | (written before M10) | pending |

Dependencies: M0 → M1 → M2 → M3 → M4 → M5 → M6 → M7 → M8 → M9 → M10. M4–M9 depend on M3's ledger, runs, and event stream; their internal order follows the spec's milestone list.

---

## M0 Security and groundwork (spec §14, §13.2, §5.1, §11.1)

- **T0.1 Workspace writer hardening.** Files: `hosts/rust-daemon/src/tools/workspace.rs`, `hosts/rust-daemon/src/tools/filesystem/edit.rs`, `hosts/rust-daemon/src/tools/tests.rs`. Produces `resolve_workspace_write_path` (rejects `..`, dangling or escaping final symlinks) and `write_workspace_bytes(workspace_root: &Path, file_path: &str, bytes: &[u8], tool_name: &str) -> Result<PathBuf, String>` (validate, create parents, re-verify parent, write), used by `write_file` now and by skills and uploads later. Acceptance: `newdir/../../x`, `../x`, `a/b/../../../x`, and a dangling symlink are rejected with nothing created outside; internal symlinks still work.
- **T0.2 Event-log cap.** Files: `packages/core-rust/crates/anima-core/src/runtime.rs`, `.../runtime/tests.rs`. Produces `pub const MAX_RETAINED_EVENTS: usize = 500` and a running `event_total`. Acceptance: snapshots hold the newest 500 events; `event_count` is the running total; oversized snapshots are trimmed on load.
- **T0.3 Usage accounting.** Files: `anima-core/src/agent.rs` (`TokenUsage` gains `cached_prompt_tokens`, `reasoning_tokens`, both `#[serde(default)]`), every `TokenUsage { .. }` literal, adapters `common.rs`, `anthropic.rs`, `google.rs`, `stream.rs`, `adapter.rs`, `chatgpt.rs`. Acceptance: providers documented to accept it (`openai`, `deepseek`, `vllm`) receive `stream_options.include_usage`; cached and reasoning details parse; Google completion includes thinking tokens with a consistent total; Anthropic cache tokens parse; old snapshots without the new fields load.
- **T0.4 Model table and cost estimation.** Files: create `packages/core-rust/crates/anima-model-adapters/src/models.rs`; modify `lib.rs`. Produces `ModelInfo`, `ModelPricing`, `CostEstimate`, `model_info(provider: &str, model: &str) -> Option<&'static ModelInfo>`, `estimate_cost_micros(provider: &str, model: &str, usage: &TokenUsage) -> CostEstimate`, `price_usage`. Acceptance: alias resolution, longest-prefix match, local providers free, ChatGPT reports subscription, unknown models return no price; rows transcribed from the verified data file `docs/superpowers/plans/data/2026-09-23-model-table.md`.

## M1 Run coordinator (spec §4.3, §4.4, §4.1, §4.8, §4.9)

- **T1.1 Isolated per-run runtime and change-set commit/rollback.** Files: `hosts/rust-daemon/src/agent_runs.rs`, `state.rs`. Produces `RunChangeSet { run_id, message_ids, event_ids, token_delta, step_delta }`, `DaemonState::commit_run(change_set, outcome)`, `DaemonState::rollback_run(change_set)`. Acceptance: two runs in different rooms of one agent both commit; rolling back one leaves the other's turn intact; PATCH during a run applies to later runs only.
- **T1.2 Session locks, agent slots, derived status, deletion.** Files: `agent_runs.rs`, `state.rs`, `routes/agents.rs`, `routes/mod.rs`. Produces `RunCoordinator::admit(agent_id, room_key, mode)`, `DaemonState::in_flight_runs(agent_id) -> usize`. Acceptance: same-room FIFO; cross-room concurrency up to `ANIMAOS_RS_MAX_RUNS_PER_AGENT`; helpers 1; derived `Running` status; delete returns 409 during running runs; commits for deleted agents are discarded.
- **T1.3 Hooks receive the reply id.** Files: `connectors/runtime.rs`, `schedules.rs`, `jobs.rs`, `connectors/gcalendar/mod.rs`. Produces `RunOutcome { run_id, session_id, reply_message_id: Option<String>, result, status }`. Acceptance: Telegram, schedule, and job hooks build outbound and outcome records from `reply_message_id`; no backwards transcript scan remains; rollback is change-set based.
- **T1.4 Run ledger, restart recovery, legacy route, reserved rooms.** Files: create `hosts/rust-daemon/src/runs/ledger.rs`; modify `state.rs`, `control_plane_store.rs`, `routes/agents.rs`. Produces `RunRecord`, `RunStatus`, `DaemonState::runs`. Acceptance: every coordinator run has a ledger record; restart maps queued→`interrupted/restart_before_start` and running→`interrupted/restart_during_run`; `POST /run` contract unchanged; reserved prefixes rejected with 400.
- **T1.5 Shared-resource safety.** Files: `tools/todo.rs`, `agent_runs.rs` (helper reservation), `anima-core/src/runtime.rs` (step keys include run id). Acceptance: `todo_write` compare-and-swap conflict message; helper reuse reserves its single slot; concurrent runs never collide in step keys.
- **T1.6 Test rewrite and full suite.** Rewrite the serialization-dependent tests named in the parallel-runs assessment (spec §17) and run the full daemon suite.

## M2 Sessions (spec §3, §13)

- **T2.1 History store and outbox.** Files: create `hosts/rust-daemon/src/history/{mod.rs,sqlite.rs,postgres.rs,memory.rs,outbox.rs}`, `hosts/rust-daemon/migrations/20260923000000_history_store.sql`. Produces trait `HistoryStore` (append/upsert messages, runs, usage, approvals, schedule runs, attachments; page and search messages; aggregate usage), `HistoryOutbox::enqueue(..)`, flush loop, readiness issue after 5 minutes of failures.
- **T2.2 Session registry and migration.** Files: create `hosts/rust-daemon/src/sessions/{mod.rs,migration.rs}`; modify `state.rs`, `control_plane_store.rs` (version bump, backup). Produces `SessionRecord`, `SessionKind`, `DaemonState::sessions`, `derive_sessions_for_legacy_rooms(..)`. Acceptance: backup written first; rooms mapped per §3.1; legacy check-in rooms relabelled; tools granted per §13.3 step 5.
- **T2.3 Session routes.** Files: create `hosts/rust-daemon/src/routes/sessions.rs`, contracts in `routes/contracts/sessions.rs`; modify `routes/mod.rs`, `routes/agents.rs` (`view=summary`). Acceptance: list/get/messages/create/patch/delete/export behave per §3.3 with owner auth, paging, search, and 409 rules.
- **T2.4 Check-in rooms, helper linkage, silent-memory skip.** Files: `schedules.rs`, `agent_runs.rs`, `components/evaluators.rs`. Acceptance: workspace automations run in `schedule:<id>`; delegations record parent session/run/agent; silent outcomes store no memories.
- **T2.5 Hot-tail pruning.** Files: `sessions/pruning.rs`, `state.rs` validation. Acceptance: pruning rules and `messagePruned` validation per §13.2; disabled in ephemeral mode and until mirroring completes.
- **T2.6 SDK sessions client.** Files: create `packages/sdk/src/sessions.ts`, spec file; export from `index.ts`.
- **T2.7 Web shell and routing.** Files: create `apps/web/src/lib/hash-route.ts`, `apps/web/src/components/sessions/SessionSidebar.tsx`; modify `WorkspaceShell.tsx`, `ViewHarness.tsx`; remove Telegram destination, `ActivityView`, `CheckinsView` usage.
- **T2.8 Web session view on blocking runs.** Files: create `apps/web/src/hooks/useCompanionSessions.ts`, `apps/web/src/components/sessions/SessionView.tsx`; adapt `ChatScreen.tsx`.

## M3 Live runs (spec §4.2, §4.5–§4.7, §5, §6, §12.3, §12.4, §7 of search)

- **T3.1 Core observer, cancellation, steering.** `anima-core/src/runtime.rs` (+ new `runtime/observer.rs`, `runtime/control.rs`). Produces `RunObserver`, `RunControl { cancel: CancelSignal, steering: SteeringInbox }`.
- **T3.2 Core context selection.** Create `anima-core/src/context_window.rs`. Produces `select_context(messages, summary, budget, estimator) -> ContextSelection`.
- **T3.3 Streaming for every provider.** `anima-model-adapters/src/{google.rs,ollama.rs,stream.rs,adapter.rs}`, `hosts/rust-daemon/src/runtime_model.rs`.
- **T3.4 Event stream.** Create `hosts/rust-daemon/src/live/{fanout.rs,events.rs}`, route `GET /api/agents/{id}/events`.
- **T3.5 Async runs, queue, steer, stop.** Create `hosts/rust-daemon/src/routes/runs.rs`; modify coordinator and ledger.
- **T3.6 Stop for Telegram and jobs.** `connectors/runtime.rs` (`Stopped`, `Suppressed`), `jobs.rs` + `jobs/records.rs` (stop marker, `stopped` attempt).
- **T3.7 Context wiring, compaction, titles, conversation search.** `agent_runs.rs`, create `sessions/compaction.rs`, `sessions/titles.rs`, `tools/conversations.rs`.
- **T3.8 SDK runs and events.** Create `packages/sdk/src/runs.ts`, `packages/sdk/src/events.ts`.
- **T3.9 Web live rendering.** Create `apps/web/src/hooks/useAgentEvents.ts`, `apps/web/src/lib/session-events.ts`, `apps/web/src/components/sessions/{RunActivity.tsx,ToolStepCard.tsx,HelperCard.tsx}`.
- **T3.10 Web composer.** Modify `ChatScreen.tsx` composer; create `apps/web/src/components/sessions/SlashCommandMenu.tsx`, `apps/web/src/lib/slash-commands.ts`.

## M4 Approvals (spec §7)

- **T4.1** Risk table, policy, rules, gate, waiter, timeouts, helper denial, restart expiry: create `hosts/rust-daemon/src/approvals/{mod.rs,policy.rs,gate.rs}`; modify `tools.rs`.
- **T4.2** Routes and history: create `routes/approvals.rs`.
- **T4.3** SDK `approvals.ts`; web `ApprovalCard.tsx`, `pages/ApprovalsPage.tsx`, badges.

## M5 Skills (spec §8)

- **T5.1** Registry, scan, hash pinning, drafts, routes: create `hosts/rust-daemon/src/skills/{mod.rs,registry.rs,drafts.rs}`, `routes/skills.rs`.
- **T5.2** Index injection, `load_skill`, `propose_skill`, `/skill` runs: `agent_runs.rs`, create `tools/skills.rs`.
- **T5.3** SDK `skills.ts`; web `pages/SkillsPage.tsx`, composer skill commands.

## M6 Automations (spec §9)

- **T6.1** `cron` and `once` triggers with `croner`, active hours, restore validation, preview route: `schedules.rs`, `routes/schedules.rs`, contracts.
- **T6.2** Run now, history, counters, heartbeat preset.
- **T6.3** Companion tools and limits: create `tools/automations.rs`.
- **T6.4** SDK `automations.ts`; web `pages/AutomationsPage.tsx`, `lib/schedule-parse.ts`, notice cards.

## M7 Memory (spec §10)

- **T7.1** `anima-memory`: `update_memory`, `delete_entity`, citation cleanup.
- **T7.2** Daemon routes with owner auth and embedding sync: `routes/memories.rs`.
- **T7.3** SDK memory additions; web `pages/MemoryPage.tsx`, Save to memory.

## M8 Usage, logs, health (spec §11)

- **T8.1** Usage records from `StepUsage` and secondary calls, pricing overrides, usage routes, CSV: create `hosts/rust-daemon/src/usage/{mod.rs,pricing.rs}`, `routes/usage.rs`.
- **T8.2** Logs ring buffer layer and routes: create `hosts/rust-daemon/src/logs.rs`, `routes/logs.rs`; modify `main.rs`.
- **T8.3** Status aggregate and metrics: `routes/health.rs`, create `routes/status.rs`.
- **T8.4** SDK `usage.ts`, `logs.ts`, `status.ts`; web `pages/{UsagePage,LogsPage,HealthPage}.tsx`, session usage, `/usage`.

## M9 Attachments and voice (spec §12.1, §12.2)

- **T9.1** `Attachment.mime_type`; image rendering in every adapter; `vision` flag use.
- **T9.2** Upload, serve, run input attachments, context image cap: create `hosts/rust-daemon/src/attachments.rs`, `routes/attachments.rs` (uses `write_workspace_bytes`).
- **T9.3** Web attachments, thumbnails, dictation, Read aloud.

## M10 Deployment and docs (spec §13.4, §17)

- **T10.1** `deploy/vps/compose.yaml` and README (history store, upgrade and rollback), `hosts/rust-daemon/README.md` route table, `apps/web/README.md`.
- **T10.2** Playwright specs in `apps/web-e2e`.
- **T10.3** Full verification and isolated container smoke.
