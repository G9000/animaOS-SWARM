# Companion Console M8: Usage, Logs, Health Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **This is a lean plan:** each task gives files, exact interfaces, rules, a test list, commands, and a commit message, and complete code only for the few tricky pieces. The implementer writes the tests and the rest of the code from the interfaces, test-first, against the code as it is.

**Goal:** Let the owner see what the companion costs, what the daemon is doing, and whether it is healthy (spec §11, §15.4 Usage, Logs, Health): a usage record for every model call (run steps, plus titles, compaction, profile and agency generation), prices with owner overrides, summaries and CSV export; a redacted, bounded log tail with a live stream; one status aggregate and richer `/metrics`. The web gets three System pages (Usage, Logs, Health), a usage line in the session header, and `/usage`. The three M3 carry-overs land too: stopped or failed model calls keep the usage the provider already reported, a run's pending steers count toward the 8-per-agent queue cap, and compaction and title calls record usage.

**Architecture:** Usage rows live in the history store's existing `history_usage` / `usage` tables (no migration: they hold the full JSON `record`). Run-step rows are **derived from the terminal `RunRecord`** when the outbox mirrors it (`write_runs`), so they inherit the ledger's durability; secondary calls go through a `MeteredAdapter` wrapper and a small in-memory usage queue in the outbox. Pricing is the `anima-model-adapters` model table plus owner overrides kept in the **control plane (snapshot version 10)**. Summaries are computed in Rust over paged `page_usage` reads, so the three stores need only `upsert_usage` and `page_usage`. Logs are a `tracing` `Layer` writing redacted lines into a bounded ring buffer (`logs.rs`) with a `broadcast` channel for the SSE stream. Status and metrics share one `StatusSnapshot` built in `routes/status.rs`. The SDK gets `UsageClient`, `LogsClient`, `StatusClient`; the web gets `lib/{usage,logs,status}.ts`, three hooks, and three pages.

**Tech Stack:** Rust 2021 (tokio, axum 0.8, serde, utoipa 5, tracing-subscriber 0.3, regex), TypeScript (React 19, Vite, Tailwind v4, Vitest, Testing Library), Nx with Bun.

**Spec:** `docs/superpowers/specs/2026-09-23-companion-console-design.md` (§11 is the core; also §6 events (no new event), §11.3 status and metrics, §13 persistence, §14 owner authorization, §15.2 header usage, §15.3 `/usage`, §15.4 Usage, Logs, Health, §16 limits, §17 tests). Master plan: `docs/superpowers/plans/2026-09-23-companion-console.md` (M8, T8.1–T8.4, "Carried from M3", and Global Constraints).

## Global Constraints

- Master plan Global Constraints apply. **No new third-party dependencies and no new dependency features** (Rust, SDK, web). `tracing-subscriber` is `{ version = "0.3", features = ["env-filter", "fmt"] }` with default features: its `registry` and `Layer` APIs are already available (`fmt` enables `registry`). `regex = "1"` is already a daemon dependency; the log redactor uses it. `anima-core` and `anima-model-adapters` gain no HTTP, DB, or host dependency.
- **Precondition: M7 is merged.** Before Task 1 run `git log --oneline -1 && grep -n "CONTROL_PLANE_STORE_VERSION: u32 = 9" hosts/rust-daemon/src/control_plane_store.rs && grep -n "async fn upsert_schedule_runs" hosts/rust-daemon/src/history/mod.rs && grep -n "StepUsage" hosts/rust-daemon/src/live/observer.rs && ls apps/web/src/pages/MemoryPage.tsx`. Expected: head at or after `248c9b6`, and a match in each. Otherwise stop and report.
- **Snapshot version moves 9 to 10** (Task 3). Reason: owner pricing overrides are control-plane state (spec §11.1); `RunStepUsage` also gains two defaulted fields (Task 2), which ride the same bump. Follow the M6 precedent (`8c50a05`): `CONTROL_PLANE_STORE_VERSION = 10`; a new `AUTOMATIONS_STORE_VERSION = 9` ("the version that added automations"); `PRE_USAGE_BACKUP_SUFFIX = ".pre-usage.bak"` and `pre_usage_backup_path`; `pre_upgrade_backup_path` gains the branch `loaded_version < AUTOMATIONS_STORE_VERSION` keeps `.pre-automations.bak` and a loaded version 9 now takes `.pre-usage.bak` (its comment says a future version 10 must add one); the Postgres key stays `control_plane.backup.<loadedVersion>` through the existing `postgres_backup_key`. A daemon older than M8 refuses a v10 snapshot (existing rule), so rolling back needs the backup; Task 3's README note says so and Task 12 greps the version. Everything else M8 adds lives in the history store (existing tables) or in memory (the log buffer).
- **Routes, exactly** (spec §11; all new, all `#[utoipa::path(...)]` registered in `ApiDoc` with tags `usage`, `logs`, `status`; all answer `Cache-Control: no-store`; reads call `state.local_owner.authorize_read` through `routes::jobs::authorize(&state, &request, true)`, mutations `authorize(…, false)` (403 `local owner authorization required`); handlers reuse `routes::jobs::{authorize, no_store}` and `routes::sessions::rejected` as `routes/skills.rs` does). `/metrics` stays exempt from the API key, as today; it carries counts only.
  - `GET /api/usage/summary?from=&to=&agentId=&sessionId=&groupBy=day|model|source|session&tzOffsetMinutes=` → 200 `UsageSummary`.
  - `GET /api/usage/records?from=&to=&agentId=&cursor=&limit=` → 200 `{ records: UsageRecord[], nextCursor: string | null }`, newest first.
  - `GET /api/usage/export.csv?from=&to=&agentId=` → 200 `text/csv; charset=utf-8` with `Content-Disposition: attachment; filename="anima-usage-<fromDate>-<toDate>.csv"`.
  - `GET /api/usage/pricing` → 200 `{ overrides: PricingOverride[], tableDate: string }`; `PUT /api/usage/pricing` with `{ overrides: PricingOverride[] }` (replace all) → 200 the same shape.
  - `GET /api/logs?level=&q=&after=&limit=` → 200 `{ lines: LogLine[], newestSeq: number }`, oldest first.
  - `GET /api/logs/stream?level=&q=&after=` → SSE: `event: log` (data is one `LogLine`), `event: resync` (data `{ "newestSeq": n }`, sent when this stream fell behind).
  - `GET /api/status` → 200 `StatusResponse`.
  - `GET /api/agents/{agent_id}/sessions/{session_id}` (existing) gains `usage: SessionUsage` in its response (the only change to an existing shape; additive).
- **JSON, exactly** (camelCase; absent values are `null`; timestamps are epoch milliseconds; money is micro-USD integers):
  - `UsageRecord`: `id`, `agentId`, `sessionId` (null), `runId` (null), `source` (`chat | telegram | automation | job | helper | api | title | compaction | profile | agency`), `provider`, `model`, `promptTokens`, `completionTokens`, `cachedPromptTokens`, `reasoningTokens`, `totalTokens`, `costMicros` (null), `pricingSource` (`table | override | free | subscription | unknown`), `durationMs`, `createdAtMs`.
  - `UsageTotals`: `calls`, `promptTokens`, `completionTokens`, `cachedPromptTokens`, `reasoningTokens`, `totalTokens`, `costMicros` (sum of the priced calls), `unpricedCalls` (calls whose `costMicros` is null and whose `pricingSource` is not `subscription`), `subscriptionCalls`.
  - `UsageSummary`: `{ from, to, groupBy: string | null, tzOffsetMinutes, totals: UsageTotals, groups: { key: string, totals: UsageTotals }[], truncated: boolean }`. Group keys: `day` is `YYYY-MM-DD` in the caller's offset, ascending; `model` is `<provider>/<model>`, `source` is the source name, `session` is the session id (`""` for rows with none); the last three sorted by `totalTokens` descending then key, `session` cut to the top 20.
  - `SessionUsage` (on the session response) is the `UsageTotals` of that session.
  - `PricingOverride`: `{ provider, model, inputMicrosPerMtok, outputMicrosPerMtok, cachedInputMicrosPerMtok: number | null }`. `model` is a lowercase prefix; the longest matching prefix wins; an override beats the table.
  - `LogLine`: `{ seq, at, level: "error"|"warn"|"info"|"debug"|"trace", target, message }`.
  - `StatusResponse`: see Task 8.
- **Limits and constants, named once** (each tested once):
  - `usage/mod.rs`: `DEFAULT_USAGE_RANGE_DAYS = 30`, `MAX_USAGE_RANGE_DAYS = 366`, `DEFAULT_RECORDS_LIMIT = 50`, `MAX_RECORDS_LIMIT = 200`, `USAGE_SCAN_PAGE = 2_000`, `MAX_SUMMARY_ROWS = 200_000` (past it the summary says `truncated: true`), `MAX_SESSION_GROUPS = 20`, `MAX_CSV_ROWS = 100_000`, `USAGE_QUEUE_MAX = 10_000`, `HISTORY_USAGE_BATCH = 500` (outbox).
  - `usage/pricing.rs`: `MAX_PRICING_OVERRIDES = 100`, `MAX_PRICE_MICROS_PER_MTOK = 1_000_000_000_000`, `MAX_PRICING_PROVIDER_CHARS = 64`, `MAX_PRICING_MODEL_CHARS = 128`.
  - `logs.rs`: `LOG_BUFFER_LINES = 2_000`, `LOG_LINE_MAX_BYTES = 4_096` (the whole formatted line, suffix included), `LOG_REDACTED = "[redacted]"`, `LOG_TRUNCATED_SUFFIX = "…[truncated]"`, `LOG_BROADCAST_CAPACITY = 256`; `routes/logs.rs`: `DEFAULT_LOGS_LIMIT = 200`, `MAX_LOGS_LIMIT = 1_000`, `MAX_LOG_STREAMS = 8`, `MAX_LOG_QUERY_CHARS = 200`.
  - Web: `lib/usage.ts` `USAGE_RANGES_DAYS = [7, 30, 90]`; `lib/logs.ts` `MAX_LOGS_SHOWN = 2_000`, `LOGS_FETCH_LIMIT = 500`.
- **Strings, exact** (named constants, each tested once):
  - `routes/usage.rs`: `USAGE_RANGE_INVALID = "from and to must be epoch milliseconds, from before to, at most 366 days apart"`, `USAGE_GROUP_INVALID = "groupBy must be one of day, model, source, session"`, `USAGE_LIMIT_INVALID = "limit must be from 1 to 200"`, `USAGE_CURSOR_INVALID = "cursor is not valid"`, `USAGE_TZ_INVALID = "tzOffsetMinutes must be from -840 to 840"`, `USAGE_EXPORT_TOO_LARGE = "That range has too many calls to export; choose a shorter range"`, `USAGE_TASK_FAILED = "The usage change did not finish; check Usage and try again"` (503).
  - `usage/pricing.rs`: `PRICING_TOO_MANY = "at most 100 pricing overrides"`, `PRICING_ENTRY_INVALID = "each override needs a provider, a model, and prices from 0 to 1000000000000"`, `PRICING_DUPLICATE = "each provider and model may appear once"`.
  - `routes/logs.rs`: `LOGS_LEVEL_INVALID = "level must be one of error, warn, info, debug, trace"`, `LOGS_LIMIT_INVALID = "limit must be from 1 to 1000"`, `LOGS_AFTER_INVALID = "after must be a whole number"`, `LOGS_QUERY_TOO_LONG = "q must be at most 200 characters"`, `LOGS_TOO_MANY_STREAMS = "Too many log streams are open"` (429).
  - Web strings are named constants in `apps/web/src/lib/{usage,logs,status}.ts` (Tasks 10 and 11).
- **Untrusted and secret-bearing content.** Log lines can carry text the model or a provider wrote (error bodies, tool names) and must never carry secrets. Rules: (1) every captured line goes through `logs::redact` **before** it enters the buffer, so neither `GET /api/logs`, the stream, nor a copy in the UI can hold a secret; (2) the redactor is tested against every secret shape the daemon handles (API keys, `Authorization` and `Bearer` values, Telegram bot tokens including inside `/bot<token>/` URLs, OAuth `access_token`, `refresh_token`, `client_secret`, `code`, `state` query values, cookies, JWTs, and unlabeled long mixed-case tokens); (3) structured fields whose names look secret are replaced by value, not scanned; (4) the web shows every log line as a text node through `RevealedText` (spec §14), never `MarkdownMessage` or HTML. Task 12 greps for the last and reviews every `info!`/`warn!`/`error!` call site that mentions a credential-like word.
- **Concurrency, every task.** Lock order: control-plane transaction → state lock → leaf mutexes (the outbox, the log buffer, the usage queue). No `std::sync::Mutex` is held across `.await`. The log buffer's `std::sync::Mutex` is taken only inside `Layer::on_event` and the read paths, holds no `.await`, and **the layer never logs while holding it** (no `tracing` macro and no call that can emit one inside the lock; the broadcast `send` does not log). The owner mutation (`PUT /api/usage/pricing`) runs its whole body, transaction included, in its own `tokio::spawn` (as `put_approval_policy` does), so a dropped request never leaves a half-applied change.
- Existing behavior stays except where a task says so. Deliberate changes: a `ModelStreamFrame::Usage` variant (Task 1); a rollback keeps the failed run's steps (Task 4); steers count toward the 8-queue cap (Task 8); `GET session` gains `usage` (Task 5); `init_tracing` builds a layered subscriber (Task 6).
- Commands. Rust iteration: `CARGO_INCREMENTAL=0 cargo test -p <crate> --lib -- <filter>` (crates `anima-core`, `anima-model-adapters`, `anima-daemon`); integration: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test <name> 2>&1 | tail -30` (pipe the others through `tail -30` too). SDK: `bun x nx test @animaOS-SWARM/sdk`, and **every SDK-changing task ends with `bun x nx run @animaOS-SWARM/sdk:build`** so later direct web Vitest runs resolve the new exports. Web: `cd apps/web && bun x vitest run <files>`. The milestone gate (Task 12): `bun x nx run rust-daemon:test --skipNxCache` and `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`.
- Formatting. Every task ends with `cargo fmt --all` when it touched Rust (then `git diff --stat` must show only the task's files; if `cargo fmt` reformatted unrelated files, tell the controller instead of staging them) and `bun x nx format:write --files=<each changed TS/TSX/CSS/MD file>` when it touched TypeScript, CSS, or Markdown, then re-runs its tests. On Windows the working tree may carry CRLF: `nx format:write` fixes it, and `nx format:check --base=origin/main` in Task 12 is the arbiter.
- Git. Stage files by explicit path only; never `git add -A`, `git add .`, or `git commit -a`. Never stage anything under `docs/` or `.superpowers/`, nor `nx.json` or `anima.yaml`. `hosts/rust-daemon/README.md` is staged with the task that changes it. Never use `git stash`, `git reset`, `git checkout -- <path>`, `git restore`, or `git worktree`, and never switch branches. Do not start the daemon, a dev server, a database, or a container; tests start what they need. Commits are GPG-signed on this machine and can block on a pinentry dialog until the owner answers it. End each commit message with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Disk is tight: never set a new `CARGO_TARGET_DIR`. No Postgres is available (the Postgres history store's new SQL stays `#[ignore]`/untested here, as before; hand-check it against the SQLite twin).
- Large files stay put: `agent_runs.rs`, `connectors/runtime.rs`, and `ViewHarness.tsx` only gain wiring lines (M8 adds a few to `ViewHarness.tsx` and none to the other two; the secondary-call wiring is in `agent_runs/titles.rs` and `agent_runs/compact.rs`, which are small). New logic goes in new modules and components. Web tests stay pristine: no new `act()` warnings or console noise.
- Windows: no test builds a path by string concatenation; temp paths use `std::env::temp_dir().join(…)` with a UUID.
- Tests are deterministic: no sleeps and no wall-clock races. Time comes from a parameter (`now_ms`) wherever a rule depends on it; stream and reconnect tests use the existing fake-timer or injected-delay patterns.
- Code fences: complete functions keep their language; partial fragments are fenced as `text` so Prettier leaves them alone.
- Out of scope (do not build): per-agent budgets or spending alerts; a cost forecast; usage for tool calls; per-request HTTP logs in the buffer beyond what `tower_http` already emits at info; log persistence across restarts; a `usage.updated` or `status.changed` event; editing or deleting usage rows; provider-side billing reconciliation; alert routing for health issues.

## Review Focus

1. **Usage is complete and not double counted.** Every run step and every secondary call yields exactly one row; the row id is stable so the outbox's retries and a restart re-mirror do not duplicate; a run past 50 steps adds one `:rest` row so totals match the run. Tests: Task 4 (`a_remirrored_run_does_not_duplicate_usage`, `steps_past_the_cap_add_one_remainder_row`), Task 2 (`usage_upserts_are_idempotent_by_id`).
2. **Stopped and failed calls keep what the provider reported.** Tests: Task 1 (`a_stopped_call_records_the_prompt_tokens_already_reported`, `a_failed_stream_records_the_running_usage`).
3. **Logs never hold a secret.** Redaction runs at capture, covers labeled and unlabeled shapes, and is tested with every shape. Tests: Task 6 (`redacts_every_secret_shape`, `a_secret_field_name_redacts_its_value_without_reading_it`, `redaction_survives_truncation_boundaries`).
4. **The log layer cannot deadlock or recurse.** Tests: Task 6 (`the_layer_emits_no_events_of_its_own`, `the_logging_modules_contain_no_tracing_macros`, `capture_is_bounded`, `seq_never_repeats_under_concurrent_pushes`), Task 7 (`a_stream_gets_the_backlog_then_live_lines_without_a_gap`).
5. **Pricing is honest.** Unknown is null, local is 0, the ChatGPT subscription is null with `subscription`, overrides win by longest prefix, and a changed override affects later calls only. Tests: Task 2 (`pricing_resolution_order`), Task 5 (`put_pricing_*`).
6. **Owner-only, drop-safe, and v10-safe.** Tests: Task 5 (`the_usage_routes_refuse_a_non_owner`, `a_dropped_pricing_put_still_finishes`), Task 3 (`a_version_nine_snapshot_loads_and_is_backed_up`).

## File map

| Area          | Files                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Core          | `packages/core-rust/crates/anima-core/src`: modify `model.rs` (`ModelStreamFrame::Usage`), `engine.rs` (one match arm), `runtime.rs` and `runtime/observer.rs` (partial usage), their tests                                                                                                                                                                                                                                                             |
| Adapters      | `packages/core-rust/crates/anima-model-adapters/src`: modify `stream.rs`, `google.rs`, `chatgpt.rs` (match arm only), `tests.rs`                                                                                                                                                                                                                                                                                                                        |
| Usage         | `hosts/rust-daemon/src`: create `usage/mod.rs`, `usage/pricing.rs`, `usage/summary.rs`, `usage/metered.rs`, `routes/usage.rs`, `routes/contracts/usage.rs`, `routes/tests/usage.rs`; modify `lib.rs`, `history/{mod,memory,sqlite,postgres,outbox,conformance}.rs`, `runs/ledger.rs`, `live/registry.rs`, `state/run_commit.rs`, `agent_runs/{titles,compact}.rs`, `routes/{profile,agencies}.rs`, `routes/sessions.rs`, `routes/contracts/sessions.rs` |
| Control plane | `control_plane_store.rs`, `state.rs`, `app/persistence.rs` (v10, pricing overrides, backup)                                                                                                                                                                                                                                                                                                                                                             |
| Logs          | create `logs.rs`, `logs/redact.rs`, `routes/logs.rs`, `routes/contracts/logs.rs`, `routes/tests/logs.rs`; modify `main.rs` (`init_tracing`), `lib.rs`                                                                                                                                                                                                                                                                                                   |
| Status        | create `routes/status.rs`, `routes/contracts/status.rs`, `routes/tests/status.rs`; modify `routes/health.rs`, `routes/mod.rs` (routes, `ApiDoc`, `AppState`), `runs/ledger.rs`, `agent_runs/queue.rs`, `live/fanout.rs`, `history/outbox.rs`, `sessions/pruning.rs`, `hosts/rust-daemon/README.md`                                                                                                                                                      |
| SDK           | `packages/sdk/src`: create `usage.ts`, `logs.ts`, `status.ts` (+ `.spec.ts` each); modify `client.ts`, `index.ts`, `sessions.ts` (`Session.usage`)                                                                                                                                                                                                                                                                                                      |
| Web data      | `apps/web/src`: create `lib/{usage,logs,status}.ts` (+ tests), `hooks/{useUsage,useLogs,useStatus}.ts` (+ tests), `hooks/useSessionUsage.ts`, `test/usage.ts`; modify `lib/daemon-api.ts` (+ test)                                                                                                                                                                                                                                                      |
| Web pages     | create `pages/{UsagePage,LogsPage,HealthPage}.tsx` (+ tests), `components/usage/{UsageChart,UsageTable}.tsx`, `components/system/{LogLine,StatusCard}.tsx`, `system.css`; modify `styles.css`, `components/WorkspaceShell.tsx` (+ test), `components/sessions/SessionView.tsx` (+ test), `lib/slash-commands.ts`, `hooks/useSessionCommands.ts`, `ViewHarness.tsx` (wiring), `ViewHarness.test.tsx`                                                     |
| Docs          | `docs/superpowers/plans/2026-09-23-companion-console.md` (the M8 status row and task lines, Task 12, controller only)                                                                                                                                                                                                                                                                                                                                   |

## Task list

1. Core and adapters: partial usage frames; stopped and failed calls keep reported usage (carry-over)
2. Daemon: usage records, pricing resolution, and the history store's usage methods (T8.1)
3. Daemon: control plane version 10 with pricing overrides (T8.1)
4. Daemon: record usage for run steps and secondary calls (T8.1 and carry-over)
5. Daemon: usage routes, CSV, pricing routes, and session usage totals (T8.1)
6. Daemon: the log ring buffer, the tracing layer, and redaction (T8.2)
7. Daemon: log routes with the live stream (T8.2)
8. Daemon: status aggregate, richer metrics, and the queue-cap carry-over (T8.3)
9. SDK: usage, logs, and status clients (T8.4)
10. Web data and the Usage page: helpers, hooks, session usage, `/usage` (T8.4)
11. Web Logs and Health pages and the System group (T8.4)
12. M8 verification (controller)

---

### Task 1: Core and adapters: partial usage frames; stopped and failed calls keep reported usage

**Model tier:** sonnet.

**Files:**

- Modify: `anima-core/src/model.rs` (new variant), `anima-core/src/engine.rs` (the sink match at the `ModelStreamFrame::Final` arm: ignore `Usage`), `anima-core/src/runtime/observer.rs` (`StepSink` keeps the running usage), `anima-core/src/runtime.rs` (record it on stop and on a failed stream), `anima-core/src/runtime/observer_tests.rs`
- Modify: `anima-model-adapters/src/stream.rs`, `.../google.rs`, `.../chatgpt.rs` (its sink `if let` already ignores other frames; confirm it compiles), `.../tests.rs`

**Interfaces (exact):**

```text
// anima-core/src/model.rs
pub enum ModelStreamFrame {
    TextDelta(String),
    /// Provider-reported usage so far; each frame replaces the last. `Final` still carries the definitive usage.
    Usage(TokenUsage),
    Final(ModelGenerateResponse),
}

// anima-core/src/runtime/observer.rs  (StepSink)
usage: Mutex<Option<TokenUsage>>                 // set by ModelStreamFrame::Usage, latest wins
pub(crate) fn partial_usage(&self) -> Option<TokenUsage>   // None until a Usage frame arrives; None when total_tokens == 0
```

**Rules:**

- Adapters emit a `Usage` frame **only when the running usage changed** since the last one they emitted, after parsing an event (the existing `consume_sse_events` loop and the NDJSON loop). The frame holds what `finish()` would compute so far: `total_tokens = max(reported total, prompt + completion)`. Anthropic's `message_start` input tokens, Google's per-chunk `usageMetadata`, and an OpenAI-compatible provider that reports usage before its last chunk therefore produce a frame; a provider that reports usage only in its last chunk produces none before `Final` (an accepted limit, see Risks). The mechanism is the implementer's choice (for example the parse closure returns `(Option<String>, Option<TokenUsage>)`); the accumulators gain a `running_usage(&self) -> Option<TokenUsage>` and the emit is best-effort (`let _ =`) like text deltas.
- `Runtime` (the stop branch where `streamed` is `None`, and the `Err(error)` outcome branch before `fail_step`): `if let Some(usage) = sink.partial_usage() { self.apply_token_usage(&usage); self.emit_frame(RunFrame::StepUsage { step_id, usage }); }`, placed **before** `record_unfinished_step` / `fail_step`, so the run's `token_usage`, the live `steps`, and the committed record all hold it. A call that completed normally is unchanged (it records `response.usage` once; the partial is not also added). A call that failed after a `Final` (`MODEL_STREAM_WITHOUT_FINAL`, an evaluator abort) is the normal path and records `response.usage`.
- Hosts that implement `ModelStreamSink` in tests need no change; `engine.rs`'s sink ignores `Usage`.

**Tests:**

- `observer_tests.rs`: `a_stopped_call_records_the_prompt_tokens_already_reported` (a scripted adapter emits `Usage { prompt 120, completion 0 }`, a delta, then waits; stop; the run's `token_usage.prompt_tokens == 120` and a `StepUsage` frame with that usage precedes `StepFinished`); `a_failed_stream_records_the_running_usage` (adapter emits `Usage` then returns `Err`; same assertions, run fails); `a_call_without_usage_frames_records_nothing_extra` (stop with no frame: no `StepUsage`); `a_completed_call_counts_its_usage_once` (frames then `Final`: `token_usage` equals `Final`'s usage, one `StepUsage`); `partial_usage_is_none_for_a_zero_total`.
- `anima-model-adapters/src/tests.rs`, following the existing stream fixtures (grep `consume_anthropic_sse` and the `TcpListener` helper near line 946): `anthropic_stream_emits_usage_after_message_start` (a `Usage` frame with prompt tokens (input + cache) precedes the first `TextDelta`'s successor and `Final` carries the final output tokens); `google_stream_emits_usage_when_it_changes_only` (three chunks, two distinct usages, two frames); `openai_compatible_final_chunk_usage_emits_no_early_frame` (documents the limit); `usage_frames_do_not_change_the_final_response`.
- `model.rs`: the existing default-stream test still passes (`generate` then `Final` only).

**Steps:**

- [ ] **Step 1:** Run the precondition command. Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-core --lib -- observer_tests 2>&1 | tail -30`, `CARGO_INCREMENTAL=0 cargo test -p anima-model-adapters --lib -- usage 2>&1 | tail -30`). Expected: compile errors, then FAIL.
- [ ] **Step 2:** Implement. Re-run both, then `CARGO_INCREMENTAL=0 cargo test -p anima-core 2>&1 | tail -15` and `CARGO_INCREMENTAL=0 cargo test -p anima-model-adapters 2>&1 | tail -15`. Expected: PASS.
- [ ] **Step 3:** `CARGO_INCREMENTAL=0 cargo check -p anima-daemon --tests 2>&1 | tail -10` (every `match` over `ModelStreamFrame` in the daemon and `apps/server`-adjacent crates still compiles; `grep -rn "ModelStreamFrame::" hosts packages --include=*.rs | grep -v "TextDelta\|Final"` lists any new arm needed).
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage the files by path; commit:

```bash
git commit -m "feat(core): keep the usage a stopped or failed model call already reported

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Daemon: usage records, pricing resolution, and the history store's usage methods

**Model tier:** sonnet.

**Files:**

- Create: `hosts/rust-daemon/src/usage/mod.rs` (types, constants, `usage_records_for_run`), `usage/pricing.rs` (`PricingOverride`, `resolve_price`), `usage/summary.rs` (aggregation over pages)
- Modify: `hosts/rust-daemon/src/lib.rs` (`mod usage;`), `history/mod.rs` (trait methods, `UsagePageQuery`), `history/memory.rs`, `history/sqlite.rs`, `history/postgres.rs`, `history/conformance.rs`, `runs/ledger.rs` (`RunStepUsage` fields), `live/registry.rs` (step timing)

No schema change: the `history_usage` (Postgres) and `usage` (SQLite) tables already hold `id, agent_id, session_id, run_id, created_at_ms, record`.

**Interfaces (exact):**

```text
// usage/mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UsageSource { Chat, Telegram, Automation, Job, Helper, Api, Title, Compaction, Profile, Agency }
impl UsageSource { pub(crate) const fn as_str(self) -> &'static str }
impl From<RunSource> for UsageSource      // Web->Chat, Api->Api, Telegram->Telegram, Schedule->Automation, Job->Job, Delegation->Helper, Peer->Helper

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageRecord {
    pub(crate) id: String, pub(crate) agent_id: String, pub(crate) session_id: Option<String>, pub(crate) run_id: Option<String>,
    pub(crate) source: UsageSource, pub(crate) provider: String, pub(crate) model: String,
    pub(crate) prompt_tokens: u64, pub(crate) completion_tokens: u64, pub(crate) cached_prompt_tokens: u64,
    pub(crate) reasoning_tokens: u64, pub(crate) total_tokens: u64,
    pub(crate) cost_micros: Option<u64>, pub(crate) pricing_source: PricingSource,
    pub(crate) duration_ms: u64, pub(crate) created_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PricingSource { Table, Override, Free, Subscription, Unknown }

/// Builds a record from one call's `TokenUsage`, priced with `price_call`.
pub(crate) fn usage_record(call: &UsageCall, usage: &TokenUsage, overrides: &[PricingOverride]) -> UsageRecord;
pub(crate) struct UsageCall { pub(crate) id: String, pub(crate) agent_id: String, pub(crate) session_id: Option<String>, pub(crate) run_id: Option<String>, pub(crate) source: UsageSource, pub(crate) provider: String, pub(crate) model: String, pub(crate) duration_ms: u64, pub(crate) created_at_ms: u64 }

/// One record per `RunStepUsage` (id = the step id), plus one `<runId>:rest` record for any usage the run total holds beyond the steps (a run past the 50-step cap). Skips steps whose usage is all zero.
pub(crate) fn usage_records_for_run(run: &RunRecord, overrides: &[PricingOverride]) -> Vec<UsageRecord>;

// usage/pricing.rs
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PricingOverride { pub(crate) provider: String, pub(crate) model: String, pub(crate) input_micros_per_mtok: u64, pub(crate) output_micros_per_mtok: u64, #[serde(default)] pub(crate) cached_input_micros_per_mtok: Option<u64> }
/// Validates and normalizes (provider and model trimmed and lowercased); Err is one of the PRICING_* constants.
pub(crate) fn validate_overrides(list: Vec<PricingOverride>) -> Result<Vec<PricingOverride>, &'static str>;
/// Resolution order: owner override (same canonical provider, longest model prefix) -> `chatgpt` is Subscription (cost None) -> local providers Free (cost Some(0)) -> the table (`estimate_cost_micros`) -> Unknown (cost None).
pub(crate) fn price_call(provider: &str, model: &str, usage: &TokenUsage, overrides: &[PricingOverride]) -> (Option<u64>, PricingSource);

// history/mod.rs  (HistoryStore additions)
async fn upsert_usage(&self, records: &[UsageRecord]) -> Result<(), HistoryError>;       // idempotent by id
async fn page_usage(&self, query: &UsagePageQuery) -> Result<Vec<UsageRecord>, HistoryError>;   // newest first by (createdAtMs, id)
pub(crate) struct UsagePageQuery { pub(crate) from_ms: u64, pub(crate) to_ms: u64 /* exclusive */, pub(crate) agent_id: Option<String>, pub(crate) session_id: Option<String>, pub(crate) before: Option<(u64, String)> /* strictly older than */, pub(crate) limit: usize }

// usage/summary.rs
pub(crate) struct UsageTotals { calls, prompt_tokens, completion_tokens, cached_prompt_tokens, reasoning_tokens, total_tokens, cost_micros: u64, unpriced_calls: u64, subscription_calls: u64 }   // all u64, Default, Serialize camelCase
pub(crate) enum GroupBy { Day, Model, Source, Session }
pub(crate) struct Summary { pub(crate) totals: UsageTotals, pub(crate) groups: Vec<(String, UsageTotals)>, pub(crate) truncated: bool }
/// Pages `page_usage` (USAGE_SCAN_PAGE rows at a time) until the range is exhausted or MAX_SUMMARY_ROWS rows were read (then `truncated`).
pub(crate) async fn summarize(store: &dyn HistoryStore, query: SummaryQuery) -> Result<Summary, HistoryError>;
pub(crate) fn day_key(created_at_ms: u64, tz_offset_minutes: i32) -> String;   // YYYY-MM-DD of (ms + offset) in UTC arithmetic
```

**Rules:**

- `RunStepUsage` (runs/ledger.rs) gains `#[serde(default)] at_ms: u64` and `#[serde(default)] duration_ms: u64`. `LiveRuns` notes when each step starts (`start_step` stores `(step_id, now_ms)`; keep only the current step's start, a `Option<(String, u64)>` per run) and `record_step_usage` sets `at_ms = now` and `duration_ms = now - start` (0 when the step was not started here). Tests in `live/registry.rs`'s module: `a_step_usage_records_when_and_how_long`, `a_usage_without_a_started_step_has_zero_duration`; and `ledger.rs`: `an_old_step_without_timing_loads_with_zeros`.
- `usage_records_for_run`: provider is `run.provider` or `"unknown"` when absent; model `run.model`; `session_id` is `Some(run.session_id)`; `source` from `RunSource`; `created_at_ms` is the step's `at_ms` (Task 3 adds it) falling back to `run.started_at_ms.or(created_at_ms)` when 0; `duration_ms` from the step. The remainder record's id is `<run.id>:rest`, its usage is `run.usage` minus the steps' sum (saturating per field), emitted only when its `total_tokens > 0`, with `duration_ms` 0 and `created_at_ms` the run's `finished_at_ms`.
- `price_call` rounds through `anima_model_adapters::price_usage` for overrides (build a `ModelPricing` from the override); the table path uses `estimate_cost_micros`. Provider names are canonicalized the way the table does (case-insensitive; reuse the adapters' lookup, adding a small `pub fn canonical_provider_id(&str) -> Option<&'static str>` to `anima-model-adapters` only if it is not already exported; that is the one allowed adapters change in this task).
- `validate_overrides`: at most `MAX_PRICING_OVERRIDES`; provider 1 to 64 chars and model 1 to 128 chars after trim, both without control characters; each rate `<= MAX_PRICE_MICROS_PER_MTOK`; no duplicate (provider, model) after lowercasing. Errors map to `PRICING_TOO_MANY`, `PRICING_ENTRY_INVALID`, `PRICING_DUPLICATE`.
- Store semantics (all three): `upsert_usage` replaces by id; `page_usage` filters `from_ms <= created_at_ms < to_ms`, optional agent and session, orders `created_at_ms DESC, id DESC`, applies `before` strictly, `limit` rows. `delete_session` and `delete_agent` keep usage rows (existing behavior; spec §3.3); a conformance test pins it. The in-memory store keeps usage in `EPHEMERAL_HISTORY_MAX_ROWS` like the others. SQLite and Postgres store the whole record as JSON in `record` and the five indexed columns, ids bound as parameters.
- `summarize` is generic over the store; `unpriced_calls` counts rows with `cost_micros == None` and `pricing_source != Subscription`; `subscription_calls` counts the rest of the nulls; `cost_micros` sums only priced rows. `GroupBy::Session` keeps the top `MAX_SESSION_GROUPS`.

**Tests:**

- `usage/pricing.rs`: `pricing_resolution_order` (override beats table; longest override prefix wins; chatgpt is subscription with `None`; ollama and vllm are `Some(0)` free; an unknown model is `None` unknown; a known table model prices equal to `estimate_cost_micros`); `validate_overrides_normalizes_and_rejects` (case folded and trimmed; 101 entries, empty provider, a 129-char model, a rate over the maximum, a duplicate after lowercasing; each exact constant); `an_override_can_price_a_model_the_table_lacks`; `cached_rate_defaults_to_the_input_rate`.
- `usage/mod.rs`: `run_steps_become_one_record_each_with_the_runs_context`; `a_zero_usage_step_makes_no_record`; `steps_past_the_cap_add_one_remainder_row` (a run with 3 steps summing 60 tokens and `usage.total_tokens = 100` gives a `:rest` row of 40); `no_remainder_when_the_steps_account_for_the_total`; `run_source_maps_to_usage_source` (all seven); `constants` (one assertion per exported constant of this module).
- `usage/summary.rs`: `totals_sum_every_field`; `unpriced_and_subscription_calls_are_counted_apart`; `groups_by_day_in_the_callers_offset` (a record at 23:30 UTC lands on the next day for +60); `groups_by_model_source_and_session_sorted_by_tokens`; `session_groups_keep_the_top_twenty`; `a_scan_past_the_row_cap_is_truncated` (use a small injected cap parameter, `summarize_with_cap`, not 200,000 rows); `day_key_handles_negative_offsets_and_year_ends`.
- `history/conformance.rs` (the shared suite every store runs; follow how `schedule_runs` is covered): `usage_upserts_are_idempotent_by_id`; `page_usage_filters_by_range_agent_and_session`; `page_usage_pages_newest_first_with_a_cursor`; `deleting_a_session_or_an_agent_keeps_usage_rows`. The SQLite and memory stores run them; the Postgres store's `#[sqlx::test]` twin follows its existing `#[ignore]`-without-database convention.

**Steps:**

- [ ] **Step 1:** Run the precondition command. Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- usage history::conformance 2>&1 | tail -30`).
- [ ] **Step 2:** Implement types, pricing, summary, and the three stores. Re-run; then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history 2>&1 | tail -20`. Expected: PASS.
- [ ] **Step 3:** Hand-check the Postgres statements against the SQLite twin (same columns, `ON CONFLICT (id) DO UPDATE`, `ORDER BY created_at_ms DESC, id DESC`, `LIMIT $n`); record in the commit body that they were not run here.
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage by path; commit:

```bash
git commit -m "feat(daemon): add usage records, pricing, and the history store's usage methods

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Daemon: control plane version 10 with pricing overrides

**Model tier:** sonnet.

**Files:**

- Modify: `hosts/rust-daemon/src/control_plane_store.rs`, `hosts/rust-daemon/src/state.rs`, `hosts/rust-daemon/src/app/persistence.rs`, `hosts/rust-daemon/README.md`

This is M6's `8c50a05` again in miniature: read that commit's diff for `control_plane_store.rs`, `app/persistence.rs`, and `state.rs` first and mirror its shape.

**Interfaces (exact):**

```text
// control_plane_store.rs
pub(crate) const CONTROL_PLANE_STORE_VERSION: u32 = 10;
pub(crate) const AUTOMATIONS_STORE_VERSION: u32 = 9;
pub(crate) const PRE_USAGE_BACKUP_SUFFIX: &str = ".pre-usage.bak";
pub(crate) fn pre_usage_backup_path(path: &Path) -> PathBuf;
// pre_upgrade_backup_path: ... else if loaded_version < AUTOMATIONS_STORE_VERSION { pre_automations_backup_path } else { pre_usage_backup_path }
// ControlPlaneSnapshot gains:  #[serde(default)] pricing_overrides: Vec<PricingOverride>     // JSON "pricingOverrides"

// state.rs  (DaemonState)
pub(crate) pricing_overrides: Vec<PricingOverride>,
/// Replaces the list; returns the previous one (for a rollback after a failed save).
pub(crate) fn set_pricing_overrides(&mut self, overrides: Vec<PricingOverride>) -> Vec<PricingOverride>;
// snapshot(): writes it; restore: validate_overrides on the saved list; a list that fails validation is dropped whole with a `warn!` (a restore never fails on it).
```

**Rules:**

- The first start of v10 over a v9 snapshot writes the `.pre-usage.bak` backup (JSON) or `control_plane.backup.9` (Postgres) **before** any v10 save, through the existing `write_pre_upgrade_backup` flow; a start that already found a v10 snapshot writes none. A snapshot with version above 10 is refused (existing message).
- Older fields, `RunStepUsage` without timing, and snapshots without `pricingOverrides` load unchanged (`#[serde(default)]`).
- README (`hosts/rust-daemon/README.md`): the "Rolling back" note for M8: the control plane is version 10 and a pre-M8 daemon refuses it; restore `<file>.pre-usage.bak` (or the Postgres backup row `control_plane.backup.9`) to go back, losing pricing overrides and any state saved since.

**Tests** (model on M6's version-9 tests in `control_plane_store.rs` and `app/persistence.rs`):

- `a_version_nine_snapshot_loads_and_is_backed_up`: a v9 JSON file loads with empty `pricing_overrides`; the backup file equals the original bytes and exists before the first v10 save; the saved file is version 10.
- `a_version_ten_snapshot_writes_no_backup`.
- `backup_paths_follow_the_loaded_version`: loaded 8 gives `.pre-automations.bak`, loaded 9 `.pre-usage.bak`, loaded 4 `.pre-sessions.bak` (the table of `pre_upgrade_backup_path`).
- `a_snapshot_newer_than_ten_is_refused`.
- `the_postgres_backup_key_for_version_nine` (`control_plane.backup.9`).
- `pricing_overrides_round_trip_through_a_snapshot` and `invalid_saved_overrides_are_dropped_with_a_warning_not_fatal` (an oversized rate; the state restores with an empty list and no panic).
- `the_version_constants` (10, 9, and the suffix).

**Steps:**

- [ ] **Step 1:** Run the precondition command; read `git show 8c50a05 -- hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/app/persistence.rs`. Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- control_plane_store persistence 2>&1 | tail -30`).
- [ ] **Step 2:** Implement. Re-run; then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib 2>&1 | tail -15` (the whole library suite: other tests assert the version and may need `CONTROL_PLANE_STORE_VERSION` instead of a literal 9; fix only those).
- [ ] **Step 3:** `grep -rn "version, 9\|== 9\|: u32 = 9\|\"version\": 9" hosts/rust-daemon/src hosts/rust-daemon/tests` and confirm each hit is a deliberate v9 fixture.
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage by path (README included); commit:

```bash
git commit -m "feat(daemon): move the control plane to version 10 with pricing overrides

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Daemon: record usage for run steps and secondary calls

**Model tier:** opus (outbox ordering under the control-plane transaction, durability of the run-derived path, and the cancellation behavior of the secondary-call wrapper).

**Files:**

- Create: `hosts/rust-daemon/src/usage/metered.rs`
- Modify: `hosts/rust-daemon/src/history/outbox.rs` (usage queue, `write_usage`, derive in `write_runs`, `FlushReport.usage`), `state/run_commit.rs` (`rollback_run` keeps steps), `agent_runs/titles.rs`, `agent_runs/compact.rs`, `routes/profile.rs`, `routes/agencies.rs`, and the tests beside each (`agent_runs/title_tests.rs`, `agent_runs/compaction_tests.rs`, `routes/profile.rs`'s and `routes/agencies.rs`'s test modules, `history/outbox.rs`'s test module, `history/conformance.rs`'s `FlakyHistoryStore`)

**Interfaces (exact):**

```text
// usage/metered.rs
/// A `ModelAdapter` that forwards to `inner` and remembers each completed `generate` (its usage, start time, duration). A failed or cancelled call remembers nothing. `stream` is not overridden, so the default (`generate` then `Final`) is metered too.
pub(crate) struct Metered { inner: Arc<dyn ModelAdapter>, calls: StdMutex<Vec<MeteredCall>> }     // never held across .await
pub(crate) struct MeteredCall { pub(crate) usage: TokenUsage, pub(crate) at_ms: u64, pub(crate) duration_ms: u64 }
impl Metered { pub(crate) fn new(inner: Arc<dyn ModelAdapter>) -> Self; pub(crate) fn take(&self) -> Vec<MeteredCall>; }

pub(crate) struct SecondaryCall { pub(crate) agent_id: String, pub(crate) session_id: Option<String>, pub(crate) source: UsageSource, pub(crate) provider: String, pub(crate) model: String }
pub(crate) const USAGE_NO_AGENT: &str = "system";    // agent id for a call made for no agent (profile or agency generation without one)
/// Prices the calls the meter holds with the overrides now in force (a short state read lock) and queues the rows on the history service. Ids are `usage_<uuid-v4>`. A no-op when the meter is empty.
pub(crate) async fn record_secondary(state: &SharedDaemonState, meter: &Metered, call: SecondaryCall);

// history/outbox.rs
impl HistoryService { pub(crate) fn enqueue_usage(&self, records: Vec<UsageRecord>); }    // bounded by USAGE_QUEUE_MAX; drops the oldest past the bound with one warn! per overflow burst
FlushReport { ..., pub(crate) usage: usize }
```

**Rules:**

- **Run steps (derived, durable).** In `write_runs`, before `upsert_runs(&runs)`, read the overrides under the same control-plane transaction hold that read the runs (`state.read().await.pricing_overrides.clone()` inside the existing block), build `usage_records_for_run` for every run in the batch, and `upsert_usage` them (skipped when empty). Both writes are idempotent by id, so a failure of either leaves the runs unmirrored and the next flush repeats both. The prices are those in force at the first mirror (within about a second of the run's end, or after a restart); a re-mirror of a changed record re-prices with the overrides then in force (accepted, Risks).
- **Secondary calls (queued, best-effort).** `enqueue_usage` pushes to an in-memory `VecDeque<UsageRecord>` that is **separate from the message queue** (usage rows are never ordered against deletions: usage survives them). `write_usage` runs in `flush_locked` after `write_schedule_fires`: clone up to `HISTORY_USAGE_BATCH` from the front, `upsert_usage`, then pop exactly those; a failure leaves them queued (the flush already reports the error and backs off). The queue is lost on a crash within about a second of a title or compaction call; accepted (Risks). Past `USAGE_QUEUE_MAX` the oldest are dropped with one `warn!`.
- **Stopped and failed runs.** The ledger record must carry the steps and usage the run spent even when it did not commit: in `rollback_run`, and in every other path that finishes a run record without `commit_run` having copied them (find them with `grep -n "\.finish(" hosts/rust-daemon/src/state hosts/rust-daemon/src/agent_runs hosts/rust-daemon/src/runs`), set `record.steps = self.live.runs().steps(run_id)` before finishing. `commit_run` already does. Runs finished with no registered live entry keep empty steps. (Runs of an agent deleted before their mirror are never mirrored, so they leave no usage; accepted.)
- **Wiring the four secondary callers** (a wrapper around the adapter they already clone; the wrapped value is passed where `adapter.as_ref()` was):
  - Titles (`agent_runs/titles.rs`): `let meter = Metered::new(adapter);`, call `generate_title(&meter, …)`, then `record_secondary(…, Title, Some(session_id))` **whether the title was usable or not** (the tokens were spent) and also when the owner renamed meanwhile; not on a timeout (the call was dropped).
  - Compaction (`agent_runs/compact.rs`): wrap before `summarize`; record after the `select!` (so a call that completed before a stop or deadline still counts), source `Compaction`, the session's id.
  - Profile (`routes/profile.rs`) and agency (`routes/agencies.rs`: both the team generation and `generate_seed_memories`): wrap, record with source `Profile` / `Agency`, `agent_id` the agent the request is for when it names one, else `USAGE_NO_AGENT`; `session_id` `None`.
  - Provider and model in `SecondaryCall` come from the `AgentConfig` the call used (`config.provider` falls back to `"unknown"`).
- The wrapper adds no logging and no locking beyond its own `StdMutex`.

**Tests:**

- `usage/metered.rs`: `metered_records_each_completed_generate_with_timing` (assert `at_ms` lies between the test's own start and end readings; no sleeps); `a_failed_generate_records_nothing`; `a_cancelled_generate_records_nothing` (drop the future mid-call); `streaming_through_the_default_path_is_counted_once`; `record_secondary_is_a_noop_for_an_empty_meter`; `record_secondary_prices_with_the_overrides_in_force`.
- `history/outbox.rs`: `mirroring_a_run_writes_one_usage_row_per_step`; `a_remirrored_run_does_not_duplicate_usage`; `a_failed_usage_write_keeps_the_run_unmirrored_and_a_retry_succeeds` (use `FlakyHistoryStore`; add `upsert_usage` failure injection to it); `secondary_usage_flushes_in_batches_of_500`; `the_usage_queue_drops_the_oldest_past_its_bound`; `usage_rows_survive_deleting_the_session_and_the_agent`; `usage_rows_use_the_overrides_at_mirror_time`; `steps_past_the_cap_add_one_remainder_row` (end to end through `flush_once`).
- `state/run_commit.rs`: `a_rolled_back_run_keeps_its_steps_and_usage`.
- Callers: `a_title_call_records_usage_with_source_title` (also: an unusable title still records; a timeout records nothing); `compaction_records_usage_with_source_compaction`; `a_cancelled_compaction_records_nothing_for_the_dropped_call`; `profile_generation_records_usage_with_source_profile`; `agency_generation_records_usage_for_the_team_and_the_seed_memories`.
- End to end (`agent_runs/live_tests.rs` or `routes/tests/runs.rs`, following the scripted-adapter pattern there): `a_completed_run_records_a_usage_row_per_model_call_after_a_flush` (two model calls with a tool between; rows have source `chat`, the run's provider and model, `session_id`, and `cost_micros` from a table model); `a_stopped_run_records_its_partial_usage_row` (the Task 1 `Usage` frame, then stop).

**Steps:**

- [ ] **Step 1:** Run the precondition command. Write the tests (they need only the signatures); confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- metered outbox title_tests compaction_tests run_commit 2>&1 | tail -30`).
- [ ] **Step 2:** Implement. Re-run the same, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history agent_runs routes::profile routes::agencies 2>&1 | tail -20`. Expected: PASS.
- [ ] **Step 3:** `git diff --stat -- hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/connectors/runtime.rs` shows nothing; `grep -rn "Metered::new" hosts/rust-daemon/src | grep -v test` lists exactly the four callers (five sites).
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage by path; commit:

```bash
git commit -m "feat(daemon): record usage for run steps, titles, compaction, and generation

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Daemon: usage routes, CSV, pricing routes, and session usage totals

**Model tier:** sonnet.

**Files:**

- Create: `hosts/rust-daemon/src/routes/usage.rs`, `routes/contracts/usage.rs`, `routes/tests/usage.rs`
- Modify: `routes/mod.rs` (five handlers on four paths, `ApiDoc` paths and the `usage` tag, the test module line), `routes/contracts/mod.rs`, `routes/sessions.rs` (`get_session` only), `routes/contracts/sessions.rs` (`SessionResponse.usage`)

**Interfaces (exact):**

```text
// routes/contracts/usage.rs  (serde camelCase, ToSchema)
UsageRecordResponse (the UsageRecord JSON), UsageTotalsResponse, UsageGroupResponse { key, totals }, UsageSummaryResponse { from, to, group_by: Option<String>, tz_offset_minutes: i32, totals, groups, truncated },
UsageRecordsEnvelope { records, next_cursor: Option<String> },
PricingOverrideBody (== PricingOverride), PricingEnvelope { overrides: Vec<PricingOverrideBody>, table_date: String }   // table_date = anima_model_adapters::PRICING_TABLE_DATE
PricingPutRequest { overrides }     // deny_unknown_fields

// routes/usage.rs
pub(super) async fn usage_summary(State(AppState), request: Request) -> Response;
pub(super) async fn usage_records(State(AppState), request: Request) -> Response;
pub(super) async fn usage_export(State(AppState), request: Request) -> Response;
pub(super) async fn get_pricing(State(AppState), request: Request) -> Response;
pub(super) async fn put_pricing(State(AppState), request: Request) -> Response;
fn csv_cell(value: &str) -> String;            // quotes on , " CR LF; prefixes ' when the cell starts with = + - @ TAB or CR
const CSV_HEADER: &str = "id,createdAt,agentId,sessionId,runId,source,provider,model,promptTokens,completionTokens,cachedPromptTokens,reasoningTokens,totalTokens,costUsd,pricingSource,durationMs";
```

Routes: `.route("/api/usage/summary", get(usage::usage_summary))`, `/api/usage/records`, `/api/usage/export.csv`, `.route("/api/usage/pricing", get(usage::get_pricing).put(usage::put_pricing))`.

**Rules:**

- Query via `http::request_query` (a malformed query is 400 with the first applicable constant). **Range:** `from`/`to` are epoch ms; `to` defaults to `now_ms + 1`, `from` to `to - 30 days`; both present must satisfy `from < to` and `to - from <= 366 days`, else 400 `USAGE_RANGE_INVALID`. `groupBy` absent means totals only (`groups: []`, `groupBy: null`); an unknown value is 400 `USAGE_GROUP_INVALID`. `tzOffsetMinutes` default 0, integer -840 to 840 else 400 `USAGE_TZ_INVALID`. `agentId` and `sessionId` filters are exact. Read the store through `state.daemon.read().await.history.store()` (clone the `Arc`, drop the guard, then await the store).
- Records: `limit` default 50, 1 to 200 else `USAGE_LIMIT_INVALID`; cursor is `<createdAtMs>:<id>` (split at the first `:`), a bad one is `USAGE_CURSOR_INVALID`; fetch `limit + 1` to know whether a `nextCursor` exists.
- CSV: oldest first; pages the store newest first through `page_usage` and reverses; more than `MAX_CSV_ROWS` rows is 400 `USAGE_EXPORT_TOO_LARGE`; `createdAt` is ISO-8601 UTC; `costUsd` is micros / 1,000,000 with six decimals, blank when null; every text cell goes through `csv_cell`; line endings `\n`; a header-only file for an empty range. The filename's dates are the UTC dates of `from` and `to - 1`.
- Pricing PUT: owner `authorize(…, false)`; body through the limited-read helper (`state.config.max_request_bytes`); `validate_overrides` (400 with its message); then **`tokio::spawn`** the whole body: `state.agent_runs.control_plane_transaction().await` → `state.daemon.write().await.set_pricing_overrides(list)` → `persist.save().await` (on failure restore the previous list and answer 503 with the error text, exactly as `put_approval_policy` does) → drop the transaction → 200 `PricingEnvelope`. A panicked or cancelled task is 503 `USAGE_TASK_FAILED`. The new prices apply to calls mirrored afterwards; stored rows keep their price.
- **Session usage:** `get_session` adds `usage` (`UsageTotalsResponse`) from `summarize(store, session filter, from 0, to now+1, no grouping)`; a store error or a truncated scan omits it (`null`) rather than failing the GET. The list endpoint, `view=summary`, and every other session response are unchanged (no `usage` key).
- Reads set `no-store`; the CSV also sets `x-content-type-options: nosniff`.

**Tests** (`routes/tests/usage.rs`; seed rows through the history store handle with `upsert_usage`; helpers and `OWNER_ORIGIN` as in `routes/tests/skills.rs`):

- `the_usage_routes_refuse_a_non_owner`: all five handlers answer 403 `no-store` with a foreign origin; the pricing list is unchanged after a foreign PUT.
- `summary_totals_and_groups`: rows for three agents; `groupBy=model|source|day` give the expected sums (day with `tzOffsetMinutes=480`); `agentId` and `sessionId` filters narrow; no `groupBy` returns `groups: []`.
- `summary_validates_its_query`: each of `USAGE_RANGE_INVALID` (from after to; 367 days; non-numeric), `USAGE_GROUP_INVALID`, `USAGE_TZ_INVALID`.
- `summary_defaults_to_the_last_thirty_days` (rows at 31 days old are excluded; build rows relative to the response's own `from`/`to`, not the wall clock).
- `records_page_newest_first_with_a_cursor` (limit 2 over 5 rows: three pages, the last with `nextCursor: null`); `records_validate_limit_and_cursor`.
- `csv_has_the_header_rows_and_safe_cells` (a model named `=cmd|calc` is exported as `'=cmd|calc`; a comma and a quote are quoted; null cost is blank; oldest first; the header equals `CSV_HEADER`); `csv_for_an_empty_range_is_only_the_header`; `csv_refuses_more_than_the_row_cap` (inject a small cap through a `#[cfg(test)]` parameter on the inner function; `MAX_CSV_ROWS` stays 100,000 and is asserted once in `constants`).
- `get_pricing_starts_empty_and_reports_the_table_date`; `put_pricing_replaces_the_list_and_prices_later_calls` (PUT an override for a table model; a run mirrored afterwards is priced `override`; an earlier row keeps its price); `put_pricing_validates` (each `PRICING_*` constant, unknown field 400); `a_failed_save_restores_the_pricing_and_answers_503` (the failing-persist fixture used by the approvals policy test); `a_dropped_pricing_put_still_finishes` (hold the control-plane transaction, poll the PUT once, drop it, release, then take the transaction again and assert the list changed; no sleeps, a bounded `yield_now` loop as in M7's drop test).
- `session_get_includes_usage_totals_and_the_list_does_not`; `session_usage_is_null_when_the_store_fails`.
- `the_usage_routes_are_in_the_openapi_document` (all four paths, their methods, and the `usage` tag).
- `constants` (one assertion per exported constant of `routes/usage.rs`, and of `usage/mod.rs` and `usage/pricing.rs`: 30, 366, 50, 200, 2,000, 200,000, 20, 100,000, 10,000, 500, 100, 1,000,000,000,000, 64, 128).

**Steps:**

- [ ] **Step 1:** Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::usage 2>&1 | tail -30`).
- [ ] **Step 2:** Implement contracts, handlers, routes, `ApiDoc`. Re-run; then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::sessions 2>&1 | tail -15` (the existing session tests still pass: the `usage` key appears only on GET detail).
- [ ] **Step 3:** `grep -n "usage::" hosts/rust-daemon/src/routes/mod.rs | head` shows the five handlers and their `ApiDoc` entries.
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage by path; commit:

```bash
git commit -m "feat(daemon): add the usage routes, CSV export, and pricing overrides

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Daemon: the log ring buffer, the tracing layer, and redaction

**Model tier:** opus (secret redaction is a security rule, and the layer runs inside every `tracing` call on every thread).

**Files:**

- Create: `hosts/rust-daemon/src/logs.rs`, `hosts/rust-daemon/src/logs/redact.rs`
- Modify: `hosts/rust-daemon/src/lib.rs` (`mod logs;` and `pub use logs::init_tracing;`), `hosts/rust-daemon/src/main.rs` (delete its own `init_tracing` and the `EnvFilter` import; call `anima_daemon::init_tracing()`)

**Interfaces (exact):**

```text
// logs.rs
pub(crate) const LOG_BUFFER_LINES: usize = 2_000;      pub(crate) const LOG_LINE_MAX_BYTES: usize = 4_096;
pub(crate) const LOG_REDACTED: &str = "[redacted]";    pub(crate) const LOG_TRUNCATED_SUFFIX: &str = "…[truncated]";
pub(crate) const LOG_BROADCAST_CAPACITY: usize = 256;  const LOG_INPUT_MAX_BYTES: usize = 65_536;   // read before redaction

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)] #[serde(rename_all = "lowercase")]
pub(crate) enum LogLevel { Trace, Debug, Info, Warn, Error }       // Ord: Trace < Error
impl LogLevel { pub(crate) fn parse(text: &str) -> Option<Self>; pub(crate) fn as_str(self) -> &'static str }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)] #[serde(rename_all = "camelCase")]
pub(crate) struct LogLine { pub(crate) seq: u64, pub(crate) at: u64, pub(crate) level: LogLevel, pub(crate) target: String, pub(crate) message: String }

#[derive(Clone, Debug, Default)]
pub(crate) struct LogFilter { pub(crate) min_level: Option<LogLevel>, pub(crate) query: Option<String> }   // query lowercased by the constructor
impl LogFilter { pub(crate) fn new(min_level: Option<LogLevel>, query: Option<&str>) -> Self; pub(crate) fn matches(&self, line: &LogLine) -> bool }   // level >= min; query is a case-insensitive substring of message or target

pub(crate) struct LogBuffer { /* StdMutex<Inner>, broadcast::Sender<Arc<LogLine>>, AtomicUsize streams */ }
impl LogBuffer {
    pub(crate) fn new() -> Arc<Self>;                                   // LOG_BUFFER_LINES, LOG_BROADCAST_CAPACITY
    pub(crate) fn with_limits(lines: usize, channel: usize) -> Arc<Self>;   // for tests
    /// Sanitizes (control characters and ANSI removed), redacts, truncates, assigns the next seq, stores, and broadcasts. Returns the seq.
    pub(crate) fn push(&self, at_ms: u64, level: LogLevel, target: &str, message: &str) -> u64;
    /// Oldest first. No `after`: the newest `limit` matches. With `after`: the first `limit` matches with seq > after.
    pub(crate) fn lines(&self, filter: &LogFilter, after: Option<u64>, limit: usize) -> Vec<LogLine>;
    pub(crate) fn newest_seq(&self) -> u64;                             // 0 when empty
    pub(crate) fn buffered(&self) -> usize;
    /// The lines with seq > after and a receiver, taken under one lock hold, so no line falls between them.
    pub(crate) fn subscribe_after(&self, after: u64) -> (Vec<Arc<LogLine>>, broadcast::Receiver<Arc<LogLine>>);
    pub(crate) fn try_open_stream(self: &Arc<Self>, max: usize) -> Option<StreamGuard>;   // Drop frees the slot
}
pub(crate) fn global() -> Arc<LogBuffer>;                               // process-wide, OnceLock

pub(crate) struct LogLayer { buffer: Arc<LogBuffer> }
impl LogLayer { pub(crate) fn new(buffer: Arc<LogBuffer>) -> Self }
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogLayer { fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) }

/// Env filter (default `anima_daemon=info,tower_http=info`), the compact fmt layer, and the log layer on `global()`.
pub fn init_tracing();

// logs/redact.rs
pub(crate) fn redact(text: &str) -> String;
pub(crate) fn is_secret_field(name: &str) -> bool;    // name (lowercased) contains api_key, apikey, secret, token (but not ending in "tokens"), password, passwd, authorization, credential, cookie, private_key, signature
```

**Tricky piece: the layer and `init_tracing`** (complete):

```rust
impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = LineVisitor::default();
        event.record(&mut visitor);
        // Everything above runs without the buffer lock; `push` takes it once, briefly,
        // and neither it nor anything it calls may log.
        self.buffer.push(
            anima_core::primitives::now_millis(),
            LogLevel::from(*meta.level()),
            meta.target(),
            &visitor.finish(),
        );
    }
}

pub fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("anima_daemon=info,tower_http=info"));
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_target(false).compact())
        .with(LogLayer::new(global()))
        .try_init();
}
```

`LineVisitor` implements `tracing::field::Visit`: the `message` field becomes the line's text; every other field is appended as ` name=value` (strings unquoted when they hold no spaces, otherwise Debug-quoted); a field for which `is_secret_field(name)` is true is appended as `name=[redacted]` **without reading its value**. `finish()` returns the joined text.

**Rules:**

- `push` order: cap the input to `LOG_INPUT_MAX_BYTES` (on a char boundary), replace every control character (including newline and ESC sequences like `\x1b[31m`, which are removed whole) with a single space (tab too), run `redact`, then cut to `LOG_LINE_MAX_BYTES` **including** the `LOG_TRUNCATED_SUFFIX` on a char boundary. Redaction runs on the whole capped text **before** the cut, so a secret straddling byte 4,096 is replaced whole and never half-shown.
- `redact` replaces each secret with `LOG_REDACTED`, keeping the label: build the patterns once in a `OnceLock` and apply them in this order. (1) `Authorization` / `Proxy-Authorization` values, with an optional `Bearer|Basic|Token` word. (2) `Bearer <token>` anywhere. (3) `key=value`, `key: value`, and JSON `"key":"value"` where the key contains `api_key`, `apikey`, `secret`, `token`, `password`, `passwd`, `credential`, `cookie`, `session_id`, or `private_key`, except a key that ends in `tokens`, `token_count`, or `token_limit` (so `prompt_tokens=120` and `max_tokens=2000` survive). (4) URL query values after `?` or `&` for `key`, `api_key`, `apikey`, `token`, `access_token`, `refresh_token`, `id_token`, `client_secret`, `secret`, `password`, `code`, `state`, `signature`, `sig`, `auth`. (5) Telegram bot tokens: `bot<digits>:<35ish chars>` inside URLs and bare `<6-12 digits>:<30+ chars of [A-Za-z0-9_-]>`. (6) Known key prefixes followed by 8+ key characters: `sk-`, `sk-ant-`, `sk-proj-`, `pk-`, `rk-`, `AIza`, `ghp_`, `gho_`, `ghu_`, `ghs_`, `github_pat_`, `xox[abprs]-`, `ya29.`, `glpat-`, and `AKIA[0-9A-Z]{16}`. (7) JWTs (`eyJ…`, three dot-separated segments). (8) Unlabeled long tokens: `[A-Za-z0-9_-]{32,}` containing a lowercase letter, an uppercase letter, **and** a digit (this keeps `run_<uuid>` ids, lowercase hex hashes, and plain words; it will occasionally hide an innocent long mixed-case identifier, which is the right side to err on). The `regex` crate guarantees linear time, so no pattern needs a size guard beyond the 64 KiB input cap.
- The buffer is a `VecDeque<Arc<LogLine>>` capped at `lines` (oldest dropped); `seq` starts at 1 and never repeats; `push` holds the `std::sync::Mutex` only to assign the seq, store, and `tx.send` (ignore the error when nobody listens). A poisoned lock is recovered with `into_inner`. No `.await`, and no `tracing` macro, anywhere in `logs.rs` or `logs/redact.rs` outside `#[cfg(test)]`.
- `try_open_stream` increments an `AtomicUsize` only when below `max` (a compare-and-swap loop) and returns a guard whose `Drop` decrements.
- `global()` is created on first use; a second `init_tracing` call (tests, `app()` called twice) is a no-op through `try_init`.

**Tests:**

- `logs/redact.rs`: `redacts_every_secret_shape` (table: an `Authorization: Bearer abc.def-ghi` header; `Bearer` alone; `api_key=sk-ant-…`; `{"access_token":"…","refresh_token":"…"}`; `client_secret=…`; `password: hunter2hunter2`; `Cookie: session_id=…`; a URL `https://x/cb?code=…&state=…&ok=1`; `https://api.telegram.org/bot123456789:AAE_…35chars…/getUpdates` inside an error sentence; a bare Telegram token; `sk-`, `AIza`, `ghp_`, `xoxb-`, `ya29.` shapes; a JWT; an unlabeled 40-character mixed-case token; each secret substring is absent from the output and `[redacted]` is present); `keeps_the_label_and_the_rest_of_the_line` (`token=abc other=1` keeps `token=` and `other=1`); `leaves_ordinary_text_alone` (`prompt_tokens=120 completion_tokens=45 max_tokens=2000`, `run_3f2b8c0e-1111-4222-8333-444455556666`, a 64-character lowercase hex hash, a Windows path, a sentence with the word "token" and no value); `redaction_is_idempotent`; `case_insensitive_labels`; `several_secrets_on_one_line`; `redaction_is_linear_on_large_input` (64 KiB of `=` and of `a`, finishes and keeps its length bound); `is_secret_field_names` (api_key, X-Api-Key, access_token, client_secret, Authorization yes; `prompt_tokens`, `max_tokens`, `session`, `agent_id` no).
- `logs.rs`: `captures_message_target_level_and_time` (use `tracing::subscriber::with_default` on a local `Registry` with the layer and a private buffer; assert `at` lies between the test's own `now_millis()` readings); `fields_are_appended_as_key_value`; `a_secret_field_name_redacts_its_value_without_reading_it` (a field whose Debug impl panics is never formatted); `a_secret_in_the_message_is_redacted`; `control_characters_and_ansi_are_removed`; `capture_is_bounded` (push 2,500: 2,000 kept, the oldest seq is 501, seqs contiguous); `a_long_line_is_cut_on_a_char_boundary_with_the_suffix` (multi-byte characters across byte 4,096; valid UTF-8; `len() <= 4096`; ends with the suffix); `redaction_survives_truncation_boundaries` (a 40-character secret placed to straddle byte 4,096 never appears, whole or in part); `seq_never_repeats_under_concurrent_pushes` (8 threads by 200 pushes: 1,600 distinct seqs, no panic); `a_poisoned_lock_does_not_stop_logging` (poison it on purpose, push again); `level_filter_orders_error_above_warn_above_info`; `query_filter_is_case_insensitive_over_message_and_target`; `subscribe_after_returns_the_backlog_and_then_live_lines_without_a_gap` (push 3, subscribe after 1: backlog is 2 and 3; push 4: the receiver yields 4 only); `a_stream_slot_is_freed_when_its_guard_drops` (max 2: third refused; drop one: accepted); `the_layer_emits_no_events_of_its_own` (install a counting layer beside it; N events in, N counted); `the_logging_modules_contain_no_tracing_macros` (a `include_str!` of `logs.rs` and `logs/redact.rs`, cut at `#[cfg(test)]`, contains none of `info!(`, `warn!(`, `error!(`, `debug!(`, `trace!(`); `init_tracing_twice_does_not_panic`; `constants` (2,000; 4,096; `[redacted]`; `…[truncated]`; 256).

**Steps:**

- [ ] **Step 1:** Run the precondition command. Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- logs 2>&1 | tail -30`).
- [ ] **Step 2:** Implement. Re-run. Then `CARGO_INCREMENTAL=0 cargo build -p anima-daemon 2>&1 | tail -10` (the bin builds with `anima_daemon::init_tracing`) and `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::worker 2>&1 | tail -10` (that module builds its own subscriber and must still pass).
- [ ] **Step 3:** `grep -n "init_tracing\|EnvFilter" hosts/rust-daemon/src/main.rs` prints only the call.
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage by path; commit:

```bash
git commit -m "feat(daemon): capture a redacted, bounded log tail

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Daemon: log routes with the live stream

**Model tier:** sonnet.

**Files:**

- Create: `hosts/rust-daemon/src/routes/logs.rs`, `routes/contracts/logs.rs`, `routes/tests/logs.rs`
- Modify: `routes/mod.rs` (two routes, `ApiDoc` paths and the `logs` tag, the `AppState.logs: Arc<LogBuffer>` field, the router builder's parameter, the test module line), `routes/contracts/mod.rs`, `app.rs` (passes `logs::global()` where it builds the router)

**Interfaces (exact):**

```text
// routes/contracts/logs.rs
LogLineResponse (the LogLine JSON), LogsEnvelope { lines: Vec<LogLineResponse>, newest_seq: u64 }

// routes/logs.rs
pub(super) async fn list_logs(State(AppState), request: Request) -> Response;
pub(super) async fn stream_logs(State(AppState), request: Request) -> Response;
const DEFAULT_LOGS_LIMIT: usize = 200;  const MAX_LOGS_LIMIT: usize = 1_000;  const MAX_LOG_STREAMS: usize = 8;  const MAX_LOG_QUERY_CHARS: usize = 200;
// + the five string constants in Global Constraints
```

Routes: `.route("/api/logs", get(logs::list_logs))`, `.route("/api/logs/stream", get(logs::stream_logs))`. `AppState` takes the buffer as a builder parameter so production passes `logs::global()` and every test builds its own `LogBuffer::new()` (or `with_limits`) through a `router_with_logs` test helper next to `router_with_runs`.

**Rules:**

- Both routes: owner `authorize(…, true)`; query via `request_query`; `level` through `LogLevel::parse` (400 `LOGS_LEVEL_INVALID`); `q` at most 200 chars (400 `LOGS_QUERY_TOO_LONG`); `after` a whole number (400 `LOGS_AFTER_INVALID`); `limit` (list only) 1 to 1,000 (400 `LOGS_LIMIT_INVALID`), default 200. The list answers `{ lines, newestSeq }`, oldest first. The buffer holds only what the subscriber lets through (default `anima_daemon=info,tower_http=info`), already redacted at capture.
- The stream: 429 `LOGS_TOO_MANY_STREAMS` past `MAX_LOG_STREAMS` open streams (the guard lives in the stream state, so a closed connection frees its slot). Without `after`, only new lines are sent; with `after`, the backlog after it first (up to the buffer), then live lines, filtered by `level` and `q`. SSE events: `event: log` with `LogLine` JSON as data; `event: resync` with `{"newestSeq": n}` when the receiver lagged (the client refetches the list `after` its newest seq). Keep-alive uses `live::EVENT_KEEP_ALIVE_SECS`; `x-accel-buffering: no`; `no-store`.

**Tricky piece: the stream body** (complete shape; the implementer fills the event builders):

```rust
fn log_events(
    buffer: Arc<LogBuffer>,
    filter: LogFilter,
    after: u64,
    guard: StreamGuard,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let (backlog, receiver) = buffer.subscribe_after(after);
    let state = (VecDeque::from(backlog), receiver, after, buffer, filter, guard);
    stream::unfold(state, |(mut pending, mut receiver, mut last, buffer, filter, guard)| async move {
        loop {
            while let Some(line) = pending.pop_front() {
                if line.seq <= last {
                    continue; // already sent (the backlog and the receiver can overlap)
                }
                last = line.seq;
                if filter.matches(&line) {
                    let event = log_event(&line);
                    return Some((Ok(event), (pending, receiver, last, buffer, filter, guard)));
                }
            }
            match receiver.recv().await {
                Ok(line) => pending.push_back(line),
                Err(RecvError::Lagged(_)) => {
                    last = buffer.newest_seq();
                    let event = resync_event(last);
                    return Some((Ok(event), (pending, receiver, last, buffer, filter, guard)));
                }
                Err(RecvError::Closed) => return None,
            }
        }
    })
}
```

**Tests** (`routes/tests/logs.rs`; own buffer per test, lines pushed with `buffer.push`; SSE read as in `routes/tests/events.rs`):

- `the_log_routes_refuse_a_non_owner` (list and stream: 403, `no-store`).
- `list_returns_the_newest_limit_oldest_first`; `list_after_returns_only_newer_lines_up_to_the_limit`; `list_filters_by_level_and_query`; `list_validates_its_query` (each of the four constants, plus `limit=0`, `limit=1001`, `after=-1`).
- `a_secret_logged_through_the_layer_never_reaches_the_response` (a local subscriber with the layer on the test's buffer: `tracing::warn!("call failed key=sk-ant-api03-AAAA…")`; the list body and a stream event lack the secret).
- `a_stream_gets_the_backlog_then_live_lines_without_a_gap` (push 1..3; connect with `after=1`: events 2, 3; push 4: event 4; no duplicate of 3).
- `a_stream_without_after_sends_only_new_lines`.
- `a_lagged_stream_sends_resync_then_continues` (`with_limits(100, 2)`: push 6 lines before the first read; the first event is `resync` with `newestSeq` 6; a line pushed afterwards arrives).
- `the_stream_applies_its_level_and_query_filters`.
- `too_many_streams_answer_429_and_a_closed_stream_frees_its_slot` (open 8, the 9th is 429 `LOGS_TOO_MANY_STREAMS`; drop one body; the next opens).
- `the_log_routes_are_in_the_openapi_document`; `constants` (200, 1,000, 8, 200, and the five strings).

**Steps:**

- [ ] **Step 1:** Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::logs 2>&1 | tail -30`).
- [ ] **Step 2:** Implement. Re-run; then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests 2>&1 | tail -15` (the builder signature change compiles everywhere).
- [ ] **Step 3:** `cargo fmt --all`; `git diff --stat`; stage by path; commit:

```bash
git commit -m "feat(daemon): add the log tail routes with a live stream

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Daemon: status aggregate, richer metrics, and the queue-cap carry-over

**Model tier:** sonnet.

**Files:**

- Create: `hosts/rust-daemon/src/routes/status.rs`, `routes/contracts/status.rs`, `routes/tests/status.rs`
- Modify: `routes/mod.rs` (the route, `ApiDoc`, `AppState.started_at_ms`, extract the provider list into a shared function), `routes/health.rs` (`handle_metrics` reads the shared snapshot), `runs/ledger.rs` (`status_counts`, `queued_count` semantics), `agent_runs/queue.rs` (steer-join cap), `live/fanout.rs` (`total_subscribers`; drop the two `#[allow(dead_code)]`), `history/outbox.rs` (`HistoryStats`, `flush_errors`, `note_pruned`), `sessions/pruning.rs` (one `note_pruned` call), `hosts/rust-daemon/README.md`

**Interfaces (exact):**

```text
// routes/status.rs
pub(crate) struct StatusSnapshot { /* every number the response and the metrics need */ }
pub(crate) async fn collect(state: &SharedDaemonState, config: &DaemonConfig, logs: &LogBuffer, started_at_ms: u64, now_ms: u64) -> StatusSnapshot;
pub(super) async fn get_status(State(AppState), request: Request) -> Response;

// routes/contracts/status.rs  (camelCase, ToSchema)
StatusResponse {
  version: String,                         // env!("CARGO_PKG_VERSION")
  build_revision: Option<String>,          // option_env!("ANIMAOS_BUILD_REVISION"), null when unset
  started_at_ms: u64, now_ms: u64, uptime_seconds: u64,
  readiness: { status: "ready" | "not_ready", issues: Vec<String> },
  storage: { persistence_mode: String, control_plane: String /* json | postgres | memory */, control_plane_durability: String,
             history: { store: String, ephemeral: bool, healthy: bool, pending_flush: usize, usage_queued: usize,
                        last_error: Option<String> /* redacted, at most 500 chars */, failing_since_ms: Option<u64>, flush_errors: u64 } },
  providers: Vec<{ id, label, configured }>,
  connectors: Vec<{ id, agent_id, #[serde(rename = "type")] connector_type, status, enabled }>,     // at most 50; never a credential
  automations: { total: usize, enabled: usize, failing: usize /* consecutive_failures > 0 */, failures_total: u64 },
  approvals: { pending: usize },
  runs: { running: usize, queued: usize, by_status: BTreeMap<String, usize> },   // runs the ledger still holds
  events: { subscribers: usize, lagged_events: u64 },
  logs: { buffered: usize, newest_seq: u64 },
  limits: { max_request_bytes, max_concurrent_runs, max_runs_per_agent, queued_runs_per_agent: 8, max_background_processes, log_buffer_lines: 2000, event_buffer: 1024 },
}

// runs/ledger.rs
pub(crate) fn status_counts(&self) -> BTreeMap<&'static str, usize>;
// history/outbox.rs
pub(crate) struct HistoryStats { pending: usize, usage_queued: usize, failing_since_ms: Option<u64>, last_error: Option<String>, flush_errors: u64, pruned_messages: u64 }
impl HistoryService { pub(crate) fn stats(&self) -> HistoryStats; pub(crate) fn note_pruned(&self, count: usize); }
// live/fanout.rs
pub(crate) fn total_subscribers(&self) -> usize;
```

**Rules:**

- `collect` clones what it needs under the state read lock (and the history `Arc`, live hub, ledger counts, approval and connector records), drops the guard, then awaits the connector runtime statuses and the ChatGPT status. It never holds the lock across an `.await`. `/api/status` and `/metrics` both call it, so the two cannot disagree. Provider `configured` reuses the `/api/providers` logic (extract `provider_responses(&state)` in `routes/mod.rs`; `list_providers_entry` and `collect` both call it).
- `readiness` is `handle_readiness`'s result. The history `last_error` goes through `logs::redact` and is cut to 500 characters (a database error can echo a URL).
- Status is `authorize(…, true)` + `no-store`. `/metrics` stays unauthenticated and exposes counts only.
- **Metrics (added; the existing lines are unchanged):** gauges `anima_daemon_uptime_seconds`, `anima_daemon_runs_running`, `anima_daemon_runs_queued`, `anima_daemon_runs{status="…"}` (one line per ledger status), `anima_daemon_approvals_pending`, `anima_daemon_event_subscribers`, `anima_daemon_history_pending_flush`, `anima_daemon_history_usage_queued`, `anima_daemon_history_failing` (0 or 1), `anima_daemon_automations_failing`, `anima_daemon_log_lines_buffered`; counters `anima_daemon_event_lagged_total`, `anima_daemon_history_flush_errors_total`, `anima_daemon_messages_pruned_total`. Each with `# HELP` and `# TYPE` lines like the existing ones. The runs-by-status gauge counts runs the ledger still holds (it prunes old terminal runs), documented in the HELP text.
- `flush_errors` increments in `record_result`'s error arm; `note_pruned(n)` is called once after a successful prune save with the number of messages pruned (`sessions/pruning.rs`, the success path after `prune_hot_tail`'s save at the call near line 283; one line).
- **Queue cap (carry-over).** `RunLedger::queued_count(agent_id)` becomes the number of messages **waiting** for the agent: its `Queued` records plus the `pending_steers` held by its other records (doc comment updated). The steer-join path in `agent_runs/queue.rs` checks it before attaching a steer: at 8 or more it answers 429 `QUEUE_FULL` instead of joining. The leftover-steer path that computes open slots already runs after `take_leftover_steers` has removed the run's own steers, so a run's own steers are not counted against themselves; a test pins this. Existing `queue_tests.rs` stay green.
- README: a **Usage, logs, and health (owner)** section with a row per route (method, path, auth, query or body, answers, errors), the notes that pricing overrides are control-plane state (version 10) and priced at first mirror, that logs are redacted at capture and bounded (2,000 lines of 4 KiB), that `/metrics` is unauthenticated counts, and the M8 rolling-back pointer from Task 3.

**Tests:**

- `routes/tests/status.rs`: `status_requires_the_owner`; `status_reports_version_uptime_and_readiness` (a builder taking `now_ms`); `status_lists_providers_without_keys` (serialized body has no `apiKey`); `status_lists_connectors_without_credentials` (seed a Telegram connector with a fake bot token; the serialized body does not contain the token); `status_counts_pending_approvals_runs_and_subscribers` (one pending approval, one running and one queued run, one open event stream); `status_reports_history_health_and_redacts_the_last_error` (`FlakyHistoryStore` error text containing `sk-…` appears as `[redacted]`; `healthy` false while failing); `status_counts_automations_and_failures`; `status_is_in_the_openapi_document`; `limits_match_the_constants`; `build_revision_is_null_when_unset`.
- `routes/health.rs` tests: `metrics_keep_the_existing_lines_and_add_the_new_ones` (every name above present once, plus one `anima_daemon_runs{status=` line per ledger status); `metrics_counters_increase` (a flush error, a prune, a lagged subscriber); `metrics_need_no_authorization` (the API-key middleware exemption is unchanged).
- Queue cap: `ledger.rs`: `pending_steers_count_toward_the_waiting_total`, `a_queued_run_is_counted_once`, `status_counts_lists_every_status`; `agent_runs/queue_tests.rs`: `a_steer_is_refused_with_429_when_eight_messages_wait`, `leftover_steers_use_the_open_slots_after_the_runs_own_are_taken`, `a_full_queue_still_refuses_a_new_run`.
- `live/fanout.rs`: `total_subscribers_sums_every_agent`. `history/outbox.rs`: `stats_report_pending_failures_and_pruned_counts`.

**Steps:**

- [ ] **Step 1:** Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::status routes::health queue_tests ledger fanout 2>&1 | tail -30`).
- [ ] **Step 2:** Implement. Re-run the same, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test '*' 2>&1 | tail -20` (the integration suites still pass, in particular any that read `/metrics`).
- [ ] **Step 3:** `grep -n "Usage, logs, and health" hosts/rust-daemon/README.md` shows the section; `grep -rn "allow(dead_code)" hosts/rust-daemon/src/live/fanout.rs` is empty.
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage by path (README included); commit:

```bash
git commit -m "feat(daemon): add the status aggregate and richer metrics, and count steers toward the queue cap

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 9: SDK: usage, logs, and status clients

**Model tier:** sonnet.

**Files:**

- Create: `packages/sdk/src/usage.ts`, `usage.spec.ts`, `logs.ts`, `logs.spec.ts`, `status.ts`, `status.spec.ts`
- Modify: `packages/sdk/src/client.ts` (three `readonly` clients, constructed beside `automations`), `packages/sdk/src/index.ts` (exports), `packages/sdk/src/sessions.ts` (`Session.usage?: UsageTotals | null`)

Model the specs on `skills.spec.ts` / `events.spec.ts` for how a `DaemonClient` and a fake `fetch` are built.

**Interfaces (exact; exported from `index.ts`):**

```ts
// usage.ts
export type UsageSource =
  | 'chat'
  | 'telegram'
  | 'automation'
  | 'job'
  | 'helper'
  | 'api'
  | 'title'
  | 'compaction'
  | 'profile'
  | 'agency';
export type PricingSource =
  | 'table'
  | 'override'
  | 'free'
  | 'subscription'
  | 'unknown';
export interface UsageRecord {
  id: string;
  agentId: string;
  sessionId: string | null;
  runId: string | null;
  source: UsageSource;
  provider: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  cachedPromptTokens: number;
  reasoningTokens: number;
  totalTokens: number;
  costMicros: number | null;
  pricingSource: PricingSource;
  durationMs: number;
  createdAtMs: number;
}
export interface UsageTotals {
  calls: number;
  promptTokens: number;
  completionTokens: number;
  cachedPromptTokens: number;
  reasoningTokens: number;
  totalTokens: number;
  costMicros: number;
  unpricedCalls: number;
  subscriptionCalls: number;
}
export type UsageGroupBy = 'day' | 'model' | 'source' | 'session';
export interface UsageQuery {
  from?: number;
  to?: number;
  agentId?: string;
  sessionId?: string;
  groupBy?: UsageGroupBy;
  tzOffsetMinutes?: number;
}
export interface UsageGroup {
  key: string;
  totals: UsageTotals;
}
export interface UsageSummary {
  from: number;
  to: number;
  groupBy: UsageGroupBy | null;
  tzOffsetMinutes: number;
  totals: UsageTotals;
  groups: UsageGroup[];
  truncated: boolean;
}
export interface UsageRecordsQuery {
  from?: number;
  to?: number;
  agentId?: string;
  cursor?: string;
  limit?: number;
}
export interface UsageRecordsPage {
  records: UsageRecord[];
  nextCursor: string | null;
}
export interface PricingOverride {
  provider: string;
  model: string;
  inputMicrosPerMtok: number;
  outputMicrosPerMtok: number;
  cachedInputMicrosPerMtok: number | null;
}
export interface Pricing {
  overrides: PricingOverride[];
  tableDate: string;
}
export const MAX_PRICING_OVERRIDES = 100;
export const MAX_PRICE_MICROS_PER_MTOK = 1_000_000_000_000;
export const MAX_USAGE_RECORDS_LIMIT = 200;
export class UsageClient {
  summary(query?: UsageQuery): Promise<UsageSummary>; // GET /api/usage/summary (only set params are sent)
  records(query?: UsageRecordsQuery): Promise<UsageRecordsPage>; // GET /api/usage/records
  exportCsv(query?: {
    from?: number;
    to?: number;
    agentId?: string;
  }): Promise<string>; // GET /api/usage/export.csv through requestText
  pricing(): Promise<Pricing>; // GET /api/usage/pricing
  setPricing(overrides: PricingOverride[]): Promise<Pricing>; // PUT /api/usage/pricing { overrides }
}

// logs.ts
export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';
export const LOG_LEVELS: readonly LogLevel[] = [
  'error',
  'warn',
  'info',
  'debug',
  'trace',
];
/** Untrusted text: the daemon redacts secrets, but a line can still hold text a model or provider wrote. */
export interface LogLine {
  seq: number;
  at: number;
  level: LogLevel;
  target: string;
  message: string;
}
export interface LogsQuery {
  level?: LogLevel;
  q?: string;
  after?: number;
  limit?: number;
}
export interface LogsPage {
  lines: LogLine[];
  newestSeq: number;
}
export type LogEvent =
  | { kind: 'line'; line: LogLine }
  | { kind: 'resync'; newestSeq: number };
export const MAX_LOGS_LIMIT = 1_000;
export class LogsClient {
  list(query?: LogsQuery): Promise<LogsPage>; // GET /api/logs
  /** Ends when the connection closes; reconnecting is the caller's choice. `resync` means refetch the list after your newest seq. */
  stream(options?: {
    level?: LogLevel;
    q?: string;
    after?: number;
    signal?: AbortSignal;
  }): AsyncGenerator<LogEvent>; // GET /api/logs/stream via client.subscribe
}

// status.ts
export interface DaemonStatus {
  /* the StatusResponse JSON of Task 8, camelCase, field for field */
}
export const STATUS_TOO_OLD = 'Update the daemon to see its health.';
export class StatusTooOldError extends Error {
  readonly code = 'daemon_too_old' as const;
} // message STATUS_TOO_OLD
export class StatusClient {
  get(): Promise<DaemonStatus>;
} // GET /api/status; a 404 throws StatusTooOldError (spec §13.4)
```

`client.ts` exposes them as `client.usage`, `client.logs`, `client.status`. The SDK validates nothing (the constants are for callers). `Session.usage` is present only on a single-session `get`.

**Tests:**

- `usage.spec.ts`: `summary sends only the options that are set`; `summary parses totals and groups`; `records sends the cursor and limit`; `exportCsv returns the text and sends the range`; `pricing reads the overrides and the table date`; `setPricing puts the overrides`; `a daemon refusal reaches the caller as DaemonHttpError`; `the limits match the daemon` (100, 1,000,000,000,000, 200).
- `logs.spec.ts`: `list sends the filters`; `stream yields line events in order`; `stream yields a resync event with the newest seq`; `stream skips malformed events with a warning` (spy on `console.warn` and restore it, as `events.spec.ts` does); `stream sends after, level, and q`; `stream stops when the signal aborts`; `the log levels and limit match the daemon`.
- `status.spec.ts`: `get parses the status`; `a 404 becomes StatusTooOldError with the update text`; `other failures stay DaemonHttpError`; `the status types accept the daemon's full JSON` (a typed fixture).
- `sessions.spec.ts` (one case): `a session read keeps its usage totals`.
- `index.spec.ts`: the new exports exist.

**Steps:**

- [ ] **Step 1:** Write the specs; `bun x nx test @animaOS-SWARM/sdk 2>&1 | tail -20`. Expected: FAIL.
- [ ] **Step 2:** Implement; run again. Expected: PASS. `bun x nx run @animaOS-SWARM/sdk:typecheck 2>&1 | tail -10` if the target exists (`bun x nx show project @animaOS-SWARM/sdk` names it).
- [ ] **Step 3:** `bun x nx format:write --files=<each changed file>`; re-run the SDK tests; then `bun x nx run @animaOS-SWARM/sdk:build 2>&1 | tail -10`. Expected: the build succeeds.
- [ ] **Step 4:** Stage by path; commit:

```bash
git commit -m "feat(sdk): add usage, logs, and status clients

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Web data and the Usage page: helpers, hooks, session usage, `/usage`

**Model tier:** sonnet.

**Files:**

- Create: `apps/web/src/lib/usage.ts` (+ `usage.test.ts`), `lib/download.ts` (+ test), `hooks/useUsage.ts` (+ test), `hooks/useSessionUsage.ts` (+ test), `test/usage.ts`, `pages/UsagePage.tsx` (+ test), `components/usage/UsageChart.tsx` (+ test), `components/usage/UsageTable.tsx`, `system.css`
- Modify: `lib/daemon-api.ts` (+ test), `styles.css` (import `system.css`; palette variables only, it must pass `visual-tokens.test.ts`), `components/WorkspaceShell.tsx` (+ test), `components/sessions/SessionView.tsx` (+ test), `lib/slash-commands.ts` (+ test), `hooks/useSessionCommands.ts` (+ test), `ViewHarness.tsx` (wiring only), `ViewHarness.test.tsx`

Precondition: Task 9's SDK build is done. All daemon calls go through the SDK.

**Interfaces (exact):**

```ts
// lib/daemon-api.ts additions to `daemon`
usageSummary: (query: UsageQuery) => setupClient.usage.summary(query),
usageRecords: (query: UsageRecordsQuery) => setupClient.usage.records(query),
exportUsageCsv: (query: { from?: number; to?: number; agentId?: string }) => setupClient.usage.exportCsv(query),
usagePricing: () => setupClient.usage.pricing(),
setUsagePricing: (overrides: PricingOverride[]) => setupClient.usage.setPricing(overrides),

// lib/usage.ts
export const USAGE_RANGES_DAYS = [7, 30, 90] as const;
export type UsageRangeDays = (typeof USAGE_RANGES_DAYS)[number];
export const DEFAULT_USAGE_RANGE_DAYS: UsageRangeDays = 30;
export const UNPRICED = '—';
export function usageRange(days: number, now: Date): { from: number; to: number; tzOffsetMinutes: number };   // to = start of the next local day; from = `days` local days earlier; tzOffsetMinutes = -now.getTimezoneOffset()
export function fillDays(groups: readonly UsageGroup[], range: { from: number; to: number; tzOffsetMinutes: number }): UsageGroup[];   // every day of the range ascending, zero totals for missing days
export function formatTokens(count: number): string;     // 999 -> '999'; 1234 -> '1.2k'; 1000 -> '1k'; 3_400_000 -> '3.4M'
export function formatCost(micros: number | null): string;   // null -> UNPRICED; 0 -> '$0.00'; < 10_000 micros -> four decimals ('$0.0042'); else two ('$1.23', '$1,204.50')
export function costNote(totals: UsageTotals): string | null;   // USAGE_UNPRICED_NOTE when unpricedCalls > 0
export function subscriptionNote(totals: UsageTotals): string | null;   // USAGE_SUBSCRIPTION_NOTE(n) when subscriptionCalls > 0
export function sourceLabel(source: string): string;     // 'chat' -> 'Chat', 'api' -> 'API', unknown -> as is
export function sessionUsageLine(totals: UsageTotals | null | undefined): string | null;   // '12.3k tokens · $0.04'; null when calls is 0 or absent; '12.3k tokens · —' when nothing was priced
export function usageErrorMessage(error: unknown): { message: string; status: number | null };   // DaemonHttpError: its message (404 -> USAGE_TOO_OLD); else COMPANION_UNREACHABLE
export function usageCsvFilename(range: { from: number; to: number }): string;   // 'anima-usage-YYYY-MM-DD-to-YYYY-MM-DD.csv' in local dates
export const USAGE_EMPTY = 'No model calls in this range yet.';
export const USAGE_TOO_OLD = 'Update the daemon to see usage.';
export const USAGE_UNPRICED_NOTE = 'Some calls have no known price, so their cost is not counted.';
export const USAGE_SUBSCRIPTION_NOTE = (count: number) => `${count} call${count === 1 ? '' : 's'} on your ChatGPT subscription have no per-token cost.`;
export const USAGE_EXPORT_FAILED = 'Couldn’t export the usage. Try again.';
export const USAGE_TRUNCATED_NOTE = 'This range is too large to total completely; choose a shorter one.';

// lib/download.ts
export function downloadText(filename: string, text: string, mime: string): void;   // Blob + temporary anchor, URL revoked after the click (the pattern at ConversationTools.tsx)

// hooks/useUsage.ts
export interface UsageOptions { enabled: boolean; days: UsageRangeDays; agentId: string | null; epoch: number; now?: () => Date }
export interface UsageView {
  range: { from: number; to: number; tzOffsetMinutes: number };
  days: UsageGroup[]; models: UsageGroup[]; sources: UsageGroup[]; sessions: UsageGroup[];
  totals: UsageTotals | null; today: UsageTotals | null;
  truncated: boolean; loaded: boolean; error: string | null; errorStatus: number | null;
  refresh: () => void;
  /** Reads the range's CSV and downloads it; true when it did. */
  exportCsv: () => Promise<boolean>;
}
export function useUsage(options: UsageOptions): UsageView;

// hooks/useSessionUsage.ts
export function useSessionUsage(target: { agentId: string; sessionId: string } | null, refreshKey: number): UsageTotals | null;   // reads daemon.getSession(...).usage; keeps the last value while reloading
```

**Hook rules:**

- `useUsage` reads four summaries together (`groupBy` day, model, source, session over the range, for `agentId`) with `Promise.allSettled` when `enabled && agentId`: on mount, when `days`, `agentId`, or `epoch` changes, and on `refresh`. `totals` is the day summary's totals; `today` is the last day's totals after `fillDays`. A read answered after a newer one started is ignored (a sequence counter in a ref), unmounting ignores late answers, each list keeps its previous value when its own read failed, and the first failure's message sets `error` (via `usageErrorMessage`). **Empty reload bail-out:** a list that was empty and is empty again keeps its previous array (the M5/M7 lesson). `now` is injectable so tests fix the clock. `exportCsv` fetches with `exportUsageCsv`, calls `downloadText(usageCsvFilename(range), text, 'text/csv')`, and on failure sets `error` to `USAGE_EXPORT_FAILED`.
- `useSessionUsage` reads once per `(target, refreshKey)`; a failed read keeps the last value and sets nothing visible (the header simply shows no line).

**Page behavior (spec §15.4):** `UsagePage({ agentId, online, epoch, sessionId })`.

- A range switch (`role="group"`, buttons "7 days", "30 days", "90 days", `aria-pressed`; the choice is remembered in `sessionStorage` under `anima.usage.range` inside try/catch). Header text "What your companion costs". While `!online`, show `COMPANION_UNREACHABLE`; while `!loaded`, "Loading usage…"; an error in a `role="alert"` with Refresh; a 404 shows `USAGE_TOO_OLD`.
- **Summary cards:** Total tokens (`formatTokens`) with prompt and completion in small text, Cost (`formatCost`, with `costNote` and `subscriptionNote` beneath when they apply), Calls, Today (tokens and cost), and **This chat** (the `sessionId` prop's totals through `useSessionUsage`, hidden when no session has been viewed).
- **Per-day bar chart** (`UsageChart`): inline SVG over `fillDays`, a bar per day by `totalTokens`, `role="img"` with an `aria-label` summarizing the range ("Tokens per day, 30 days, busiest day …"), a `<title>` per bar ("2026-09-23: 12.3k tokens"), colors from the existing palette variables, no library. A range with no calls shows `USAGE_EMPTY` instead of the chart. `truncated` shows `USAGE_TRUNCATED_NOTE`.
- **Tables** (`UsageTable`): by model (`provider/model`, calls, tokens, cost), by source (`sourceLabel`), and top sessions (session id, calls, tokens, cost; the id is a link to `#/s/<id>` when it is not empty). All strings render as text nodes (provider and model names come from config; session ids from the daemon). A cost cell is `formatCost(totals.costMicros)` only when `unpricedCalls` is 0 for that row, otherwise `formatCost(...)` followed by " + unpriced".
- **Export CSV** button (`useUsage.exportCsv`), disabled while exporting.
- **Session header:** `SessionView`'s header shows `sessionUsageLine` next to the model name (no element when null), refreshed when the session's finished-run count changes (`refreshKey`; find how `SessionView` derives its run state and reuse it).
- **`/usage`:** `SlashCommandName` gains `'usage'`; `SLASH_COMMANDS` gains `{ name: 'usage', description: 'Show usage for this chat and today' }` (placed before `help`); `useSessionCommands` handles it by opening `#/usage` through the same navigation callback `/model` uses. `ViewHarness` keeps the last viewed session id in a ref and passes it as `sessionId`.
- **Destination:** `AVAILABLE_PAGES` gains `'usage'`; `SYSTEM_DESTINATIONS` gains `{ page: 'usage', label: 'Usage', icon }` before Capabilities (use an existing icon from `components/icons.tsx`; add one only if none fits); `WorkspaceShell` takes `usage?: ReactNode | null` and renders it at `page === 'usage'`; `ViewHarness` passes `<UsagePage agentId={…} online={…} epoch={live.state.epoch} sessionId={…} />` (one element, one import, three lines for the ref).

**Tests:**

- `lib/usage.test.ts`: `builds a range of whole local days ending tomorrow` (fixed `Date`); `fills missing days with zero totals`; `formats tokens at the boundaries 999, 1000, 1234, 3.4M`; `formats cost: null as a dash, zero, sub-cent, cents, thousands`; `notes unpriced and subscription calls`; `labels sources`; `builds the session usage line`; `maps a daemon refusal and a network failure`; `names the csv file by local dates`; `owner-facing strings` (one assertion per exported constant).
- `lib/download.test.ts`: `downloads text through a temporary link and revokes the url`.
- `hooks/useUsage.test.tsx` (mock `daemon`): `reads the four summaries for the range`; `a range change reads again`; `a stale read is ignored`; `a failed list keeps the others and sets the error`; `an empty reload keeps the empty arrays` (referential equality); `a 404 sets the update-the-daemon text and the status`; `nothing is read while offline or without an agent`; `exportCsv downloads and reports a failure`; `today is the last day`.
- `hooks/useSessionUsage.test.tsx`: `reads the session's totals`; `reads again when the key changes and keeps the last value meanwhile`; `nothing is read without a target`; `a failed read keeps the last value`.
- `components/usage/UsageChart.test.tsx`: `draws a bar per day with a title and a label`; `scales bars to the busiest day`; `shows nothing for a range with no calls` (the page shows `USAGE_EMPTY`).
- `pages/UsagePage.test.tsx`: `shows the empty state`; `shows totals, cost, calls, today, and this chat`; `notes unpriced and subscription calls`; `switching the range reads again and remembers it`; `still renders when storage throws`; `renders the model, source, and session tables as text` (a model named `<img src=x onerror=alert(1)>` yields no `img` element); `session rows link to the session`; `exports the csv`; `offline shows the unreachable text and no lists`; `a 404 says to update the daemon`; `the truncated note shows`.
- `WorkspaceShell.test.tsx`: `the System group offers Usage before Capabilities`, `Usage opens the usage page and the command menu offers Go to Usage`. `SessionView.test.tsx`: `the header shows the session's usage line and hides it when there are no calls`, `the line refreshes when a run finishes`. `slash-commands.test.ts`: `/usage is offered before /help`. `useSessionCommands.test.tsx`: `/usage opens the usage page`. `ViewHarness.test.tsx`: `#/usage shows the Usage page for the companion` (and the existing harness tests stay quiet).
- `daemon-api.test.ts`: one test that each new method calls the matching SDK method with the arguments above.

**Steps:**

- [ ] **Step 1:** Write the tests; `cd apps/web && bun x vitest run src/lib/usage.test.ts src/lib/download.test.ts src/hooks/useUsage.test.tsx src/hooks/useSessionUsage.test.tsx src/components/usage src/pages/UsagePage.test.tsx src/components/WorkspaceShell.test.tsx src/components/sessions/SessionView.test.tsx src/lib/slash-commands.test.ts src/hooks/useSessionCommands.test.tsx src/ViewHarness.test.tsx src/lib/daemon-api.test.ts 2>&1 | tail -30`. Expected: FAIL.
- [ ] **Step 2:** Implement. Re-run the same, then `bun x vitest run src/visual-tokens.test.ts 2>&1 | tail -10` (find its path with `find src -name "visual-tokens.test.ts"`). Expected: PASS and silent.
- [ ] **Step 3:** `grep -rn "dangerouslySetInnerHTML\|MarkdownMessage\|innerHTML" apps/web/src/pages/UsagePage.tsx apps/web/src/components/usage` prints nothing; `git diff --stat -- apps/web/src/ViewHarness.tsx` shows a handful of added lines.
- [ ] **Step 4:** `bun x nx format:write --files=<each changed file>`; re-run the tests; `bun x nx typecheck @animaOS-SWARM/web 2>&1 | tail -10` (confirm the target name with `bun x nx show project @animaOS-SWARM/web`).
- [ ] **Step 5:** Stage by path; commit:

```bash
git commit -m "feat(web): add the Usage page, session usage, and /usage

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Web Logs and Health pages and the System group

**Model tier:** sonnet.

**Files:**

- Create: `apps/web/src/lib/logs.ts` (+ test), `lib/status.ts` (+ test), `hooks/useLogs.ts` (+ test), `hooks/useStatus.ts` (+ test), `pages/LogsPage.tsx` (+ test), `pages/HealthPage.tsx` (+ test), `components/system/LogLine.tsx`, `components/system/StatusCard.tsx`, `test/system.ts`
- Modify: `lib/daemon-api.ts` (+ test), `system.css` (Task 10 created it), `components/WorkspaceShell.tsx` (+ test), `ViewHarness.tsx` (wiring), `ViewHarness.test.tsx`

**Interfaces (exact):**

```ts
// lib/daemon-api.ts additions
logs: (query: LogsQuery) => setupClient.logs.list(query),
logStream: (options: { level?: LogLevel; q?: string; after?: number; signal?: AbortSignal }) => setupClient.logs.stream(options),
status: () => setupClient.status.get(),

// lib/logs.ts
export const MAX_LOGS_SHOWN = 2_000;
export const LOGS_FETCH_LIMIT = 500;
export const LOG_LEVEL_LABELS: Record<LogLevel, string> = { error: 'Error', warn: 'Warning', info: 'Info', debug: 'Debug', trace: 'Trace' };
export function mergeLines(existing: readonly LogLine[], incoming: readonly LogLine[], cap?: number): LogLine[];   // ascending by seq, duplicates dropped by seq, newest `cap` kept (default MAX_LOGS_SHOWN); returns `existing` itself when nothing changed
export function formatLogTime(ms: number): string;       // 'HH:MM:SS.mmm' local
export function logsAsText(lines: readonly LogLine[]): string;    // 'HH:MM:SS.mmm LEVEL target message', one per line
export function nextReconnectDelay(attempt: number, random: number): number;   // 1s doubling to 30s, plus up to 20% jitter from `random` in [0,1)
export const LOGS_EMPTY = 'No log lines match.';
export const LOGS_CONNECTING = 'Connecting…';
export const LOGS_RECONNECTING = 'Reconnecting…';
export const LOGS_PAUSED_NOTE = 'Paused. New lines are held and will appear when you resume.';
export const LOGS_COPIED = 'Copied';
export const LOGS_COPY_FAILED = 'Couldn’t copy. Select the lines and copy them instead.';
export const LOGS_TOO_OLD = 'Update the daemon to see its logs.';

// hooks/useLogs.ts
export interface LogsOptions { enabled: boolean; level: LogLevel | null; query: string; paused: boolean; reconnectDelay?: (attempt: number) => number }
export interface LogsView { lines: LogLine[]; held: number; connected: boolean; loaded: boolean; error: string | null; errorStatus: number | null; refresh: () => void }
export function useLogs(options: LogsOptions): LogsView;

// lib/status.ts
export const STATUS_POLL_MS = 15_000;
export const HEALTH_TOO_OLD = 'Update the daemon to see its health.';
export type CardState = 'ok' | 'warn' | 'bad';
export interface HealthCard { id: 'readiness' | 'storage' | 'providers' | 'connectors' | 'automations' | 'approvals' | 'runs' | 'daemon'; title: string; state: CardState; summary: string; details: string[]; link?: { label: string; hash: string } }
export function healthCards(status: DaemonStatus): HealthCard[];    // fixed order as listed
export function formatUptime(seconds: number): string;              // '45s', '12m', '3h 05m', '2d 4h'
export function approvalsSummary(pending: number): string;          // 'Nothing is waiting for you' | '1 request is waiting for you' | 'N requests are waiting for you'
// hooks/useStatus.ts
export function useStatus(options: { enabled: boolean; epoch: number }): { status: DaemonStatus | null; loaded: boolean; error: string | null; errorStatus: number | null; refresh: () => void };
```

**Hook rules:**

- `useLogs`: on mount and on a `level` or `query` change, read `daemon.logs({ level, q, limit: LOGS_FETCH_LIMIT })`, set `lines`, then open `daemon.logStream({ level, q, after: newestSeq, signal })`. Stream `line` events merge through `mergeLines`; a `resync` event refetches the list `after` the newest seq held and merges. A closed or failed stream shows `connected: false`, waits `reconnectDelay(attempt)` (default `nextReconnectDelay(attempt, Math.random())`), and reopens with `after` the newest seq; the attempt counter resets after a line arrives. **Paused:** lines keep arriving into a ref (capped like `lines`), `held` counts those not yet shown, and `lines` stops changing; resuming merges them. A newer filter aborts the older stream and ignores its late answers (an abort controller per run plus a sequence counter); unmount aborts. A 404 sets `error = LOGS_TOO_OLD`. The empty-reload bail-out applies (an empty list stays the same array).
- `useStatus`: reads `daemon.status()` on mount, on `epoch` change, on `refresh`, and every `STATUS_POLL_MS` while `enabled` (a plain `setInterval` cleared on unmount); a stale answer is ignored; a failed read keeps the last status and sets `error`; a 404 (`StatusTooOldError`) sets `error = HEALTH_TOO_OLD` and stops polling.

**Page behavior (spec §15.4):**

- **Logs** (`LogsPage({ online })`): header "What the daemon is saying"; a level `select` labelled "Show at least" (All, then the five levels, default All), a search input labelled "Search logs" (submitted with Enter or the button, like Memory), a **Pause** / **Resume** toggle (`aria-pressed`) with the `held` count ("12 new lines held"), **Copy** (copies `logsAsText` of the visible lines with `navigator.clipboard.writeText` inside try/catch; shows `LOGS_COPIED` or `LOGS_COPY_FAILED` in a `role="status"` span for the click, no timers: the message stays until the next click or filter change), and a "Connecting…"/"Reconnecting…" note. Lines render in a scrollable `role="log"` region (`aria-live="off"`), oldest to newest, each as a monospace row: time, level badge, target, message, **every field as a text node through `RevealedText`** (a line can carry model or provider text; hidden characters show as markers). The view sticks to the bottom while the user has not scrolled up and not paused. `LOGS_EMPTY` for no lines. Offline: `COMPANION_UNREACHABLE` and no stream. A 404 shows `LOGS_TOO_OLD`.
- **Health** (`HealthPage({ online, epoch })`): one `StatusCard` per `healthCards(status)` in a responsive grid (single column under 640px): Readiness (state `bad` with the issues listed when `not_ready`), Storage and history store (store label, pending flush count, a `bad` state with the redacted last error through `RevealedText` when the history store is failing), Providers (configured count out of known, the configured names), Connectors (each with its status; `warn` when any is not running), Automations (enabled count; `warn` with "N failing" when any has consecutive failures, link to `#/automations`), **Pending approvals** (`approvalsSummary`; `warn` when above zero; a link "Review approvals" to `#/approvals`; this is the card deferred from M4), Runs (running, queued, and the by-status counts), Daemon (version, build revision when present, uptime via `formatUptime`, event subscribers). A Refresh button; while loading "Loading health…"; offline shows `COMPANION_UNREACHABLE`; a 404 shows `HEALTH_TOO_OLD`. State is shown with text and a shape, not color alone.
- **System group:** `AVAILABLE_PAGES` gains `'logs'` and `'health'`; `SYSTEM_DESTINATIONS` becomes Usage, Logs, Health, Capabilities (spec §15.1); `WorkspaceShell` takes `logs?` and `health?` ReactNodes; `ViewHarness` passes `<LogsPage online={…} />` and `<HealthPage online={…} epoch={live.state.epoch} />` (two elements, two imports).
- Styles in `system.css` with `.system-*` classes only; reuse the `studio-*` buttons and panel classes; mobile friendly.

**Tests:**

- `lib/logs.test.ts`: `merges ascending, drops duplicate seqs, and keeps the newest cap`; `returns the same array when nothing changed`; `formats the time`; `copies lines as text`; `backs off from one second to thirty with jitter` (deterministic `random`); `owner-facing strings`.
- `lib/status.test.ts`: `builds the cards in order`; `readiness is bad with issues listed`; `a failing history store is bad and shows the redacted error`; `connectors warn when one is not running`; `automations warn on failing ones and link to Automations`; `pending approvals warn and link to Approvals`; `formats uptime`; `approvals summary wording`; `owner-facing strings`.
- `hooks/useLogs.test.tsx` (mock `daemon.logs` and `daemon.logStream` with controllable async generators): `loads the recent lines and then streams new ones after the newest seq`; `a duplicate line from the stream is dropped`; `a resync refetches after the newest seq`; `a closed stream reconnects after the delay and resumes after the newest seq` (inject `reconnectDelay` returning 0 and drive it with the generator, no timers); `pausing holds new lines and resuming merges them`; `changing the filter aborts the old stream and ignores its late lines`; `unmounting aborts the stream`; `a 404 sets the update text`; `an empty reload keeps the empty array`.
- `hooks/useStatus.test.tsx` (fake timers): `reads on mount and on epoch change`; `polls every 15 seconds and stops on unmount`; `a stale answer is ignored`; `a failed read keeps the last status and sets the error`; `a 404 sets the update text and stops polling`.
- `pages/LogsPage.test.tsx`: `lists lines with time, level, target, and message`; `filters by level and submitted search (the hook is called with them)`; `pause and resume with the held count`; `copy reports success and failure`; `shows hidden characters as markers` (a ZWSP in a message and a target); `renders log text as text, never markup`; `shows connecting and reconnecting notes`; `offline shows the unreachable text and no stream`; `a 404 says to update the daemon`; `shows the empty state`.
- `pages/HealthPage.test.tsx`: `shows a card for each area`; `a not-ready daemon lists its issues`; `a failing history store shows its redacted error as text`; `pending approvals link to the Approvals page`; `failing automations link to Automations`; `shows version, revision, and uptime`; `refresh reads again`; `offline and too-old states`.
- `WorkspaceShell.test.tsx`: `the System group lists Usage, Logs, Health, Capabilities in order`; `Logs and Health open their pages and the command menu offers them`. `ViewHarness.test.tsx`: `#/logs and #/health show their pages`.
- `daemon-api.test.ts`: the three new methods call the matching SDK methods.

**Steps:**

- [ ] **Step 1:** Write the tests; `cd apps/web && bun x vitest run src/lib/logs.test.ts src/lib/status.test.ts src/hooks/useLogs.test.tsx src/hooks/useStatus.test.tsx src/pages/LogsPage.test.tsx src/pages/HealthPage.test.tsx src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx src/lib/daemon-api.test.ts 2>&1 | tail -30`. Expected: FAIL.
- [ ] **Step 2:** Implement. Re-run the same, then `bun x vitest run src/visual-tokens.test.ts 2>&1 | tail -10`, then the full web suite once: `bun x vitest run 2>&1 | tail -15`. Expected: PASS and silent.
- [ ] **Step 3:** `grep -rn "dangerouslySetInnerHTML\|MarkdownMessage\|innerHTML" apps/web/src/pages/LogsPage.tsx apps/web/src/pages/HealthPage.tsx apps/web/src/components/system` prints nothing; `grep -L "RevealedText" apps/web/src/components/system/LogLine.tsx` prints nothing (the log row uses it).
- [ ] **Step 4:** `bun x nx format:write --files=<each changed file>`; re-run the tests; `bun x nx typecheck @animaOS-SWARM/web 2>&1 | tail -10`.
- [ ] **Step 5:** Stage by path; commit:

```bash
git commit -m "feat(web): add the Logs and Health pages and the System group

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 12: M8 verification

**Controller only.**

**Files:**

- Modify: `docs/superpowers/plans/2026-09-23-companion-console.md` (the M8 status row and task lines)

- [ ] **Step 1: Check the contracts**

Run: `grep -rn "const LOG_BUFFER_LINES\|const LOG_LINE_MAX_BYTES\|const LOG_REDACTED\|const LOG_TRUNCATED_SUFFIX\|const MAX_SUMMARY_ROWS\|const MAX_CSV_ROWS\|const USAGE_QUEUE_MAX\|const MAX_PRICING_OVERRIDES\|const DEFAULT_LOGS_LIMIT\|const MAX_LOGS_LIMIT\|const MAX_LOG_STREAMS" hosts/rust-daemon/src`
Expected: each constant defined once (`logs.rs`, `usage/mod.rs`, `usage/pricing.rs`, `routes/logs.rs`).

Run: `grep -n '"/api/usage/summary"\|"/api/usage/records"\|"/api/usage/export.csv"\|"/api/usage/pricing"\|"/api/logs"\|"/api/logs/stream"\|"/api/status"' hosts/rust-daemon/src/routes/mod.rs`
Expected: each path in the router, and in `ApiDoc` through the handlers' `#[utoipa::path]`.

Run: `grep -n "Usage, logs, and health" hosts/rust-daemon/README.md`
Expected: the new section.

Run: `grep -n "CONTROL_PLANE_STORE_VERSION: u32 = 10\|AUTOMATIONS_STORE_VERSION: u32 = 9\|PRE_USAGE_BACKUP_SUFFIX" hosts/rust-daemon/src/control_plane_store.rs`
Expected: all three. Then `git diff 248c9b6 --stat -- hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/app/persistence.rs` shows the three Task 3 files.

Run: `grep -rn "allow(dead_code)\|allow(unused_imports)" hosts/rust-daemon/src/usage hosts/rust-daemon/src/logs.rs hosts/rust-daemon/src/logs hosts/rust-daemon/src/routes/usage.rs hosts/rust-daemon/src/routes/logs.rs hosts/rust-daemon/src/routes/status.rs hosts/rust-daemon/src/live/fanout.rs`
Expected: no output (the two fanout allowances for M8 are gone).

Run: `git diff 248c9b6 --stat -- hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/connectors/runtime.rs`
Expected: no output. `git diff 248c9b6 --stat -- apps/web/src/ViewHarness.tsx` shows a handful of added lines.

Run: `git diff 248c9b6 --stat -- Cargo.lock hosts/rust-daemon/Cargo.toml packages/core-rust/crates/anima-core/Cargo.toml packages/core-rust/crates/anima-model-adapters/Cargo.toml packages/sdk/package.json apps/web/package.json bun.lock`
Expected: no output (no new dependencies or features).

Run: `grep -rn "dangerouslySetInnerHTML\|MarkdownMessage\|innerHTML" apps/web/src/pages/UsagePage.tsx apps/web/src/pages/LogsPage.tsx apps/web/src/pages/HealthPage.tsx apps/web/src/components/usage apps/web/src/components/system`
Expected: no output (logs and names render as text).

- [ ] **Step 2: Audit for secrets in logs**

Run: `grep -rnE "(info|warn|error|debug|trace)!\(" hosts/rust-daemon/src --include=*.rs | grep -iE "token|secret|api_key|apikey|password|authorization|bearer|credential|cookie" | grep -v "tests\|_tests\|mod tests"`
Expected: read every hit. Each may name a credential (a message such as "token rejected") but none may print its value (`%token`, `?headers`, `{:?}` of a request, a full URL with a query, or a provider body that the adapters have not sanitized). Fix any that do, and add a redaction test for the shape.

Run: `grep -rn "tracing_subscriber" hosts/rust-daemon/src | grep -v "^hosts/rust-daemon/src/logs.rs"`
Expected: only `history/worker.rs`'s test subscriber.

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- logs::redact 2>&1 | tail -15`
Expected: the redaction table passes.

- [ ] **Step 3: Run the milestone gate**

Run: `df -h .`

- With at least 12 GB available: `bun x nx run rust-daemon:test --skipNxCache` (it also runs `core-rust:test`). Expected: PASS (M7 ended at 1,925 passed; M8 adds about 130 Rust tests).
- Otherwise run the fallback in the shared `target/` (no new `CARGO_TARGET_DIR`): `CARGO_INCREMENTAL=0 cargo test -p anima-core`, `CARGO_INCREMENTAL=0 cargo test -p anima-model-adapters`, `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib`, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --tests`. Expected: PASS. The fallback does not satisfy AGENTS.md's completion rule; record that the Nx gate is pending disk space. On Windows, if a running daemon locks `target/debug/anima-daemon.exe`, use AGENTS.md's `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon` rerun only with the owner's go-ahead.

Run: `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`
Expected: every target succeeds (M7 ended with the web at about 990 tests; M8 adds about 110 web and 25 SDK tests).

Run: `cargo fmt --all --check && bun x nx format:check --base=origin/main`
Expected: both succeed.

- [ ] **Step 4: Manual acceptance (not claimed from automated tests)**

Record these as pending for the owner, with a real provider key: stop a streaming Anthropic or Google reply and confirm the Usage page shows a row with the prompt tokens (Task 1 covers only providers that report usage before the end); one xAI call (running usage) and one OpenAI call (`stream_options.include_usage`) priced from the table; a title and a compaction row appearing as sources `title` and `compaction`; with a real Telegram connector, read the Logs page and confirm no bot token or API key appears; the Postgres history store's `upsert_usage` and `page_usage` SQL hand-checked against the SQLite twin (no database here).

- [ ] **Step 5: Update the master plan status**

In `docs/superpowers/plans/2026-09-23-companion-console.md`, replace the M8 row (match it by content; the table is padded)

```markdown
| M8 Usage, logs, health | (written before M8) | pending |
```

with the following only if every gate command passed (fill in the Nx test count and the head commit):

```markdown
| M8 Usage, logs, health | `2026-09-23-companion-console-m8.md` | done (Nx rust-daemon:test <count> passed; sdk + web test, typecheck, build green at <sha>) |
```

If the Rust gate ran only through the fallback, use `implemented — Nx gate pending (disk)`. Also adjust the master plan's M8 task lines to what shipped: T8.1 added `usage/{mod,pricing,summary,metered}.rs` and the control plane moved to version 10 for pricing overrides; the three carried M3 items are done (Tasks 1, 4, 8); T8.2's `init_tracing` moved from `main.rs` into `logs.rs`; T8.3's aggregate is `routes/status.rs` and `handle_metrics` reads it. Then run `bun x nx format:write --files=docs/superpowers/plans/2026-09-23-companion-console.md` (it realigns the table). The controller commits this file:

```bash
git add docs/superpowers/plans/2026-09-23-companion-console.md
git commit -m "docs: mark the M8 usage, logs, and health milestone complete"
```

Recommended implementer tier: the controller runs this task.

---

## Notes for the controller

**Task shape against the master plan.** T8.1 → Tasks 2 to 5 (types, stores, and pricing; the control plane; recording; routes), with the three carried M3 usage items in Tasks 1 (partial usage), 4 (compaction and title rows, rollback keeps steps), and 8 (steers count toward the queue cap). T8.2 → Tasks 6 and 7. T8.3 → Task 8. T8.4 → Tasks 9 to 11. Task 12 is the gate. Order is strictly 1 → 12. Tasks 9 to 11 need only Task 9's SDK, but stay sequential for the SDK build.

**Spec vs. code decisions.**

- **Snapshot version 10.** Pricing overrides are control-plane state (spec §11.1); the history store holds usage rows, and the existing `history_usage` / `usage` tables already fit, so no migration. Everything else (log buffer, usage queue, metrics counters) is in memory.
- **Run usage is derived from the terminal `RunRecord`.** The ledger already keeps `steps` and `usage` per run through `commit_run`; deriving rows in the outbox's `write_runs` makes them as durable as the run itself and idempotent by step id. Cost is priced when the run is mirrored (usually within a second), with the overrides then in force. Secondary calls (titles, compaction, profile, agency) use a `Metered` adapter wrapper and a small in-memory queue, because they have no ledger record.
- **Summaries are computed in Rust over `page_usage`.** The stores need only two usage methods (`upsert_usage`, `page_usage`), which keeps the Postgres twin small and the three stores consistent. A summary scans at most 200,000 rows and says `truncated` past that.
- **Day buckets follow the caller's offset** (`tzOffsetMinutes`), the browser's current offset; a range that crosses a DST change buckets all days with that one offset.
- **`/usage` opens the Usage page** (which shows This chat and Today) instead of printing into the transcript; the page needs no new transcript message kind.
- **`/metrics` stays unauthenticated** and carries counts only; the richer status (providers, connectors, redacted errors) is behind owner read authorization.
- **Steers count toward the 8-queue cap** (carry-over): a steer is refused with 429 `QUEUE_FULL` when eight messages already wait, instead of joining without limit. The web already maps that error for sends.

**Deferred.** Budgets and alerts; usage for tool calls; a `usage.updated` or `status.changed` event; persisting the log buffer; log levels adjustable at runtime; editing or deleting usage rows; per-day timezone handling across DST; a provider invoice reconciliation; the Playwright Usage, Logs, and Health flows (M10, T10.2); the README route table regeneration (M10, T10.1).

## Risks (top 5 for the controller to rule on)

1. **Stopped-call usage is only as good as the provider's early usage.** Task 1 records usage a provider has already reported; Anthropic (input tokens at `message_start`) and Google (per chunk) report early, but OpenAI-compatible, Ollama, and ChatGPT streams report usage only in their last chunk, so a stop before it still records nothing. The alternative is to estimate tokens locally, which would put invented numbers in a cost report. Recommendation: accept the gap, label nothing, and note it on the Usage page's help text only if the owner asks.
2. **Usage durability has three soft spots.** Secondary-call rows sit in an in-memory queue for about a second (lost on a crash in that window); runs of an agent deleted before their mirror leave no usage rows (the ledger drops them); a run past 50 steps keeps one `:rest` row instead of per-step rows. The alternative is to persist the queue in the control plane, which grows the snapshot and the save time for little. Recommendation: accept as planned.
3. **Pricing semantics and rollback.** Rows are priced when first mirrored with the overrides then in force, and a re-mirror of a changed run record re-prices that run; the control plane is now version 10, so an older daemon refuses the file and rolling back needs the `.pre-usage.bak` backup. The alternative (storing the price at step time, or the overrides in a separate file that old daemons ignore) avoids both but needs the live observer to read control-plane state. Recommendation: accept, and keep the README rollback note prominent.
4. **Redaction is pattern-based.** It covers the shapes the daemon handles and an unlabeled-long-token heuristic (mixed-case, 32+ characters), so it can miss a secret in an unfamiliar format and can hide an innocent long identifier; `RUST_LOG=debug` raises what reaches the buffer (still redacted). The owner-only routes and the audit grep in Task 12 are the other layers. Rule whether that is enough or whether the buffer should keep only `info` and above regardless of `RUST_LOG`.
5. **Two behavior changes are visible to the owner.** A steer at eight waiting messages is now refused with 429 where before it always joined (consistent with the cap, but a change), and each Usage page load runs four summaries (each may scan up to 200,000 rows in Rust) while each `GET session` runs one small scan. Both are fine for a personal companion; SQL aggregation per store is the escape hatch if the scans ever hurt, at the price of untested Postgres SQL.

## Controller rulings on the risks (binding)

1. **Usage for stopped calls:** only providers that report usage before the last chunk keep it. There is no token estimation for the others. The README's usage section says so in one line.
2. **Usage durability gaps:** accepted and documented.
   - Secondary-call rows sit in memory for about a second.
   - Runs of an agent deleted before their mirror leave no rows.
   - A run past 50 steps keeps one `:rest` row.
   - The in-memory queue must be bounded, with an oldest-first drop and a warning.
3. **Pricing:** a run row is priced when first mirrored. Re-mirroring the same id must not change the stored cost; the upsert keeps the existing price fields. Test it. Rolling back to a pre-M8 daemon needs `.pre-usage.bak`, and the README says so.
4. **Redaction:**
   - Pattern-based redaction is accepted.
   - It runs before truncation and never logs the original.
   - Task 6 tests the common secret shapes: bearer tokens, `sk-…`, `xoxb-…`, `ghp_…`, `Authorization` headers, API-key query parameters and JSON `apiKey`/`token`/`secret` fields.
   - Task 12's grep audit of log call sites is required.
   - The README warns that `RUST_LOG=debug` can surface more detail in the logs.
5. **Visible changes:**
   - A steer at eight waiting messages now gets 429. This is the M3 carry-over's intent; the web shows the daemon's message.
   - The four summaries per Usage page load are accepted for M8; a single combined endpoint is a follow-up.
   - `/metrics` stays unauthenticated with counts only: no agent names, ids, prompts or costs. Test it.
