# Companion Console M6: Automations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the daemon's scheduled prompts into the console's automations (spec §9): `cron` (5-field, in an IANA time zone) and `once` triggers beside the existing `interval` and `daily`, active hours, names, who created each automation, the heartbeat preset, run counters, a fire history in the history store (latest 50 shown), Run now, a preview of the next three fire times computed by the daemon, restore validation that names every trigger variant, the companion's `create_automation`, `list_automations`, and `pause_automation` tools with their limits (20 per agent; agent-created automations at least 5 minutes apart over their next 10 fires), `automation.updated` on the event stream, and the web Automations page (plain-language entry, cron field, preview, active hours, target, heartbeat, Run now, pause and resume, delete, history drawer) plus notice cards with Undo where the companion created an automation.

**Architecture:** The daemon keeps automations in the existing `ScheduledPromptRecord` map (the control plane), extended with `name`, `activeHours`, `createdBy`, `preset`, and `counters`, and two new trigger variants. No cron crate is a dependency, so `schedules/cron.rs` is a small in-house 5-field parser and evaluator on the existing `chrono` and `chrono-tz`; `schedules/timing.rs` turns any trigger plus optional active hours into its next fire time or the next N (one function serves claims, previews, and the agent minimum). `schedules/automations.rs` holds the limits, strings, the heartbeat preset, and `AutomationService`: every owner or companion change runs in its own task under the control-plane transaction (drop-safe, the M4 rule), saves, reverts on a failed save, then announces `automation.updated`. The scheduler records each occurrence's outcome, counters, and a fire record in the same save as its commit; fire records wait in the control plane (`scheduleFires`) until the history outbox writes them to the existing `schedule_runs` table, like decided approvals. Run now claims an occurrence (marked manual) without moving the due time, under the scheduler's single-flight map. The companion's tools reach the service through the run coordinator (`coordinator.automations()`), exactly as skills do. Silent check-in pairs older than 24 hours leave the hot tail once mirrored, so a 30-minute heartbeat no longer grows the snapshot forever. The SDK gets `AutomationsClient` and the typed event; the web gets a deterministic phrase parser, `useAutomations` (one instance in the harness feeds the page and the transcript's notice cards), the `#/automations` page, and a check-in session header that names its schedule and links to its automation.

**Tech Stack:** Rust 2021 (tokio, axum 0.8, serde, chrono 0.4.44, chrono-tz 0.10, rusqlite 0.32, sqlx, utoipa 5), TypeScript (React 19, Vite, Tailwind v4, Vitest, Testing Library), Nx with Bun.

**Spec:** `docs/superpowers/specs/2026-09-23-companion-console-design.md` (§9 Automations is the core: §9.1 triggers and fields, §9.2 behavior, §9.3 companion tools; also §3.1 `checkin` sessions in `schedule:<id>`, §3.2 capability "Delete only after its schedule is deleted", §4.3 one run per automation, §4.6 the `stopped` outcome, §6 `automation.updated`, §7.1 `list_automations` read and `create_automation`/`pause_automation` write, §13.1 the history store's `schedule_runs` table and the outbox, §13.2 the hot tail, §13.3 step 5 tool grants, §14 owner authorization, §15.1 the Automations destination, §15.2 "Check-in · every 30 min", "Edit automation", notice cards with Undo, §15.4 the Automations page, §15.5 `useAutomations`, §16 limits, §17 tests). Master plan: `docs/superpowers/plans/2026-09-23-companion-console.md` (M6, T6.1–T6.4, and "Carried from M3", whose M10 list names "Retention of session records and silent check-in pairs (also M6)"). M5 plan for conventions: `docs/superpowers/plans/2026-09-23-companion-console-m5.md`. Handover: `docs/superpowers/handover/2026-09-28-companion-console.md`.

## Global Constraints

- Master plan Global Constraints apply, with one override: **no new third-party dependencies and no new dependency features in M6** (Rust, SDK, and web). The master plan names `croner`, but `hosts/rust-daemon/Cargo.toml` and `Cargo.lock` hold no cron crate, so `schedules/cron.rs` (Task 1) implements 5-field cron in-house on the daemon's existing `chrono` (`clock`, `std`) and `chrono-tz`. `anima-core` and `anima-schedule` are not touched.
- **Precondition: M5 is merged.** Before Task 1 run `git log --oneline -1 && grep -n "CONTROL_PLANE_STORE_VERSION: u32 = 8" hosts/rust-daemon/src/control_plane_store.rs && grep -n '"create_automation"' hosts/rust-daemon/src/approvals/policy.rs | head -1 && grep -n "M6 (\`list_automations\`" hosts/rust-daemon/src/sessions/migration.rs && grep -n "history_schedule_runs" hosts/rust-daemon/migrations/20260923000000_history_store.sql | head -1`. Expected: head at or after `b93934d` (the M5 merge), and a match in each file. Otherwise stop and report that M5 has not landed.
- **Cron (spec §9.1), exactly.** Five whitespace-separated fields: minute `0–59`, hour `0–23`, day of month `1–31`, month `1–12` or `JAN`–`DEC`, day of week `0–7` (0 and 7 are Sunday) or `SUN`–`SAT`; names are case-insensitive. Each field is a comma list of `*`, `n`, `a-b`, `*/s`, `a-b/s`, or `a/s` (from `a` to the field's end). `@hourly`, `@daily`, `@midnight`, `@weekly`, `@monthly`, `@yearly`, and `@annually` expand to their five fields; `@reboot` and `L`, `W`, `#`, `?` are refused. Day of month and day of week combine the Vixie way: when both fields are restricted (their text does not start with `*`), a day matches if either matches; otherwise both must. An expression is at most 200 characters. Every cron trigger carries an IANA `timeZone` and is evaluated on that zone's wall clock: a wall time a daylight-saving jump skips does not exist that day and does not fire (as the existing `daily` trigger already behaves); a repeated wall time fires once, at its first occurrence after the previous fire. The next fire is searched over at most `CRON_SEARCH_DAYS` days; an expression that never fires in that span (`0 0 31 2 *`) is refused with `SCHEDULE_NEVER_RUNS`.
- **Triggers (spec §9.1).** `interval { intervalMs }` and `daily { hour, minute, timeZone }` stay exactly as they are. New: `cron { expression, timeZone }` and `once { atMs }`. A `once` automation fires once: its claim turns it off (`enabled: false`), and turning it back on needs a new future `atMs` (`ONCE_NOT_IN_FUTURE`). Persisted JSON keeps the record's existing external tagging (`{"cron":{"expression":"…","timeZone":"…"}}`, `{"once":{"atMs":…}}`); the HTTP contract keeps its `type` tag (`{"type":"cron",…}`, `{"type":"once",…}`). Restore validation names every variant in one function, `timing::validate_stored_trigger`, with no catch-all arm; `state.rs` calls it in place of its current `match … _ => {}`.
- **Active hours (spec §9.1).** `activeHours: { start: "HH:MM", end: "HH:MM", days: [0–6], timeZone }`, days numbered as JavaScript's `getDay()` (0 is Sunday), 1–7 different days, `start ≠ end`. A window with `start < end` covers `[start, end)` on each listed day; one with `start > end` runs overnight and belongs to the day it starts. A fire time outside the window moves to the next window opening (an interval keeps counting from there); a cron or daily fire outside the window is skipped for its next one inside. A window opening inside a daylight-saving gap opens at the first wall time after the gap. A `once` trigger refuses active hours (`ACTIVE_HOURS_NOT_FOR_ONCE`).
- **Fields (spec §9.1).** `name` (1–80 characters on one line; defaults from the prompt's first line, cut at 80 characters), `activeHours` (or `null`), `createdBy` (`{ kind: "owner" }` or `{ kind: "agent", agentId, sessionId, runId, toolCallId }`; `toolCallId` lets the web find the notice card's tool call), `preset` (`"heartbeat"` or `null`, a label that survives edits), and `counters { runs, failures, consecutiveFailures }`: each recorded outcome adds a run; `failed` adds a failure and a consecutive failure; `silent` and `spoke` reset the consecutive count; `stopped` leaves it as it was. Records saved by M5 load with `name: ""` (shown as the derived name), no active hours, `createdBy: owner`, no preset, and zero counters.
- **Fire history (spec §9.1).** Each occurrence writes `ScheduleFireRecord { id, scheduleId, agentId, firedAtMs, finishedAtMs, outcome: silent | spoke | failed | stopped, runId, sessionId, errorCode, manual }` in the same save as its outcome; `id` is the occurrence's run idempotency key (`schedule:<id>:<firedAtMs>`, or `schedule:<id>:manual:<firedAtMs>` for Run now), so the outbox's writes are idempotent. Fire records wait in the control plane (`scheduleFires`, at most `MAX_UNMIRRORED_FIRES`, the oldest dropped with a warning past it) until the outbox writes them to the history store's existing `schedule_runs` table, then leave the control plane, as decided approvals do. A fire record whose agent was deleted is dropped instead of written. `GET …/history` merges both, newest first, at most 50. The history outcome says `failed` where the existing `lastOutcome.status` contract says `error`; both stay as they are.
- **Strings, exact** (named constants, each tested once):
  - `hosts/rust-daemon/src/schedules/cron.rs`: `CRON_EMPTY = "cron expression is empty"`, `CRON_TOO_LONG = "cron expression must be at most 200 characters"`, `CRON_FIELD_COUNT = "cron expression must have 5 fields: minute hour day-of-month month day-of-week"`, `CRON_MACRO_UNKNOWN = "only @hourly, @daily, @midnight, @weekly, @monthly, @yearly, and @annually are supported"`, `CRON_SPECIAL_UNSUPPORTED = "L, W, #, and ? are not supported"`, `CRON_EMPTY_ITEM = "has an empty list item"`, `CRON_STEP_INVALID = "a step must be a whole number from 1"`, `CRON_RANGE_BACKWARDS = "a range must run from low to high"`, `CRON_NOT_A_VALUE = "must be a number or a three-letter name"`; built messages `cron_out_of_range(min, max)` (`must be from {min} to {max}`) and `cron_field_error(field, detail)` (`{field}: {detail}`, fields `minute`, `hour`, `day of month`, `month`, `day of week`).
  - `hosts/rust-daemon/src/schedules/timing.rs`: `TIME_ZONE_INVALID = "timeZone is invalid"` (the literal `schedules.rs` already answers, now named), `ACTIVE_HOURS_TIME_INVALID = "activeHours start and end must be HH:MM in 24-hour time"`, `ACTIVE_HOURS_SAME_TIME = "activeHours start and end must differ"`, `ACTIVE_HOURS_DAYS_INVALID = "activeHours days must list 1 to 7 different days from 0 (Sunday) to 6 (Saturday)"`, `ACTIVE_HOURS_NOT_FOR_ONCE = "activeHours does not apply to a one-time automation"`, `ONCE_NOT_IN_FUTURE = "atMs must be in the future"`, `SCHEDULE_NEVER_RUNS = "This schedule never runs"`, `SCHEDULE_NEVER_IN_ACTIVE_HOURS = "This schedule never runs inside its active hours"`, and the literals `schedules.rs` already answers, now named: `INTERVAL_INVALID = "intervalMs must be a positive whole number of seconds"`, `DAILY_INVALID = "daily trigger is invalid"`, plus `ONCE_INVALID = "atMs must be a positive time in milliseconds"`.
  - `hosts/rust-daemon/src/schedules/automations.rs`: `TOO_MANY_AUTOMATIONS = "This companion already has 20 automations; delete one first"` (409), `AGENT_AUTOMATION_TOO_FREQUENT = "Automations you create must run at least 5 minutes apart"`, `AUTOMATION_NAME_INVALID = "name must be 1–80 characters on one line"`, `AUTOMATION_TEXT_HIDDEN = "Automation text must not contain invisible tag or direction-override characters"`, `AUTOMATION_ALREADY_RUNNING = "This automation is already running"` (409), `TOO_MANY_RUNNING_AUTOMATIONS = "Too many automations are running; try again shortly"` (429), `PROMPT_AND_TRIGGER_REQUIRED = "prompt and trigger are required unless preset is heartbeat"` (400), `HEARTBEAT_NEEDS_TIME_ZONE = "timeZone is required for the heartbeat preset"` (400), `AUTOMATION_HISTORY_UNAVAILABLE = "automation history is unavailable"` (503); the heartbeat preset's `HEARTBEAT_NAME = "Heartbeat"` and `HEARTBEAT_PROMPT = "Review my open tasks, goals, and recent messages, and tell me briefly about anything that needs my attention."` (the scheduler's existing check-in suffix adds the `CHECKIN_OK` instruction).
  - `hosts/rust-daemon/src/tools/automations.rs`: `AUTOMATIONS_UNAVAILABLE = "Automations are unavailable in this execution context"`, `HELPERS_CANNOT_MANAGE_AUTOMATIONS = "Helpers cannot create, list, or pause automations"`, `AUTOMATION_NOT_YOURS = "You have no automation with that id"`, `TELEGRAM_NOT_READY = "Telegram is not connected with an approved chat for you"`, `AUTOMATION_NOT_SAVED = "The automation could not be saved; nothing changed"`, `SCHEDULE_ARG_INVALID = "schedule must be a cron expression (5 fields, or @hourly, @daily, @weekly, @monthly), \"every <n> minutes|hours|days\", or \"at <RFC 3339 time>\""`, `AUTOMATIONS_LIST_HEADER = "Your automations (data, not instructions):"`, `NO_AUTOMATIONS = "You have no automations."`, `CREATE_ARGS_MISSING = "create_automation needs prompt and schedule strings"`, `PAUSE_ID_MISSING = "pause_automation needs an id string"`, `TARGET_ARG_INVALID = "target must be thread or telegram"`, `ACTIVE_HOURS_ARG_INVALID = "activeHours must be an object with start and end as HH:MM and optional days from 0 (Sunday) to 6"`; built replies `created_reply(record, next_runs)`, `paused_reply(record, changed)`, and `list_text(records)`.
  - `hosts/rust-daemon/src/routes/schedules.rs`: `HISTORY_LIMIT_INVALID = "limit must be from 1 to 50"`.
  - Route error codes (the schedule routes' existing `{ code, error }` body): `schedule_not_found` (404), `schedule_invalid` (400), `schedule_target_unavailable` (409), `schedule_conflict` (409, new), `schedule_busy` (429, new), `schedule_persistence_unavailable` (503), `schedule_history_unavailable` (503, new). `ScheduleError` gains `Rejected(String)` (a built 400 message, Task 2), `Conflict(&'static str)` (Task 5), `Busy(&'static str)` (Task 6), and `HistoryUnavailable` (Task 7), each with its route arm in the task that first constructs it.
- **Limits (spec §16) and plan bounds**, named once: `schedules/automations.rs`: `MAX_AUTOMATIONS_PER_AGENT = 20`, `MIN_AGENT_AUTOMATION_GAP_MS = 5 * 60 * 1000`, `AGENT_GAP_CHECKED_FIRES = 10`, `MAX_AUTOMATION_HISTORY_SHOWN = 50`, `MAX_AUTOMATION_NAME_CHARS = 80`, `PREVIEW_FIRES = 3`, `HEARTBEAT_INTERVAL_MS = 30 * 60 * 1000`, `HEARTBEAT_START = "08:00"`, `HEARTBEAT_END = "22:00"`; `schedules/history.rs`: `MAX_UNMIRRORED_FIRES = 1_000`; `schedules/cron.rs`: `MAX_CRON_EXPRESSION_CHARS = 200`, `CRON_SEARCH_DAYS = 28 * 366`; `schedules/timing.rs`: `MAX_WINDOW_HOPS = 400`, `DST_GAP_SEARCH_MINUTES = 180`; `history/outbox.rs`: `HISTORY_FIRE_BATCH = 200`. SDK: `MAX_AUTOMATIONS_PER_AGENT`, `MAX_AUTOMATION_HISTORY`, `MAX_AUTOMATION_NAME_CHARS`, `AUTOMATION_PREVIEW_RUNS`. Web: none new beyond the SDK's.
- **Routes, exactly** (spec §9.2; Task 7). Every schedule route answers through `routes::schedules`' existing error body and `Cache-Control: no-store`, has a `#[utoipa::path(... tag = "schedules" ...)]` registered in `ApiDoc`, and a row in the README's new **Automations** section:
  - `GET /api/agents/{agent_id}/schedules` — now calls `state.local_owner.authorize_read` (a deliberate change: the list carries prompts and who created each automation); returns the new fields plus `running`.
  - `POST /api/agents/{agent_id}/schedules` — accepts `name`, the `cron` and `once` triggers, `activeHours`, and `preset: "heartbeat"` with `timeZone` (prompt and trigger then optional); `409` (`TOO_MANY_AUTOMATIONS`) at 20 (legacy import is exempt).
  - `PATCH /api/agents/{agent_id}/schedules/{schedule_id}` — accepts `name` and `activeHours` (`null` clears).
  - `DELETE …/{schedule_id}` and `POST …/schedules/import` — unchanged.
  - `POST /api/agents/{agent_id}/schedules/{schedule_id}/run` — `202` with `{ schedule }`; `404`; `409` (`AUTOMATION_ALREADY_RUNNING`); `429` (`TOO_MANY_RUNNING_AUTOMATIONS`); `503`.
  - `GET /api/agents/{agent_id}/schedules/{schedule_id}/history?limit=` — `{ runs }`, newest first, `limit` 1–50 (default 50); `400` for another limit; `404`; `503` (`AUTOMATION_HISTORY_UNAVAILABLE`) when the history store cannot be read.
  - `POST /api/schedules/preview` — `{ trigger, activeHours? }` → `{ nextRuns: [ms, ms, ms] }` (one for `once`); owner read authorization; `400` with the trigger's problem.
- **Events (spec §6):** `automation.updated` carries `scheduleId` and `deleted` plus `agentId`, `seq`, and `at` (no `sessionId` or `runId`), on the automation's agent's stream, published only after the change was saved: owner and companion changes, claims (tick and Run now), recorded outcomes, and restart reconciliation.
- **Untrusted content.** A companion-written name or prompt is untrusted: names refuse control, line-separator, and invisible format characters (`skills::is_hidden_in_one_line` and `has_variation_selector_run`, made `pub(crate)`), prompts refuse tag and direction-override characters (`skills::is_smuggling_character`, made `pub(crate)`), for owner and companion writes alike (restored records are not re-checked). `list_automations` frames its output as data. The web renders names, prompts, and history as text nodes only, never through `MarkdownMessage` or HTML. Task 13 greps for this.
- **Concurrency, every task.** Lock order: the scheduler's `jobs` mutex (a tokio mutex; `tick_inner` already takes it first) → control-plane transaction → state lock → leaf mutexes (live fanout, history outbox). Nothing takes `jobs` while holding the transaction. No `std::sync::Mutex` is held across `.await`. Every `AutomationService` change runs in its own `tokio::spawn` holding the transaction (`AutomationService::locked`), so a dropped request or tool call never leaves an unsaved change in memory; Run now's claim and job start run in their own task too. Each change is: validate (no lock) → change under the state write lock → save without the state lock → on a failed save put the previous record back → announce `automation.updated`. Commit hooks run under the coordinator's transaction and state lock; their rollback undoes exactly what they did (outcome, counters, fire record). Tests never race the wall clock: every timing function takes `now_ms`/`from_ms`, the scheduler is driven through `tick_at(now)`, and run waits use the existing gated models with `within(…)` bounds.
- **Snapshot version 9.** M6 adds `scheduleFires` and new schedule fields; an M5 daemon would load a v9 file, drop the names, active hours, creators, and counters, and refuse to deserialize a `cron` or `once` trigger. The version moves to 9, and the first start writes `<file>.pre-automations.bak` (JSON) or the `control_plane.backup.8` row (Postgres) first; `pre_upgrade_backup_path` gains a version-8 branch so the M6 upgrade never overwrites `.pre-skills.bak`.
- **Tool grants (spec §13.3 step 5).** `sessions::migration::TOOL_GRANTS` appends `{ id: "m6-automations", read_class: ["list_automations"], write_class: ["create_automation", "pause_automation"] }`. The web's Observe, Collaborate, and Operate profiles add `list_automations`; Collaborate and Operate add `create_automation` and `pause_automation`. Helpers never get the three tools (`agent_runs::helper_config` filters them; each handler refuses a helper).
- Existing behavior stays except where a task's Interfaces block says so. Deliberate changes: `GET …/schedules` needs owner read authorization; the schedule responses gain fields; agent deletion also deletes the agent's `schedule_runs` rows; silent check-in pairs leave the hot tail once mirrored and older than 24 hours, wherever they sit (Task 4; see Notes); schedule prompts refuse tag and direction-override characters on create and update.
- Commands. Rust iteration: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- <filter> <filter>` (filters after `--`), piped through `tail -30`; integration tests: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test schedule_api 2>&1 | tail -30`. SDK: `bun x nx test @animaOS-SWARM/sdk`, and **every SDK-changing task ends with `bun x nx run @animaOS-SWARM/sdk:build`** so later direct web Vitest runs resolve the new exports. Web: `cd apps/web && bun x vitest run <files>`. The milestone gate (Task 13) runs `bun x nx run rust-daemon:test --skipNxCache` and `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`.
- Formatting is clean at the start. Every task ends with `cargo fmt --all` when it touched Rust (then `git diff --stat` must show only the task's files; if `cargo fmt` reformatted unrelated files, tell the controller instead of staging them) and `bun x nx format:write --files=<each changed TS/TSX/CSS/MD file>` when it touched TypeScript, CSS, or Markdown, then re-runs its tests, so every commit stays formatted.
- Stage files by explicit path only; never `git add -A`, `git add .`, or `git commit -a`. Never stage anything under `docs/` or `.superpowers/`, nor `nx.json` or `anima.yaml` (they show in `git status` and are not yours). `hosts/rust-daemon/README.md` is not under `docs/` and is staged with the task that changes it. Never use `git stash`, `git reset`, `git checkout -- <path>`, `git restore`, or `git worktree`, and never switch branches. Do not start the daemon, a dev server, a database, or a container; tests start what they need. Commits are GPG-signed on this machine: a commit can block on a pinentry dialog until the owner answers it.
- Disk is tight (about 13 GB free; the Nx Rust gate needs about 12): never set a new `CARGO_TARGET_DIR`. CI stops at `nx start-ci-run` (Nx Cloud), so the local commands are the verification. No Postgres is available: Postgres tests stay `#[ignore]`.
- Large files stay put: `agent_runs.rs` (~5,900 lines), `connectors/runtime.rs` (~7,600), and `ViewHarness.tsx` (~1,300) only gain wiring lines (`connectors/runtime.rs` gains five field lines in one test literal; nothing else); `schedules.rs` (~1,900) gains the variant arms, the hook changes, and Run now, while new logic goes in `schedules/{cron,timing,automations,history}.rs`. New code goes in new modules, hooks, and components. Web tests stay pristine: no new `act()` warnings or console noise.
- Windows: no test builds a path by string concatenation; every temp path uses `std::env::temp_dir().join(…)` with a UUID, and the Rust tests in this plan touch no workspace files.
- Code fences: complete files and complete functions keep their language; partial fragments (a few lines to insert, a changed signature) are fenced as `text` so Prettier leaves them alone.
- Out of scope (later milestones or non-goals, do not build): auto-pausing after repeated failures (the counters make it possible; Health shows failing automations in M8); usage records (M8); the Health page (M8); the Playwright automations flow (M10, T10.2); editing automations from Telegram; `resume_automation` or `delete_automation` tools (spec §9.3 names three tools); per-automation history retention after its automation is deleted (rows go with the agent); retention of session records (M10).

## Review Focus

1. **Fire times are right and never in the past.** Cron fields, names, steps, macros, the day-of-month/day-of-week rule, time zones, and both daylight-saving edges; active hours (same-day, overnight, gap openings); intervals that keep their cadence inside the window; `once` that fires once and turns itself off. Tests: Task 1 (`dom_and_dow_follow_vixie`, `a_skipped_wall_time_does_not_fire_and_a_repeated_one_fires_once`, `february_31st_never_runs_and_february_29th_waits_for_a_leap_year`), Task 2 (`an_overnight_window_belongs_to_its_start_day`, `an_interval_outside_the_window_waits_for_the_next_opening`, `a_window_opening_in_a_gap_opens_after_it`), Task 6 (`a_once_automation_fires_once_and_turns_itself_off`).
2. **The companion cannot flood or hide automations.** 20 per agent; agent-created ones at least 5 minutes apart over their next 10 fires; helpers refused; hidden characters refused; every agent-created automation is visible on the page and in its chat with Undo. Tests: Task 5 (`the_twenty_first_automation_is_refused`, `agent_automations_must_be_five_minutes_apart`, `hidden_text_and_bad_names_are_refused`), Task 8 (`helpers_cannot_manage_automations`, `create_automation_records_its_creator_and_answers_its_next_runs`, `a_helper_never_gets_the_automation_tools`), Task 12 (RunActivity: "shows a notice card with Undo for a created automation, even collapsed").
3. **An occurrence's outcome, counters, and history are one save.** Commit and rollback agree; a failed save leaves nothing behind; restart reconciliation records a fire too; the outbox writes each fire exactly once and drops a deleted agent's. Tests: Task 3 (`an_outcome_sets_the_counters_and_a_fire_and_its_undo_puts_them_back`, `mirroring_removes_only_unchanged_fires_and_orphans_go`, `unmirrored_fires_skip_deleted_agents`), Task 4 (`fires_reach_the_store_and_leave_the_control_plane`), Task 6 (`a_failed_commit_save_rolls_back_the_outcome_counters_and_fire`, `restart_reconciliation_records_a_failed_fire`).
4. **Run now keeps single-flight and the caps, and survives a dropped request.** Tests: Task 5 (`a_dropped_request_still_saves_its_automation`), Task 6 (`run_now_refuses_while_the_automation_runs`, `run_now_fires_without_moving_the_due_time_and_is_recorded_as_manual`, `run_now_keeps_ownership_and_the_admission_cap`), Task 7 (`run_now_answers_202_and_records_a_manual_fire`, `run_now_conflicts_while_an_occurrence_has_no_outcome`).
5. **Upgrades and rollbacks are safe.** Version 9 with `.pre-automations.bak`; M5 records load with defaults; restore validation names every trigger variant. Tests: Task 2 (`restore_validates_every_trigger_variant`, `stored_triggers_are_validated_variant_by_variant`), Task 3 (`upgrading_a_version_eight_snapshot_writes_the_automations_backup_and_loads_it`, `restore_refuses_invalid_automation_fields_and_fires`).

## File map

| Area            | Files                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Daemon schedule | `hosts/rust-daemon/src`: create `schedules/{cron.rs,timing.rs,automations.rs,history.rs,run_now_tests.rs}`, `state/automation_state.rs`; modify `schedules.rs`, `skills/mod.rs` (three `pub(crate)`), `state.rs`, `connectors/runtime.rs` (one test literal)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Daemon storage  | modify `control_plane_store.rs`, `app/persistence.rs`, `history/{mod.rs,sqlite.rs,postgres.rs,memory.rs,conformance.rs,outbox.rs}`, `sessions/pruning.rs`, and the version-8 assertions in `state.rs`, `approvals/registry.rs`, `skills/registry.rs`, `agent_runs/live_tests.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Daemon events   | modify `live/events.rs`, `live/tests.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Daemon tools    | create `tools/automations.rs`, `agent_runs/{automations.rs,automation_tests.rs}`; modify `tools.rs`, `tools/tests.rs`, `agent_runs.rs` (two module lines, one filter), `sessions/migration.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Daemon routes   | create `routes/tests/automations.rs`; modify `routes/schedules.rs`, `routes/contracts/schedules.rs`, `routes/mod.rs`, `tests/schedule_api.rs`, `hosts/rust-daemon/README.md`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| SDK             | `packages/sdk/src`: create `automations.ts`, `automations.spec.ts`; modify `events.ts`, `events.spec.ts`, `client.ts`, `index.ts`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Web             | `apps/web/src`: create `lib/{schedule-parse.ts,schedule-parse.test.ts,automations.ts,automations.test.ts}`, `hooks/{useAutomations.ts,useAutomations.test.tsx}`, `pages/{AutomationsPage.tsx,AutomationsPage.test.tsx}`, `components/automations/{AutomationEditor.tsx,AutomationHistory.tsx}`, `components/sessions/AutomationNoticeCard.tsx`, `test/automations.ts`, `automations.css`; modify `test/live.ts`, `lib/{session-events.ts,session-events.test.ts,daemon-api.ts,agent-access.ts,agent-access.test.ts,transcript.ts}`, `hooks/useTranscriptActions.ts`, `components/sessions/{RunActivity.tsx,RunActivity.test.tsx,SessionView.tsx,SessionView.test.tsx}`, `components/{WorkspaceShell.tsx,WorkspaceShell.test.tsx,icons.tsx}`, `ViewHarness.tsx`, `ViewHarness.test.tsx`, `styles.css` |
| Docs            | `docs/superpowers/plans/2026-09-23-companion-console.md` (the M6 status row, Task 13, controller only)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |

## Task list

1. Cron expressions: parse and find the next fire time in a time zone (T6.1)
2. Trigger timing: `cron` and `once`, active hours, upcoming fire times, and explicit trigger validation (T6.1)
3. Automation fields, the fire log, restore validation, `automation.updated`, and snapshot version 9 (T6.1, T6.2)
4. Fire history in the history store, the outbox, and silent check-in retention (T6.2)
5. `AutomationService`: create, update, delete, limits, hidden text, and the heartbeat preset (T6.1–T6.3)
6. The scheduler: active hours and `once` at claim, outcomes with counters and fire records, and Run now (T6.2)
7. Automation routes and contracts: new fields, preview, history, Run now, and the README (T6.1, T6.2)
8. Companion tools `create_automation`, `list_automations`, `pause_automation`, helpers, and the tool grant (T6.3)
9. SDK automations client and the `automation.updated` event (T6.4)
10. Web automations data: the phrase parser, labels, the reducer's counter, the facade, `useAutomations`, and the access profiles (T6.4)
11. Web Automations page: list, editor with preview and active hours, heartbeat, Run now, and the history drawer (T6.4)
12. Web notice cards with Undo, the check-in header, and the Automations destination (T6.4)
13. M6 verification

---

### Task 1: Cron expressions: parse and find the next fire time in a time zone

**Files:**

- Create: `hosts/rust-daemon/src/schedules/cron.rs`
- Modify: `hosts/rust-daemon/src/schedules.rs` (one module line)

**Interfaces:**

- Consumes: `chrono::{Datelike, LocalResult, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc}`, `chrono_tz::Tz` (existing daemon dependencies).
- Produces (Tasks 2, 5, 8 use these names):
  - The constants and strings of the Global Constraints' `cron.rs` list; `cron_out_of_range(min, max) -> String`, `cron_field_error(field, detail) -> String`.
  - `CronSchedule` (opaque; `Clone, Debug, PartialEq, Eq`), `parse_cron(expression: &str) -> Result<CronSchedule, String>`, `CronSchedule::next_after(&self, time_zone: Tz, after_ms: u64) -> Option<u64>` (the first fire strictly after `after_ms`, or `None` when none falls within `CRON_SEARCH_DAYS`).
  - `resolve_wall_time(time_zone: Tz, wall: NaiveDateTime, after_ms: u64) -> Option<u64>` (the shared daylight-saving rule; Task 2's window openings use it).
- Behavior: pure; no clock, no state.

- [ ] **Step 1: Write the failing tests**

Add to `hosts/rust-daemon/src/schedules.rs`, after the `use` block and before `const CHECKIN_SENTINEL`:

```text
pub(crate) mod cron;
```

Create `hosts/rust-daemon/src/schedules/cron.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64
    }

    fn next(expression: &str, zone: &str, after_ms: u64) -> Option<u64> {
        parse_cron(expression)
            .unwrap()
            .next_after(zone.parse().unwrap(), after_ms)
    }

    fn bits(values: &[u32]) -> u64 {
        values.iter().fold(0, |bits, value| bits | 1u64 << value)
    }

    #[test]
    fn the_limits_and_strings_are_the_specs() {
        assert_eq!(MAX_CRON_EXPRESSION_CHARS, 200);
        assert_eq!(CRON_SEARCH_DAYS, 28 * 366);
        assert_eq!(CRON_EMPTY, "cron expression is empty");
        assert_eq!(CRON_TOO_LONG, "cron expression must be at most 200 characters");
        assert_eq!(
            CRON_FIELD_COUNT,
            "cron expression must have 5 fields: minute hour day-of-month month day-of-week"
        );
        assert_eq!(
            CRON_MACRO_UNKNOWN,
            "only @hourly, @daily, @midnight, @weekly, @monthly, @yearly, and @annually are supported"
        );
        assert_eq!(CRON_SPECIAL_UNSUPPORTED, "L, W, #, and ? are not supported");
        assert_eq!(CRON_EMPTY_ITEM, "has an empty list item");
        assert_eq!(CRON_STEP_INVALID, "a step must be a whole number from 1");
        assert_eq!(CRON_RANGE_BACKWARDS, "a range must run from low to high");
        assert_eq!(CRON_NOT_A_VALUE, "must be a number or a three-letter name");
        assert_eq!(cron_out_of_range(0, 59), "must be from 0 to 59");
        assert_eq!(cron_field_error("hour", "x"), "hour: x");
    }

    #[test]
    fn fields_accept_numbers_names_ranges_steps_and_lists() {
        let schedule = parse_cron("*/15 9-17 1,15 JAN,jul MON-FRI").unwrap();
        assert_eq!(schedule.minutes, bits(&[0, 15, 30, 45]));
        assert_eq!(schedule.hours, bits(&[9, 10, 11, 12, 13, 14, 15, 16, 17]));
        assert_eq!(schedule.days_of_month, bits(&[1, 15]));
        assert_eq!(schedule.months, bits(&[1, 7]));
        assert_eq!(schedule.days_of_week, bits(&[1, 2, 3, 4, 5]));
        assert!(schedule.dom_restricted && schedule.dow_restricted);

        let every = parse_cron("* * * * *").unwrap();
        assert_eq!(every.minutes, (1u64 << 60) - 1);
        assert_eq!(every.days_of_week, bits(&[0, 1, 2, 3, 4, 5, 6]));
        assert!(!every.dom_restricted && !every.dow_restricted);
        assert_eq!(
            parse_cron("  0   9 * * 1  ").unwrap(),
            parse_cron("0 9 * * mon").unwrap()
        );
    }

    #[test]
    fn seven_is_sunday_and_a_slash_runs_to_the_field_end() {
        assert_eq!(parse_cron("0 0 * * 7").unwrap().days_of_week, bits(&[0]));
        assert_eq!(parse_cron("0 0 * * 5-7").unwrap().days_of_week, bits(&[0, 5, 6]));
        assert_eq!(parse_cron("5/20 * * * *").unwrap().minutes, bits(&[5, 25, 45]));
        assert_eq!(parse_cron("0-10/5 * * * *").unwrap().minutes, bits(&[0, 5, 10]));
        // A huge step is one value, never an overflow.
        assert_eq!(parse_cron("*/4294967295 * * * *").unwrap().minutes, bits(&[0]));
    }

    #[test]
    fn macros_expand_and_reboot_is_refused() {
        for (shorthand, fields) in [
            ("@hourly", "0 * * * *"),
            ("@daily", "0 0 * * *"),
            ("@midnight", "0 0 * * *"),
            ("@Weekly", "0 0 * * 0"),
            ("@monthly", "0 0 1 * *"),
            ("@yearly", "0 0 1 1 *"),
            ("@ANNUALLY", "0 0 1 1 *"),
        ] {
            assert_eq!(parse_cron(shorthand), parse_cron(fields), "{shorthand}");
        }
        assert_eq!(parse_cron("@reboot"), Err(CRON_MACRO_UNKNOWN.to_string()));
    }

    #[test]
    fn bad_expressions_name_their_problem() {
        let too_long = format!("0 0 * * *{}x", " ".repeat(200));
        for (expression, problem) in [
            ("", CRON_EMPTY.to_string()),
            ("   ", CRON_EMPTY.to_string()),
            (too_long.as_str(), CRON_TOO_LONG.to_string()),
            ("* * * *", CRON_FIELD_COUNT.to_string()),
            ("* * * * * *", CRON_FIELD_COUNT.to_string()),
            ("60 * * * *", "minute: must be from 0 to 59".to_string()),
            ("* 24 * * *", "hour: must be from 0 to 23".to_string()),
            ("* * 0 * *", "day of month: must be from 1 to 31".to_string()),
            ("* * * 13 *", "month: must be from 1 to 12".to_string()),
            ("* * * * 8", "day of week: must be from 0 to 7".to_string()),
            ("99999999999 * * * *", "minute: must be from 0 to 59".to_string()),
            ("*/0 * * * *", cron_field_error("minute", CRON_STEP_INVALID)),
            ("*/x * * * *", cron_field_error("minute", CRON_STEP_INVALID)),
            ("5-1 * * * *", cron_field_error("minute", CRON_RANGE_BACKWARDS)),
            ("1,,2 * * * *", cron_field_error("minute", CRON_EMPTY_ITEM)),
            ("-5 * * * *", cron_field_error("minute", CRON_EMPTY_ITEM)),
            ("* * L * *", cron_field_error("day of month", CRON_SPECIAL_UNSUPPORTED)),
            ("* * 15W * *", cron_field_error("day of month", CRON_SPECIAL_UNSUPPORTED)),
            ("* * ? * *", cron_field_error("day of month", CRON_SPECIAL_UNSUPPORTED)),
            ("* * * * 1#2", cron_field_error("day of week", CRON_SPECIAL_UNSUPPORTED)),
            ("* * * * 5L", cron_field_error("day of week", CRON_SPECIAL_UNSUPPORTED)),
            ("* * * FOO *", cron_field_error("month", CRON_NOT_A_VALUE)),
            ("* * * * SUNDAY", cron_field_error("day of week", CRON_NOT_A_VALUE)),
        ] {
            assert_eq!(parse_cron(expression), Err(problem), "{expression:?}");
        }
    }

    #[test]
    fn the_next_fire_is_strictly_after_and_on_the_wall_clock() {
        let nine_thirty = utc(2026, 1, 5, 9, 30);
        assert_eq!(next("30 9 * * *", "UTC", nine_thirty - 1), Some(nine_thirty));
        assert_eq!(
            next("30 9 * * *", "UTC", nine_thirty),
            Some(utc(2026, 1, 6, 9, 30)),
            "never the minute it was asked from"
        );
        assert_eq!(
            next("30 9 * * *", "UTC", nine_thirty + 30_000),
            Some(utc(2026, 1, 6, 9, 30))
        );
        // Kuala Lumpur is UTC+8 all year: 09:00 there is 01:00 UTC.
        assert_eq!(
            next("0 9 * * *", "Asia/Kuala_Lumpur", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 5, 1, 0))
        );
        assert_eq!(
            next("*/15 * * * *", "UTC", utc(2026, 1, 5, 23, 59)),
            Some(utc(2026, 1, 6, 0, 0)),
            "the search rolls over midnight"
        );
    }

    #[test]
    fn a_time_zone_shifts_the_day() {
        // Kiritimati is UTC+14: Sunday 14:00 there when it is 00:00 UTC.
        assert_eq!(
            next("0 0 * * 1", "Pacific/Kiritimati", utc(2026, 1, 4, 0, 0)),
            Some(utc(2026, 1, 4, 10, 0))
        );
    }

    #[test]
    fn dom_and_dow_follow_vixie() {
        // 2026-01-05 is a Monday. Both restricted: the 13th or any Friday.
        assert_eq!(
            next("0 0 13 * 5", "UTC", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 9, 0, 0))
        );
        assert_eq!(
            next("0 0 13 * *", "UTC", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 13, 0, 0))
        );
        // A day-of-month field starting with `*` is unrestricted: both must
        // match, so an odd-numbered Monday (19 January, not 7 January).
        assert_eq!(
            next("0 0 */2 * 1", "UTC", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 19, 0, 0))
        );
    }

    #[test]
    fn a_skipped_wall_time_does_not_fire_and_a_repeated_one_fires_once() {
        // New York springs forward at 02:00 on 2026-03-08: 02:30 does not
        // exist that day, so the next 02:30 is on the 9th (EDT, UTC-4).
        assert_eq!(
            next("30 2 * * *", "America/New_York", utc(2026, 3, 7, 12, 0)),
            Some(utc(2026, 3, 9, 6, 30))
        );
        // It falls back at 02:00 on 2026-11-01: 01:30 happens twice.
        let first = utc(2026, 11, 1, 5, 30); // 01:30 EDT
        assert_eq!(
            next("30 1 * * *", "America/New_York", utc(2026, 11, 1, 4, 0)),
            Some(first)
        );
        assert_eq!(
            next("30 1 * * *", "America/New_York", first),
            Some(utc(2026, 11, 2, 6, 30)),
            "the repeated 01:30 does not fire again"
        );
        // Asked from inside the repeated hour, the next 01:30 is its second
        // occurrence (EST, UTC-5), the first one after the question.
        assert_eq!(
            next("30 1 * * *", "America/New_York", utc(2026, 11, 1, 6, 10)),
            Some(utc(2026, 11, 1, 6, 30))
        );
    }

    #[test]
    fn february_31st_never_runs_and_february_29th_waits_for_a_leap_year() {
        assert_eq!(next("0 0 31 2 *", "UTC", utc(2026, 1, 5, 0, 0)), None);
        assert_eq!(
            next("0 0 29 2 *", "UTC", utc(2026, 3, 1, 0, 0)),
            Some(utc(2028, 2, 29, 0, 0))
        );
    }

    #[test]
    fn an_instant_outside_chronos_range_has_no_next_fire() {
        assert_eq!(next("* * * * *", "UTC", u64::MAX), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::cron 2>&1 | tail -30`
Expected: FAIL to compile (`parse_cron`, `CronSchedule`, and the constants are not defined).

- [ ] **Step 3: Write the implementation**

Put this above the test module in `hosts/rust-daemon/src/schedules/cron.rs`:

```rust
//! Five-field cron expressions (spec §9.1), evaluated on an IANA time
//! zone's wall clock. In-house, because the daemon takes no cron crate (M6
//! Global Constraints). Day of month and day of week combine the Vixie way:
//! when both are restricted, either may match.
#![allow(dead_code)] // M6 Task 2 uses every item.

use chrono::{Datelike, LocalResult, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

/// The longest expression accepted, in characters.
pub(crate) const MAX_CRON_EXPRESSION_CHARS: usize = 200;
/// Days searched for the next fire: 28 years cover every pairing of a day
/// of the month with a day of the week; anything rarer never fires.
pub(crate) const CRON_SEARCH_DAYS: u32 = 28 * 366;

pub(crate) const CRON_EMPTY: &str = "cron expression is empty";
pub(crate) const CRON_TOO_LONG: &str = "cron expression must be at most 200 characters";
pub(crate) const CRON_FIELD_COUNT: &str =
    "cron expression must have 5 fields: minute hour day-of-month month day-of-week";
pub(crate) const CRON_MACRO_UNKNOWN: &str =
    "only @hourly, @daily, @midnight, @weekly, @monthly, @yearly, and @annually are supported";
pub(crate) const CRON_SPECIAL_UNSUPPORTED: &str = "L, W, #, and ? are not supported";
pub(crate) const CRON_EMPTY_ITEM: &str = "has an empty list item";
pub(crate) const CRON_STEP_INVALID: &str = "a step must be a whole number from 1";
pub(crate) const CRON_RANGE_BACKWARDS: &str = "a range must run from low to high";
pub(crate) const CRON_NOT_A_VALUE: &str = "must be a number or a three-letter name";

pub(crate) fn cron_out_of_range(min: u32, max: u32) -> String {
    format!("must be from {min} to {max}")
}

pub(crate) fn cron_field_error(field: &str, detail: &str) -> String {
    format!("{field}: {detail}")
}

#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    min: u32,
    max: u32,
    /// Three-letter names, the first worth `name_base`.
    names: &'static [&'static str],
    name_base: u32,
}

const MINUTE: Field = Field {
    name: "minute",
    min: 0,
    max: 59,
    names: &[],
    name_base: 0,
};
const HOUR: Field = Field {
    name: "hour",
    min: 0,
    max: 23,
    names: &[],
    name_base: 0,
};
const DAY_OF_MONTH: Field = Field {
    name: "day of month",
    min: 1,
    max: 31,
    names: &[],
    name_base: 0,
};
const MONTH: Field = Field {
    name: "month",
    min: 1,
    max: 12,
    names: &[
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ],
    name_base: 1,
};
const DAY_OF_WEEK: Field = Field {
    name: "day of week",
    min: 0,
    max: 7,
    names: &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
    name_base: 0,
};

impl Field {
    fn error(&self, detail: &str) -> String {
        cron_field_error(self.name, detail)
    }

    fn value(&self, token: &str) -> Result<u32, String> {
        if token.is_empty() {
            return Err(self.error(CRON_EMPTY_ITEM));
        }
        if token.chars().all(|character| character.is_ascii_digit()) {
            return token
                .parse::<u32>()
                .ok()
                .filter(|value| (self.min..=self.max).contains(value))
                .ok_or_else(|| self.error(&cron_out_of_range(self.min, self.max)));
        }
        if let Some(index) = self
            .names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(token))
        {
            return Ok(self.name_base + index as u32);
        }
        let last = token.chars().last().map(|last| last.to_ascii_uppercase());
        if token.contains(['#', '?']) || matches!(last, Some('L' | 'W')) {
            return Err(self.error(CRON_SPECIAL_UNSUPPORTED));
        }
        Err(self.error(CRON_NOT_A_VALUE))
    }

    /// The field's values as bits (bit `n` is value `n`).
    fn parse(&self, text: &str) -> Result<u64, String> {
        let mut bits = 0u64;
        for item in text.split(',') {
            if item.is_empty() {
                return Err(self.error(CRON_EMPTY_ITEM));
            }
            let (range, step) = match item.split_once('/') {
                Some((range, step)) => {
                    let step = step
                        .parse::<u32>()
                        .ok()
                        .filter(|step| *step >= 1)
                        .ok_or_else(|| self.error(CRON_STEP_INVALID))?;
                    (range, Some(step))
                }
                None => (item, None),
            };
            let (start, end) = if range == "*" {
                (self.min, self.max)
            } else if let Some((low, high)) = range.split_once('-') {
                let (low, high) = (self.value(low)?, self.value(high)?);
                if low > high {
                    return Err(self.error(CRON_RANGE_BACKWARDS));
                }
                (low, high)
            } else {
                let value = self.value(range)?;
                // `a/s` runs from `a` to the field's end.
                (value, if step.is_some() { self.max } else { value })
            };
            let step = step.unwrap_or(1);
            let mut value = start;
            while value <= end {
                bits |= 1u64 << value;
                value = match value.checked_add(step) {
                    Some(next) => next,
                    None => break,
                };
            }
        }
        Ok(bits)
    }
}

/// A parsed expression: the allowed values of each field as bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CronSchedule {
    minutes: u64,
    hours: u64,
    days_of_month: u64,
    months: u64,
    /// Sunday is bit 0 (a 7 in the expression folds onto it).
    days_of_week: u64,
    dom_restricted: bool,
    dow_restricted: bool,
}

pub(crate) fn parse_cron(expression: &str) -> Result<CronSchedule, String> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return Err(CRON_EMPTY.into());
    }
    if trimmed.chars().count() > MAX_CRON_EXPRESSION_CHARS {
        return Err(CRON_TOO_LONG.into());
    }
    let expanded = match trimmed.strip_prefix('@') {
        Some(name) => match name.to_ascii_lowercase().as_str() {
            "hourly" => "0 * * * *",
            "daily" | "midnight" => "0 0 * * *",
            "weekly" => "0 0 * * 0",
            "monthly" => "0 0 1 * *",
            "yearly" | "annually" => "0 0 1 1 *",
            _ => return Err(CRON_MACRO_UNKNOWN.into()),
        },
        None => trimmed,
    };
    let fields = expanded.split_whitespace().collect::<Vec<_>>();
    let &[minute, hour, day_of_month, month, day_of_week] = fields.as_slice() else {
        return Err(CRON_FIELD_COUNT.into());
    };
    let minutes = MINUTE.parse(minute)?;
    let hours = HOUR.parse(hour)?;
    let days_of_month = DAY_OF_MONTH.parse(day_of_month)?;
    let months = MONTH.parse(month)?;
    let mut days_of_week = DAY_OF_WEEK.parse(day_of_week)?;
    if days_of_week & (1 << 7) != 0 {
        days_of_week = (days_of_week | 1) & !(1 << 7);
    }
    Ok(CronSchedule {
        minutes,
        hours,
        days_of_month,
        months,
        days_of_week,
        dom_restricted: !day_of_month.starts_with('*'),
        dow_restricted: !day_of_week.starts_with('*'),
    })
}

impl CronSchedule {
    fn day_matches(&self, date: NaiveDate) -> bool {
        if self.months & (1u64 << date.month()) == 0 {
            return false;
        }
        let dom = self.days_of_month & (1u64 << date.day()) != 0;
        let dow = self.days_of_week & (1u64 << date.weekday().num_days_from_sunday()) != 0;
        if self.dom_restricted && self.dow_restricted {
            dom || dow
        } else {
            dom && dow
        }
    }

    /// The first fire strictly after `after_ms` on `time_zone`'s wall clock:
    /// wall-clock minutes after the current one are tried in order, each
    /// resolved by [`resolve_wall_time`]. `None` when none falls within
    /// `CRON_SEARCH_DAYS` days.
    pub(crate) fn next_after(&self, time_zone: Tz, after_ms: u64) -> Option<u64> {
        let after = Utc
            .timestamp_millis_opt(i64::try_from(after_ms).ok()?)
            .single()?;
        let local = after.with_timezone(&time_zone).naive_local();
        let first = local
            .date()
            .and_hms_opt(local.hour(), local.minute(), 0)?
            .checked_add_signed(TimeDelta::minutes(1))?;
        let mut date = first.date();
        for day in 0..CRON_SEARCH_DAYS {
            if self.day_matches(date) {
                let earliest = if day == 0 {
                    first.hour() * 60 + first.minute()
                } else {
                    0
                };
                for hour in (0..24u32).filter(|hour| self.hours & (1u64 << hour) != 0) {
                    for minute in (0..60u32).filter(|minute| self.minutes & (1u64 << minute) != 0)
                    {
                        if hour * 60 + minute < earliest {
                            continue;
                        }
                        let wall = date.and_hms_opt(hour, minute, 0)?;
                        if let Some(at_ms) = resolve_wall_time(time_zone, wall, after_ms) {
                            return Some(at_ms);
                        }
                    }
                }
            }
            date = date.succ_opt()?;
        }
        None
    }
}

/// The instant of wall-clock `wall` in `time_zone` that comes after
/// `after_ms`. A time a daylight-saving jump skips does not exist (`None`);
/// a repeated time gives its first occurrence after `after_ms`.
pub(crate) fn resolve_wall_time(time_zone: Tz, wall: NaiveDateTime, after_ms: u64) -> Option<u64> {
    let instants = match time_zone.from_local_datetime(&wall) {
        LocalResult::Single(at) => [Some(at), None],
        LocalResult::Ambiguous(first, second) => [Some(first.min(second)), Some(first.max(second))],
        LocalResult::None => [None, None],
    };
    instants
        .into_iter()
        .flatten()
        .filter_map(|at| u64::try_from(at.timestamp_millis()).ok())
        .find(|at_ms| *at_ms > after_ms)
}
```

`TimeDelta::minutes` is chrono 0.4.44's constructor; if the compiler reports it as missing on this toolchain, use `chrono::Duration::minutes(1)` (the same type under its older name). Do not add a chrono feature.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::cron 2>&1 | tail -30`
Expected: PASS (10 tests).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/schedules/cron.rs
git commit -m "feat(daemon): parse cron expressions and find their next fire time"
```

Recommended implementer tier: standard (pure code given in full; the daylight-saving tests pin the rules).

---

### Task 2: Trigger timing: `cron` and `once`, active hours, upcoming fire times, and explicit trigger validation

**Files:**

- Create: `hosts/rust-daemon/src/schedules/timing.rs`
- Modify: `hosts/rust-daemon/src/schedules.rs` (module line, trigger variants, `ScheduleError::Rejected`, the timing functions), `hosts/rust-daemon/src/schedules/cron.rs` (drop the temporary `allow`), `hosts/rust-daemon/src/routes/schedules.rs` (one error arm), `hosts/rust-daemon/src/routes/contracts/schedules.rs` (response arms), `hosts/rust-daemon/src/state.rs` (restore validation and one test)

**Interfaces:**

- Consumes: Task 1's `parse_cron`, `CronSchedule::next_after`, `resolve_wall_time`.
- Produces:
  - `ScheduleTrigger::Cron { expression, time_zone }` and `ScheduleTrigger::Once { at_ms }` (persisted as `{"cron":{"expression","timeZone"}}` and `{"once":{"atMs"}}`).
  - `ScheduleError::Rejected(String)` (a 400 with a built message; the routes answer `schedule_invalid`).
  - `schedules::timing::{ActiveHours { start, end, days, time_zone }, ActiveWindow, parse_time_zone, parse_clock, normalized_active_hours, validate_stored_trigger, next_fire_after, next_fire_after_claim, upcoming_fires}` and the constants and strings of the Global Constraints' `timing.rs` list plus `INTERVAL_INVALID = "intervalMs must be a positive whole number of seconds"`, `DAILY_INVALID = "daily trigger is invalid"`, `ONCE_INVALID = "atMs must be a positive time in milliseconds"` (the first two are the literals `schedules.rs` answers today, now named). `ActiveWindow::{parse(&ActiveHours) -> Result<Self, String>, contains(at_ms) -> bool, next_open(at_ms) -> Option<u64>}`.
  - `schedules::{next_due(trigger, active_hours: Option<&ActiveHours>, from_ms) -> Result<u64, ScheduleError>, next_due_after_claim(trigger, active_hours, previous_due, now) -> Result<u64, ScheduleError>}`; `next_due_at_ms(trigger, from_ms)` stays (it is `next_due(trigger, None, from_ms)`).
- Behavior: `interval` and `daily` fire exactly as before. Validation of every trigger, at creation, update, and restore, is one explicit `match` (`validate_stored_trigger`); `state.rs` loses its catch-all arm. `anima-schedule` is no longer called by the daemon (its validation was a roundabout way to check what `validate_stored_trigger` now checks directly); the dependency line stays, since removing dependencies is out of scope.

- [ ] **Step 1: Write the failing tests**

Add to `hosts/rust-daemon/src/schedules.rs`, after `pub(crate) mod cron;`:

```text
pub(crate) mod timing;
pub(crate) use timing::ActiveHours;
```

Add two variants to `ScheduleTrigger` in `schedules.rs`, after `Daily { … }`:

```rust
    /// Five-field cron on `time_zone`'s wall clock (spec §9.1).
    Cron {
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Fires once; the claim turns the automation off (spec §9.1).
    Once {
        #[serde(rename = "atMs")]
        at_ms: u64,
    },
```

Create `hosts/rust-daemon/src/schedules/timing.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64
    }

    fn hours(start: &str, end: &str, days: &[u8], zone: &str) -> ActiveHours {
        ActiveHours {
            start: start.into(),
            end: end.into(),
            days: days.to_vec(),
            time_zone: zone.into(),
        }
    }

    fn window(start: &str, end: &str, days: &[u8], zone: &str) -> ActiveWindow {
        ActiveWindow::parse(&hours(start, end, days, zone)).unwrap()
    }

    const EVERY_DAY: &[u8] = &[0, 1, 2, 3, 4, 5, 6];
    const WEEKDAYS: &[u8] = &[1, 2, 3, 4, 5];
    const HALF_HOUR: ScheduleTrigger = ScheduleTrigger::Interval {
        interval_ms: 30 * 60_000,
    };

    fn cron(expression: &str) -> ScheduleTrigger {
        ScheduleTrigger::Cron {
            expression: expression.into(),
            time_zone: "UTC".into(),
        }
    }

    #[test]
    fn the_bounds_and_strings_are_the_specs() {
        assert_eq!(MAX_WINDOW_HOPS, 400);
        assert_eq!(DST_GAP_SEARCH_MINUTES, 180);
        assert_eq!(TIME_ZONE_INVALID, "timeZone is invalid");
        assert_eq!(
            INTERVAL_INVALID,
            "intervalMs must be a positive whole number of seconds"
        );
        assert_eq!(DAILY_INVALID, "daily trigger is invalid");
        assert_eq!(ONCE_INVALID, "atMs must be a positive time in milliseconds");
        assert_eq!(
            ACTIVE_HOURS_TIME_INVALID,
            "activeHours start and end must be HH:MM in 24-hour time"
        );
        assert_eq!(ACTIVE_HOURS_SAME_TIME, "activeHours start and end must differ");
        assert_eq!(
            ACTIVE_HOURS_DAYS_INVALID,
            "activeHours days must list 1 to 7 different days from 0 (Sunday) to 6 (Saturday)"
        );
        assert_eq!(
            ACTIVE_HOURS_NOT_FOR_ONCE,
            "activeHours does not apply to a one-time automation"
        );
        assert_eq!(ONCE_NOT_IN_FUTURE, "atMs must be in the future");
        assert_eq!(SCHEDULE_NEVER_RUNS, "This schedule never runs");
        assert_eq!(
            SCHEDULE_NEVER_IN_ACTIVE_HOURS,
            "This schedule never runs inside its active hours"
        );
    }

    #[test]
    fn active_hours_are_validated_and_stored_with_their_days_in_order() {
        assert_eq!(
            normalized_active_hours(hours("08:00", "22:00", &[5, 1, 3], "UTC")),
            Ok(hours("08:00", "22:00", &[1, 3, 5], "UTC"))
        );
        for (bad, problem) in [
            (hours("8:00", "22:00", WEEKDAYS, "UTC"), ACTIVE_HOURS_TIME_INVALID),
            (hours("24:00", "22:00", WEEKDAYS, "UTC"), ACTIVE_HOURS_TIME_INVALID),
            (hours("08:60", "22:00", WEEKDAYS, "UTC"), ACTIVE_HOURS_TIME_INVALID),
            (hours("08:00", "2200", WEEKDAYS, "UTC"), ACTIVE_HOURS_TIME_INVALID),
            (hours("+8:00", "22:00", WEEKDAYS, "UTC"), ACTIVE_HOURS_TIME_INVALID),
            (hours("08:00", "08:00", WEEKDAYS, "UTC"), ACTIVE_HOURS_SAME_TIME),
            (hours("08:00", "22:00", &[], "UTC"), ACTIVE_HOURS_DAYS_INVALID),
            (hours("08:00", "22:00", &[7], "UTC"), ACTIVE_HOURS_DAYS_INVALID),
            (hours("08:00", "22:00", &[1, 1], "UTC"), ACTIVE_HOURS_DAYS_INVALID),
            (hours("08:00", "22:00", WEEKDAYS, "Mars/Base"), TIME_ZONE_INVALID),
            (hours("08:00", "22:00", WEEKDAYS, " "), TIME_ZONE_INVALID),
        ] {
            assert_eq!(
                normalized_active_hours(bad.clone()),
                Err(problem.to_string()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_same_day_window_covers_start_up_to_end() {
        // 2026-01-05 is a Monday; 2026-01-10 a Saturday.
        let open = window("08:00", "22:00", WEEKDAYS, "UTC");
        assert!(open.contains(utc(2026, 1, 5, 8, 0)));
        assert!(open.contains(utc(2026, 1, 5, 21, 59)));
        assert!(!open.contains(utc(2026, 1, 5, 22, 0)));
        assert!(!open.contains(utc(2026, 1, 5, 7, 59)));
        assert!(!open.contains(utc(2026, 1, 10, 10, 0)));
    }

    #[test]
    fn an_overnight_window_belongs_to_its_start_day() {
        // Friday 22:00 to Saturday 06:00 only (2026-01-09 is a Friday).
        let night = window("22:00", "06:00", &[5], "UTC");
        assert!(night.contains(utc(2026, 1, 9, 23, 0)));
        assert!(night.contains(utc(2026, 1, 10, 5, 59)));
        assert!(!night.contains(utc(2026, 1, 10, 23, 0)), "Saturday night is not listed");
        assert!(!night.contains(utc(2026, 1, 9, 5, 0)), "Thursday night is not listed");
    }

    #[test]
    fn the_next_opening_is_now_inside_or_the_next_listed_day() {
        let open = window("08:00", "22:00", WEEKDAYS, "UTC");
        let inside = utc(2026, 1, 5, 9, 0);
        assert_eq!(open.next_open(inside), Some(inside));
        assert_eq!(
            open.next_open(utc(2026, 1, 5, 7, 0)),
            Some(utc(2026, 1, 5, 8, 0))
        );
        assert_eq!(
            open.next_open(utc(2026, 1, 9, 22, 30)),
            Some(utc(2026, 1, 12, 8, 0)),
            "Friday night waits for Monday"
        );
    }

    #[test]
    fn a_window_opening_in_a_gap_opens_after_it() {
        // New York has no 02:30 on 2026-03-08: the window opens at 03:00 EDT.
        let early = window("02:30", "05:00", EVERY_DAY, "America/New_York");
        assert_eq!(
            early.next_open(utc(2026, 3, 8, 5, 0)),
            Some(utc(2026, 3, 8, 7, 0))
        );
    }

    #[test]
    fn an_interval_outside_the_window_waits_for_the_next_opening() {
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after(&HALF_HOUR, Some(&day), utc(2026, 1, 5, 21, 45)),
            Ok(utc(2026, 1, 6, 8, 0))
        );
        assert_eq!(
            upcoming_fires(&HALF_HOUR, Some(&day), utc(2026, 1, 5, 21, 0), 3),
            Ok(vec![
                utc(2026, 1, 5, 21, 30),
                utc(2026, 1, 6, 8, 0),
                utc(2026, 1, 6, 8, 30),
            ])
        );
        assert_eq!(
            upcoming_fires(&HALF_HOUR, None, utc(2026, 1, 5, 21, 0), 2),
            Ok(vec![utc(2026, 1, 5, 21, 30), utc(2026, 1, 5, 22, 0)])
        );
        assert_eq!(upcoming_fires(&HALF_HOUR, None, 1, 0), Ok(vec![]));
    }

    #[test]
    fn cron_and_daily_skip_fires_outside_the_window() {
        let mornings = window("09:00", "11:00", WEEKDAYS, "UTC");
        assert_eq!(
            upcoming_fires(&cron("0 * * * *"), Some(&mornings), utc(2026, 1, 5, 7, 30), 3),
            Ok(vec![
                utc(2026, 1, 5, 9, 0),
                utc(2026, 1, 5, 10, 0),
                utc(2026, 1, 6, 9, 0),
            ])
        );
        let early = ScheduleTrigger::Daily {
            hour: 7,
            minute: 0,
            time_zone: "UTC".into(),
        };
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after(&early, Some(&day), utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string())
        );
        assert_eq!(
            next_fire_after(&cron("0 0 31 2 *"), None, utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_RUNS.to_string())
        );
    }

    #[test]
    fn claims_keep_an_intervals_cadence_inside_the_window() {
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after_claim(
                &HALF_HOUR,
                Some(&day),
                utc(2026, 1, 5, 21, 30),
                utc(2026, 1, 5, 21, 31)
            ),
            Ok(utc(2026, 1, 6, 8, 0))
        );
        // Claimed 70 minutes late: the missed steps are skipped, not replayed.
        assert_eq!(
            next_fire_after_claim(
                &HALF_HOUR,
                Some(&day),
                utc(2026, 1, 6, 8, 0),
                utc(2026, 1, 6, 9, 10)
            ),
            Ok(utc(2026, 1, 6, 9, 30))
        );
        assert_eq!(
            next_fire_after_claim(&cron("0 9 * * *"), None, 1, utc(2026, 1, 5, 9, 0)),
            Ok(utc(2026, 1, 6, 9, 0))
        );
    }

    #[test]
    fn once_fires_once_and_refuses_active_hours() {
        let at = utc(2026, 1, 5, 15, 0);
        let once = ScheduleTrigger::Once { at_ms: at };
        assert_eq!(next_fire_after(&once, None, at - 1), Ok(at));
        assert_eq!(
            next_fire_after(&once, None, at),
            Err(ONCE_NOT_IN_FUTURE.to_string())
        );
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after(&once, Some(&day), at - 1),
            Err(ACTIVE_HOURS_NOT_FOR_ONCE.to_string())
        );
        assert_eq!(upcoming_fires(&once, None, at - 1, 3), Ok(vec![at]));
        assert_eq!(next_fire_after_claim(&once, None, at, at + 5), Ok(at));
    }

    #[test]
    fn stored_triggers_are_validated_variant_by_variant() {
        for good in [
            ScheduleTrigger::Interval { interval_ms: 60_000 },
            ScheduleTrigger::Daily {
                hour: 23,
                minute: 59,
                time_zone: "Asia/Kuala_Lumpur".into(),
            },
            cron("@daily"),
            ScheduleTrigger::Once { at_ms: 1 },
        ] {
            assert_eq!(validate_stored_trigger(&good), Ok(()), "{good:?}");
        }
        for (bad, problem) in [
            (ScheduleTrigger::Interval { interval_ms: 0 }, INTERVAL_INVALID.to_string()),
            (ScheduleTrigger::Interval { interval_ms: 1_500 }, INTERVAL_INVALID.to_string()),
            (
                ScheduleTrigger::Daily {
                    hour: 24,
                    minute: 0,
                    time_zone: "UTC".into(),
                },
                DAILY_INVALID.to_string(),
            ),
            (
                ScheduleTrigger::Daily {
                    hour: 9,
                    minute: 0,
                    time_zone: "".into(),
                },
                TIME_ZONE_INVALID.to_string(),
            ),
            (cron("61 * * * *"), "minute: must be from 0 to 59".to_string()),
            (
                ScheduleTrigger::Cron {
                    expression: "@daily".into(),
                    time_zone: "Nowhere/Town".into(),
                },
                TIME_ZONE_INVALID.to_string(),
            ),
            (ScheduleTrigger::Once { at_ms: 0 }, ONCE_INVALID.to_string()),
        ] {
            assert_eq!(validate_stored_trigger(&bad), Err(problem), "{bad:?}");
        }
    }

    #[test]
    fn a_daily_trigger_fires_as_before() {
        // Kuala Lumpur 09:30 is 01:30 UTC; asked at 01:30 UTC exactly, the
        // next one is tomorrow's.
        let daily = ScheduleTrigger::Daily {
            hour: 9,
            minute: 30,
            time_zone: "Asia/Kuala_Lumpur".into(),
        };
        assert_eq!(
            next_fire_after(&daily, None, utc(2026, 1, 5, 1, 0)),
            Ok(utc(2026, 1, 5, 1, 30))
        );
        assert_eq!(
            next_fire_after(&daily, None, utc(2026, 1, 5, 1, 30)),
            Ok(utc(2026, 1, 6, 1, 30))
        );
    }
}
```

Add to `hosts/rust-daemon/src/state.rs`'s test module, after `restore_allows_an_overdue_schedule_next_due_time`:

```rust
    #[test]
    fn restore_validates_every_trigger_variant() {
        // `ScheduleTrigger` is already imported by this test module.
        for (trigger, valid) in [
            (
                ScheduleTrigger::Cron {
                    expression: "0 9 * * 1-5".into(),
                    time_zone: "Europe/London".into(),
                },
                true,
            ),
            (ScheduleTrigger::Once { at_ms: 5 }, true),
            (
                ScheduleTrigger::Cron {
                    expression: "0 9 * *".into(),
                    time_zone: "Europe/London".into(),
                },
                false,
            ),
            (
                ScheduleTrigger::Cron {
                    expression: "0 9 * * *".into(),
                    time_zone: "".into(),
                },
                false,
            ),
            (ScheduleTrigger::Once { at_ms: 0 }, false),
        ] {
            let (mut snapshot, _) = valid_connector_snapshot();
            snapshot.schedules[0].trigger = trigger.clone();
            let mut state = DaemonState::new();
            assert_eq!(
                state.restore_control_plane_snapshot(snapshot).is_ok(),
                valid,
                "{trigger:?}"
            );
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::timing state::tests::restore_validates_every_trigger_variant 2>&1 | tail -30`
Expected: FAIL to compile (`timing` has no items; the new variants make `match`es in `schedules.rs` and `routes/contracts/schedules.rs` non-exhaustive).

- [ ] **Step 3: Write `timing.rs`**

Put this above the test module in `hosts/rust-daemon/src/schedules/timing.rs`:

```rust
//! When an automation fires (spec §9.1): the next fire time of any trigger,
//! moved into its active hours; the next few, for previews and the agent
//! minimum; and the validation every trigger passes, on creation and on
//! restore, one variant at a time.

use chrono::{Datelike, LocalResult, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::cron::parse_cron;
use super::ScheduleTrigger;

/// Window openings tried before a cron or daily trigger is declared never
/// to fire inside its active hours.
pub(crate) const MAX_WINDOW_HOPS: usize = 400;
/// Minutes a window opening inside a daylight-saving gap is moved forward,
/// at most, to the first wall time that exists.
pub(crate) const DST_GAP_SEARCH_MINUTES: i64 = 180;

pub(crate) const TIME_ZONE_INVALID: &str = "timeZone is invalid";
pub(crate) const INTERVAL_INVALID: &str = "intervalMs must be a positive whole number of seconds";
pub(crate) const DAILY_INVALID: &str = "daily trigger is invalid";
pub(crate) const ONCE_INVALID: &str = "atMs must be a positive time in milliseconds";
pub(crate) const ACTIVE_HOURS_TIME_INVALID: &str =
    "activeHours start and end must be HH:MM in 24-hour time";
pub(crate) const ACTIVE_HOURS_SAME_TIME: &str = "activeHours start and end must differ";
pub(crate) const ACTIVE_HOURS_DAYS_INVALID: &str =
    "activeHours days must list 1 to 7 different days from 0 (Sunday) to 6 (Saturday)";
pub(crate) const ACTIVE_HOURS_NOT_FOR_ONCE: &str =
    "activeHours does not apply to a one-time automation";
pub(crate) const ONCE_NOT_IN_FUTURE: &str = "atMs must be in the future";
pub(crate) const SCHEDULE_NEVER_RUNS: &str = "This schedule never runs";
pub(crate) const SCHEDULE_NEVER_IN_ACTIVE_HOURS: &str =
    "This schedule never runs inside its active hours";
/// The literal `schedules.rs` already answered for an overflowing time.
const TIMING_OVERFLOW: &str = "schedule timing overflow";

/// When an automation may fire (spec §9.1), as stored and sent: wall-clock
/// `start` and `end` (`HH:MM`), days numbered as JavaScript's `getDay()`
/// (0 is Sunday), and the zone they are read in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActiveHours {
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) days: Vec<u8>,
    pub(crate) time_zone: String,
}

pub(crate) fn parse_time_zone(name: &str) -> Result<Tz, String> {
    name.trim()
        .parse::<Tz>()
        .map_err(|_| TIME_ZONE_INVALID.to_string())
}

/// Minutes after midnight of an `HH:MM` wall time.
pub(crate) fn parse_clock(text: &str) -> Option<u32> {
    let (hour, minute) = text.split_once(':')?;
    let digits = |part: &str| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return None;
    }
    let (hour, minute) = (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?);
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

/// Parsed active hours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveWindow {
    start: u32,
    end: u32,
    /// Bit `n` is day `n` (0 is Sunday).
    days: u8,
    time_zone: Tz,
}

impl ActiveWindow {
    pub(crate) fn parse(hours: &ActiveHours) -> Result<Self, String> {
        let (Some(start), Some(end)) = (parse_clock(&hours.start), parse_clock(&hours.end)) else {
            return Err(ACTIVE_HOURS_TIME_INVALID.into());
        };
        if start == end {
            return Err(ACTIVE_HOURS_SAME_TIME.into());
        }
        if hours.days.is_empty() || hours.days.len() > 7 {
            return Err(ACTIVE_HOURS_DAYS_INVALID.into());
        }
        let mut days = 0u8;
        for day in &hours.days {
            if *day > 6 || days & (1 << day) != 0 {
                return Err(ACTIVE_HOURS_DAYS_INVALID.into());
            }
            days |= 1 << day;
        }
        Ok(Self {
            start,
            end,
            days,
            time_zone: parse_time_zone(&hours.time_zone)?,
        })
    }

    fn local(&self, at_ms: u64) -> Option<NaiveDateTime> {
        let at = Utc
            .timestamp_millis_opt(i64::try_from(at_ms).ok()?)
            .single()?;
        Some(at.with_timezone(&self.time_zone).naive_local())
    }

    fn has_day(&self, day: u32) -> bool {
        self.days & (1 << day) != 0
    }

    /// Whether `at_ms` is inside the window. An overnight window
    /// (`start > end`) belongs to the day it starts.
    pub(crate) fn contains(&self, at_ms: u64) -> bool {
        let Some(local) = self.local(at_ms) else {
            return false;
        };
        let minute = local.hour() * 60 + local.minute();
        let day = local.weekday().num_days_from_sunday();
        if self.start < self.end {
            self.has_day(day) && (self.start..self.end).contains(&minute)
        } else {
            let yesterday = (day + 6) % 7;
            (self.has_day(day) && minute >= self.start)
                || (self.has_day(yesterday) && minute < self.end)
        }
    }

    /// `at_ms` when it is inside the window, else the window's next opening.
    pub(crate) fn next_open(&self, at_ms: u64) -> Option<u64> {
        if self.contains(at_ms) {
            return Some(at_ms);
        }
        let local = self.local(at_ms)?;
        let mut date = local.date();
        for _ in 0..=8 {
            if self.has_day(date.weekday().num_days_from_sunday()) {
                let wall = date.and_hms_opt(self.start / 60, self.start % 60, 0)?;
                if let Some(open) = self.opening(wall) {
                    if open >= at_ms {
                        return Some(open);
                    }
                }
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// The instant the window opens at wall time `wall`; inside a
    /// daylight-saving gap, the first wall minute after the gap.
    fn opening(&self, wall: NaiveDateTime) -> Option<u64> {
        (0..=DST_GAP_SEARCH_MINUTES)
            .find_map(|shift| {
                let shifted = wall.checked_add_signed(TimeDelta::minutes(shift))?;
                match self.time_zone.from_local_datetime(&shifted) {
                    LocalResult::Single(at) => Some(at),
                    LocalResult::Ambiguous(first, second) => Some(first.min(second)),
                    LocalResult::None => None,
                }
            })
            .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
    }
}

/// `hours` checked, with its days in order: the stored form.
pub(crate) fn normalized_active_hours(hours: ActiveHours) -> Result<ActiveHours, String> {
    ActiveWindow::parse(&hours)?;
    let mut days = hours.days;
    days.sort_unstable();
    Ok(ActiveHours {
        start: hours.start,
        end: hours.end,
        days,
        time_zone: hours.time_zone.trim().to_string(),
    })
}

/// Every trigger variant by name, with no catch-all arm (spec §9.1): used on
/// creation, update, and restore.
pub(crate) fn validate_stored_trigger(trigger: &ScheduleTrigger) -> Result<(), String> {
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => {
            if *interval_ms == 0 || interval_ms % 1_000 != 0 {
                return Err(INTERVAL_INVALID.into());
            }
        }
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => {
            if *hour > 23 || *minute > 59 {
                return Err(DAILY_INVALID.into());
            }
            parse_time_zone(time_zone)?;
        }
        ScheduleTrigger::Cron {
            expression,
            time_zone,
        } => {
            parse_cron(expression)?;
            parse_time_zone(time_zone)?;
        }
        ScheduleTrigger::Once { at_ms } => {
            if *at_ms == 0 {
                return Err(ONCE_INVALID.into());
            }
        }
    }
    Ok(())
}

/// The next `hour:minute` in `time_zone` strictly after `after_ms`, by the
/// `daily` trigger's existing rule: a wall time a daylight-saving jump skips
/// moves to the next day, and a repeated one fires at its earlier instant.
fn next_daily_after(hour: u8, minute: u8, time_zone: &str, after_ms: u64) -> Result<u64, String> {
    let zone = parse_time_zone(time_zone)?;
    let after = Utc
        .timestamp_millis_opt(i64::try_from(after_ms).map_err(|_| TIMING_OVERFLOW.to_string())?)
        .single()
        .ok_or_else(|| TIMING_OVERFLOW.to_string())?;
    let local = after.with_timezone(&zone);
    for day_offset in 0..=2 {
        let date = local
            .date_naive()
            .checked_add_days(chrono::Days::new(day_offset))
            .ok_or_else(|| TIMING_OVERFLOW.to_string())?;
        let wall = date
            .and_hms_opt(u32::from(hour), u32::from(minute), 0)
            .ok_or_else(|| DAILY_INVALID.to_string())?;
        let candidate = match zone.from_local_datetime(&wall) {
            LocalResult::Single(at) => at,
            LocalResult::Ambiguous(first, second) => first.min(second),
            LocalResult::None => continue,
        };
        if let Ok(at_ms) = u64::try_from(candidate.timestamp_millis()) {
            if at_ms > after_ms {
                return Ok(at_ms);
            }
        }
    }
    Err(SCHEDULE_NEVER_RUNS.into())
}

/// The trigger's own next fire strictly after `after_ms`, ignoring windows.
fn raw_next(trigger: &ScheduleTrigger, after_ms: u64) -> Result<u64, String> {
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => after_ms
            .checked_add(*interval_ms)
            .ok_or_else(|| TIMING_OVERFLOW.to_string()),
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => next_daily_after(*hour, *minute, time_zone, after_ms),
        ScheduleTrigger::Cron {
            expression,
            time_zone,
        } => parse_cron(expression)?
            .next_after(parse_time_zone(time_zone)?, after_ms)
            .ok_or_else(|| SCHEDULE_NEVER_RUNS.to_string()),
        ScheduleTrigger::Once { at_ms } => {
            if *at_ms > after_ms {
                Ok(*at_ms)
            } else {
                Err(ONCE_NOT_IN_FUTURE.into())
            }
        }
    }
}

/// The first fire of `trigger` after `from_ms` inside `window`: an interval
/// counts from `from_ms` and waits for the window to open; a cron or daily
/// fire outside the window is skipped for its next one inside.
pub(crate) fn next_fire_after(
    trigger: &ScheduleTrigger,
    window: Option<&ActiveWindow>,
    from_ms: u64,
) -> Result<u64, String> {
    validate_stored_trigger(trigger)?;
    let Some(window) = window else {
        return raw_next(trigger, from_ms);
    };
    match trigger {
        ScheduleTrigger::Once { .. } => Err(ACTIVE_HOURS_NOT_FOR_ONCE.into()),
        ScheduleTrigger::Interval { .. } => window
            .next_open(raw_next(trigger, from_ms)?)
            .ok_or_else(|| SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string()),
        ScheduleTrigger::Daily { .. } | ScheduleTrigger::Cron { .. } => {
            let mut after = from_ms;
            for _ in 0..MAX_WINDOW_HOPS {
                let candidate = raw_next(trigger, after)?;
                if window.contains(candidate) {
                    return Ok(candidate);
                }
                let open = window
                    .next_open(candidate)
                    .ok_or_else(|| SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string())?;
                // The next fire at or after the opening.
                after = open.saturating_sub(1).max(candidate);
            }
            Err(SCHEDULE_NEVER_IN_ACTIVE_HOURS.into())
        }
    }
}

/// The due time once the occurrence due at `previous_due` was claimed at
/// `now_ms`: an interval keeps its cadence (missed steps are skipped, never
/// replayed) and waits for the window; cron and daily take their next fire
/// after `now_ms`; a `once` keeps its time (its claim turns it off).
pub(crate) fn next_fire_after_claim(
    trigger: &ScheduleTrigger,
    window: Option<&ActiveWindow>,
    previous_due: u64,
    now_ms: u64,
) -> Result<u64, String> {
    validate_stored_trigger(trigger)?;
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => {
            let steps = now_ms.saturating_sub(previous_due) / interval_ms + 1;
            let stepped = interval_ms
                .checked_mul(steps)
                .and_then(|span| previous_due.checked_add(span))
                .ok_or_else(|| TIMING_OVERFLOW.to_string())?;
            match window {
                None => Ok(stepped),
                Some(window) => window
                    .next_open(stepped)
                    .ok_or_else(|| SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string()),
            }
        }
        ScheduleTrigger::Once { .. } => Ok(previous_due),
        ScheduleTrigger::Daily { .. } | ScheduleTrigger::Cron { .. } => {
            next_fire_after(trigger, window, now_ms)
        }
    }
}

/// The next `count` fires after `from_ms` (one for `once`), as the scheduler
/// would claim them on time: for previews and the agent minimum.
pub(crate) fn upcoming_fires(
    trigger: &ScheduleTrigger,
    window: Option<&ActiveWindow>,
    from_ms: u64,
    count: usize,
) -> Result<Vec<u64>, String> {
    let mut fires = Vec::with_capacity(count);
    if count == 0 {
        return Ok(fires);
    }
    let mut at = next_fire_after(trigger, window, from_ms)?;
    loop {
        fires.push(at);
        if fires.len() == count || matches!(trigger, ScheduleTrigger::Once { .. }) {
            return Ok(fires);
        }
        at = next_fire_after(trigger, window, at)?;
    }
}
```

Right after the module doc comment of `timing.rs`, add `#![allow(dead_code)] // M6 Task 5 uses every item.` (`upcoming_fires`, `normalized_active_hours`, and `parse_clock` have no caller outside the tests until then). In `hosts/rust-daemon/src/schedules/cron.rs`, delete the line `#![allow(dead_code)] // M6 Task 2 uses every item.` (`timing.rs` now calls `parse_cron`, which uses the rest).

- [ ] **Step 4: Route `schedules.rs` through `timing`**

In `hosts/rust-daemon/src/schedules.rs`:

1. Add the variant to `ScheduleError`, after `Invalid(&'static str),`:

```text
    /// A trigger or active hours the daemon refuses, with a built message (400).
    Rejected(String),
```

2. Replace `next_due_at_ms`, `next_due_after_claim`, `next_daily_at_ms`, and `validate_trigger` (from `pub(crate) fn next_due_at_ms(` through the end of `next_daily_at_ms`, and the whole `fn validate_trigger`) with:

```rust
pub(crate) fn next_due_at_ms(
    trigger: &ScheduleTrigger,
    from_ms: u64,
) -> Result<u64, ScheduleError> {
    next_due(trigger, None, from_ms)
}

/// The first fire of `trigger` after `from_ms` inside `active_hours`.
pub(crate) fn next_due(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    from_ms: u64,
) -> Result<u64, ScheduleError> {
    let window = active_window(active_hours)?;
    timing::next_fire_after(trigger, window.as_ref(), from_ms).map_err(ScheduleError::Rejected)
}

/// The due time after the occurrence due at `previous_due` was claimed.
pub(crate) fn next_due_after_claim(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    previous_due: u64,
    now: u64,
) -> Result<u64, ScheduleError> {
    let window = active_window(active_hours)?;
    timing::next_fire_after_claim(trigger, window.as_ref(), previous_due, now)
        .map_err(ScheduleError::Rejected)
}

fn active_window(
    active_hours: Option<&ActiveHours>,
) -> Result<Option<timing::ActiveWindow>, ScheduleError> {
    active_hours
        .map(timing::ActiveWindow::parse)
        .transpose()
        .map_err(ScheduleError::Rejected)
}

fn validate_trigger(trigger: &ScheduleTrigger) -> Result<(), ScheduleError> {
    timing::validate_stored_trigger(trigger).map_err(ScheduleError::Rejected)
}
```

3. In `claim_due`, change the `next_due_after_claim` call to pass the active hours (none yet; Task 6 passes the record's):

```text
        claimed.next_due_at_ms =
            next_due_after_claim(&claimed.trigger, None, previous.next_due_at_ms, now)?;
```

4. Remove the imports the compiler now reports unused (`chrono::{LocalResult, Offset, TimeZone, Utc}` and `chrono_tz::Tz` are expected to go).

In `hosts/rust-daemon/src/routes/schedules.rs`'s `schedule_error`, after the `ScheduleError::Invalid(message)` arm, add:

```text
        ScheduleError::Rejected(message) => {
            error_response(StatusCode::BAD_REQUEST, "schedule_invalid", &message)
        }
```

In `hosts/rust-daemon/src/routes/contracts/schedules.rs`, add to `ScheduleTriggerResponse`, after `Daily { … }`:

```rust
    Cron {
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    Once {
        #[serde(rename = "atMs")]
        at_ms: u64,
    },
```

and to the trigger `match` in `From<ScheduledPromptRecord> for ScheduleResponse`:

```text
            ScheduleTrigger::Cron {
                expression,
                time_zone,
            } => ScheduleTriggerResponse::Cron {
                expression,
                time_zone,
            },
            ScheduleTrigger::Once { at_ms } => ScheduleTriggerResponse::Once { at_ms },
```

In `hosts/rust-daemon/src/state.rs`'s `validate_control_plane_snapshot`, replace the whole `match &schedule.trigger { … _ => {} }` block with:

```rust
            // Spec §9.1: every variant by name, no catch-all arm.
            crate::schedules::timing::validate_stored_trigger(&schedule.trigger).map_err(
                |problem| format!("schedule '{}' has an invalid trigger: {problem}", schedule.id),
            )?;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules state:: routes::tests 2>&1 | tail -30`
Expected: PASS (13 new timing tests, the new restore test, every existing schedule, state, and route test).

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test schedule_api 2>&1 | tail -30`
Expected: PASS (unchanged behavior for `interval` and `daily`).

Run: `grep -n "_ => {}" hosts/rust-daemon/src/state.rs | head; grep -n "anima_schedule" hosts/rust-daemon/src/schedules.rs`
Expected: no `_ => {}` line inside the schedule loop of `validate_control_plane_snapshot` (others elsewhere in the file are unrelated), and no `anima_schedule` match.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/schedules/cron.rs hosts/rust-daemon/src/schedules/timing.rs hosts/rust-daemon/src/routes/schedules.rs hosts/rust-daemon/src/routes/contracts/schedules.rs hosts/rust-daemon/src/state.rs
git commit -m "feat(daemon): add cron and once triggers with active hours and explicit trigger validation"
```

Recommended implementer tier: standard (complete code; the window tests pin the rules).

---

### Task 3: Automation fields, the fire log, restore validation, `automation.updated`, and snapshot version 9

**Files:**

- Create: `hosts/rust-daemon/src/schedules/automations.rs`, `hosts/rust-daemon/src/schedules/history.rs`, `hosts/rust-daemon/src/state/automation_state.rs`
- Modify: `hosts/rust-daemon/src/schedules.rs` (module lines, re-exports, record fields, `ScheduleLastFired.manual`, the record literal in `create`, the `ScheduleLastFired` literal in `claim_due`), `hosts/rust-daemon/src/state.rs` (field, snapshot, restore, validation, the `test_schedule` literal, three version assertions), `hosts/rust-daemon/src/control_plane_store.rs` (field, version 9, backup path, tests), `hosts/rust-daemon/src/app/persistence.rs` (tests), `hosts/rust-daemon/src/approvals/registry.rs`, `hosts/rust-daemon/src/skills/registry.rs`, `hosts/rust-daemon/src/agent_runs/live_tests.rs` (version assertions), `hosts/rust-daemon/src/live/events.rs`, `hosts/rust-daemon/src/live/tests.rs`, `hosts/rust-daemon/src/connectors/runtime.rs` (five field lines in the one `ScheduledPromptRecord` test literal), `hosts/rust-daemon/README.md` (rollback note)

**Interfaces:**

- Consumes: Task 2's `ActiveHours`, `ActiveWindow`, `ACTIVE_HOURS_NOT_FOR_ONCE`; `ControlPlaneSnapshot`, `DaemonState::{control_plane_snapshot, restore_control_plane_snapshot, validate_control_plane_snapshot}`, `pre_upgrade_backup_path`, `postgres_backup_key`; `LiveEvent`, `LiveEventBody`.
- Produces:
  - `ScheduledPromptRecord` gains `name: String`, `active_hours: Option<ActiveHours>`, `created_by: AutomationCreator`, `preset: Option<AutomationPreset>`, `counters: AutomationCounters` (all `#[serde(default)]`); `ScheduleLastFired` gains `manual: bool` (`#[serde(default)]`).
  - `schedules::automations::{MAX_AUTOMATION_NAME_CHARS = 80, AutomationCreator { Owner, Agent { agent_id, session_id, run_id, tool_call_id } }, AutomationPreset { Heartbeat } (as_str), AutomationCounters { runs, failures, consecutive_failures } (record(&ScheduleOutcomeStatus)), default_name(prompt) -> String, display_name(&ScheduledPromptRecord) -> String, validate_stored_automation(&ScheduledPromptRecord) -> Result<(), String>}`, and `#[cfg(test)] test_automation(agent_id, id) -> ScheduledPromptRecord` (an enabled 60-second workspace interval automation, never fired, owner-created).
  - `schedules::history::{MAX_UNMIRRORED_FIRES = 1_000, ScheduleFireRecord { id, schedule_id, agent_id, fired_at_ms, finished_at_ms, outcome: ScheduleOutcomeStatus, run_id, session_id, error_code, manual }, FireLog}` with `FireLog::{record(fire) -> Option<ScheduleFireRecord>, remove(id), for_schedule(schedule_id) -> Vec<ScheduleFireRecord>, retain_agents(&HashSet<String>) -> usize, unmirrored(limit) -> Vec<_>, mark_mirrored(&[_]) -> usize, snapshot() -> Vec<_>, restored(Vec<_>) -> FireLog, validate(&[_]) -> Result<(), String>, len()}`.
  - `DaemonState.schedule_fires: FireLog`; `ControlPlaneSnapshot.schedule_fires` (`scheduleFires` in JSON).
  - `DaemonState::{publish_automation_updated(agent_id, schedule_id, deleted), unmirrored_schedule_fires(limit) -> Vec<ScheduleFireRecord>, record_automation_outcome(schedule_id, ScheduleSafeOutcome, run: Option<(run_id, session_id)>, finished_at_ms) -> Option<OutcomeUndo>, undo_automation_outcome(OutcomeUndo)}` (`state/automation_state.rs`).
  - `LiveEventBody::AutomationUpdated { schedule_id, deleted }` (`automation.updated`, JSON `scheduleId`, `deleted`).
  - `control_plane_store::{CONTROL_PLANE_STORE_VERSION = 9, SKILLS_STORE_VERSION = 8, PRE_AUTOMATIONS_BACKUP_SUFFIX = ".pre-automations.bak", pre_automations_backup_path}`.
- Behavior: an M5 (version-8) snapshot loads with default automation fields after `<file>.pre-automations.bak` (or `control_plane.backup.8`) is written; earlier backups are never overwritten. A snapshot with a bad name (over 80 characters or holding a control character), invalid active hours (or active hours on a `once`), an agent creator with an empty field, inconsistent counters, or a duplicate or malformed fire record refuses to load. Recording an outcome adds to the counters and stores a fire record whose id is the occurrence's run idempotency key; its undo puts back exactly the outcome, the counters, and the fire log's entry.

- [ ] **Step 1: Write the failing tests**

Add to `hosts/rust-daemon/src/schedules.rs`, after `pub(crate) use timing::ActiveHours;`:

```text
pub(crate) mod automations;
pub(crate) mod history;
#[cfg(test)]
pub(crate) use automations::test_automation;
pub(crate) use automations::{
    validate_stored_automation, AutomationCounters, AutomationCreator, AutomationPreset,
};
pub(crate) use history::{FireLog, ScheduleFireRecord};
```

Create `hosts/rust-daemon/src/schedules/automations.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedules::{ActiveHours, ScheduleTrigger};

    #[test]
    fn creators_presets_and_counters_serialize_in_camel_case() {
        assert_eq!(MAX_AUTOMATION_NAME_CHARS, 80);
        assert_eq!(
            serde_json::to_value(AutomationCreator::Owner).unwrap(),
            serde_json::json!({"kind": "owner"})
        );
        assert_eq!(
            serde_json::to_value(AutomationCreator::Agent {
                agent_id: "agent-1".into(),
                session_id: "chat:1".into(),
                run_id: "run_1".into(),
                tool_call_id: "call-1".into(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "agent",
                "agentId": "agent-1",
                "sessionId": "chat:1",
                "runId": "run_1",
                "toolCallId": "call-1"
            })
        );
        assert_eq!(
            serde_json::to_value(AutomationPreset::Heartbeat).unwrap(),
            "heartbeat"
        );
        assert_eq!(AutomationPreset::Heartbeat.as_str(), "heartbeat");
        assert_eq!(
            serde_json::to_value(AutomationCounters {
                runs: 3,
                failures: 2,
                consecutive_failures: 1,
            })
            .unwrap(),
            serde_json::json!({"runs": 3, "failures": 2, "consecutiveFailures": 1})
        );
        let legacy: ScheduledPromptRecord = serde_json::from_value({
            let mut value = serde_json::to_value(test_automation("agent-1", "s1")).unwrap();
            let object = value.as_object_mut().unwrap();
            for key in ["name", "activeHours", "createdBy", "preset", "counters"] {
                object.remove(key);
            }
            value
        })
        .unwrap();
        assert_eq!(legacy.name, "");
        assert_eq!(legacy.created_by, AutomationCreator::Owner);
        assert_eq!(legacy.counters, AutomationCounters::default());
    }

    #[test]
    fn counters_follow_each_outcome() {
        let mut counters = AutomationCounters::default();
        counters.record(&ScheduleOutcomeStatus::Failed);
        counters.record(&ScheduleOutcomeStatus::Failed);
        assert_eq!(
            counters,
            AutomationCounters {
                runs: 2,
                failures: 2,
                consecutive_failures: 2
            }
        );
        counters.record(&ScheduleOutcomeStatus::Stopped);
        assert_eq!(counters.consecutive_failures, 2, "a stop is not a success");
        counters.record(&ScheduleOutcomeStatus::Silent);
        assert_eq!(
            counters,
            AutomationCounters {
                runs: 4,
                failures: 2,
                consecutive_failures: 0
            }
        );
        counters.record(&ScheduleOutcomeStatus::Spoke);
        assert_eq!(counters.runs, 5);
    }

    #[test]
    fn a_missing_name_comes_from_the_prompts_first_line() {
        assert_eq!(default_name("\n  Check the inbox \nthen more"), "Check the inbox");
        assert_eq!(default_name(&"x".repeat(200)).chars().count(), 80);
        assert_eq!(default_name(" \n "), "Automation");
        let mut record = test_automation("agent-1", "s1");
        record.name = String::new();
        record.prompt = "Water the plants".into();
        assert_eq!(display_name(&record), "Water the plants");
        record.name = "Plants".into();
        assert_eq!(display_name(&record), "Plants");
    }

    #[test]
    fn stored_automation_fields_are_validated() {
        let good = test_automation("agent-1", "s1");
        assert_eq!(validate_stored_automation(&good), Ok(()));
        let mut long_name = good.clone();
        long_name.name = "x".repeat(81);
        let mut control_name = good.clone();
        control_name.name = "two\nlines".into();
        let mut bad_hours = good.clone();
        bad_hours.active_hours = Some(ActiveHours {
            start: "08:00".into(),
            end: "08:00".into(),
            days: vec![1],
            time_zone: "UTC".into(),
        });
        let mut once_hours = good.clone();
        once_hours.trigger = ScheduleTrigger::Once { at_ms: 5 };
        once_hours.active_hours = Some(ActiveHours {
            start: "08:00".into(),
            end: "22:00".into(),
            days: vec![1],
            time_zone: "UTC".into(),
        });
        let mut empty_creator = good.clone();
        empty_creator.created_by = AutomationCreator::Agent {
            agent_id: "agent-1".into(),
            session_id: " ".into(),
            run_id: "run_1".into(),
            tool_call_id: "call-1".into(),
        };
        let mut counters = good.clone();
        counters.counters = AutomationCounters {
            runs: 1,
            failures: 2,
            consecutive_failures: 0,
        };
        for bad in [
            long_name,
            control_name,
            bad_hours,
            once_hours,
            empty_creator,
            counters,
        ] {
            assert!(validate_stored_automation(&bad).is_err(), "{bad:?}");
        }
    }
}
```

Create `hosts/rust-daemon/src/schedules/history.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn fire(id: &str, agent_id: &str, fired_at_ms: u64) -> ScheduleFireRecord {
        ScheduleFireRecord {
            id: id.into(),
            schedule_id: "s1".into(),
            agent_id: agent_id.into(),
            fired_at_ms,
            finished_at_ms: fired_at_ms + 5,
            outcome: ScheduleOutcomeStatus::Spoke,
            run_id: Some("run_1".into()),
            session_id: Some("schedule:s1".into()),
            error_code: None,
            manual: false,
        }
    }

    #[test]
    fn fire_records_serialize_in_camel_case() {
        assert_eq!(MAX_UNMIRRORED_FIRES, 1_000);
        assert_eq!(
            serde_json::to_value(fire("schedule:s1:10", "agent-1", 10)).unwrap(),
            serde_json::json!({
                "id": "schedule:s1:10",
                "scheduleId": "s1",
                "agentId": "agent-1",
                "firedAtMs": 10,
                "finishedAtMs": 15,
                "outcome": "spoke",
                "runId": "run_1",
                "sessionId": "schedule:s1",
                "errorCode": null,
                "manual": false
            })
        );
    }

    #[test]
    fn the_log_records_replaces_removes_and_caps() {
        let mut log = FireLog::default();
        assert_eq!(log.record(fire("a", "agent-1", 10)), None);
        let mut changed = fire("a", "agent-1", 10);
        changed.outcome = ScheduleOutcomeStatus::Failed;
        assert_eq!(log.record(changed.clone()), Some(fire("a", "agent-1", 10)));
        assert_eq!(log.len(), 1);
        assert_eq!(log.for_schedule("s1"), vec![changed]);
        assert!(log.for_schedule("other").is_empty());
        assert!(log.remove("a").is_some());
        assert_eq!(log.len(), 0);

        for n in 0..=MAX_UNMIRRORED_FIRES as u64 {
            log.record(fire(&format!("f{n}"), "agent-1", n + 1));
        }
        assert_eq!(log.len(), MAX_UNMIRRORED_FIRES);
        assert!(
            log.snapshot().iter().all(|kept| kept.id != "f0"),
            "the oldest leaves first"
        );
    }

    #[test]
    fn mirroring_removes_only_unchanged_fires_and_orphans_go() {
        let mut log = FireLog::default();
        log.record(fire("a", "agent-1", 10));
        log.record(fire("b", "agent-1", 20));
        log.record(fire("c", "agent-gone", 30));
        let written = log.unmirrored(2);
        assert_eq!(
            written.iter().map(|fire| fire.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"],
            "oldest first"
        );
        let mut rewritten = fire("b", "agent-1", 20);
        rewritten.outcome = ScheduleOutcomeStatus::Stopped;
        log.record(rewritten);
        assert_eq!(log.mark_mirrored(&written), 1, "b changed since it was read");
        assert_eq!(log.len(), 2);

        let live = HashSet::from(["agent-1".to_string()]);
        assert_eq!(log.retain_agents(&live), 1);
        assert_eq!(
            log.snapshot().iter().map(|fire| fire.id.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
    }

    #[test]
    fn restored_logs_are_validated_and_capped() {
        assert_eq!(FireLog::validate(&[fire("a", "agent-1", 10)]), Ok(()));
        let mut finished_early = fire("b", "agent-1", 10);
        finished_early.finished_at_ms = 9;
        let mut unnamed = fire("c", "agent-1", 10);
        unnamed.schedule_id = String::new();
        for bad in [
            vec![fire("a", "agent-1", 10), fire("a", "agent-1", 11)],
            vec![fire(" ", "agent-1", 10)],
            vec![fire("z", "agent-1", 0)],
            vec![finished_early],
            vec![unnamed],
        ] {
            assert!(FireLog::validate(&bad).is_err(), "{bad:?}");
        }
        let many = (0..MAX_UNMIRRORED_FIRES as u64 + 5)
            .map(|n| fire(&format!("f{n}"), "agent-1", n + 1))
            .collect::<Vec<_>>();
        assert_eq!(FireLog::restored(many).len(), MAX_UNMIRRORED_FIRES);
    }
}
```

Create `hosts/rust-daemon/src/state/automation_state.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use crate::agent_runs::test_support::{companion_config, next_event};
    use crate::schedules::{
        test_automation, ActiveHours, AutomationCounters, AutomationCreator, AutomationPreset,
        ScheduleFireRecord, ScheduleLastFired, ScheduleOutcomeStatus, ScheduleSafeOutcome,
        ScheduleTrigger,
    };
    use crate::state::DaemonState;

    fn claimed(agent_id: &str) -> crate::schedules::ScheduledPromptRecord {
        let mut record = test_automation(agent_id, "s1");
        record.last_fired = Some(ScheduleLastFired {
            fired_at_ms: 10_000,
            run_idempotency_key: "schedule:s1:10000".into(),
            manual: true,
        });
        record.updated_at_ms = 10_000;
        record
    }

    fn outcome(status: ScheduleOutcomeStatus) -> ScheduleSafeOutcome {
        ScheduleSafeOutcome {
            error_code: crate::schedules::checkin_error_code(&status),
            status,
            occurred_at_ms: 10_000,
        }
    }

    #[test]
    fn an_outcome_sets_the_counters_and_a_fire_and_its_undo_puts_them_back() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let before = claimed(&agent);
        state.schedules.insert("s1".into(), before.clone());

        let undo = state
            .record_automation_outcome(
                "s1",
                outcome(ScheduleOutcomeStatus::Failed),
                Some(("run_1".into(), "schedule:s1".into())),
                12_000,
            )
            .expect("the automation exists");

        let after = &state.schedules["s1"];
        assert_eq!(after.counters.failures, 1);
        assert_eq!(after.counters.consecutive_failures, 1);
        assert_eq!(
            after.last_safe_outcome.as_ref().map(|outcome| outcome.status.clone()),
            Some(ScheduleOutcomeStatus::Failed)
        );
        assert_eq!(
            state.schedule_fires.snapshot(),
            vec![ScheduleFireRecord {
                id: "schedule:s1:10000".into(),
                schedule_id: "s1".into(),
                agent_id: agent.clone(),
                fired_at_ms: 10_000,
                finished_at_ms: 12_000,
                outcome: ScheduleOutcomeStatus::Failed,
                run_id: Some("run_1".into()),
                session_id: Some("schedule:s1".into()),
                error_code: Some("schedule_run_failed".into()),
                manual: true,
            }]
        );

        state.undo_automation_outcome(undo);

        let reverted = &state.schedules["s1"];
        assert_eq!(reverted.counters, before.counters);
        assert_eq!(reverted.last_safe_outcome, None);
        assert_eq!(state.schedule_fires.len(), 0);
        assert!(state
            .record_automation_outcome("missing", outcome(ScheduleOutcomeStatus::Spoke), None, 1)
            .is_none());
    }

    #[tokio::test]
    async fn automation_updates_reach_the_automations_agent() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let mut stream = state.live.subscribe(&agent).unwrap();

        state.publish_automation_updated(&agent, "s1", true);

        let event = next_event(&mut stream).await.to_json(1);
        assert_eq!(event["type"], "automation.updated");
        assert_eq!(event["agentId"], agent.as_str());
        assert_eq!(event["scheduleId"], "s1");
        assert_eq!(event["deleted"], true);
        assert!(event.get("sessionId").is_none());
    }

    #[test]
    fn unmirrored_fires_skip_deleted_agents() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let mut live_fire = crate::schedules::history::tests_support_fire(&agent);
        live_fire.id = "kept".into();
        let mut orphan = live_fire.clone();
        orphan.id = "gone".into();
        orphan.agent_id = "agent-deleted".into();
        state.schedule_fires.record(live_fire.clone());
        state.schedule_fires.record(orphan);

        assert_eq!(state.unmirrored_schedule_fires(10), vec![live_fire]);
        assert_eq!(state.schedule_fires.len(), 1, "the orphan is dropped, not written");
    }

    #[test]
    fn automation_fields_and_fires_round_trip_through_a_snapshot() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let mut record = test_automation(&agent, "s1");
        record.name = "Morning brief".into();
        record.trigger = ScheduleTrigger::Cron {
            expression: "0 9 * * 1-5".into(),
            time_zone: "Europe/London".into(),
        };
        record.active_hours = Some(ActiveHours {
            start: "08:00".into(),
            end: "22:00".into(),
            days: vec![1, 2, 3, 4, 5],
            time_zone: "Europe/London".into(),
        });
        record.created_by = AutomationCreator::Agent {
            agent_id: agent.clone(),
            session_id: "chat:1".into(),
            run_id: "run_1".into(),
            tool_call_id: "call-1".into(),
        };
        record.preset = Some(AutomationPreset::Heartbeat);
        record.counters = AutomationCounters {
            runs: 3,
            failures: 1,
            consecutive_failures: 1,
        };
        state.schedules.insert("s1".into(), record.clone());
        let mut fire = crate::schedules::history::tests_support_fire(&agent);
        fire.id = "schedule:s1:10".into();
        state.schedule_fires.record(fire.clone());

        let snapshot = state.control_plane_snapshot();
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["schedules"][0]["createdBy"]["kind"], "agent");
        assert_eq!(json["schedules"][0]["trigger"]["cron"]["timeZone"], "Europe/London");
        assert_eq!(json["scheduleFires"][0]["id"], "schedule:s1:10");
        let mut restored = DaemonState::new();
        restored.restore_control_plane_snapshot(snapshot).unwrap();
        assert_eq!(restored.schedules["s1"], record);
        assert_eq!(restored.schedule_fires.snapshot(), vec![fire]);
    }

    #[test]
    fn restore_refuses_invalid_automation_fields_and_fires() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        state
            .schedules
            .insert("s1".into(), test_automation(&agent, "s1"));
        let good = state.control_plane_snapshot();

        let mut long_name = good.clone();
        long_name.schedules[0].name = "x".repeat(81);
        let mut bad_counters = good.clone();
        bad_counters.schedules[0].counters.failures = 5;
        let mut duplicate_fires = good.clone();
        let fire = crate::schedules::history::tests_support_fire(&agent);
        duplicate_fires.schedule_fires = vec![fire.clone(), fire];
        for bad in [long_name, bad_counters, duplicate_fires] {
            assert!(DaemonState::new().restore_control_plane_snapshot(bad).is_err());
        }
        assert!(DaemonState::new().restore_control_plane_snapshot(good).is_ok());
    }
}
```

The fire builder these tests share lives beside `FireLog`. Add to `hosts/rust-daemon/src/schedules/history.rs` (outside its test module):

```rust
/// A finished fire of automation `s1`, for tests elsewhere in the crate.
#[cfg(test)]
pub(crate) fn tests_support_fire(agent_id: &str) -> ScheduleFireRecord {
    ScheduleFireRecord {
        id: "schedule:s1:10".into(),
        schedule_id: "s1".into(),
        agent_id: agent_id.into(),
        fired_at_ms: 10,
        finished_at_ms: 15,
        outcome: ScheduleOutcomeStatus::Spoke,
        run_id: Some("run_1".into()),
        session_id: Some("schedule:s1".into()),
        error_code: None,
        manual: false,
    }
}
```

Add to `hosts/rust-daemon/src/state.rs`, after `mod approval_state;`:

```text
mod automation_state;
```

and, after the module lines:

```text
#[allow(unused_imports)] // M6 Task 6's scheduler names it.
pub(crate) use automation_state::OutcomeUndo;
```

Add to `hosts/rust-daemon/src/live/tests.rs`, after `skill_updated_names_the_skill_and_the_draft`:

```rust
#[test]
fn automation_updated_names_the_schedule() {
    let event = LiveEvent::new(
        "agent-1",
        LiveEventBody::AutomationUpdated {
            schedule_id: "schedule-1".into(),
            deleted: false,
        },
    )
    .to_json(6);
    assert_eq!(event["type"], "automation.updated");
    assert_eq!(event["scheduleId"], "schedule-1");
    assert_eq!(event["deleted"], false);
    assert_eq!(event["seq"], 6);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::automations schedules::history state::automation_state live::tests 2>&1 | tail -30`
Expected: FAIL to compile (the types, the fire log, and the new event do not exist).

- [ ] **Step 3: Write `automations.rs` and `history.rs`**

Put this above the test module in `hosts/rust-daemon/src/schedules/automations.rs`:

```rust
//! Automations (spec §9): what M6 adds to a scheduled prompt (a name,
//! active hours, who made it, a preset, and counters). Task 5 adds the
//! limits, the strings, the heartbeat preset, and `AutomationService`.

use serde::{Deserialize, Serialize};

use super::timing::{ActiveWindow, ACTIVE_HOURS_NOT_FOR_ONCE};
use super::{ScheduleOutcomeStatus, ScheduleTrigger, ScheduledPromptRecord};

/// The longest name, in characters (spec §9.1).
pub(crate) const MAX_AUTOMATION_NAME_CHARS: usize = 80;

/// Who made an automation (spec §9.1). A companion's carries its tool
/// call, so the web can show the notice card beside it (spec §15.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum AutomationCreator {
    #[default]
    Owner,
    #[serde(rename_all = "camelCase")]
    Agent {
        agent_id: String,
        session_id: String,
        run_id: String,
        tool_call_id: String,
    },
}

/// A preset the automation was made from; a label that survives edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutomationPreset {
    Heartbeat,
}

impl AutomationPreset {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Heartbeat => "heartbeat",
        }
    }
}

/// How an automation's occurrences ended (spec §9.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationCounters {
    pub(crate) runs: u64,
    pub(crate) failures: u64,
    pub(crate) consecutive_failures: u64,
}

impl AutomationCounters {
    /// Counts one recorded outcome: a failure adds to both failure counts,
    /// a silent or spoken reply ends a failure streak, a stop changes neither.
    pub(crate) fn record(&mut self, status: &ScheduleOutcomeStatus) {
        self.runs = self.runs.saturating_add(1);
        match status {
            ScheduleOutcomeStatus::Failed => {
                self.failures = self.failures.saturating_add(1);
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            }
            ScheduleOutcomeStatus::Silent | ScheduleOutcomeStatus::Spoke => {
                self.consecutive_failures = 0;
            }
            ScheduleOutcomeStatus::Stopped => {}
        }
    }
}

/// A name from the prompt's first non-blank line, at most 80 characters.
pub(crate) fn default_name(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Automation");
    line.chars()
        .filter(|character| !character.is_control())
        .take(MAX_AUTOMATION_NAME_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The name clients show: the stored one, or the prompt's for records saved
/// before M6.
pub(crate) fn display_name(record: &ScheduledPromptRecord) -> String {
    if record.name.trim().is_empty() {
        default_name(&record.prompt)
    } else {
        record.name.clone()
    }
}

/// Restore validation of the M6 fields (the trigger has its own).
pub(crate) fn validate_stored_automation(record: &ScheduledPromptRecord) -> Result<(), String> {
    if record.name.chars().count() > MAX_AUTOMATION_NAME_CHARS
        || record.name.chars().any(char::is_control)
    {
        return Err("has an invalid name".into());
    }
    if let Some(hours) = &record.active_hours {
        if matches!(record.trigger, ScheduleTrigger::Once { .. }) {
            return Err(format!(
                "has invalid active hours: {ACTIVE_HOURS_NOT_FOR_ONCE}"
            ));
        }
        ActiveWindow::parse(hours)
            .map_err(|problem| format!("has invalid active hours: {problem}"))?;
    }
    if let AutomationCreator::Agent {
        agent_id,
        session_id,
        run_id,
        tool_call_id,
    } = &record.created_by
    {
        if [agent_id, session_id, run_id, tool_call_id]
            .iter()
            .any(|value| value.trim().is_empty())
        {
            return Err("has an invalid creator".into());
        }
    }
    let counters = &record.counters;
    if counters.failures > counters.runs || counters.consecutive_failures > counters.failures {
        return Err("has inconsistent counters".into());
    }
    Ok(())
}

/// An enabled, never-fired, owner-made workspace automation repeating every
/// 60 seconds, for tests.
#[cfg(test)]
pub(crate) fn test_automation(agent_id: &str, id: &str) -> ScheduledPromptRecord {
    ScheduledPromptRecord {
        id: id.into(),
        import_idempotency_key: None,
        agent_id: agent_id.into(),
        prompt: "Check status".into(),
        trigger: ScheduleTrigger::Interval {
            interval_ms: 60_000,
        },
        enabled: true,
        target: super::ScheduleTarget::Workspace,
        next_due_at_ms: 70_000,
        last_fired: None,
        last_safe_outcome: None,
        created_at_ms: 10_000,
        updated_at_ms: 10_000,
        name: "Check status".into(),
        active_hours: None,
        created_by: AutomationCreator::Owner,
        preset: None,
        counters: AutomationCounters::default(),
    }
}
```

Put this above the test module in `hosts/rust-daemon/src/schedules/history.rs` (the `tests_support_fire` builder from Step 1 goes after it):

```rust
//! Automation fire history (spec §9.1): one record per occurrence, saved
//! with its outcome and kept in the control plane until the history store
//! holds it (spec §13.1).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::ScheduleOutcomeStatus;

/// Fire records the control plane keeps while the history store fails; past
/// it the oldest leave with a warning.
pub(crate) const MAX_UNMIRRORED_FIRES: usize = 1_000;

/// One occurrence of an automation (spec §9.1). `id` is the occurrence's run
/// idempotency key, so writing it twice is harmless.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleFireRecord {
    pub(crate) id: String,
    pub(crate) schedule_id: String,
    pub(crate) agent_id: String,
    pub(crate) fired_at_ms: u64,
    pub(crate) finished_at_ms: u64,
    pub(crate) outcome: ScheduleOutcomeStatus,
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) error_code: Option<String>,
    #[serde(default)]
    pub(crate) manual: bool,
}

/// Fire records the history store does not hold yet, oldest first.
#[derive(Clone, Debug, Default)]
pub(crate) struct FireLog {
    pending: Vec<ScheduleFireRecord>,
}

impl FireLog {
    /// Adds `fire`, replacing one with its id (returned); past the cap the
    /// oldest leaves.
    pub(crate) fn record(&mut self, fire: ScheduleFireRecord) -> Option<ScheduleFireRecord> {
        if let Some(existing) = self.pending.iter_mut().find(|known| known.id == fire.id) {
            return Some(std::mem::replace(existing, fire));
        }
        self.pending.push(fire);
        if self.pending.len() > MAX_UNMIRRORED_FIRES {
            let dropped = self.pending.remove(0);
            warn!(
                fire_id = %dropped.id,
                "dropped the oldest automation fire record: the history store has not taken any for a while"
            );
        }
        None
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<ScheduleFireRecord> {
        let index = self.pending.iter().position(|fire| fire.id == id)?;
        Some(self.pending.remove(index))
    }

    pub(crate) fn for_schedule(&self, schedule_id: &str) -> Vec<ScheduleFireRecord> {
        self.pending
            .iter()
            .filter(|fire| fire.schedule_id == schedule_id)
            .cloned()
            .collect()
    }

    /// Drops the fires of agents not in `live`; returns how many.
    pub(crate) fn retain_agents(&mut self, live: &HashSet<String>) -> usize {
        let before = self.pending.len();
        self.pending.retain(|fire| live.contains(&fire.agent_id));
        before - self.pending.len()
    }

    pub(crate) fn unmirrored(&self, limit: usize) -> Vec<ScheduleFireRecord> {
        self.pending.iter().take(limit).cloned().collect()
    }

    /// Removes each written fire still exactly as written; returns how many.
    pub(crate) fn mark_mirrored(&mut self, written: &[ScheduleFireRecord]) -> usize {
        let before = self.pending.len();
        self.pending.retain(|fire| !written.contains(fire));
        before - self.pending.len()
    }

    pub(crate) fn snapshot(&self) -> Vec<ScheduleFireRecord> {
        self.pending.clone()
    }

    /// A restored log keeps the newest `MAX_UNMIRRORED_FIRES`.
    pub(crate) fn restored(mut fires: Vec<ScheduleFireRecord>) -> Self {
        if fires.len() > MAX_UNMIRRORED_FIRES {
            fires.drain(..fires.len() - MAX_UNMIRRORED_FIRES);
        }
        Self { pending: fires }
    }

    pub(crate) fn validate(fires: &[ScheduleFireRecord]) -> Result<(), String> {
        let mut ids = HashSet::new();
        for fire in fires {
            if fire.id.trim().is_empty() || !ids.insert(fire.id.as_str()) {
                return Err(format!(
                    "duplicate or empty schedule fire id in snapshot: {}",
                    fire.id
                ));
            }
            if fire.schedule_id.trim().is_empty()
                || fire.agent_id.trim().is_empty()
                || fire.fired_at_ms == 0
                || fire.finished_at_ms < fire.fired_at_ms
            {
                return Err(format!("schedule fire '{}' is invalid", fire.id));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }
}
```

Each of the three new files starts with its module doc comment followed by a temporary allowance, so no intermediate commit warns about items later tasks call: `schedules/automations.rs` gets `#![allow(dead_code)] // M6 Task 8 uses every item.`; `schedules/history.rs` and `state/automation_state.rs` get `#![allow(dead_code)] // M6 Task 7 uses every item.` (Tasks 7 and 8 delete them; Task 13 checks).

- [ ] **Step 4: Add the fields, the state helpers, and the event**

In `hosts/rust-daemon/src/schedules.rs`:

1. Add after `pub(crate) updated_at_ms: u64,` in `ScheduledPromptRecord`:

```rust
    /// Spec §9.1. Empty for records saved before M6 (`display_name` reads the
    /// prompt's first line for those).
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) active_hours: Option<ActiveHours>,
    #[serde(default)]
    pub(crate) created_by: AutomationCreator,
    #[serde(default)]
    pub(crate) preset: Option<AutomationPreset>,
    #[serde(default)]
    pub(crate) counters: AutomationCounters,
```

2. Add after `pub(crate) run_idempotency_key: String,` in `ScheduleLastFired`:

```rust
    /// Run now (spec §9.2) fired it, not the trigger.
    #[serde(default)]
    pub(crate) manual: bool,
```

3. In `create`, add to the `ScheduledPromptRecord { … }` literal after `updated_at_ms: now.max(created_at_ms),` (Task 5 replaces `create` with the service; until then owner creates get these defaults):

```text
                name: automations::default_name(&prompt),
                active_hours: None,
                created_by: AutomationCreator::Owner,
                preset: None,
                counters: AutomationCounters::default(),
```

The `prompt` is moved into the literal above these lines; compute `let name = automations::default_name(&prompt);` before the literal and write `name,` instead if the borrow checker objects.

4. In `claim_due`, add `manual: false,` to the `ScheduleLastFired { … }` literal.

In `hosts/rust-daemon/src/connectors/runtime.rs`'s test `deletion_archives_completed_history_purges_pending_work_and_disables_schedules`, add to the one `ScheduledPromptRecord { … }` literal after `updated_at_ms: now,`:

```text
                    name: "check in".into(),
                    active_hours: None,
                    created_by: crate::schedules::AutomationCreator::Owner,
                    preset: None,
                    counters: Default::default(),
```

In `hosts/rust-daemon/src/state.rs`'s test helper `test_schedule`, add `manual: false,` to its `ScheduleLastFired { … }` and, after `updated_at_ms: 14,`:

```text
            name: "Review the workspace".into(),
            active_hours: None,
            created_by: crate::schedules::AutomationCreator::Owner,
            preset: None,
            counters: crate::schedules::AutomationCounters {
                runs: 1,
                failures: 0,
                consecutive_failures: 0,
            },
```

Put this above the test module in `hosts/rust-daemon/src/state/automation_state.rs`:

```rust
//! Automations in the daemon state (spec §9): announcing changes, recording
//! an occurrence's outcome with its counters and fire record (and undoing
//! that when the commit's save fails), and handing fires to the outbox.

use super::DaemonState;
use crate::live::{LiveEvent, LiveEventBody};
use crate::schedules::{ScheduleFireRecord, ScheduleSafeOutcome, ScheduledPromptRecord};

/// What `record_automation_outcome` changed, so a rollback puts it back.
#[derive(Clone, Debug)]
pub(crate) struct OutcomeUndo {
    previous: ScheduledPromptRecord,
    fire_id: Option<String>,
    previous_fire: Option<ScheduleFireRecord>,
}

impl DaemonState {
    /// Publishes `automation.updated` on the automation's agent's stream.
    /// Call it only after the change was saved.
    pub(crate) fn publish_automation_updated(&self, agent_id: &str, schedule_id: &str, deleted: bool) {
        self.live.publish(
            LiveEvent::new(
                agent_id,
                LiveEventBody::AutomationUpdated {
                    schedule_id: schedule_id.to_string(),
                    deleted,
                },
            ),
            None,
        );
    }

    /// Fire records for the outbox, oldest first. Those of deleted agents
    /// are dropped instead: their history went with the agent.
    pub(crate) fn unmirrored_schedule_fires(&mut self, limit: usize) -> Vec<ScheduleFireRecord> {
        let live = self.live_agent_ids();
        self.schedule_fires.retain_agents(&live);
        self.schedule_fires.unmirrored(limit)
    }

    /// Records the outcome of `schedule_id`'s current occurrence: the outcome,
    /// the counters, and a fire record keyed by the occurrence's run
    /// idempotency key. `run` is the occurrence's `(run id, session id)` when
    /// a run started. `None` when the automation is gone.
    pub(crate) fn record_automation_outcome(
        &mut self,
        schedule_id: &str,
        outcome: ScheduleSafeOutcome,
        run: Option<(String, String)>,
        finished_at_ms: u64,
    ) -> Option<OutcomeUndo> {
        let schedule = self.schedules.get_mut(schedule_id)?;
        let previous = schedule.clone();
        schedule.counters.record(&outcome.status);
        schedule.updated_at_ms = schedule
            .updated_at_ms
            .max(outcome.occurred_at_ms)
            .max(schedule.created_at_ms);
        let fire = schedule.last_fired.as_ref().map(|fired| ScheduleFireRecord {
            id: fired.run_idempotency_key.clone(),
            schedule_id: schedule.id.clone(),
            agent_id: schedule.agent_id.clone(),
            fired_at_ms: fired.fired_at_ms,
            finished_at_ms: finished_at_ms.max(fired.fired_at_ms),
            outcome: outcome.status.clone(),
            run_id: run.as_ref().map(|(run_id, _)| run_id.clone()),
            session_id: run.map(|(_, session_id)| session_id),
            error_code: outcome.error_code.clone(),
            manual: fired.manual,
        });
        schedule.last_safe_outcome = Some(outcome);
        let (fire_id, previous_fire) = match fire {
            Some(fire) => (Some(fire.id.clone()), self.schedule_fires.record(fire)),
            None => (None, None),
        };
        Some(OutcomeUndo {
            previous,
            fire_id,
            previous_fire,
        })
    }

    /// Puts back what `record_automation_outcome` changed: the outcome, the
    /// counters, and the fire log's entry. Other fields keep any change made
    /// since (an owner's edit is not undone).
    pub(crate) fn undo_automation_outcome(&mut self, undo: OutcomeUndo) {
        if let Some(id) = &undo.fire_id {
            self.schedule_fires.remove(id);
            if let Some(previous) = undo.previous_fire {
                self.schedule_fires.record(previous);
            }
        }
        if let Some(schedule) = self.schedules.get_mut(&undo.previous.id) {
            schedule.last_safe_outcome = undo.previous.last_safe_outcome;
            schedule.counters = undo.previous.counters;
        }
    }
}
```

In `hosts/rust-daemon/src/state.rs`:

1. Add the field after `skills` in `DaemonState`:

```rust
    /// Automation fires the history store does not hold yet (spec §9.1).
    pub(crate) schedule_fires: crate::schedules::FireLog,
```

and in `with_model_adapter_and_events_and_limits` after `skills: crate::skills::SkillRegistry::default(),`:

```text
            schedule_fires: crate::schedules::FireLog::default(),
```

2. At the end of `control_plane_snapshot`, before `snapshot` is returned:

```rust
        snapshot.schedule_fires = self.schedule_fires.snapshot();
```

3. In `restore_control_plane_snapshot`, after the `self.skills = …restored(…);` statement:

```rust
        self.schedule_fires = crate::schedules::FireLog::restored(snapshot.schedule_fires);
```

4. In `validate_control_plane_snapshot`, right after the `validate_stored_trigger` call Task 2 added:

```rust
            crate::schedules::validate_stored_automation(schedule)
                .map_err(|problem| format!("schedule '{}' {problem}", schedule.id))?;
```

and after `crate::skills::SkillRegistry::validate(…)?;`:

```rust
        crate::schedules::FireLog::validate(&snapshot.schedule_fires)?;
```

5. Change `assert_eq!(snapshot.version, 8);` (two places) and `assert_eq!(loaded.version, 8);` to `9`, and the message `a v8 snapshot holding stopped/suppressed values restores` to `a v9 snapshot holding stopped/suppressed values restores`.

In `hosts/rust-daemon/src/live/events.rs`, add to `LiveEventBody` after `SkillUpdated { … }`:

```rust
    /// An automation changed, fired, or finished an occurrence (spec §6);
    /// clients read it again. No session or run.
    AutomationUpdated {
        schedule_id: String,
        deleted: bool,
    },
```

to `type_name` after the `SkillUpdated` arm:

```text
            Self::AutomationUpdated { .. } => "automation.updated",
```

and to `to_json`'s `match` after the `SkillUpdated` arm:

```text
            LiveEventBody::AutomationUpdated {
                schedule_id,
                deleted,
            } => {
                value["scheduleId"] = json!(schedule_id);
                value["deleted"] = json!(deleted);
            }
```

- [ ] **Step 5: Snapshot version 9**

In `hosts/rust-daemon/src/control_plane_store.rs`:

1. Replace the version doc comment and the constants from `CONTROL_PLANE_STORE_VERSION` through `APPROVALS_STORE_VERSION` with:

```rust
/// Snapshot format version. Version 5 adds sessions (companion console M2);
/// version 6 adds the live-run fields (M3: accepted `queued` runs and each
/// run's `replyMessageId`); version 7 adds approvals, approval policies and
/// rules, and session allowances (M4); version 8 adds the skills registry and
/// skill drafts (M5); version 9 adds the automation fields, the `cron` and
/// `once` triggers, and the fire log (M6). Older daemons refuse a newer
/// version, so the first start of a new version writes a backup (spec §13.3).
pub(crate) const CONTROL_PLANE_STORE_VERSION: u32 = 9;
/// The version that added skills: older snapshots are backed up as
/// `.pre-skills.bak` at the latest, this one as `.pre-automations.bak`.
pub(crate) const SKILLS_STORE_VERSION: u32 = 8;
/// The version that added approvals: older snapshots are backed up as
/// `.pre-approvals.bak` at the latest, this one as `.pre-skills.bak`.
pub(crate) const APPROVALS_STORE_VERSION: u32 = 7;
```

2. After `PRE_SKILLS_BACKUP_SUFFIX`, add:

```rust
/// Suffix of the JSON backup taken before the automations upgrade.
pub(crate) const PRE_AUTOMATIONS_BACKUP_SUFFIX: &str = ".pre-automations.bak";
```

3. Add to `ControlPlaneSnapshot`, after `skill_drafts`:

```rust
    /// Automation fires the history store does not hold yet (spec §9.1,
    /// §13.1).
    #[serde(default)]
    pub(crate) schedule_fires: Vec<crate::schedules::ScheduleFireRecord>,
```

and in `with_connector_state_and_cleanup` after `skill_drafts: vec![],`:

```text
            schedule_fires: vec![],
```

4. After `pre_skills_backup_path`, add:

```rust
/// Where the JSON snapshot is backed up before the automations upgrade (from
/// version `SKILLS_STORE_VERSION`).
pub(crate) fn pre_automations_backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(PRE_AUTOMATIONS_BACKUP_SUFFIX);
    path.with_file_name(name)
}
```

5. Replace `pre_upgrade_backup_path` with:

```rust
/// Where the JSON snapshot of `loaded_version` is backed up before it is
/// upgraded: each upgrade keeps its own file, so a later upgrade never
/// overwrites an earlier one's backup.
pub(crate) fn pre_upgrade_backup_path(path: &Path, loaded_version: u32) -> PathBuf {
    if loaded_version < SESSIONS_STORE_VERSION {
        pre_sessions_backup_path(path)
    } else if loaded_version < LIVE_RUNS_STORE_VERSION {
        pre_live_runs_backup_path(path)
    } else if loaded_version < APPROVALS_STORE_VERSION {
        pre_approvals_backup_path(path)
    } else if loaded_version < SKILLS_STORE_VERSION {
        pre_skills_backup_path(path)
    } else {
        // A future version 10 must add its own branch above, or its upgrade
        // would overwrite `.pre-automations.bak`.
        pre_automations_backup_path(path)
    }
}
```

6. In the test module: in `the_backup_path_appends_the_suffix_to_the_file_name` add `assert_eq!(super::postgres_backup_key(8), "control_plane.backup.8");`; at the end of `the_backup_is_named_by_the_version_it_upgrades_from` add:

```rust
        assert_eq!(
            super::pre_upgrade_backup_path(path, 8),
            std::path::PathBuf::from("/data/control-plane.json.pre-automations.bak")
        );
        assert_eq!(
            super::pre_automations_backup_path(path),
            super::pre_upgrade_backup_path(path, 8)
        );
```

after `a_version_seven_backup_leaves_the_earlier_backups_alone` add:

```rust
    #[tokio::test]
    async fn a_version_eight_backup_leaves_the_earlier_backups_alone() {
        let path = test_snapshot_path("automations-backup");
        let m5_backup = "{\"version\":7,\"agents\":[],\"swarms\":[]}";
        std::fs::write(super::pre_skills_backup_path(&path), m5_backup).unwrap();
        let original = "{\n  \"version\": 8,\n  \"agents\": [],\n  \"swarms\": []\n}\n";
        std::fs::write(&path, original).unwrap();
        let config = super::ControlPlaneStoreConfig::Json(path.clone());

        let location = super::write_pre_upgrade_backup(&config, 8).await.unwrap();

        let backup = super::pre_automations_backup_path(&path);
        assert_eq!(location, backup.display().to_string());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(
            std::fs::read_to_string(super::pre_skills_backup_path(&path)).unwrap(),
            m5_backup,
            "the M5 upgrade's backup survives the M6 upgrade"
        );
        assert_no_temp_residue(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
```

and in `snapshot_serializes_current_version_with_empty_connector_collections` change `assert_eq!(payload["version"], 8);` to `9` and add `assert_eq!(payload["scheduleFires"], serde_json::json!([]));`.

Change `assert_eq!(payload["version"], 8);` in `hosts/rust-daemon/src/approvals/registry.rs` and in `hosts/rust-daemon/src/skills/registry.rs`, and `assert_eq!(snapshot.version, 8);` in `hosts/rust-daemon/src/agent_runs/live_tests.rs`, to `9`.

In `hosts/rust-daemon/src/app/persistence.rs`'s tests:

1. Change every `assert_eq!(saved["version"], 8` to `9` (five places: lines near 635, 686, 736, 787, and 821, the last with the message `"a version-8 snapshot writes no backup"`, which becomes `"a version-9 snapshot writes no backup"`). Update the stale prose beside them: both messages `a fresh start saves a version-8 snapshot` become `a fresh start saves a version-9 snapshot`, and the comment `proves loading a current (v8) snapshot never rewrites it` becomes `(v9)`. In `upgrading_any_pre_sessions_snapshot_writes_the_backup_before_saving_the_current_version` and `a_current_snapshot_loads_without_a_backup` also assert `!crate::control_plane_store::pre_automations_backup_path(&path).exists()` (the first with the message `"{version:?}: a pre-sessions snapshot writes no automations backup"`). That makes eleven version-8 assertions in all (state.rs three, approvals/registry.rs one, skills/registry.rs one, live_tests.rs one, control_plane_store.rs one, persistence.rs five, counting the M5 upgrade test's `saved["version"]`, which now saves 9).
2. After `upgrading_a_version_seven_snapshot_writes_the_skills_backup_and_loads_it`, add:

```rust
    /// M6: an M5 (version-8) snapshot is backed up as `.pre-automations.bak`
    /// before version 9 is saved; its automations load with the M6 fields'
    /// defaults, and the earlier upgrades' backups stay.
    #[tokio::test]
    async fn upgrading_a_version_eight_snapshot_writes_the_automations_backup_and_loads_it() {
        let dir = temp_dir("upgrade-automations");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control-plane.json");
        let mut source = crate::state::DaemonState::new();
        let agent_id = source.create_agent(upgrader()).unwrap().state.id;
        source.schedules.insert(
            "schedule-1".into(),
            crate::schedules::test_automation(&agent_id, "schedule-1"),
        );
        let mut value = serde_json::to_value(source.control_plane_snapshot()).unwrap();
        value["version"] = 8.into();
        value.as_object_mut().unwrap().remove("scheduleFires");
        let stored = value["schedules"][0].as_object_mut().unwrap();
        for key in ["name", "activeHours", "createdBy", "preset", "counters"] {
            stored.remove(key);
        }
        let original = serde_json::to_string_pretty(&value).unwrap();
        std::fs::write(&path, &original).unwrap();
        let m5_backup = older_snapshot_file(Some(7));
        let pre_skills = crate::control_plane_store::pre_skills_backup_path(&path);
        std::fs::write(&pre_skills, &m5_backup).unwrap();
        let state = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));

        configure_control_plane_store(&state, Some(ControlPlaneStoreConfig::Json(path.clone())))
            .await
            .unwrap();

        let backup = crate::control_plane_store::pre_automations_backup_path(&path);
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            original,
            "the backup is the untouched version-8 original"
        );
        assert_eq!(
            std::fs::read_to_string(&pre_skills).unwrap(),
            m5_backup,
            "the M5 upgrade's backup is never overwritten"
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 9);
        assert_eq!(saved["scheduleFires"], serde_json::json!([]));
        let guard = state.read().await;
        let restored = &guard.schedules["schedule-1"];
        assert_eq!(restored.name, "");
        assert_eq!(restored.active_hours, None);
        assert_eq!(
            restored.created_by,
            crate::schedules::AutomationCreator::Owner
        );
        assert_eq!(restored.preset, None);
        assert_eq!(
            restored.counters,
            crate::schedules::AutomationCounters::default()
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(dir);
    }
```

In `hosts/rust-daemon/README.md`'s "Operational notes", after the "Rolling back from M5 to an M4 daemon" bullet, add:

```markdown
- Rolling back from M6 to an M5 daemon: M5 refuses the version-9 snapshot the first M6 start saves (`unsupported control plane store version: 9`). Stop the daemon, then restore `<file>.pre-automations.bak` over the control-plane file (JSON store) or the `control_plane.backup.8` row over the current one (Postgres). Control-plane changes made since the upgrade are lost, including automation names, active hours, cron and one-time automations, counters, and fire records not yet in the history store. Rows already in the history store's `schedule_runs` table stay; an M5 daemon ignores them.
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules state:: live::tests control_plane_store app::persistence approvals::registry skills::registry agent_runs::live_tests connectors::runtime 2>&1 | tail -30`
Expected: PASS (4 automation tests, 4 history tests, 5 automation-state tests, the event test, the version-9 tests, and every existing test in those modules).

Run: `grep -rn '"version"\], 8\|version, 8)' hosts/rust-daemon/src`
Expected: no output.

Run: `grep -rn 'saves a version-8 snapshot\|current (v8) snapshot\|a v8 snapshot\|a version-8 snapshot writes' hosts/rust-daemon/src`
Expected: no output.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/schedules/automations.rs hosts/rust-daemon/src/schedules/history.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/state/automation_state.rs hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/app/persistence.rs hosts/rust-daemon/src/approvals/registry.rs hosts/rust-daemon/src/skills/registry.rs hosts/rust-daemon/src/agent_runs/live_tests.rs hosts/rust-daemon/src/live/events.rs hosts/rust-daemon/src/live/tests.rs hosts/rust-daemon/src/connectors/runtime.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): add automation fields and the fire log, and move the control plane to version 9"
```

Recommended implementer tier: standard (the M2–M5 persistence pattern plus pure types with complete tests).

---

### Task 4: Fire history in the history store, the outbox, and silent check-in retention

**Files:**

- Modify: `hosts/rust-daemon/src/history/mod.rs` (two trait methods, the delete docs), `hosts/rust-daemon/src/history/sqlite.rs`, `hosts/rust-daemon/src/history/postgres.rs`, `hosts/rust-daemon/src/history/memory.rs`, `hosts/rust-daemon/src/history/conformance.rs` (the shared suite and `FlakyHistoryStore`), `hosts/rust-daemon/src/history/outbox.rs` (`write_schedule_fires`, `FlushReport.fires`), `hosts/rust-daemon/src/sessions/pruning.rs`

**Interfaces:**

- Consumes: Task 3's `ScheduleFireRecord`, `FireLog::mark_mirrored`, `DaemonState::unmirrored_schedule_fires`, `history::tests_support_fire`; the existing `schedule_runs` (SQLite) and `history_schedule_runs` (Postgres) tables, unchanged (no schema version change: they exist since the history store's first version).
- Produces:
  - `HistoryStore::upsert_schedule_runs(&self, fires: &[ScheduleFireRecord]) -> Result<(), HistoryError>` (idempotent by id) and `HistoryStore::page_schedule_runs(&self, agent_id: &str, schedule_id: &str, limit: usize) -> Result<Vec<ScheduleFireRecord>, HistoryError>` (newest first by `firedAtMs`, then id descending), in every store.
  - `delete_agent` also deletes the agent's fire rows; `delete_session` leaves them (an automation's history is not a session's).
  - `history::conformance::{fire_record(id, schedule_id, agent_id, fired_at_ms), assert_history_store_schedule_run_conformance}`.
  - `outbox::{HISTORY_FIRE_BATCH = 200}`, `FlushReport.fires`; each flush writes unmirrored fires after decided approvals and removes from the control plane each one the store now holds unchanged; fires of deleted agents are dropped, never written.
  - Hot-tail pruning: a silent check-in turn (spec §3.3 hidden messages) that is mirrored, older than 24 hours, and unreferenced leaves the hot tail wherever it sits, whole; a turn with any message not yet mirrored stays whole.
- Behavior: everything else in the outbox and pruning is unchanged (the window of the newest 200 visible messages, the `prunedThrough` mark, active-run and outbound-reference exclusions, ephemeral and unreconciled stores).

- [ ] **Step 1: Write the failing tests**

Add to `hosts/rust-daemon/src/history/conformance.rs`, after `assert_history_store_approval_conformance`:

```rust
pub(crate) fn fire_record(
    id: &str,
    schedule_id: &str,
    agent_id: &str,
    fired_at_ms: u64,
) -> ScheduleFireRecord {
    ScheduleFireRecord {
        id: id.into(),
        schedule_id: schedule_id.into(),
        agent_id: agent_id.into(),
        fired_at_ms,
        finished_at_ms: fired_at_ms + 10,
        outcome: ScheduleOutcomeStatus::Spoke,
        run_id: Some(format!("run_{id}")),
        session_id: Some(format!("schedule:{schedule_id}")),
        error_code: None,
        manual: false,
    }
}

/// Automation fire history (spec §9.1): idempotent writes, newest first per
/// automation, kept by session deletions and removed with the agent.
pub(crate) async fn assert_history_store_schedule_run_conformance(store: &dyn HistoryStore) {
    let agent = format!("agent-{}", uuid::Uuid::new_v4());
    let other = format!("agent-{}", uuid::Uuid::new_v4());
    let schedule = format!("schedule-{}", uuid::Uuid::new_v4());
    let fire = |n: u32, at: u64| {
        fire_record(&format!("schedule:{schedule}:{n}"), &schedule, &agent, at)
    };
    let (first, second, tie) = (fire(1, 100), fire(2, 200), fire(3, 200));
    let elsewhere = fire_record(&format!("elsewhere-{schedule}"), "schedule-other", &agent, 300);
    let foreign = fire_record(&format!("foreign-{schedule}"), &schedule, &other, 400);
    store
        .upsert_schedule_runs(&[
            first.clone(),
            second.clone(),
            tie.clone(),
            elsewhere.clone(),
            foreign.clone(),
        ])
        .await
        .unwrap();
    let mut rewritten = first.clone();
    rewritten.outcome = ScheduleOutcomeStatus::Failed;
    store
        .upsert_schedule_runs(&[rewritten.clone()])
        .await
        .unwrap();
    store.upsert_schedule_runs(&[]).await.unwrap();

    assert_eq!(
        store.page_schedule_runs(&agent, &schedule, 50).await.unwrap(),
        vec![tie.clone(), second.clone(), rewritten.clone()],
        "newest first, the id breaking a tie, the rewrite in place"
    );
    assert_eq!(
        store.page_schedule_runs(&agent, &schedule, 1).await.unwrap(),
        vec![tie]
    );
    assert_eq!(
        store.page_schedule_runs(&other, &schedule, 50).await.unwrap(),
        vec![foreign.clone()]
    );

    store
        .delete_session(&agent, &format!("schedule:{schedule}"))
        .await
        .unwrap();
    assert_eq!(
        store
            .page_schedule_runs(&agent, &schedule, 50)
            .await
            .unwrap()
            .len(),
        3,
        "deleting a session leaves the automation's history"
    );
    store.delete_agent(&agent).await.unwrap();
    assert!(store
        .page_schedule_runs(&agent, &schedule, 50)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .page_schedule_runs(&agent, "schedule-other", 50)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.page_schedule_runs(&other, &schedule, 50).await.unwrap(),
        vec![foreign],
        "another agent's history stays"
    );
}
```

and add `use crate::schedules::{ScheduleFireRecord, ScheduleOutcomeStatus};` to its imports.

Call it in the three conformance tests: add `assert_history_store_schedule_run_conformance(&store).await;` after `assert_history_store_approval_conformance(&store).await;` in `memory.rs` (`memory_store_meets_the_conformance_suite`), `sqlite.rs` (`sqlite_store_meets_the_conformance_suite`), and `postgres.rs` (`postgres_store_meets_the_conformance_suite`, still `#[ignore]`), and add `assert_history_store_schedule_run_conformance` to each of their `use crate::history::conformance::{…}` lists.

Add to `hosts/rust-daemon/src/history/outbox.rs`'s test module, after `a_failing_store_keeps_decided_approvals_and_a_deleted_session_drops_its_own`:

```rust
    #[tokio::test]
    async fn fires_reach_the_store_and_leave_the_control_plane() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let kept = crate::schedules::history::tests_support_fire(&agent_id);
        let mut orphan = kept.clone();
        orphan.id = "schedule:s1:20".into();
        orphan.agent_id = "agent-deleted".into();
        {
            let mut guard = state.write().await;
            guard.schedule_fires.record(kept.clone());
            guard.schedule_fires.record(orphan);
        }
        let history = state.read().await.history.clone();
        store.set_failing(true);

        assert!(history
            .flush_once(&state, &transactions, now_millis())
            .await
            .is_err());
        assert_eq!(
            state.read().await.schedule_fires.snapshot(),
            vec![kept.clone()],
            "a failing store keeps the fire; the deleted agent's was dropped, not written"
        );

        store.set_failing(false);
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(report.fires, 1);
        assert_eq!(
            store.page_schedule_runs(&agent_id, "s1", 50).await.unwrap(),
            vec![kept]
        );
        assert!(
            store
                .page_schedule_runs("agent-deleted", "s1", 50)
                .await
                .unwrap()
                .is_empty(),
            "a deleted agent's fire is dropped, never written"
        );
        assert_eq!(state.read().await.schedule_fires.len(), 0);
    }
```

In the same module's `committed_turns_and_terminal_runs_reach_the_store_and_runs_are_marked_mirrored`, add `fires: 0,` to the `FlushReport { … }` literal after `approvals: 0,`.

In `hosts/rust-daemon/src/sessions/pruning.rs`'s test module, replace the last part of `silent_checkin_pairs_do_not_take_a_place_among_a_sessions_newest_200`, from `let mut pruned = undo.message_ids.clone();` to the end of the test, with:

```rust
        let mut pruned = undo.message_ids.clone();
        pruned.sort();
        let mut expected = (95..100)
            .map(|index| format!("recent-silent-{index}"))
            .chain((0..3).map(|index| format!("silent-{index}")))
            .flat_map(|turn| [format!("{turn}-prompt"), format!("{turn}-reply")])
            .collect::<Vec<_>>();
        expected.sort();
        // M6: old, mirrored silent pairs leave wherever they sit, including
        // those among the newest 200 visible messages.
        assert_eq!(pruned, expected);
        let hot = guard.get_agent(&agent).unwrap().messages;
        assert_eq!(
            hot.iter()
                .filter(|message| message.id.starts_with("visible-"))
                .count(),
            200,
            "silent pairs never push a visible message out of the newest 200"
        );
        assert_eq!(hot.len(), 200, "no silent pair is left");
```

and add after that test:

```rust
    #[tokio::test]
    async fn silent_checkin_pairs_leave_a_small_room_once_old_and_mirrored() {
        // M6: a heartbeat writes a silent pair every 30 minutes into a room
        // with few visible messages; without this, those pairs would never
        // leave the control plane.
        let (state, agent) = mirrored_state(|agent| {
            let mut messages = Vec::new();
            messages.extend(checkin_turn(agent, "spoken-a", "Something came up.", 1_000));
            for index in 0..10u64 {
                messages.extend(checkin_turn(
                    agent,
                    &format!("silent-{index}"),
                    "CHECKIN_OK",
                    2_000 + 2 * index,
                ));
            }
            messages.extend(checkin_turn(agent, "spoken-b", "Another update.", 3_000));
            messages.extend(checkin_turn(agent, "half-mirrored", "CHECKIN_OK", 3_100));
            messages.extend(checkin_turn(agent, "recent", "CHECKIN_OK", NOW_MS - 1_000));
            messages
        })
        .await;
        let history = state.read().await.history.clone();
        history.forget_mirrored(["half-mirrored-reply"]);
        let mut guard = state.write().await;

        let undo = guard
            .prune_hot_tail(NOW_MS)
            .expect("the old, mirrored silent pairs leave");

        let mut pruned = undo.message_ids.clone();
        pruned.sort();
        let mut expected = (0..10)
            .flat_map(|index| [format!("silent-{index}-prompt"), format!("silent-{index}-reply")])
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(pruned, expected);
        let hot = guard
            .get_agent(&agent)
            .unwrap()
            .messages
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        assert_eq!(
            hot,
            [
                "spoken-a-prompt",
                "spoken-a-reply",
                "spoken-b-prompt",
                "spoken-b-reply",
                "half-mirrored-prompt",
                "half-mirrored-reply",
                "recent-prompt",
                "recent-reply",
            ],
            "visible turns, a pair not wholly mirrored, and a recent pair stay"
        );
        assert!(
            guard
                .sessions
                .get(&agent, "schedule:s1")
                .and_then(|session| session.pruned_through.clone())
                .is_none(),
            "silent pairs never move the pruned-through mark"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history:: sessions::pruning 2>&1 | tail -30`
Expected: FAIL to compile (`upsert_schedule_runs`, `page_schedule_runs`, and `FlushReport.fires` do not exist).

- [ ] **Step 3: The trait and the three stores**

In `hosts/rust-daemon/src/history/mod.rs`, add `use crate::schedules::ScheduleFireRecord;` to the imports and these methods to `HistoryStore`, after `page_approvals`:

```rust
    /// Automation fire records (spec §9.1), idempotent by id.
    async fn upsert_schedule_runs(&self, fires: &[ScheduleFireRecord]) -> Result<(), HistoryError>;

    /// An automation's fires, newest first (then by id, descending), at most
    /// `limit`.
    async fn page_schedule_runs(
        &self,
        agent_id: &str,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<ScheduleFireRecord>, HistoryError>;
```

and change the two delete docs to: `/// Removes a session's messages, runs, approvals, and attachment records; an automation's fire history stays.` and `/// Removes an agent's messages, runs, approvals, attachment records, and automation fires in every session; usage rows stay (spec §3.3).`

In `hosts/rust-daemon/src/history/sqlite.rs`, add `use crate::schedules::ScheduleFireRecord;`, these statements after `PAGE_APPROVALS`:

```rust
const UPSERT_SCHEDULE_RUN: &str = "
INSERT INTO schedule_runs (id, schedule_id, agent_id, fired_at_ms, record)
VALUES (?1, ?2, ?3, ?4, ?5)
ON CONFLICT (id) DO UPDATE SET
    schedule_id = excluded.schedule_id,
    agent_id = excluded.agent_id,
    fired_at_ms = excluded.fired_at_ms,
    record = excluded.record";

const PAGE_SCHEDULE_RUNS: &str = "
SELECT record FROM schedule_runs
WHERE agent_id = ?1 AND schedule_id = ?2
ORDER BY fired_at_ms DESC, id DESC
LIMIT ?3";
```

these methods after `page_approvals` in `impl HistoryStore for SqliteHistoryStore`:

```rust
    async fn upsert_schedule_runs(&self, fires: &[ScheduleFireRecord]) -> Result<(), HistoryError> {
        if fires.is_empty() {
            return Ok(());
        }
        let rows = fires
            .iter()
            .map(
                |fire| -> Result<(String, String, String, i64, String), HistoryError> {
                    Ok((
                        fire.id.clone(),
                        fire.schedule_id.clone(),
                        fire.agent_id.clone(),
                        to_i64(fire.fired_at_ms)?,
                        serde_json::to_string(fire)?,
                    ))
                },
            )
            .collect::<Result<Vec<_>, HistoryError>>()?;
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            {
                let mut statement = transaction.prepare_cached(UPSERT_SCHEDULE_RUN)?;
                for (id, schedule_id, agent_id, fired_at_ms, record) in &rows {
                    statement.execute(params![id, schedule_id, agent_id, fired_at_ms, record])?;
                }
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn page_schedule_runs(
        &self,
        agent_id: &str,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<ScheduleFireRecord>, HistoryError> {
        let (agent_id, schedule_id) = (agent_id.to_string(), schedule_id.to_string());
        self.run(move |connection| {
            let mut statement = connection.prepare_cached(PAGE_SCHEDULE_RUNS)?;
            let records = statement
                .query_map(
                    params![
                        agent_id,
                        schedule_id,
                        i64::try_from(limit).unwrap_or(i64::MAX)
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            records
                .iter()
                .map(|record| serde_json::from_str(record).map_err(HistoryError::from))
                .collect()
        })
        .await
    }
```

and in `delete_agent` change the table list to `["messages", "runs", "attachments", "approvals", "schedule_runs"]` and the comment to `// Usage rows stay (spec §3.3); approvals and automation fires go with their agent.` (`delete_session` is unchanged).

In `hosts/rust-daemon/src/history/postgres.rs`, add `use crate::schedules::ScheduleFireRecord;`, these statements after `PAGE_APPROVALS`:

```rust
const UPSERT_SCHEDULE_RUN: &str = "
INSERT INTO history_schedule_runs (id, schedule_id, agent_id, fired_at_ms, record)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (id) DO UPDATE SET
    schedule_id = EXCLUDED.schedule_id,
    agent_id = EXCLUDED.agent_id,
    fired_at_ms = EXCLUDED.fired_at_ms,
    record = EXCLUDED.record";

const PAGE_SCHEDULE_RUNS: &str = "
SELECT record FROM history_schedule_runs
WHERE agent_id = $1 AND schedule_id = $2
ORDER BY fired_at_ms DESC, id COLLATE \"C\" DESC
LIMIT $3";
```

these methods after `page_approvals`:

```rust
    async fn upsert_schedule_runs(&self, fires: &[ScheduleFireRecord]) -> Result<(), HistoryError> {
        if fires.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for fire in fires {
            sqlx::query(UPSERT_SCHEDULE_RUN)
                .bind(&fire.id)
                .bind(&fire.schedule_id)
                .bind(&fire.agent_id)
                .bind(to_i64(fire.fired_at_ms)?)
                .bind(serde_json::to_value(fire)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn page_schedule_runs(
        &self,
        agent_id: &str,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<ScheduleFireRecord>, HistoryError> {
        let rows = sqlx::query(PAGE_SCHEDULE_RUNS)
            .bind(agent_id)
            .bind(schedule_id)
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| -> Result<ScheduleFireRecord, HistoryError> {
                let record: serde_json::Value = row.try_get("record")?;
                Ok(serde_json::from_value(record)?)
            })
            .collect()
    }
```

and add `"history_schedule_runs",` to `delete_agent`'s table list (after `"history_approvals",`) with the same comment change.

In `hosts/rust-daemon/src/history/memory.rs`, add `use crate::schedules::ScheduleFireRecord;`, two fields to `Tables` after `approval_seqs`:

```rust
    schedule_runs: HashMap<String, (u64, ScheduleFireRecord)>,
    schedule_run_seqs: BTreeMap<u64, String>,
```

these methods after `page_approvals`:

```rust
    async fn upsert_schedule_runs(&self, fires: &[ScheduleFireRecord]) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        for fire in fires {
            upsert(
                &mut tables.schedule_runs,
                &mut tables.schedule_run_seqs,
                &mut tables.next_seq,
                fire.id.clone(),
                fire.clone(),
                self.max_rows,
            );
        }
        Ok(())
    }

    async fn page_schedule_runs(
        &self,
        agent_id: &str,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<ScheduleFireRecord>, HistoryError> {
        let tables = self.tables();
        let mut rows = tables
            .schedule_runs
            .values()
            .map(|(_, fire)| fire)
            .filter(|fire| fire.agent_id == agent_id && fire.schedule_id == schedule_id)
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .fired_at_ms
                .cmp(&left.fired_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        rows.truncate(limit);
        Ok(rows)
    }
```

and in `delete_agent`, after the approvals `remove_where`:

```rust
        remove_where(
            &mut tables.schedule_runs,
            &mut tables.schedule_run_seqs,
            |fire| fire.agent_id == agent_id,
        );
```

In `hosts/rust-daemon/src/history/conformance.rs`'s `impl HistoryStore for FlakyHistoryStore`, after `page_approvals`:

```rust
    async fn upsert_schedule_runs(&self, fires: &[ScheduleFireRecord]) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.upsert_schedule_runs(fires).await
    }

    async fn page_schedule_runs(
        &self,
        agent_id: &str,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<ScheduleFireRecord>, HistoryError> {
        self.check()?;
        self.inner
            .page_schedule_runs(agent_id, schedule_id, limit)
            .await
    }
```

- [ ] **Step 4: The outbox writes fires**

In `hosts/rust-daemon/src/history/outbox.rs`:

1. After `HISTORY_APPROVAL_BATCH`, add:

```rust
/// Automation fire records per store write.
pub(crate) const HISTORY_FIRE_BATCH: usize = 200;
```

2. Add `pub(crate) fires: usize,` to `FlushReport` after `approvals`.

3. In `flush_locked`, change the last line to:

```text
        self.write_approvals(state, transactions, report).await?;
        self.write_schedule_fires(state, transactions, report).await
```

4. After `write_approvals`, add:

```rust
    /// Writes automation fire records in batches, read under the
    /// control-plane transaction so only saved outcomes are mirrored; each
    /// one the store then holds unchanged leaves the control plane, and a
    /// deleted agent's are dropped instead (spec §9.1, §13.1).
    async fn write_schedule_fires(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        loop {
            let fires = {
                let _transaction = transactions.lock().await;
                state
                    .write()
                    .await
                    .unmirrored_schedule_fires(HISTORY_FIRE_BATCH)
            };
            if fires.is_empty() {
                return Ok(());
            }
            self.store.upsert_schedule_runs(&fires).await?;
            let removed = state.write().await.schedule_fires.mark_mirrored(&fires);
            report.fires += removed;
            if removed == 0 || fires.len() < HISTORY_FIRE_BATCH {
                return Ok(());
            }
        }
    }
```

5. Update the module doc's first sentence to `History outbox (spec §13.1): committed messages, terminal runs, decided approvals, and automation fire records reach the history store within about a second, …` (keep the rest).

- [ ] **Step 5: Silent check-in pairs leave once old and mirrored**

In `hosts/rust-daemon/src/sessions/pruning.rs`'s `prune_hot_tail`, replace the body of `for (room_id, messages) in rooms { … }` with:

```rust
            for (room_id, messages) in rooms {
                if active_sessions.contains(&(agent_id.clone(), session_id_for_room(room_id))) {
                    continue;
                }
                let hidden = hidden_message_ids(messages.iter().copied());
                let eligible = |message: &&Message| {
                    message.created_at_ms <= cutoff
                        && !undelivered_references.contains(message.id.as_str())
                        && self.history.is_mirrored(&message.id)
                };
                // A room no longer than the window has no window candidates.
                let room_prunable: Vec<&Message> = if messages.len() > HOT_TAIL_MESSAGES {
                    outside_newest_visible(&messages, &hidden)
                        .iter()
                        .copied()
                        .filter(eligible)
                        .collect()
                } else {
                    Vec::new()
                };
                // The mark names the newest pruned message the model could
                // see: silent check-in pairs leaving take nothing out of its
                // view, so a pass pruning only those leaves the mark alone.
                if let Some(newest) = room_prunable
                    .iter()
                    .rev()
                    .find(|message| !hidden.contains(&message.id))
                {
                    newest_pruned.push((
                        agent_id.clone(),
                        session_id_for_room(room_id),
                        SessionPrunedThrough::of(newest),
                    ));
                }
                prunable.extend(room_prunable.iter().map(|message| message.id.clone()));
                // M6: a silent check-in turn leaves once every message of it
                // is old, mirrored, and unreferenced, wherever it sits, so a
                // heartbeat's session does not grow the control plane forever
                // (see the M6 plan's notes on spec §13.2).
                for turn in silent_turns(&messages, &hidden) {
                    if turn.iter().all(eligible) {
                        prunable.extend(turn.iter().map(|message| message.id.clone()));
                    }
                }
            }
```

and add after `outside_newest_visible`:

```rust
/// A room's hidden messages grouped into their turns (a user message starts
/// one), so a silent check-in pair only ever leaves whole.
fn silent_turns<'a>(messages: &[&'a Message], hidden: &HashSet<String>) -> Vec<Vec<&'a Message>> {
    let mut turns = Vec::new();
    let mut current: Vec<&'a Message> = Vec::new();
    for &message in messages {
        if !hidden.contains(&message.id) {
            if !current.is_empty() {
                turns.push(std::mem::take(&mut current));
            }
            continue;
        }
        if message.role == anima_core::MessageRole::User && !current.is_empty() {
            turns.push(std::mem::take(&mut current));
        }
        current.push(message);
    }
    if !current.is_empty() {
        turns.push(current);
    }
    turns
}
```

Also update `HOT_TAIL_MESSAGES`'s doc comment to: `/// Each session keeps at least its newest 200 visible messages (spec §16). Hidden messages -- silent check-in pairs (spec §3.3) -- take no place among them (Controller ruling 1, M2 pre-flight audit), and since M6 they leave once old and mirrored wherever they sit.`

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history:: sessions::pruning 2>&1 | tail -30`
Expected: PASS (the conformance suites now covering fires in memory and SQLite, the outbox test, the two pruning tests, and every existing history and pruning test; the Postgres suite stays ignored).

- [ ] **Step 7: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add hosts/rust-daemon/src/history/mod.rs hosts/rust-daemon/src/history/sqlite.rs hosts/rust-daemon/src/history/postgres.rs hosts/rust-daemon/src/history/memory.rs hosts/rust-daemon/src/history/conformance.rs hosts/rust-daemon/src/history/outbox.rs hosts/rust-daemon/src/sessions/pruning.rs
git commit -m "feat(daemon): mirror automation fires to the history store and prune old silent check-ins"
```

Recommended implementer tier: standard (the approvals mirroring pattern, with complete code).

---

### Task 5: `AutomationService`: create, update, delete, limits, hidden text, and the heartbeat preset

**Files:**

- Modify: `hosts/rust-daemon/src/schedules/automations.rs` (limits, strings, inputs, the heartbeat preset, `AutomationService`, tests), `hosts/rust-daemon/src/schedules.rs` (re-exports, `ScheduleError::Conflict`, `SchedulerService::{automations, list, create, update, delete}` become wrappers; the old bodies move into the service), `hosts/rust-daemon/src/schedules/timing.rs` (drop the temporary `allow`), `hosts/rust-daemon/src/skills/mod.rs` (three functions become `pub(crate)`), `hosts/rust-daemon/src/routes/schedules.rs` (one error arm)

**Interfaces:**

- Consumes: Tasks 2–3 (`next_due`, `normalized_active_hours`, `upcoming_fires`, `ActiveWindow`, the record fields, `publish_automation_updated`); `skills::{is_smuggling_character, is_hidden_in_one_line, has_variation_selector_run}`; `schedules::{validate_prompt, validate_target, next_schedule_id}` (private items of the parent module, reachable from the child).
- Produces:
  - The Global Constraints' `automations.rs` limits and strings (all but `AUTOMATION_ALREADY_RUNNING`, `TOO_MANY_RUNNING_AUTOMATIONS`, `AUTOMATION_HISTORY_UNAVAILABLE`, and `PROMPT_AND_TRIGGER_REQUIRED`, which Tasks 6 and 7 add with their first use).
  - `ScheduleError::Conflict(&'static str)` (409, code `schedule_conflict`).
  - `AutomationInput { agent_id, name: Option<String>, prompt, trigger, active_hours: Option<ActiveHours>, target, enabled, preset, created_by, import_idempotency_key, explicit_next_due_at_ms, created_at_override_ms }` with `AutomationInput::owner(agent_id, prompt, trigger, target)`; `AutomationPatch { name, prompt, trigger, active_hours: Option<Option<ActiveHours>>, target, enabled }` (`Default`); `heartbeat_input(agent_id, time_zone, target) -> Result<AutomationInput, ScheduleError>`; `preview(trigger, active_hours, now_ms) -> Result<Vec<u64>, ScheduleError>` (the next `PREVIEW_FIRES`).
  - `AutomationService::{new(state, transactions), list(agent_id), get(agent_id, schedule_id), create(input, now_ms) -> Result<(record, created), _>, update(agent_id, schedule_id, patch, now_ms), pause(agent_id, schedule_id, now_ms) -> Result<(record, changed), _>, delete(agent_id, schedule_id)}`; `SchedulerService::automations()`.
- Behavior: every change runs in its own task holding the control-plane transaction, saves, puts the previous state back on a failed save (`Persistence`, 503), and then publishes `automation.updated`. Creation refuses a 21st automation of an agent (`Conflict(TOO_MANY_AUTOMATIONS)`; a legacy import, identified by its import key, is exempt), a companion-made automation whose next 10 fires are less than 5 minutes apart (`Invalid(AGENT_AUTOMATION_TOO_FREQUENT)`), hidden characters (`Invalid(AUTOMATION_TEXT_HIDDEN)`), and a bad name (`Invalid(AUTOMATION_NAME_INVALID)`); a missing name comes from the prompt. An update recomputes the due time when the trigger or the active hours change or the automation is turned back on. The existing `SchedulerService::{create, update, delete, list}` keep their signatures (routes, the legacy import, and the scheduler tests keep calling them) and delegate to the service.

- [ ] **Step 1: Write the failing tests**

Add to the test module of `hosts/rust-daemon/src/schedules/automations.rs` (after the Task 3 tests, inside the same `mod tests`):

```rust
    use std::sync::Arc;

    use tokio::sync::{Mutex, RwLock};

    use crate::agent_runs::test_support::{companion_config, next_event};
    use crate::app::SharedDaemonState;
    use crate::schedules::timing::{ONCE_NOT_IN_FUTURE, TIME_ZONE_INVALID};
    use crate::schedules::{ScheduleError, ScheduleTarget};
    use crate::sessions::test_support::within;
    use crate::state::DaemonState;

    /// 2026-01-05 08:00 UTC, a Monday.
    const NOW: u64 = 1_767_600_000_000;
    const MINUTE: u64 = 60_000;

    fn service() -> (AutomationService, SharedDaemonState, String) {
        let mut daemon = DaemonState::new();
        let agent = daemon
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let state = Arc::new(RwLock::new(daemon));
        (
            AutomationService::new(Arc::clone(&state), Arc::new(Mutex::new(()))),
            state,
            agent,
        )
    }

    fn every(agent: &str, minutes: u64) -> AutomationInput {
        AutomationInput::owner(
            agent.into(),
            "Check status".into(),
            ScheduleTrigger::Interval {
                interval_ms: minutes * MINUTE,
            },
            ScheduleTarget::Workspace,
        )
    }

    fn as_agent(mut input: AutomationInput, agent: &str) -> AutomationInput {
        input.created_by = AutomationCreator::Agent {
            agent_id: agent.into(),
            session_id: "chat:1".into(),
            run_id: "run_1".into(),
            tool_call_id: "call-1".into(),
        };
        input
    }

    fn invalid_snapshot_directory() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "anima-automation-invalid-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_limits_and_strings_are_the_specs() {
        assert_eq!(MAX_AUTOMATIONS_PER_AGENT, 20);
        assert_eq!(MIN_AGENT_AUTOMATION_GAP_MS, 5 * 60 * 1000);
        assert_eq!(AGENT_GAP_CHECKED_FIRES, 10);
        assert_eq!(MAX_AUTOMATION_HISTORY_SHOWN, 50);
        assert_eq!(PREVIEW_FIRES, 3);
        assert_eq!(HEARTBEAT_INTERVAL_MS, 30 * 60 * 1000);
        assert_eq!((HEARTBEAT_START, HEARTBEAT_END), ("08:00", "22:00"));
        assert_eq!(HEARTBEAT_NAME, "Heartbeat");
        assert_eq!(
            HEARTBEAT_PROMPT,
            "Review my open tasks, goals, and recent messages, and tell me briefly about anything that needs my attention."
        );
        assert_eq!(
            TOO_MANY_AUTOMATIONS,
            "This companion already has 20 automations; delete one first"
        );
        assert_eq!(
            AGENT_AUTOMATION_TOO_FREQUENT,
            "Automations you create must run at least 5 minutes apart"
        );
        assert_eq!(AUTOMATION_NAME_INVALID, "name must be 1–80 characters on one line");
        assert_eq!(
            AUTOMATION_TEXT_HIDDEN,
            "Automation text must not contain invisible tag or direction-override characters"
        );
        assert_eq!(
            HEARTBEAT_NEEDS_TIME_ZONE,
            "timeZone is required for the heartbeat preset"
        );
    }

    #[tokio::test]
    async fn create_names_validates_saves_and_announces() {
        let (service, state, agent) = service();
        let mut stream = state.read().await.live.subscribe(&agent).unwrap();
        let input = AutomationInput::owner(
            agent.clone(),
            "Water the plants\nand the herbs".into(),
            ScheduleTrigger::Cron {
                expression: "0 9 * * *".into(),
                time_zone: "UTC".into(),
            },
            ScheduleTarget::Workspace,
        );

        let (record, created) = service.create(input, NOW).await.unwrap();

        assert!(created);
        assert_eq!(record.name, "Water the plants");
        assert_eq!(record.next_due_at_ms, NOW + 60 * MINUTE, "09:00 the same day");
        assert_eq!(record.created_by, AutomationCreator::Owner);
        assert_eq!(state.read().await.schedules[&record.id], record);
        let event = next_event(&mut stream).await.to_json(1);
        assert_eq!(event["type"], "automation.updated");
        assert_eq!(event["scheduleId"], record.id.as_str());
        assert_eq!(event["deleted"], false);
        assert_eq!(service.list(&agent).await.unwrap(), vec![record.clone()]);
        assert_eq!(service.get(&agent, &record.id).await.unwrap(), record);
        assert_eq!(
            service.get("someone-else", &record.id).await,
            Err(ScheduleError::AgentNotFound)
        );
    }

    #[tokio::test]
    async fn the_twenty_first_automation_is_refused() {
        let (service, _, agent) = service();
        for _ in 0..MAX_AUTOMATIONS_PER_AGENT {
            service.create(every(&agent, 60), NOW).await.unwrap();
        }
        assert_eq!(
            service.create(every(&agent, 60), NOW).await,
            Err(ScheduleError::Conflict(TOO_MANY_AUTOMATIONS))
        );
        let mut legacy = every(&agent, 60);
        legacy.import_idempotency_key = Some("legacy:agent:1".into());
        assert!(
            service.create(legacy, NOW).await.is_ok(),
            "a legacy import is exempt"
        );
    }

    #[tokio::test]
    async fn agent_automations_must_be_five_minutes_apart() {
        let (service, _, agent) = service();
        assert_eq!(
            service.create(as_agent(every(&agent, 4), &agent), NOW).await,
            Err(ScheduleError::Invalid(AGENT_AUTOMATION_TOO_FREQUENT))
        );
        let mut cron = as_agent(every(&agent, 60), &agent);
        cron.trigger = ScheduleTrigger::Cron {
            expression: "*/4 * * * *".into(),
            time_zone: "UTC".into(),
        };
        assert_eq!(
            service.create(cron, NOW).await,
            Err(ScheduleError::Invalid(AGENT_AUTOMATION_TOO_FREQUENT))
        );
        assert!(service.create(as_agent(every(&agent, 5), &agent), NOW).await.is_ok());
        assert!(
            service.create(every(&agent, 1), NOW).await.is_ok(),
            "the owner has no minimum"
        );
        let mut once = as_agent(every(&agent, 60), &agent);
        once.trigger = ScheduleTrigger::Once {
            at_ms: NOW + MINUTE,
        };
        assert!(service.create(once, NOW).await.is_ok(), "one fire has no gap");
    }

    #[tokio::test]
    async fn hidden_text_and_bad_names_are_refused() {
        let (service, state, agent) = service();
        let mut hidden_prompt = every(&agent, 60);
        hidden_prompt.prompt = "Check \u{202E}status".into();
        let mut hidden_name = every(&agent, 60);
        hidden_name.name = Some("Check\u{200B}".into());
        let mut two_lines = every(&agent, 60);
        two_lines.name = Some("Check\nstatus".into());
        let mut long_name = every(&agent, 60);
        long_name.name = Some("x".repeat(81));
        let mut blank_name = every(&agent, 60);
        blank_name.name = Some("  ".into());
        for (input, problem) in [
            (hidden_prompt, AUTOMATION_TEXT_HIDDEN),
            (hidden_name, AUTOMATION_TEXT_HIDDEN),
            (two_lines, AUTOMATION_NAME_INVALID),
            (long_name, AUTOMATION_NAME_INVALID),
            (blank_name, AUTOMATION_NAME_INVALID),
        ] {
            assert_eq!(
                service.create(input, NOW).await,
                Err(ScheduleError::Invalid(problem))
            );
        }
        assert!(state.read().await.schedules.is_empty());
    }

    #[tokio::test]
    async fn active_hours_are_stored_in_order_and_a_once_refuses_them() {
        let (service, _, agent) = service();
        let mut input = every(&agent, 30);
        input.active_hours = Some(ActiveHours {
            start: "09:00".into(),
            end: "17:00".into(),
            days: vec![5, 1],
            time_zone: "UTC".into(),
        });
        let (record, _) = service.create(input, NOW).await.unwrap();
        assert_eq!(record.active_hours.as_ref().unwrap().days, vec![1, 5]);
        assert_eq!(record.next_due_at_ms, NOW + 60 * MINUTE, "the window opens at 09:00");

        let mut once = every(&agent, 30);
        once.trigger = ScheduleTrigger::Once {
            at_ms: NOW + MINUTE,
        };
        once.active_hours = record.active_hours.clone();
        assert_eq!(
            service.create(once, NOW).await,
            Err(ScheduleError::Rejected(ACTIVE_HOURS_NOT_FOR_ONCE.into()))
        );
        let mut past = every(&agent, 30);
        past.trigger = ScheduleTrigger::Once { at_ms: NOW };
        assert_eq!(
            service.create(past, NOW).await,
            Err(ScheduleError::Rejected(ONCE_NOT_IN_FUTURE.into()))
        );
    }

    #[tokio::test]
    async fn an_update_recomputes_timing_only_when_it_must() {
        let (service, _, agent) = service();
        let (record, _) = service.create(every(&agent, 60), NOW).await.unwrap();
        let renamed = service
            .update(
                &agent,
                &record.id,
                AutomationPatch {
                    name: Some(" Status ".into()),
                    ..AutomationPatch::default()
                },
                NOW + MINUTE,
            )
            .await
            .unwrap();
        assert_eq!(renamed.name, "Status");
        assert_eq!(renamed.next_due_at_ms, record.next_due_at_ms);

        let windowed = service
            .update(
                &agent,
                &record.id,
                AutomationPatch {
                    active_hours: Some(Some(ActiveHours {
                        start: "12:00".into(),
                        end: "13:00".into(),
                        days: vec![1],
                        time_zone: "UTC".into(),
                    })),
                    ..AutomationPatch::default()
                },
                NOW,
            )
            .await
            .unwrap();
        assert_eq!(windowed.next_due_at_ms, NOW + 4 * 60 * MINUTE, "noon");
        let cleared = service
            .update(
                &agent,
                &record.id,
                AutomationPatch {
                    active_hours: Some(None),
                    ..AutomationPatch::default()
                },
                NOW,
            )
            .await
            .unwrap();
        assert_eq!(cleared.active_hours, None);
        assert_eq!(cleared.next_due_at_ms, NOW + 60 * MINUTE);

        let mut once = every(&agent, 60);
        once.trigger = ScheduleTrigger::Once {
            at_ms: NOW + MINUTE,
        };
        once.enabled = false;
        let (once, _) = service.create(once, NOW).await.unwrap();
        assert_eq!(
            service
                .update(
                    &agent,
                    &once.id,
                    AutomationPatch {
                        enabled: Some(true),
                        ..AutomationPatch::default()
                    },
                    NOW + 2 * MINUTE,
                )
                .await,
            Err(ScheduleError::Rejected(ONCE_NOT_IN_FUTURE.into())),
            "a one-time automation whose time passed needs a new time"
        );
        assert_eq!(
            service
                .update(&agent, &record.id, AutomationPatch::default(), NOW)
                .await,
            Err(ScheduleError::Invalid("at least one field is required"))
        );
        assert_eq!(
            service
                .update(
                    "someone-else",
                    &record.id,
                    AutomationPatch {
                        name: Some("Theirs".into()),
                        ..AutomationPatch::default()
                    },
                    NOW,
                )
                .await,
            Err(ScheduleError::NotFound),
            "another agent's automation is not found"
        );
    }

    #[tokio::test]
    async fn pause_is_idempotent_and_delete_announces() {
        let (service, state, agent) = service();
        let (record, _) = service.create(every(&agent, 60), NOW).await.unwrap();
        let (paused, changed) = service.pause(&agent, &record.id, NOW).await.unwrap();
        assert!(changed && !paused.enabled);
        let (_, changed) = service.pause(&agent, &record.id, NOW).await.unwrap();
        assert!(!changed, "already paused: nothing saved");
        assert_eq!(
            service.pause(&agent, "missing", NOW).await,
            Err(ScheduleError::NotFound)
        );

        let mut stream = state.read().await.live.subscribe(&agent).unwrap();
        service.delete(&agent, &record.id).await.unwrap();
        let event = next_event(&mut stream).await.to_json(1);
        assert_eq!(event["deleted"], true);
        assert!(state.read().await.schedules.is_empty());
        assert_eq!(
            service.delete(&agent, &record.id).await,
            Err(ScheduleError::NotFound)
        );
    }

    #[tokio::test]
    async fn a_failed_save_puts_everything_back() {
        use crate::control_plane_store::ControlPlaneStoreConfig;
        let (service, state, agent) = service();
        let (kept, _) = service.create(every(&agent, 60), NOW).await.unwrap();
        let invalid = invalid_snapshot_directory();
        state
            .write()
            .await
            .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid.clone())));

        assert_eq!(
            service.create(every(&agent, 60), NOW).await,
            Err(ScheduleError::Persistence)
        );
        assert_eq!(
            service
                .update(
                    &agent,
                    &kept.id,
                    AutomationPatch {
                        prompt: Some("Changed".into()),
                        ..AutomationPatch::default()
                    },
                    NOW,
                )
                .await,
            Err(ScheduleError::Persistence)
        );
        assert_eq!(
            service.delete(&agent, &kept.id).await,
            Err(ScheduleError::Persistence)
        );
        let guard = state.read().await;
        assert_eq!(guard.schedules.len(), 1);
        assert_eq!(guard.schedules[&kept.id], kept);
        drop(guard);
        let _ = std::fs::remove_dir_all(invalid);
    }

    #[tokio::test]
    async fn a_dropped_request_still_saves_its_automation() {
        let (service, state, agent) = service();
        let transactions = Arc::clone(&service.transactions);
        let held = transactions.lock().await;
        let mut request = Box::pin(service.create(every(&agent, 60), NOW));
        // One poll spawns the change, which then waits for the transaction.
        tokio::select! {
            biased;
            _ = &mut request => panic!("the change cannot finish while the transaction is held"),
            _ = std::future::ready(()) => {}
        }
        drop(request);
        drop(held);

        within("the dropped create to save", async {
            while state.read().await.schedules.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    #[tokio::test]
    async fn the_heartbeat_preset_runs_every_30_minutes_from_8_to_22_local() {
        let (service, _, agent) = service();
        let input =
            heartbeat_input(agent.clone(), "Asia/Kuala_Lumpur", ScheduleTarget::Workspace)
                .unwrap();
        let (record, _) = service.create(input, NOW).await.unwrap();
        assert_eq!(record.preset, Some(AutomationPreset::Heartbeat));
        assert_eq!(record.name, HEARTBEAT_NAME);
        assert_eq!(record.prompt, HEARTBEAT_PROMPT);
        assert_eq!(
            record.trigger,
            ScheduleTrigger::Interval {
                interval_ms: HEARTBEAT_INTERVAL_MS
            }
        );
        assert_eq!(
            record.active_hours,
            Some(ActiveHours {
                start: HEARTBEAT_START.into(),
                end: HEARTBEAT_END.into(),
                days: vec![0, 1, 2, 3, 4, 5, 6],
                time_zone: "Asia/Kuala_Lumpur".into(),
            })
        );
        // 08:00 UTC is 16:00 in Kuala Lumpur: inside the window.
        assert_eq!(record.next_due_at_ms, NOW + 30 * MINUTE);
        assert_eq!(
            heartbeat_input(agent.clone(), " ", ScheduleTarget::Workspace).err(),
            Some(ScheduleError::Invalid(HEARTBEAT_NEEDS_TIME_ZONE))
        );
        assert_eq!(
            heartbeat_input(agent, "Mars/Base", ScheduleTarget::Workspace).err(),
            Some(ScheduleError::Rejected(TIME_ZONE_INVALID.into()))
        );
    }

    #[test]
    fn previews_list_the_next_three_fires() {
        let trigger = ScheduleTrigger::Interval {
            interval_ms: 30 * MINUTE,
        };
        assert_eq!(
            preview(&trigger, None, NOW),
            Ok(vec![NOW + 30 * MINUTE, NOW + 60 * MINUTE, NOW + 90 * MINUTE])
        );
        assert_eq!(
            preview(&ScheduleTrigger::Interval { interval_ms: 0 }, None, NOW),
            Err(ScheduleError::Rejected(
                crate::schedules::timing::INTERVAL_INVALID.into()
            ))
        );
    }
```

`ScheduleError` needs `PartialEq` for these assertions; it already derives `Clone, Debug, PartialEq, Eq`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::automations 2>&1 | tail -30`
Expected: FAIL to compile (`AutomationService`, `AutomationInput`, the constants, and `ScheduleError::Conflict` do not exist).

- [ ] **Step 3: Make the hidden-text checks shareable**

In `hosts/rust-daemon/src/skills/mod.rs`, change `fn is_smuggling_character`, `fn is_hidden_in_one_line`, and `fn has_variation_selector_run` to `pub(crate) fn` (no other change).

- [ ] **Step 4: Write the service**

In `hosts/rust-daemon/src/schedules/automations.rs`, replace the `use` lines at the top with:

```rust
use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::timing::{
    normalized_active_hours, upcoming_fires, ActiveHours, ActiveWindow, ACTIVE_HOURS_NOT_FOR_ONCE,
};
use super::{
    next_due, next_schedule_id, validate_prompt, validate_target, validate_trigger,
    ScheduleError, ScheduleOutcomeStatus, ScheduleTarget, ScheduleTrigger, ScheduledPromptRecord,
};
use crate::app::SharedDaemonState;
use crate::skills::{has_variation_selector_run, is_hidden_in_one_line, is_smuggling_character};
```

Update the module doc's second sentence to `This module also holds their limits and strings, the heartbeat preset, and \`AutomationService\`, through which the owner's routes and the companion's tools change them.`

Add after `MAX_AUTOMATION_NAME_CHARS`:

```rust
/// Automations per agent (spec §9.3, §16).
pub(crate) const MAX_AUTOMATIONS_PER_AGENT: usize = 20;
/// A companion-made automation's fires are at least this far apart (spec §9.3).
pub(crate) const MIN_AGENT_AUTOMATION_GAP_MS: u64 = 5 * 60 * 1000;
/// Fires checked for that minimum (spec §9.3).
pub(crate) const AGENT_GAP_CHECKED_FIRES: usize = 10;
/// History entries shown (spec §9.1, §16).
pub(crate) const MAX_AUTOMATION_HISTORY_SHOWN: usize = 50;
/// Fire times a preview lists (spec §9.2).
pub(crate) const PREVIEW_FIRES: usize = 3;
/// The heartbeat preset (spec §9.2): every 30 minutes, 08:00 to 22:00 local.
pub(crate) const HEARTBEAT_INTERVAL_MS: u64 = 30 * 60 * 1000;
pub(crate) const HEARTBEAT_START: &str = "08:00";
pub(crate) const HEARTBEAT_END: &str = "22:00";
pub(crate) const HEARTBEAT_NAME: &str = "Heartbeat";
/// The preset's editable prompt; the scheduler's check-in suffix adds the
/// `CHECKIN_OK` instruction.
pub(crate) const HEARTBEAT_PROMPT: &str =
    "Review my open tasks, goals, and recent messages, and tell me briefly about anything that needs my attention.";

pub(crate) const TOO_MANY_AUTOMATIONS: &str =
    "This companion already has 20 automations; delete one first";
pub(crate) const AGENT_AUTOMATION_TOO_FREQUENT: &str =
    "Automations you create must run at least 5 minutes apart";
pub(crate) const AUTOMATION_NAME_INVALID: &str = "name must be 1–80 characters on one line";
pub(crate) const AUTOMATION_TEXT_HIDDEN: &str =
    "Automation text must not contain invisible tag or direction-override characters";
pub(crate) const HEARTBEAT_NEEDS_TIME_ZONE: &str = "timeZone is required for the heartbeat preset";
/// The existing update literal, now named.
const NOTHING_TO_UPDATE: &str = "at least one field is required";
```

Add after `validate_stored_automation` (before `test_automation`):

```rust
/// A new automation, as the owner's routes or the companion's tools ask for it.
#[derive(Clone, Debug)]
pub(crate) struct AutomationInput {
    pub(crate) agent_id: String,
    /// `None`: from the prompt's first line.
    pub(crate) name: Option<String>,
    pub(crate) prompt: String,
    pub(crate) trigger: ScheduleTrigger,
    pub(crate) active_hours: Option<ActiveHours>,
    pub(crate) target: ScheduleTarget,
    pub(crate) enabled: bool,
    pub(crate) preset: Option<AutomationPreset>,
    pub(crate) created_by: AutomationCreator,
    /// A legacy browser import's key: such a create is idempotent and exempt
    /// from the per-agent limit.
    pub(crate) import_idempotency_key: Option<String>,
    pub(crate) explicit_next_due_at_ms: Option<u64>,
    pub(crate) created_at_override_ms: Option<u64>,
}

impl AutomationInput {
    /// An enabled automation the owner makes, with nothing else set.
    pub(crate) fn owner(
        agent_id: String,
        prompt: String,
        trigger: ScheduleTrigger,
        target: ScheduleTarget,
    ) -> Self {
        Self {
            agent_id,
            name: None,
            prompt,
            trigger,
            active_hours: None,
            target,
            enabled: true,
            preset: None,
            created_by: AutomationCreator::Owner,
            import_idempotency_key: None,
            explicit_next_due_at_ms: None,
            created_at_override_ms: None,
        }
    }
}

/// An edit; `active_hours: Some(None)` clears them.
#[derive(Clone, Debug, Default)]
pub(crate) struct AutomationPatch {
    pub(crate) name: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) trigger: Option<ScheduleTrigger>,
    pub(crate) active_hours: Option<Option<ActiveHours>>,
    pub(crate) target: Option<ScheduleTarget>,
    pub(crate) enabled: Option<bool>,
}

impl AutomationPatch {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.prompt.is_none()
            && self.trigger.is_none()
            && self.active_hours.is_none()
            && self.target.is_none()
            && self.enabled.is_none()
    }
}

/// The heartbeat preset (spec §9.2) in the owner's `time_zone`; the caller
/// may replace any field before creating it.
pub(crate) fn heartbeat_input(
    agent_id: String,
    time_zone: &str,
    target: ScheduleTarget,
) -> Result<AutomationInput, ScheduleError> {
    if time_zone.trim().is_empty() {
        return Err(ScheduleError::Invalid(HEARTBEAT_NEEDS_TIME_ZONE));
    }
    super::timing::parse_time_zone(time_zone).map_err(ScheduleError::Rejected)?;
    let mut input = AutomationInput::owner(
        agent_id,
        HEARTBEAT_PROMPT.into(),
        ScheduleTrigger::Interval {
            interval_ms: HEARTBEAT_INTERVAL_MS,
        },
        target,
    );
    input.name = Some(HEARTBEAT_NAME.into());
    input.preset = Some(AutomationPreset::Heartbeat);
    input.active_hours = Some(ActiveHours {
        start: HEARTBEAT_START.into(),
        end: HEARTBEAT_END.into(),
        days: (0..=6).collect(),
        time_zone: time_zone.trim().into(),
    });
    Ok(input)
}

/// The next `PREVIEW_FIRES` fire times (spec §9.2): the browser never
/// computes schedules itself.
pub(crate) fn preview(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    now_ms: u64,
) -> Result<Vec<u64>, ScheduleError> {
    validate_trigger(trigger)?;
    let hours = checked_hours(active_hours.cloned(), trigger)?;
    let window = hours
        .as_ref()
        .map(ActiveWindow::parse)
        .transpose()
        .map_err(ScheduleError::Rejected)?;
    upcoming_fires(trigger, window.as_ref(), now_ms, PREVIEW_FIRES).map_err(ScheduleError::Rejected)
}

fn checked_prompt(prompt: &str) -> Result<(), ScheduleError> {
    validate_prompt(prompt)?;
    if prompt.chars().any(is_smuggling_character) {
        return Err(ScheduleError::Invalid(AUTOMATION_TEXT_HIDDEN));
    }
    Ok(())
}

fn checked_name(name: &str) -> Result<String, ScheduleError> {
    if name.chars().any(is_hidden_in_one_line) || has_variation_selector_run(name) {
        return Err(ScheduleError::Invalid(AUTOMATION_TEXT_HIDDEN));
    }
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_AUTOMATION_NAME_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err(ScheduleError::Invalid(AUTOMATION_NAME_INVALID));
    }
    Ok(trimmed.to_string())
}

fn checked_hours(
    hours: Option<ActiveHours>,
    trigger: &ScheduleTrigger,
) -> Result<Option<ActiveHours>, ScheduleError> {
    let Some(hours) = hours else {
        return Ok(None);
    };
    if matches!(trigger, ScheduleTrigger::Once { .. }) {
        return Err(ScheduleError::Rejected(ACTIVE_HOURS_NOT_FOR_ONCE.into()));
    }
    normalized_active_hours(hours)
        .map(Some)
        .map_err(ScheduleError::Rejected)
}

/// Spec §9.3: a companion's automation fires at least 5 minutes apart over
/// its next 10 fires.
fn check_agent_gap(
    trigger: &ScheduleTrigger,
    active_hours: Option<&ActiveHours>,
    now_ms: u64,
) -> Result<(), ScheduleError> {
    let window = active_hours
        .map(ActiveWindow::parse)
        .transpose()
        .map_err(ScheduleError::Rejected)?;
    let fires = upcoming_fires(trigger, window.as_ref(), now_ms, AGENT_GAP_CHECKED_FIRES)
        .map_err(ScheduleError::Rejected)?;
    if fires
        .windows(2)
        .any(|pair| pair[1].saturating_sub(pair[0]) < MIN_AGENT_AUTOMATION_GAP_MS)
    {
        return Err(ScheduleError::Invalid(AGENT_AUTOMATION_TOO_FREQUENT));
    }
    Ok(())
}

/// The owner's and the companion's changes to automations (spec §9). Each
/// runs in its own task holding the control-plane transaction (a dropped
/// request never leaves an unsaved change in memory), saves, puts the
/// previous state back when the save fails, and then announces
/// `automation.updated`.
#[derive(Clone)]
pub(crate) struct AutomationService {
    state: SharedDaemonState,
    transactions: Arc<Mutex<()>>,
}

impl AutomationService {
    pub(crate) fn new(state: SharedDaemonState, transactions: Arc<Mutex<()>>) -> Self {
        Self {
            state,
            transactions,
        }
    }

    async fn locked<T, F, Fut>(&self, work: F) -> Result<T, ScheduleError>
    where
        T: Send + 'static,
        F: FnOnce(AutomationService) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ScheduleError>> + Send + 'static,
    {
        let this = self.clone();
        tokio::spawn(async move {
            let _transaction = Arc::clone(&this.transactions).lock_owned().await;
            work(this).await
        })
        .await
        .unwrap_or(Err(ScheduleError::Persistence))
    }

    /// The agent's automations, oldest first.
    pub(crate) async fn list(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ScheduledPromptRecord>, ScheduleError> {
        let state = self.state.read().await;
        if state.get_agent(agent_id).is_none() {
            return Err(ScheduleError::AgentNotFound);
        }
        let mut records = state
            .schedules
            .values()
            .filter(|item| item.agent_id == agent_id)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(records)
    }

    pub(crate) async fn get(
        &self,
        agent_id: &str,
        schedule_id: &str,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        let state = self.state.read().await;
        if state.get_agent(agent_id).is_none() {
            return Err(ScheduleError::AgentNotFound);
        }
        state
            .schedules
            .get(schedule_id)
            .filter(|item| item.agent_id == agent_id)
            .cloned()
            .ok_or(ScheduleError::NotFound)
    }

    /// Creates an automation; `(record, false)` when a legacy import's key
    /// already made it.
    pub(crate) async fn create(
        &self,
        input: AutomationInput,
        now_ms: u64,
    ) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        checked_prompt(&input.prompt)?;
        let name = match &input.name {
            Some(name) => checked_name(name)?,
            None => default_name(&input.prompt),
        };
        validate_trigger(&input.trigger)?;
        let active_hours = checked_hours(input.active_hours.clone(), &input.trigger)?;
        let created_at_ms = input.created_at_override_ms.unwrap_or(now_ms);
        if created_at_ms == 0 || created_at_ms > now_ms.saturating_add(300_000) {
            return Err(ScheduleError::Invalid("createdAtMs is invalid"));
        }
        let next_due_at_ms = match input.explicit_next_due_at_ms {
            Some(value) if value > 0 => value,
            Some(_) => return Err(ScheduleError::Invalid("next due time is invalid")),
            None => next_due(&input.trigger, active_hours.as_ref(), now_ms)?,
        };
        if input
            .import_idempotency_key
            .as_ref()
            .is_some_and(|key| key.trim().is_empty() || key.len() > 256)
        {
            return Err(ScheduleError::Invalid("import idempotency key is invalid"));
        }
        if matches!(input.created_by, AutomationCreator::Agent { .. }) {
            check_agent_gap(&input.trigger, active_hours.as_ref(), now_ms)?;
        }
        let record = ScheduledPromptRecord {
            id: String::new(),
            import_idempotency_key: input.import_idempotency_key,
            agent_id: input.agent_id,
            prompt: input.prompt,
            trigger: input.trigger,
            enabled: input.enabled,
            target: input.target,
            next_due_at_ms,
            last_fired: None,
            last_safe_outcome: None,
            created_at_ms,
            updated_at_ms: now_ms.max(created_at_ms),
            name,
            active_hours,
            created_by: input.created_by,
            preset: input.preset,
            counters: AutomationCounters::default(),
        };
        self.locked(move |service| async move { service.insert(record, now_ms).await })
            .await
    }

    async fn insert(
        &self,
        mut record: ScheduledPromptRecord,
        now_ms: u64,
    ) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        let persist = {
            let mut state = self.state.write().await;
            if state.get_agent(&record.agent_id).is_none() {
                return Err(ScheduleError::AgentNotFound);
            }
            if let Some(key) = record.import_idempotency_key.as_ref() {
                if let Some(existing) = state
                    .schedules
                    .values()
                    .find(|item| {
                        item.agent_id == record.agent_id
                            && item.import_idempotency_key.as_ref() == Some(key)
                    })
                    .cloned()
                {
                    return Ok((existing, false));
                }
            }
            validate_target(&state, &record.agent_id, &record.target, record.enabled)?;
            let owned = state
                .schedules
                .values()
                .filter(|item| item.agent_id == record.agent_id)
                .count();
            if record.import_idempotency_key.is_none() && owned >= MAX_AUTOMATIONS_PER_AGENT {
                return Err(ScheduleError::Conflict(TOO_MANY_AUTOMATIONS));
            }
            record.id = loop {
                let candidate = next_schedule_id(now_ms);
                if !state.schedules.contains_key(&candidate) {
                    break candidate;
                }
            };
            state.schedules.insert(record.id.clone(), record.clone());
            state.control_plane_persist_request()
        };
        if persist.save().await.is_err() {
            self.state.write().await.schedules.remove(&record.id);
            return Err(ScheduleError::Persistence);
        }
        self.state
            .read()
            .await
            .publish_automation_updated(&record.agent_id, &record.id, false);
        Ok((record, true))
    }

    /// Changes an automation. The due time is computed again when the
    /// trigger or the active hours change, or when it is turned back on.
    pub(crate) async fn update(
        &self,
        agent_id: &str,
        schedule_id: &str,
        patch: AutomationPatch,
        now_ms: u64,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        if patch.is_empty() {
            return Err(ScheduleError::Invalid(NOTHING_TO_UPDATE));
        }
        if let Some(prompt) = &patch.prompt {
            checked_prompt(prompt)?;
        }
        let name = patch.name.as_deref().map(checked_name).transpose()?;
        if let Some(trigger) = &patch.trigger {
            validate_trigger(trigger)?;
        }
        let (agent_id, schedule_id) = (agent_id.to_string(), schedule_id.to_string());
        self.locked(move |service| async move {
            let (updated, previous, persist) = {
                let mut state = service.state.write().await;
                let previous = state
                    .schedules
                    .get(&schedule_id)
                    .filter(|item| item.agent_id == agent_id)
                    .cloned()
                    .ok_or(ScheduleError::NotFound)?;
                let mut updated = previous.clone();
                if let Some(name) = name {
                    updated.name = name;
                }
                if let Some(prompt) = patch.prompt {
                    updated.prompt = prompt;
                }
                if let Some(target) = patch.target {
                    updated.target = target;
                }
                let was_enabled = updated.enabled;
                if let Some(enabled) = patch.enabled {
                    updated.enabled = enabled;
                }
                let retimed = patch.trigger.is_some()
                    || patch.active_hours.is_some()
                    || (!was_enabled && updated.enabled);
                if let Some(trigger) = patch.trigger {
                    updated.trigger = trigger;
                }
                if let Some(hours) = patch.active_hours {
                    updated.active_hours = checked_hours(hours, &updated.trigger)?;
                }
                if retimed {
                    updated.next_due_at_ms =
                        next_due(&updated.trigger, updated.active_hours.as_ref(), now_ms)?;
                }
                validate_target(&state, &agent_id, &updated.target, updated.enabled)?;
                updated.updated_at_ms = now_ms
                    .max(updated.created_at_ms)
                    .max(previous.updated_at_ms);
                state.schedules.insert(schedule_id.clone(), updated.clone());
                (updated, previous, state.control_plane_persist_request())
            };
            if persist.save().await.is_err() {
                service
                    .state
                    .write()
                    .await
                    .schedules
                    .insert(schedule_id, previous);
                return Err(ScheduleError::Persistence);
            }
            service
                .state
                .read()
                .await
                .publish_automation_updated(&updated.agent_id, &updated.id, false);
            Ok(updated)
        })
        .await
    }

    /// Turns an automation off; `(record, false)` when it already was.
    pub(crate) async fn pause(
        &self,
        agent_id: &str,
        schedule_id: &str,
        now_ms: u64,
    ) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        let current = self
            .get(agent_id, schedule_id)
            .await
            .map_err(|error| match error {
                ScheduleError::AgentNotFound => ScheduleError::NotFound,
                other => other,
            })?;
        if !current.enabled {
            return Ok((current, false));
        }
        let patch = AutomationPatch {
            enabled: Some(false),
            ..AutomationPatch::default()
        };
        Ok((self.update(agent_id, schedule_id, patch, now_ms).await?, true))
    }

    pub(crate) async fn delete(&self, agent_id: &str, schedule_id: &str) -> Result<(), ScheduleError> {
        let (agent_id, schedule_id) = (agent_id.to_string(), schedule_id.to_string());
        self.locked(move |service| async move {
            let (removed, persist) = {
                let mut state = service.state.write().await;
                if !state
                    .schedules
                    .get(&schedule_id)
                    .is_some_and(|item| item.agent_id == agent_id)
                {
                    return Err(ScheduleError::NotFound);
                }
                let removed = state.schedules.remove(&schedule_id).expect("checked");
                (removed, state.control_plane_persist_request())
            };
            if persist.save().await.is_err() {
                service
                    .state
                    .write()
                    .await
                    .schedules
                    .insert(schedule_id, removed);
                return Err(ScheduleError::Persistence);
            }
            service
                .state
                .read()
                .await
                .publish_automation_updated(&agent_id, &schedule_id, true);
            Ok(())
        })
        .await
    }
}
```

Delete `#![allow(dead_code)] // M6 Task 5 uses every item.` from `timing.rs`.

- [ ] **Step 5: `schedules.rs` delegates to the service**

In `hosts/rust-daemon/src/schedules.rs`:

1. Extend the `automations` re-export to (Task 7 adds what only the routes and tools use, so no re-export sits unused in between):

```text
pub(crate) use automations::{
    validate_stored_automation, AutomationCounters, AutomationCreator, AutomationInput,
    AutomationPatch, AutomationPreset, AutomationService,
};
```

2. Add to `ScheduleError`, after `Rejected(String),`:

```text
    /// The change conflicts with the automation's state or a limit (409).
    Conflict(&'static str),
```

3. Add to `impl SchedulerService`, before `list`:

```rust
    /// Owner and companion changes to automations (spec §9).
    pub(crate) fn automations(&self) -> AutomationService {
        AutomationService::new(
            Arc::clone(&self.inner.state),
            self.inner.runs.control_plane_transactions(),
        )
    }
```

4. Replace the bodies of `list`, `create`, `update`, and `delete` (keeping their signatures) with:

```text
    pub(crate) async fn list(…) -> Result<Vec<ScheduledPromptRecord>, ScheduleError> {
        self.automations().list(agent_id).await
    }

    pub(crate) async fn create(…) -> Result<(ScheduledPromptRecord, bool), ScheduleError> {
        let mut input = AutomationInput::owner(agent_id, prompt, trigger, target);
        input.enabled = enabled;
        input.import_idempotency_key = import_idempotency_key;
        input.explicit_next_due_at_ms = explicit_next_due_at_ms;
        input.created_at_override_ms = created_at_override_ms;
        self.automations().create(input, now_ms()).await
    }

    pub(crate) async fn update(…) -> Result<ScheduledPromptRecord, ScheduleError> {
        let patch = AutomationPatch {
            prompt,
            trigger,
            target,
            enabled,
            ..AutomationPatch::default()
        };
        self.automations()
            .update(agent_id, schedule_id, patch, now_ms())
            .await
    }

    pub(crate) async fn delete(…) -> Result<(), ScheduleError> {
        self.automations().delete(agent_id, schedule_id).await
    }
```

(`…` stands for each method's existing parameter list, unchanged.) Task 3's `name: automations::default_name(&prompt), …` lines in the old `create` literal go with the old body.

5. `validate_prompt`, `validate_target`, `validate_trigger`, and `next_schedule_id` stay private functions of `schedules.rs`; `automations.rs` reaches them as a child module.

In `hosts/rust-daemon/src/routes/schedules.rs`'s `schedule_error`, after the `Rejected` arm:

```text
        ScheduleError::Conflict(message) => {
            error_response(StatusCode::CONFLICT, "schedule_conflict", message)
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules skills:: 2>&1 | tail -30`
Expected: PASS (11 new service tests, every existing schedule and skill test).

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test schedule_api 2>&1 | tail -30`
Expected: PASS (create, update, delete, and the legacy import behave as before).

- [ ] **Step 7: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/schedules/automations.rs hosts/rust-daemon/src/schedules/timing.rs hosts/rust-daemon/src/skills/mod.rs hosts/rust-daemon/src/routes/schedules.rs
git commit -m "feat(daemon): add the automation service with limits, hidden-text checks, and the heartbeat preset"
```

Recommended implementer tier: most capable (drop-safe changes under the transaction, the revert paths, and the limits; the code is given, the risk is in wiring it without a lost revert).

---

### Task 6: The scheduler: active hours and `once` at claim, outcomes with counters and fire records, and Run now

**Files:**

- Create: `hosts/rust-daemon/src/schedules/run_now_tests.rs`
- Modify: `hosts/rust-daemon/src/schedules.rs` (`claim_due`, `execute_claimed`, `record_outcome`, `reconcile_interrupted`, `tick_inner`'s reaping, `run_now`, `ScheduleError::Busy`, test helpers made `pub(super)`, the one test call of `record_outcome`), `hosts/rust-daemon/src/schedules/automations.rs` (two strings), `hosts/rust-daemon/src/state.rs` (drop the `allow` on the `OutcomeUndo` re-export), `hosts/rust-daemon/src/routes/schedules.rs` (one error arm)

**Interfaces:**

- Consumes: Task 2's `next_due_after_claim`; Task 3's `record_automation_outcome`, `undo_automation_outcome`, `publish_automation_updated`, `OutcomeUndo`, `ScheduleLastFired.manual`, `test_automation`; Task 5's `AutomationInput`, `AutomationService`; `AgentRunCoordinator::run_with_commit_waiting`; `RunLedger::find_by_idempotency_key`.
- Produces:
  - `AUTOMATION_ALREADY_RUNNING`, `TOO_MANY_RUNNING_AUTOMATIONS` in `schedules/automations.rs`; `ScheduleError::Busy(&'static str)` (429, code `schedule_busy`).
  - `SchedulerService::run_now(&self, agent_id, schedule_id) -> Result<ScheduledPromptRecord, ScheduleError>` (the claimed record); `#[cfg(test)] SchedulerService::drain(&self)` (awaits every job).
  - `record_outcome(inner, id, status, error_code, now, run_key: Option<&str>)` (gains `run_key`, the occurrence's run idempotency key, to name its run in the fire record).
- Behavior:
  - A claim moves the due time with the automation's active hours (`next_due_after_claim(trigger, active_hours, previous_due, now)`); a `once` claim turns the automation off; an automation whose next fire cannot be computed runs this occurrence and is turned off, with a warning, instead of closing admission for every automation. Each saved claim announces `automation.updated`.
  - Every recorded outcome (the commit hook, a run that failed to commit, an unavailable target, restart reconciliation) goes through `record_automation_outcome` in the same save as before, so the counters and a fire record are saved with it; the commit's rollback, and each failed save, undo exactly that. Each saved outcome announces `automation.updated`.
  - Run now (spec §9.2): in its own task, so a dropped request still finishes its claim and starts its job. Under the scheduler's `jobs` mutex (taken first, as `tick_inner` does): `404` for another agent's or an unknown automation; `409` (`AUTOMATION_ALREADY_RUNNING`) when its job is live or its previous occurrence has no outcome yet; `429` (`TOO_MANY_RUNNING_AUTOMATIONS`) at `MAX_ACTIVE_SCHEDULES` live jobs. The claim saves `lastFired { firedAtMs, runIdempotencyKey: "schedule:<id>:manual:<ms>", manual: true }` and clears `lastOutcome`, but never moves `nextDueAtMs` or changes `enabled` (a paused or finished one-time automation can be run now). The job then runs exactly as a scheduled occurrence (single flight, the global run permits, the target rules), and a restart during it disables the automation like any interrupted occurrence.

- [ ] **Step 1: Write the failing tests**

In `hosts/rust-daemon/src/schedules.rs`'s test module, make these helpers reachable from a sibling test module by changing their visibility to `pub(super)` (no other change): `struct NoopTelegram`, `fn service`, `fn service_with_daemon`, `struct GatedModel` and both of its fields (`pub(super) entered`, `pub(super) release`), and `async fn due_schedule`. In `reconciliation_preserves_completed_and_never_claimed_schedules`, add `None,` as the last argument of the `record_outcome(…)` call.

Add to `hosts/rust-daemon/src/schedules.rs`, after `mod tests { … }`:

```text
#[cfg(test)]
mod run_now_tests;
```

Create `hosts/rust-daemon/src/schedules/run_now_tests.rs`:

```rust
//! Run now, active hours and `once` at claim, and outcomes with counters,
//! fire records, and `automation.updated` (spec §9.1, §9.2).

use std::sync::Arc;

use tokio::sync::Semaphore;

use super::tests::{due_schedule, service, service_with_daemon, GatedModel};
use super::*;
use crate::agent_runs::test_support::next_event;
use crate::sessions::test_support::within;
use crate::state::DaemonState;

type Gated = (
    SchedulerService,
    SharedDaemonState,
    String,
    Arc<Semaphore>,
    Arc<Semaphore>,
);

/// A scheduler whose agent's model call waits for a `release` permit.
fn gated() -> Gated {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let daemon = DaemonState::with_model_adapter(Arc::new(GatedModel {
        entered: entered.clone(),
        release: release.clone(),
    }));
    let (service, state, agent_id, _) = service_with_daemon(daemon);
    (service, state, agent_id, entered, release)
}

/// An automation of `agent_id` next due in an hour.
async fn later(service: &SchedulerService, agent_id: &str) -> ScheduledPromptRecord {
    service
        .create(
            agent_id.into(),
            "Check status".into(),
            ScheduleTrigger::Interval {
                interval_ms: 60_000,
            },
            ScheduleTarget::Workspace,
            true,
            None,
            Some(now_ms() + 3_600_000),
            None,
        )
        .await
        .unwrap()
        .0
}

async fn entered_run(entered: &Semaphore) {
    within("the automation's run to reach its model", entered.acquire())
        .await
        .unwrap()
        .forget();
}

#[test]
fn the_run_now_strings_are_the_specs() {
    assert_eq!(AUTOMATION_ALREADY_RUNNING, "This automation is already running");
    assert_eq!(
        TOO_MANY_RUNNING_AUTOMATIONS,
        "Too many automations are running; try again shortly"
    );
}

#[tokio::test]
async fn run_now_fires_without_moving_the_due_time_and_is_recorded_as_manual() {
    let (service, state, agent_id, entered, release) = gated();
    let record = later(&service, &agent_id).await;
    let paused = service
        .update(&agent_id, &record.id, None, None, None, Some(false))
        .await
        .unwrap();
    release.add_permits(1);

    let claimed = service.run_now(&agent_id, &record.id).await.unwrap();
    entered_run(&entered).await;
    service.drain().await;

    assert!(claimed.last_fired.as_ref().unwrap().manual);
    let after = state.read().await.schedules[&record.id].clone();
    assert_eq!(after.next_due_at_ms, paused.next_due_at_ms, "the due time stays");
    assert!(!after.enabled, "a paused automation stays paused");
    assert_eq!(
        after.last_safe_outcome.as_ref().unwrap().status,
        ScheduleOutcomeStatus::Silent
    );
    assert_eq!(after.counters.runs, 1);
    let fires = state.read().await.schedule_fires.for_schedule(&record.id);
    assert_eq!(fires.len(), 1);
    assert!(fires[0].manual);
    assert!(fires[0]
        .id
        .starts_with(&format!("schedule:{}:manual:", record.id)));
    assert!(fires[0].run_id.is_some());
    assert_eq!(
        fires[0].session_id.as_deref(),
        Some(crate::sessions::schedule_room_id(&record.id).as_str())
    );
}

#[tokio::test]
async fn run_now_refuses_while_the_automation_runs() {
    let (service, _state, agent_id, entered, release) = gated();
    let record = later(&service, &agent_id).await;
    service.run_now(&agent_id, &record.id).await.unwrap();
    entered_run(&entered).await;

    assert_eq!(
        service.run_now(&agent_id, &record.id).await,
        Err(ScheduleError::Conflict(AUTOMATION_ALREADY_RUNNING))
    );

    release.add_permits(2);
    service.drain().await;
    service
        .run_now(&agent_id, &record.id)
        .await
        .expect("free again once its run finished");
    entered_run(&entered).await;
    service.drain().await;
}

#[tokio::test]
async fn run_now_keeps_ownership_and_the_admission_cap() {
    let (service, _state, agent_id, _, _) = gated();
    let record = later(&service, &agent_id).await;
    assert_eq!(
        service.run_now("someone-else", &record.id).await,
        Err(ScheduleError::NotFound)
    );
    assert_eq!(
        service.run_now(&agent_id, "missing").await,
        Err(ScheduleError::NotFound)
    );
    {
        let mut jobs = service.inner.jobs.lock().await;
        for n in 0..MAX_ACTIVE_SCHEDULES {
            jobs.insert(
                format!("busy-{n}"),
                tokio::spawn(std::future::pending::<()>()),
            );
        }
    }
    assert_eq!(
        service.run_now(&agent_id, &record.id).await,
        Err(ScheduleError::Busy(TOO_MANY_RUNNING_AUTOMATIONS))
    );
    for (_, job) in std::mem::take(&mut *service.inner.jobs.lock().await) {
        job.abort();
    }
}

#[tokio::test]
async fn a_once_automation_fires_once_and_turns_itself_off() {
    let (service, state, agent_id, entered, release) = gated();
    let at = now_ms() + 3_600_000;
    let (record, _) = service
        .automations()
        .create(
            AutomationInput::owner(
                agent_id.clone(),
                "Remind me".into(),
                ScheduleTrigger::Once { at_ms: at },
                ScheduleTarget::Workspace,
            ),
            now_ms(),
        )
        .await
        .unwrap();
    release.add_permits(1);

    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(at).await });
    entered_run(&entered).await;
    assert_eq!(tick.await.unwrap().unwrap(), 1);

    let fired = state.read().await.schedules[&record.id].clone();
    assert!(!fired.enabled, "the claim turned it off");
    assert_eq!(fired.next_due_at_ms, at);
    assert_eq!(fired.counters.runs, 1);
    assert_eq!(
        service.tick_at(at + 3_600_000).await.unwrap(),
        0,
        "it never fires again"
    );
}

#[tokio::test]
async fn active_hours_move_the_next_due_time_at_claim() {
    let (service, state, agent_id, entered, release) = gated();
    // 2026-01-05 21:30 UTC; the window closes at 22:00.
    let due = 1_767_648_600_000;
    let mut record = crate::schedules::test_automation(&agent_id, "windowed");
    record.trigger = ScheduleTrigger::Interval {
        interval_ms: 30 * 60_000,
    };
    record.active_hours = Some(ActiveHours {
        start: "08:00".into(),
        end: "22:00".into(),
        days: (0..=6).collect(),
        time_zone: "UTC".into(),
    });
    record.next_due_at_ms = due;
    state
        .write()
        .await
        .schedules
        .insert(record.id.clone(), record);
    release.add_permits(1);

    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(due + 60_000).await });
    entered_run(&entered).await;
    tick.await.unwrap().unwrap();

    assert_eq!(
        state.read().await.schedules["windowed"].next_due_at_ms,
        due + (10 * 60 + 30) * 60_000,
        "the next day's 08:00, not 22:00"
    );
}

#[tokio::test]
async fn outcomes_count_record_a_fire_and_announce() {
    let (service, state, agent_id, entered, release) = gated();
    let mut stream = state.read().await.live.subscribe(&agent_id).unwrap();
    let record = due_schedule(&service, &agent_id).await;
    release.add_permits(1);

    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(now_ms()).await });
    entered_run(&entered).await;
    tick.await.unwrap().unwrap();

    let after = state.read().await.schedules[&record.id].clone();
    assert_eq!(
        after.counters,
        AutomationCounters {
            runs: 1,
            failures: 0,
            consecutive_failures: 0
        }
    );
    let fires = state.read().await.schedule_fires.for_schedule(&record.id);
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].outcome, ScheduleOutcomeStatus::Silent);
    assert!(!fires[0].manual);
    // Created, claimed, and finished.
    let mut announced = 0;
    while announced < 3 {
        let event = next_event(&mut stream).await.to_json(1);
        if event["type"] == "automation.updated" && event["scheduleId"] == record.id.as_str() {
            announced += 1;
        }
    }
}

#[tokio::test]
async fn a_failed_commit_save_rolls_back_the_outcome_counters_and_fire() {
    let (service, state, agent_id, entered, release) = gated();
    let record = due_schedule(&service, &agent_id).await;
    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(now_ms()).await });
    entered_run(&entered).await;
    let directory =
        std::env::temp_dir().join(format!("anima-fire-rollback-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    state.write().await.set_control_plane_store(Some(
        crate::control_plane_store::ControlPlaneStoreConfig::Json(directory.clone()),
    ));
    release.add_permits(1);
    tick.await.unwrap().unwrap();

    let after = state.read().await.schedules[&record.id].clone();
    assert!(after.last_safe_outcome.is_none());
    assert_eq!(after.counters, AutomationCounters::default());
    assert!(state
        .read()
        .await
        .schedule_fires
        .for_schedule(&record.id)
        .is_empty());
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn restart_reconciliation_records_a_failed_fire() {
    let (service, state, agent_id, _) = service();
    let record = due_schedule(&service, &agent_id).await;
    claim_due(&service.inner, &record.id, now_ms())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(service.tick_at(now_ms()).await.unwrap(), 0);

    let after = state.read().await.schedules[&record.id].clone();
    assert!(!after.enabled);
    assert_eq!(
        after.counters,
        AutomationCounters {
            runs: 1,
            failures: 1,
            consecutive_failures: 1
        }
    );
    let fires = state.read().await.schedule_fires.for_schedule(&record.id);
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].outcome, ScheduleOutcomeStatus::Failed);
    assert_eq!(
        fires[0].error_code.as_deref(),
        Some("schedule_run_interrupted")
    );
    assert_eq!(fires[0].run_id, None, "no run started before the restart");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::run_now_tests 2>&1 | tail -30`
Expected: FAIL to compile (`run_now`, `drain`, `ScheduleError::Busy`, and the two strings do not exist).

- [ ] **Step 3: The strings, the error, and the routes' arm**

Add to `hosts/rust-daemon/src/schedules/automations.rs`, after `HEARTBEAT_NEEDS_TIME_ZONE`:

```rust
pub(crate) const AUTOMATION_ALREADY_RUNNING: &str = "This automation is already running";
pub(crate) const TOO_MANY_RUNNING_AUTOMATIONS: &str =
    "Too many automations are running; try again shortly";
```

In `hosts/rust-daemon/src/schedules.rs`, add `AUTOMATION_ALREADY_RUNNING, TOO_MANY_RUNNING_AUTOMATIONS` to the `automations` re-export, and to `ScheduleError` after `Conflict(&'static str),`:

```text
    /// The scheduler is at its admission cap (429).
    Busy(&'static str),
```

In `hosts/rust-daemon/src/routes/schedules.rs`'s `schedule_error`, after the `Conflict` arm:

```text
        ScheduleError::Busy(message) => {
            error_response(StatusCode::TOO_MANY_REQUESTS, "schedule_busy", message)
        }
```

In `hosts/rust-daemon/src/state.rs`, delete the `#[allow(unused_imports)] // M6 Task 6's scheduler names it.` line above `pub(crate) use automation_state::OutcomeUndo;`.

- [ ] **Step 4: Claims, outcomes, reconciliation, and Run now in `schedules.rs`**

1. Add `use crate::state::OutcomeUndo;` to the imports.

2. Replace the reaping loop at the top of `tick_inner` (from `let finished = jobs` through the end of its `for id in finished { … }` loop) with `reap_finished(&mut jobs).await;`, and add after `drain_jobs`:

```rust
/// Awaits the jobs that finished, so their automations can run again.
async fn reap_finished(jobs: &mut BTreeMap<String, JoinHandle<()>>) {
    let finished = jobs
        .iter()
        .filter(|(_, job)| job.is_finished())
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in finished {
        if let Some(job) = jobs.remove(&id) {
            if let Err(error) = job.await {
                tracing::warn!(?error, "scheduled worker stopped unexpectedly");
            }
        }
    }
}
```

3. Add to `impl SchedulerService`, after `tick_at`:

```rust
    /// Awaits every job (tests drive Run now without the worker loop).
    #[cfg(test)]
    pub(crate) async fn drain(&self) {
        drain_jobs(&self.inner).await;
    }

    /// Fires an automation now (spec §9.2). Its due time and switch stay,
    /// one run per automation and the scheduler's admission cap still hold,
    /// and the fire is recorded as manual. The claim and the job's start run
    /// in their own task, so a dropped request still finishes them.
    pub(crate) async fn run_now(
        &self,
        agent_id: &str,
        schedule_id: &str,
    ) -> Result<ScheduledPromptRecord, ScheduleError> {
        let inner = Arc::clone(&self.inner);
        let (agent_id, schedule_id) = (agent_id.to_string(), schedule_id.to_string());
        tokio::spawn(async move { run_now_inner(&inner, &agent_id, &schedule_id, now_ms()).await })
            .await
            .unwrap_or(Err(ScheduleError::Persistence))
    }
```

and after `reap_finished`:

```rust
async fn run_now_inner(
    inner: &Arc<SchedulerInner>,
    agent_id: &str,
    schedule_id: &str,
    now: u64,
) -> Result<ScheduledPromptRecord, ScheduleError> {
    let owned = inner
        .state
        .read()
        .await
        .schedules
        .get(schedule_id)
        .is_some_and(|record| record.agent_id == agent_id);
    if !owned {
        return Err(ScheduleError::NotFound);
    }
    // The scheduler's lock order: its jobs, then the transaction.
    let mut jobs = inner.jobs.lock().await;
    reap_finished(&mut jobs).await;
    if jobs.contains_key(schedule_id) {
        return Err(ScheduleError::Conflict(AUTOMATION_ALREADY_RUNNING));
    }
    if jobs.len() >= MAX_ACTIVE_SCHEDULES {
        return Err(ScheduleError::Busy(TOO_MANY_RUNNING_AUTOMATIONS));
    }
    let record = claim_manual(inner, agent_id, schedule_id, now).await?;
    let job = {
        let (inner, record) = (Arc::clone(inner), record.clone());
        tokio::spawn(async move { execute_claimed(&inner, record, now).await })
    };
    jobs.insert(schedule_id.to_string(), job);
    Ok(record)
}

/// Saves a manual occurrence's claim: `lastFired` (marked manual) and no
/// outcome yet; the due time and the switch stay.
async fn claim_manual(
    inner: &Arc<SchedulerInner>,
    agent_id: &str,
    schedule_id: &str,
    now: u64,
) -> Result<ScheduledPromptRecord, ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (claimed, previous, persist) = {
        let mut state = inner.state.write().await;
        let previous = state
            .schedules
            .get(schedule_id)
            .filter(|record| record.agent_id == agent_id)
            .cloned()
            .ok_or(ScheduleError::NotFound)?;
        if unresolved_occurrence(&previous) {
            return Err(ScheduleError::Conflict(AUTOMATION_ALREADY_RUNNING));
        }
        let mut claimed = previous.clone();
        claimed.last_fired = Some(ScheduleLastFired {
            fired_at_ms: now,
            run_idempotency_key: format!("schedule:{}:manual:{now}", claimed.id),
            manual: true,
        });
        claimed.last_safe_outcome = None;
        claimed.updated_at_ms = now
            .max(claimed.created_at_ms)
            .max(previous.updated_at_ms);
        state
            .schedules
            .insert(schedule_id.to_string(), claimed.clone());
        (claimed, previous, state.control_plane_persist_request())
    };
    if persist.save().await.is_err() {
        inner
            .state
            .write()
            .await
            .schedules
            .insert(schedule_id.to_string(), previous);
        return Err(ScheduleError::Persistence);
    }
    inner
        .state
        .read()
        .await
        .publish_automation_updated(agent_id, schedule_id, false);
    Ok(claimed)
}
```

4. Replace `claim_due` with:

```rust
async fn claim_due(
    inner: &Arc<SchedulerInner>,
    id: &str,
    now: u64,
) -> Result<Option<ScheduledPromptRecord>, ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (claimed, previous, persist) = {
        let mut state = inner.state.write().await;
        let Some(previous) = state
            .schedules
            .get(id)
            .filter(|item| item.enabled && item.next_due_at_ms <= now)
            .cloned()
        else {
            return Ok(None);
        };
        let mut claimed = previous.clone();
        match next_due_after_claim(
            &claimed.trigger,
            claimed.active_hours.as_ref(),
            previous.next_due_at_ms,
            now,
        ) {
            Ok(next) => claimed.next_due_at_ms = next,
            // One automation that cannot compute its next fire must not
            // close admission for every other: this occurrence runs, then
            // it stays off until the owner edits it.
            Err(error) => {
                tracing::warn!(schedule_id = %claimed.id, ?error, "an automation has no next fire time; it is turned off after this occurrence");
                claimed.enabled = false;
            }
        }
        if matches!(claimed.trigger, ScheduleTrigger::Once { .. }) {
            // Spec §9.1: a one-time automation fires once, then turns itself off.
            claimed.enabled = false;
        }
        claimed.last_fired = Some(ScheduleLastFired {
            fired_at_ms: now,
            run_idempotency_key: format!("schedule:{}:{}", claimed.id, now),
            manual: false,
        });
        claimed.last_safe_outcome = None;
        claimed.updated_at_ms = now.max(claimed.created_at_ms);
        state.schedules.insert(id.to_string(), claimed.clone());
        (claimed, previous, state.control_plane_persist_request())
    };
    if persist.save().await.is_err() {
        inner
            .state
            .write()
            .await
            .schedules
            .insert(id.to_string(), previous);
        return Err(ScheduleError::Persistence);
    }
    inner
        .state
        .read()
        .await
        .publish_automation_updated(&claimed.agent_id, &claimed.id, false);
    Ok(Some(claimed))
}
```

5. In `execute_claimed`: before `let schedule_id = record.id.clone();` add `let run_key = record.last_fired.as_ref().map(|fired| fired.run_idempotency_key.clone());`; change the target-unavailable `record_outcome(…)` call to pass `None` as its new last argument; and replace everything from `let schedule_id = record.id.clone();` to the end of the function with:

```rust
    let schedule_id = record.id.clone();
    let agent_id = record.agent_id.clone();
    let target = record.target.clone();
    let request = AgentRunRequest {
        agent_id: record.agent_id.clone(),
        content: Content {
            text: wrap_checkin_prompt(&record.prompt),
            metadata: Some(BTreeMap::from([
                ("kind".into(), DataValue::String("checkin".into())),
                ("id".into(), DataValue::String(record.id.clone())),
            ])),
            attachments: None,
        },
        room,
        idempotency_key: run_key.clone(),
        source: RunSource::Schedule,
        source_ref: Some(record.id.clone()),
        parent: None,
    };
    // What the commit hook did, so the rollback undoes exactly that.
    let recorded = Arc::new(std::sync::Mutex::new(
        None::<(Option<OutcomeUndo>, Option<TelegramOutboundRecord>)>,
    ));
    let commit_recorded = Arc::clone(&recorded);
    let result = inner
        .runs
        .run_with_commit_waiting(
            request,
            move |state, outcome| {
                let result = &outcome.result;
                let status = checkin_outcome_status(outcome);
                if !state.schedules.contains_key(&schedule_id) {
                    return Err(ApiError::not_found());
                }
                let mut outbound = None;
                if status == ScheduleOutcomeStatus::Spoke {
                    if let ScheduleTarget::Connector { connector_id } = &target {
                        let connector = state
                            .connectors
                            .get(connector_id)
                            .filter(|item| item.is_active() && item.approved_chat.is_some())
                            .ok_or_else(ApiError::not_found)?;
                        let (Some(reply_id), Some(reply)) =
                            (outcome.reply_message_id.as_ref(), result.data.as_ref())
                        else {
                            return Err(ApiError::bad_request(
                                "agent produced no assistant message",
                            ));
                        };
                        if !is_silent_checkin_reply(&reply.text) {
                            let item = TelegramOutboundRecord {
                                id: format!(
                                    "telegram:{}:schedule:{}:{}",
                                    connector_id, schedule_id, reply_id
                                ),
                                connector_id: connector_id.clone(),
                                agent_id: connector.agent_id.clone(),
                                room_id: connector.room_id.clone(),
                                assistant_message_id: reply_id.clone(),
                                text: reply.text.clone(),
                                created_at_ms: now,
                                delivered_at_ms: None,
                                attempts: 0,
                                delivery_state: OutboundDeliveryState::Pending,
                                message_pruned: false,
                            };
                            state
                                .outbound
                                .entry(item.id.clone())
                                .or_insert_with(|| item.clone());
                            outbound = Some(item);
                        }
                    }
                }
                // Spec §9.1: the outcome, the counters, and a fire record,
                // in the commit's save.
                let undo = state.record_automation_outcome(
                    &schedule_id,
                    ScheduleSafeOutcome {
                        error_code: checkin_error_code(&status),
                        status,
                        occurred_at_ms: now,
                    },
                    Some((outcome.run_id.clone(), outcome.session_id.clone())),
                    now_ms(),
                );
                *commit_recorded.lock().unwrap_or_else(|p| p.into_inner()) = Some((undo, outbound));
                Ok(())
            },
            move |state| {
                let done = recorded.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some((undo, outbound)) = done {
                    if let Some(outbound) = outbound {
                        if state.outbound.get(&outbound.id) == Some(&outbound) {
                            state.outbound.remove(&outbound.id);
                        }
                    }
                    if let Some(undo) = undo {
                        state.undo_automation_outcome(undo);
                    }
                }
                Ok(())
            },
        )
        .await;
    if result.is_err() {
        let _ = record_outcome(
            inner,
            &record.id,
            ScheduleOutcomeStatus::Failed,
            Some("schedule_run_failed"),
            now,
            run_key.as_deref(),
        )
        .await;
    } else {
        inner
            .state
            .read()
            .await
            .publish_automation_updated(&agent_id, &record.id, false);
    }
}
```

6. Replace `record_outcome` with:

```rust
/// Records an occurrence's outcome outside a run's commit (a run that did
/// not commit, an unavailable target): the outcome, the counters, and a fire
/// record naming the occurrence's run, if one started (`run_key`).
async fn record_outcome(
    inner: &Arc<SchedulerInner>,
    id: &str,
    status: ScheduleOutcomeStatus,
    error_code: Option<&str>,
    now: u64,
    run_key: Option<&str>,
) -> Result<(), ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (agent_id, undo, persist) = {
        let mut state = inner.state.write().await;
        let agent_id = state
            .schedules
            .get(id)
            .map(|record| record.agent_id.clone())
            .ok_or(ScheduleError::NotFound)?;
        let run = run_key
            .and_then(|key| state.runs.find_by_idempotency_key(&agent_id, key, 0))
            .map(|run| (run.id.clone(), run.session_id.clone()));
        let undo = state
            .record_automation_outcome(
                id,
                ScheduleSafeOutcome {
                    status,
                    occurred_at_ms: now,
                    error_code: error_code.map(str::to_string),
                },
                run,
                now_ms(),
            )
            .ok_or(ScheduleError::NotFound)?;
        (agent_id, undo, state.control_plane_persist_request())
    };
    if persist.save().await.is_err() {
        inner.state.write().await.undo_automation_outcome(undo);
        return Err(ScheduleError::Persistence);
    }
    inner
        .state
        .read()
        .await
        .publish_automation_updated(&agent_id, id, false);
    Ok(())
}
```

7. Replace `reconcile_interrupted` with:

```rust
async fn reconcile_interrupted(
    inner: &Arc<SchedulerInner>,
    now: u64,
    active_schedules: &BTreeSet<String>,
) -> Result<(), ScheduleError> {
    let _transaction = inner.runs.control_plane_transaction().await;
    let (changes, persist) = {
        let mut state = inner.state.write().await;
        // Occurrences whose run the owner stopped before a restart could
        // record its outcome: the saved stop survives the restart on the
        // interrupted run, and a stop keeps the schedule enabled (spec §4.6,
        // audit M24).
        let unresolved = state
            .schedules
            .values()
            .filter(|s| !active_schedules.contains(&s.id) && unresolved_occurrence(s))
            .map(|s| {
                let fired = s
                    .last_fired
                    .as_ref()
                    .expect("an unresolved occurrence fired");
                let run = state
                    .runs
                    .find_by_idempotency_key(&s.agent_id, &fired.run_idempotency_key, 0)
                    .filter(|run| {
                        run.source == RunSource::Schedule
                            && run.source_ref.as_deref() == Some(s.id.as_str())
                    });
                (
                    s.id.clone(),
                    s.agent_id.clone(),
                    fired.fired_at_ms,
                    run.is_some_and(|run| run.stop.is_some()),
                    run.map(|run| (run.id.clone(), run.session_id.clone())),
                )
            })
            .collect::<Vec<_>>();
        if unresolved.is_empty() {
            return Ok(());
        }
        let mut changes = Vec::new();
        for (id, agent_id, fired_at_ms, stopped, run) in unresolved {
            let previous = state.schedules[&id].clone();
            let occurred_at_ms = now.max(fired_at_ms);
            let outcome = if stopped {
                let status = ScheduleOutcomeStatus::Stopped;
                ScheduleSafeOutcome {
                    error_code: checkin_error_code(&status),
                    status,
                    occurred_at_ms,
                }
            } else {
                ScheduleSafeOutcome {
                    status: ScheduleOutcomeStatus::Failed,
                    occurred_at_ms,
                    error_code: Some("schedule_run_interrupted".into()),
                }
            };
            let undo = state.record_automation_outcome(&id, outcome, run, now);
            let schedule = state.schedules.get_mut(&id).expect("just recorded");
            if !stopped {
                schedule.enabled = false;
            }
            schedule.updated_at_ms = now.max(schedule.updated_at_ms);
            changes.push((previous, undo, agent_id));
        }
        (changes, state.control_plane_persist_request())
    };
    if persist.save().await.is_err() {
        let mut state = inner.state.write().await;
        for (previous, undo, _) in changes {
            if let Some(undo) = undo {
                state.undo_automation_outcome(undo);
            }
            state.schedules.insert(previous.id.clone(), previous);
        }
        return Err(ScheduleError::Persistence);
    }
    let state = inner.state.read().await;
    for (previous, _, agent_id) in &changes {
        state.publish_automation_updated(agent_id, &previous.id, false);
    }
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules 2>&1 | tail -30`
Expected: PASS (9 new tests and every existing scheduler test, including the stop, restart, connector, and admission tests).

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- connectors::runtime routes::tests::runs 2>&1 | tail -30`
Expected: PASS (schedule-sourced runs elsewhere are unaffected).

- [ ] **Step 6: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/schedules/run_now_tests.rs hosts/rust-daemon/src/schedules/automations.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/routes/schedules.rs
git commit -m "feat(daemon): record automation outcomes with counters and fires, honor active hours and once, and add run now"
```

Recommended implementer tier: most capable (commit and rollback hooks, the scheduler's lock order, and restart reconciliation).

---

### Task 7: Automation routes and contracts: new fields, preview, history, Run now, and the README

**Files:**

- Create: `hosts/rust-daemon/src/routes/tests/automations.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/schedules.rs`, `hosts/rust-daemon/src/routes/schedules.rs`, `hosts/rust-daemon/src/routes/mod.rs` (three routes, three `ApiDoc` paths, one test module line), `hosts/rust-daemon/src/schedules.rs` (`is_running`, `ScheduleError::HistoryUnavailable`, re-exports), `hosts/rust-daemon/src/schedules/automations.rs` (`AutomationService::history`, two strings), `hosts/rust-daemon/tests/schedule_api.rs` (owner headers on the two list reads, one 403 assertion, three OpenAPI paths), `hosts/rust-daemon/README.md` (the Automations section)

**Interfaces:**

- Consumes: Tasks 2–6 (`ScheduleTrigger::{Cron, Once}`, `ActiveHours`, `display_name`, `AutomationCreator`, `AutomationPreset`, `AutomationCounters`, `AutomationInput`, `AutomationPatch`, `heartbeat_input`, `preview`, `AutomationService`, `SchedulerService::{automations, run_now}`, `ScheduleFireRecord`, `HistoryStore::page_schedule_runs`); `routes::http::request_query`.
- Produces:
  - Contracts: `ScheduleTriggerRequest`/`Response` gain `cron { expression, timeZone }` and `once { atMs }`; `ActiveHoursBody { start, end, days, timeZone }` (request and response); `PresetRequest { Heartbeat }`; `ScheduleCreateRequest` gains `name`, `activeHours`, `preset`, `timeZone`, and makes `prompt`, `trigger`, and `target` optional (`target` defaults to `workspace`); `ScheduleUpdateRequest` gains `name` and `activeHours` (`null` clears); `ScheduleResponse` gains `name`, `activeHours`, `createdBy` (`{ kind: "owner" }` or `{ kind: "agent", agentId, sessionId, runId, toolCallId }`), `preset`, `counters { runs, failures, consecutiveFailures }`, and `running`; `SchedulePreviewRequest { trigger, activeHours? }`, `SchedulePreviewResponse { nextRuns }`, `ScheduleRunsEnvelope { runs: [ScheduleRunResponse { id, scheduleId, agentId, firedAtMs, finishedAtMs, outcome, runId, sessionId, errorCode, manual }] }`.
  - Handlers `routes::schedules::{run_schedule, schedule_history, preview_schedule}` at the routes of the Global Constraints; `list_schedules` now authorizes owner reads.
  - `schedules::is_running(&ScheduledPromptRecord) -> bool` (its occurrence has no outcome yet); `ScheduleError::HistoryUnavailable` (503, code `schedule_history_unavailable`); `AutomationService::history(agent_id, schedule_id, limit) -> Result<Vec<ScheduleFireRecord>, ScheduleError>`.
  - Strings in `schedules/automations.rs`: `PROMPT_AND_TRIGGER_REQUIRED = "prompt and trigger are required unless preset is heartbeat"`, `AUTOMATION_HISTORY_UNAVAILABLE = "automation history is unavailable"`; in `routes/schedules.rs`: `HISTORY_LIMIT_INVALID = "limit must be from 1 to 50"`.
- Behavior: a create with `preset: "heartbeat"` starts from the preset in `timeZone` and takes any `prompt`, `trigger`, `name`, `activeHours`, or `enabled` the request also names; without a preset, `prompt` and `trigger` are required (`400`). `timeZone` is read only with a preset. History merges the control plane's unmirrored fires with the store's, deduplicated by id, newest first.

- [ ] **Step 1: Write the failing tests**

Add to `hosts/rust-daemon/src/routes/mod.rs`'s test module, after `mod approvals;`:

```text
    mod automations;
```

Create `hosts/rust-daemon/src/routes/tests/automations.rs`:

```rust
use super::*;

use serde_json::{json, Value};

use crate::agent_runs::test_support::{companion_config, ScriptedModel, Step};
use crate::history::conformance::{fire_record, FlakyHistoryStore};
use crate::history::HistoryService;
use crate::schedules::{
    AutomationInput, ScheduleLastFired, ScheduleTarget, ScheduleTrigger, HEARTBEAT_NEEDS_TIME_ZONE,
    MAX_AUTOMATIONS_PER_AGENT, PROMPT_AND_TRIGGER_REQUIRED, TOO_MANY_AUTOMATIONS,
};

const OWNER_ORIGIN: &str = "http://localhost:4200";

fn request(method: &str, uri: &str, origin: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(method, uri, OWNER_ORIGIN, body))
        .await
        .unwrap()
}

fn daemon_with(daemon: DaemonState) -> (Arc<RwLock<DaemonState>>, String) {
    let mut daemon = daemon;
    let agent = daemon
        .create_agent(companion_config("companion"))
        .unwrap()
        .state
        .id;
    (Arc::new(RwLock::new(daemon)), agent)
}

fn hourly() -> Value {
    json!({
        "prompt": "Check status",
        "trigger": {"type": "interval", "intervalMs": 3_600_000},
        "target": {"type": "workspace"}
    })
}

#[tokio::test]
async fn the_automation_routes_require_the_owner() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    for (method, uri, body) in [
        ("GET", format!("/api/agents/{agent}/schedules"), None),
        ("GET", format!("/api/agents/{agent}/schedules/s1/history"), None),
        ("POST", format!("/api/agents/{agent}/schedules/s1/run"), None),
        (
            "POST",
            "/api/schedules/preview".to_string(),
            Some(json!({"trigger": {"type": "interval", "intervalMs": 60_000}})),
        ),
    ] {
        let refused = app
            .clone()
            .oneshot(request(method, &uri, "https://untrusted.example", body))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(refused.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn a_cron_automation_with_a_name_and_active_hours_lists_its_new_fields() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let created = send(
        &app,
        "POST",
        &base,
        Some(json!({
            "name": "Weekday brief",
            "prompt": "Summarize my day",
            "trigger": {"type": "cron", "expression": "0 9 * * 1-5", "timeZone": "Europe/London"},
            "activeHours": {"start": "08:00", "end": "18:00", "days": [5, 1, 2, 3, 4], "timeZone": "Europe/London"}
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let schedule = json_body(created).await["schedule"].clone();
    assert_eq!(schedule["name"], "Weekday brief");
    assert_eq!(schedule["trigger"]["type"], "cron");
    assert_eq!(schedule["target"]["type"], "workspace", "the default target");
    assert_eq!(schedule["activeHours"]["days"], json!([1, 2, 3, 4, 5]));
    assert_eq!(schedule["createdBy"], json!({"kind": "owner"}));
    assert_eq!(schedule["preset"], Value::Null);
    assert_eq!(
        schedule["counters"],
        json!({"runs": 0, "failures": 0, "consecutiveFailures": 0})
    );
    assert_eq!(schedule["running"], false);

    let listed = send(&app, "GET", &base, None).await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(listed.headers()["cache-control"], "no-store");
    assert_eq!(json_body(listed).await["schedules"][0]["id"], schedule["id"]);

    let bad = send(
        &app,
        "POST",
        &base,
        Some(json!({
            "prompt": "x",
            "trigger": {"type": "cron", "expression": "61 * * * *", "timeZone": "UTC"}
        })),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(bad).await["error"], "minute: must be from 0 to 59");
}

#[tokio::test]
async fn the_heartbeat_preset_takes_the_owners_overrides() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let created = send(
        &app,
        "POST",
        &base,
        Some(json!({"preset": "heartbeat", "timeZone": "UTC", "prompt": "Anything urgent?"})),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let schedule = json_body(created).await["schedule"].clone();
    assert_eq!(schedule["preset"], "heartbeat");
    assert_eq!(schedule["name"], "Heartbeat");
    assert_eq!(schedule["prompt"], "Anything urgent?");
    assert_eq!(schedule["trigger"]["intervalMs"], 1_800_000);
    assert_eq!(schedule["activeHours"]["start"], "08:00");
    assert_eq!(schedule["activeHours"]["end"], "22:00");

    for (body, problem) in [
        (json!({"preset": "heartbeat"}), HEARTBEAT_NEEDS_TIME_ZONE),
        (json!({"prompt": "Check"}), PROMPT_AND_TRIGGER_REQUIRED),
    ] {
        let refused = send(&app, "POST", &base, Some(body)).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(refused).await["error"], problem);
    }
}

#[tokio::test]
async fn patch_renames_and_sets_then_clears_active_hours() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let item = format!("{base}/{id}");

    let set = json_body(
        send(
            &app,
            "PATCH",
            &item,
            Some(json!({
                "name": "Status",
                "activeHours": {"start": "09:00", "end": "17:00", "days": [1], "timeZone": "UTC"}
            })),
        )
        .await,
    )
    .await;
    assert_eq!(set["schedule"]["name"], "Status");
    assert_eq!(set["schedule"]["activeHours"]["start"], "09:00");

    let cleared = json_body(
        send(&app, "PATCH", &item, Some(json!({"activeHours": null}))).await,
    )
    .await;
    assert_eq!(cleared["schedule"]["activeHours"], Value::Null);
    assert_eq!(cleared["schedule"]["name"], "Status");
}

#[tokio::test]
async fn preview_lists_the_next_fires_or_the_triggers_problem() {
    let (state, _) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let preview = |body: Value| {
        let app = app.clone();
        async move { send(&app, "POST", "/api/schedules/preview", Some(body)).await }
    };

    let hourly = preview(json!({"trigger": {"type": "interval", "intervalMs": 3_600_000}})).await;
    assert_eq!(hourly.status(), StatusCode::OK);
    assert_eq!(hourly.headers()["cache-control"], "no-store");
    let runs = json_body(hourly).await["nextRuns"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 3);
    let first = runs[0].as_u64().unwrap();
    assert_eq!(runs[1].as_u64().unwrap(), first + 3_600_000);

    let once = preview(json!({"trigger": {"type": "once", "atMs": 4_102_444_800_000u64}})).await;
    assert_eq!(json_body(once).await["nextRuns"], json!([4_102_444_800_000u64]));

    let bad = preview(json!({"trigger": {"type": "cron", "expression": "* * *", "timeZone": "UTC"}})).await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(bad).await["code"], "schedule_invalid");
}

#[tokio::test]
async fn run_now_answers_202_and_records_a_manual_fire() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["CHECKIN_OK"])]);
    let (state, agent) = daemon_with(DaemonState::with_model_adapter(model));
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let accepted = send(&app, "POST", &format!("{base}/{id}/run"), None).await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    assert!(json_body(accepted).await["schedule"]["lastFiredAtMs"].is_u64());
    crate::sessions::test_support::within("the manual run to finish", async {
        while state.read().await.schedules[&id].last_safe_outcome.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await;

    let history = json_body(send(&app, "GET", &format!("{base}/{id}/history"), None).await).await;
    let runs = history["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["manual"], true);
    assert_eq!(runs[0]["outcome"], "silent");

    let missing = send(&app, "POST", &format!("{base}/missing/run"), None).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_now_conflicts_while_an_occurrence_has_no_outcome() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    state
        .write()
        .await
        .schedules
        .get_mut(&id)
        .unwrap()
        .last_fired = Some(ScheduleLastFired {
        fired_at_ms: 1,
        run_idempotency_key: format!("schedule:{id}:1"),
        manual: false,
    });

    let refused = send(&app, "POST", &format!("{base}/{id}/run"), None).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(refused).await["code"], "schedule_conflict");
    let listed = json_body(send(&app, "GET", &base, None).await).await;
    assert_eq!(listed["schedules"][0]["running"], true);
}

#[tokio::test]
async fn history_merges_pending_and_stored_fires_newest_first() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let store = state.read().await.history.store();
    store
        .upsert_schedule_runs(&[
            fire_record("old", &id, &agent, 10),
            fire_record("both", &id, &agent, 20),
        ])
        .await
        .unwrap();
    {
        let mut guard = state.write().await;
        guard.schedule_fires.record(fire_record("both", &id, &agent, 20));
        guard.schedule_fires.record(fire_record("new", &id, &agent, 30));
    }

    let page = json_body(send(&app, "GET", &format!("{base}/{id}/history?limit=2"), None).await)
        .await;
    let ids = page["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["new", "both"]);

    for limit in ["0", "51", "x"] {
        let refused =
            send(&app, "GET", &format!("{base}/{id}/history?limit={limit}"), None).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{limit}");
    }
    let unknown = send(&app, "GET", &format!("{base}/missing/history"), None).await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_failing_history_store_answers_503() {
    let mut daemon = DaemonState::new();
    let store = Arc::new(FlakyHistoryStore::new());
    daemon.set_history(HistoryService::new(store.clone()));
    let (state, agent) = daemon_with(daemon);
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    store.set_failing(true);

    let failed = send(&app, "GET", &format!("{base}/{id}/history"), None).await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failed.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(failed).await["code"],
        "schedule_history_unavailable"
    );
}

#[tokio::test]
async fn the_twenty_first_automation_answers_409_and_a_failed_save_503() {
    use crate::control_plane_store::ControlPlaneStoreConfig;
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let service = crate::schedules::AutomationService::new(
        state.clone(),
        Arc::new(tokio::sync::Mutex::new(())),
    );
    for _ in 0..MAX_AUTOMATIONS_PER_AGENT {
        service
            .create(
                AutomationInput::owner(
                    agent.clone(),
                    "Check".into(),
                    ScheduleTrigger::Interval {
                        interval_ms: 3_600_000,
                    },
                    ScheduleTarget::Workspace,
                ),
                anima_core::primitives::now_millis(),
            )
            .await
            .unwrap();
    }
    let refused = send(&app, "POST", &base, Some(hourly())).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(refused).await["error"], TOO_MANY_AUTOMATIONS);

    let id = state.read().await.schedules.keys().next().unwrap().clone();
    let invalid = std::env::temp_dir().join(format!(
        "anima-automation-route-invalid-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&invalid).unwrap();
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid.clone())));
    let failed = send(&app, "DELETE", &format!("{base}/{id}"), None).await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(state.read().await.schedules.contains_key(&id), "the automation stays");
    let _ = std::fs::remove_dir_all(invalid);
}
```

In `hosts/rust-daemon/tests/schedule_api.rs`:

1. In `schedule_crud_is_agent_scoped_and_mutations_require_local_owner`, give the `listed` request the owner headers (`.header("host", "127.0.0.1:8080").header("origin", "http://localhost:4200")`), and before it add:

```rust
    let unguarded_list = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&create_uri)
                .header("host", "127.0.0.1:8080")
                .header("origin", "https://untrusted.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        unguarded_list.status(),
        StatusCode::FORBIDDEN,
        "the list needs the owner since M6"
    );
```

2. In `legacy_import_is_idempotent_and_preserves_browser_due_time`, give the final `list` request the same owner headers.
3. In `openapi_registers_schedule_crud_and_import_contracts`, add:

```rust
    assert!(paths["/api/agents/{agent_id}/schedules/{schedule_id}/run"]
        .get("post")
        .is_some());
    assert!(paths["/api/agents/{agent_id}/schedules/{schedule_id}/history"]
        .get("get")
        .is_some());
    assert!(paths["/api/schedules/preview"].get("post").is_some());
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::automations 2>&1 | tail -30`
Expected: FAIL to compile (the new routes, contracts, strings, and `history` do not exist).

- [ ] **Step 3: The service's history, `is_running`, and the new error**

In `hosts/rust-daemon/src/schedules/automations.rs`, add after `TOO_MANY_RUNNING_AUTOMATIONS`:

```rust
pub(crate) const PROMPT_AND_TRIGGER_REQUIRED: &str =
    "prompt and trigger are required unless preset is heartbeat";
pub(crate) const AUTOMATION_HISTORY_UNAVAILABLE: &str = "automation history is unavailable";
```

add `use super::ScheduleFireRecord;` to the imports, and to `impl AutomationService`, after `get`:

```rust
    /// The automation's latest fires, newest first: the control plane's
    /// unmirrored ones merged with the history store's (spec §9.1).
    pub(crate) async fn history(
        &self,
        agent_id: &str,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<ScheduleFireRecord>, ScheduleError> {
        let (pending, store) = {
            let state = self.state.read().await;
            let owned = state.get_agent(agent_id).is_some()
                && state
                    .schedules
                    .get(schedule_id)
                    .is_some_and(|record| record.agent_id == agent_id);
            if !owned {
                return Err(ScheduleError::NotFound);
            }
            (
                state.schedule_fires.for_schedule(schedule_id),
                state.history.store(),
            )
        };
        let stored = store
            .page_schedule_runs(agent_id, schedule_id, limit)
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, schedule_id, "could not read an automation's history");
                ScheduleError::HistoryUnavailable
            })?;
        let mut merged = pending;
        for fire in stored {
            if !merged.iter().any(|known| known.id == fire.id) {
                merged.push(fire);
            }
        }
        merged.retain(|fire| fire.agent_id == agent_id);
        merged.sort_by(|left, right| {
            right
                .fired_at_ms
                .cmp(&left.fired_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        merged.truncate(limit);
        Ok(merged)
    }
```

In `hosts/rust-daemon/src/schedules.rs`:

1. Add `display_name, heartbeat_input, preview, AUTOMATION_HISTORY_UNAVAILABLE, MAX_AUTOMATION_HISTORY_SHOWN, PROMPT_AND_TRIGGER_REQUIRED` to the `automations` re-export (the routes and contracts use them), and add the names only tests use from outside the module:

```text
#[cfg(test)]
pub(crate) use automations::{
    AGENT_AUTOMATION_TOO_FREQUENT, HEARTBEAT_NEEDS_TIME_ZONE, MAX_AUTOMATIONS_PER_AGENT,
    TOO_MANY_AUTOMATIONS,
};
```

2. Add to `ScheduleError`, after `Busy(&'static str),`:

```text
    /// The history store could not be read (503).
    HistoryUnavailable,
```

3. After `unresolved_occurrence`, add:

```rust
/// Its latest occurrence has no outcome yet: it is running, or a restart
/// interrupted it and the next tick will record that.
pub(crate) fn is_running(record: &ScheduledPromptRecord) -> bool {
    unresolved_occurrence(record)
}
```

Delete the `#![allow(dead_code)] // M6 Task 7 uses every item.` lines of `hosts/rust-daemon/src/schedules/history.rs` and `hosts/rust-daemon/src/state/automation_state.rs` (the history route now reads `for_schedule`; every other item already has a caller). Add both files to this task's `git add`.

- [ ] **Step 4: The contracts**

In `hosts/rust-daemon/src/routes/contracts/schedules.rs`:

1. Replace the imports with:

```rust
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use crate::schedules::{
    display_name, is_running, ActiveHours, AutomationCreator, ScheduleFireRecord,
    ScheduleOutcomeStatus, ScheduleTarget, ScheduleTrigger, ScheduledPromptRecord,
};
```

2. Add to `ScheduleTriggerRequest`, after `Daily { … }`:

```rust
    Cron {
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    Once {
        #[serde(rename = "atMs")]
        at_ms: u64,
    },
```

and to its `From` impl:

```text
            ScheduleTriggerRequest::Cron {
                expression,
                time_zone,
            } => Self::Cron {
                expression,
                time_zone,
            },
            ScheduleTriggerRequest::Once { at_ms } => Self::Once { at_ms },
```

3. Add after the `ScheduleTargetRequest` `From` impl:

```rust
/// When an automation may fire (spec §9.1): `HH:MM` wall times, days with
/// 0 for Sunday, and the time zone they are read in.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActiveHoursBody {
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) days: Vec<u8>,
    pub(crate) time_zone: String,
}

impl From<ActiveHoursBody> for ActiveHours {
    fn from(value: ActiveHoursBody) -> Self {
        Self {
            start: value.start,
            end: value.end,
            days: value.days,
            time_zone: value.time_zone,
        }
    }
}

impl From<ActiveHours> for ActiveHoursBody {
    fn from(value: ActiveHours) -> Self {
        Self {
            start: value.start,
            end: value.end,
            days: value.days,
            time_zone: value.time_zone,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PresetRequest {
    Heartbeat,
}

/// `Some(None)` for an explicit `null`, so a PATCH can clear a field.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
```

4. Replace `ScheduleCreateRequest` and `ScheduleUpdateRequest` with:

```rust
/// Without `preset`, `prompt` and `trigger` are required; with
/// `preset: "heartbeat"`, `timeZone` is, and any field named here replaces
/// the preset's.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScheduleCreateRequest {
    pub(crate) prompt: Option<String>,
    pub(crate) trigger: Option<ScheduleTriggerRequest>,
    /// Defaults to the automation's own thread (`workspace`).
    pub(crate) target: Option<ScheduleTargetRequest>,
    pub(crate) enabled: Option<bool>,
    pub(crate) import_idempotency_key: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) active_hours: Option<ActiveHoursBody>,
    pub(crate) preset: Option<PresetRequest>,
    /// The owner's IANA time zone; read only with a preset.
    pub(crate) time_zone: Option<String>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScheduleUpdateRequest {
    pub(crate) prompt: Option<String>,
    pub(crate) trigger: Option<ScheduleTriggerRequest>,
    pub(crate) target: Option<ScheduleTargetRequest>,
    pub(crate) enabled: Option<bool>,
    pub(crate) name: Option<String>,
    /// `null` clears the active hours.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<ActiveHoursBody>)]
    pub(crate) active_hours: Option<Option<ActiveHoursBody>>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SchedulePreviewRequest {
    pub(crate) trigger: ScheduleTriggerRequest,
    pub(crate) active_hours: Option<ActiveHoursBody>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SchedulePreviewResponse {
    /// The next fire times, oldest first (one for a one-time automation).
    pub(crate) next_runs: Vec<u64>,
}
```

5. Add to `ScheduleTriggerResponse`'s `Cron`/`Once` nothing more (Task 2 added them). Add after `ScheduleOutcomeResponse`:

```rust
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum AutomationCreatorResponse {
    Owner,
    #[serde(rename_all = "camelCase")]
    Agent {
        agent_id: String,
        session_id: String,
        run_id: String,
        tool_call_id: String,
    },
}

impl From<AutomationCreator> for AutomationCreatorResponse {
    fn from(value: AutomationCreator) -> Self {
        match value {
            AutomationCreator::Owner => Self::Owner,
            AutomationCreator::Agent {
                agent_id,
                session_id,
                run_id,
                tool_call_id,
            } => Self::Agent {
                agent_id,
                session_id,
                run_id,
                tool_call_id,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationCountersResponse {
    pub(crate) runs: u64,
    pub(crate) failures: u64,
    pub(crate) consecutive_failures: u64,
}
```

6. Add to `ScheduleResponse`, after `updated_at_ms`:

```rust
    pub(crate) name: String,
    pub(crate) active_hours: Option<ActiveHoursBody>,
    pub(crate) created_by: AutomationCreatorResponse,
    /// `heartbeat` or null.
    pub(crate) preset: Option<String>,
    pub(crate) counters: AutomationCountersResponse,
    /// Its latest occurrence has no outcome yet.
    pub(crate) running: bool,
```

In `From<ScheduledPromptRecord> for ScheduleResponse`, compute before the `trigger` match `let name = display_name(&value); let running = is_running(&value);`, and add to the `Self { … }` literal:

```text
            name,
            active_hours: value.active_hours.map(Into::into),
            created_by: value.created_by.into(),
            preset: value.preset.map(|preset| preset.as_str().to_string()),
            counters: AutomationCountersResponse {
                runs: value.counters.runs,
                failures: value.counters.failures,
                consecutive_failures: value.counters.consecutive_failures,
            },
            running,
```

7. At the end of the file, add:

```rust
/// One occurrence (spec §9.1). `outcome` is `silent`, `spoke`, `failed`, or
/// `stopped` (the schedule's `lastOutcome.status` says `error` for a failure).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleRunResponse {
    pub(crate) id: String,
    pub(crate) schedule_id: String,
    pub(crate) agent_id: String,
    pub(crate) fired_at_ms: u64,
    pub(crate) finished_at_ms: u64,
    pub(crate) outcome: String,
    pub(crate) run_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) manual: bool,
}

impl From<ScheduleFireRecord> for ScheduleRunResponse {
    fn from(value: ScheduleFireRecord) -> Self {
        let outcome = match value.outcome {
            ScheduleOutcomeStatus::Silent => "silent",
            ScheduleOutcomeStatus::Spoke => "spoke",
            ScheduleOutcomeStatus::Failed => "failed",
            ScheduleOutcomeStatus::Stopped => "stopped",
        };
        Self {
            id: value.id,
            schedule_id: value.schedule_id,
            agent_id: value.agent_id,
            fired_at_ms: value.fired_at_ms,
            finished_at_ms: value.finished_at_ms,
            outcome: outcome.into(),
            run_id: value.run_id,
            session_id: value.session_id,
            error_code: value.error_code,
            manual: value.manual,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ScheduleRunsEnvelope {
    pub(crate) runs: Vec<ScheduleRunResponse>,
}
```

- [ ] **Step 5: The handlers and the router**

In `hosts/rust-daemon/src/routes/schedules.rs`:

1. Replace the imports with:

```rust
use anima_core::primitives::now_millis;
use axum::extract::{Path, Request as AxumRequest, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response as AxumResponse;

use crate::schedules::{
    heartbeat_input, legacy_next_due_at_ms, preview, AutomationInput, AutomationPatch,
    ScheduleError, ScheduleTarget, ScheduleTrigger, AUTOMATION_HISTORY_UNAVAILABLE,
    MAX_AUTOMATION_HISTORY_SHOWN, PROMPT_AND_TRIGGER_REQUIRED,
};

use super::contracts::{
    ConnectorErrorBody, DeleteResponse, LegacyScheduleImportRequest, PresetRequest,
    ScheduleCreateRequest, ScheduleEnvelope, SchedulePreviewRequest, SchedulePreviewResponse,
    ScheduleResponse, ScheduleRunsEnvelope, ScheduleUpdateRequest, SchedulesEnvelope,
};
use super::http::{json_response, read_limited_body, request_query, LocalOwnerRejection};
use super::{parse_json_body, AppState};

/// Spec §16: the history shows the latest 50.
pub(crate) const HISTORY_LIMIT_INVALID: &str = "limit must be from 1 to 50";
```

2. Replace `list_schedules`, `create_schedule`, and `update_schedule` with:

```rust
#[utoipa::path(get, path = "/api/agents/{agent_id}/schedules", tag = "schedules", params(("agent_id" = String, Path)), responses((status = 200, body = SchedulesEnvelope), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody)))]
pub(super) async fn list_schedules(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize_read(request.headers()) {
        return local_owner_error(rejection);
    }
    match state.scheduler.list(&agent_id).await {
        Ok(items) => no_store(json_response(
            StatusCode::OK,
            &SchedulesEnvelope {
                schedules: items.into_iter().map(Into::into).collect(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/schedules", tag = "schedules", params(("agent_id" = String, Path)), request_body = ScheduleCreateRequest, responses((status = 201, body = ScheduleEnvelope), (status = 200, body = ScheduleEnvelope), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 409, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
pub(super) async fn create_schedule(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid("malformed request"),
    };
    let request = match parse_json_body::<ScheduleCreateRequest>(body) {
        Ok(request) => request,
        Err(_) => return invalid("request body is invalid"),
    };
    let input = match automation_input(agent_id, request) {
        Ok(input) => input,
        Err(error) => return schedule_error(error),
    };
    match state
        .scheduler
        .automations()
        .create(input, now_millis())
        .await
    {
        Ok((record, created)) => no_store(json_response(
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            &ScheduleEnvelope {
                schedule: record.into(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

/// A create request as the service's input: the heartbeat preset's fields
/// unless the request names its own (spec §9.2).
fn automation_input(
    agent_id: String,
    request: ScheduleCreateRequest,
) -> Result<AutomationInput, ScheduleError> {
    let target = request
        .target
        .map(Into::into)
        .unwrap_or(ScheduleTarget::Workspace);
    let mut input = match request.preset {
        Some(PresetRequest::Heartbeat) => heartbeat_input(
            agent_id,
            request.time_zone.as_deref().unwrap_or_default(),
            target,
        )?,
        None => {
            let (Some(prompt), Some(trigger)) = (request.prompt.clone(), request.trigger.clone())
            else {
                return Err(ScheduleError::Invalid(PROMPT_AND_TRIGGER_REQUIRED));
            };
            AutomationInput::owner(agent_id, prompt, ScheduleTrigger::from(trigger), target)
        }
    };
    if let Some(prompt) = request.prompt {
        input.prompt = prompt;
    }
    if let Some(trigger) = request.trigger {
        input.trigger = trigger.into();
    }
    if let Some(name) = request.name {
        input.name = Some(name);
    }
    if let Some(hours) = request.active_hours {
        input.active_hours = Some(hours.into());
    }
    if let Some(enabled) = request.enabled {
        input.enabled = enabled;
    }
    input.import_idempotency_key = request.import_idempotency_key;
    Ok(input)
}

#[utoipa::path(patch, path = "/api/agents/{agent_id}/schedules/{schedule_id}", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path)), request_body = ScheduleUpdateRequest, responses((status = 200, body = ScheduleEnvelope), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
pub(super) async fn update_schedule(
    State(state): State<AppState>,
    Path((agent_id, schedule_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid("malformed request"),
    };
    let request = match parse_json_body::<ScheduleUpdateRequest>(body) {
        Ok(request) => request,
        Err(_) => return invalid("request body is invalid"),
    };
    let patch = AutomationPatch {
        name: request.name,
        prompt: request.prompt,
        trigger: request.trigger.map(Into::into),
        active_hours: request.active_hours.map(|hours| hours.map(Into::into)),
        target: request.target.map(Into::into),
        enabled: request.enabled,
    };
    match state
        .scheduler
        .automations()
        .update(&agent_id, &schedule_id, patch, now_millis())
        .await
    {
        Ok(record) => no_store(json_response(
            StatusCode::OK,
            &ScheduleEnvelope {
                schedule: record.into(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}
```

3. After `delete_schedule`, add:

```rust
#[utoipa::path(post, path = "/api/agents/{agent_id}/schedules/{schedule_id}/run", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path)), responses((status = 202, body = ScheduleEnvelope), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 409, body = ConnectorErrorBody), (status = 429, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
pub(super) async fn run_schedule(
    State(state): State<AppState>,
    Path((agent_id, schedule_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    match state.scheduler.run_now(&agent_id, &schedule_id).await {
        Ok(record) => no_store(json_response(
            StatusCode::ACCEPTED,
            &ScheduleEnvelope {
                schedule: record.into(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/schedules/{schedule_id}/history", tag = "schedules", params(("agent_id" = String, Path), ("schedule_id" = String, Path), ("limit" = Option<usize>, Query, description = "1 to 50, default 50")), responses((status = 200, body = ScheduleRunsEnvelope), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody), (status = 404, body = ConnectorErrorBody), (status = 503, body = ConnectorErrorBody)))]
pub(super) async fn schedule_history(
    State(state): State<AppState>,
    Path((agent_id, schedule_id)): Path<(String, String)>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize_read(request.headers()) {
        return local_owner_error(rejection);
    }
    let Ok(params) = request_query(request.uri()) else {
        return invalid("malformed query");
    };
    let limit = match params.get("limit").map(String::as_str) {
        None | Some("") => MAX_AUTOMATION_HISTORY_SHOWN,
        Some(value) => match value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_AUTOMATION_HISTORY_SHOWN).contains(limit))
        {
            Some(limit) => limit,
            None => return invalid(HISTORY_LIMIT_INVALID),
        },
    };
    match state
        .scheduler
        .automations()
        .history(&agent_id, &schedule_id, limit)
        .await
    {
        Ok(runs) => no_store(json_response(
            StatusCode::OK,
            &ScheduleRunsEnvelope {
                runs: runs.into_iter().map(Into::into).collect(),
            },
        )),
        Err(error) => schedule_error(error),
    }
}

#[utoipa::path(post, path = "/api/schedules/preview", tag = "schedules", request_body = SchedulePreviewRequest, responses((status = 200, body = SchedulePreviewResponse), (status = 400, body = ConnectorErrorBody), (status = 403, body = ConnectorErrorBody)))]
pub(super) async fn preview_schedule(State(state): State<AppState>, request: AxumRequest) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize_read(request.headers()) {
        return local_owner_error(rejection);
    }
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return invalid("malformed request"),
    };
    let request = match parse_json_body::<SchedulePreviewRequest>(body) {
        Ok(request) => request,
        Err(_) => return invalid("request body is invalid"),
    };
    let active_hours = request.active_hours.map(Into::into);
    match preview(&request.trigger.into(), active_hours.as_ref(), now_millis()) {
        Ok(next_runs) => no_store(json_response(
            StatusCode::OK,
            &SchedulePreviewResponse { next_runs },
        )),
        Err(error) => schedule_error(error),
    }
}
```

4. Add to `schedule_error`, after the `Busy` arm:

```text
        ScheduleError::HistoryUnavailable => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "schedule_history_unavailable",
            AUTOMATION_HISTORY_UNAVAILABLE,
        ),
```

In `hosts/rust-daemon/src/routes/mod.rs`, add to `ApiDoc`'s `paths(…)`, after `schedules::import_legacy_schedules,`:

```text
        schedules::run_schedule,
        schedules::schedule_history,
        schedules::preview_schedule,
```

and to the router, after the `/api/agents/{agent_id}/schedules/{schedule_id}` route:

```text
        .route(
            "/api/agents/{agent_id}/schedules/{schedule_id}/run",
            axum::routing::post(schedules::run_schedule),
        )
        .route(
            "/api/agents/{agent_id}/schedules/{schedule_id}/history",
            get(schedules::schedule_history),
        )
        .route(
            "/api/schedules/preview",
            axum::routing::post(schedules::preview_schedule),
        )
```

Change the `schedules` tag description to `"Automations: daemon-backed scheduled prompts, their history, Run now, and previews"`.

- [ ] **Step 6: The README**

In `hosts/rust-daemon/README.md`, add after the `### Skills` section (before `### Agencies`):

```markdown
### Automations

Every automation route requires local-owner authorization (the list too, since M6), and answers `Cache-Control: no-store` with errors as `{ code, error }`. An automation is a prompt the daemon sends its agent on a trigger: `interval { intervalMs }` (whole seconds), `daily { hour, minute, timeZone }`, `cron { expression, timeZone }` (five fields — minute, hour, day of month, month, day of week — with `*`, numbers, `JAN`–`DEC`, `SUN`–`SAT`, ranges, steps, and lists, or `@hourly`, `@daily`, `@midnight`, `@weekly`, `@monthly`, `@yearly`; `L`, `W`, `#`, and `?` are refused; when both day fields are restricted either may match), or `once { atMs }`, which turns itself off once it fires. Cron and daily times are wall-clock times in their IANA `timeZone`: a time a daylight-saving jump skips does not fire that day, and a repeated time fires once. `activeHours { start, end, days, timeZone }` (`HH:MM`, days 0 for Sunday to 6) keeps fires inside a window; a window whose `end` is before its `start` runs overnight from the day it starts; an interval waits for the window to open; a one-time automation takes no active hours. Each automation has a `name` (1–80 characters on one line, from the prompt when omitted), `createdBy` (`owner`, or `agent` with the session, run, and tool call that made it), `preset` (`heartbeat` or null), `counters { runs, failures, consecutiveFailures }`, and `running`. Every saved change, claim, and outcome is announced as `automation.updated` (`scheduleId`, `deleted`) on the agent's event stream. Each occurrence's outcome (`silent`, `spoke`, `failed`, or `stopped`) is kept as a fire record, saved with the outcome and mirrored to the history store's `schedule_runs` table; fire records go with their agent, not with their automation.

**Limits.** An agent has at most 20 automations (a legacy browser import is exempt). Prompts may not contain invisible Unicode tag or direction-override characters, and names may not contain any invisible format character or line break. A companion with `create_automation` (a write-class tool, allowed by default) can schedule its own future runs; each one shows in its chat with Undo and on the Automations page, must be at least 5 minutes apart over its next 10 fires, and counts toward the 20. Set the companion's write policy to `ask` to approve each one. Silent check-in pairs (`CHECKIN_OK` replies) leave the control plane's hot tail once they are mirrored and older than 24 hours, wherever they sit in their session; the history store keeps them.

| Method   | Path                                                     | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| -------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `GET`    | `/api/agents/{agent_id}/schedules`                       | `{ schedules }`, oldest first.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `POST`   | `/api/agents/{agent_id}/schedules`                       | Create `{ prompt, trigger, target?, name?, activeHours?, enabled? }` (target defaults to `workspace`, the automation's own thread), or `{ preset: "heartbeat", timeZone, … }` (every 30 minutes from 08:00 to 22:00 in `timeZone`; any other field named replaces the preset's). `201` with `{ schedule }` (`200` when a legacy import key already made it). `400` for an invalid field (`prompt and trigger are required unless preset is heartbeat`, `timeZone is required for the heartbeat preset`, or the trigger's problem); `409` (`This companion already has 20 automations; delete one first`) or for an unavailable Telegram target; `503` when it cannot be saved. |
| `PATCH`  | `/api/agents/{agent_id}/schedules/{schedule_id}`         | Change `{ prompt?, trigger?, target?, enabled?, name?, activeHours? }` (`activeHours: null` clears them); the next fire is computed again when the trigger or the active hours change, or when it is turned back on (a one-time automation whose time passed needs a new `atMs`).                                                                                                                                                                                                                                                                                                                                                                                              |
| `DELETE` | `/api/agents/{agent_id}/schedules/{schedule_id}`         | Delete; returns `{ deleted: true }`. Its check-in session can then be deleted; its fire records stay until its agent is deleted.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `POST`   | `/api/agents/{agent_id}/schedules/import`                | Import legacy browser check-ins (unchanged).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `POST`   | `/api/agents/{agent_id}/schedules/{schedule_id}/run`     | Run now: `202` with `{ schedule }`. The due time and the switch stay, and the fire is recorded as `manual`. `404`; `409` (`This automation is already running`); `429` (`Too many automations are running; try again shortly`, at eight running automations); `503`.                                                                                                                                                                                                                                                                                                                                                                                                           |
| `GET`    | `/api/agents/{agent_id}/schedules/{schedule_id}/history` | `{ runs }`, newest first, `?limit=` 1–50 (default 50): `{ id, scheduleId, agentId, firedAtMs, finishedAtMs, outcome, runId, sessionId, errorCode, manual }`. `400` (`limit must be from 1 to 50`); `404`; `503` (`automation history is unavailable`) when the history store cannot be read.                                                                                                                                                                                                                                                                                                                                                                                   |
| `POST`   | `/api/schedules/preview`                                 | `{ trigger, activeHours? }` → `{ nextRuns }`, the next three fire times (one for `once`), so the browser never computes schedules. `400` with the trigger's problem.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes:: schedules 2>&1 | tail -30`
Expected: PASS (10 new route tests and every existing route and schedule test).

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test schedule_api 2>&1 | tail -30`
Expected: PASS (the owner headers on the list reads, the new 403, and the three new OpenAPI paths).

- [ ] **Step 8: Format and commit**

```bash
cargo fmt --all
bun x nx format:write --files=hosts/rust-daemon/README.md
git diff --stat
git add hosts/rust-daemon/src/routes/contracts/schedules.rs hosts/rust-daemon/src/routes/schedules.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/routes/tests/automations.rs hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/schedules/automations.rs hosts/rust-daemon/src/schedules/history.rs hosts/rust-daemon/src/state/automation_state.rs hosts/rust-daemon/tests/schedule_api.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): add automation routes for run now, history, and previews, with the new fields"
```

Recommended implementer tier: standard (route and contract patterns from M4–M5, complete code).

---

### Task 8: Companion tools `create_automation`, `list_automations`, `pause_automation`, helpers, and the tool grant

**Files:**

- Create: `hosts/rust-daemon/src/tools/automations.rs`, `hosts/rust-daemon/src/agent_runs/automations.rs`, `hosts/rust-daemon/src/agent_runs/automation_tests.rs`
- Modify: `hosts/rust-daemon/src/tools.rs` (module line, re-export, three registrations), `hosts/rust-daemon/src/tools/tests.rs` (three expectations), `hosts/rust-daemon/src/agent_runs.rs` (two module lines, the helper filter and its comment), `hosts/rust-daemon/src/schedules/automations.rs` (`AutomationService::telegram_target`), `hosts/rust-daemon/src/sessions/migration.rs` (the grant and two test expectations)

**Interfaces:**

- Consumes: Task 5's `AutomationService::{create, list, pause}`, `AutomationInput`, `AutomationCreator`; Task 5's `preview`; Task 7's `display_name`; `ToolExecutionContext::{team, run_link}`; `is_helper_config`; the M5 test helpers (`skill_tests::call`, `test_support::{companion_config, chat_request, ledger_run, tool_input, tool_results, ScriptedModel, Step}`).
- Produces:
  - `tools::automations::{create_automation, list_automations, pause_automation}` handlers; `is_automation_tool(name) -> bool` (re-exported from `tools`); `parse_schedule(text, time_zone) -> Result<ScheduleTrigger, &'static str>`; `created_reply(record, next_runs) -> String`, `paused_reply(record, changed) -> String`, `list_text(records) -> String`; the Global Constraints' `tools/automations.rs` strings plus `CREATE_ARGS_MISSING = "create_automation needs prompt and schedule strings"`, `PAUSE_ID_MISSING = "pause_automation needs an id string"`, `TARGET_ARG_INVALID = "target must be thread or telegram"`, `ACTIVE_HOURS_ARG_INVALID = "activeHours must be an object with start and end as HH:MM and optional days from 0 (Sunday) to 6"`.
  - `AgentRunCoordinator::automations() -> AutomationService`; `AutomationService::telegram_target(agent_id) -> Option<ScheduleTarget>` (the agent's active connector with an approved chat).
  - `TOOL_GRANTS` gains `m6-automations`.
- Behavior (spec §9.3): `create_automation { prompt, schedule, name?, timeZone?, target?: thread | telegram, activeHours? }` reads `schedule` as a cron expression (5 fields or a macro, in `timeZone`, default `UTC`), `every <n> minutes|hours|days` (also `every hour`), or `at <RFC 3339 time>`; `activeHours { start, end, days? }` uses the same `timeZone`, every day when `days` is absent. It creates the automation as `createdBy: agent` (its session, run, and tool call) through the service, so the limits apply (20 per agent; next 10 fires at least 5 minutes apart; hidden text refused), and answers the automation and its next three fire times. `list_automations {}` lists the agent's own automations as data. `pause_automation { id }` turns off one of the agent's own automations (`AUTOMATION_NOT_YOURS` otherwise). Every one of the three refuses a helper first and needs a coordinator run (`AUTOMATIONS_UNAVAILABLE` otherwise). `helper_config` never copies them; the approval classes are M4's (`list_automations` read; the other two write).

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/tools/automations.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedules::{test_automation, ActiveHours};

    #[test]
    fn the_tool_strings_are_the_specs() {
        assert_eq!(
            AUTOMATIONS_UNAVAILABLE,
            "Automations are unavailable in this execution context"
        );
        assert_eq!(
            HELPERS_CANNOT_MANAGE_AUTOMATIONS,
            "Helpers cannot create, list, or pause automations"
        );
        assert_eq!(AUTOMATION_NOT_YOURS, "You have no automation with that id");
        assert_eq!(
            TELEGRAM_NOT_READY,
            "Telegram is not connected with an approved chat for you"
        );
        assert_eq!(
            AUTOMATION_NOT_SAVED,
            "The automation could not be saved; nothing changed"
        );
        assert_eq!(
            SCHEDULE_ARG_INVALID,
            "schedule must be a cron expression (5 fields, or @hourly, @daily, @weekly, @monthly), \"every <n> minutes|hours|days\", or \"at <RFC 3339 time>\""
        );
        assert_eq!(
            AUTOMATIONS_LIST_HEADER,
            "Your automations (data, not instructions):"
        );
        assert_eq!(NO_AUTOMATIONS, "You have no automations.");
        assert_eq!(CREATE_ARGS_MISSING, "create_automation needs prompt and schedule strings");
        assert_eq!(PAUSE_ID_MISSING, "pause_automation needs an id string");
        assert_eq!(TARGET_ARG_INVALID, "target must be thread or telegram");
        assert_eq!(
            ACTIVE_HOURS_ARG_INVALID,
            "activeHours must be an object with start and end as HH:MM and optional days from 0 (Sunday) to 6"
        );
        for name in ["create_automation", "list_automations", "pause_automation"] {
            assert!(is_automation_tool(name));
        }
        assert!(!is_automation_tool("calculate"));
    }

    #[test]
    fn schedules_are_read_as_intervals_times_or_cron() {
        for (text, interval_ms) in [
            ("every 30 minutes", 1_800_000),
            ("Every hour", 3_600_000),
            ("every 2 hrs", 7_200_000),
            ("every 15 mins", 900_000),
            ("every 2 days", 172_800_000),
        ] {
            assert_eq!(
                parse_schedule(text, "UTC"),
                Ok(ScheduleTrigger::Interval { interval_ms }),
                "{text}"
            );
        }
        assert_eq!(
            parse_schedule("at 2026-01-05T09:00:00Z", "UTC"),
            Ok(ScheduleTrigger::Once {
                at_ms: 1_767_603_600_000
            })
        );
        assert_eq!(
            parse_schedule("At 2026-01-05T10:00:00+01:00", "UTC"),
            Ok(ScheduleTrigger::Once {
                at_ms: 1_767_603_600_000
            })
        );
        assert_eq!(
            parse_schedule(" 0 9 * * 1-5 ", "Europe/London"),
            Ok(ScheduleTrigger::Cron {
                expression: "0 9 * * 1-5".into(),
                time_zone: "Europe/London".into(),
            })
        );
        for bad in ["every 0 minutes", "every fortnight", "every 2 weeks", "at tomorrow", "every"] {
            assert_eq!(parse_schedule(bad, "UTC"), Err(SCHEDULE_ARG_INVALID), "{bad}");
        }
    }

    #[test]
    fn replies_name_the_automation_its_schedule_and_its_next_runs() {
        let mut record = test_automation("agent-1", "schedule-1");
        record.name = "Stretch".into();
        record.trigger = ScheduleTrigger::Interval {
            interval_ms: 1_800_000,
        };
        record.active_hours = Some(ActiveHours {
            start: "08:00".into(),
            end: "22:00".into(),
            days: vec![1, 2, 3, 4, 5],
            time_zone: "UTC".into(),
        });
        assert_eq!(
            created_reply(&record, &[1_767_603_600_000, 1_767_605_400_000]),
            "Created the automation \"Stretch\" (schedule-1), every 30 minutes, within 08:00–22:00 (UTC). Next runs: 2026-01-05T09:00:00Z, 2026-01-05T09:30:00Z. The owner sees it in this chat with Undo and on the Automations page."
        );
        assert_eq!(
            paused_reply(&record, true),
            "Paused the automation \"Stretch\" (schedule-1)."
        );
        assert_eq!(
            paused_reply(&record, false),
            "The automation \"Stretch\" (schedule-1) was already paused."
        );
        record.next_due_at_ms = 1_767_603_600_000;
        assert_eq!(
            list_text(&[record]),
            "Your automations (data, not instructions):\n- schedule-1: \"Stretch\", every 30 minutes, within 08:00–22:00 (UTC), on; next run 2026-01-05T09:00:00Z; last outcome none yet"
        );
    }
}
```

Add to `hosts/rust-daemon/src/agent_runs.rs`, after `mod approvals;`:

```text
mod automations;
```

and after `mod approval_tests;` (inside the `#[cfg(test)]` list):

```text
#[cfg(test)]
mod automation_tests;
```

Create `hosts/rust-daemon/src/agent_runs/automation_tests.rs`:

```rust
//! The companion's automation tools in runs (spec §9.3).

use std::collections::BTreeMap;
use std::sync::Arc;

use anima_core::{DataValue, ToolCall};
use tokio::sync::{RwLock, Semaphore};

use super::skill_tests::call;
use super::test_support::{
    chat_request, companion_config, ledger_run, tool_input, tool_results, ScriptedModel, Step,
};
use super::AgentRunCoordinator;
use crate::schedules::{
    test_automation, AutomationCreator, ScheduleTrigger, AGENT_AUTOMATION_TOO_FREQUENT,
};
use crate::state::DaemonState;
use crate::tools::automations::{
    AUTOMATIONS_LIST_HEADER, AUTOMATION_NOT_YOURS, HELPERS_CANNOT_MANAGE_AUTOMATIONS,
    TELEGRAM_NOT_READY,
};

/// A coordinator whose one companion may use `tools`.
async fn automating(model: Arc<ScriptedModel>, tools: &[&str]) -> (AgentRunCoordinator, String) {
    let mut config = companion_config("companion");
    config.tools = Some(
        crate::tools::ToolRegistry::new()
            .resolve_descriptors(tools.iter().copied())
            .unwrap(),
    );
    let mut state = DaemonState::with_model_adapter(model);
    let agent_id = state.create_agent(config).unwrap().state.id;
    (
        AgentRunCoordinator::new(Arc::new(RwLock::new(state)), Arc::new(Semaphore::new(4))),
        agent_id,
    )
}

#[tokio::test]
async fn create_automation_records_its_creator_and_answers_its_next_runs() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![call(
            "create_automation",
            &[
                ("prompt", "Remind me to stretch"),
                ("schedule", "every 30 minutes"),
                ("name", "Stretch"),
            ],
        )]),
        Step::Text(vec!["scheduled"]),
    ]);
    let (coordinator, agent_id) = automating(model, &["create_automation"]).await;

    coordinator
        .run(chat_request(&agent_id, "chat:x", "remind me to stretch"))
        .await
        .unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    let reply = results.last().unwrap();
    assert!(
        reply.starts_with("Created the automation \"Stretch\" (schedule-"),
        "{reply}"
    );
    assert_eq!(reply.matches("Z,").count() + 1, 3, "three next runs: {reply}");
    let guard = coordinator.state.read().await;
    let record = guard.schedules.values().next().expect("one automation");
    assert_eq!(
        record.trigger,
        ScheduleTrigger::Interval {
            interval_ms: 1_800_000
        }
    );
    let AutomationCreator::Agent {
        agent_id: creator,
        session_id,
        run_id,
        tool_call_id,
    } = &record.created_by
    else {
        panic!("made by the agent: {:?}", record.created_by);
    };
    assert_eq!(creator, &agent_id);
    assert_eq!(session_id, "chat:x");
    assert!(run_id.starts_with("run_"));
    assert_eq!(tool_call_id, "create_automation-1");
}

#[tokio::test]
async fn too_frequent_automations_and_a_missing_telegram_are_refused() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![call(
            "create_automation",
            &[("prompt", "Ping"), ("schedule", "every 4 minutes")],
        )]),
        Step::Tools(vec![call(
            "create_automation",
            &[
                ("prompt", "Ping"),
                ("schedule", "every hour"),
                ("target", "telegram"),
            ],
        )]),
        Step::Text(vec!["no"]),
    ]);
    let (coordinator, agent_id) = automating(model, &["create_automation"]).await;

    coordinator
        .run(chat_request(&agent_id, "chat:x", "ping me"))
        .await
        .unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    assert!(results[0].contains(AGENT_AUTOMATION_TOO_FREQUENT), "{results:?}");
    assert!(results[1].contains(TELEGRAM_NOT_READY), "{results:?}");
    assert!(coordinator.state.read().await.schedules.is_empty());
}

#[tokio::test]
async fn active_hours_and_a_time_zone_are_read() {
    let mut args = BTreeMap::from([
        ("prompt".to_string(), DataValue::String("Brief me".into())),
        ("schedule".to_string(), DataValue::String("0 9 * * 1-5".into())),
        ("timeZone".to_string(), DataValue::String("Europe/London".into())),
    ]);
    args.insert(
        "activeHours".into(),
        DataValue::Object(BTreeMap::from([
            ("start".to_string(), DataValue::String("08:00".into())),
            ("end".to_string(), DataValue::String("18:00".into())),
            (
                "days".to_string(),
                DataValue::Array((1..=5).map(|day| DataValue::Number(day as f64)).collect()),
            ),
        ])),
    );
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![ToolCall {
            id: "call-hours".into(),
            name: "create_automation".into(),
            args,
        }]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = automating(model, &["create_automation"]).await;

    coordinator
        .run(chat_request(&agent_id, "chat:x", "brief me on weekdays"))
        .await
        .unwrap();

    let guard = coordinator.state.read().await;
    let record = guard.schedules.values().next().expect("one automation");
    assert_eq!(
        record.trigger,
        ScheduleTrigger::Cron {
            expression: "0 9 * * 1-5".into(),
            time_zone: "Europe/London".into(),
        }
    );
    let hours = record.active_hours.as_ref().unwrap();
    assert_eq!(hours.days, vec![1, 2, 3, 4, 5]);
    assert_eq!(hours.time_zone, "Europe/London");
}

#[tokio::test]
async fn list_and_pause_reach_only_the_agents_own_automations() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![call("list_automations", &[])]),
        Step::Tools(vec![call("pause_automation", &[("id", "mine")])]),
        Step::Tools(vec![call("pause_automation", &[("id", "theirs")])]),
        Step::Text(vec!["done"]),
    ]);
    let (coordinator, agent_id) =
        automating(model, &["list_automations", "pause_automation"]).await;
    {
        let mut guard = coordinator.state.write().await;
        let other = guard
            .create_agent(companion_config("other"))
            .unwrap()
            .state
            .id;
        guard
            .schedules
            .insert("mine".into(), test_automation(&agent_id, "mine"));
        guard
            .schedules
            .insert("theirs".into(), test_automation(&other, "theirs"));
    }

    coordinator
        .run(chat_request(&agent_id, "chat:x", "tidy my automations"))
        .await
        .unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    assert!(results[0].starts_with(AUTOMATIONS_LIST_HEADER), "{results:?}");
    assert!(results[0].contains("- mine:"));
    assert!(!results[0].contains("theirs"));
    assert_eq!(results[1], "Paused the automation \"Check status\" (mine).");
    assert!(results[2].contains(AUTOMATION_NOT_YOURS), "{results:?}");
    let guard = coordinator.state.read().await;
    assert!(!guard.schedules["mine"].enabled);
    assert!(guard.schedules["theirs"].enabled);
}

#[tokio::test]
async fn helpers_cannot_manage_automations() {
    let (coordinator, agent_id) = automating(
        ScriptedModel::new(vec![]),
        &["create_automation", "list_automations", "pause_automation"],
    )
    .await;
    let link = ledger_run(&coordinator, &agent_id, "chat:x").await;
    let (context, mut helper) = {
        let guard = coordinator.state.read().await;
        (
            guard
                .tool_execution_context()
                .with_team(coordinator.clone(), false)
                .with_run_link(Some(link)),
            guard.agents[&agent_id].state(),
        )
    };
    let additional = &mut helper.config.settings.as_mut().unwrap().additional;
    additional.insert("workspaceRole".into(), DataValue::String("helper".into()));
    additional.insert(
        "parentAgentId".into(),
        DataValue::String("companion-1".into()),
    );

    for tool in [
        call(
            "create_automation",
            &[("prompt", "P"), ("schedule", "every hour")],
        ),
        call("list_automations", &[]),
        call("pause_automation", &[("id", "x")]),
    ] {
        let result = context
            .clone()
            .execute_tool(helper.clone(), tool_input(&agent_id, "chat:x"), tool)
            .await;
        assert_eq!(
            result.error.as_deref(),
            Some(HELPERS_CANNOT_MANAGE_AUTOMATIONS)
        );
    }
    assert!(coordinator.state.read().await.schedules.is_empty());
}

#[test]
fn a_helper_never_gets_the_automation_tools() {
    let mut parent = companion_config("companion");
    parent.tools = Some(
        crate::tools::ToolRegistry::new()
            .resolve_descriptors([
                "create_automation",
                "list_automations",
                "pause_automation",
                "calculate",
            ])
            .unwrap(),
    );
    let mut state = DaemonState::new();
    let parent = state.create_agent(parent).unwrap().state;

    let helper = super::helper_config(&parent, "helper".into());

    let names: Vec<String> = helper
        .tools
        .unwrap_or_default()
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert_eq!(names, ["calculate"]);
}
```

In `hosts/rust-daemon/src/tools/tests.rs`'s `registry_defines_every_registered_tool_schema`, add after the `propose_skill` expectation:

```text
        (
            "create_automation",
            &["prompt", "schedule"][..],
            &["name", "timeZone", "target", "activeHours"][..],
        ),
        ("list_automations", &[][..], &[][..]),
        ("pause_automation", &["id"][..], &[][..]),
```

In `hosts/rust-daemon/src/sessions/migration.rs`'s tests: in `the_search_conversations_grant_reaches_non_helper_agents_only` change the companion's expected tools to `["calculate", "search_conversations", "load_skill", "list_automations"]`; in `the_skills_grant_adds_load_skill_and_for_writers_propose_skill` change the reader's to `["read_file", "search_conversations", "load_skill", "list_automations"]`, the writer's to `["write_file", "search_conversations", "load_skill", "propose_skill", "list_automations", "create_automation", "pause_automation"]`, and add `assert!(state.tool_grants_applied.contains("m6-automations"));`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- tools:: agent_runs::automation_tests sessions::migration 2>&1 | tail -30`
Expected: FAIL to compile (the tools, `automations()`, and the strings do not exist).

- [ ] **Step 3: The coordinator's accessor and the Telegram target**

Create `hosts/rust-daemon/src/agent_runs/automations.rs`:

```rust
//! Automations from the coordinator (spec §9.3): the service the
//! companion's tools change them through, under this coordinator's
//! control-plane transaction.

use std::sync::Arc;

use super::AgentRunCoordinator;
use crate::schedules::AutomationService;

impl AgentRunCoordinator {
    pub(crate) fn automations(&self) -> AutomationService {
        AutomationService::new(Arc::clone(&self.state), self.control_plane_transactions())
    }
}
```

In `hosts/rust-daemon/src/schedules/automations.rs`, add to `impl AutomationService`, after `get`:

```rust
    /// The agent's Telegram chat as a target: its active connector with an
    /// approved chat (the first by id when there are several).
    pub(crate) async fn telegram_target(&self, agent_id: &str) -> Option<ScheduleTarget> {
        let state = self.state.read().await;
        state
            .connectors
            .values()
            .filter(|connector| {
                connector.agent_id == agent_id
                    && connector.is_active()
                    && connector.approved_chat.is_some()
            })
            .min_by(|left, right| left.id.cmp(&right.id))
            .map(|connector| ScheduleTarget::Connector {
                connector_id: connector.id.clone(),
            })
    }
```

In `hosts/rust-daemon/src/agent_runs.rs`'s `helper_config`, add `&& !crate::tools::is_automation_tool(&tool.name)` to the `filter` closure after `tool.name != "propose_skill"`, and append to the comment above it: ` The automation tools never reach a helper either (spec §9.3: they would schedule runs of the helper).`

Delete `#![allow(dead_code)] // M6 Task 8 uses every item.` from `hosts/rust-daemon/src/schedules/automations.rs`: the tools now call `pause` and `telegram_target`, and the routes the rest.

- [ ] **Step 4: The tools**

Put this above the test module in `hosts/rust-daemon/src/tools/automations.rs`:

```rust
//! The companion's automation tools (spec §9.3): `create_automation`,
//! `list_automations`, and `pause_automation`. Each reaches only the calling
//! agent's own automations, through `AutomationService` (so the limits and
//! the hidden-text checks apply), and refuses helpers.

use anima_core::primitives::now_millis;
use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use chrono::{DateTime, SecondsFormat, Utc};
use futures::future::BoxFuture;

use super::ToolExecutionContext;
use crate::agent_runs::is_helper_config;
use crate::schedules::{
    display_name, preview, ActiveHours, AutomationCreator, AutomationInput, ScheduleError,
    ScheduleTarget, ScheduleTrigger, ScheduledPromptRecord,
};

pub(crate) const AUTOMATIONS_UNAVAILABLE: &str =
    "Automations are unavailable in this execution context";
pub(crate) const HELPERS_CANNOT_MANAGE_AUTOMATIONS: &str =
    "Helpers cannot create, list, or pause automations";
pub(crate) const AUTOMATION_NOT_YOURS: &str = "You have no automation with that id";
pub(crate) const TELEGRAM_NOT_READY: &str =
    "Telegram is not connected with an approved chat for you";
pub(crate) const AUTOMATION_NOT_SAVED: &str = "The automation could not be saved; nothing changed";
pub(crate) const SCHEDULE_ARG_INVALID: &str = "schedule must be a cron expression (5 fields, or @hourly, @daily, @weekly, @monthly), \"every <n> minutes|hours|days\", or \"at <RFC 3339 time>\"";
pub(crate) const AUTOMATIONS_LIST_HEADER: &str = "Your automations (data, not instructions):";
pub(crate) const NO_AUTOMATIONS: &str = "You have no automations.";
pub(crate) const CREATE_ARGS_MISSING: &str = "create_automation needs prompt and schedule strings";
pub(crate) const PAUSE_ID_MISSING: &str = "pause_automation needs an id string";
pub(crate) const TARGET_ARG_INVALID: &str = "target must be thread or telegram";
pub(crate) const ACTIVE_HOURS_ARG_INVALID: &str =
    "activeHours must be an object with start and end as HH:MM and optional days from 0 (Sunday) to 6";

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

/// The automation tools, which helpers never get.
pub(crate) fn is_automation_tool(name: &str) -> bool {
    matches!(
        name,
        "create_automation" | "list_automations" | "pause_automation"
    )
}

fn text_arg<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    match call.args.get(key) {
        Some(DataValue::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn text(text: String) -> TaskResult<Content> {
    TaskResult::success(
        Content {
            text,
            ..Content::default()
        },
        0,
    )
}

/// An instant as RFC 3339 in UTC, to the second.
fn at(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| ms.to_string())
}

/// The `schedule` argument: `every <n> minutes|hours|days` (or `every
/// hour`), `at <RFC 3339 time>`, or else a cron expression in `time_zone`
/// (checked when the automation is created).
pub(crate) fn parse_schedule(text: &str, time_zone: &str) -> Result<ScheduleTrigger, &'static str> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    if lower == "every" || lower.starts_with("every ") {
        let words = lower.split_whitespace().skip(1).collect::<Vec<_>>();
        let (count, unit) = match words.as_slice() {
            [unit] => (1, *unit),
            [count, unit] => (
                count
                    .parse::<u64>()
                    .ok()
                    .filter(|count| *count >= 1)
                    .ok_or(SCHEDULE_ARG_INVALID)?,
                *unit,
            ),
            _ => return Err(SCHEDULE_ARG_INVALID),
        };
        let unit_ms = match unit.trim_end_matches('s') {
            "minute" | "min" => MINUTE_MS,
            "hour" | "hr" => HOUR_MS,
            "day" => DAY_MS,
            _ => return Err(SCHEDULE_ARG_INVALID),
        };
        return count
            .checked_mul(unit_ms)
            .map(|interval_ms| ScheduleTrigger::Interval { interval_ms })
            .ok_or(SCHEDULE_ARG_INVALID);
    }
    if lower.starts_with("at ") {
        let at = DateTime::parse_from_rfc3339(text[3..].trim()).map_err(|_| SCHEDULE_ARG_INVALID)?;
        let at_ms = u64::try_from(at.timestamp_millis()).map_err(|_| SCHEDULE_ARG_INVALID)?;
        return Ok(ScheduleTrigger::Once { at_ms });
    }
    Ok(ScheduleTrigger::Cron {
        expression: text.to_string(),
        time_zone: time_zone.to_string(),
    })
}

fn every(interval_ms: u64) -> String {
    let (count, unit) = if interval_ms % DAY_MS == 0 {
        (interval_ms / DAY_MS, "day")
    } else if interval_ms % HOUR_MS == 0 {
        (interval_ms / HOUR_MS, "hour")
    } else if interval_ms % MINUTE_MS == 0 {
        (interval_ms / MINUTE_MS, "minute")
    } else {
        (interval_ms / 1_000, "second")
    };
    format!("every {count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// The automation's schedule in words, for the model.
fn describe(record: &ScheduledPromptRecord) -> String {
    let schedule = match &record.trigger {
        ScheduleTrigger::Interval { interval_ms } => every(*interval_ms),
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => format!("daily at {hour:02}:{minute:02} ({time_zone})"),
        ScheduleTrigger::Cron {
            expression,
            time_zone,
        } => format!("cron \"{expression}\" ({time_zone})"),
        ScheduleTrigger::Once { at_ms } => format!("once at {}", at(*at_ms)),
    };
    match &record.active_hours {
        Some(hours) => format!(
            "{schedule}, within {}–{} ({})",
            hours.start, hours.end, hours.time_zone
        ),
        None => schedule,
    }
}

pub(crate) fn created_reply(record: &ScheduledPromptRecord, next_runs: &[u64]) -> String {
    let next = if next_runs.is_empty() {
        "none".to_string()
    } else {
        next_runs
            .iter()
            .map(|ms| at(*ms))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "Created the automation \"{}\" ({}), {}. Next runs: {next}. The owner sees it in this chat with Undo and on the Automations page.",
        display_name(record),
        record.id,
        describe(record)
    )
}

pub(crate) fn paused_reply(record: &ScheduledPromptRecord, changed: bool) -> String {
    if changed {
        format!(
            "Paused the automation \"{}\" ({}).",
            display_name(record),
            record.id
        )
    } else {
        format!(
            "The automation \"{}\" ({}) was already paused.",
            display_name(record),
            record.id
        )
    }
}

/// The agent's automations as data, one line each.
pub(crate) fn list_text(records: &[ScheduledPromptRecord]) -> String {
    let mut lines = vec![AUTOMATIONS_LIST_HEADER.to_string()];
    for record in records {
        let outcome = record
            .last_safe_outcome
            .as_ref()
            .map(|outcome| outcome.status.contract_name())
            .unwrap_or("none yet");
        lines.push(format!(
            "- {}: \"{}\", {}, {}; next run {}; last outcome {}",
            record.id,
            display_name(record),
            describe(record),
            if record.enabled { "on" } else { "paused" },
            at(record.next_due_at_ms),
            outcome
        ));
    }
    lines.join("\n")
}

fn refusal(error: ScheduleError) -> String {
    match error {
        ScheduleError::Invalid(message)
        | ScheduleError::Conflict(message)
        | ScheduleError::Busy(message) => message.to_string(),
        ScheduleError::Rejected(message) => message,
        ScheduleError::NotFound => AUTOMATION_NOT_YOURS.to_string(),
        ScheduleError::AgentNotFound => AUTOMATIONS_UNAVAILABLE.to_string(),
        ScheduleError::TargetUnavailable => TELEGRAM_NOT_READY.to_string(),
        ScheduleError::Persistence | ScheduleError::HistoryUnavailable => {
            AUTOMATION_NOT_SAVED.to_string()
        }
    }
}

/// `activeHours { start, end, days? }` in `time_zone`; every day without
/// `days`.
fn active_hours_arg(call: &ToolCall, time_zone: &str) -> Result<Option<ActiveHours>, &'static str> {
    let fields = match call.args.get("activeHours") {
        None | Some(DataValue::Null) => return Ok(None),
        Some(DataValue::Object(fields)) => fields,
        Some(_) => return Err(ACTIVE_HOURS_ARG_INVALID),
    };
    let string = |key: &str| match fields.get(key) {
        Some(DataValue::String(value)) => Some(value.clone()),
        _ => None,
    };
    let (Some(start), Some(end)) = (string("start"), string("end")) else {
        return Err(ACTIVE_HOURS_ARG_INVALID);
    };
    let days = match fields.get("days") {
        None | Some(DataValue::Null) => (0..=6).collect(),
        Some(DataValue::Array(items)) => items
            .iter()
            .map(|item| match item {
                DataValue::Number(day) if day.fract() == 0.0 && (0.0..=6.0).contains(day) => {
                    Ok(*day as u8)
                }
                _ => Err(ACTIVE_HOURS_ARG_INVALID),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(ACTIVE_HOURS_ARG_INVALID),
    };
    Ok(Some(ActiveHours {
        start,
        end,
        days,
        time_zone: time_zone.to_string(),
    }))
}

pub(super) fn create_automation(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        if is_helper_config(&agent.config) {
            return TaskResult::error(HELPERS_CANNOT_MANAGE_AUTOMATIONS, 0);
        }
        let (Some(coordinator), Some(link)) = (context.team.clone(), context.run_link.clone())
        else {
            return TaskResult::error(AUTOMATIONS_UNAVAILABLE, 0);
        };
        let (Some(prompt), Some(schedule)) = (text_arg(&call, "prompt"), text_arg(&call, "schedule"))
        else {
            return TaskResult::error(CREATE_ARGS_MISSING, 0);
        };
        let time_zone = text_arg(&call, "timeZone")
            .map(str::trim)
            .filter(|zone| !zone.is_empty())
            .unwrap_or("UTC");
        let trigger = match parse_schedule(schedule, time_zone) {
            Ok(trigger) => trigger,
            Err(problem) => return TaskResult::error(problem, 0),
        };
        let active_hours = match active_hours_arg(&call, time_zone) {
            Ok(hours) => hours,
            Err(problem) => return TaskResult::error(problem, 0),
        };
        let service = coordinator.automations();
        let target = match text_arg(&call, "target").unwrap_or("thread") {
            "thread" => ScheduleTarget::Workspace,
            "telegram" => match service.telegram_target(&agent.id).await {
                Some(target) => target,
                None => return TaskResult::error(TELEGRAM_NOT_READY, 0),
            },
            _ => return TaskResult::error(TARGET_ARG_INVALID, 0),
        };
        let mut input =
            AutomationInput::owner(agent.id.clone(), prompt.to_string(), trigger, target);
        input.name = text_arg(&call, "name")
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string);
        input.active_hours = active_hours;
        input.created_by = AutomationCreator::Agent {
            agent_id: agent.id.clone(),
            session_id: link.session_id,
            run_id: link.run_id,
            tool_call_id: call.id.clone(),
        };
        let now = now_millis();
        match service.create(input, now).await {
            Ok((record, _)) => {
                let next = preview(&record.trigger, record.active_hours.as_ref(), now)
                    .unwrap_or_default();
                text(created_reply(&record, &next))
            }
            Err(error) => TaskResult::error(refusal(error), 0),
        }
    })
}

pub(super) fn list_automations(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    _call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        if is_helper_config(&agent.config) {
            return TaskResult::error(HELPERS_CANNOT_MANAGE_AUTOMATIONS, 0);
        }
        let Some(coordinator) = context.team.clone() else {
            return TaskResult::error(AUTOMATIONS_UNAVAILABLE, 0);
        };
        match coordinator.automations().list(&agent.id).await {
            Ok(records) if records.is_empty() => text(NO_AUTOMATIONS.to_string()),
            Ok(records) => text(list_text(&records)),
            Err(error) => TaskResult::error(refusal(error), 0),
        }
    })
}

pub(super) fn pause_automation(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        if is_helper_config(&agent.config) {
            return TaskResult::error(HELPERS_CANNOT_MANAGE_AUTOMATIONS, 0);
        }
        let Some(id) = text_arg(&call, "id")
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            return TaskResult::error(PAUSE_ID_MISSING, 0);
        };
        let Some(coordinator) = context.team.clone() else {
            return TaskResult::error(AUTOMATIONS_UNAVAILABLE, 0);
        };
        match coordinator
            .automations()
            .pause(&agent.id, id, now_millis())
            .await
        {
            Ok((record, changed)) => text(paused_reply(&record, changed)),
            Err(error) => TaskResult::error(refusal(error), 0),
        }
    })
}
```

In `hosts/rust-daemon/src/tools.rs`:

1. Add `pub(crate) mod automations;` after `pub(crate) mod calendar;` and `pub(crate) use automations::is_automation_tool;` after the `workspace` re-export.
2. After the `propose_skill` registration, add:

```rust
        registry.register(
            tool_descriptor(
                "create_automation",
                "Schedule a prompt that runs you again later, in the automation's own thread or in the owner's Telegram chat. The owner sees it in this chat with Undo and on the Automations page. Its runs must be at least 5 minutes apart; you may have at most 20 automations.",
                object_parameters(vec![
                    required_parameter(
                        "prompt",
                        non_blank_string_parameter("What to do on each run, at most 32 KiB"),
                    ),
                    required_parameter(
                        "schedule",
                        non_blank_string_parameter(
                            "A 5-field cron expression or @hourly, @daily, @weekly, @monthly; \"every <n> minutes|hours|days\"; or \"at <RFC 3339 time>\" for one run",
                        ),
                    ),
                    optional_parameter(
                        "name",
                        string_parameter("A short name, at most 80 characters; from the prompt when absent"),
                    ),
                    optional_parameter(
                        "timeZone",
                        string_parameter("IANA time zone for a cron schedule and active hours, such as Europe/London; UTC when absent"),
                    ),
                    optional_parameter(
                        "target",
                        string_enum_parameter(
                            "Where it runs: its own thread (the default) or the owner's Telegram chat",
                            &["thread", "telegram"],
                        ),
                    ),
                    optional_parameter(
                        "activeHours",
                        object_parameter(vec![
                            required_parameter("start", non_blank_string_parameter("HH:MM, 24-hour")),
                            required_parameter(
                                "end",
                                non_blank_string_parameter("HH:MM, 24-hour; before start for an overnight window"),
                            ),
                            optional_parameter(
                                "days",
                                array_parameter(
                                    "Days it may run, 0 for Sunday to 6 for Saturday; every day when absent",
                                    bounded_integer_parameter("A day, 0 for Sunday", 0, 6),
                                    Some(1),
                                ),
                            ),
                        ]),
                    ),
                ]),
            ),
            automations::create_automation,
        );
        registry.register(
            tool_descriptor(
                "list_automations",
                "List your automations: id, name, schedule, whether each is on, its next run, and its last outcome.",
                object_parameters(vec![]),
            ),
            automations::list_automations,
        );
        registry.register(
            tool_descriptor(
                "pause_automation",
                "Turn off one of your automations by its id (from list_automations). The owner can turn it back on.",
                object_parameters(vec![required_parameter(
                    "id",
                    non_blank_string_parameter("The automation's id"),
                )]),
            ),
            automations::pause_automation,
        );
```

In `hosts/rust-daemon/src/sessions/migration.rs`, append to `TOOL_GRANTS` and replace the doc comment above it with `/// Grant sets in the order they shipped.`:

```rust
    ToolGrantSet {
        id: "m6-automations",
        read_class: &["list_automations"],
        write_class: &["create_automation", "pause_automation"],
    },
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- tools:: agent_runs::automation_tests agent_runs::skill_tests sessions::migration approvals::policy 2>&1 | tail -30`
Expected: PASS (3 tool unit tests, 6 run tests, the updated registry and grant tests, and every policy test: the three tools already have their classes).

- [ ] **Step 6: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add hosts/rust-daemon/src/tools.rs hosts/rust-daemon/src/tools/automations.rs hosts/rust-daemon/src/tools/tests.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/agent_runs/automations.rs hosts/rust-daemon/src/agent_runs/automation_tests.rs hosts/rust-daemon/src/schedules/automations.rs hosts/rust-daemon/src/sessions/migration.rs
git commit -m "feat(daemon): let the companion create, list, and pause its automations"
```

Recommended implementer tier: standard (the M5 tool pattern; complete code).

---

### Task 9: SDK automations client and the `automation.updated` event

**Files:**

- Create: `packages/sdk/src/automations.ts`, `packages/sdk/src/automations.spec.ts`
- Modify: `packages/sdk/src/events.ts`, `packages/sdk/src/events.spec.ts`, `packages/sdk/src/client.ts`, `packages/sdk/src/index.ts`

**Interfaces:**

- Consumes: the JSON of Task 7's routes and Task 3's `automation.updated`; `DaemonClient::requestJson`.
- Produces:
  - Types `AutomationTrigger`, `ActiveHours`, `AutomationTarget`, `AutomationCreator`, `AutomationOutcome`, `AutomationCounters`, `Automation`, `AutomationInput`, `HeartbeatInput`, `AutomationPatch`, `AutomationRunOutcome`, `AutomationRun`; constants `MAX_AUTOMATIONS_PER_AGENT = 20`, `MAX_AUTOMATION_HISTORY = 50`, `MAX_AUTOMATION_NAME_CHARS = 80`, `AUTOMATION_PREVIEW_RUNS = 3`.
  - `AutomationsClient` (`client.automations`): `list(agentId, options?) -> Automation[]`, `create(agentId, input) -> Automation`, `createHeartbeat(agentId, input) -> Automation`, `update(agentId, id, patch) -> Automation`, `remove(agentId, id) -> void`, `runNow(agentId, id) -> Automation`, `history(agentId, id, options?) -> AutomationRun[]`, `preview({ trigger, activeHours? }, options?) -> number[]`.
  - `AgentEvent` gains `{ type: 'automation.updated'; scheduleId: string; deleted: boolean }`; `isAutomationEvent(event)`.
- Behavior: every path segment is percent-encoded; bodies carry only the known fields (the daemon refuses unknown keys), and `update` keeps an explicit `activeHours: null` (it clears them). The existing `AgentsClient` schedule methods stay as they are.

- [ ] **Step 1: Write the failing tests**

Create `packages/sdk/src/automations.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';

import {
  AUTOMATION_PREVIEW_RUNS,
  MAX_AUTOMATION_HISTORY,
  MAX_AUTOMATION_NAME_CHARS,
  MAX_AUTOMATIONS_PER_AGENT,
  createDaemonClient,
  type Automation,
  type AutomationRun,
} from './index.js';

function transport(respond: (url: string, init?: RequestInit) => Response) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return respond(String(url), init);
    },
  });
  return { automations: client.automations, requests };
}

const automation: Automation = {
  id: 'schedule/1',
  agentId: 'agent/a',
  name: 'Stretch',
  prompt: 'Remind me to stretch',
  trigger: { type: 'interval', intervalMs: 1_800_000 },
  activeHours: null,
  enabled: true,
  target: { type: 'workspace' },
  nextDueAtMs: 10,
  lastFiredAtMs: null,
  lastOutcome: null,
  running: false,
  createdBy: {
    kind: 'agent',
    agentId: 'agent/a',
    sessionId: 'chat:1',
    runId: 'run_1',
    toolCallId: 'call-1',
  },
  preset: null,
  counters: { runs: 0, failures: 0, consecutiveFailures: 0 },
  importIdempotencyKey: null,
  createdAtMs: 1,
  updatedAtMs: 1,
};

const run: AutomationRun = {
  id: 'schedule:schedule/1:5',
  scheduleId: 'schedule/1',
  agentId: 'agent/a',
  firedAtMs: 5,
  finishedAtMs: 9,
  outcome: 'failed',
  runId: 'run_2',
  sessionId: 'schedule:schedule/1',
  errorCode: 'schedule_run_failed',
  manual: true,
};

describe('automations client', () => {
  it('lists, creates, edits, runs, and deletes with encoded ids', async () => {
    const { automations, requests } = transport((url, init) => {
      if (init?.method === 'DELETE') return Response.json({ deleted: true });
      if (!init?.method) return Response.json({ schedules: [automation] });
      if (url.endsWith('/run'))
        return Response.json({ schedule: automation }, { status: 202 });
      return Response.json({ schedule: automation });
    });

    expect(await automations.list('agent/a')).toEqual([automation]);
    await automations.create('agent/a', {
      prompt: 'Remind me to stretch',
      trigger: { type: 'cron', expression: '0 9 * * 1-5', timeZone: 'UTC' },
      name: 'Stretch',
      activeHours: {
        start: '08:00',
        end: '22:00',
        days: [1, 2],
        timeZone: 'UTC',
      },
    });
    await automations.createHeartbeat('agent/a', { timeZone: 'Europe/London' });
    await automations.update('agent/a', 'schedule/1', {
      enabled: false,
      activeHours: null,
    });
    expect(await automations.runNow('agent/a', 'schedule/1')).toEqual(
      automation,
    );
    await automations.remove('agent/a', 'schedule/1');

    expect(
      requests.map(({ url, init }) => [init?.method ?? 'GET', url, init?.body]),
    ).toEqual([
      ['GET', '/api/agents/agent%2Fa/schedules', undefined],
      [
        'POST',
        '/api/agents/agent%2Fa/schedules',
        JSON.stringify({
          prompt: 'Remind me to stretch',
          trigger: { type: 'cron', expression: '0 9 * * 1-5', timeZone: 'UTC' },
          name: 'Stretch',
          activeHours: {
            start: '08:00',
            end: '22:00',
            days: [1, 2],
            timeZone: 'UTC',
          },
        }),
      ],
      [
        'POST',
        '/api/agents/agent%2Fa/schedules',
        JSON.stringify({ preset: 'heartbeat', timeZone: 'Europe/London' }),
      ],
      [
        'PATCH',
        '/api/agents/agent%2Fa/schedules/schedule%2F1',
        JSON.stringify({ enabled: false, activeHours: null }),
      ],
      ['POST', '/api/agents/agent%2Fa/schedules/schedule%2F1/run', undefined],
      ['DELETE', '/api/agents/agent%2Fa/schedules/schedule%2F1', undefined],
    ]);
  });

  it('reads the history and previews fire times', async () => {
    const { automations, requests } = transport((url) =>
      url.includes('/history')
        ? Response.json({ runs: [run] })
        : Response.json({ nextRuns: [1, 2, 3] }),
    );

    expect(
      await automations.history('agent/a', 'schedule/1', { limit: 10 }),
    ).toEqual([run]);
    expect(await automations.history('agent/a', 'schedule/1')).toEqual([run]);
    expect(
      await automations.preview({
        trigger: { type: 'interval', intervalMs: 60_000 },
      }),
    ).toEqual([1, 2, 3]);

    expect(requests.map(({ url, init }) => [url, init?.body])).toEqual([
      [
        '/api/agents/agent%2Fa/schedules/schedule%2F1/history?limit=10',
        undefined,
      ],
      ['/api/agents/agent%2Fa/schedules/schedule%2F1/history', undefined],
      [
        '/api/schedules/preview',
        JSON.stringify({ trigger: { type: 'interval', intervalMs: 60_000 } }),
      ],
    ]);
  });

  it('exports the daemon limits', () => {
    expect(MAX_AUTOMATIONS_PER_AGENT).toBe(20);
    expect(MAX_AUTOMATION_HISTORY).toBe(50);
    expect(MAX_AUTOMATION_NAME_CHARS).toBe(80);
    expect(AUTOMATION_PREVIEW_RUNS).toBe(3);
  });
});
```

Add to `packages/sdk/src/events.spec.ts`, after the `isSkillEvent` describe block (and add `isAutomationEvent` to its import from `./index.js`):

```ts
describe('isAutomationEvent', () => {
  it('recognizes automation.updated', () => {
    const event: AgentEvent = {
      type: 'automation.updated',
      agentId: 'agent-1',
      seq: 4,
      at: 5,
      scheduleId: 'schedule-1',
      deleted: false,
    };
    expect(isAutomationEvent(event)).toBe(true);
    expect(
      isAutomationEvent({
        type: 'skill.updated',
        agentId: 'a',
        seq: 1,
        at: 1,
        slug: null,
        draftId: null,
      }),
    ).toBe(false);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `bun x nx test @animaOS-SWARM/sdk 2>&1 | tail -30`
Expected: FAIL (`client.automations` and `isAutomationEvent` do not exist).

- [ ] **Step 3: Write the client and the event**

Create `packages/sdk/src/automations.ts`:

```ts
import type { DaemonClient } from './client.js';

/** When an automation fires (spec §9.1). Daily and cron times are wall-clock
 *  times in their IANA `timeZone`; `once` fires one time and then turns the
 *  automation off. */
export type AutomationTrigger =
  | { type: 'interval'; intervalMs: number }
  | { type: 'daily'; hour: number; minute: number; timeZone: string }
  | { type: 'cron'; expression: string; timeZone: string }
  | { type: 'once'; atMs: number };

/** When it may fire: `HH:MM` wall times, days 0 (Sunday) to 6. An `end`
 *  before `start` runs overnight from the day it starts. */
export interface ActiveHours {
  start: string;
  end: string;
  days: number[];
  timeZone: string;
}

/** `workspace`: the automation's own thread. */
export type AutomationTarget =
  | { type: 'workspace' }
  | { type: 'connector'; connectorId: string };

/** Who made it; the companion's names the tool call (for its notice card). */
export type AutomationCreator =
  | { kind: 'owner' }
  | {
      kind: 'agent';
      agentId: string;
      sessionId: string;
      runId: string;
      toolCallId: string;
    };

export interface AutomationOutcome {
  /** `error` for a failure (the history says `failed`). */
  status: 'silent' | 'spoke' | 'error' | 'stopped';
  occurredAtMs: number;
  errorCode: string | null;
}

export interface AutomationCounters {
  runs: number;
  failures: number;
  consecutiveFailures: number;
}

export interface Automation {
  id: string;
  agentId: string;
  /** Written by the owner or the companion: show it as text only. */
  name: string;
  prompt: string;
  trigger: AutomationTrigger;
  activeHours: ActiveHours | null;
  enabled: boolean;
  target: AutomationTarget;
  nextDueAtMs: number;
  lastFiredAtMs: number | null;
  lastOutcome: AutomationOutcome | null;
  /** Its latest occurrence has no outcome yet. */
  running: boolean;
  createdBy: AutomationCreator;
  preset: 'heartbeat' | null;
  counters: AutomationCounters;
  importIdempotencyKey: string | null;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface AutomationInput {
  prompt: string;
  trigger: AutomationTrigger;
  /** The automation's own thread when absent. */
  target?: AutomationTarget;
  /** From the prompt's first line when absent. */
  name?: string;
  activeHours?: ActiveHours;
  enabled?: boolean;
}

/** The heartbeat preset (spec §9.2): every 30 minutes from 08:00 to 22:00
 *  in `timeZone`; any other field replaces the preset's. */
export interface HeartbeatInput {
  timeZone: string;
  prompt?: string;
  name?: string;
  target?: AutomationTarget;
}

export interface AutomationPatch {
  prompt?: string;
  trigger?: AutomationTrigger;
  target?: AutomationTarget;
  enabled?: boolean;
  name?: string;
  /** `null` clears them. */
  activeHours?: ActiveHours | null;
}

export type AutomationRunOutcome = 'silent' | 'spoke' | 'failed' | 'stopped';

/** One occurrence (spec §9.1). */
export interface AutomationRun {
  id: string;
  scheduleId: string;
  agentId: string;
  firedAtMs: number;
  finishedAtMs: number;
  outcome: AutomationRunOutcome;
  runId: string | null;
  sessionId: string | null;
  errorCode: string | null;
  /** Run now, not the trigger. */
  manual: boolean;
}

/** The daemon's limits (spec §9.3, §16). */
export const MAX_AUTOMATIONS_PER_AGENT = 20;
export const MAX_AUTOMATION_HISTORY = 50;
export const MAX_AUTOMATION_NAME_CHARS = 80;
export const AUTOMATION_PREVIEW_RUNS = 3;

function collection(agentId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}/schedules`;
}

function item(agentId: string, id: string): string {
  return `${collection(agentId)}/${encodeURIComponent(id)}`;
}

function only<T extends object>(
  value: T,
  keys: readonly (keyof T)[],
): Partial<T> {
  const picked: Partial<T> = {};
  for (const key of keys)
    if (value[key] !== undefined) picked[key] = value[key];
  return picked;
}

const INPUT_KEYS = [
  'prompt',
  'trigger',
  'target',
  'name',
  'activeHours',
  'enabled',
] as const;
const PATCH_KEYS = [
  'prompt',
  'trigger',
  'target',
  'enabled',
  'name',
  'activeHours',
] as const;

export class AutomationsClient {
  constructor(private readonly client: DaemonClient) {}

  /** The agent's automations, oldest first. */
  async list(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<Automation[]> {
    const response = await this.client.requestJson<{
      schedules: Automation[];
    }>(collection(agentId), { signal: options.signal });
    return response.schedules;
  }

  async create(agentId: string, input: AutomationInput): Promise<Automation> {
    const response = await this.client.requestJson<{ schedule: Automation }>(
      collection(agentId),
      { method: 'POST', body: only(input, INPUT_KEYS) },
    );
    return response.schedule;
  }

  async createHeartbeat(
    agentId: string,
    input: HeartbeatInput,
  ): Promise<Automation> {
    const body = {
      preset: 'heartbeat',
      ...only(input, ['timeZone', 'prompt', 'name', 'target'] as const),
    };
    const response = await this.client.requestJson<{ schedule: Automation }>(
      collection(agentId),
      { method: 'POST', body },
    );
    return response.schedule;
  }

  async update(
    agentId: string,
    id: string,
    patch: AutomationPatch,
  ): Promise<Automation> {
    const response = await this.client.requestJson<{ schedule: Automation }>(
      item(agentId, id),
      { method: 'PATCH', body: only(patch, PATCH_KEYS) },
    );
    return response.schedule;
  }

  async remove(agentId: string, id: string): Promise<void> {
    await this.client.requestJson(item(agentId, id), { method: 'DELETE' });
  }

  /** Fires it now; its due time and switch stay (spec §9.2). */
  async runNow(agentId: string, id: string): Promise<Automation> {
    const response = await this.client.requestJson<{ schedule: Automation }>(
      `${item(agentId, id)}/run`,
      { method: 'POST' },
    );
    return response.schedule;
  }

  /** Its latest occurrences, newest first (at most 50). */
  async history(
    agentId: string,
    id: string,
    options: { limit?: number; signal?: AbortSignal } = {},
  ): Promise<AutomationRun[]> {
    const query =
      options.limit === undefined ? '' : `?limit=${String(options.limit)}`;
    const response = await this.client.requestJson<{ runs: AutomationRun[] }>(
      `${item(agentId, id)}/history${query}`,
      { signal: options.signal },
    );
    return response.runs;
  }

  /** The next fire times the daemon computes (spec §9.2): three, or one for
   *  `once`. */
  async preview(
    input: { trigger: AutomationTrigger; activeHours?: ActiveHours },
    options: { signal?: AbortSignal } = {},
  ): Promise<number[]> {
    const response = await this.client.requestJson<{ nextRuns: number[] }>(
      '/api/schedules/preview',
      {
        method: 'POST',
        body: only(input, ['trigger', 'activeHours'] as const),
        signal: options.signal,
      },
    );
    return response.nextRuns;
  }
}
```

In `packages/sdk/src/events.ts`, add to the `AgentEvent` union, after the `skill.updated` member:

```text
  | (EventBase & {
      type: 'automation.updated';
      scheduleId: string;
      /** The automation is gone. */
      deleted: boolean;
    })
```

and after `isSkillEvent`:

```ts
/** An automation changed, fired, or finished (spec §6): read them again. */
export function isAutomationEvent(
  event: AgentEvent,
): event is Extract<AgentEvent, { type: 'automation.updated' }> {
  return event.type === 'automation.updated';
}
```

In `packages/sdk/src/client.ts`, add `import { AutomationsClient } from './automations.js';`, the field `readonly automations: AutomationsClient;` after `skills`, and `this.automations = new AutomationsClient(this);` after `this.skills = new SkillsClient(this);`.

In `packages/sdk/src/index.ts`, add `isAutomationEvent,` to the `./events.js` value export list, and after the skills exports:

```ts
export {
  AUTOMATION_PREVIEW_RUNS,
  AutomationsClient,
  MAX_AUTOMATION_HISTORY,
  MAX_AUTOMATION_NAME_CHARS,
  MAX_AUTOMATIONS_PER_AGENT,
} from './automations.js';
export type {
  ActiveHours,
  Automation,
  AutomationCounters,
  AutomationCreator,
  AutomationInput,
  AutomationOutcome,
  AutomationPatch,
  AutomationRun,
  AutomationRunOutcome,
  AutomationTarget,
  AutomationTrigger,
  HeartbeatInput,
} from './automations.js';
```

If `packages/sdk/src/index.spec.ts` asserts the list of `DaemonClient` members or of exported names, add `automations` and the new names to it the way it lists `skills`.

- [ ] **Step 4: Run the tests, then build**

Run: `bun x nx test @animaOS-SWARM/sdk 2>&1 | tail -30`
Expected: PASS (3 new client tests, the event test, every existing SDK test).

Run: `bun x nx run @animaOS-SWARM/sdk:typecheck 2>&1 | tail -15 && bun x nx run @animaOS-SWARM/sdk:build 2>&1 | tail -15`
Expected: both succeed (later web tasks resolve the new exports from the build).

- [ ] **Step 5: Format and commit**

```bash
bun x nx format:write --files=packages/sdk/src/automations.ts,packages/sdk/src/automations.spec.ts,packages/sdk/src/events.ts,packages/sdk/src/events.spec.ts,packages/sdk/src/client.ts,packages/sdk/src/index.ts
git diff --stat
git add packages/sdk/src/automations.ts packages/sdk/src/automations.spec.ts packages/sdk/src/events.ts packages/sdk/src/events.spec.ts packages/sdk/src/client.ts packages/sdk/src/index.ts
git commit -m "feat(sdk): add the automations client and the automation.updated event"
```

Recommended implementer tier: cheap (typed client with complete code and tests).

---

### Task 10: Web automations data: the phrase parser, labels, the reducer's counter, the facade, `useAutomations`, and the access profiles

**Files:**

- Create: `apps/web/src/lib/schedule-parse.ts`, `apps/web/src/lib/schedule-parse.test.ts`, `apps/web/src/lib/automations.ts`, `apps/web/src/lib/automations.test.ts`, `apps/web/src/hooks/useAutomations.ts`, `apps/web/src/hooks/useAutomations.test.tsx`, `apps/web/src/test/automations.ts`
- Modify: `apps/web/src/lib/session-events.ts`, `apps/web/src/lib/session-events.test.ts`, `apps/web/src/test/live.ts` (`automationEvent`), `apps/web/src/lib/daemon-api.ts`, `apps/web/src/lib/agent-access.ts`, `apps/web/src/lib/agent-access.test.ts`

**Interfaces:**

- Consumes: Task 9's `AutomationsClient`, types, and `isAutomationEvent`'s event shape; `LiveState`, `applyEvent`; `DaemonHttpError`; `COMPANION_UNREACHABLE`, `formatWhen` (`lib/approvals.ts`); `ToolStep` (`lib/transcript.ts`).
- Produces:
  - `lib/schedule-parse.ts`: `ScheduleParseOptions { nowMs, timeZone }`, `parseSchedule(text, options) -> AutomationTrigger | null`, `parseClock(text) -> { hour, minute } | null`, `zonedTimeToUtc(year, month, day, hour, minute, timeZone) -> number`, `PHRASE_EXAMPLES` (the spec's five phrases).
  - `lib/automations.ts`: `DAY_NAMES`, `localTimeZone()`, `describeTrigger(trigger)`, `describeActiveHours(hours)`, `OUTCOME_LABELS`, `RUN_OUTCOME_LABELS`, `automationNoticeFor(step, automations) -> Automation | null`, `checkinScheduleId(sessionId) -> string | null`, and the strings `PHRASE_NOT_UNDERSTOOD`, `AGENT_CREATED_NOTE`.
  - `LiveState.automationsVersion: number`, bumped by each `automation.updated` (a snapshot keeps it).
  - `daemon.{listAutomations, createAutomation, createHeartbeat, updateAutomation, deleteAutomation, runAutomationNow, automationHistory, previewAutomation}`.
  - `hooks/useAutomations.ts`: `useAutomations({ agentId, version, epoch, enabled }) -> AutomationsView { automations, loaded, error, errorStatus, refresh, create, createHeartbeat, update, setEnabled, remove, runNow }` (every function stable across renders).
  - `test/automations.ts`: `automationFixture(id, overrides)`, `automationRunFixture(id, overrides)`; `test/live.ts`: `automationEvent(seq, scheduleId?, agentId?)`.
  - Access profiles: `list_automations` in every profile; `create_automation` and `pause_automation` in Collaborate and Operate (spec §13.3 step 5).
- Behavior: the phrase parser is deterministic: it reads wall-clock phrases in the given time zone (through `Intl`), never the machine's, so tests do not depend on where they run. It understands `every <n> minutes|hours|days` (and `every hour`), `every day at <time>` / `daily at <time>`, `weekdays at <time>`, `weekends at <time>`, `every <weekday>[ and <weekday>…] at <time>` / `<weekday>s at <time>`, `today|tomorrow at <time>`, `at <time>`, and `in <n> minutes|hours|days`; anything else is `null` and the editor offers the cron field. Times are `9`, `9am`, `9:30 pm`, `15:00`, `noon`, or `midnight`. A `today` time already past is `null`; a bare `at <time>` already past means tomorrow. `useAutomations` reads the agent's list whenever `version`, `epoch`, or its own refresh moves, aborting a superseded read; each action reads the list again before it answers `true`, and a 404 or 409 refusal reads it again too.

- [ ] **Step 1: Write the failing tests**

Create `apps/web/src/test/automations.ts`:

```ts
import type { Automation, AutomationRun } from '@animaOS-SWARM/sdk';

export function automationFixture(
  id: string,
  overrides: Partial<Automation> = {},
): Automation {
  return {
    id,
    agentId: 'agent-main',
    name: `Automation ${id}`,
    prompt: 'Check in',
    trigger: { type: 'interval', intervalMs: 1_800_000 },
    activeHours: null,
    enabled: true,
    target: { type: 'workspace' },
    nextDueAtMs: 10,
    lastFiredAtMs: null,
    lastOutcome: null,
    running: false,
    createdBy: { kind: 'owner' },
    preset: null,
    counters: { runs: 0, failures: 0, consecutiveFailures: 0 },
    importIdempotencyKey: null,
    createdAtMs: 1,
    updatedAtMs: 1,
    ...overrides,
  };
}

export function automationRunFixture(
  id: string,
  overrides: Partial<AutomationRun> = {},
): AutomationRun {
  return {
    id,
    scheduleId: 'schedule-1',
    agentId: 'agent-main',
    firedAtMs: 5,
    finishedAtMs: 9,
    outcome: 'spoke',
    runId: 'run_1',
    sessionId: 'schedule:schedule-1',
    errorCode: null,
    manual: false,
    ...overrides,
  };
}
```

Add to `apps/web/src/test/live.ts`, after `skillEvent`:

```ts
export function automationEvent(
  seq: number,
  scheduleId = 'schedule-1',
  agentId = 'agent-main',
): AgentEvent {
  return {
    type: 'automation.updated',
    agentId,
    seq,
    at: 1,
    scheduleId,
    deleted: false,
  };
}
```

Create `apps/web/src/lib/schedule-parse.test.ts`:

```ts
import { describe, expect, it } from 'vitest';

import {
  PHRASE_EXAMPLES,
  parseClock,
  parseSchedule,
  zonedTimeToUtc,
} from './schedule-parse';

/** 2026-01-05 08:00 UTC, a Monday. */
const NOW = Date.UTC(2026, 0, 5, 8, 0);
const utc = { nowMs: NOW, timeZone: 'UTC' };
const kualaLumpur = { nowMs: NOW, timeZone: 'Asia/Kuala_Lumpur' };

describe('parseClock', () => {
  it('reads 12- and 24-hour times, noon, and midnight', () => {
    expect(parseClock('9')).toEqual({ hour: 9, minute: 0 });
    expect(parseClock('9am')).toEqual({ hour: 9, minute: 0 });
    expect(parseClock('9:30 pm')).toEqual({ hour: 21, minute: 30 });
    expect(parseClock('12am')).toEqual({ hour: 0, minute: 0 });
    expect(parseClock('12pm')).toEqual({ hour: 12, minute: 0 });
    expect(parseClock('15:00')).toEqual({ hour: 15, minute: 0 });
    expect(parseClock('noon')).toEqual({ hour: 12, minute: 0 });
    expect(parseClock('midnight')).toEqual({ hour: 0, minute: 0 });
    for (const bad of ['25:00', '9:60', '13pm', '0am', 'soon', '']) {
      expect(parseClock(bad), bad).toBeNull();
    }
  });
});

describe('parseSchedule', () => {
  it('reads the spec examples', () => {
    expect(PHRASE_EXAMPLES).toEqual([
      'every 2 hours',
      'weekdays at 9am',
      'every monday at 8:30',
      'tomorrow at 15:00',
      'in 20 minutes',
    ]);
    expect(parseSchedule('every 2 hours', utc)).toEqual({
      type: 'interval',
      intervalMs: 7_200_000,
    });
    expect(parseSchedule('Weekdays at 9am', utc)).toEqual({
      type: 'cron',
      expression: '0 9 * * 1-5',
      timeZone: 'UTC',
    });
    expect(parseSchedule('every monday at 8:30', utc)).toEqual({
      type: 'cron',
      expression: '30 8 * * 1',
      timeZone: 'UTC',
    });
    // Kuala Lumpur is UTC+8: 15:00 there tomorrow is 07:00 UTC on the 6th.
    expect(parseSchedule('tomorrow at 15:00', kualaLumpur)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 6, 7, 0),
    });
    expect(parseSchedule('in 20 minutes', utc)).toEqual({
      type: 'once',
      atMs: NOW + 1_200_000,
    });
  });

  it('reads intervals, days, weekends, and lists of weekdays', () => {
    expect(parseSchedule('every hour', utc)).toEqual({
      type: 'interval',
      intervalMs: 3_600_000,
    });
    expect(parseSchedule('every 30 mins', utc)).toEqual({
      type: 'interval',
      intervalMs: 1_800_000,
    });
    expect(parseSchedule('every day at 9:30', utc)).toEqual({
      type: 'daily',
      hour: 9,
      minute: 30,
      timeZone: 'UTC',
    });
    expect(parseSchedule('daily at noon', kualaLumpur)).toEqual({
      type: 'daily',
      hour: 12,
      minute: 0,
      timeZone: 'Asia/Kuala_Lumpur',
    });
    expect(parseSchedule('weekends at 10am', utc)).toEqual({
      type: 'cron',
      expression: '0 10 * * 0,6',
      timeZone: 'UTC',
    });
    expect(parseSchedule('mondays and thursdays at 7pm', utc)).toEqual({
      type: 'cron',
      expression: '0 19 * * 1,4',
      timeZone: 'UTC',
    });
    expect(parseSchedule('every tue, fri at 6:15', utc)).toEqual({
      type: 'cron',
      expression: '15 6 * * 2,5',
      timeZone: 'UTC',
    });
  });

  it('reads one-time phrases against now, in the time zone', () => {
    expect(parseSchedule('today at 17:00', kualaLumpur)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 5, 9, 0),
    });
    expect(parseSchedule('today at 15:00', kualaLumpur)).toBeNull();
    expect(parseSchedule('at 9am', utc)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 5, 9, 0),
    });
    expect(parseSchedule('at 7am', utc)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 6, 7, 0),
    });
    expect(parseSchedule('in 2 days', utc)).toEqual({
      type: 'once',
      atMs: NOW + 2 * 86_400_000,
    });
  });

  it('crosses a daylight-saving change on the wall clock', () => {
    // New York springs forward on 2026-03-08: 09:00 EDT is 13:00 UTC.
    expect(
      parseSchedule('tomorrow at 9:00', {
        nowMs: Date.UTC(2026, 2, 7, 12, 0),
        timeZone: 'America/New_York',
      }),
    ).toEqual({ type: 'once', atMs: Date.UTC(2026, 2, 8, 13, 0) });
    expect(zonedTimeToUtc(2026, 3, 7, 9, 0, 'America/New_York')).toBe(
      Date.UTC(2026, 2, 7, 14, 0),
    );
  });

  it('returns null for what it does not understand', () => {
    for (const phrase of [
      '',
      'whenever',
      'every 0 minutes',
      'every 2 weeks',
      'tomorrow at 25:00',
      'every someday at 9',
      '0 9 * * *',
    ]) {
      expect(parseSchedule(phrase, utc), phrase).toBeNull();
    }
  });
});
```

Create `apps/web/src/lib/automations.test.ts`:

```ts
import { describe, expect, it } from 'vitest';

import { automationFixture } from '../test/automations';
import {
  AGENT_CREATED_NOTE,
  OUTCOME_LABELS,
  PHRASE_NOT_UNDERSTOOD,
  RUN_OUTCOME_LABELS,
  automationNoticeFor,
  checkinScheduleId,
  describeActiveHours,
  describeTrigger,
} from './automations';
import type { ToolStep } from './transcript';

function step(overrides: Partial<ToolStep> = {}): ToolStep {
  return {
    stepId: 'run_1:0',
    toolCallId: 'call-1',
    name: 'create_automation',
    argumentsPreview: '{}',
    status: 'success',
    durationMs: 5,
    result: 'Created',
    truncated: false,
    runId: 'run_1',
    helper: null,
    ...overrides,
  };
}

describe('automation labels', () => {
  it('describes triggers and active hours in words', () => {
    expect(describeTrigger({ type: 'interval', intervalMs: 1_800_000 })).toBe(
      'every 30 min',
    );
    expect(describeTrigger({ type: 'interval', intervalMs: 3_600_000 })).toBe(
      'every hour',
    );
    expect(describeTrigger({ type: 'interval', intervalMs: 7_200_000 })).toBe(
      'every 2 hours',
    );
    expect(describeTrigger({ type: 'interval', intervalMs: 86_400_000 })).toBe(
      'every day',
    );
    expect(
      describeTrigger({ type: 'daily', hour: 9, minute: 5, timeZone: 'UTC' }),
    ).toBe('daily at 09:05 (UTC)');
    expect(
      describeTrigger({
        type: 'cron',
        expression: '0 9 * * 1-5',
        timeZone: 'Europe/London',
      }),
    ).toBe('cron 0 9 * * 1-5 (Europe/London)');
    expect(
      describeTrigger({ type: 'once', atMs: 0 }).startsWith('once, '),
    ).toBe(true);
    const hours = { start: '08:00', end: '22:00', timeZone: 'UTC' };
    expect(describeActiveHours({ ...hours, days: [0, 1, 2, 3, 4, 5, 6] })).toBe(
      '08:00–22:00, every day (UTC)',
    );
    expect(describeActiveHours({ ...hours, days: [1, 2, 3, 4, 5] })).toBe(
      '08:00–22:00, weekdays (UTC)',
    );
    expect(describeActiveHours({ ...hours, days: [6, 0] })).toBe(
      '08:00–22:00, weekends (UTC)',
    );
    expect(describeActiveHours({ ...hours, days: [5, 1] })).toBe(
      '08:00–22:00, Mon, Fri (UTC)',
    );
    expect(OUTCOME_LABELS.error).toBe('Failed');
    expect(RUN_OUTCOME_LABELS.failed).toBe('Failed');
    expect(PHRASE_NOT_UNDERSTOOD).toContain('cron');
    expect(AGENT_CREATED_NOTE).toBe('Created by your companion');
  });

  it('finds the automation a create_automation call made', () => {
    const made = automationFixture('schedule-1', {
      createdBy: {
        kind: 'agent',
        agentId: 'agent-main',
        sessionId: 'chat:1',
        runId: 'run_1',
        toolCallId: 'call-1',
      },
    });
    const others = [automationFixture('schedule-0'), made];
    expect(automationNoticeFor(step(), others)).toBe(made);
    expect(automationNoticeFor(step({ runId: null }), others)).toBe(made);
    expect(automationNoticeFor(step({ runId: 'run_2' }), others)).toBeNull();
    expect(automationNoticeFor(step({ status: 'error' }), others)).toBeNull();
    expect(
      automationNoticeFor(step({ name: 'list_automations' }), others),
    ).toBeNull();
    expect(automationNoticeFor(step(), [])).toBeNull();
  });

  it('reads a check-in session id', () => {
    expect(checkinScheduleId('schedule:schedule-1')).toBe('schedule-1');
    expect(checkinScheduleId('schedule:')).toBeNull();
    expect(checkinScheduleId('chat:1')).toBeNull();
  });
});
```

Create `apps/web/src/hooks/useAutomations.test.tsx`:

```tsx
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { automationFixture } from '../test/automations';
import { useAutomations } from './useAutomations';

beforeEach(() => {
  vi.spyOn(daemon, 'listAutomations').mockResolvedValue([
    automationFixture('schedule-1'),
  ]);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useAutomations', () => {
  it('reads the list, and again when an automation event arrives', async () => {
    const { result, rerender } = renderHook(
      (props: { version: number }) =>
        useAutomations({
          agentId: 'agent-main',
          version: props.version,
          epoch: 0,
          enabled: true,
        }),
      { initialProps: { version: 0 } },
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.automations.map((item) => item.id)).toEqual([
      'schedule-1',
    ]);
    expect(daemon.listAutomations).toHaveBeenCalledWith('agent-main', {
      signal: expect.any(AbortSignal),
    });

    rerender({ version: 1 });

    await waitFor(() =>
      expect(daemon.listAutomations).toHaveBeenCalledTimes(2),
    );
  });

  it('reads nothing while disabled or without a companion', async () => {
    const { rerender } = renderHook(
      (props: { enabled: boolean; agentId: string | null }) =>
        useAutomations({
          agentId: props.agentId,
          version: 0,
          epoch: 0,
          enabled: props.enabled,
        }),
      {
        initialProps: {
          enabled: false,
          agentId: 'agent-main' as string | null,
        },
      },
    );
    rerender({ enabled: true, agentId: null });
    expect(daemon.listAutomations).not.toHaveBeenCalled();
  });

  it('acts through the daemon, reads again, and reports refusals', async () => {
    const run = vi
      .spyOn(daemon, 'runAutomationNow')
      .mockResolvedValue(automationFixture('schedule-1', { running: true }));
    vi.spyOn(daemon, 'deleteAutomation').mockRejectedValue(
      new DaemonHttpError(409, { error: 'This automation is already running' }),
    );
    const update = vi
      .spyOn(daemon, 'updateAutomation')
      .mockResolvedValue(automationFixture('schedule-1', { enabled: false }));
    const { result } = renderHook(() =>
      useAutomations({
        agentId: 'agent-main',
        version: 0,
        epoch: 0,
        enabled: true,
      }),
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    const first = result.current.runNow;

    let done = false;
    await act(async () => {
      done = await result.current.runNow(result.current.automations[0]);
    });
    expect(done).toBe(true);
    expect(run).toHaveBeenCalledWith('agent-main', 'schedule-1');
    expect(result.current.runNow).toBe(first);

    await act(async () => {
      done = await result.current.setEnabled(
        result.current.automations[0],
        false,
      );
    });
    expect(update).toHaveBeenCalledWith('agent-main', 'schedule-1', {
      enabled: false,
    });

    await act(async () => {
      done = await result.current.remove(result.current.automations[0]);
    });
    expect(done).toBe(false);
    expect(result.current.error).toBe('This automation is already running');
    expect(result.current.errorStatus).toBe(409);
    // Each action, and the 409, read the list again.
    expect(daemon.listAutomations).toHaveBeenCalledTimes(4);
  });
});
```

Add to `apps/web/src/lib/session-events.test.ts`, after the `skill events` describe block (and add `automationEvent` to its `../test/live` import):

```ts
describe('automation events', () => {
  it('count automation.updated events, ignore repeats, and survive a snapshot', () => {
    let state = applyEvent(EMPTY_LIVE_STATE, snapshotEvent([], 1));
    expect(state.automationsVersion).toBe(0);
    state = applyEvent(state, automationEvent(2));
    state = applyEvent(state, automationEvent(2));
    state = applyEvent(state, automationEvent(3, 'schedule-2'));
    expect(state.automationsVersion).toBe(2);
    expect(state.skillsVersion).toBe(0);
    const reconnected = applyEvent(state, snapshotEvent([], 1));
    expect(reconnected.automationsVersion).toBe(2);
  });
});
```

In `apps/web/src/lib/agent-access.test.ts`, add `'list_automations',` after `'load_skill',` in `COMMON_TOOLS`, and `'create_automation', 'pause_automation',` after `'propose_skill',` in `COLLABORATE_TOOLS`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/web && bun x vitest run src/lib/schedule-parse.test.ts src/lib/automations.test.ts src/hooks/useAutomations.test.tsx src/lib/session-events.test.ts src/lib/agent-access.test.ts 2>&1 | tail -30`
Expected: FAIL (the modules and `automationsVersion` do not exist; the access profiles lack the tools).

- [ ] **Step 3: The parser and the labels**

Create `apps/web/src/lib/schedule-parse.ts`:

```ts
import type { AutomationTrigger } from '@animaOS-SWARM/sdk';

/** Spec §15.4: phrases the Automations page understands. The daemon
 *  previews and stores what they mean; the browser never computes fire
 *  times. */
export const PHRASE_EXAMPLES = [
  'every 2 hours',
  'weekdays at 9am',
  'every monday at 8:30',
  'tomorrow at 15:00',
  'in 20 minutes',
] as const;

export interface ScheduleParseOptions {
  /** Now, in epoch milliseconds: "in 20 minutes", "today", "tomorrow". */
  nowMs: number;
  /** The owner's IANA time zone; wall-clock phrases are read in it. */
  timeZone: string;
}

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

const UNITS: Record<string, number> = {
  minute: MINUTE_MS,
  minutes: MINUTE_MS,
  min: MINUTE_MS,
  mins: MINUTE_MS,
  hour: HOUR_MS,
  hours: HOUR_MS,
  hr: HOUR_MS,
  hrs: HOUR_MS,
  day: DAY_MS,
  days: DAY_MS,
};

const WEEKDAYS: Record<string, number> = {
  sunday: 0,
  sun: 0,
  monday: 1,
  mon: 1,
  tuesday: 2,
  tue: 2,
  tues: 2,
  wednesday: 3,
  wed: 3,
  thursday: 4,
  thu: 4,
  thur: 4,
  thurs: 4,
  friday: 5,
  fri: 5,
  saturday: 6,
  sat: 6,
};

/** `9`, `9am`, `9:30 pm`, `15:00`, `noon`, or `midnight`. */
export function parseClock(
  text: string,
): { hour: number; minute: number } | null {
  const clock = text.trim().toLowerCase();
  if (clock === 'noon') return { hour: 12, minute: 0 };
  if (clock === 'midnight') return { hour: 0, minute: 0 };
  const match = /^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$/.exec(clock);
  if (!match) return null;
  let hour = Number(match[1]);
  const minute = match[2] === undefined ? 0 : Number(match[2]);
  if (minute > 59) return null;
  if (match[3]) {
    if (hour < 1 || hour > 12) return null;
    hour = (hour % 12) + (match[3] === 'pm' ? 12 : 0);
  } else if (hour > 23) {
    return null;
  }
  return { hour, minute };
}

interface WallClock {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
}

/** The wall-clock reading of `ms` in `timeZone`. */
function wallClock(ms: number, timeZone: string): WallClock {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone,
    hourCycle: 'h23',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).formatToParts(new Date(ms));
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    Number(parts.find((item) => item.type === type)?.value ?? 0);
  return {
    year: part('year'),
    month: part('month'),
    day: part('day'),
    hour: part('hour') % 24,
    minute: part('minute'),
    second: part('second'),
  };
}

/** The instant of a wall-clock time in `timeZone` (month 1–12). */
export function zonedTimeToUtc(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute: number,
  timeZone: string,
): number {
  const wall = Date.UTC(year, month - 1, day, hour, minute);
  const offset = (ms: number) => {
    const clock = wallClock(ms, timeZone);
    return (
      Date.UTC(
        clock.year,
        clock.month - 1,
        clock.day,
        clock.hour,
        clock.minute,
        clock.second,
      ) - ms
    );
  };
  const first = wall - offset(wall);
  return wall - offset(first);
}

/** The instant of `clock` on the day `days` after today in `timeZone`. */
function onDay(
  options: ScheduleParseOptions,
  days: number,
  clock: { hour: number; minute: number },
): number {
  const today = wallClock(options.nowMs, options.timeZone);
  const date = new Date(
    Date.UTC(today.year, today.month - 1, today.day + days),
  );
  return zonedTimeToUtc(
    date.getUTCFullYear(),
    date.getUTCMonth() + 1,
    date.getUTCDate(),
    clock.hour,
    clock.minute,
    options.timeZone,
  );
}

function weekdays(text: string): number[] | null {
  const names = text
    .split(/\s*(?:,|\band\b)\s*/)
    .map((name) => name.trim().replace(/s$/, ''))
    .filter(Boolean);
  if (names.length === 0) return null;
  const days = names.map((name) => WEEKDAYS[name]);
  if (days.some((day) => day === undefined)) return null;
  return [...new Set(days as number[])].sort((left, right) => left - right);
}

function cron(
  clock: { hour: number; minute: number },
  days: string,
  timeZone: string,
): AutomationTrigger {
  return {
    type: 'cron',
    expression: `${clock.minute} ${clock.hour} * * ${days}`,
    timeZone,
  };
}

/** What a plain-language phrase means, or null (spec §15.4). */
export function parseSchedule(
  text: string,
  options: ScheduleParseOptions,
): AutomationTrigger | null {
  const phrase = text.trim().toLowerCase().replace(/\s+/g, ' ');
  if (!phrase) return null;
  const { timeZone } = options;

  let match = /^(?:every day|daily|each day) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    return clock && { type: 'daily', ...clock, timeZone };
  }
  match = /^(?:every weekday|weekdays) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    return clock && cron(clock, '1-5', timeZone);
  }
  match = /^(?:every weekend|weekends) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    return clock && cron(clock, '0,6', timeZone);
  }
  match = /^(today|tomorrow) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[2]);
    if (!clock) return null;
    const atMs = onDay(options, match[1] === 'tomorrow' ? 1 : 0, clock);
    return atMs > options.nowMs ? { type: 'once', atMs } : null;
  }
  match = /^at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    if (!clock) return null;
    const today = onDay(options, 0, clock);
    return {
      type: 'once',
      atMs: today > options.nowMs ? today : onDay(options, 1, clock),
    };
  }
  match = /^in (\d+) ([a-z]+)$/.exec(phrase);
  if (match) {
    const count = Number(match[1]);
    const unit = UNITS[match[2]];
    return count >= 1 && unit
      ? { type: 'once', atMs: options.nowMs + count * unit }
      : null;
  }
  match = /^every (?:(\d+) )?([a-z]+)$/.exec(phrase);
  if (match) {
    const count = match[1] === undefined ? 1 : Number(match[1]);
    const unit = UNITS[match[2]];
    return count >= 1 && unit
      ? { type: 'interval', intervalMs: count * unit }
      : null;
  }
  match = /^(?:every |on )?([a-z, ]+?) at (.+)$/.exec(phrase);
  if (match) {
    const days = weekdays(match[1]);
    const clock = parseClock(match[2]);
    return days && clock ? cron(clock, days.join(','), timeZone) : null;
  }
  return null;
}
```

Create `apps/web/src/lib/automations.ts`:

```ts
import type {
  ActiveHours,
  Automation,
  AutomationOutcome,
  AutomationRunOutcome,
  AutomationTrigger,
} from '@animaOS-SWARM/sdk';

import { formatWhen } from './approvals';
import type { ToolStep } from './transcript';

export const DAY_NAMES = [
  'Sun',
  'Mon',
  'Tue',
  'Wed',
  'Thu',
  'Fri',
  'Sat',
] as const;

export const PHRASE_NOT_UNDERSTOOD =
  'Try “every 2 hours”, “weekdays at 9am”, “every monday at 8:30”, “tomorrow at 15:00”, or “in 20 minutes”, or enter a cron expression.';
export const AGENT_CREATED_NOTE = 'Created by your companion';

/** The browser's IANA time zone, for wall-clock phrases and presets. */
export function localTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

function pad(value: number): string {
  return String(value).padStart(2, '0');
}

/** A trigger in words (spec §15.2 "Check-in · every 30 min"). */
export function describeTrigger(trigger: AutomationTrigger): string {
  switch (trigger.type) {
    case 'interval': {
      const ms = trigger.intervalMs;
      if (ms % 86_400_000 === 0)
        return ms === 86_400_000
          ? 'every day'
          : `every ${ms / 86_400_000} days`;
      if (ms % 3_600_000 === 0)
        return ms === 3_600_000
          ? 'every hour'
          : `every ${ms / 3_600_000} hours`;
      if (ms % 60_000 === 0) return `every ${ms / 60_000} min`;
      return `every ${Math.round(ms / 1000)} sec`;
    }
    case 'daily':
      return `daily at ${pad(trigger.hour)}:${pad(trigger.minute)} (${trigger.timeZone})`;
    case 'cron':
      return `cron ${trigger.expression} (${trigger.timeZone})`;
    case 'once':
      return `once, ${formatWhen(trigger.atMs)}`;
  }
}

export function describeActiveHours(hours: ActiveHours): string {
  const days = [...hours.days].sort((left, right) => left - right);
  const key = days.join(',');
  const label =
    days.length === 7
      ? 'every day'
      : key === '1,2,3,4,5'
        ? 'weekdays'
        : key === '0,6'
          ? 'weekends'
          : days.map((day) => DAY_NAMES[day]).join(', ');
  return `${hours.start}–${hours.end}, ${label} (${hours.timeZone})`;
}

export const OUTCOME_LABELS: Record<AutomationOutcome['status'], string> = {
  silent: 'Nothing to report',
  spoke: 'Replied',
  error: 'Failed',
  stopped: 'Stopped',
};

export const RUN_OUTCOME_LABELS: Record<AutomationRunOutcome, string> = {
  silent: 'Nothing to report',
  spoke: 'Replied',
  failed: 'Failed',
  stopped: 'Stopped',
};

/** The automation a successful `create_automation` call made, by its tool
 *  call (and run, when the step knows it). */
export function automationNoticeFor(
  step: ToolStep,
  automations: readonly Automation[],
): Automation | null {
  if (step.name !== 'create_automation' || step.status !== 'success')
    return null;
  return (
    automations.find(
      (automation) =>
        automation.createdBy.kind === 'agent' &&
        automation.createdBy.toolCallId === step.toolCallId &&
        (step.runId === null || automation.createdBy.runId === step.runId),
    ) ?? null
  );
}

/** A check-in session's automation id (`schedule:<id>`). */
export function checkinScheduleId(sessionId: string): string | null {
  const id = sessionId.startsWith('schedule:')
    ? sessionId.slice('schedule:'.length)
    : '';
  return id || null;
}
```

- [ ] **Step 4: The reducer, the facade, the hook, and the profiles**

In `apps/web/src/lib/session-events.ts`:

1. Add to `LiveState` after `skillsVersion`:

```text
  /** Bumped by every `automation.updated` (spec §6), so automation views
   *  read again. */
  automationsVersion: number;
```

2. Add `automationsVersion: 0,` to `EMPTY_LIVE_STATE`, and `automationsVersion: state.automationsVersion,` to the object the snapshot branch of `applyEvent` returns (after `skillsVersion: state.skillsVersion,`).
3. After the `skill.updated` line in `applyEvent`, add:

```text
  if (event.type === 'automation.updated')
    return { ...next, automationsVersion: state.automationsVersion + 1 };
```

Run `cd apps/web && bun x tsc -p tsconfig.app.json --noEmit 2>&1 | tail -20`; add `automationsVersion: 0` to any other `LiveState` object literal the compiler names (test files included).

In `apps/web/src/lib/daemon-api.ts`, add `type ActiveHours, type AutomationInput, type AutomationPatch, type AutomationTrigger, type HeartbeatInput,` to the `@animaOS-SWARM/sdk` import, and after `importSkill: …`:

```ts
  /** Automations (spec §9.2, §15.4). */
  listAutomations: (agentId: string, options: { signal?: AbortSignal } = {}) =>
    setupClient.automations.list(agentId, options),
  createAutomation: (agentId: string, input: AutomationInput) =>
    setupClient.automations.create(agentId, input),
  createHeartbeat: (agentId: string, input: HeartbeatInput) =>
    setupClient.automations.createHeartbeat(agentId, input),
  updateAutomation: (agentId: string, id: string, patch: AutomationPatch) =>
    setupClient.automations.update(agentId, id, patch),
  deleteAutomation: (agentId: string, id: string) =>
    setupClient.automations.remove(agentId, id),
  runAutomationNow: (agentId: string, id: string) =>
    setupClient.automations.runNow(agentId, id),
  automationHistory: (
    agentId: string,
    id: string,
    options: { limit?: number; signal?: AbortSignal } = {},
  ) => setupClient.automations.history(agentId, id, options),
  previewAutomation: (
    input: { trigger: AutomationTrigger; activeHours?: ActiveHours },
    options: { signal?: AbortSignal } = {},
  ) => setupClient.automations.preview(input, options),
```

Create `apps/web/src/hooks/useAutomations.ts`:

```ts
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  DaemonHttpError,
  type Automation,
  type AutomationInput,
  type AutomationPatch,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';

export interface AutomationsOptions {
  /** The companion whose automations these are; null reads nothing. */
  agentId: string | null;
  /** `LiveState.automationsVersion`: bumped by every `automation.updated`. */
  version: number;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
  /** False while the daemon is offline: nothing is read. */
  enabled: boolean;
}

export interface AutomationsView {
  automations: Automation[];
  loaded: boolean;
  error: string | null;
  /** The HTTP status of the failed action, when the daemon refused it. */
  errorStatus: number | null;
  refresh: () => void;
  /** Each answers true when the daemon took it, after the list was read
   *  again. */
  create: (input: AutomationInput) => Promise<boolean>;
  createHeartbeat: (timeZone: string) => Promise<boolean>;
  update: (automation: Automation, patch: AutomationPatch) => Promise<boolean>;
  setEnabled: (automation: Automation, enabled: boolean) => Promise<boolean>;
  remove: (automation: Automation) => Promise<boolean>;
  runNow: (automation: Automation) => Promise<boolean>;
}

function message(error: unknown): string {
  return error instanceof DaemonHttpError
    ? error.message
    : COMPANION_UNREACHABLE;
}

/** The companion's automations (spec §15.4, §15.5 `useAutomations`). */
export function useAutomations({
  agentId,
  version,
  epoch,
  enabled,
}: AutomationsOptions): AutomationsView {
  const [automations, setAutomations] = useState<Automation[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [readError, setReadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionStatus, setActionStatus] = useState<number | null>(null);
  const reading = useRef<AbortController | null>(null);

  const load = useCallback(async () => {
    if (!agentId) return;
    reading.current?.abort();
    const controller = new AbortController();
    reading.current = controller;
    try {
      const next = await daemon.listAutomations(agentId, {
        signal: controller.signal,
      });
      if (controller.signal.aborted) return;
      // An empty reload keeps the empty list it had, so a harness that goes
      // online with no automations renders nothing more (the M5 I4 lesson).
      setAutomations((previous) =>
        previous.length === 0 && next.length === 0 ? previous : next,
      );
      setReadError(null);
      setLoaded(true);
    } catch (caught) {
      if (controller.signal.aborted) return;
      setReadError(message(caught));
      setLoaded(true);
    }
  }, [agentId]);

  useEffect(() => {
    if (!enabled || !agentId) return;
    void load();
  }, [enabled, agentId, version, epoch, load]);

  useEffect(() => () => reading.current?.abort(), []);

  const refresh = useCallback(() => void load(), [load]);

  const act = useCallback(
    async (work: (agentId: string) => Promise<unknown>) => {
      if (!agentId) return false;
      setActionError(null);
      setActionStatus(null);
      try {
        await work(agentId);
      } catch (caught) {
        setActionError(message(caught));
        if (caught instanceof DaemonHttpError) {
          setActionStatus(caught.status);
          if (caught.status === 404 || caught.status === 409) await load();
        }
        return false;
      }
      await load();
      return true;
    },
    [agentId, load],
  );

  const create = useCallback(
    (input: AutomationInput) => act((id) => daemon.createAutomation(id, input)),
    [act],
  );
  const createHeartbeat = useCallback(
    (timeZone: string) => act((id) => daemon.createHeartbeat(id, { timeZone })),
    [act],
  );
  const update = useCallback(
    (automation: Automation, patch: AutomationPatch) =>
      act((id) => daemon.updateAutomation(id, automation.id, patch)),
    [act],
  );
  const setEnabled = useCallback(
    (automation: Automation, value: boolean) =>
      act((id) =>
        daemon.updateAutomation(id, automation.id, { enabled: value }),
      ),
    [act],
  );
  const remove = useCallback(
    (automation: Automation) =>
      act((id) => daemon.deleteAutomation(id, automation.id)),
    [act],
  );
  const runNow = useCallback(
    (automation: Automation) =>
      act((id) => daemon.runAutomationNow(id, automation.id)),
    [act],
  );

  return {
    automations,
    loaded,
    error: actionError ?? readError,
    errorStatus: actionError ? actionStatus : null,
    refresh,
    create,
    createHeartbeat,
    update,
    setEnabled,
    remove,
    runNow,
  };
}
```

In `apps/web/src/lib/agent-access.ts`, add `'list_automations',` after `'load_skill',` in `COMMON_TOOLS`, and `'create_automation', 'pause_automation',` after `'propose_skill',` in `COLLABORATE_TOOLS`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd apps/web && bun x vitest run src/lib/schedule-parse.test.ts src/lib/automations.test.ts src/hooks/useAutomations.test.tsx src/lib/session-events.test.ts src/lib/agent-access.test.ts 2>&1 | tail -30`
Expected: PASS, with no `act()` warnings or console noise.

Run: `bun x nx run @animaOS-SWARM/web:typecheck 2>&1 | tail -15`
Expected: success.

- [ ] **Step 6: Format and commit**

```bash
bun x nx format:write --files=apps/web/src/lib/schedule-parse.ts,apps/web/src/lib/schedule-parse.test.ts,apps/web/src/lib/automations.ts,apps/web/src/lib/automations.test.ts,apps/web/src/hooks/useAutomations.ts,apps/web/src/hooks/useAutomations.test.tsx,apps/web/src/test/automations.ts,apps/web/src/test/live.ts,apps/web/src/lib/session-events.ts,apps/web/src/lib/session-events.test.ts,apps/web/src/lib/daemon-api.ts,apps/web/src/lib/agent-access.ts,apps/web/src/lib/agent-access.test.ts
git diff --stat
git add apps/web/src/lib/schedule-parse.ts apps/web/src/lib/schedule-parse.test.ts apps/web/src/lib/automations.ts apps/web/src/lib/automations.test.ts apps/web/src/hooks/useAutomations.ts apps/web/src/hooks/useAutomations.test.tsx apps/web/src/test/automations.ts apps/web/src/test/live.ts apps/web/src/lib/session-events.ts apps/web/src/lib/session-events.test.ts apps/web/src/lib/daemon-api.ts apps/web/src/lib/agent-access.ts apps/web/src/lib/agent-access.test.ts
git commit -m "feat(web): add the automations data layer, phrase parser, and access profiles"
```

Add any other file the `tsc` step above made you touch to both commands.

Recommended implementer tier: standard (complete code; the parser's tests pin every phrase).

---

### Task 11: Web Automations page: list, editor with preview and active hours, heartbeat, Run now, and the history drawer

**Files:**

- Create: `apps/web/src/pages/AutomationsPage.tsx`, `apps/web/src/pages/AutomationsPage.test.tsx`, `apps/web/src/components/automations/AutomationEditor.tsx`, `apps/web/src/components/automations/AutomationHistory.tsx`, `apps/web/src/automations.css`
- Modify: `apps/web/src/styles.css` (one `@import`)

**Interfaces:**

- Consumes: Task 10's `AutomationsView`, `parseSchedule`, `PHRASE_EXAMPLES`, `describeTrigger`, `describeActiveHours`, `OUTCOME_LABELS`, `RUN_OUTCOME_LABELS`, `DAY_NAMES`, `localTimeZone`, `PHRASE_NOT_UNDERSTOOD`, `AGENT_CREATED_NOTE`, `daemon.{previewAutomation, automationHistory}`; `formatWhen`, `COMPANION_UNREACHABLE` (`lib/approvals.ts`); SDK types and `MAX_AUTOMATION_NAME_CHARS`.
- Produces:
  - `AutomationsPage({ view, agentId, version, online, telegramConnectorId, focusId?, onFocusHandled?, onOpenSession })` (spec §15.4): the list with next run, last outcome, failures, creator, and badges; New automation and Add heartbeat; Run now, Pause/Resume, Edit, History, Open thread, and Delete with a confirmation; the editor; the history drawer.
  - `AutomationEditor({ automation, telegramConnectorId, onSave, onCancel })` and `AutomationDraft { name, prompt, trigger: AutomationTrigger | null, activeHours: ActiveHours | null, target }` (`trigger: null` while editing keeps the current schedule).
  - `AutomationHistory({ agentId, automation, version, onClose, onOpenSession })`: the latest 50 runs, read again when `version` moves.
- Behavior: the page renders every name, prompt, and history field as text (never Markdown or HTML). The editor turns a phrase into a trigger with `parseSchedule` in the chosen time zone (the browser's by default), or takes a cron expression; it shows the daemon's preview of the next three runs (`daemon.previewAutomation`, a superseded request aborted) or the daemon's problem, and `PHRASE_NOT_UNDERSTOOD` for a phrase it cannot read (Create stays off). Editing with an empty phrase keeps the schedule. Active hours default to 08:00–22:00 every day when turned on. Telegram is offered only when the companion has a connector with an approved chat. Add heartbeat sends the browser's time zone and is off once a heartbeat exists. `focusId` opens that automation's editor once it is listed, then calls `onFocusHandled`.

- [ ] **Step 1: Write the failing tests**

Create `apps/web/src/pages/AutomationsPage.test.tsx`:

```tsx
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import type { AutomationsView } from '../hooks/useAutomations';
import {
  AGENT_CREATED_NOTE,
  PHRASE_NOT_UNDERSTOOD,
  localTimeZone,
} from '../lib/automations';
import { daemon } from '../lib/daemon-api';
import { automationFixture, automationRunFixture } from '../test/automations';
import { AutomationsPage } from './AutomationsPage';

function fakeView(overrides: Partial<AutomationsView> = {}): AutomationsView {
  return {
    automations: [],
    loaded: true,
    error: null,
    errorStatus: null,
    refresh: vi.fn(),
    create: vi.fn().mockResolvedValue(true),
    createHeartbeat: vi.fn().mockResolvedValue(true),
    update: vi.fn().mockResolvedValue(true),
    setEnabled: vi.fn().mockResolvedValue(true),
    remove: vi.fn().mockResolvedValue(true),
    runNow: vi.fn().mockResolvedValue(true),
    ...overrides,
  };
}

function renderPage(view: AutomationsView, extra: { focusId?: string } = {}) {
  const onOpenSession = vi.fn();
  const onFocusHandled = vi.fn();
  render(
    <AutomationsPage
      view={view}
      agentId="agent-main"
      version={0}
      online
      telegramConnectorId={null}
      focusId={extra.focusId ?? null}
      onFocusHandled={onFocusHandled}
      onOpenSession={onOpenSession}
    />,
  );
  return { onOpenSession, onFocusHandled };
}

const agentMade = automationFixture('schedule-1', {
  name: '<b>Stretch</b>',
  prompt: '**Remind** me to stretch',
  activeHours: {
    start: '08:00',
    end: '22:00',
    days: [1, 2, 3, 4, 5],
    timeZone: 'UTC',
  },
  lastOutcome: {
    status: 'error',
    occurredAtMs: 5,
    errorCode: 'schedule_run_failed',
  },
  lastFiredAtMs: 5,
  counters: { runs: 3, failures: 2, consecutiveFailures: 2 },
  createdBy: {
    kind: 'agent',
    agentId: 'agent-main',
    sessionId: 'chat:1',
    runId: 'run_1',
    toolCallId: 'call-1',
  },
});
const heartbeat = automationFixture('schedule-2', {
  name: 'Heartbeat',
  preset: 'heartbeat',
  enabled: false,
});

beforeEach(() => {
  // Settled only by the tests that read them, so no update lands after a
  // test ends.
  vi.spyOn(daemon, 'previewAutomation').mockReturnValue(new Promise(() => {}));
  vi.spyOn(daemon, 'automationHistory').mockReturnValue(new Promise(() => {}));
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AutomationsPage', () => {
  it('lists automations with their schedule, outcome, and creator as text', () => {
    renderPage(fakeView({ automations: [agentMade, heartbeat] }));

    const row = screen.getByRole('article', { name: '<b>Stretch</b>' });
    expect(within(row).getByText('<b>Stretch</b>')).toBeInTheDocument();
    expect(
      within(row).getByText('**Remind** me to stretch'),
    ).toBeInTheDocument();
    expect(
      within(row).getByText('every 30 min · 08:00–22:00, weekdays (UTC)'),
    ).toBeInTheDocument();
    expect(within(row).getByText(/Last: Failed/)).toBeInTheDocument();
    expect(within(row).getByText(/2 failed in a row/)).toBeInTheDocument();
    expect(within(row).getByText(AGENT_CREATED_NOTE)).toBeInTheDocument();
    const paused = screen.getByRole('article', { name: 'Heartbeat' });
    expect(within(paused).getAllByText('Paused').length).toBeGreaterThan(0);
    expect(
      screen.getByRole('button', { name: 'Add heartbeat' }),
    ).toBeDisabled();
    expect(document.querySelector('.automations-page strong')).toBeNull();
  });

  it('runs, pauses, resumes, opens the thread, and deletes after confirming', async () => {
    const user = userEvent.setup();
    const view = fakeView({ automations: [agentMade, heartbeat] });
    const { onOpenSession } = renderPage(view);
    const row = screen.getByRole('article', { name: '<b>Stretch</b>' });

    await user.click(within(row).getByRole('button', { name: 'Run now' }));
    expect(view.runNow).toHaveBeenCalledWith(agentMade);
    await user.click(within(row).getByRole('button', { name: 'Pause' }));
    expect(view.setEnabled).toHaveBeenCalledWith(agentMade, false);
    await user.click(
      within(screen.getByRole('article', { name: 'Heartbeat' })).getByRole(
        'button',
        { name: 'Resume' },
      ),
    );
    expect(view.setEnabled).toHaveBeenCalledWith(heartbeat, true);
    await user.click(within(row).getByRole('button', { name: 'Open thread' }));
    expect(onOpenSession).toHaveBeenCalledWith('schedule:schedule-1');

    await user.click(within(row).getByRole('button', { name: 'Delete' }));
    expect(view.remove).not.toHaveBeenCalled();
    await user.click(
      within(row).getByRole('button', { name: 'Confirm delete' }),
    );
    expect(view.remove).toHaveBeenCalledWith(agentMade);
  });

  it('creates an automation from a phrase with the daemon preview', async () => {
    const user = userEvent.setup();
    const preview = vi
      .mocked(daemon.previewAutomation)
      .mockResolvedValue([
        Date.UTC(2026, 0, 5, 10),
        Date.UTC(2026, 0, 5, 12),
        Date.UTC(2026, 0, 5, 14),
      ]);
    const view = fakeView();
    renderPage(view);

    await user.click(screen.getByRole('button', { name: 'New automation' }));
    const form = screen.getByRole('form', { name: 'New automation' });
    await user.type(within(form).getByLabelText('Name'), 'Stretch');
    await user.type(
      within(form).getByLabelText('Prompt'),
      'Remind me to stretch',
    );
    await user.type(within(form).getByLabelText('When'), 'every 2 hours');

    const runs = await within(form).findByRole('region', { name: 'Next runs' });
    expect(within(runs).getAllByRole('listitem')).toHaveLength(3);
    expect(preview).toHaveBeenLastCalledWith(
      { trigger: { type: 'interval', intervalMs: 7_200_000 } },
      { signal: expect.any(AbortSignal) },
    );
    await user.click(
      within(form).getByRole('button', { name: 'Create automation' }),
    );

    expect(view.create).toHaveBeenCalledWith({
      prompt: 'Remind me to stretch',
      trigger: { type: 'interval', intervalMs: 7_200_000 },
      target: { type: 'workspace' },
      name: 'Stretch',
    });
    await waitFor(() =>
      expect(screen.queryByRole('form', { name: 'New automation' })).toBeNull(),
    );
  });

  it('explains an unknown phrase and shows the daemon problem with a cron expression', async () => {
    const user = userEvent.setup();
    renderPage(fakeView());
    await user.click(screen.getByRole('button', { name: 'New automation' }));
    const form = screen.getByRole('form', { name: 'New automation' });
    await user.type(within(form).getByLabelText('Prompt'), 'Check');
    await user.type(within(form).getByLabelText('When'), 'whenever');

    expect(within(form).getByText(PHRASE_NOT_UNDERSTOOD)).toBeInTheDocument();
    expect(
      within(form).getByRole('button', { name: 'Create automation' }),
    ).toBeDisabled();

    vi.mocked(daemon.previewAutomation).mockRejectedValue(
      new DaemonHttpError(400, { error: 'minute: must be from 0 to 59' }),
    );
    await user.click(within(form).getByLabelText('Use a cron expression'));
    await user.type(
      within(form).getByLabelText('Cron expression'),
      '61 * * * *',
    );

    expect(
      await within(form).findByText('minute: must be from 0 to 59'),
    ).toBeInTheDocument();
  });

  it('sends active hours with a new automation', async () => {
    const user = userEvent.setup();
    const view = fakeView();
    renderPage(view);
    await user.click(screen.getByRole('button', { name: 'New automation' }));
    const form = screen.getByRole('form', { name: 'New automation' });
    await user.type(within(form).getByLabelText('Prompt'), 'Check');
    await user.type(within(form).getByLabelText('When'), 'every hour');
    await user.click(within(form).getByLabelText('Only run between'));
    await user.click(within(form).getByLabelText('Sun'));
    await user.click(within(form).getByLabelText('Sat'));
    await user.click(
      within(form).getByRole('button', { name: 'Create automation' }),
    );

    expect(view.create).toHaveBeenCalledWith({
      prompt: 'Check',
      trigger: { type: 'interval', intervalMs: 3_600_000 },
      target: { type: 'workspace' },
      activeHours: {
        start: '08:00',
        end: '22:00',
        days: [1, 2, 3, 4, 5],
        timeZone: localTimeZone(),
      },
    });
  });

  it('edits without touching the schedule and can clear active hours', async () => {
    const user = userEvent.setup();
    const view = fakeView({ automations: [agentMade] });
    renderPage(view);
    await user.click(
      within(screen.getByRole('article', { name: '<b>Stretch</b>' })).getByRole(
        'button',
        { name: 'Edit' },
      ),
    );
    const form = screen.getByRole('form', { name: 'Edit <b>Stretch</b>' });
    const prompt = within(form).getByLabelText('Prompt');
    await user.clear(prompt);
    await user.type(prompt, 'Stand up');
    await user.click(within(form).getByLabelText('Only run between'));
    await user.click(
      within(form).getByRole('button', { name: 'Save changes' }),
    );

    expect(view.update).toHaveBeenCalledWith(agentMade, {
      name: '<b>Stretch</b>',
      prompt: 'Stand up',
      target: { type: 'workspace' },
      activeHours: null,
    });
  });

  it('adds the heartbeat in the browser time zone', async () => {
    const user = userEvent.setup();
    const view = fakeView();
    renderPage(view);
    await user.click(screen.getByRole('button', { name: 'Add heartbeat' }));
    expect(view.createHeartbeat).toHaveBeenCalledWith(localTimeZone());
  });

  it('shows the latest runs in the history drawer', async () => {
    const user = userEvent.setup();
    vi.mocked(daemon.automationHistory).mockResolvedValue([
      automationRunFixture('run-a', {
        outcome: 'failed',
        manual: true,
        errorCode: 'schedule_run_failed',
      }),
    ]);
    renderPage(fakeView({ automations: [agentMade] }));
    await user.click(
      within(screen.getByRole('article', { name: '<b>Stretch</b>' })).getByRole(
        'button',
        { name: 'History' },
      ),
    );

    const drawer = screen.getByRole('complementary', {
      name: 'History of <b>Stretch</b>',
    });
    expect(await within(drawer).findByText('Failed')).toBeInTheDocument();
    expect(within(drawer).getByText('Run now')).toBeInTheDocument();
    expect(within(drawer).getByText('schedule_run_failed')).toBeInTheDocument();
    expect(daemon.automationHistory).toHaveBeenCalledWith(
      'agent-main',
      'schedule-1',
      { signal: expect.any(AbortSignal) },
    );
    await user.click(
      within(drawer).getByRole('button', { name: 'Close history' }),
    );
    expect(screen.queryByRole('complementary')).toBeNull();
  });

  it('opens the editor of a focused automation', () => {
    const { onFocusHandled } = renderPage(
      fakeView({ automations: [agentMade] }),
      {
        focusId: 'schedule-1',
      },
    );
    expect(
      screen.getByRole('form', { name: 'Edit <b>Stretch</b>' }),
    ).toBeInTheDocument();
    expect(onFocusHandled).toHaveBeenCalledTimes(1);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/web && bun x vitest run src/pages/AutomationsPage.test.tsx 2>&1 | tail -30`
Expected: FAIL (`./AutomationsPage` does not exist).

- [ ] **Step 3: The editor and the history drawer**

Create `apps/web/src/components/automations/AutomationEditor.tsx`:

```tsx
import { useEffect, useMemo, useState } from 'react';
import {
  DaemonHttpError,
  MAX_AUTOMATION_NAME_CHARS,
  type ActiveHours,
  type Automation,
  type AutomationTarget,
  type AutomationTrigger,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE, formatWhen } from '../../lib/approvals';
import {
  DAY_NAMES,
  PHRASE_NOT_UNDERSTOOD,
  describeTrigger,
  localTimeZone,
} from '../../lib/automations';
import { daemon } from '../../lib/daemon-api';
import { PHRASE_EXAMPLES, parseSchedule } from '../../lib/schedule-parse';

export interface AutomationDraft {
  name: string;
  prompt: string;
  /** Null while editing: the schedule stays as it is. */
  trigger: AutomationTrigger | null;
  activeHours: ActiveHours | null;
  target: AutomationTarget;
}

export interface AutomationEditorProps {
  /** Null for a new automation. */
  automation: Automation | null;
  /** The companion's Telegram connector with an approved chat, if any. */
  telegramConnectorId: string | null;
  /** True when the daemon took it. */
  onSave: (draft: AutomationDraft) => Promise<boolean>;
  onCancel: () => void;
}

type Preview = { runs: number[] } | { error: string } | null;

const EVERY_DAY = [0, 1, 2, 3, 4, 5, 6];

function zoneOf(automation: Automation | null): string {
  const trigger = automation?.trigger;
  if (trigger && (trigger.type === 'cron' || trigger.type === 'daily'))
    return trigger.timeZone;
  return automation?.activeHours?.timeZone ?? localTimeZone();
}

/** Create or edit an automation (spec §15.4): a plain-language schedule or
 *  a cron expression, the daemon's preview of the next runs, active hours,
 *  and where it runs. */
export function AutomationEditor({
  automation,
  telegramConnectorId,
  onSave,
  onCancel,
}: AutomationEditorProps) {
  const [name, setName] = useState(automation?.name ?? '');
  const [prompt, setPrompt] = useState(automation?.prompt ?? '');
  const [cronMode, setCronMode] = useState(automation?.trigger.type === 'cron');
  const [phrase, setPhrase] = useState('');
  const [expression, setExpression] = useState(
    automation?.trigger.type === 'cron' ? automation.trigger.expression : '',
  );
  const [timeZone, setTimeZone] = useState(() => zoneOf(automation));
  const [hoursOn, setHoursOn] = useState(automation?.activeHours != null);
  const [start, setStart] = useState(automation?.activeHours?.start ?? '08:00');
  const [end, setEnd] = useState(automation?.activeHours?.end ?? '22:00');
  const [days, setDays] = useState<number[]>(
    automation?.activeHours?.days ?? EVERY_DAY,
  );
  const [target, setTarget] = useState<'workspace' | 'telegram'>(
    automation?.target.type === 'connector' ? 'telegram' : 'workspace',
  );
  const [preview, setPreview] = useState<Preview>(null);
  const [saving, setSaving] = useState(false);

  const trigger = useMemo<AutomationTrigger | null>(() => {
    if (cronMode)
      return expression.trim()
        ? { type: 'cron', expression: expression.trim(), timeZone }
        : null;
    if (!phrase.trim()) return null;
    return parseSchedule(phrase, { nowMs: Date.now(), timeZone });
  }, [cronMode, expression, phrase, timeZone]);
  const phraseProblem = !cronMode && phrase.trim() !== '' && trigger === null;
  const activeHours: ActiveHours | null = hoursOn
    ? { start, end, days: [...days].sort((a, b) => a - b), timeZone }
    : null;
  const shown = trigger ?? (cronMode ? null : (automation?.trigger ?? null));
  const previewKey = JSON.stringify([
    phraseProblem ? null : shown,
    activeHours,
  ]);

  useEffect(() => {
    const [previewed, hours] = JSON.parse(previewKey) as [
      AutomationTrigger | null,
      ActiveHours | null,
    ];
    if (!previewed) {
      setPreview(null);
      return;
    }
    const controller = new AbortController();
    daemon
      .previewAutomation(
        { trigger: previewed, ...(hours ? { activeHours: hours } : {}) },
        { signal: controller.signal },
      )
      .then((runs) => {
        if (!controller.signal.aborted) setPreview({ runs });
      })
      .catch((caught: unknown) => {
        if (controller.signal.aborted) return;
        setPreview({
          error:
            caught instanceof DaemonHttpError
              ? caught.message
              : COMPANION_UNREACHABLE,
        });
      });
    return () => controller.abort();
  }, [previewKey]);

  const keepsSchedule = automation !== null && !cronMode && trigger === null;
  const canSave =
    prompt.trim() !== '' &&
    !phraseProblem &&
    (trigger !== null || keepsSchedule) &&
    (!hoursOn || days.length > 0) &&
    !saving;

  const submit = async () => {
    if (!canSave) return;
    setSaving(true);
    const saved = await onSave({
      name: name.trim(),
      prompt,
      trigger,
      activeHours,
      target:
        target === 'telegram' && telegramConnectorId
          ? { type: 'connector', connectorId: telegramConnectorId }
          : { type: 'workspace' },
    });
    if (!saved) setSaving(false);
  };

  const toggleDay = (day: number) =>
    setDays((current) =>
      current.includes(day)
        ? current.filter((item) => item !== day)
        : [...current, day],
    );

  return (
    <form
      className="automation-editor"
      aria-label={automation ? `Edit ${automation.name}` : 'New automation'}
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <label className="automation-field">
        Name
        <input
          value={name}
          maxLength={MAX_AUTOMATION_NAME_CHARS}
          placeholder="From the prompt when empty"
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label className="automation-field">
        Prompt
        <textarea
          value={prompt}
          rows={3}
          onChange={(event) => setPrompt(event.target.value)}
        />
      </label>
      <fieldset className="automation-fieldset">
        <legend>Schedule</legend>
        {cronMode ? (
          <label className="automation-field">
            Cron expression
            <input
              value={expression}
              placeholder="0 9 * * 1-5"
              onChange={(event) => setExpression(event.target.value)}
            />
          </label>
        ) : (
          <label className="automation-field">
            When
            <input
              value={phrase}
              placeholder={
                automation
                  ? `Keep: ${describeTrigger(automation.trigger)}`
                  : PHRASE_EXAMPLES[1]
              }
              onChange={(event) => setPhrase(event.target.value)}
            />
          </label>
        )}
        <label className="automation-check">
          <input
            type="checkbox"
            checked={cronMode}
            onChange={(event) => setCronMode(event.target.checked)}
          />
          Use a cron expression
        </label>
        <label className="automation-field">
          Time zone
          <input
            value={timeZone}
            onChange={(event) => setTimeZone(event.target.value)}
          />
        </label>
        {phraseProblem && (
          <p className="automation-problem" role="status">
            {PHRASE_NOT_UNDERSTOOD}
          </p>
        )}
        {preview && 'runs' in preview && (
          <section className="automation-preview" aria-label="Next runs">
            <ul>
              {preview.runs.map((run) => (
                <li key={run}>{formatWhen(run)}</li>
              ))}
            </ul>
          </section>
        )}
        {preview && 'error' in preview && (
          <p className="automation-problem" role="status">
            {preview.error}
          </p>
        )}
      </fieldset>
      <fieldset className="automation-fieldset">
        <legend>Active hours</legend>
        <label className="automation-check">
          <input
            type="checkbox"
            checked={hoursOn}
            onChange={(event) => setHoursOn(event.target.checked)}
          />
          Only run between
        </label>
        {hoursOn && (
          <div className="automation-hours">
            <label className="automation-field">
              From
              <input
                type="time"
                value={start}
                onChange={(event) => setStart(event.target.value)}
              />
            </label>
            <label className="automation-field">
              To
              <input
                type="time"
                value={end}
                onChange={(event) => setEnd(event.target.value)}
              />
            </label>
            <div className="automation-days">
              {DAY_NAMES.map((day, index) => (
                <label key={day} className="automation-check">
                  <input
                    type="checkbox"
                    checked={days.includes(index)}
                    onChange={() => toggleDay(index)}
                  />
                  {day}
                </label>
              ))}
            </div>
          </div>
        )}
      </fieldset>
      <label className="automation-field">
        Runs in
        <select
          value={target}
          onChange={(event) =>
            setTarget(event.target.value as 'workspace' | 'telegram')
          }
        >
          <option value="workspace">Its own thread</option>
          <option value="telegram" disabled={!telegramConnectorId}>
            Telegram
          </option>
        </select>
      </label>
      <div className="automation-actions">
        <button
          type="submit"
          className="studio-tool-button"
          disabled={!canSave}
        >
          {automation ? 'Save changes' : 'Create automation'}
        </button>
        <button type="button" className="studio-tool-button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
```

Create `apps/web/src/components/automations/AutomationHistory.tsx`:

```tsx
import { useEffect, useState } from 'react';
import {
  DaemonHttpError,
  type Automation,
  type AutomationRun,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE, formatWhen } from '../../lib/approvals';
import { RUN_OUTCOME_LABELS } from '../../lib/automations';
import { daemon } from '../../lib/daemon-api';

/** An automation's latest runs, newest first (spec §9.1, §15.4). */
export function AutomationHistory({
  agentId,
  automation,
  version,
  onClose,
  onOpenSession,
}: {
  agentId: string;
  automation: Automation;
  /** `LiveState.automationsVersion`: a new run reads the list again. */
  version: number;
  onClose: () => void;
  onOpenSession: (sessionId: string) => void;
}) {
  const [runs, setRuns] = useState<AutomationRun[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    daemon
      .automationHistory(agentId, automation.id, { signal: controller.signal })
      .then((next) => {
        if (controller.signal.aborted) return;
        setRuns(next);
        setError(null);
      })
      .catch((caught: unknown) => {
        if (controller.signal.aborted) return;
        setError(
          caught instanceof DaemonHttpError
            ? caught.message
            : COMPANION_UNREACHABLE,
        );
      });
    return () => controller.abort();
  }, [agentId, automation.id, version]);

  return (
    <aside
      className="automation-history"
      aria-label={`History of ${automation.name}`}
    >
      <div className="automation-history-header">
        <h3>History</h3>
        <button type="button" className="studio-tool-button" onClick={onClose}>
          Close history
        </button>
      </div>
      {error && (
        <p className="automations-error" role="alert">
          {error}
        </p>
      )}
      {runs === null && !error && <p className="automations-empty">Loading…</p>}
      {runs !== null && runs.length === 0 && (
        <p className="automations-empty">It has not run yet.</p>
      )}
      {runs !== null && runs.length > 0 && (
        <ul className="automation-history-list">
          {runs.map((run) => (
            <li key={run.id} className="automation-history-run">
              <span>{formatWhen(run.firedAtMs)}</span>
              <span data-outcome={run.outcome}>
                {RUN_OUTCOME_LABELS[run.outcome]}
              </span>
              {run.manual && <span className="automation-badge">Run now</span>}
              {run.errorCode && <code>{run.errorCode}</code>}
              {run.sessionId && (
                <button
                  type="button"
                  className="studio-tool-button"
                  onClick={() => onOpenSession(run.sessionId as string)}
                >
                  Open
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
```

- [ ] **Step 4: The page and its styles**

Create `apps/web/src/pages/AutomationsPage.tsx`:

```tsx
import { useEffect, useState } from 'react';
import type { Automation } from '@animaOS-SWARM/sdk';

import {
  AutomationEditor,
  type AutomationDraft,
} from '../components/automations/AutomationEditor';
import { AutomationHistory } from '../components/automations/AutomationHistory';
import type { AutomationsView } from '../hooks/useAutomations';
import { formatWhen } from '../lib/approvals';
import {
  AGENT_CREATED_NOTE,
  OUTCOME_LABELS,
  describeActiveHours,
  describeTrigger,
  localTimeZone,
} from '../lib/automations';

export interface AutomationsPageProps {
  view: AutomationsView;
  agentId: string;
  /** `LiveState.automationsVersion`, so an open history reads again. */
  version: number;
  online: boolean;
  /** The companion's Telegram connector with an approved chat, if any. */
  telegramConnectorId: string | null;
  /** An automation to open in the editor ("Edit automation" in a check-in). */
  focusId?: string | null;
  onFocusHandled?: () => void;
  onOpenSession: (sessionId: string) => void;
}

/** Spec §15.4: automations with their next run, last outcome, and
 *  failures; create and edit; Run now, pause and resume, delete; history. */
export function AutomationsPage({
  view,
  agentId,
  version,
  online,
  telegramConnectorId,
  focusId = null,
  onFocusHandled,
  onOpenSession,
}: AutomationsPageProps) {
  const [editing, setEditing] = useState<{
    automation: Automation | null;
  } | null>(null);
  const [historyId, setHistoryId] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  useEffect(() => {
    if (!focusId) return;
    const focused = view.automations.find((item) => item.id === focusId);
    if (!focused) return;
    setEditing({ automation: focused });
    onFocusHandled?.();
  }, [focusId, view.automations, onFocusHandled]);

  const hasHeartbeat = view.automations.some(
    (item) => item.preset === 'heartbeat',
  );
  const history = historyId
    ? (view.automations.find((item) => item.id === historyId) ?? null)
    : null;

  const save = async (draft: AutomationDraft) => {
    const current = editing?.automation ?? null;
    let saved = false;
    if (current) {
      saved = await view.update(current, {
        name: draft.name,
        prompt: draft.prompt,
        target: draft.target,
        activeHours: draft.activeHours,
        ...(draft.trigger ? { trigger: draft.trigger } : {}),
      });
    } else if (draft.trigger) {
      saved = await view.create({
        prompt: draft.prompt,
        trigger: draft.trigger,
        target: draft.target,
        ...(draft.name ? { name: draft.name } : {}),
        ...(draft.activeHours ? { activeHours: draft.activeHours } : {}),
      });
    }
    if (saved) setEditing(null);
    return saved;
  };

  return (
    <div className="automations-page">
      {view.error && (
        <p className="automations-error" role="alert">
          {view.error}
        </p>
      )}
      <section
        className="automations-section"
        aria-labelledby="automations-heading"
      >
        <div className="automations-header">
          <h2 id="automations-heading">Automations</h2>
          <button
            type="button"
            className="studio-tool-button"
            disabled={!online}
            onClick={() => setEditing({ automation: null })}
          >
            New automation
          </button>
          <button
            type="button"
            className="studio-tool-button"
            disabled={!online || hasHeartbeat}
            onClick={() => void view.createHeartbeat(localTimeZone())}
          >
            Add heartbeat
          </button>
        </div>
        {editing && (
          <AutomationEditor
            key={editing.automation?.id ?? 'new'}
            automation={editing.automation}
            telegramConnectorId={telegramConnectorId}
            onSave={save}
            onCancel={() => setEditing(null)}
          />
        )}
        {view.loaded && view.automations.length === 0 ? (
          <p className="automations-empty">
            No automations yet. Create one, add the heartbeat, or ask your
            companion to schedule something.
          </p>
        ) : (
          <ul className="automations-list">
            {view.automations.map((automation) => (
              <li key={automation.id}>
                <article
                  className="automation-row"
                  aria-label={automation.name}
                  data-enabled={automation.enabled || undefined}
                >
                  <div className="automation-row-header">
                    <h3 className="automation-name">{automation.name}</h3>
                    {automation.preset === 'heartbeat' && (
                      <span className="automation-badge">Heartbeat</span>
                    )}
                    {automation.running && (
                      <span className="automation-badge" data-running>
                        Running
                      </span>
                    )}
                    {!automation.enabled && (
                      <span className="automation-badge">Paused</span>
                    )}
                  </div>
                  <p className="automation-schedule">
                    {describeTrigger(automation.trigger)}
                    {automation.activeHours &&
                      ` · ${describeActiveHours(automation.activeHours)}`}
                  </p>
                  <p className="automation-meta">
                    {automation.enabled
                      ? `Next run ${formatWhen(automation.nextDueAtMs)}`
                      : 'Paused'}
                    {automation.lastOutcome &&
                      ` · Last: ${OUTCOME_LABELS[automation.lastOutcome.status]}`}
                    {automation.counters.consecutiveFailures > 0 &&
                      ` · ${automation.counters.consecutiveFailures} failed in a row`}
                    {automation.target.type === 'connector' && ' · Telegram'}
                  </p>
                  {automation.createdBy.kind === 'agent' && (
                    <p className="automation-note">{AGENT_CREATED_NOTE}</p>
                  )}
                  <pre className="automation-prompt">{automation.prompt}</pre>
                  <div className="automation-actions">
                    <button
                      type="button"
                      className="studio-tool-button"
                      disabled={!online || automation.running}
                      onClick={() => void view.runNow(automation)}
                    >
                      Run now
                    </button>
                    <button
                      type="button"
                      className="studio-tool-button"
                      disabled={!online}
                      onClick={() =>
                        void view.setEnabled(automation, !automation.enabled)
                      }
                    >
                      {automation.enabled ? 'Pause' : 'Resume'}
                    </button>
                    <button
                      type="button"
                      className="studio-tool-button"
                      disabled={!online}
                      onClick={() => setEditing({ automation })}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      className="studio-tool-button"
                      onClick={() => setHistoryId(automation.id)}
                    >
                      History
                    </button>
                    {automation.lastFiredAtMs !== null &&
                      automation.target.type === 'workspace' && (
                        <button
                          type="button"
                          className="studio-tool-button"
                          onClick={() =>
                            onOpenSession(`schedule:${automation.id}`)
                          }
                        >
                          Open thread
                        </button>
                      )}
                    {confirming === automation.id ? (
                      <>
                        <span className="automations-empty">
                          Delete it? Its check-in thread stays until you delete
                          it.
                        </span>
                        <button
                          type="button"
                          className="studio-tool-button"
                          onClick={() => {
                            setConfirming(null);
                            void view.remove(automation);
                          }}
                        >
                          Confirm delete
                        </button>
                        <button
                          type="button"
                          className="studio-tool-button"
                          onClick={() => setConfirming(null)}
                        >
                          Keep
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        className="studio-tool-button"
                        disabled={!online}
                        onClick={() => setConfirming(automation.id)}
                      >
                        Delete
                      </button>
                    )}
                  </div>
                </article>
              </li>
            ))}
          </ul>
        )}
      </section>
      {history && (
        <AutomationHistory
          agentId={agentId}
          automation={history}
          version={version}
          onClose={() => setHistoryId(null)}
          onOpenSession={onOpenSession}
        />
      )}
    </div>
  );
}
```

Create `apps/web/src/automations.css`:

```css
/* The Automations page (spec §15.4). Names, prompts, and history are text. */
.automations-page {
  display: flex;
  flex-direction: column;
  gap: 24px;
  overflow-y: auto;
  padding: 24px;
}
.automations-section,
.automations-list,
.automation-editor,
.automation-history {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.automations-section h2 {
  color: var(--color-ink);
  font-size: 15px;
  font-weight: 600;
}
.automations-header,
.automation-row-header,
.automation-actions,
.automation-days,
.automation-hours,
.automation-history-header,
.automation-history-run {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}
.automation-row,
.automation-editor,
.automation-history {
  border: 1px solid var(--color-line);
  border-radius: 12px;
  padding: 10px 12px;
}
.automation-row:not([data-enabled]) {
  opacity: 0.75;
}
.automation-name {
  color: var(--color-ink);
  font-size: 14px;
  font-weight: 600;
  overflow-wrap: anywhere;
}
.automation-badge {
  border: 1px solid var(--color-line);
  border-radius: 999px;
  padding: 0 8px;
  color: var(--color-ink-2);
  font-size: 11px;
}
.automation-badge[data-running] {
  color: var(--color-mint);
}
.automation-schedule,
.automation-meta,
.automation-note,
.automations-empty {
  color: var(--color-ink-3);
  font-size: 12px;
}
.automation-prompt {
  max-height: 8rem;
  overflow: auto;
  border-radius: 8px;
  padding: 8px;
  background: var(--color-abyss);
  color: var(--color-ink-2);
  font-family: var(--font-mono);
  font-size: 11px;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.automation-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  color: var(--color-ink-2);
  font-size: 12px;
}
.automation-field input,
.automation-field textarea,
.automation-field select {
  border: 1px solid var(--color-line);
  border-radius: 8px;
  padding: 6px 8px;
  background: var(--color-abyss);
  color: var(--color-ink);
  font-size: 13px;
}
.automation-fieldset {
  display: flex;
  flex-direction: column;
  gap: 8px;
  border: 1px solid var(--color-line);
  border-radius: 8px;
  padding: 8px;
}
.automation-fieldset legend,
.automation-check {
  color: var(--color-ink-2);
  font-size: 12px;
}
.automation-check {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.automation-problem,
.automations-error {
  color: var(--color-danger);
  font-size: 13px;
}
.automation-preview ul,
.automation-history-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  color: var(--color-ink-2);
  font-size: 12px;
}
.automation-history-run [data-outcome='failed'] {
  color: var(--color-danger);
}
.automation-history-run code {
  color: var(--color-ink-3);
  font-family: var(--font-mono);
  font-size: 11px;
}
```

Add `@import './automations.css';` to `apps/web/src/styles.css` after `@import './skills.css';`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd apps/web && bun x vitest run src/pages/AutomationsPage.test.tsx src/visual-tokens.test.ts 2>&1 | tail -30`
Expected: PASS (9 page tests and the visual-token test), with no `act()` warnings or console noise.

Run: `bun x nx run @animaOS-SWARM/web:typecheck 2>&1 | tail -15`
Expected: success.

- [ ] **Step 6: Format and commit**

```bash
bun x nx format:write --files=apps/web/src/pages/AutomationsPage.tsx,apps/web/src/pages/AutomationsPage.test.tsx,apps/web/src/components/automations/AutomationEditor.tsx,apps/web/src/components/automations/AutomationHistory.tsx,apps/web/src/automations.css,apps/web/src/styles.css
git diff --stat
git add apps/web/src/pages/AutomationsPage.tsx apps/web/src/pages/AutomationsPage.test.tsx apps/web/src/components/automations/AutomationEditor.tsx apps/web/src/components/automations/AutomationHistory.tsx apps/web/src/automations.css apps/web/src/styles.css
git commit -m "feat(web): add the Automations page with previews, active hours, and history"
```

Recommended implementer tier: standard (complete components; the tests fix the copy and the calls).

---

### Task 12: Web notice cards with Undo, the check-in header, and the Automations destination

**Files:**

- Create: `apps/web/src/components/sessions/AutomationNoticeCard.tsx`
- Modify: `apps/web/src/lib/transcript.ts` (two `TranscriptActions` members), `apps/web/src/components/sessions/RunActivity.tsx`, `apps/web/src/components/sessions/RunActivity.test.tsx`, `apps/web/src/hooks/useTranscriptActions.ts`, `apps/web/src/hooks/useTranscriptActions.test.tsx`, `apps/web/src/components/sessions/SessionView.tsx`, `apps/web/src/components/sessions/SessionView.test.tsx`, `apps/web/src/components/icons.tsx` (`ClockIcon`), `apps/web/src/components/WorkspaceShell.tsx`, `apps/web/src/components/WorkspaceShell.test.tsx`, `apps/web/src/ViewHarness.tsx` (wiring lines), `apps/web/src/ViewHarness.test.tsx`, `apps/web/src/automations.css` (the card)

**Interfaces:**

- Consumes: Task 10's `useAutomations`, `automationNoticeFor`, `checkinScheduleId`, `describeTrigger`, `describeActiveHours`; Task 11's `AutomationsPage`; `formatWhen`.
- Produces:
  - `TranscriptActions.automationNotice?: (step: ToolStep) => Automation | null` and `TranscriptActions.onUndoAutomation?: (automation: Automation) => Promise<boolean>`; `TranscriptActionOptions.{automations?, undoAutomation?}`.
  - `AutomationNoticeCard({ automation, onUndo? })` (spec §15.2): a `note` named `Automation <name>` with the schedule, the next run, and Undo (delete), shown below its tool block even when the block is collapsed.
  - `SessionViewProps.{automation?, onEditAutomation?}`: a check-in's header badge reads `Check-in · <schedule>` (spec §15.2 "Check-in · every 30 min") and offers Edit automation.
  - `WorkspaceShell` gains `automations?: ReactNode | null` and the `automations` destination (after Approvals, spec §15.1 order); `AVAILABLE_PAGES` includes `automations`; `ClockIcon`.
  - `ViewHarness` keeps one `useAutomations` for the page, the notice cards, and the check-in header, and an `automationFocus` id that "Edit automation" sets before opening `#/automations`.
- Behavior: a successful `create_automation` call, live or in history, shows its automation's notice card while the automation exists; Undo deletes it (the list's next read removes the card), and a refused Undo enables the button again with the page's error. Everything is text (names are untrusted).

- [ ] **Step 1: Write the failing tests**

Add to `apps/web/src/components/sessions/RunActivity.test.tsx` (import `automationFixture` from `../../test/automations` and `automationNoticeFor` from `../../lib/automations`), inside `describe('ToolBlock', …)`:

```tsx
it('shows a notice card with Undo for a created automation, even collapsed', async () => {
  const user = userEvent.setup();
  const made = automationFixture('schedule-1', {
    name: 'Stretch',
    createdBy: {
      kind: 'agent',
      agentId: 'agent-main',
      sessionId: 'chat:1',
      runId: 'run_1',
      toolCallId: 'call-automation',
    },
  });
  const undo = vi.fn().mockResolvedValue(false);
  render(
    <ToolBlock
      steps={[
        step,
        { ...step, name: 'create_automation', toolCallId: 'call-automation' },
      ]}
      active={false}
      actions={{
        automationNotice: (candidate) => automationNoticeFor(candidate, [made]),
        onUndoAutomation: undo,
      }}
    />,
  );

  const card = screen.getByRole('note', { name: 'Automation Stretch' });
  expect(within(card).getByText(/every 30 min/)).toBeVisible();
  await user.click(within(card).getByRole('button', { name: 'Undo' }));
  expect(undo).toHaveBeenCalledWith(made);
  // A refused Undo can be tried again.
  expect(
    await within(card).findByRole('button', { name: 'Undo' }),
  ).toBeEnabled();
  expect(screen.getAllByRole('note')).toHaveLength(1);
});
```

Add to `apps/web/src/hooks/useTranscriptActions.test.tsx` (import `automationFixture` from `../test/automations`), inside `describe('useTranscriptActions', …)`:

```tsx
it('finds the automation a tool call made and undoes it', async () => {
  const made = automationFixture('schedule-1', {
    createdBy: {
      kind: 'agent',
      agentId: 'agent-main',
      sessionId: 'room-7',
      runId: 'run_7',
      toolCallId: 'call_a',
    },
  });
  const undoAutomation = vi.fn().mockResolvedValue(true);
  const { result } = actionsFor({ automations: [made], undoAutomation });
  const created: ToolStep = {
    ...helperStep(null),
    name: 'create_automation',
    toolCallId: 'call_a',
    status: 'success',
    helper: null,
  };

  expect(result.current.automationNotice?.(created)).toBe(made);
  await act(async () => {
    expect(await result.current.onUndoAutomation?.(made)).toBe(true);
  });
  expect(undoAutomation).toHaveBeenCalledWith(made);
  expect(actionsFor({}).result.current.onUndoAutomation).toBeUndefined();
});
```

Add to `apps/web/src/components/sessions/SessionView.test.tsx` (import `automationFixture` from `../../test/automations`):

```tsx
it('names a check-in’s schedule and opens its automation', async () => {
  const user = userEvent.setup();
  const automation = automationFixture('daily');
  const props = renderView({
    session: sessionFixture('schedule:daily', {
      kind: 'checkin',
      origin: 'schedule',
      title: 'Check-in · goals',
    }),
    automation,
    onEditAutomation: vi.fn(),
  });

  expect(screen.getByText('Check-in · every 30 min')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Edit automation' }));
  expect(props.onEditAutomation).toHaveBeenCalledWith(automation);
});
```

In `apps/web/src/components/WorkspaceShell.test.tsx`, change `shows the conversation for pages that arrive in later releases` to use `{ kind: 'page', page: 'memory' }`, and add after `names the destination plainly when nothing waits`:

```tsx
it('opens Automations from the navigation', async () => {
  const user = userEvent.setup();
  render(<Shell automations={<div>Automations page</div>} />);
  const nav = screen.getByRole('navigation', {
    name: 'Workspace navigation',
  });

  await user.click(within(nav).getByRole('button', { name: 'Automations' }));

  expect(screen.getByText('Automations page')).toBeVisible();
  expect(screen.getByText('Workspace canvas')).not.toBeVisible();
  expect(
    within(nav).getByRole('button', { name: 'Automations' }),
  ).toHaveAttribute('aria-current', 'page');
});
```

If another assertion in that file lists the destinations in order, add `Automations` after `Approvals`.

In `apps/web/src/ViewHarness.test.tsx`:

1. In the top-level `beforeEach`, after `vi.spyOn(daemon, 'listSkills').mockResolvedValue([]);`, add `vi.spyOn(daemon, 'listAutomations').mockResolvedValue([]);` (keeps every other harness test quiet).
2. In `opens the new session on a page that still shows the conversation`, change the comment to `// Memory arrives in a later release; until then it shows the chat.` and the URL to `'/#/memory'`.
3. Add after that test (import `automationFixture` from `./test/automations`):

```tsx
it('shows the companion’s automations at #/automations', async () => {
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  vi.mocked(daemon.listAutomations).mockResolvedValue([
    automationFixture('schedule-1', { name: 'Morning brief' }),
  ]);
  window.history.replaceState(null, '', '/#/automations');
  render(<ViewHarness />);

  expect(
    await screen.findByRole('article', { name: 'Morning brief' }),
  ).toBeVisible();
  expect(daemon.listAutomations).toHaveBeenCalledWith('agent-main', {
    signal: expect.any(AbortSignal),
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/web && bun x vitest run src/components/sessions/RunActivity.test.tsx src/hooks/useTranscriptActions.test.tsx src/components/sessions/SessionView.test.tsx src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx 2>&1 | tail -30`
Expected: FAIL (no notice card, no `automationNotice`, no header badge detail, no Automations destination, and the harness does not wire the page).

- [ ] **Step 3: The notice card and the transcript actions**

In `apps/web/src/lib/transcript.ts`, add `import type { Automation } from '@animaOS-SWARM/sdk';` (merge with an existing SDK type import if there is one) and to `TranscriptActions`, after `companionAgentId?: string;`:

```text
  /** The automation a `create_automation` call made, for its notice card
   *  (spec §15.2). */
  automationNotice?: (step: ToolStep) => Automation | null;
  /** Undo (delete) an automation the companion made; true when it went. */
  onUndoAutomation?: (automation: Automation) => Promise<boolean>;
```

Create `apps/web/src/components/sessions/AutomationNoticeCard.tsx`:

```tsx
import { useState } from 'react';
import type { Automation } from '@animaOS-SWARM/sdk';

import { formatWhen } from '../../lib/approvals';
import { describeActiveHours, describeTrigger } from '../../lib/automations';

/** An automation the companion created (spec §9.3, §15.2), with Undo. Its
 *  name is the companion's text: shown as text. */
export function AutomationNoticeCard({
  automation,
  onUndo,
}: {
  automation: Automation;
  onUndo?: (automation: Automation) => Promise<boolean>;
}) {
  const [undoing, setUndoing] = useState(false);
  const undo = async () => {
    if (!onUndo) return;
    setUndoing(true);
    // On success the automation leaves the list, and this card with it.
    if (!(await onUndo(automation))) setUndoing(false);
  };
  return (
    <div
      className="automation-notice"
      role="note"
      aria-label={`Automation ${automation.name}`}
    >
      <p className="automation-notice-title">
        <span className="automation-badge">New automation</span>{' '}
        {automation.name}
      </p>
      <p className="automation-notice-detail">
        {describeTrigger(automation.trigger)}
        {automation.activeHours &&
          ` · ${describeActiveHours(automation.activeHours)}`}
        {automation.enabled
          ? ` · next ${formatWhen(automation.nextDueAtMs)}`
          : ' · paused'}
      </p>
      {onUndo && (
        <button
          type="button"
          className="studio-tool-button"
          disabled={undoing}
          onClick={() => void undo()}
        >
          {undoing ? 'Undoing…' : 'Undo'}
        </button>
      )}
    </div>
  );
}
```

In `apps/web/src/components/sessions/RunActivity.tsx`, add `import { AutomationNoticeCard } from './AutomationNoticeCard';` and `import type { Automation } from '@animaOS-SWARM/sdk';`, and in `ToolBlock`, after `const keys = stepKeys(steps);`:

```tsx
// Spec §15.2: an automation the companion created stays visible with
// Undo, even when the block is collapsed.
const notices: Automation[] = [];
for (const candidate of steps) {
  const automation = actions?.automationNotice?.(candidate) ?? null;
  if (automation && !notices.some((known) => known.id === automation.id))
    notices.push(automation);
}
```

and as the last child of the `tool-block` `div` (after the `expanded && count > 0` list):

```tsx
{
  notices.map((automation) => (
    <AutomationNoticeCard
      key={automation.id}
      automation={automation}
      onUndo={actions?.onUndoAutomation}
    />
  ));
}
```

In `apps/web/src/hooks/useTranscriptActions.ts`:

1. Add `import type { Automation } from '@animaOS-SWARM/sdk';` (merge with the existing SDK type import) and `import { automationNoticeFor } from '../lib/automations';`.
2. Add to `TranscriptActionOptions`, after `companionId?`:

```text
  /** The companion's automations, where a notice card finds its own. */
  automations?: readonly Automation[];
  /** Deletes an automation (the notice card's Undo). */
  undoAutomation?: (automation: Automation) => Promise<boolean>;
```

3. Destructure `automations = NO_AUTOMATIONS` and `undoAutomation` (add `const NO_AUTOMATIONS: readonly Automation[] = [];` at module level), add `undoAutomation` to `latest`, and inside the memo's object, after `onDecideApproval`:

```text
      automationNotice: (step: ToolStep) => automationNoticeFor(step, automations),
      ...(undoAutomation
        ? {
            onUndoAutomation: (automation: Automation) =>
              latestRef.current.undoAutomation?.(automation) ??
              Promise.resolve(false),
          }
        : {}),
```

and add `automations` and `undoAutomation !== undefined` (as a boolean `canUndo` computed above the memo) to its dependency list.

Append to `apps/web/src/automations.css`:

```css
.automation-notice {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  margin-top: 8px;
  border: 1px solid var(--color-line);
  border-radius: 12px;
  padding: 8px 10px;
}
.automation-notice-title {
  color: var(--color-ink);
  font-size: 13px;
  overflow-wrap: anywhere;
}
.automation-notice-detail {
  color: var(--color-ink-3);
  font-size: 12px;
}
```

- [ ] **Step 4: The check-in header and the destination**

In `apps/web/src/components/sessions/SessionView.tsx`:

1. Add `type Automation` to the SDK import and `import { describeTrigger } from '../../lib/automations';`.
2. Add to `SessionViewProps`, after `onExport`:

```text
  /** A check-in's automation, for its header (spec §15.2). */
  automation?: Automation | null;
  /** Opens the automation's editor on the Automations page. */
  onEditAutomation?: (automation: Automation) => void;
```

3. Pass `automation` and `onEditAutomation` from `SessionView` (destructure with defaults `automation = null`) to `SessionHeader`, add both to its props type, render the badge as:

```tsx
<span className="session-kind-badge">
  {SESSION_KIND_LABELS[session.kind]}
  {automation ? ` · ${describeTrigger(automation.trigger)}` : ''}
</span>
```

and, after the Export button:

```tsx
{
  automation && onEditAutomation && (
    <button
      type="button"
      className={ghostBtnCls}
      onClick={() => onEditAutomation(automation)}
    >
      Edit automation
    </button>
  );
}
```

In `apps/web/src/components/icons.tsx`, after `PulseIcon`:

```tsx
export const ClockIcon = (p: IconProps) =>
  base(
    p,
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </>,
  );
```

In `apps/web/src/components/WorkspaceShell.tsx`:

1. Add `'automations',` after `'approvals',` in `AVAILABLE_PAGES`, import `ClockIcon`, and add `{ page: 'automations', label: 'Automations', icon: <ClockIcon size={16} /> },` after the Approvals entry of `PRIMARY_DESTINATIONS`.
2. Add the prop (destructured `automations = null`, typed `/** The Automations page, shown at \`#/automations\`. \*/ automations?: ReactNode | null;`) after `approvals`, and in the page switch, after the `approvals` branch:

```text
              ) : page === 'automations' ? (
                automations
```

- [ ] **Step 5: Wire the harness**

In `apps/web/src/ViewHarness.tsx` (wiring lines only):

1. Imports: `import { useAutomations } from './hooks/useAutomations';`, `import { AutomationsPage } from './pages/AutomationsPage';`, `import { checkinScheduleId } from './lib/automations';`.
2. After the `useSkillCommands` call:

```tsx
const automations = useAutomations({
  agentId,
  version: live.state.automationsVersion,
  epoch: live.state.epoch,
  enabled: connection === 'online',
});
const [automationFocus, setAutomationFocus] = useState<string | null>(null);
const clearAutomationFocus = useCallback(() => setAutomationFocus(null), []);
```

(`useCallback` and `useState` are already imported; add them to the React import if not.) 3. In the `useTranscriptActions({ … })` options, after `companionId: agentId,`:

```text
    automations: automations.automations,
    undoAutomation: automations.remove,
```

4. Before `const sessionView = (`:

```tsx
const checkinId =
  viewedSession?.kind === 'checkin'
    ? checkinScheduleId(viewedSession.id)
    : null;
const checkinAutomation = checkinId
  ? (automations.automations.find((item) => item.id === checkinId) ?? null)
  : null;
```

and to `<SessionView … />`, after `actions={transcriptActions}`:

```text
      automation={checkinAutomation}
      onEditAutomation={(automation) => {
        setAutomationFocus(automation.id);
        navigate({ kind: 'page', page: 'automations' });
      }}
```

5. To `<WorkspaceShell … />`, after the `approvals={…}` prop:

```tsx
          automations={
            <AutomationsPage
              view={automations}
              agentId={agent.id}
              version={live.state.automationsVersion}
              online={connection === 'online'}
              telegramConnectorId={
                telegramConnector?.approvedChat ? telegramConnector.id : null
              }
              focusId={automationFocus}
              onFocusHandled={clearAutomationFocus}
              onOpenSession={(sessionId) =>
                commands.openTarget({ agentId: agent.id, sessionId })
              }
            />
          }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd apps/web && bun x vitest run src/components/sessions/RunActivity.test.tsx src/hooks/useTranscriptActions.test.tsx src/components/sessions/SessionView.test.tsx src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx src/visual-tokens.test.ts 2>&1 | tail -30`
Expected: PASS, with no `act()` warnings or console noise.

Run: `cd apps/web && bun x vitest run 2>&1 | tail -15`
Expected: the whole web suite passes (M5 ended at 807 tests; M6 adds about 40).

Run: `bun x nx run @animaOS-SWARM/web:typecheck 2>&1 | tail -15`
Expected: success.

- [ ] **Step 7: Format and commit**

```bash
bun x nx format:write --files=apps/web/src/components/sessions/AutomationNoticeCard.tsx,apps/web/src/lib/transcript.ts,apps/web/src/components/sessions/RunActivity.tsx,apps/web/src/components/sessions/RunActivity.test.tsx,apps/web/src/hooks/useTranscriptActions.ts,apps/web/src/hooks/useTranscriptActions.test.tsx,apps/web/src/components/sessions/SessionView.tsx,apps/web/src/components/sessions/SessionView.test.tsx,apps/web/src/components/icons.tsx,apps/web/src/components/WorkspaceShell.tsx,apps/web/src/components/WorkspaceShell.test.tsx,apps/web/src/ViewHarness.tsx,apps/web/src/ViewHarness.test.tsx,apps/web/src/automations.css
git diff --stat
git add apps/web/src/components/sessions/AutomationNoticeCard.tsx apps/web/src/lib/transcript.ts apps/web/src/components/sessions/RunActivity.tsx apps/web/src/components/sessions/RunActivity.test.tsx apps/web/src/hooks/useTranscriptActions.ts apps/web/src/hooks/useTranscriptActions.test.tsx apps/web/src/components/sessions/SessionView.tsx apps/web/src/components/sessions/SessionView.test.tsx apps/web/src/components/icons.tsx apps/web/src/components/WorkspaceShell.tsx apps/web/src/components/WorkspaceShell.test.tsx apps/web/src/ViewHarness.tsx apps/web/src/ViewHarness.test.tsx apps/web/src/automations.css
git commit -m "feat(web): show automation notice cards with undo and add the Automations destination"
```

Recommended implementer tier: most capable (harness wiring across the transcript, the header, and the shell, with pristine web tests).

---

### Task 13: M6 verification

**Files:**

- Modify: `docs/superpowers/plans/2026-09-23-companion-console.md` (the M6 status row; controller only)

- [ ] **Step 1: Check the contracts**

Run: `grep -n "const MAX_AUTOMATIONS_PER_AGENT\|const MIN_AGENT_AUTOMATION_GAP_MS\|const AGENT_GAP_CHECKED_FIRES\|const MAX_AUTOMATION_HISTORY_SHOWN\|const MAX_AUTOMATION_NAME_CHARS\|const PREVIEW_FIRES\|const HEARTBEAT_INTERVAL_MS" hosts/rust-daemon/src/schedules/automations.rs; grep -rn "const MAX_CRON_EXPRESSION_CHARS\|const CRON_SEARCH_DAYS\|const MAX_UNMIRRORED_FIRES\|const MAX_WINDOW_HOPS\|const DST_GAP_SEARCH_MINUTES\|const HISTORY_FIRE_BATCH" hosts/rust-daemon/src`
Expected: each constant defined once (the first seven in `schedules/automations.rs`; the rest in `cron.rs`, `history.rs`, `timing.rs`, and `history/outbox.rs`).

Run: `grep -rn '"/api/agents/{agent_id}/schedules/{schedule_id}/run"\|"/api/agents/{agent_id}/schedules/{schedule_id}/history"\|"/api/schedules/preview"' hosts/rust-daemon/src/routes`
Expected: each path in `routes/mod.rs` (the router) and in its handler's `#[utoipa::path]` in `routes/schedules.rs`.

Run: `grep -n "### Automations\|Rolling back from M6" hosts/rust-daemon/README.md`
Expected: the Automations section and the M6 rollback note.

Run: `grep -rn "allow(dead_code)\|allow(unused_imports)" hosts/rust-daemon/src/schedules hosts/rust-daemon/src/state/automation_state.rs hosts/rust-daemon/src/tools/automations.rs`
Expected: no output (Tasks 2, 5, and 6 removed the temporary allowances).

Run: `grep -n "validate_stored_trigger" hosts/rust-daemon/src/state.rs; grep -n "CONTROL_PLANE_STORE_VERSION: u32 = 9" hosts/rust-daemon/src/control_plane_store.rs; grep -n '"m6-automations"' hosts/rust-daemon/src/sessions/migration.rs`
Expected: one match each.

Run: `grep -rn "dangerouslySetInnerHTML\|MarkdownMessage\|innerHTML" apps/web/src/pages/AutomationsPage.tsx apps/web/src/components/automations apps/web/src/components/sessions/AutomationNoticeCard.tsx`
Expected: no output (names, prompts, and history render as text).

Run: `grep -n "croner\|^cron" hosts/rust-daemon/Cargo.toml; git diff b93934d --stat -- Cargo.lock hosts/rust-daemon/Cargo.toml packages/sdk/package.json apps/web/package.json bun.lock`
Expected: no output from either (no new dependencies or features).

- [ ] **Step 2: Run the milestone gate**

Run: `df -h .`

- With at least 12 GB available: run `bun x nx run rust-daemon:test --skipNxCache` (it also runs `core-rust:test`). Expected: PASS (M5 ended at 1,775 passed / 7 ignored; M6 adds about 90 tests).
- Otherwise run the fallback in the shared `target/` (no new `CARGO_TARGET_DIR`): `CARGO_INCREMENTAL=0 cargo test -p anima-core --lib`, `CARGO_INCREMENTAL=0 cargo test -p anima-model-adapters --lib`, `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib`, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --tests`. Expected: PASS. The fallback does not satisfy AGENTS.md's completion rule; record that the Nx gate is pending disk space. On Windows, if a running daemon locks `target/debug/anima-daemon.exe`, use AGENTS.md's `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon` rerun only with the owner's go-ahead (it is a second target directory on a tight disk).

Run: `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`
Expected: every target succeeds (M5 ended at web 807 tests; M6 adds about 45 web and 4 SDK tests).

Run: `cargo fmt --all --check && bun x nx format:check --base=origin/main`
Expected: both succeed.

- [ ] **Step 3: Update the master plan status**

In `docs/superpowers/plans/2026-09-23-companion-console.md`, replace the M6 row (match it by content; the table is padded)

```markdown
| M6 Automations | (written before M6) | pending |
```

with the following only if every gate command passed (fill in the Nx test count and the head commit):

```markdown
| M6 Automations | `2026-09-23-companion-console-m6.md` | done (Nx rust-daemon:test <count> passed; sdk + web test, typecheck, build green at <sha>) |
```

If the Rust gate ran only through the fallback, use `implemented — Nx gate pending (disk)` as the status. Also change the master plan's Global Constraints sentence `Only new third-party dependency allowed: \`croner\` (cron parsing) in \`hosts/rust-daemon\`.`to`No new third-party dependency: M6 implements 5-field cron in-house on the existing \`chrono\` and \`chrono-tz\`.`and drop`croner`from its Tech Stack line. Then run`bun x nx format:write --files=docs/superpowers/plans/2026-09-23-companion-console.md` (it realigns the table). The controller commits this file:

```bash
git add docs/superpowers/plans/2026-09-23-companion-console.md
git commit -m "docs: mark the M6 automations milestone complete"
```

Recommended implementer tier: the controller runs this task.

---

## Notes for the controller

**Task shape against the master plan.** Every master task is covered; T6.1 and T6.2 are split so each commit stays reviewable:

- T6.1 (`cron` and `once` with active hours, restore validation, preview route) → Tasks 1 (cron), 2 (timing, the two variants, explicit validation), 3 (fields, fire log, `automation.updated`, version 9), 5 (the service the owner's routes and the tools share), and 7 (routes, contracts, preview). T6.2 (run now, history, counters, heartbeat) → Tasks 3 (counters and the fire log), 4 (the history store, the outbox, silent check-in retention), 5 (the heartbeat preset), 6 (counters and fires at every outcome, `once`, active hours at claim, Run now), and 7 (the history and run routes). T6.3 (tools and limits) → Tasks 5 (limits, in the service) and 8 (tools, helpers, grant). T6.4 (SDK, page, parser, notice cards) → Tasks 9 (SDK), 10 (data layer and parser), 11 (page), and 12 (notice cards, header, destination, wiring). Task 13 is the gate.
- Master names kept: `schedules.rs`, `routes/schedules.rs`, contracts (`routes/contracts/schedules.rs`), `tools/automations.rs`, `packages/sdk/src/automations.ts`, `pages/AutomationsPage.tsx`, `lib/schedule-parse.ts`.
- Files the master plan did not list: `schedules/{cron.rs,timing.rs,automations.rs,history.rs,run_now_tests.rs}`, `state/automation_state.rs`, `agent_runs/{automations.rs,automation_tests.rs}`, `routes/tests/automations.rs`; web `lib/automations.ts`, `hooks/useAutomations.ts` (spec §15.5 names it), `components/automations/{AutomationEditor.tsx,AutomationHistory.tsx}`, `components/sessions/AutomationNoticeCard.tsx`, `test/automations.ts`, `automations.css`. `ChatScreen.tsx` needs no change: its `ToolBlock` already receives the transcript actions.
- Order: strictly 1 → 13. Task 5 must follow Task 4 only for the file-map order; its code needs Tasks 2–3. Task 6 needs Task 5's service (the `once` test creates through it). Task 7's history route needs Task 4's store methods. Tasks 9–12 need the daemon's JSON only through the SDK types, so they could start after Task 7, but stay sequential for the SDK build.
- Sizes (plan lines): Task 1 ≈ 560, 2 ≈ 900, 3 ≈ 1,330, 4 ≈ 680, 5 ≈ 1,160, 6 ≈ 880, 7 ≈ 1,160, 8 ≈ 980, 9 ≈ 550, 10 ≈ 1,100, 11 ≈ 1,170, 12 ≈ 470. None is over 1,500, so none is split.

**Carry-forwards.** The master plan's "Carried from M3" list names, under M10, "Retention of session records and silent check-in pairs (also M6)"; M3's plan suggested M6 own the check-in pair pruning. Task 4 does that part (see the decision below); session-record retention stays with M10. Nothing else in the list or in the M4/M5 ledgers' open items is for M6 (the "skip a scan while the previous one runs" follow-up is skills-only). M4/M5 conventions applied: drop-safe changes in their own task (`AutomationService::locked`, Run now's spawned claim); save-then-announce; revert on a failed save with a 503 route test; named strings tested once; `text` fences for fragments; the SDK build at the end of Task 9; no new `CARGO_TARGET_DIR`; hidden-Unicode refusal for model-written text; an empty reload bails out in `useAutomations` (the M5 I4 lesson); deterministic tests (every timing function takes `now`; the scheduler runs through `tick_at`; gated models with `within` bounds).

**Spec vs. code decisions.**

- **No `croner`.** The master plan allows `croner`, but it is not a dependency of the daemon, and M6 adds none, so `schedules/cron.rs` implements Vixie-style 5-field cron in-house (about 300 lines with tests): numbers, three-letter names, `*`, ranges, steps (`a/s` runs to the field's end), lists, `7` as Sunday, the `@` macros except `@reboot`; `L`, `W`, `#`, and `?` are refused with a message. Day of month and day of week combine with OR only when both fields are restricted (a field starting with `*` counts as unrestricted, as in Vixie cron). It lives in the daemon, not in `anima-schedule`, because it uses `chrono-tz`, which that crate does not depend on.
- **Time zones and daylight saving.** Every cron trigger stores an IANA `timeZone` (the browser's, from the page; `UTC` when the companion's tool omits it), evaluated on that zone's wall clock. A wall time skipped by a spring-forward jump does not fire that day (the same rule the existing `daily` trigger already follows); a repeated fall-back time fires once, at its first occurrence after the previous fire. Active-hours windows opening inside a gap open at the first wall minute after it. The next fire is searched over at most 28 years of days; an expression with no fire in that span is refused at creation (`This schedule never runs`).
- **Active hours.** Days are numbered like JavaScript's `getDay()`; an overnight window belongs to the day it starts; an interval outside the window waits for the next opening and keeps counting from there; a cron or daily fire outside the window is skipped (bounded by `MAX_WINDOW_HOPS` openings, after which the schedule is refused as never running inside its window). A `once` trigger refuses active hours. The heartbeat's window is every day.
- **Agent minimum.** "At least 5 minutes apart, checked over their next 10 fire times" is checked on the next 10 fires (9 gaps) including active hours, at creation by the companion only; the owner's automations have no minimum.
- **Limit of 20.** Applied to the owner's and the companion's creates alike (spec §16 names it per agent); a legacy browser import is exempt so `importLegacyCheckins` keeps working for owners with older browser check-ins. Restore does not enforce it.
- **Snapshot version 9.** `.pre-automations.bak` / `control_plane.backup.8`, as M2–M5 did; an M5 daemon would drop the new fields and fail on a `cron` or `once` trigger. Task 3 updates the eleven version-8 assertions and the stale "version-8"/"v8" prose.
- **Fire records in the control plane until mirrored**, like decided approvals, keyed by the occurrence's run idempotency key so the outbox's writes are idempotent; capped at 1,000 while the store fails (the oldest leave with a warning, which can lose history rows but never an outcome). History rows stay when an automation is deleted (they go with the agent); `delete_session` leaves them.
- **History route** merges the control plane's unmirrored fires with the store's and answers 503 when the store cannot be read, rather than a partial list.
- **Counters.** A failure adds to both failure counts; a silent or spoken reply ends a streak; a stop changes neither. Restart reconciliation's `schedule_run_interrupted` counts as a failure. Nothing auto-pauses on failures (deferred).
- **Run now** on a paused or finished one-time automation is allowed (the owner asked for it explicitly); it never moves the due time or the switch. "Global caps" are the scheduler's eight live jobs (429) and the coordinator's run permits; an occurrence without an outcome (running or interrupted) answers 409. A restart during a manual run disables the automation like any interrupted occurrence, so nothing is replayed.
- **`automation.updated`** carries only `scheduleId` and `deleted`; clients read the list again. It is published for claims and outcomes too, so the page's "Running" and counters stay current.
- **`createdBy` names the tool call** (`toolCallId`, beyond the spec's session and run), so the web can put the notice card beside the right tool step, in live runs and in history, without parsing the tool's reply.
- **Heartbeat preset** is expanded by the daemon from the request's `timeZone`, so the browser never holds the preset's prompt or window; the `preset` label stays through edits.
- **The list route now needs owner authorization** (spec §14 covers new routes; this one changed shape and now carries who created each automation). The integration test's two unauthenticated list reads gain owner headers; the web already sends them.
- **Silent check-in retention.** Spec §13.2 says silent check-in turns among the newest 200 visible messages stay. Taken literally, a heartbeat's session (about 28 silent pairs a day, few visible messages) would keep every pair in the control plane forever, against spec §1's bounded snapshot growth. Task 4 prunes a silent turn once all of it is mirrored, older than 24 hours, and unreferenced, wherever it sits; the history store keeps it (and `includeHidden=true` still shows it). The existing test that pinned the literal rule is updated.
- **Hidden text.** Prompts refuse Unicode tag and direction-override characters and names refuse every invisible format character, for the owner's writes too (the M5 rules, shared from `skills/mod.rs`); restored records are not re-checked.
- **Tools.** `schedule` is one string in three grammars (cron, `every …`, `at <RFC 3339>`), simpler for a model than a nested object; `target: telegram` picks the companion's active connector with an approved chat. Helpers get none of the three tools (filtered and refused), since an automation of a helper would run the helper unsupervised; `list_automations` is filtered too for simplicity.
- **Web.** The phrase parser computes `once` instants in the browser (it is turning words into a trigger, not computing fire times; the next runs always come from the daemon's preview). The Automations page receives the harness's `useAutomations` view instead of reading on its own, so the notice cards and the check-in header share one list. "Edit automation" opens the page with that automation's editor through a focus id rather than a new hash route.

**Deferred.** Auto-pausing after repeated failures; a Health card for failing automations (M8); usage records for automation runs (M8); the Playwright automations flow (M10, T10.2); editing automations from Telegram; `resume_automation`/`delete_automation` tools; deleting an automation's history rows with it; a capability-inventory category for the automation tools (they show as `utility`); session-record retention (M10); AgentWork's older schedule form (it keeps creating interval and daily schedules through the existing `AgentsClient`; the Automations page is the full editor).

**Risks for the pre-flight audit.**

- Cron and window correctness (Tasks 1–2): the day-of-month/day-of-week rule, the `a/s` meaning, daylight-saving edges in both cron and window openings, the 28-year search bound's cost on a 400-hop window loop (worst case about 400 × 10,000 day checks for a never-matching window; check it stays well under the tick), and that `next_fire_after_claim` never returns a time at or before `now`.
- Commit/rollback (Tasks 3, 6): `record_automation_outcome` runs inside the coordinator's commit hook under its transaction and state lock; the rollback must undo exactly it; restart reconciliation's undo after a failed save restores both the record and the fire log; the commit hook now checks the automation exists before building the Telegram outbound, so an `Err` leaves nothing behind.
- Lock order (Task 6): Run now takes the scheduler's `jobs` mutex before the transaction, as `tick_inner` does; check nothing takes `jobs` under the transaction, and that `run_now`'s spawned task cannot deadlock with a tick.
- The companion's reach (Tasks 5, 8): `create_automation` is a write-class tool allowed by default, so a prompt-injected run can schedule future runs with an arbitrary prompt; the mitigations are the notice card with Undo, the page listing `createdBy`, the 20 cap, the 5-minute minimum, hidden-text refusal, and the README advice to set write to `ask`. Consider whether to recommend `ask` for `create_automation` by default (a policy change M4 did not make).
- The deliberate deviations: silent check-in pruning against spec §13.2's letter (Task 4) and the list route's new authorization (Task 7); check no other test or client relies on the old behavior (the CLI and TUI do not call the schedule routes).
- History (Tasks 4, 7): the outbox drops a deleted agent's fires in `unmirrored_schedule_fires` before writing; the memory store's 100,000-row cap applies to fires too; the Postgres methods are untested here (`#[ignore]`).
- Web (Tasks 10–12): the editor's preview is asynchronous (the page tests default it to a promise that never settles and await it where they read it); the harness's top-level `listAutomations` mock and the empty-reload bail-out keep the existing suite quiet; two harness tests change (`#/automations` no longer shows the chat).

## Controller rulings (binding; no separate pre-flight audit, to save cost)

The controller ruled on the plan writer's risk list directly. The per-task reviews check the code.

1. **Pruning silent check-ins (spec §13.2 deviation):** accepted. Silent check-in pairs are pruned from the hot tail once mirrored and older than 24 hours. Without this, heartbeats would grow the snapshot forever.
2. **`create_automation` default:** it stays in the write class and is allowed by default. The mitigations are the notice card with Undo, the creator shown on the page, the 20-per-agent limit, the 5-minute minimum, hidden-text refusal, and README advice to set `write: ask`. No per-tool default is added to the M4 policy model.
3. **Outcome commit and rollback:** the Task 6 implementer and reviewer must show, with tests, that a failed save restores exactly the previous counters and fire records, including during restart reconciliation.
4. **Lock order for Run now:** taking the scheduler's `jobs` mutex before the control-plane transaction is accepted only if nothing ever takes `jobs` while holding the transaction. Task 6 adds a comment saying so, and its reviewer greps for it.
5. **Cron cost:**
   - Task 1 bounds the next-fire search, so a pathological expression or window can't stall the scheduler.
   - Task 2 bounds the work for an active-hours window that never matches.
   - Any expression that can't fire within the bound is refused when it's created.
   - Both bounds are tested.
6. **Behavior changes for existing clients:** accepted. Owner auth on the schedule list, history rows deleted with their agent, and counter updates in reconciliation tests.
7. **Web tests:** the preview stub that never settles is accepted. The two harness tests change for `#/automations`.
8. **Postgres:** the store methods stay `#[ignore]`; hand-check the SQL against the migration.
9. **Lessons from M4 and M5 apply to every task:**
   - Owner mutations run their transaction/save/undo body in `tokio::spawn`.
   - No test depends on wall-clock races.
   - Model-written text (automation names and prompts from the companion's tool) is refused for hidden Unicode with the M5 rule.
   - Every owner-facing string is a named constant, tested once.
