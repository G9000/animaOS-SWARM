# M5 pre-flight audit — `docs/superpowers/plans/2026-09-23-companion-console-m5.md`

Audited at HEAD `231655a` (feat/companion-console, M4 merged at `f34313b`). Read-only; no build was run (disk). "Plan:N" is a line of the plan; code refs are repo paths at HEAD.

**Result: 0 Blocker · 4 Important · 19 Minor.**

- Every anchor, name, signature, test name, and command the plan edits exists at HEAD, and the Interfaces blocks agree with each other.
- Read line by line, the Rust and TypeScript compile: borrow order in the struct literals, the `FnOnce` captures, `Send` futures under `tokio::spawn`, and edition-2021 disjoint captures all check out.
- No new dependency or dependency feature is needed. `serde_yaml 0.9.34`, `sha2 0.10.9`, `uuid` v4, `async-trait`, tokio `rt-multi-thread`/`sync`/`time`, and rustc 1.93 (for `Option::is_none_or`) are all present.
- The Important findings:
  - invisible characters defeat owner review;
  - the web editor can approve an edited-on-disk body without a review;
  - Windows reserved device names are valid slugs;
  - the web tests' `act()` hygiene is left to after-the-fact fixes.

---

## 1. Interface table (task pairs sharing a file or an interface)

| Producer → consumer | Shared file / interface                                                                                                                                                                                                             | Produced                                        | Consumed                                                                                                                       | Result                                                                                                     |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------- |
| T1 → T2–T9          | `skills/mod.rs` constants, strings, `SkillRecord`, `SkillDraft::{new,decide,is_pending}`, `ProposedBy`, `DraftSource`/`DraftStatus`/`SkillStatus` + `as_str`; `file::{parse_skill_file, compose_skill_file, skill_hash, SkillFile}` | Plan:111-469, 699-798                           | registry Plan:1180-1184, disk Plan:2135-2141, service Plan:2933-2941, drafts Plan:3949-3958                                    | ✓                                                                                                          |
| T2 → T3             | `ScannedFile::{read, unreadable, problem}`                                                                                                                                                                                          | Plan:1186-1229                                  | disk Plan:2135, 2213-2233                                                                                                      | ✓                                                                                                          |
| T2 → T4             | `SkillRegistry::{get, records, find, index, runnable, generation, put, restore, remove, set_scanned, scanned, scanned_files, apply_scan, file_drafts}`; `DaemonState.skills`                                                        | Plan:1306-1430, 1741-1752                       | service Plan:3094-3455                                                                                                         | ✓                                                                                                          |
| T2 → T5             | `put_draft`, `remove_draft`, `draft`, `pending_from`, `pending_imports`, `pending_drafts`, `decided_drafts`, `prune_decided`, `DraftView`                                                                                           | Plan:1461-1545, 1243-1251                       | drafts Plan:4005-4319                                                                                                          | ✓                                                                                                          |
| T2 → T6/T7          | `DraftView`, `ScannedFile` re-exports                                                                                                                                                                                               | Plan:849-851                                    | contracts Plan:4652, 5583                                                                                                      | ✓                                                                                                          |
| T3 → T4/T5          | `disk::{read_skill_bytes, scan_skill, scan_skills_folder, write_skill_file, trash_skill_folder, untrash_skill_folder}` (blocking)                                                                                                   | Plan:2164-2336                                  | Plan:3098-3099, 3141, 3158, 3196, 3299, 3323, 3356, 3416, 4190, 4285                                                           | ✓ signatures match every call site                                                                         |
| T4 → T5             | `SkillService::{locked, apply, record, scan, workspace, state}`, `blocking`, `Change<T>` (all `pub(super)` within `skills`)                                                                                                         | Plan:3006-3145                                  | Plan:3949-3951, 4030-4318                                                                                                      | ✓                                                                                                          |
| T4 → T6             | `list`, `detail` (`SkillDetail { record, file }`), `save` (`SkillContent`), `set_enabled`, `delete`, `approve_changed`; `AgentRunCoordinator::skills()`                                                                             | Plan:3171-3392, 3558-3561                       | routes Plan:4832-5019                                                                                                          | ✓                                                                                                          |
| T4 → T8/T9          | `load` → `LoadedSkill { slug, name, body }`, `index`, `check_runnable`                                                                                                                                                              | Plan:3397-3455                                  | tools Plan:6298, run path Plan:6866-6883, route Plan:7004                                                                      | ✓ (`LoadedSkill.slug` is never read outside tests: m15)                                                    |
| T4 → T6–T9 (events) | `LiveEventBody::SkillUpdated { slug, draft_id }`, `DaemonState::publish_skill_updated`                                                                                                                                              | Plan:2880-2904, 2788-2808                       | `apply` Plan:3121-3124, `scan` Plan:3164, `delete` Plan:3333-3337                                                              | ✓ (exhaustive `to_json` match, events.rs:143-216, gains its arm)                                           |
| T5 → T7             | `drafts(decided)`, `approve_draft(id, DraftApproval)`, `reject_draft`, `import(bytes, slug)`, `ApprovedDraft { skill, draft }`                                                                                                      | Plan:3961-3983, 4026-4319                       | Plan:5737, 5784-5801, 5821, 5868-5872                                                                                          | ✓                                                                                                          |
| T6 → T7             | `routes/skills.rs` (`skill_error`, `answer`, `MAX_SKILL_REQUEST_BYTES`), `contracts/skills.rs`, `routes/tests/skills.rs` helpers (`daemon`, `send`, `json_body`, `notes`, `request`), README Skills table                           | Plan:4747-4793, 4646-4737, 4368-4415, 5061-5075 | Plan:5683-5883, 5583-5679, 5348-5576, 5911-5916                                                                                | ✓                                                                                                          |
| T4/T5 → T8          | `SkillService::{load, propose}`, `Proposal { by, name, description, body, slug }`, `proposed_reply`                                                                                                                                 | Plan:3962-3968, 236-241                         | tools/skills.rs Plan:6298-6346                                                                                                 | ✓ (`ToolExecutionContext.{team, run_link}` are `pub(super)` in tools.rs:60,67, visible to `tools::skills`) |
| T8 → T9             | `agent_runs/skill_tests.rs` helpers `call`, `skilled`                                                                                                                                                                               | Plan:5986-6017                                  | Plan:6533-6646                                                                                                                 | ✓                                                                                                          |
| T4 → T9             | `agent_runs/skills.rs` replaced, keeping `skills()` and adding `apply_skills`                                                                                                                                                       | Plan:3546-3561                                  | Plan:6843-6885                                                                                                                 | ✓                                                                                                          |
| T6/T7/T4 → T10      | JSON: `{skills}`, `{skill, file}`, `{skill}`, `{deleted, trashPath}`, `{drafts}`, `{draft}`, `{skill, draft}`, `skill.updated {slug, draftId}`                                                                                      | Plan:4654-4736, 5586-5678, 2898-2903            | SDK Plan:7272-7350, 7484-7489                                                                                                  | ✓ field for field (15 draft fields, 8 skill fields; nulls are not skipped)                                 |
| T9 → T10/T13        | run route `skill`                                                                                                                                                                                                                   | Plan:6994-7018                                  | SDK `StartRunInput.skill?` already exists (packages/sdk/src/runs.ts:61); `runs.start` passes the body through (runs.ts:87-101) | ✓ no SDK change needed                                                                                     |
| T10 → T11           | `SkillsClient`, types, limits; `sdk:build` last                                                                                                                                                                                     | Plan:7265-7528, 7535                            | daemon-api Plan:7871-7895, lib/skills Plan:7951-7958                                                                           | ✓                                                                                                          |
| T11 → T12           | `useSkills`, `lineDiff`, `STATUS_LABELS`/`SOURCE_LABELS`, `skillInputProblem`, `slugFromName`, `daemon.skill`, `test/skills.ts`                                                                                                     | Plan:8093-8173, 7899-8036, 7574-7618            | Plan:8565-9171                                                                                                                 | ✓                                                                                                          |
| T11 → T13           | `LiveState.skillsVersion`, `daemon.listSkills`, `skillFixture`                                                                                                                                                                      | Plan:7853-7866                                  | Plan:9806-9848, 9964-9968                                                                                                      | ✓ (no other `LiveState` literal in web code or tests)                                                      |
| T12 → T13           | `SkillsPage({ version, epoch, online, onOpenSession })`; `SkillDraftProposer` → `commands.openTarget({ agentId, sessionId })` (`HelperTarget`, useSessionCommands.ts:92,231)                                                        | Plan:8891-8898                                  | Plan:9978-9990                                                                                                                 | ✓                                                                                                          |

## 2. Per-task self-consistency

| Task | Tests ↔ code                                                                                                                                                                                                                                        | Files created ↔ later edits                                                                                | Commands / filters                            | Anchors at HEAD                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Notes                                                                                    |
| ---- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------- |
| 1    | ✓ 11 tests. Traced against the code: round trip, BOM, CRLF, `---` inside the body, 4 KiB front matter, `[a, b]` name, missing description, `"two\nlines"`, 32 KiB+1 body, the SHA-256 vector                                                        | `mod.rs` extended by T2–T5, T9; `file.rs` final                                                            | `skills::` ✓                                  | `mod sessions;` lib.rs:18 ✓                                                                                                                                                                                                                                                                                                                                                                                                                                        | Reserved device names (I3); U+2028/2029 pass `one_line` (I1)                             |
| 2    | ✓ 11 registry tests, 2 store tests, 1 persistence test. Generation, prune boundaries (`>= cutoff`), the cap order, and restore pruning traced                                                                                                       | registry.rs extended by none; state.rs field used by T4                                                    | ✓ multiple libtest filters; version grep ✓    | consts control_plane_store.rs:18-36; `pre_upgrade_backup_path` :244 (else-branch comment "future version 8"); `postgres_backup_key` :257; `with_connector_state_and_cleanup` :433/464; tests :588-592, :708, :720, :771; state.rs `approvals:` :1506/:1668, `control_plane_snapshot` :1795, restore :1879/:2018, validate :2091; persistence.rs :598, :631/:682/:732/:766                                                                                          | Ten version-7 assertions, not eight (m12)                                                |
| 3    | ✓ 7 tests on Windows, 8 on Unix. The cache test changes content at equal length and restores mtime (`File::set_modified`, stable since 1.75)                                                                                                        | —                                                                                                          | ✓                                             | `crate::tools::{canonical_workspace_root, write_workspace_bytes}` re-exported tools.rs:35-38; `canonicalize()` workspace.rs:23-30                                                                                                                                                                                                                                                                                                                                  | Windows forms (I3, m5, m6, m7)                                                           |
| 4    | ✓ 11 service, 1 state, 1 live test. Every rescan test changes the file length, so the mtime+len cache misses deterministically; the scanner test is liveness only (5 s window, 20 ms period)                                                        | creates service/scanner/test_support/skill_state/agent_runs/skills.rs; skills.rs replaced by T9 as planned | ✓; `app::` run ✓                              | `mod session_state;` state.rs:6; `mod shutdown;` agent_runs.rs:28; `is_helper_config` :76; `control_plane_transactions` :680; `ApprovalResolved` events.rs:63/86/212; `serve_with_state` app.rs:226-271 (`scheduler.start` :245, `scheduler.shutdown` :266; the other `scheduler.start` at :200 is `app_with_configured_persistence`, correctly untouched); `companion_config`/`next_event` test_support.rs:243/312                                                | ~1,240 plan lines (§8)                                                                   |
| 5    | ✓ 9 tests (undo of record + draft, file-draft hash, rejection hiding, cap per agent, import cap)                                                                                                                                                    | —                                                                                                          | ✓                                             | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | Approving a stored draft overwrites an unrecorded `SKILL.md` (m9)                        |
| 6    | ✓ 6 route tests; `broken_store` (a directory as the JSON path) fails `AtomicFile` the same way M4's `invalid_snapshot_directory` does                                                                                                               | routes/skills.rs + tests extended by T7, T9                                                                | ✓                                             | `mod sessions;` routes/mod.rs:22; tests `mod sessions;` :1941; `approvals::delete_approval_rule,` :178; approvals tag :193; rule route :536; contracts `mod shared;` :11, `pub(crate) use sessions::*;` :41; `parse_json_body` re-export :73; `authorize`/`no_store` jobs.rs:56/63; `rejected` sessions.rs:38; README `### Agencies` :214                                                                                                                          | `request_timeout` 30 s applies (m3)                                                      |
| 7    | ✓ 3 multipart + 3 route tests. Parser bounds traced: no panic paths, `windows(n)` with n ≥ 3, monotone position                                                                                                                                     | —                                                                                                          | ✓                                             | `mod memories;` routes/mod.rs:17; `request_query` http.rs:413 (`pub(super)`)                                                                                                                                                                                                                                                                                                                                                                                       | `DRAFT_STATUS_INVALID` duplicates approvals.rs:34 (m13)                                  |
| 8    | ✓ 6 run tests, schema test (`tool_names().len() == expectations.len()`, tools/tests.rs:358), 2 grant tests                                                                                                                                          | skill_tests.rs extended by T9                                                                              | ✓                                             | `helper_config` tools line agent_runs.rs:113; `mod queue_tests;` :2171; `search_conversations` registration tools.rs:393-413; schema row tools/tests.rs:298; `TOOL_GRANTS` migration.rs:30-38; grant test :1229-1269; `apply_pending_tool_grants` session_state.rs:72-130; policy.rs:30/42 already class both tools                                                                                                                                                | Capability inventory test compares with `tool_names()` dynamically (capabilities.rs:6) ✓ |
| 9    | ✓ 3 runtime, 4 run, 1 route test. Context parts render as `[name]: text` under `## Context` (anima-core runtime.rs:1219); providers registered on the per-run runtime built from a snapshot (run_commit.rs:137-147) never reach the canonical agent | —                                                                                                          | `--no-run` reuses the test profile ✓          | `run_origin` block agent*runs.rs:1561-1570, `set_run_id` :1571; `AcceptRun` queue.rs:164-173 (four literals: test_support.rs:361, steer_tests.rs:19, approval_stop_tests.rs:491, routes/runs.rs:248); `RunRecord::queued` :477; `web_start` :751; "Skills arrive in M5" runs.rs:101-104; `connector` is `Option<Option<*>>`runs.rs:167-178 so`is_some()` catches an inactive Telegram connector ✓; M3 test runs.rs:183-184 ✓; README :181                          | Predictable dead-code warnings at Step 5 (m15)                                           |
| 10   | ✓ 4 + 1 tests; FormData leaves content-type unset (client.ts:315-344)                                                                                                                                                                               | —                                                                                                          | ✓ `sdk:build` last                            | `approvals` client.ts:79/104; `AgentEvent` union events.ts:47                                                                                                                                                                                                                                                                                                                                                                                                      | —                                                                                        |
| 11   | ✓ Reducer dedupe by seq (session-events.ts:299) makes the repeat test hold; the 2000×2001 diff case returns null                                                                                                                                    | —                                                                                                          | ✓                                             | `LiveState` session-events.ts:34-49, snapshot return :297, resync :302; `COMMON_TOOLS`/`COLLABORATE_TOOLS` agent-access.ts:6/24; `COMPANION_UNREACHABLE` approvals.ts:19                                                                                                                                                                                                                                                                                           | `act()` (I4)                                                                             |
| 12   | ✓ 10 page tests traced against the components                                                                                                                                                                                                       | —                                                                                                          | ✓                                             | `formatWhen` approvals.ts:172; every CSS token exists in styles.css; `@import './approvals.css';` styles.css:8                                                                                                                                                                                                                                                                                                                                                     | Edit pre-fill (I2); `act()` (I4)                                                         |
| 13   | ✓                                                                                                                                                                                                                                                   | —                                                                                                          | ✓ + web typecheck                             | `pick` ChatScreen.tsx:537-544; `commands?` :503; `submit`/`sendAgain` useSessionCommands.ts:169-229; `attempt` useSessionSends.ts:250-255; `AVAILABLE_PAGES`/`PRIMARY_DESTINATIONS` WorkspaceShell.tsx:19/37, page switch :589; `BoltIcon` icons.tsx:50; ViewHarness.tsx `SLASH_COMMANDS` :50/:1095, `live` :463, `queueSend` :801, `startChat` :825, `useSessionCommands` :929, `approvals=` :1246; `startRun` mocked in `mockProviders` ViewHarness.test.tsx:242 | Every harness test now reads `listSkills` (I4)                                           |
| 14   | ✓                                                                                                                                                                                                                                                   | —                                                                                                          | gate ✓; fallback keeps the shared `target/` ✓ | master status row ✓                                                                                                                                                                                                                                                                                                                                                                                                                                                | —                                                                                        |

## 3. Code-vs-plan check

No name, signature, or path the plan relies on is missing or different at HEAD. These facts change or sharpen the plan's claims:

| Plan claim                                                                 | Code at HEAD                                                                                                                                                                                                                                                                                  |
| -------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| "Task 2 updates the eight version-7 assertions" (Plan:10091)               | There are ten: state.rs:1177/1359/1381, approvals/registry.rs:921, live_tests.rs:446, control_plane_store.rs:592, and persistence.rs:631/682/732/766. Step 5's grep catches them all. Stale prose stays in persistence.rs:754/773/784 and state.rs:1395 ("version-7", "v7").                  |
| Slugs `^[a-z0-9][a-z0-9-]{0,63}$` minus `import` are safe folder names     | `canonicalize()` returns verbatim `\\?\C:\…` roots on Windows (workspace.rs:23-30), and `write_workspace_bytes` builds the target from that root (workspace.rs:151-161, 172-191). Verbatim paths skip Win32 name normalization, so `skills\con` or `skills\nul` is created literally (I3).    |
| The scan cache key is "modification time and size" (Plan:1189-1191)        | The workspace root can change at runtime (routes/workspace.rs:266/465/535/747/805). The cache is keyed by slug only, so a scan of the old root can be applied after the switch, or reused when mtime and size collide (m8).                                                                   |
| Skill routes "answer" within the transaction plus ≤10 s of file work       | `request_timeout` is 30 s (app.rs:97) behind a `TimeoutLayer` for non-run routes. A route waiting behind a long run commit, plus up to 30 s of file work on `delete`'s failure path, can answer 408 while its spawned change completes (m3).                                                  |
| `DRAFT_STATUS_INVALID` is a new named string (Plan:5701)                   | The same text is already `STATUS_INVALID` in routes/approvals.rs:34, and the route test asserts the literal (Plan:5457).                                                                                                                                                                      |
| `/skill` loads "the skill the message was sent with" (Plan:6873-6881)      | `SkillService::load` resolves through `SkillRegistry::find` (Plan:1318-1326), which falls back to a case-insensitive name match. If the slug's record is gone, a different approved skill whose name equals the slug is injected (m10).                                                       |
| Run route: skill checks right after `validate_input` (Plan:6999-7008)      | Replay detection happens later, inside `accept_run` (queue.rs:402-425); the route's Telegram branch already consults `replayed_run` before refusing (runs.rs:220-229). A retry with the same key after the skill turned off or changed gets 400, not the 200 replay spec §4.2 requires (m11). |
| `FormPart.filename` is part of the reader's interface                      | No non-test code reads it, so it warns as dead code; so do `LoadedSkill.slug` and the `status_for`/`SkillDetail` re-exports (m15).                                                                                                                                                            |
| Scans count "folders whose names are valid slugs, at most 200" (Plan:1883) | The cap is applied before `scan_in` drops folders without a `SKILL.md` (Plan:2252-2267). Empty or non-skill folders, which `write_file` can create (any `skills/<x>/file`), push real skills past the cap, and they then read `missing` (m4).                                                 |
| SDK/web need a `skill` field on run starts                                 | Already present: `StartRunInput.skill?` (packages/sdk/src/runs.ts:61), `Run.input.skill` (:38), `RunInput.skill` (runs/ledger.rs:134), and `RunResponse.input.skill` (routes/contracts/runs.rs:83).                                                                                           |
| Restarted queued skill runs                                                | Queued runs become `interrupted` at restart (ledger.rs:408-421). Task 13's `sendAgain` resends `run.input.skill` (Plan:9935-9943), so the skill survives "Send again".                                                                                                                        |

## 4. Spec coverage

| Spec item                                                                                                         | Task            | Status                                                                                  |
| ----------------------------------------------------------------------------------------------------------------- | --------------- | --------------------------------------------------------------------------------------- |
| §8.1 path, slug pattern, front matter limits, 32 KiB body, other files allowed                                    | T1, T3          | ✓ (+ reserved `import`; I3 adds device names)                                           |
| §8.1 registry fields; statuses; rescan at startup, on page requests, every 60 s (hash only on mtime change)       | T2, T4          | ✓ (mtime and size)                                                                      |
| §8.1 changed file not loaded until approved; file without a record is a draft                                     | T4, T5          | ✓ every load rehashes                                                                   |
| §8.2 draft fields, 10 pending per agent, approval writes through the hardened writer and pins the hash            | T5              | ✓ (+ `fileHash`, `decidedAtMs`)                                                         |
| §8.2 rejected kept 30 days; delete → `.anima-trash/skills/<slug>-<ts>`                                            | T2, T3, T4      | ✓ (approved also kept; cap 50)                                                          |
| §8.3 index of ≤50 enabled active skills in every run, helpers included, header text                               | T9              | ✓ for agents with `load_skill` (documented deviation, Plan:10100; ruled acceptable, §7) |
| §8.3 scan failure never fails a run; helpers cannot `propose_skill`                                               | T8, T9          | ✓                                                                                       |
| §8.3 `load_skill` prefix; `propose_skill` reply; `/skill` context part + `metadata.skill`                         | T8, T9          | ✓                                                                                       |
| §8.4 routes; 409 without a workspace; import is multipart                                                         | T6, T7          | ✓ (+ `hash` on approvals, documented)                                                   |
| §3.3 message `metadata.skill`; §4.1 `input.skill`                                                                 | T9              | ✓                                                                                       |
| §4.2 `skill` accepted; 400 unknown skill                                                                          | T9              | ✓ (+ three more 400s; m11 on replay order)                                              |
| §6 `skill.updated`                                                                                                | T4, T10, T11    | ✓                                                                                       |
| §7.1 `load_skill` read, `propose_skill` write                                                                     | — (M4 table)    | ✓ already at policy.rs:30/42                                                            |
| §13.1 control-plane tier                                                                                          | T2              | ✓ v8 + backup                                                                           |
| §13.3 step 1 backup; step 4 files under `skills/` appear as drafts; step 5 grants                                 | T2, T4, T8, T11 | ✓                                                                                       |
| §13.4 OpenAPI entry and README row per route                                                                      | T6, T7          | ✓ (10 rows)                                                                             |
| §14 hardened writer; owner auth + `no-store`; hash pinning; skill text framed as data                             | T3–T9           | ✓ in the daemon; ✗ review surface (I1, I2)                                              |
| §15.1 Skills destination; §15.3 `/<skill-slug>` in the composer                                                   | T13             | ✓                                                                                       |
| §15.4 list + toggles + status, drafts with diff and Approve/Edit/Reject, editor with preview, New, Delete, Import | T12             | ✓ (the preview is plain text; documented)                                               |
| §15.5 `useSkills`; reducer tolerant of repeats                                                                    | T11             | ✓                                                                                       |
| §16 skills limits; "skills scan … failures never fail a run"                                                      | T1, T9          | ✓                                                                                       |
| §17 daemon skills tests (hash pinning, changed-file hold, drafts, index, `load_skill`, `/skill`), SDK, web        | T1–T13          | ✓; Playwright skills flow deferred to M10 T10.2                                         |

## 5. Carry-forward coverage

| Source                                                                            | Task        | Status                                                                                     |
| --------------------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------ |
| Master plan M5 T5.1 (registry, scan, pinning, drafts, routes)                     | T1–T7       | ✓ master file names kept                                                                   |
| Master plan M5 T5.2 (index, `load_skill`, `propose_skill`, `/skill`)              | T8, T9      | ✓                                                                                          |
| Master plan M5 T5.3 (SDK, page, composer)                                         | T10–T13     | ✓                                                                                          |
| Master "Carried from M3"                                                          | —           | nothing for M5 ✓                                                                           |
| M4 ledger follow-up: tell Telegram/CLI users an approval waits                    | —           | stays post-M4 ✓ (Plan:10087)                                                               |
| M4 audit I2: an exec rule lets the companion act as the owner                     | T6 README   | ✗ partly: the Skills "Limits" paragraph doesn't say this extends to approving skills (m18) |
| M4 lesson: owner mutations drop-safe (spawned)                                    | T4 `locked` | ✓                                                                                          |
| M4 lesson: timing-dependent tests                                                 | T4          | ✓ (liveness-only scanner test; negative 100 ms check can't fail spuriously)                |
| M4 lesson: empty or degenerate inputs                                             | T1, T5–T7   | ✓ mostly; an empty `hash` answers 409 mismatch, not 400 (m14)                              |
| M4 lesson: Windows path forms                                                     | T3          | ✗ reserved names (I3); case-insensitive names (m6); no Windows link test (m7)              |
| M4 lesson: strings tested once as named constants                                 | T1–T9       | ✓ mostly; three strings untested (m16); one duplicate (m13)                                |
| M4 lesson: `text` fences, SDK build last, no new `CARGO_TARGET_DIR`, pristine web | Global      | ✓ (pristine web at risk: I4)                                                               |
| M4 rollback-doc precedent                                                         | T2          | ✓ README bullet Plan:1845                                                                  |

## 6. Findings

### Blocker

None. Every anchor exists, and the code as written compiles on inspection.

### Important

**I1. Invisible characters make owner review meaningless.** Plan:279-285 (`one_line`), 295-303 (`validate_body`), 8836-8838 / 8823-8834 / 9031 (bodies, diffs, and reviews rendered in `<pre>`).

- **What's wrong:** a draft from `propose_skill`, an import, or a file the companion wrote can hide instructions in Unicode that a reviewer cannot see but the model reads:
  - tag characters U+E0000–E007F ("ASCII smuggling");
  - bidi embeddings, overrides, and isolates U+202A–202E and U+2066–2069;
  - zero-width and other format characters U+200B–200F, U+2060–2064, U+FEFF.
- **Why it slips through:** `char::is_control` covers only Cc, so names and descriptions also accept U+2028/U+2029 line and paragraph separators. That breaks the "never spans lines" framing claim (Plan:30, 6755-6756). The page renders all of this as plain text, so the owner approves exactly what they cannot see (spec §14 "framed as data"; Review Focus 2).
- **Ruling:**
  - Daemon (Task 1):
    - Add `SKILL_TEXT_HIDDEN = "Skill text must not contain invisible tag or direction-override characters"`.
    - Refuse U+E0000–E007F, U+202A–202E, and U+2066–2069 in `validate_body` and `one_line`.
    - Make `one_line` also refuse U+2028, U+2029, and every `General_Category=Cf` character except a leading BOM.
    - Test each once.
  - Web (Task 12): render the remaining format characters in bodies, diffs, and reviews as visible `⟨U+200B⟩` markers. Show "This text contains N invisible characters" on the card, through a pure helper in `lib/skills.ts` with a test.
  - No change for ZWJ inside emoji beyond the visible marker.

**I2. "Edit" can approve an unreviewed, changed `SKILL.md` with one click.** Plan:8929-8941 (`edit`), 9101-9107.

- **What's wrong:**
  - `edit(skill)` pre-fills the editor from `daemon.skill(slug).file.body`, the current file and not the approved text. For a `changed` skill, or an `active` one whose edit the cache hasn't noticed yet (equal size and mtime), that is text nobody approved.
  - The owner opens Edit to fix a description, the injected 32 KiB body rides along, and "Save skill" (`PUT`) approves and pins it.
  - This bypasses the explicit "Review changes / Approve this version" flow (Review Focus 1) through the UI.
- **Ruling (cheap, Task 12):**
  - In `edit`, when `detail.file?.hash !== skill.approvedHash` or `detail.file?.problem`, do not open the editor. Open the Review section instead, with a note: "This skill's file changed since you approved it. Review it first, then edit."
  - Add a page test: a `changed` skill's Edit opens the review and `saveSkill` is not called.
  - The daemon stays as is: `PUT` is the owner's content by definition.

**I3. Windows device names are valid slugs and create folders Windows tools can't handle.** Plan:174 (`RESERVED_SKILL_SLUGS`), 243-253, 2273-2276; code tools/workspace.rs:23-30, 151-191.

- **What's wrong:**
  - `con`, `prn`, `aux`, `nul`, `com0`–`com9`, and `lpt0`–`lpt9` pass `is_valid_slug`, and `slugify("Con")` returns `con`.
  - The writer joins onto the verbatim `\\?\` canonical root, so `create_dir_all` makes a literal `skills\con` folder. Explorer, OneDrive sync, and git can't open or delete it.
  - The workspace on this machine is OneDrive-backed. A model proposal (`propose_skill` with slug `aux`) or an import is enough; approval then creates the folder.
  - Without the verbatim prefix, the same path fails with a confusing 503 instead of a 400.
- **Ruling (Task 1):**
  - Extend `RESERVED_SKILL_SLUGS` to `["import", "con", "prn", "aux", "nul", "com0"…"com9", "lpt0"…"lpt9"]` on every platform, since workspaces are portable.
  - Reword `SKILL_SLUG_INVALID` to "… and not a reserved name (import, con, nul, …)".
  - Extend the slug test.
  - Mirror the list in the web `slugFromName`/`skillInputProblem` (Task 11), with one test case.

**I4. The web tests' `act()` hygiene is left to after-the-fact fixes.** Plan:8139-8152 (`act` → `refresh()`), 9819-9848 (`useSkillCommands`), 9324, 9605, 9996.

- **What's wrong:**
  - Every `useSkills` action resolves before its reload. The reload's `setState` lands after `user.click` returns, and six of the ten page tests end on such an action: toggle, reject, file-draft approve, review approve, delete, import.
  - `useSkillCommands` calls `setSkills(next)` with a fresh `[]` on every harness test that goes online (about every ViewHarness test). That re-renders outside `act` whenever the mocked promise settles after a `waitFor` window.
  - The plan's remedy (Plan:9324) is reactive, inside a 3,900-line harness test file, against the global "pristine" rule (Plan:40).
- **Ruling:**
  1. `useSkills`: give the effect a `load(signal)` function and have each action `await load()` before returning `true`, keeping the version/epoch effect. The state then settles inside user-event's act window.
  2. `useSkillCommands`: `setSkills((previous) => (previous.length === 0 && next.length === 0 ? previous : next))`, which bails out without a render.
  3. Each page test whose last step is an action ends with a `findBy…` on post-reload state (or `await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2))` plus one awaited assertion).
  4. Keep the Step 5 instruction as the fallback.

### Minor

- **m1. Deviation: the skills index goes only to agents with `load_skill`** (Plan:6868, 10100; spec §8.3 says "any workspace agent").
  - **Ruling:** accept; it is sound, since an agent without the tool can't use the list. Name the deviation in the README Skills paragraph (Plan:5063).
- **m2. "Review changes" shows the whole file and no diff against the approved text** (deferred, Plan:10108).
  - **Ruling:** accept the deferral, and add copy to the review section: "Anything with write access to the workspace, including your companion, can change this file. Read it in full before approving."
  - Optional later: keep a hash-verified copy of approved bytes at `.anima/skills-approved/<slug>.md`. It is used only when its hash equals `approvedHash`, so the companion can't tamper with it unseen, and it adds no control-plane growth.
- **m3. Long transaction holds** (Plan:3060-3078, 3290-3341, 3354-3359, 4188-4193, 4283-4288).
  - The holds are bounded but long: `delete`'s failure path is up to 30 s (trash + untrash + refresh), and `approve_file_draft` up to 20 s. Run commits, approval decisions, and connector publishes wait meanwhile, and routes can 408 (30 s timeout).
  - **Ruling:** read and hash outside the transaction for `approve_changed`, `approve_file_draft` (unedited), and `reject_file_draft`. Pinning stays fail-closed, because the pinned hash is the hash of the bytes read and a later edit reads `changed`.
  - Keep writes and moves inside. Log a warning when one blocking call exceeds 1 s.
- **m4. The scan cap counts folders before filtering, and the companion can flood file drafts** (Plan:2252-2267, 1439-1459).
  - Any `skills/<x>/file` that `write_file` creates counts toward the 200, pushing real skills out of the scan (they read `missing`, leave the index and the composer).
  - File drafts bypass the 10-per-agent cap (up to 200 cards).
  - **Ruling:** scan record slugs first, then the rest by name, and count only folders that hold a `SKILL.md`. Show at most 20 file drafts, with "N more in the skills folder".
- **m5. `untrash_skill_folder` checks a string prefix, and the trash folder can be created outside the workspace** (Plan:2324-2336, 2302-2310).
  - The prefix check plus `contains("..")` is fragile; the input is internal today.
  - `create_dir_all(root/.anima-trash/skills)` follows a `.anima-trash` link before the canonical check, so empty folders can be created outside the workspace.
  - **Ruling:** `trash_skill_folder` returns the trash name only. `untrash` rebuilds the path from `SKILLS_TRASH_FOLDER` and refuses a name that isn't `<slug>-<digits>[-<digits>]`. Canonicalize `.anima-trash` (or refuse a link) before `create_dir_all`.
- **m6. Folder names that differ only in case** (Windows, case-insensitive): a folder `skills/Notes` is filtered out of scans (not a valid slug), but `read_skill_bytes("notes")` opens it. The record reads `missing` while loads succeed, and no file draft shows.
  - **Ruling:** make a scan report a non-lowercase folder whose lowercase is a valid slug as an `invalid` file draft with the problem "Rename the folder to lowercase".
- **m7. Link refusal is untested on Windows** (Plan:2061-2100 are `#[cfg(unix)]`); `SKILL_FILE_OUTSIDE` and `SKILLS_FOLDER_OUTSIDE` are therefore never tested on the owner's platform.
  - **Ruling:** add a `#[cfg(windows)]` test that makes a junction with `cmd /C mklink /J` (no privilege needed) for the `skills` folder and a skill folder. It returns early, without failing, if `mklink` fails.
- **m8. A workspace switch keeps the scan state** (routes/workspace.rs:266/465/535/747/805; Plan:1186-1197).
  - **Ruling:** add `SkillRegistry::reset_scan()` (clears `scanned`, bumps the generation), and call it wherever `guard.workspace` changes. One call per site, or one helper `set_workspace`.
- **m9. Approving a stored draft silently overwrites an unrecorded `SKILL.md`** (a file draft the owner never saw; Plan:4105-4170). The same applies to `PUT`.
  - **Ruling:** when a file exists without a record, move it to `.anima-trash/skills/<slug>-<ms>` first, or refuse 409 "A SKILL.md the owner hasn't reviewed is in this folder; review it first". Prefer the refusal: it is cheaper and clearer.
- **m10. `/skill` resolves by name fallback** (Plan:3407, 1318-1326, 6874).
  - **Ruling:** add `SkillService::load_slug(slug)` (exact slug) for `apply_skills`. Keep `find` for the model's `load_skill { name }`.
- **m11. Skill checks run before idempotent replay** (Plan:6999-7008).
  - **Ruling:** when `check_runnable` refuses, return `state.agent_runs.replayed_run(...)` if it finds the key (as runs.rs:220-229 does), else the 400.
- **m12. Version-7 leftovers** (Plan:10091).
  - The plan says "eight" assertions; there are ten (§3).
  - The stale "version-7"/"v7" prose at persistence.rs:754/773/784 and state.rs:1395 is not caught by Step 5's grep.
  - **Ruling:** update the prose too, and fix the count in the notes.
- **m13. Duplicate string:** `DRAFT_STATUS_INVALID` repeats approvals.rs:34's `STATUS_INVALID`, and the test asserts the literal (Plan:5457).
  - **Ruling:** reuse one `pub(super)` constant from `routes/approvals.rs` (or move it to `routes/http.rs`), and assert the constant.
- **m14. An empty or malformed `hash` answers 409 "changed since you reviewed it"** (Plan:3353-3363, 4182-4196).
  - **Ruling:** trim and lowercase it; refuse anything that is not 64 hex characters with 400 `SKILL_HASH_REQUIRED` before reading the file. Add one test case in each approve test.
- **m15. Step 5 of Task 9 will surface predictable warnings** (Plan:7025-7028): `LoadedSkill.slug` (asserted by a test, so deleting it breaks Task 4's test), `FormPart.filename`, and the `status_for`/`SkillDetail` re-exports.
  - **Ruling:** use `skill.slug` in `requested_text`'s success branch (or the warn log) so the field is read; mark `FormPart.filename` `#[cfg_attr(not(test), allow(dead_code))]`, a reader field kept for M9 with a comment; drop the two unused re-exports in Task 9.
- **m16. Three strings are never tested:** `SKILL_IO_TIMED_OUT`, `SKILLS_UNAVAILABLE`, and `PROPOSAL_NOT_SAVED`.
  - **Ruling:** test `PROPOSAL_NOT_SAVED` (broken store + `propose_skill` run) and `SKILLS_UNAVAILABLE` (`load_skill` through a context without `team`) in Task 8. `SKILL_IO_TIMED_OUT` stays covered only by the type.
- **m17. Statuses go `missing` before the first scan** (Plan:1351-1375, 1591-1604): `put`/`restore` recompute status from an empty scan map right after a restart. A PATCH before the first scan turns an `active` skill `missing` (out of the index; `/skill` 400) until the next scan.
  - **Ruling:** keep a `scanned_once: bool` in `SkillRegistry`. Until it is set, `put`/`restore` keep the record's previous status.
- **m18. README "Limits" for skills** (Plan:5065) should add:
  - an exec rule that lets the companion act as the owner (Approvals Limits) also lets it approve its own skill drafts;
  - the `Found in the skills folder` source covers files the companion wrote with `write_file`.
  - **Ruling:** add both sentences, and change `SOURCE_LABELS.file` to "Found in the skills folder (not written on this page)".
- **m19. Multipart reader for M9** (Plan:5207-5338):
  - fine for M5 (≤80 KiB, ≤8 parts, no panic paths);
  - for M9's 25 MiB uploads, `find` is O(n·m) on adversarial input and copies every part;
  - `;` inside a quoted `filename` splits wrongly.
  - **Ruling:** accept for M5. Record in the plan's Deferred list that M9 must add a per-part size cap and a linear boundary search before reuse.

## 7. Rulings on the plan writer's risks

**(a) Hash pinning end to end.**

- **Sound in the daemon:**
  - `load` (tool and `/skill`), `approve_changed`, and `approve_file_draft` reread whole files inside the workspace and compare SHA-256. They never trust `status` or the mtime+len cache.
  - Stored-draft approval pins the bytes it writes, and drafts are immutable once stored.
  - The index uses only `status`, which can at worst list a skill whose load then refuses.
  - A failed write, failed save, or timed-out blocking write that lands after the undo always leaves a file whose hash differs from the record, which reads `changed` or as a file draft (fail closed). Traced for `save`, the draft approvals, and `delete`.
- **Gaps are on the review side:** invisible text (I1) and Edit pre-filling a changed body (I2).
- **Smaller gaps:** the name fallback for `/skill` (m10) and the empty hash (m14).
- **Ruling:** keep the design; apply I1, I2, m10, m14.

**(b) Concurrency inside `apply`, and the transaction across file work.**

- **The order is correct:** registry change under the write lock → blocking write (no lock) → `set_scanned` → persist request under the write lock → `save().await` with no state lock → publish under the read lock.
- **Undo restores the record and draft exactly.** Pruning is intentionally not undone.
- **Stale scans are dropped:** every record change, `set_scanned`, and draft change bumps the generation, so a scan that began before is dropped. `apply_scan` mutates `status` without the transaction, which is acceptable: no runtime path restores whole snapshots (only tests call `restore_control_plane_snapshot`).
- **Lock order holds:** transaction → state lock → fanout `std::sync::Mutex` (fanout.rs:34-38), and no std lock is held across `.await` (the scanner's `running` guard is a statement temporary).
- **Drop safety:** every mutation runs in `tokio::spawn` (`locked`); read paths mutate only synchronously.
- **Holding the transaction:** ≤10 s per blocking call is acceptable and matches M4, whose saves also do file I/O under the transaction. Shorten the worst cases per m3, since on a OneDrive workspace, hydration or a sync lock can make the 30 s `delete` failure path real.

**(c) The model's reach through `write_file` into `skills/*/SKILL.md`.**

- **Accept:** a written file is only a file draft or a `changed` skill and never loads unapproved.
- **Required alongside:**
  - the UI must not launder it (I2);
  - review must show it fully (I1);
  - its provenance must be honest (m18);
  - it must not crowd out real skills (m4).
- **No path guard on `write_file`:** it would not stop `bash` and adds nothing to pinning.

**(d) Prompt framing.**

- **Index:** the data header plus one-line, owner-approved names and descriptions is adequate once `one_line` also refuses U+2028/U+2029/Cf (I1).
- **`/skill` body:** owner-approved instructions, deliberately not framed as data. Accept.
- **Scope:** only agents with `load_skill` get the index. Accept (m1).

**(e) Path safety.**

- Slug validation precedes every path build. Canonical checks cover a dangling link (outside), a file link, a folder link (moved as the link), and the `skills` folder link. Junctions behave like symlinks under `canonicalize`/`rename`.
- **Gaps:** device names (I3), case-only names (m6), no Windows link test (m7), the untrash prefix check and trash-folder link (m5).
- Trailing dots and spaces, drive letters, and UNC roots are not reachable through a slug. A canonical `\\?\UNC\…` root stays consistent with its files' canonical paths.

**(f) The hand-written multipart reader.**

- Bounds are correct for M5: ≤80 KiB read through `read_limited_body`, ≤8 parts, monotone parsing, no panics.
- Origin-based owner auth blocks cross-site simple POSTs.
- **Accept**, with m19's notes for M9.

**(g) Snapshot v8 and rollback docs.**

- **Correct:**
  - `pre_upgrade_backup_path` gains the version-7 branch, so `.pre-approvals.bak` is never overwritten;
  - Postgres uses `control_plane.backup.7` through `postgres_backup_key(loaded.version)` (persistence.rs:111);
  - an M4 binary refuses v8 with `unsupported control plane store version: 8` (control_plane_store.rs:195-203);
  - the README bullet (Plan:1845) names both backups and what is lost;
  - re-upgrading re-applies the `m5-skills` grant idempotently.
- **Fix only m12.**

**(h) Web `act()` warnings.** I4's three code-level changes, instead of after-the-fact test edits.

## 8. Model tier per task and split

| Task | Tier                        | Note                                                            |
| ---- | --------------------------- | --------------------------------------------------------------- |
| 1    | standard (cheap acceptable) | + I1 daemon checks, I3 reserved names                           |
| 2    | standard                    | v8 plumbing (M2–M4 pattern), + m12, m17                         |
| 3    | standard                    | + m4, m5, m6, m7 (Windows junction test)                        |
| 4    | **most capable**            | transaction, undo, drop safety (+ m3, m8); optional split below |
| 5    | most capable                | two-part undo, file-draft pinning (+ m9, m14)                   |
| 6    | standard                    |                                                                 |
| 7    | standard                    | + m13, m19 note                                                 |
| 8    | standard                    | + m16                                                           |
| 9    | most capable                | run path, route order (+ m10, m11, m15)                         |
| 10   | cheap                       |                                                                 |
| 11   | standard                    | + I4 hook changes, I3 mirror                                    |
| 12   | standard                    | + I1 visible markers, I2 Edit gate, m2/m18 copy                 |
| 13   | most capable                | composer, send queue, shell, quiet harness (+ I4 guard)         |
| 14   | controller                  |                                                                 |

**Split.** None is required. Task 4 (about 1,240 plan lines, 13 tests) is the largest daemon task. If the implementer's context runs tight, split it:

- **4a (standard):** `LiveEventBody::SkillUpdated` + its live test, `state/skill_state.rs` + test, and `skills/test_support.rs`.
- **4b (most capable):** `service.rs` and its 11 tests, `scanner.rs`, `agent_runs/skills.rs`, and the `app.rs` wiring.
