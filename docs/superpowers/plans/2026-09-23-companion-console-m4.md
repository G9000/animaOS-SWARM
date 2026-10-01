# Companion Console M4: Approvals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ask the owner before a risky tool call runs: every tool has a risk class, each companion has a policy and "Always allow" rules, a call that needs the owner waits (holding no lock) for a decision, the run's Stop, or a 30-minute timeout (15 for Telegram-started runs), helpers are denied instead of waiting, restarts expire what was pending, decided approvals move to the history store, and the web console shows inline approval cards, an Approvals page with rules and policy controls, badges, and the ⌘K additions.

**Architecture:** A new `approvals` module in the daemon holds the static risk table, matchers, and policy evaluation (pure), the control-plane registry of pending and not-yet-mirrored approvals with per-agent policies and rules, and the gate that `ToolExecutionContext::execute_tool` consults after its live checks and before dispatch. State changes (open a request, settle it, revert either) are methods on `DaemonState` made under the control-plane transaction and the state lock; the coordinator's async side (`agent_runs/approvals.rs`) saves them, wakes the one waiting call through a oneshot, and publishes `run.awaiting_approval`, `approval.requested`, `approval.resolved`, and `run.started` (a resume). The stop path settles a run's pending approvals as `stopped` inside the stop's own save. The history store's existing `approvals` tables gain their reads and writes, fed by the outbox like terminal runs. The SDK gets `ApprovalsClient` and typed approval events; the web reducer tracks pending approvals per run, the transcript renders an `ApprovalCard` inline, and a new `#/approvals` page lists pending and decided approvals and edits rules and policy.

**Tech Stack:** Rust 2021 (tokio, axum 0.8, serde, rusqlite 0.32, sqlx, utoipa 5, reqwest's `Url`), TypeScript (React 19, Vite, Tailwind v4, Vitest, Testing Library), Nx with Bun.

**Spec:** `docs/superpowers/specs/2026-09-23-companion-console-design.md` (§7 Approvals is the core; also §3.2 `sessionAllowances`, §4.1 `awaiting_approval`, §4.4 item 5 derived status, §4.6 Stop while awaiting approval, §4.7 steers wait, §4.8 restart expiry, §6 `approval.requested|resolved` and the snapshot's pending approvals, §13.1 the `approvals` table and the outbox, §14 owner authorization and revision-checked idempotent decisions, §15.1 Approvals destination and badges, §15.2 inline approval cards, §15.3 ⌘K additions, §15.4 Approvals page, §16 limits, §17 tests). Master plan: `docs/superpowers/plans/2026-09-23-companion-console.md` (M4, T4.1–T4.3, and "Carried from M3"). Carry-forwards: `.superpowers/sdd/2026-09-23-companion-console-m4/carry-forwards.md`. M3 plan for conventions: `docs/superpowers/plans/2026-09-23-companion-console-m3.md`.

## Global Constraints

- Master plan Global Constraints apply. **No new third-party dependencies in M4** (Rust, SDK, and web). URL parsing uses `reqwest::Url`, which the daemon already depends on. `anima-core` is not touched.
- **Precondition: M3 is merged.** Before Task 1 run `git log --oneline -1 && grep -n "RunAwaitingApproval" hosts/rust-daemon/src/live/events.rs && grep -n "AwaitingApproval" hosts/rust-daemon/src/runs/ledger.rs && grep -n "awaiting_approval" packages/sdk/src/runs.ts && grep -n '"approvals": \[\]' hosts/rust-daemon/src/live/events.rs`. Expected: head at or after `c411a22`, and a match in each file. Otherwise stop and report that M3 has not landed.
- **Risk classes (spec §7.1), exactly.** Unknown tools are `exec`.
  - `read` (never asks): `read_file`, `list_dir`, `glob`, `grep`, `todo_read`, `memory_search`, `recent_memories`, `get_current_time`, `calculate`, `bg_list`, `bg_output`, `list_workspace_agents`, `calendar_list_events`, `mail_list_messages`, `search_conversations`, `load_skill`, `list_automations`.
  - `write`: `write_file`, `edit_file`, `multi_edit`, `todo_write`, `memory_add`, `propose_skill`, `create_automation`, `pause_automation`, `mail_create_draft`, `calendar_create_event`, `calendar_update_event`, `calendar_delete_event`.
  - `exec`: `bash`, `bg_start`, `bg_stop`.
  - `network`: `web_fetch`, `exa_search`.
  - `delegate`: `delegate_to_agent`, `spawn_helper`, `send_message`, `broadcast_message`.
  - `load_skill`, `list_automations`, `propose_skill`, `create_automation`, and `pause_automation` are classified now; their tools arrive in M5 and M6.
- **Policy (spec §7.2).** Each agent's control-plane policy maps `write`, `exec`, `network`, and `delegate` to `allow`, `ask`, or `deny`. **The default is `exec: ask` and all others `allow`.** Policies are never writable by tools: only the owner-authorized routes of Task 8 change them.
- **Evaluation order (spec §7.2), exactly:** class `deny` → denied; matching rule or session allowance → allowed; class `ask` → ask; otherwise allowed. Read-class tools are always allowed.
- **Rules** are `{ id, agentId, tool, matcher: { kind: command_prefix | path_glob | domain | any, value }, createdAtMs, fromApprovalId }`. Matcher kinds per tool: `bash`, `bg_start` → `command_prefix`, `any`; `write_file`, `edit_file`, `multi_edit` → `path_glob`, `any`; `web_fetch` → `domain`, `any`; every other tool → `any`.
- **Decisions (spec §7.3):** `allow_once | allow_session | allow_always | deny`, with `note?` (at most 1,000 characters), `matcher?`, and `revision` (1 while pending; every resolution adds 1). `allow_session` adds a session allowance (`matcher` or the suggestion); `allow_always` creates a rule from `matcher` or the suggestion. **Statuses:** `pending`, `allowed`, `denied`, `stopped`, `expired`; `resolvedBy`: `owner`, `timeout`, `stop`, `restart`.
- **Strings, exact** (named constants, each tested once):
  - Tool results: `"Denied by owner policy"` (`DENIED_BY_POLICY`), `"Denied by owner: <note>"` or `"Denied by owner"` without a note (`DENIED_BY_OWNER`, `denial_text`), `"Needs owner approval; not available to helpers"` (`HELPER_NEEDS_APPROVAL`), `"Cancelled before running (stopped by owner)"` (the existing `anima_core::CANCELLED_TOOL_RESULT`), `"Needs owner approval, but the request could not be saved; the tool did not run"` (`APPROVAL_NOT_SAVED`), `"Needs owner approval, but this run cannot ask for it; the tool did not run"` (`APPROVAL_UNAVAILABLE`), `"The approval request was lost; the tool did not run"` (`APPROVAL_LOST`).
  - The timeout note: `"Approval timed out"` (`APPROVAL_TIMED_OUT`), so a timed-out call's result is `"Denied by owner: Approval timed out"`.
  - Route errors: `"This approval was already resolved"` (409, `APPROVAL_ALREADY_RESOLVED`), `"This approval changed; reload it and decide again"` (409, `APPROVAL_REVISION_STALE`), `"note must be at most 1,000 characters"` (400, `APPROVAL_NOTE_TOO_LONG`), `"This matcher kind does not apply to this tool"` (400, `MATCHER_KIND_NOT_FOR_TOOL`), `"matcher value is not valid for its kind"` (400, `MATCHER_VALUE_INVALID`), `"This companion already has 100 approval rules; remove one first"` (409, `TOO_MANY_RULES`), `"This session already has 50 allowances"` (409, `TOO_MANY_SESSION_ALLOWANCES`), `"This approval's session no longer exists"` (409, `APPROVAL_SESSION_GONE`), `"Read-class tools never ask, so they need no rule"` (400, `READ_TOOLS_NEED_NO_RULE`), `"Helpers use their companion's approval policy and rules"` (409, `HELPERS_USE_COMPANION_APPROVALS`), `"unknown tool"` (400, `UNKNOWN_RULE_TOOL`), `"status must be pending or decided"`, `"cursor is not valid"`, `"limit must be between 1 and 100"` (400s).
- **Limits (spec §16) and plan bounds**, constants named once in `hosts/rust-daemon/src/approvals/mod.rs`: `APPROVAL_TIMEOUT_MS = 30 * 60 * 1000`; `TELEGRAM_APPROVAL_TIMEOUT_MS = 15 * 60 * 1000`; `MAX_APPROVAL_ARGUMENTS_BYTES = 16 * 1024`; `MAX_APPROVAL_NOTE_CHARS = 1_000`; `DECIDED_APPROVAL_WINDOW_MS = 30 * 24 * 60 * 60 * 1000`; and, where the spec is silent (bounded snapshot growth, spec §1): `MAX_APPROVAL_RULES_PER_AGENT = 100`, `MAX_SESSION_ALLOWANCES = 50`, `MAX_MATCHER_VALUE_CHARS = 512`, `DEFAULT_APPROVAL_PAGE = 50`, `MAX_APPROVAL_PAGE = 100`. History: `HISTORY_APPROVAL_BATCH = 200` (`history/outbox.rs`). SDK and web: `MAX_APPROVAL_NOTE_CHARS = 1_000` (SDK, re-used by the web); `MAX_SESSION_COMMANDS = 50` (⌘K session titles).
- **Routes, exactly** (Tasks 7 and 8): `GET /api/approvals?status=pending|decided&agentId=&cursor=&limit=` (pending from the control plane, oldest first; decided from the history store merged with the control plane's not-yet-mirrored ones, 30 days, newest first); `POST /api/approvals/{approval_id}/decision`; `GET|PUT /api/agents/{agent_id}/approval-policy`; `GET|POST /api/agents/{agent_id}/approval-rules`; `DELETE /api/agents/{agent_id}/approval-rules/{rule_id}`. Reads call `authorize(&state, &request, true)`; mutations `authorize(&state, &request, false)` (both are `routes::jobs::authorize`, which calls `state.local_owner.authorize_read`/`authorize`); every response, errors included, goes through `routes::jobs::no_store` or `routes::sessions::rejected`. Every route has a `#[utoipa::path(... tag = "approvals" ...)]` registered in `ApiDoc` and a row in `hosts/rust-daemon/README.md`.
- **Events (spec §6):** `approval.requested` and `approval.resolved` carry `approval` (the `ApprovalResponse` JSON) plus `agentId`, `sessionId`, `runId`, `seq`, `at`. A request publishes `run.awaiting_approval` (only when the run moves there) and then `approval.requested`; a resolution publishes `approval.resolved` and then, when the run's last pending approval is settled, `run.started` with the run back at `running` (`startedAtMs` unchanged). `stream.snapshot`'s `approvals` lists the pending approvals of the snapshot's runs.
- **Untrusted arguments.** A request stores the call's arguments as JSON text cut to 16 KiB on a char boundary (`argumentsTruncated`). The web shows them only as a text node inside `<pre>`, never through Markdown or HTML.
- **Concurrency, every task.** Lock order: control-plane transaction → state lock → live registry, fanout, or approval-waiters mutex (the waiters mutex is a leaf and is never held while taking the state lock). No `std::sync::Mutex` is held across `.await`. The waiting call holds no lock: it `select!`s (biased) on its oneshot, the run's `CancelSignal::cancelled()`, and `tokio::time::sleep_until(deadline)`. Every settlement (owner decision, timeout, stop) takes the control-plane transaction and flips `pending` exactly once under the state write lock, so exactly one wins; the loser sees the record already resolved. The owner's decision is saved before its waiter is woken; a failed save reverts it (503) and leaves the call waiting. A request is saved before it is announced; a failed save refuses the tool (fail closed).
- **Snapshot version 7.** M4 adds `approvals`, `approvalPolicies`, `approvalRules`, and `SessionRecord.sessionAllowances` to the control plane; an M3 daemon would load a v7 file and silently drop the owner's policy and rules, so the version moves to 7 and the first start writes `<file>.pre-approvals.bak` (JSON) or the `control_plane.backup.6` row (Postgres) first, following M2's `.pre-sessions.bak` and M3's `.pre-live-runs.bak`.
- Existing behavior stays except where a task's Interfaces block says so. Deliberate change: a coordinator run's `bash`, `bg_start`, and `bg_stop` now wait for the owner by default (`exec: ask`), including runs from the legacy `POST /api/agents/{id}/run`, the CLI, Telegram, schedules, and jobs. Swarm runs (`/api/swarms/...`) have no session or owner UI and are not gated.
- Commands. Rust iteration: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- <filter> <filter>` (filters after `--`). SDK: `bun x nx test @animaOS-SWARM/sdk`, and **every SDK-changing task ends with `bun x nx run @animaOS-SWARM/sdk:build`** so later direct web Vitest runs resolve the new exports. Web: `cd apps/web && bun x vitest run <files>`. The milestone gate (Task 14) runs `bun x nx run rust-daemon:test --skipNxCache` and `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`.
- Formatting is clean at the start. Every task ends with `cargo fmt --all` when it touched Rust and `bun x nx format:write --files=<each changed TS/TSX/CSS/MD file>` when it touched TypeScript, CSS, or Markdown, then re-runs its tests, so every commit stays formatted.
- Stage files by explicit path only; never `git add -A`, `git add .`, or `git commit -a`. Never stage anything under `docs/` or `.superpowers/` (the controller commits docs; Task 14's status-row commit is the controller's). Never use `git stash`, `git reset`, `git checkout -- <path>`, `git restore`, or `git worktree`. Do not start the daemon or a dev server; tests start what they need.
- Disk is tight (about 13 GB free; the Nx Rust gate needs about 12): never set a new `CARGO_TARGET_DIR`. CI stops at `nx start-ci-run` (Nx Cloud), so the local commands are the verification. No Postgres is available: the Postgres conformance test stays `#[ignore]` and is hand-checked.
- Large files stay put: `agent_runs.rs` (~5,900 lines), `connectors/runtime.rs` (~7,600), and `ViewHarness.tsx` (~1,250) only gain wiring lines; new code goes in new modules, hooks, and components. Web tests stay pristine: no new `act()` warnings or console noise.
- Code fences: complete files and complete functions keep their language; partial fragments (a few lines to insert, a changed signature) are fenced as `text` so Prettier leaves them alone.
- Out of scope (later milestones or non-goals, do not build): approving from Telegram (spec §1 non-goal); showing or revoking session allowances in the UI (they end with the session; deferred); the Health page's pending-approvals card (M8); the Playwright approval-card flow (M10, T10.2); gating swarm runs; skills and automations tools themselves (M5, M6; only their classes are listed now).

## Review Focus

1. **A prompt-injected command riding behind an allowed prefix** (`git status; rm -rf ~`, `git status && curl …`, `` git status `x` ``, `git status $(x)`, a newline) must still ask even when "Always allow `git status`" exists; a path rule must never match `notes/../secrets` or `/etc/passwd`; a domain rule for `example.com` must not match `badexample.com` or `example.com.evil.net`. Tests: Task 1 (matchers) and Task 5 (`a_rule_never_covers_a_command_with_shell_operators`).
2. **The owner clicks Allow just as the 30-minute timeout fires** (or two tabs decide at once): exactly one outcome is recorded, the tool runs only if Allow won, the loser gets 409, and one `approval.resolved` is published. Tests: Task 5 (`a_timeout_that_queued_first_beats_a_later_decision` and `a_decision_that_queued_first_beats_the_timeout`, which hold the control-plane transaction to force each order, and `repeating_a_decision_is_idempotent_and_a_different_one_conflicts`).
3. **Stop pressed while a call waits for approval**: the approval becomes `stopped` in the stop's own save, the call's result is `Cancelled before running (stopped by owner)`, the run ends `cancelled`, a later decision is 409, and a stop whose save fails leaves the approval waiting. Tests: Task 6.
4. **The daemon restarts while a call waits**: the approval is `expired` (resolved by `restart`), the run is `interrupted/restart_during_run`, the expired record reaches the history store, and a decision on it is 409. Tests: Task 2 (restore), Task 4 (flush), Task 8 (decision on an expired approval).
5. **The browser reconnects while an approval is pending**: the card comes back from `stream.snapshot` exactly once (no duplicate from a repeated `approval.requested`), disappears on `approval.resolved`, and a stale ledger read never puts a resumed run back to "awaiting approval". Tests: Task 7 (snapshot lists it), Task 10 (reducer and merge rank), Task 13 (ViewHarness).

## File map

| Area                | Files                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Daemon approvals    | `hosts/rust-daemon/src`: create `approvals/{mod.rs,policy.rs,registry.rs,gate.rs}`, `state/approval_state.rs`, `agent_runs/{approvals.rs,approval_tests.rs,approval_stop_tests.rs}`; modify `lib.rs`, `tools.rs`, `agent_runs.rs` (module lines, two coordinator fields, one builder call), `agent_runs/{stop.rs,stop_tests.rs,test_support.rs,live_tests.rs}`, `state.rs`, `state/{run_stop.rs,live_state.rs}`                                                                                                                                                                                                                                                                                |
| Daemon persistence  | modify `control_plane_store.rs`, `app/persistence.rs`, `sessions/mod.rs`, `history/{mod.rs,memory.rs,sqlite.rs,postgres.rs,conformance.rs,outbox.rs}`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Daemon events/reads | modify `live/{events.rs,tests.rs}`, `routes/events.rs`, `sessions/views.rs`, `routes/contracts/{mod.rs,sessions.rs}`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Daemon routes       | create `routes/approvals.rs`, `routes/contracts/approvals.rs`, `routes/tests/approvals.rs`; modify `routes/mod.rs`, `routes/tests/events.rs`, `hosts/rust-daemon/README.md`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| SDK                 | `packages/sdk/src`: create `approvals.ts`, `approvals.spec.ts`; modify `events.ts`, `events.spec.ts`, `client.ts`, `index.ts`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Web                 | `apps/web/src`: create `lib/{approvals.ts,approvals.test.ts}`, `components/sessions/{ApprovalCard.tsx,ApprovalCard.test.tsx}`, `hooks/{useApprovals.ts,useApprovals.test.tsx}`, `pages/{ApprovalsPage.tsx,ApprovalsPage.test.tsx}`, `test/approvals.ts`, `approvals.css`; modify `lib/{session-events.ts,session-events.test.ts,transcript.ts,transcript.test.ts,daemon-api.ts}`, `test/live.ts`, `components/sessions/{RunActivity.tsx,RunActivity.test.tsx,SessionSidebar.tsx,SessionSidebar.test.tsx}`, `hooks/{useTranscriptActions.ts,useTranscriptActions.test.tsx}`, `components/{WorkspaceShell.tsx,WorkspaceShell.test.tsx}`, `ViewHarness.tsx`, `ViewHarness.test.tsx`, `styles.css` |
| Docs                | `docs/superpowers/plans/2026-09-23-companion-console.md` (the M4 status row, Task 14, controller only)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |

## Task list

1. Risk table, matchers, and policy evaluation (T4.1)
2. Approval records in the control plane: registry, snapshot version 7, and restart expiry (T4.1)
3. Settling approvals: decisions, rules, session allowances, and the run's status (T4.1)
4. Decided approvals in the history store and the outbox (T4.2)
5. The gate in `execute_tool`: verdicts, the waiting call, decisions, timeouts, and helpers (T4.1)
6. Stop while awaiting approval, abandoned waits, and steers that wait (T4.1)
7. Approval reads: the stream snapshot, session `pendingApprovals`, and `GET /api/approvals` (T4.2)
8. Decision, policy, and rule routes (T4.2)
9. SDK approvals client and typed approval events (T4.3)
10. Web live state: pending approvals per run (T4.3)
11. Web inline approval cards (T4.3)
12. Web Approvals page: pending, rules and policy, decided history (T4.3)
13. Web shell: the Approvals destination, badges, ⌘K additions, and wiring (T4.3, carried ⌘K)
14. M4 verification

---

### Task 1: Risk table, matchers, and policy evaluation

**Files:**

- Create: `hosts/rust-daemon/src/approvals/mod.rs`, `hosts/rust-daemon/src/approvals/policy.rs`
- Modify: `hosts/rust-daemon/src/lib.rs` (`mod approvals;`)

**Interfaces:**

- Consumes: `anima_core::{DataValue, ToolCall}`; `crate::tools::ToolRegistry::new().tool_names() -> Vec<String>` (tests only); `reqwest::Url`.
- Produces (every later daemon task uses these names):
  - `approvals::{RiskClass, PolicyAction, ApprovalPolicy, MatcherKind, ApprovalMatcher, ApprovalRule, SessionAllowance}` (serde camelCase / snake_case enums; `RiskClass`, `PolicyAction`, `MatcherKind` also derive `utoipa::ToSchema`), each enum with `as_str(self) -> &'static str`.
  - `ApprovalPolicy::default()` = `{ write: Allow, exec: Ask, network: Allow, delegate: Allow }`; `ApprovalPolicy::action(&self, class: RiskClass) -> Option<PolicyAction>` (`None` for `Read`); `ApprovalPolicy::with(self, class: RiskClass, action: PolicyAction) -> ApprovalPolicy` (test-only builder; a no-op for `Read`).
  - `ApprovalMatcher::any() -> ApprovalMatcher` (`{ kind: Any, value: "" }`).
  - `policy::{risk_class(tool: &str) -> RiskClass, is_classified(tool: &str) -> bool (test-only), matcher_kinds(tool: &str) -> &'static [MatcherKind], matcher_matches(matcher: &ApprovalMatcher, call: &ToolCall) -> bool, command_has_prefix(command: &str, prefix: &str) -> bool, path_matches(glob: &str, path: &str) -> bool, url_in_domain(url: &str, domain: &str) -> bool, suggested_matcher(call: &ToolCall) -> ApprovalMatcher, validate_matcher(tool: &str, matcher: &ApprovalMatcher) -> Result<ApprovalMatcher, &'static str>, evaluate(policy: &ApprovalPolicy, rules: &[&ApprovalRule], allowances: &[SessionAllowance], call: &ToolCall) -> Verdict}` and `Verdict { Allow, Ask, Deny }`; `approvals` re-exports `evaluate`, `matcher_kinds`, `risk_class`, `suggested_matcher`, `validate_matcher`, and `Verdict`.
  - Constants: `DENIED_BY_POLICY`, `MATCHER_KIND_NOT_FOR_TOOL`, `MATCHER_VALUE_INVALID`, `MAX_MATCHER_VALUE_CHARS = 512`, `MAX_APPROVAL_RULES_PER_AGENT = 100`, `MAX_SESSION_ALLOWANCES = 50`.
- Behavior: the table is spec §7.1's, with every currently registered tool listed explicitly (a test fails when a new tool is registered without a class). Command-prefix matching compares whole words and never matches a command holding a shell operator (`; & | ` `` ` `` ` $ > < ( )`, a newline); path globs are workspace-relative (`*` and `?` within a component, `**` across components) and never match an absolute path or one with `..`; a domain matches its host and subdomains over `http`/`https` only. `validate_matcher` normalizes an owner's matcher (trimmed, whitespace collapsed, domains lowercased) or refuses it.

- [ ] **Step 1: Write the types and the failing tests**

Add to `hosts/rust-daemon/src/lib.rs`, after `mod app;`:

```text
mod approvals;
```

Create `hosts/rust-daemon/src/approvals/mod.rs`:

```rust
//! Per-tool approvals (spec §7): every tool's risk class, the owner's
//! policy and rules, and how a call is judged against them. Later M4 tasks
//! add the approval records (`registry`) and the gate in `execute_tool`
//! (`gate`).
#![allow(dead_code)] // M4 Task 8 removes this once the gate and the routes use every item.

pub(crate) mod policy;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[allow(unused_imports)] // M4 Tasks 3, 5, and 8 use the rest.
pub(crate) use policy::{
    evaluate, matcher_kinds, risk_class, suggested_matcher, validate_matcher, Verdict,
};

/// The tool result of a call the owner's policy denies (spec §7.2).
pub(crate) const DENIED_BY_POLICY: &str = "Denied by owner policy";
/// An owner-supplied matcher whose kind does not fit the tool.
pub(crate) const MATCHER_KIND_NOT_FOR_TOOL: &str = "This matcher kind does not apply to this tool";
/// An owner-supplied matcher value that could never match safely.
pub(crate) const MATCHER_VALUE_INVALID: &str = "matcher value is not valid for its kind";
/// A matcher value's length in characters (plan bound; spec §1 bounded growth).
pub(crate) const MAX_MATCHER_VALUE_CHARS: usize = 512;
/// Rules one companion may keep (plan bound).
pub(crate) const MAX_APPROVAL_RULES_PER_AGENT: usize = 100;
/// "Allow for this session" grants one session may keep (plan bound).
pub(crate) const MAX_SESSION_ALLOWANCES: usize = 50;

/// How much a tool can change (spec §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskClass {
    Read,
    Write,
    Exec,
    Network,
    Delegate,
}

impl RiskClass {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Exec => "exec",
            Self::Network => "network",
            Self::Delegate => "delegate",
        }
    }
}

/// What a class does under a policy (spec §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PolicyAction {
    Allow,
    Ask,
    Deny,
}

impl PolicyAction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

/// One agent's policy (spec §7.2). Read-class tools never ask, so there is
/// no `read` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalPolicy {
    pub(crate) write: PolicyAction,
    pub(crate) exec: PolicyAction,
    pub(crate) network: PolicyAction,
    pub(crate) delegate: PolicyAction,
}

impl Default for ApprovalPolicy {
    /// `exec: ask`, everything else `allow` (spec §7.2).
    fn default() -> Self {
        Self {
            write: PolicyAction::Allow,
            exec: PolicyAction::Ask,
            network: PolicyAction::Allow,
            delegate: PolicyAction::Allow,
        }
    }
}

impl ApprovalPolicy {
    /// What `class` does; `None` for read-class tools, which never ask.
    pub(crate) const fn action(&self, class: RiskClass) -> Option<PolicyAction> {
        match class {
            RiskClass::Read => None,
            RiskClass::Write => Some(self.write),
            RiskClass::Exec => Some(self.exec),
            RiskClass::Network => Some(self.network),
            RiskClass::Delegate => Some(self.delegate),
        }
    }

    /// This policy with `class` set to `action` (read-class tools are not
    /// set). The routes replace a policy whole; tests build them this way.
    #[cfg(test)]
    pub(crate) fn with(mut self, class: RiskClass, action: PolicyAction) -> Self {
        match class {
            RiskClass::Read => {}
            RiskClass::Write => self.write = action,
            RiskClass::Exec => self.exec = action,
            RiskClass::Network => self.network = action,
            RiskClass::Delegate => self.delegate = action,
        }
        self
    }
}

/// How a rule or allowance picks the calls it covers (spec §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatcherKind {
    CommandPrefix,
    PathGlob,
    Domain,
    Any,
}

impl MatcherKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CommandPrefix => "command_prefix",
            Self::PathGlob => "path_glob",
            Self::Domain => "domain",
            Self::Any => "any",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalMatcher {
    pub(crate) kind: MatcherKind,
    /// Empty for `any`.
    #[serde(default)]
    pub(crate) value: String,
}

impl ApprovalMatcher {
    /// Every call of the tool.
    pub(crate) fn any() -> Self {
        Self {
            kind: MatcherKind::Any,
            value: String::new(),
        }
    }
}

/// A standing "Always allow" (spec §7.2), managed on the Approvals page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRule {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) tool: String,
    pub(crate) matcher: ApprovalMatcher,
    pub(crate) created_at_ms: u64,
    /// The approval whose "Always allow" created it; `null` for a rule the
    /// owner added on the Approvals page.
    #[serde(default)]
    pub(crate) from_approval_id: Option<String>,
}

/// An "Allow for this session" grant (spec §3.2, §7.3), kept on its
/// session record and gone with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionAllowance {
    pub(crate) tool: String,
    pub(crate) matcher: ApprovalMatcher,
    pub(crate) created_at_ms: u64,
    pub(crate) from_approval_id: String,
}
```

Create `hosts/rust-daemon/src/approvals/policy.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anima_core::{DataValue, ToolCall};

    use super::*;
    use crate::approvals::{
        ApprovalMatcher, ApprovalPolicy, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
        SessionAllowance, MATCHER_KIND_NOT_FOR_TOOL, MATCHER_VALUE_INVALID,
    };

    fn call(name: &str, args: &[(&str, &str)]) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: name.into(),
            args: args
                .iter()
                .map(|(key, value)| (key.to_string(), DataValue::String(value.to_string())))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn matcher(kind: MatcherKind, value: &str) -> ApprovalMatcher {
        ApprovalMatcher {
            kind,
            value: value.into(),
        }
    }

    fn rule(tool: &str, matcher: ApprovalMatcher) -> ApprovalRule {
        ApprovalRule {
            id: "rule-1".into(),
            agent_id: "agent-1".into(),
            tool: tool.into(),
            matcher,
            created_at_ms: 1,
            from_approval_id: None,
        }
    }

    #[test]
    fn the_table_classes_the_tools_spec_7_1_lists() {
        for (class, tools) in [
            (
                RiskClass::Read,
                &[
                    "read_file",
                    "list_dir",
                    "glob",
                    "grep",
                    "todo_read",
                    "memory_search",
                    "recent_memories",
                    "get_current_time",
                    "calculate",
                    "bg_list",
                    "bg_output",
                    "list_workspace_agents",
                    "calendar_list_events",
                    "mail_list_messages",
                    "search_conversations",
                    "load_skill",
                    "list_automations",
                ][..],
            ),
            (
                RiskClass::Write,
                &[
                    "write_file",
                    "edit_file",
                    "multi_edit",
                    "todo_write",
                    "memory_add",
                    "propose_skill",
                    "create_automation",
                    "pause_automation",
                    "mail_create_draft",
                    "calendar_create_event",
                    "calendar_update_event",
                    "calendar_delete_event",
                ][..],
            ),
            (RiskClass::Exec, &["bash", "bg_start", "bg_stop"][..]),
            (RiskClass::Network, &["web_fetch", "exa_search"][..]),
            (
                RiskClass::Delegate,
                &[
                    "delegate_to_agent",
                    "spawn_helper",
                    "send_message",
                    "broadcast_message",
                ][..],
            ),
        ] {
            for tool in tools {
                assert_eq!(risk_class(tool), class, "{tool}");
                assert!(is_classified(tool), "{tool}");
            }
        }
    }

    #[test]
    fn an_unknown_tool_is_exec() {
        assert_eq!(risk_class("teleport"), RiskClass::Exec);
        assert!(!is_classified("teleport"));
    }

    #[test]
    fn every_registered_tool_has_an_explicit_class() {
        for tool in crate::tools::ToolRegistry::new().tool_names() {
            assert!(is_classified(&tool), "classify {tool} in approvals/policy.rs");
        }
    }

    #[test]
    fn no_tool_can_change_approval_state() {
        // Policies and rules change only through the owner-authorized
        // routes (spec §7.2); no tool may be named after them.
        for tool in crate::tools::ToolRegistry::new().tool_names() {
            assert!(
                !tool.contains("approval") && !tool.contains("policy"),
                "{tool} must not reach approval state"
            );
        }
    }

    #[test]
    fn the_default_policy_asks_only_before_exec() {
        let policy = ApprovalPolicy::default();
        assert_eq!(policy.action(RiskClass::Read), None);
        assert_eq!(policy.action(RiskClass::Write), Some(PolicyAction::Allow));
        assert_eq!(policy.action(RiskClass::Exec), Some(PolicyAction::Ask));
        assert_eq!(policy.action(RiskClass::Network), Some(PolicyAction::Allow));
        assert_eq!(policy.action(RiskClass::Delegate), Some(PolicyAction::Allow));
        assert_eq!(
            policy.with(RiskClass::Read, PolicyAction::Deny),
            policy,
            "read-class tools have no policy entry"
        );
    }

    #[test]
    fn evaluation_follows_deny_then_rules_then_ask() {
        let ask_all = ApprovalPolicy {
            write: PolicyAction::Ask,
            exec: PolicyAction::Ask,
            network: PolicyAction::Ask,
            delegate: PolicyAction::Ask,
        };
        let deny_all = ApprovalPolicy {
            write: PolicyAction::Deny,
            exec: PolicyAction::Deny,
            network: PolicyAction::Deny,
            delegate: PolicyAction::Deny,
        };
        let remember = call("memory_add", &[("content", "the plan")]);
        let covering = rule("memory_add", ApprovalMatcher::any());
        let allowance = SessionAllowance {
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: 1,
            from_approval_id: "apr_1".into(),
        };

        assert_eq!(evaluate(&ask_all, &[], &[], &remember), Verdict::Ask);
        assert_eq!(evaluate(&ask_all, &[&covering], &[], &remember), Verdict::Allow);
        assert_eq!(
            evaluate(&ask_all, &[], &[allowance.clone()], &remember),
            Verdict::Allow
        );
        assert_eq!(
            evaluate(&deny_all, &[&covering], &[allowance], &remember),
            Verdict::Deny,
            "a class deny beats every rule and allowance"
        );
        assert_eq!(
            evaluate(&ApprovalPolicy::default(), &[], &[], &remember),
            Verdict::Allow
        );
        assert_eq!(
            evaluate(&deny_all, &[], &[], &call("calculate", &[("expression", "1+1")])),
            Verdict::Allow,
            "read-class tools never ask and are never denied"
        );
        let other_tool = rule("todo_write", ApprovalMatcher::any());
        assert_eq!(
            evaluate(&ask_all, &[&other_tool], &[], &remember),
            Verdict::Ask,
            "a rule covers only its own tool"
        );
    }

    #[test]
    fn a_command_prefix_matches_whole_words_and_never_a_command_with_shell_operators() {
        for command in ["git status", "git  status --short", "  git status -sb  "] {
            assert!(command_has_prefix(command, "git status"), "{command}");
        }
        for command in [
            "git statusx",
            "git",
            "git status; rm -rf ~",
            "git status && curl example.com",
            "git status | sh",
            "git status || true",
            "git status $(whoami)",
            "git status `whoami`",
            "git status > out.txt",
            "git status < in.txt",
            "git status & sleep 1",
            "git status\nrm -rf ~",
            "(git status)",
        ] {
            assert!(!command_has_prefix(command, "git status"), "{command:?}");
        }
        assert!(!command_has_prefix("git status", "  "), "an empty prefix matches nothing");
    }

    #[test]
    fn a_path_glob_is_workspace_relative() {
        for (glob, path) in [
            ("notes/**", "notes/today.md"),
            ("notes/**", "notes/2026/today.md"),
            ("notes/**", "./notes/today.md"),
            ("src/*.rs", "src/main.rs"),
            ("docs/?.md", "docs/a.md"),
            ("**", "anything/at/all.txt"),
            ("README.md", "README.md"),
        ] {
            assert!(path_matches(glob, path), "{glob} ~ {path}");
        }
        for (glob, path) in [
            ("notes/**", "notes/../secrets.txt"),
            ("notes/**", "/etc/passwd"),
            ("notes/**", "other/today.md"),
            ("src/*.rs", "src/nested/main.rs"),
            ("docs/?.md", "docs/ab.md"),
            ("**", "/etc/passwd"),
            ("../**", "../outside.txt"),
            ("notes/**", "C:notes/today.md"),
        ] {
            assert!(!path_matches(glob, path), "{glob} !~ {path}");
        }
    }

    #[test]
    fn a_domain_covers_its_host_and_subdomains_over_http() {
        for url in [
            "https://example.com/page",
            "http://docs.example.com/a?b=c",
            "https://EXAMPLE.com",
        ] {
            assert!(url_in_domain(url, "example.com"), "{url}");
        }
        for url in [
            "https://badexample.com",
            "https://example.com.evil.net/",
            "ftp://example.com/file",
            "not a url",
        ] {
            assert!(!url_in_domain(url, "example.com"), "{url}");
        }
    }

    #[test]
    fn matchers_apply_only_to_the_tools_they_fit() {
        let bash = call("bash", &[("command", "npm test")]);
        assert!(matcher_matches(&matcher(MatcherKind::CommandPrefix, "npm"), &bash));
        assert!(matcher_matches(&ApprovalMatcher::any(), &bash));
        assert!(!matcher_matches(&matcher(MatcherKind::PathGlob, "**"), &bash));
        let write = call("write_file", &[("file_path", "notes/a.md"), ("content", "x")]);
        assert!(matcher_matches(&matcher(MatcherKind::PathGlob, "notes/**"), &write));
        let fetch = call("web_fetch", &[("url", "https://docs.rs/serde")]);
        assert!(matcher_matches(&matcher(MatcherKind::Domain, "docs.rs"), &fetch));
        assert_eq!(
            matcher_kinds("memory_add"),
            &[MatcherKind::Any][..],
            "other tools take only `any`"
        );
    }

    #[test]
    fn suggestions_are_narrow_enough_to_read_before_always_allowing() {
        assert_eq!(
            suggested_matcher(&call("bash", &[("command", "git status --short")])),
            matcher(MatcherKind::CommandPrefix, "git status")
        );
        assert_eq!(
            suggested_matcher(&call("bash", &[("command", "ls -la")])),
            matcher(MatcherKind::CommandPrefix, "ls")
        );
        assert_eq!(
            suggested_matcher(&call("bash", &[("command", "npm test; rm -rf ~")])),
            matcher(MatcherKind::CommandPrefix, "npm test")
        );
        assert_eq!(
            suggested_matcher(&call("write_file", &[("file_path", "notes/today.md")])),
            matcher(MatcherKind::PathGlob, "notes/**")
        );
        assert_eq!(
            suggested_matcher(&call("edit_file", &[("file_path", "README.md")])),
            matcher(MatcherKind::PathGlob, "README.md")
        );
        assert_eq!(
            suggested_matcher(&call("web_fetch", &[("url", "https://Docs.Example.com/a")])),
            matcher(MatcherKind::Domain, "docs.example.com")
        );
        assert_eq!(
            suggested_matcher(&call("memory_add", &[("content", "x")])),
            ApprovalMatcher::any()
        );
    }

    #[test]
    fn an_owner_matcher_is_normalized_or_refused() {
        assert_eq!(
            validate_matcher("bash", &matcher(MatcherKind::CommandPrefix, "  git   status ")),
            Ok(matcher(MatcherKind::CommandPrefix, "git status"))
        );
        assert_eq!(
            validate_matcher("web_fetch", &matcher(MatcherKind::Domain, "Docs.RS.")),
            Ok(matcher(MatcherKind::Domain, "docs.rs"))
        );
        assert_eq!(
            validate_matcher("bash", &matcher(MatcherKind::Any, "ignored")),
            Ok(ApprovalMatcher::any())
        );
        assert_eq!(
            validate_matcher("memory_add", &matcher(MatcherKind::PathGlob, "**")),
            Err(MATCHER_KIND_NOT_FOR_TOOL)
        );
        for (tool, refused) in [
            ("bash", matcher(MatcherKind::CommandPrefix, "git status; rm")),
            ("bash", matcher(MatcherKind::CommandPrefix, "   ")),
            ("write_file", matcher(MatcherKind::PathGlob, "../**")),
            ("write_file", matcher(MatcherKind::PathGlob, "/etc/*")),
            ("web_fetch", matcher(MatcherKind::Domain, "https://example.com")),
            ("web_fetch", matcher(MatcherKind::Domain, ".example.com")),
            ("bash", matcher(MatcherKind::CommandPrefix, &"x".repeat(513))),
        ] {
            assert_eq!(
                validate_matcher(tool, &refused),
                Err(MATCHER_VALUE_INVALID),
                "{refused:?}"
            );
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- approvals::policy`
Expected: FAIL to compile — `risk_class`, `is_classified`, `evaluate`, `Verdict`, and the matcher functions are not defined.

- [ ] **Step 3: Implement the table, the matchers, and the evaluation**

In `hosts/rust-daemon/src/approvals/policy.rs`, above the test module, add:

```rust
//! The static risk table (spec §7.1), the matchers of rules and session
//! allowances, and the evaluation order (spec §7.2). Everything here is
//! pure: callers pass in the policy, rules, and allowances they read.

use anima_core::{DataValue, ToolCall};

use super::{
    ApprovalMatcher, ApprovalPolicy, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
    SessionAllowance, MATCHER_KIND_NOT_FOR_TOOL, MATCHER_VALUE_INVALID, MAX_MATCHER_VALUE_CHARS,
};

/// Tools that only read (spec §7.1). `load_skill` and `list_automations`
/// arrive in M5 and M6.
const READ_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "todo_read",
    "memory_search",
    "recent_memories",
    "get_current_time",
    "calculate",
    "bg_list",
    "bg_output",
    "list_workspace_agents",
    "calendar_list_events",
    "mail_list_messages",
    "search_conversations",
    "load_skill",
    "list_automations",
];
/// Tools that change files and records (spec §7.1). The mail draft and the
/// calendar writes still only create records the owner approves in
/// Connectors.
const WRITE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "multi_edit",
    "todo_write",
    "memory_add",
    "propose_skill",
    "create_automation",
    "pause_automation",
    "mail_create_draft",
    "calendar_create_event",
    "calendar_update_event",
    "calendar_delete_event",
];
const EXEC_TOOLS: &[&str] = &["bash", "bg_start", "bg_stop"];
const NETWORK_TOOLS: &[&str] = &["web_fetch", "exa_search"];
const DELEGATE_TOOLS: &[&str] = &[
    "delegate_to_agent",
    "spawn_helper",
    "send_message",
    "broadcast_message",
];

/// Characters that chain, substitute, or redirect in a shell. A command
/// holding any of them never matches a command-prefix rule or allowance, so
/// "Always allow `git status`" cannot approve `git status; rm -rf ~`.
const SHELL_OPERATORS: &[char] = &[';', '&', '|', '`', '$', '>', '<', '(', ')', '\n', '\r'];

/// What a call needs before it runs (spec §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allow,
    Ask,
    Deny,
}

const TABLE: [(&[&str], RiskClass); 5] = [
    (READ_TOOLS, RiskClass::Read),
    (WRITE_TOOLS, RiskClass::Write),
    (EXEC_TOOLS, RiskClass::Exec),
    (NETWORK_TOOLS, RiskClass::Network),
    (DELEGATE_TOOLS, RiskClass::Delegate),
];

fn named_class(tool: &str) -> Option<RiskClass> {
    TABLE
        .iter()
        .find(|(tools, _)| tools.contains(&tool))
        .map(|(_, class)| *class)
}

/// A tool's class; one the table does not name is `exec` (spec §7.1).
pub(crate) fn risk_class(tool: &str) -> RiskClass {
    named_class(tool).unwrap_or(RiskClass::Exec)
}

/// Whether the table names `tool` itself rather than defaulting it; the
/// test that every registered tool is classified uses it.
#[cfg(test)]
pub(crate) fn is_classified(tool: &str) -> bool {
    named_class(tool).is_some()
}

/// The matcher kinds a rule or allowance for `tool` may use, the narrowest
/// first; `any` fits every tool.
pub(crate) fn matcher_kinds(tool: &str) -> &'static [MatcherKind] {
    match tool {
        "bash" | "bg_start" => &[MatcherKind::CommandPrefix, MatcherKind::Any],
        "write_file" | "edit_file" | "multi_edit" => &[MatcherKind::PathGlob, MatcherKind::Any],
        "web_fetch" => &[MatcherKind::Domain, MatcherKind::Any],
        _ => &[MatcherKind::Any],
    }
}

fn string_arg<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    match call.args.get(key) {
        Some(DataValue::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// Whether `matcher` covers `call`; a kind that does not fit the tool
/// covers nothing.
pub(crate) fn matcher_matches(matcher: &ApprovalMatcher, call: &ToolCall) -> bool {
    if !matcher_kinds(&call.name).contains(&matcher.kind) {
        return false;
    }
    match matcher.kind {
        MatcherKind::Any => true,
        MatcherKind::CommandPrefix => string_arg(call, "command")
            .is_some_and(|command| command_has_prefix(command, &matcher.value)),
        MatcherKind::PathGlob => {
            string_arg(call, "file_path").is_some_and(|path| path_matches(&matcher.value, path))
        }
        MatcherKind::Domain => {
            string_arg(call, "url").is_some_and(|url| url_in_domain(url, &matcher.value))
        }
    }
}

/// Whether `command`'s first words are `prefix`'s words, and `command`
/// holds no shell operator.
pub(crate) fn command_has_prefix(command: &str, prefix: &str) -> bool {
    if command.contains(SHELL_OPERATORS) {
        return false;
    }
    let prefix = prefix.split_whitespace().collect::<Vec<_>>();
    let words = command.split_whitespace().collect::<Vec<_>>();
    !prefix.is_empty() && words.len() >= prefix.len() && words[..prefix.len()] == prefix[..]
}

/// A workspace-relative path's components, or `None` for an absolute path,
/// one that climbs with `..`, or one naming a drive.
fn relative_components(path: &str) -> Option<Vec<&str>> {
    if path.starts_with(['/', '\\']) {
        return None;
    }
    let mut components = Vec::new();
    for component in path.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return None,
            component if component.contains(':') => return None,
            component => components.push(component),
        }
    }
    (!components.is_empty()).then_some(components)
}

/// Whether `path` matches `glob`: `*` and `?` within a component, `**`
/// across any number of components. Absolute and climbing paths never match.
pub(crate) fn path_matches(glob: &str, path: &str) -> bool {
    match (relative_components(glob), relative_components(path)) {
        (Some(pattern), Some(path)) => segments_match(&pattern, &path),
        _ => false,
    }
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if *first == "**" => {
            (0..=path.len()).any(|skip| segments_match(rest, &path[skip..]))
        }
        Some((first, rest)) => path.split_first().is_some_and(|(component, remaining)| {
            component_matches(first, component) && segments_match(rest, remaining)
        }),
    }
}

/// `*` matches any run of characters and `?` exactly one, within a component.
fn component_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let (mut at, mut read) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while read < text.len() {
        if at < pattern.len() && (pattern[at] == '?' || pattern[at] == text[read]) {
            at += 1;
            read += 1;
        } else if at < pattern.len() && pattern[at] == '*' {
            star = Some((at, read));
            at += 1;
        } else if let Some((star_at, star_read)) = star {
            at = star_at + 1;
            read = star_read + 1;
            star = Some((star_at, star_read + 1));
        } else {
            return false;
        }
    }
    pattern[at..].iter().all(|character| *character == '*')
}

fn http_host(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether `url` is an `http`/`https` URL on `domain` or a subdomain of it.
pub(crate) fn url_in_domain(url: &str, domain: &str) -> bool {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    !domain.is_empty()
        && http_host(url).is_some_and(|host| host == domain || host.ends_with(&format!(".{domain}")))
}

/// The command's first word, plus its second when that reads as a
/// subcommand (`git status`, `npm test`), cut at the first shell operator.
fn command_suggestion(command: &str) -> String {
    let head = command.split(SHELL_OPERATORS).next().unwrap_or_default();
    let mut words = head.split_whitespace();
    let mut prefix = words.next().unwrap_or_default().to_string();
    if let Some(second) = words.next() {
        let subcommand = second
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
            && second
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
        if subcommand {
            prefix.push(' ');
            prefix.push_str(second);
        }
    }
    prefix
}

/// The file's folder and everything under it, or the file itself at the root.
fn path_suggestion(path: &str) -> String {
    match relative_components(path) {
        Some(components) if components.len() > 1 => {
            format!("{}/**", components[..components.len() - 1].join("/"))
        }
        Some(components) => components[0].to_string(),
        None => path.trim().to_string(),
    }
}

/// What "Always allow" and "Allow for this session" cover unless the owner
/// edits it (spec §7.3 `suggestedMatcher`): the narrowest kind the tool
/// takes, filled from the call.
pub(crate) fn suggested_matcher(call: &ToolCall) -> ApprovalMatcher {
    let kind = matcher_kinds(&call.name)[0];
    let value = match kind {
        MatcherKind::CommandPrefix => string_arg(call, "command").map(command_suggestion),
        MatcherKind::PathGlob => string_arg(call, "file_path").map(path_suggestion),
        MatcherKind::Domain => string_arg(call, "url").and_then(http_host),
        MatcherKind::Any => None,
    };
    match value {
        Some(value) => ApprovalMatcher { kind, value },
        None => ApprovalMatcher::any(),
    }
}

fn is_domain(value: &str) -> bool {
    let value = value.trim_end_matches('.');
    !value.is_empty()
        && !value.starts_with('.')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
}

/// An owner's matcher for `tool`, normalized, or why it is refused.
pub(crate) fn validate_matcher(
    tool: &str,
    matcher: &ApprovalMatcher,
) -> Result<ApprovalMatcher, &'static str> {
    if !matcher_kinds(tool).contains(&matcher.kind) {
        return Err(MATCHER_KIND_NOT_FOR_TOOL);
    }
    let value = matcher.value.trim();
    let value = match matcher.kind {
        MatcherKind::Any => return Ok(ApprovalMatcher::any()),
        _ if value.is_empty() || value.chars().count() > MAX_MATCHER_VALUE_CHARS => {
            return Err(MATCHER_VALUE_INVALID)
        }
        MatcherKind::CommandPrefix if !value.contains(SHELL_OPERATORS) => {
            value.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        MatcherKind::PathGlob if relative_components(value).is_some() => value.to_string(),
        MatcherKind::Domain if is_domain(value) => value.trim_end_matches('.').to_ascii_lowercase(),
        _ => return Err(MATCHER_VALUE_INVALID),
    };
    Ok(ApprovalMatcher {
        kind: matcher.kind,
        value,
    })
}

/// Spec §7.2's order: a class `deny` is denied; a matching rule or session
/// allowance is allowed; a class `ask` asks; everything else is allowed.
/// Read-class tools are always allowed.
pub(crate) fn evaluate(
    policy: &ApprovalPolicy,
    rules: &[&ApprovalRule],
    allowances: &[SessionAllowance],
    call: &ToolCall,
) -> Verdict {
    let Some(action) = policy.action(risk_class(&call.name)) else {
        return Verdict::Allow;
    };
    if action == PolicyAction::Deny {
        return Verdict::Deny;
    }
    let covered = rules
        .iter()
        .any(|rule| rule.tool == call.name && matcher_matches(&rule.matcher, call))
        || allowances
            .iter()
            .any(|allowance| allowance.tool == call.name && matcher_matches(&allowance.matcher, call));
    if covered || action == PolicyAction::Allow {
        Verdict::Allow
    } else {
        Verdict::Ask
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- approvals::policy`
Expected: PASS — 12 tests.

- [ ] **Step 5: Format and commit**

Run: `cargo fmt --all && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- approvals::`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/lib.rs hosts/rust-daemon/src/approvals/mod.rs hosts/rust-daemon/src/approvals/policy.rs
git commit -m "feat(daemon): classify every tool by risk and judge calls against the owner's policy"
```

Recommended implementer tier: standard (pure code, but the matcher edge cases are security-relevant).

---

### Task 2: Approval records in the control plane: registry, snapshot version 7, and restart expiry

**Files:**

- Create: `hosts/rust-daemon/src/approvals/registry.rs`
- Modify: `hosts/rust-daemon/src/approvals/mod.rs` (module, re-exports, two constants)
- Modify: `hosts/rust-daemon/src/sessions/mod.rs` (`SessionRecord.session_allowances`)
- Modify: `hosts/rust-daemon/src/control_plane_store.rs` (three fields, version 7, `.pre-approvals.bak`)
- Modify: `hosts/rust-daemon/src/state.rs` (`DaemonState.approvals`; snapshot, restore, validation; version asserts)
- Modify: `hosts/rust-daemon/src/app/persistence.rs` (tests), `hosts/rust-daemon/src/agent_runs/live_tests.rs` (one assert)

**Interfaces:**

- Consumes: Task 1's types, `risk_class`, `suggested_matcher`; `crate::routes::data_value_to_json(&DataValue) -> serde_json::Value`; `RunLedger::restored` (M1; it already interrupts `awaiting_approval` runs as `restart_during_run`).
- Produces:
  - `approvals::{ApprovalStatus { Pending, Allowed, Denied, Stopped, Expired }, ApprovalDecisionKind { AllowOnce, AllowSession, AllowAlways, Deny } (also `ToSchema`), ResolvedBy { Owner, Timeout, Stop, Restart }}`, each with `as_str`.
  - `ApprovalResolution { decision: Option<ApprovalDecisionKind>, note: Option<String>, matcher: Option<ApprovalMatcher>, rule_id: Option<String>, resolved_by: ResolvedBy, resolved_at_ms: u64 }`.
  - `ApprovalRequest { id ("apr_<uuid-v4>"), agent_id, session_id, run_id, tool_call_id, tool, class: RiskClass, arguments: String, arguments_truncated: bool, suggested_matcher: ApprovalMatcher, created_at_ms, expires_at_ms, status: ApprovalStatus, revision: u64, resolution: Option<ApprovalResolution> }` with `ApprovalRequest::pending(start: PendingApprovalStart<'_>, now_ms: u64) -> Self`, `is_pending(&self) -> bool`, `resolve(&mut self, status: ApprovalStatus, resolution: ApprovalResolution)` (revision + 1), `was_decided_as(&self, kind: ApprovalDecisionKind) -> bool`; `PendingApprovalStart<'a> { agent_id: &'a str, session_id: &'a str, run_id: &'a str, call: &'a ToolCall, timeout_ms: u64 }`; `bounded_arguments(call: &ToolCall) -> (String, bool)`.
  - `AgentApprovalPolicy { agent_id: String, policy: ApprovalPolicy }`; `ApprovalSnapshot { approvals: Vec<ApprovalRequest>, policies: Vec<AgentApprovalPolicy>, rules: Vec<ApprovalRule> }`.
  - `ApprovalRegistry` (in `DaemonState.approvals`): `get`, `get_mut`, `insert`, `remove`, `pending() -> Vec<&ApprovalRequest>` (oldest first), `pending_ids_for_run(run_id) -> Vec<String>`, `pending_count_for_session(agent_id, session_id) -> usize`, `decided() -> Vec<&ApprovalRequest>` (newest first), `policy(agent_id) -> ApprovalPolicy`, `set_policy(agent_id, policy) -> Option<ApprovalPolicy>`, `restore_policy(agent_id, previous: Option<ApprovalPolicy>)`, `rules_for(agent_id) -> Vec<&ApprovalRule>` (oldest first), `find_rule(agent_id, tool, matcher) -> Option<&ApprovalRule>`, `add_rule(rule) -> Result<(), &'static str>` (`TOO_MANY_RULES` past 100 per agent), `remove_rule(agent_id, rule_id) -> Option<ApprovalRule>`, `retain_decided(keep: impl Fn(&ApprovalRequest) -> bool) -> usize`, `unmirrored_decided(limit) -> Vec<ApprovalRequest>` (oldest resolution first), `mark_mirrored(written: &[ApprovalRequest]) -> usize` (removes the unchanged decided ones), `snapshot(live_agents, keep) -> ApprovalSnapshot`, `validate(approvals, policies, rules) -> Result<(), String>`, `restored(snapshot: ApprovalSnapshot, live_agents, now_ms) -> Self` (pending → `expired` by `restart`).
  - `SessionRecord.session_allowances: Vec<SessionAllowance>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`, camelCase `sessionAllowances`).
  - `ControlPlaneSnapshot.{approvals, approval_policies, approval_rules}` (`approvals`, `approvalPolicies`, `approvalRules`); `CONTROL_PLANE_STORE_VERSION = 7`; `LIVE_RUNS_STORE_VERSION = 6`; `PRE_APPROVALS_BACKUP_SUFFIX = ".pre-approvals.bak"`; `pre_approvals_backup_path(path: &Path) -> PathBuf`; `pre_upgrade_backup_path` names a version-6 upgrade's backup `.pre-approvals.bak`.
  - `MAX_APPROVAL_ARGUMENTS_BYTES = 16 * 1024`, `TOO_MANY_RULES`.
- Behavior: decided approvals stay in the control plane only until the history store holds them (Task 4 removes them there); approvals, policies, and rules of deleted agents, and decided approvals of deleted sessions, are never saved. A restart turns every pending approval into `expired` (resolved by `restart`) next to the ledger's `restart_during_run` interruption (spec §4.8). The first start on a version-6 file writes `.pre-approvals.bak` before saving version 7.

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/approvals/registry.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use anima_core::{AgentConfig, AgentSettings, DataValue, ToolCall};

    use super::*;
    use crate::approvals::{
        ApprovalMatcher, ApprovalPolicy, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
        SessionAllowance, MAX_APPROVAL_ARGUMENTS_BYTES, MAX_APPROVAL_RULES_PER_AGENT,
        TOO_MANY_RULES,
    };
    use crate::runs::{RunRecord, RunSource, RunStart, RunStatus, RESTART_DURING_RUN};
    use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
    use crate::state::DaemonState;

    fn remember(text: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "memory_add".into(),
            args: BTreeMap::from([("content".to_string(), DataValue::String(text.into()))]),
        }
    }

    fn request(id: &str, agent: &str, session: &str, run: &str, at_ms: u64) -> ApprovalRequest {
        let call = remember("the plan");
        let mut approval = ApprovalRequest::pending(
            PendingApprovalStart {
                agent_id: agent,
                session_id: session,
                run_id: run,
                call: &call,
                timeout_ms: 1_000,
            },
            at_ms,
        );
        approval.id = id.into();
        approval
    }

    fn resolution(by: ResolvedBy, at_ms: u64) -> ApprovalResolution {
        ApprovalResolution {
            decision: Some(ApprovalDecisionKind::AllowOnce),
            note: None,
            matcher: None,
            rule_id: None,
            resolved_by: by,
            resolved_at_ms: at_ms,
        }
    }

    fn decided(id: &str, agent: &str, session: &str, at_ms: u64) -> ApprovalRequest {
        let mut approval = request(id, agent, session, "run_1", at_ms);
        approval.resolve(ApprovalStatus::Allowed, resolution(ResolvedBy::Owner, at_ms + 1));
        approval
    }

    fn rule(id: &str, agent: &str, at_ms: u64) -> ApprovalRule {
        ApprovalRule {
            id: id.into(),
            agent_id: agent.into(),
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: at_ms,
            from_approval_id: None,
        }
    }

    fn config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }

    #[test]
    fn a_pending_request_bounds_its_arguments_and_suggests_a_matcher() {
        let call = ToolCall {
            id: "call-7".into(),
            name: "bash".into(),
            args: BTreeMap::from([(
                "command".to_string(),
                DataValue::String(format!("git status {}", "é".repeat(12_000))),
            )]),
        };
        let approval = ApprovalRequest::pending(
            PendingApprovalStart {
                agent_id: "agent-1",
                session_id: "chat:a",
                run_id: "run_1",
                call: &call,
                timeout_ms: 60_000,
            },
            1_000,
        );

        assert!(approval.id.starts_with("apr_"));
        assert_eq!(approval.tool_call_id, "call-7");
        assert_eq!(approval.class, RiskClass::Exec);
        assert!(approval.arguments.len() <= MAX_APPROVAL_ARGUMENTS_BYTES);
        assert!(approval.arguments_truncated);
        assert!(approval.arguments.starts_with("{\"command\":\"git status "));
        assert_eq!(
            approval.suggested_matcher,
            ApprovalMatcher {
                kind: MatcherKind::CommandPrefix,
                value: "git status".into()
            }
        );
        assert_eq!(approval.expires_at_ms, 61_000);
        assert_eq!(
            (approval.status, approval.revision),
            (ApprovalStatus::Pending, 1)
        );
        let (small, cut) = bounded_arguments(&remember("short"));
        assert_eq!(small, "{\"content\":\"short\"}");
        assert!(!cut);
    }

    #[test]
    fn pending_approvals_are_listed_oldest_first_by_run_and_by_session() {
        let mut registry = ApprovalRegistry::default();
        registry.insert(request("apr_b", "agent-1", "chat:a", "run_1", 20));
        registry.insert(request("apr_a", "agent-1", "chat:a", "run_1", 10));
        registry.insert(request("apr_c", "agent-1", "chat:b", "run_2", 30));
        registry.insert(decided("apr_d", "agent-1", "chat:a", 5));

        let ids = registry
            .pending()
            .iter()
            .map(|approval| approval.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["apr_a", "apr_b", "apr_c"]);
        assert_eq!(registry.pending_ids_for_run("run_1"), ["apr_a", "apr_b"]);
        assert_eq!(registry.pending_count_for_session("agent-1", "chat:a"), 2);
        assert_eq!(registry.pending_count_for_session("agent-2", "chat:a"), 0);
        assert_eq!(registry.decided().len(), 1);
    }

    #[test]
    fn a_policy_defaults_to_asking_before_exec_and_can_be_put_back() {
        let mut registry = ApprovalRegistry::default();
        assert_eq!(registry.policy("agent-1"), ApprovalPolicy::default());
        let strict = ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask);
        assert_eq!(registry.set_policy("agent-1", strict), None);
        assert_eq!(registry.policy("agent-1"), strict);
        let previous = registry.set_policy("agent-1", ApprovalPolicy::default());
        registry.restore_policy("agent-1", previous);
        assert_eq!(registry.policy("agent-1"), strict);
        registry.restore_policy("agent-1", None);
        assert_eq!(registry.policy("agent-1"), ApprovalPolicy::default());
    }

    #[test]
    fn rules_are_per_agent_capped_and_found_by_tool_and_matcher() {
        let mut registry = ApprovalRegistry::default();
        for index in 0..MAX_APPROVAL_RULES_PER_AGENT {
            registry
                .add_rule(rule(&format!("rule_{index}"), "agent-1", index as u64))
                .unwrap();
        }
        assert_eq!(
            registry.add_rule(rule("rule_extra", "agent-1", 999)),
            Err(TOO_MANY_RULES)
        );
        registry.add_rule(rule("rule_other", "agent-2", 1)).unwrap();
        assert_eq!(registry.rules_for("agent-1").len(), MAX_APPROVAL_RULES_PER_AGENT);
        assert_eq!(registry.rules_for("agent-1")[0].id, "rule_0");
        assert!(registry
            .find_rule("agent-2", "memory_add", &ApprovalMatcher::any())
            .is_some());
        assert!(registry
            .find_rule("agent-2", "todo_write", &ApprovalMatcher::any())
            .is_none());
        assert!(
            registry.remove_rule("agent-1", "rule_other").is_none(),
            "a rule is removed only through its own agent"
        );
        assert!(registry.remove_rule("agent-2", "rule_other").is_some());
    }

    #[test]
    fn a_restart_expires_pending_approvals_and_drops_missing_agents() {
        let snapshot = ApprovalSnapshot {
            approvals: vec![
                request("apr_waiting", "agent-1", "chat:a", "run_1", 10),
                decided("apr_done", "agent-1", "chat:a", 5),
                request("apr_orphan", "agent-gone", "chat:a", "run_9", 10),
            ],
            policies: vec![
                AgentApprovalPolicy {
                    agent_id: "agent-1".into(),
                    policy: ApprovalPolicy::default().with(RiskClass::Network, PolicyAction::Deny),
                },
                AgentApprovalPolicy {
                    agent_id: "agent-gone".into(),
                    policy: ApprovalPolicy::default(),
                },
            ],
            rules: vec![rule("rule_1", "agent-1", 1), rule("rule_2", "agent-gone", 1)],
        };
        let live = HashSet::from(["agent-1".to_string()]);

        let registry = ApprovalRegistry::restored(snapshot, &live, 500);

        let expired = registry.get("apr_waiting").unwrap();
        assert_eq!(expired.status, ApprovalStatus::Expired);
        assert_eq!(expired.revision, 2);
        let resolution = expired.resolution.as_ref().unwrap();
        assert_eq!(
            (resolution.resolved_by, resolution.resolved_at_ms, resolution.decision),
            (ResolvedBy::Restart, 500, None)
        );
        assert_eq!(
            registry.get("apr_done").unwrap().status,
            ApprovalStatus::Allowed
        );
        assert!(registry.get("apr_orphan").is_none());
        assert!(registry.pending().is_empty());
        assert_eq!(registry.policy("agent-1").network, PolicyAction::Deny);
        assert_eq!(registry.policy("agent-gone"), ApprovalPolicy::default());
        assert_eq!(registry.rules_for("agent-1").len(), 1);
        assert!(registry.rules_for("agent-gone").is_empty());
    }

    #[test]
    fn mirroring_removes_only_unchanged_decided_approvals() {
        let mut registry = ApprovalRegistry::default();
        registry.insert(decided("apr_late", "agent-1", "chat:a", 30));
        registry.insert(decided("apr_early", "agent-1", "chat:a", 10));
        registry.insert(request("apr_waiting", "agent-1", "chat:a", "run_1", 20));
        let written = registry.unmirrored_decided(10);
        assert_eq!(
            written
                .iter()
                .map(|approval| approval.id.as_str())
                .collect::<Vec<_>>(),
            ["apr_early", "apr_late"],
            "oldest resolution first, never a pending one"
        );
        registry.get_mut("apr_late").unwrap().resolution.as_mut().unwrap().note =
            Some("changed since".into());

        assert_eq!(registry.mark_mirrored(&written), 1);
        assert!(registry.get("apr_early").is_none());
        assert!(registry.get("apr_late").is_some(), "a changed record is written again");
        assert!(registry.get("apr_waiting").is_some());
        assert_eq!(registry.unmirrored_decided(1).len(), 1);
        assert_eq!(registry.retain_decided(|_| false), 1);
        assert!(registry.get("apr_waiting").is_some(), "pending ones always stay");
    }

    #[test]
    fn validation_rejects_duplicate_and_empty_ids() {
        let one = request("apr_1", "agent-1", "chat:a", "run_1", 1);
        assert!(ApprovalRegistry::validate(&[one.clone(), one.clone()], &[], &[]).is_err());
        let mut blank = one.clone();
        blank.run_id = " ".into();
        assert!(ApprovalRegistry::validate(&[blank], &[], &[]).is_err());
        let policy = AgentApprovalPolicy {
            agent_id: "agent-1".into(),
            policy: ApprovalPolicy::default(),
        };
        assert!(ApprovalRegistry::validate(&[], &[policy.clone(), policy], &[]).is_err());
        let duplicate = rule("rule_1", "agent-1", 1);
        assert!(ApprovalRegistry::validate(&[], &[], &[duplicate.clone(), duplicate]).is_err());
        assert!(ApprovalRegistry::validate(&[one], &[], &[rule("rule_1", "agent-1", 1)]).is_ok());
    }

    #[test]
    fn approvals_policies_rules_and_allowances_survive_a_save_and_a_restart() {
        let mut source = DaemonState::new();
        let agent_id = source.create_agent(config("companion")).unwrap().state.id;
        let mut session = SessionRecord::new(
            &agent_id,
            "chat:kept",
            SessionKind::Chat,
            SessionOrigin::Web,
            "Kept".into(),
            TitleSource::Owner,
            1,
        );
        session.session_allowances.push(SessionAllowance {
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: 2,
            from_approval_id: "apr_session".into(),
        });
        source.sessions.insert(session);
        let mut awaiting = RunRecord::running(
            RunStart {
                agent_id: agent_id.clone(),
                session_id: "chat:kept".into(),
                source: RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "remember the plan".into(),
                model: "deterministic".into(),
                provider: None,
                parent_run_id: None,
            },
            1,
        );
        awaiting.status = RunStatus::AwaitingApproval;
        let run_id = awaiting.id.clone();
        source.runs.insert(awaiting);
        source
            .approvals
            .insert(request("apr_waiting", &agent_id, "chat:kept", &run_id, 3));
        source
            .approvals
            .insert(decided("apr_done", &agent_id, "chat:kept", 2));
        source
            .approvals
            .insert(decided("apr_deleted_session", &agent_id, "chat:gone", 2));
        source.approvals.set_policy(
            &agent_id,
            ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
        );
        source.approvals.add_rule(rule("rule_1", &agent_id, 4)).unwrap();

        let payload = serde_json::to_value(source.control_plane_snapshot()).unwrap();
        assert_eq!(payload["version"], 7);
        assert_eq!(payload["approvals"].as_array().unwrap().len(), 2);
        assert_eq!(payload["approvalPolicies"][0]["policy"]["write"], "ask");
        assert_eq!(payload["approvalRules"][0]["id"], "rule_1");
        assert_eq!(
            payload["sessions"][0]["sessionAllowances"][0]["fromApprovalId"],
            "apr_session"
        );

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(serde_json::from_value(payload).unwrap())
            .unwrap();
        let expired = restored.approvals.get("apr_waiting").unwrap();
        assert_eq!(expired.status, ApprovalStatus::Expired);
        assert_eq!(
            expired.resolution.as_ref().unwrap().resolved_by,
            ResolvedBy::Restart
        );
        let run = restored.runs.get(&run_id).unwrap();
        assert_eq!(run.status, RunStatus::Interrupted);
        assert_eq!(run.error.as_ref().unwrap().code, RESTART_DURING_RUN);
        assert!(restored.approvals.get("apr_done").is_some());
        assert!(restored.approvals.get("apr_deleted_session").is_none());
        assert_eq!(restored.approvals.policy(&agent_id).write, PolicyAction::Ask);
        assert_eq!(restored.approvals.rules_for(&agent_id).len(), 1);
        assert_eq!(
            restored
                .sessions
                .get(&agent_id, "chat:kept")
                .unwrap()
                .session_allowances
                .len(),
            1
        );
    }
}
```

In `hosts/rust-daemon/src/control_plane_store.rs`'s `tests` module:

1. In `snapshot_serializes_current_version_with_empty_connector_collections`, replace `assert_eq!(payload["version"], 6);` with the following, and add the three new keys after the `pendingHistoryDeletions` assertion:

```text
        assert_eq!(payload["version"], 7);
```

```text
        assert_eq!(payload["approvals"], serde_json::json!([]));
        assert_eq!(payload["approvalPolicies"], serde_json::json!([]));
        assert_eq!(payload["approvalRules"], serde_json::json!([]));
```

2. In `the_backup_is_named_by_the_version_it_upgrades_from`, before its closing brace, add:

```text
        assert_eq!(
            super::pre_upgrade_backup_path(path, 6),
            std::path::PathBuf::from("/data/control-plane.json.pre-approvals.bak")
        );
        assert_eq!(
            super::pre_approvals_backup_path(path),
            super::pre_upgrade_backup_path(path, 6)
        );
```

3. Add this test after `a_version_five_backup_leaves_the_pre_sessions_backup_alone`:

```rust
    #[tokio::test]
    async fn a_version_six_backup_leaves_the_earlier_backups_alone() {
        let path = test_snapshot_path("approvals-backup");
        let m3_backup = "{\"version\":5,\"agents\":[],\"swarms\":[]}";
        std::fs::write(super::pre_live_runs_backup_path(&path), m3_backup).unwrap();
        let original = "{\n  \"version\": 6,\n  \"agents\": [],\n  \"swarms\": []\n}\n";
        std::fs::write(&path, original).unwrap();
        let config = super::ControlPlaneStoreConfig::Json(path.clone());

        let location = super::write_pre_upgrade_backup(&config, 6).await.unwrap();

        let backup = super::pre_approvals_backup_path(&path);
        assert_eq!(location, backup.display().to_string());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(
            std::fs::read_to_string(super::pre_live_runs_backup_path(&path)).unwrap(),
            m3_backup,
            "the M3 upgrade's backup survives the M4 upgrade"
        );
        assert_no_temp_residue(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
```

In `hosts/rust-daemon/src/app/persistence.rs`'s `tests` module:

1. In `upgrading_any_pre_sessions_snapshot_writes_the_backup_before_saving_version_six`, rename the test to `upgrading_any_pre_sessions_snapshot_writes_the_backup_before_saving_the_current_version`, replace `assert_eq!(saved["version"], 6, "{version:?}");` with `assert_eq!(saved["version"], 7, "{version:?}");`, and after the `pre_live_runs_backup_path` assertion add:

```text
            assert!(
                !crate::control_plane_store::pre_approvals_backup_path(&path).exists(),
                "{version:?}: a pre-sessions snapshot writes only the pre-sessions backup"
            );
```

2. In `upgrading_a_version_five_snapshot_writes_the_live_runs_backup_and_loads_it`, replace `assert_eq!(saved["version"], 6);` with `assert_eq!(saved["version"], 7);`, and in its doc comment replace `before version 6 is saved` with `before the current version is saved`.
3. In `a_current_snapshot_loads_without_a_backup`, replace `"a fresh start saves a version-6 snapshot"` with `"a fresh start saves a version-7 snapshot"`, replace `assert_eq!(saved["version"], 6, "a version-6 snapshot writes no backup");` with `assert_eq!(saved["version"], 7, "a version-7 snapshot writes no backup");`, and after the `pre_live_runs_backup_path` assertion add `assert!(!crate::control_plane_store::pre_approvals_backup_path(&path).exists());`.
4. In `loading_a_current_snapshot_leaves_an_existing_pre_upgrade_backup_untouched`, replace `"a fresh start saves a version-6 snapshot"` with `"a fresh start saves a version-7 snapshot"` and `(v6)` in the comment above it with `(v7)`.
5. Add this test after `upgrading_a_version_five_snapshot_writes_the_live_runs_backup_and_loads_it`:

```rust
    /// M4: an M3 (version-6) snapshot is backed up as `.pre-approvals.bak`
    /// before version 7 is saved; the earlier upgrades' backups stay.
    #[tokio::test]
    async fn upgrading_a_version_six_snapshot_writes_the_approvals_backup_and_loads_it() {
        let dir = temp_dir("upgrade-approvals");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control-plane.json");
        let mut source = crate::state::DaemonState::new();
        let agent_id = source.create_agent(upgrader()).unwrap().state.id;
        let mut value = serde_json::to_value(source.control_plane_snapshot()).unwrap();
        value["version"] = 6.into();
        let object = value.as_object_mut().unwrap();
        for key in ["approvals", "approvalPolicies", "approvalRules"] {
            object.remove(key);
        }
        let original = serde_json::to_string_pretty(&value).unwrap();
        std::fs::write(&path, &original).unwrap();
        let m3_backup = older_snapshot_file(Some(5));
        let pre_live_runs = crate::control_plane_store::pre_live_runs_backup_path(&path);
        std::fs::write(&pre_live_runs, &m3_backup).unwrap();
        let state = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));

        configure_control_plane_store(&state, Some(ControlPlaneStoreConfig::Json(path.clone())))
            .await
            .unwrap();

        let backup = crate::control_plane_store::pre_approvals_backup_path(&path);
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            original,
            "the backup is the untouched version-6 original"
        );
        assert_eq!(
            std::fs::read_to_string(&pre_live_runs).unwrap(),
            m3_backup,
            "the M3 upgrade's backup is never overwritten"
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 7);
        assert_eq!(saved["approvalRules"], serde_json::json!([]));
        let guard = state.read().await;
        assert_eq!(guard.agent_count(), 1);
        assert_eq!(
            guard.approvals.policy(&agent_id),
            crate::approvals::ApprovalPolicy::default()
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(dir);
    }
```

In `hosts/rust-daemon/src/state.rs`'s `tests` module, replace the three `assert_eq!(snapshot.version, 6);` / `assert_eq!(loaded.version, 6);` assertions (in the ledger snapshot test near line 1173 and the stop round-trip test near lines 1355 and 1377) with `7`, and the message `"a v6 snapshot holding stopped/suppressed values restores"` with `"a v7 snapshot holding stopped/suppressed values restores"`.

In `hosts/rust-daemon/src/agent_runs/live_tests.rs`, in `the_version_six_snapshot_saves_reply_ids`, replace `assert_eq!(snapshot.version, 6);` with `assert_eq!(snapshot.version, 7);` and the comment line `// M2 daemon would load and silently drop it from; version 6 saves it.` with `// M2 daemon would load and silently drop it from; version 6 and later save it.`

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- approvals::registry control_plane_store:: app::persistence`
Expected: FAIL to compile — `ApprovalRequest`, `ApprovalRegistry`, `DaemonState.approvals`, `SessionRecord.session_allowances`, and `pre_approvals_backup_path` do not exist.

- [ ] **Step 3: Add the records and the registry**

In `hosts/rust-daemon/src/approvals/mod.rs`:

1. After `pub(crate) mod policy;` add `pub(crate) mod registry;`.
2. After the `policy` re-export add:

```text
#[allow(unused_imports)] // M4 Tasks 3–8 use the rest.
pub(crate) use registry::{
    AgentApprovalPolicy, ApprovalDecisionKind, ApprovalRegistry, ApprovalRequest,
    ApprovalResolution, ApprovalSnapshot, ApprovalStatus, PendingApprovalStart, ResolvedBy,
};
```

3. After `MAX_SESSION_ALLOWANCES` add:

```text
/// A request keeps at most this much of its call's arguments (spec §7.3, §16).
pub(crate) const MAX_APPROVAL_ARGUMENTS_BYTES: usize = 16 * 1024;
/// `ApprovalRegistry::add_rule` past `MAX_APPROVAL_RULES_PER_AGENT`.
pub(crate) const TOO_MANY_RULES: &str =
    "This companion already has 100 approval rules; remove one first";
```

In `hosts/rust-daemon/src/approvals/registry.rs`, above the test module, add:

```rust
//! The approval records the control plane keeps (spec §7.3, §13.1): every
//! pending request, decided ones until the history store holds them, and
//! each agent's policy and rules.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use anima_core::{DataValue, ToolCall};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    risk_class, suggested_matcher, ApprovalMatcher, ApprovalPolicy, ApprovalRule, RiskClass,
    MAX_APPROVAL_ARGUMENTS_BYTES, MAX_APPROVAL_RULES_PER_AGENT, TOO_MANY_RULES,
};

/// Where an approval stands (spec §7.3). A timeout is `denied` by
/// `timeout`; stopping the run resolves it `stopped`; a restart `expired`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalStatus {
    Pending,
    Allowed,
    Denied,
    Stopped,
    Expired,
}

impl ApprovalStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Allowed => "allowed",
            Self::Denied => "denied",
            Self::Stopped => "stopped",
            Self::Expired => "expired",
        }
    }
}

/// The owner's four decisions (spec §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalDecisionKind {
    AllowOnce,
    AllowSession,
    AllowAlways,
    Deny,
}

impl ApprovalDecisionKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow_once",
            Self::AllowSession => "allow_session",
            Self::AllowAlways => "allow_always",
            Self::Deny => "deny",
        }
    }
}

/// Who settled an approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolvedBy {
    Owner,
    Timeout,
    Stop,
    Restart,
}

impl ResolvedBy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Timeout => "timeout",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalResolution {
    /// `None` for a stopped or expired request.
    #[serde(default)]
    pub(crate) decision: Option<ApprovalDecisionKind>,
    #[serde(default)]
    pub(crate) note: Option<String>,
    /// The allowance's or rule's matcher, for `allow_session` and `allow_always`.
    #[serde(default)]
    pub(crate) matcher: Option<ApprovalMatcher>,
    /// The rule `allow_always` created or found.
    #[serde(default)]
    pub(crate) rule_id: Option<String>,
    pub(crate) resolved_by: ResolvedBy,
    pub(crate) resolved_at_ms: u64,
}

/// One call waiting for, or decided by, the owner (spec §7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRequest {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool: String,
    pub(crate) class: RiskClass,
    /// The call's arguments as JSON text, cut to 16 KiB. The model wrote
    /// them: they are untrusted text to the owner.
    pub(crate) arguments: String,
    #[serde(default)]
    pub(crate) arguments_truncated: bool,
    pub(crate) suggested_matcher: ApprovalMatcher,
    pub(crate) created_at_ms: u64,
    pub(crate) expires_at_ms: u64,
    pub(crate) status: ApprovalStatus,
    /// 1 while pending; a resolution adds 1. A decision must carry it.
    pub(crate) revision: u64,
    #[serde(default)]
    pub(crate) resolution: Option<ApprovalResolution>,
}

/// What a new request is about.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingApprovalStart<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) call: &'a ToolCall,
    pub(crate) timeout_ms: u64,
}

/// `call`'s arguments as JSON text, cut to `MAX_APPROVAL_ARGUMENTS_BYTES` on
/// a char boundary, and whether they were cut.
pub(crate) fn bounded_arguments(call: &ToolCall) -> (String, bool) {
    let text = crate::routes::data_value_to_json(&DataValue::Object(call.args.clone())).to_string();
    if text.len() <= MAX_APPROVAL_ARGUMENTS_BYTES {
        return (text, false);
    }
    let mut end = MAX_APPROVAL_ARGUMENTS_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

impl ApprovalRequest {
    pub(crate) fn pending(start: PendingApprovalStart<'_>, now_ms: u64) -> Self {
        let (arguments, arguments_truncated) = bounded_arguments(start.call);
        Self {
            id: format!("apr_{}", uuid::Uuid::new_v4()),
            agent_id: start.agent_id.to_string(),
            session_id: start.session_id.to_string(),
            run_id: start.run_id.to_string(),
            tool_call_id: start.call.id.clone(),
            tool: start.call.name.clone(),
            class: risk_class(&start.call.name),
            arguments,
            arguments_truncated,
            suggested_matcher: suggested_matcher(start.call),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(start.timeout_ms),
            status: ApprovalStatus::Pending,
            revision: 1,
            resolution: None,
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.status == ApprovalStatus::Pending
    }

    /// Resolves this request; its revision moves on.
    pub(crate) fn resolve(&mut self, status: ApprovalStatus, resolution: ApprovalResolution) {
        self.status = status;
        self.resolution = Some(resolution);
        self.revision += 1;
    }

    /// Whether the owner decided it as `kind` (an idempotent replay).
    pub(crate) fn was_decided_as(&self, kind: ApprovalDecisionKind) -> bool {
        self.resolution.as_ref().is_some_and(|resolution| {
            resolution.resolved_by == ResolvedBy::Owner && resolution.decision == Some(kind)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentApprovalPolicy {
    pub(crate) agent_id: String,
    pub(crate) policy: ApprovalPolicy,
}

/// The registry as the control-plane snapshot stores it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ApprovalSnapshot {
    pub(crate) approvals: Vec<ApprovalRequest>,
    pub(crate) policies: Vec<AgentApprovalPolicy>,
    pub(crate) rules: Vec<ApprovalRule>,
}

fn oldest_first(left: &&ApprovalRequest, right: &&ApprovalRequest) -> Ordering {
    (left.created_at_ms, &left.id).cmp(&(right.created_at_ms, &right.id))
}

fn resolved_at(approval: &ApprovalRequest) -> u64 {
    approval
        .resolution
        .as_ref()
        .map_or(approval.created_at_ms, |resolution| resolution.resolved_at_ms)
}

/// Pending approvals, decided ones not yet in the history store, and each
/// agent's policy and rules (spec §2's control plane).
#[derive(Clone, Debug, Default)]
pub(crate) struct ApprovalRegistry {
    approvals: HashMap<String, ApprovalRequest>,
    policies: HashMap<String, ApprovalPolicy>,
    rules: HashMap<String, ApprovalRule>,
}

impl ApprovalRegistry {
    pub(crate) fn get(&self, id: &str) -> Option<&ApprovalRequest> {
        self.approvals.get(id)
    }

    pub(crate) fn get_mut(&mut self, id: &str) -> Option<&mut ApprovalRequest> {
        self.approvals.get_mut(id)
    }

    pub(crate) fn insert(&mut self, approval: ApprovalRequest) {
        self.approvals.insert(approval.id.clone(), approval);
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<ApprovalRequest> {
        self.approvals.remove(id)
    }

    /// Pending approvals, oldest first.
    pub(crate) fn pending(&self) -> Vec<&ApprovalRequest> {
        let mut pending = self
            .approvals
            .values()
            .filter(|approval| approval.is_pending())
            .collect::<Vec<_>>();
        pending.sort_by(oldest_first);
        pending
    }

    /// The pending approvals of one run, oldest first.
    pub(crate) fn pending_ids_for_run(&self, run_id: &str) -> Vec<String> {
        self.pending()
            .into_iter()
            .filter(|approval| approval.run_id == run_id)
            .map(|approval| approval.id.clone())
            .collect()
    }

    pub(crate) fn pending_count_for_session(&self, agent_id: &str, session_id: &str) -> usize {
        self.approvals
            .values()
            .filter(|approval| {
                approval.is_pending()
                    && approval.agent_id == agent_id
                    && approval.session_id == session_id
            })
            .count()
    }

    /// Decided approvals the control plane still holds, newest first.
    pub(crate) fn decided(&self) -> Vec<&ApprovalRequest> {
        let mut decided = self
            .approvals
            .values()
            .filter(|approval| !approval.is_pending())
            .collect::<Vec<_>>();
        decided.sort_by(|left, right| oldest_first(right, left));
        decided
    }

    /// `agent_id`'s policy, or the default (spec §7.2).
    pub(crate) fn policy(&self, agent_id: &str) -> ApprovalPolicy {
        self.policies.get(agent_id).copied().unwrap_or_default()
    }

    /// Replaces `agent_id`'s policy; returns the stored one it replaced.
    pub(crate) fn set_policy(
        &mut self,
        agent_id: &str,
        policy: ApprovalPolicy,
    ) -> Option<ApprovalPolicy> {
        self.policies.insert(agent_id.to_string(), policy)
    }

    /// Puts back what `set_policy` replaced, after its save failed.
    pub(crate) fn restore_policy(&mut self, agent_id: &str, previous: Option<ApprovalPolicy>) {
        match previous {
            Some(policy) => {
                self.policies.insert(agent_id.to_string(), policy);
            }
            None => {
                self.policies.remove(agent_id);
            }
        }
    }

    /// `agent_id`'s rules, oldest first.
    pub(crate) fn rules_for(&self, agent_id: &str) -> Vec<&ApprovalRule> {
        let mut rules = self
            .rules
            .values()
            .filter(|rule| rule.agent_id == agent_id)
            .collect::<Vec<_>>();
        rules.sort_by(|left, right| {
            (left.created_at_ms, &left.id).cmp(&(right.created_at_ms, &right.id))
        });
        rules
    }

    /// `agent_id`'s rule for exactly this tool and matcher.
    pub(crate) fn find_rule(
        &self,
        agent_id: &str,
        tool: &str,
        matcher: &ApprovalMatcher,
    ) -> Option<&ApprovalRule> {
        self.rules.values().find(|rule| {
            rule.agent_id == agent_id && rule.tool == tool && rule.matcher == *matcher
        })
    }

    pub(crate) fn add_rule(&mut self, rule: ApprovalRule) -> Result<(), &'static str> {
        let held = self
            .rules
            .values()
            .filter(|existing| existing.agent_id == rule.agent_id)
            .count();
        if held >= MAX_APPROVAL_RULES_PER_AGENT {
            return Err(TOO_MANY_RULES);
        }
        self.rules.insert(rule.id.clone(), rule);
        Ok(())
    }

    /// Removes `rule_id` if it is `agent_id`'s.
    pub(crate) fn remove_rule(&mut self, agent_id: &str, rule_id: &str) -> Option<ApprovalRule> {
        if self
            .rules
            .get(rule_id)
            .is_some_and(|rule| rule.agent_id == agent_id)
        {
            self.rules.remove(rule_id)
        } else {
            None
        }
    }

    /// Drops the decided approvals `keep` refuses (those of deleted agents
    /// or sessions); pending ones always stay. Returns how many went.
    pub(crate) fn retain_decided(&mut self, keep: impl Fn(&ApprovalRequest) -> bool) -> usize {
        let before = self.approvals.len();
        self.approvals
            .retain(|_, approval| approval.is_pending() || keep(approval));
        before - self.approvals.len()
    }

    /// Decided approvals for the history store, the `limit` oldest
    /// resolutions first.
    pub(crate) fn unmirrored_decided(&self, limit: usize) -> Vec<ApprovalRequest> {
        let mut decided = self
            .approvals
            .values()
            .filter(|approval| !approval.is_pending())
            .collect::<Vec<_>>();
        decided.sort_by(|left, right| {
            (resolved_at(left), &left.id).cmp(&(resolved_at(right), &right.id))
        });
        decided.into_iter().take(limit).cloned().collect()
    }

    /// Removes each written approval the registry still holds unchanged: the
    /// history store holds it now (spec §7.3 "moves the record"). A record
    /// that changed since is written again by the next flush.
    pub(crate) fn mark_mirrored(&mut self, written: &[ApprovalRequest]) -> usize {
        let mut removed = 0;
        for approval in written {
            if self
                .approvals
                .get(&approval.id)
                .is_some_and(|current| !current.is_pending() && current == approval)
            {
                self.approvals.remove(&approval.id);
                removed += 1;
            }
        }
        removed
    }

    /// What the control plane saves: nothing of agents that no longer exist,
    /// and no decided approval `keep` refuses.
    pub(crate) fn snapshot(
        &self,
        live_agents: &HashSet<String>,
        keep: impl Fn(&ApprovalRequest) -> bool,
    ) -> ApprovalSnapshot {
        let mut approvals = self
            .approvals
            .values()
            .filter(|approval| {
                live_agents.contains(&approval.agent_id) && (approval.is_pending() || keep(approval))
            })
            .collect::<Vec<_>>();
        approvals.sort_by(oldest_first);
        let mut policies = self
            .policies
            .iter()
            .filter(|(agent_id, _)| live_agents.contains(*agent_id))
            .map(|(agent_id, policy)| AgentApprovalPolicy {
                agent_id: agent_id.clone(),
                policy: *policy,
            })
            .collect::<Vec<_>>();
        policies.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        let mut rules = self
            .rules
            .values()
            .filter(|rule| live_agents.contains(&rule.agent_id))
            .cloned()
            .collect::<Vec<_>>();
        rules.sort_by(|left, right| {
            (&left.agent_id, left.created_at_ms, &left.id)
                .cmp(&(&right.agent_id, right.created_at_ms, &right.id))
        });
        ApprovalSnapshot {
            approvals: approvals.into_iter().cloned().collect(),
            policies,
            rules,
        }
    }

    pub(crate) fn validate(
        approvals: &[ApprovalRequest],
        policies: &[AgentApprovalPolicy],
        rules: &[ApprovalRule],
    ) -> Result<(), String> {
        let mut ids = HashSet::new();
        for approval in approvals {
            if approval.id.trim().is_empty() || !ids.insert(approval.id.as_str()) {
                return Err(format!(
                    "duplicate or empty approval id in snapshot: {}",
                    approval.id
                ));
            }
            if approval.agent_id.trim().is_empty()
                || approval.session_id.trim().is_empty()
                || approval.run_id.trim().is_empty()
            {
                return Err(format!(
                    "approval '{}' has an empty agent, session, or run id",
                    approval.id
                ));
            }
        }
        let mut agents = HashSet::new();
        for entry in policies {
            if entry.agent_id.trim().is_empty() || !agents.insert(entry.agent_id.as_str()) {
                return Err(format!(
                    "duplicate or empty approval policy agent in snapshot: {}",
                    entry.agent_id
                ));
            }
        }
        let mut rule_ids = HashSet::new();
        for rule in rules {
            if rule.id.trim().is_empty()
                || rule.agent_id.trim().is_empty()
                || !rule_ids.insert(rule.id.as_str())
            {
                return Err(format!(
                    "duplicate or empty approval rule id in snapshot: {}",
                    rule.id
                ));
            }
        }
        Ok(())
    }

    /// The registry after a restart (spec §4.8): every pending approval is
    /// `expired`, since no call waits on it any more, and the records of
    /// agents that no longer exist are dropped.
    pub(crate) fn restored(
        snapshot: ApprovalSnapshot,
        live_agents: &HashSet<String>,
        now_ms: u64,
    ) -> Self {
        let mut registry = Self::default();
        for mut approval in snapshot.approvals {
            if !live_agents.contains(&approval.agent_id) {
                continue;
            }
            if approval.is_pending() {
                approval.resolve(
                    ApprovalStatus::Expired,
                    ApprovalResolution {
                        decision: None,
                        note: None,
                        matcher: None,
                        rule_id: None,
                        resolved_by: ResolvedBy::Restart,
                        resolved_at_ms: now_ms,
                    },
                );
            }
            registry.insert(approval);
        }
        for entry in snapshot.policies {
            if live_agents.contains(&entry.agent_id) {
                registry.policies.insert(entry.agent_id, entry.policy);
            }
        }
        for rule in snapshot.rules {
            if live_agents.contains(&rule.agent_id) {
                registry.rules.insert(rule.id.clone(), rule);
            }
        }
        registry
    }
}
```

- [ ] **Step 4: Keep the records in the control plane**

In `hosts/rust-daemon/src/sessions/mod.rs`, add to `SessionRecord` after `room_id`:

```text
    /// "Allow for this session" grants (spec §3.2, §7.3), at most
    /// `approvals::MAX_SESSION_ALLOWANCES`; they end with the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) session_allowances: Vec<crate::approvals::SessionAllowance>,
```

and to the struct literal in `SessionRecord::new`, after `room_id: room,`: `session_allowances: Vec::new(),`.

In `hosts/rust-daemon/src/control_plane_store.rs`:

1. Replace the version doc comment and constant

```text
/// Snapshot format version. Version 5 adds sessions (companion console M2);
/// version 6 adds the live-run fields (M3: accepted `queued` runs and each
/// run's `replyMessageId`). Older daemons refuse a newer version, so the first
/// start of a new version writes a backup (spec §13.3).
pub(crate) const CONTROL_PLANE_STORE_VERSION: u32 = 6;
```

with

```text
/// Snapshot format version. Version 5 adds sessions (companion console M2);
/// version 6 adds the live-run fields (M3: accepted `queued` runs and each
/// run's `replyMessageId`); version 7 adds approvals, approval policies and
/// rules, and session allowances (M4). Older daemons refuse a newer version,
/// so the first start of a new version writes a backup (spec §13.3).
pub(crate) const CONTROL_PLANE_STORE_VERSION: u32 = 7;
/// The version that added the live-run fields: older snapshots are backed
/// up as `.pre-live-runs.bak`, this one as `.pre-approvals.bak`.
pub(crate) const LIVE_RUNS_STORE_VERSION: u32 = 6;
```

2. After `PRE_LIVE_RUNS_BACKUP_SUFFIX` add:

```text
/// Suffix of the JSON backup taken before the approvals upgrade.
pub(crate) const PRE_APPROVALS_BACKUP_SUFFIX: &str = ".pre-approvals.bak";
```

3. In `ControlPlaneSnapshot`, after `pending_history_deletions`, add:

```text
    /// Pending approvals and decided ones the history store does not hold
    /// yet (spec §7.3, §13.1).
    #[serde(default)]
    pub(crate) approvals: Vec<crate::approvals::ApprovalRequest>,
    #[serde(default)]
    pub(crate) approval_policies: Vec<crate::approvals::AgentApprovalPolicy>,
    #[serde(default)]
    pub(crate) approval_rules: Vec<crate::approvals::ApprovalRule>,
```

and to the struct literal in `with_connector_state_and_cleanup`, after `pending_history_deletions: vec![],`:

```text
            approvals: vec![],
            approval_policies: vec![],
            approval_rules: vec![],
```

4. After `pre_live_runs_backup_path` add:

```rust
/// Where the JSON snapshot is backed up before the approvals upgrade (from
/// version `LIVE_RUNS_STORE_VERSION`).
pub(crate) fn pre_approvals_backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(PRE_APPROVALS_BACKUP_SUFFIX);
    path.with_file_name(name)
}
```

5. Replace the body of `pre_upgrade_backup_path` with:

```text
    if loaded_version < SESSIONS_STORE_VERSION {
        pre_sessions_backup_path(path)
    } else if loaded_version < LIVE_RUNS_STORE_VERSION {
        pre_live_runs_backup_path(path)
    } else {
        pre_approvals_backup_path(path)
    }
```

In `hosts/rust-daemon/src/state.rs`:

1. In `DaemonState`, after `pub(crate) sessions: crate::sessions::SessionRegistry,` add:

```text
    /// Pending and not-yet-mirrored approvals, policies, and rules (spec §7).
    pub(crate) approvals: crate::approvals::ApprovalRegistry,
```

and in `with_model_adapter_and_events_and_limits`, after `sessions: crate::sessions::SessionRegistry::default(),`: `approvals: crate::approvals::ApprovalRegistry::default(),`.

2. In `control_plane_snapshot`, before `snapshot` is returned, add:

```text
        // A decided approval of a deleted session is never saved again:
        // its history went with the session (Task 4).
        let approvals = self.approvals.snapshot(&self.live_agent_ids(), |approval| {
            self.sessions
                .contains(&approval.agent_id, &approval.session_id)
        });
        snapshot.approvals = approvals.approvals;
        snapshot.approval_policies = approvals.policies;
        snapshot.approval_rules = approvals.rules;
```

3. In `restore_control_plane_snapshot`, right after the `self.runs = crate::runs::RunLedger::restored(...)` statement, add:

```text
        // Spec §4.8: the runs that waited are interrupted above; the
        // approvals they waited on expire next to them.
        self.approvals = crate::approvals::ApprovalRegistry::restored(
            crate::approvals::ApprovalSnapshot {
                approvals: snapshot.approvals,
                policies: snapshot.approval_policies,
                rules: snapshot.approval_rules,
            },
            &self.live_agent_ids(),
            anima_core::primitives::now_millis(),
        );
```

4. In `validate_control_plane_snapshot`, after `crate::sessions::SessionRegistry::validate(&snapshot.sessions)?;` add:

```text
        crate::approvals::ApprovalRegistry::validate(
            &snapshot.approvals,
            &snapshot.approval_policies,
            &snapshot.approval_rules,
        )?;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- approvals:: control_plane_store:: app::persistence state::tests agent_runs::live_tests sessions::`
Expected: PASS — the 8 registry tests, the control-plane store and persistence tests (including the two new backup tests), and the existing state, live, and sessions tests.

- [ ] **Step 6: Format and commit**

Run: `cargo fmt --all && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- approvals:: control_plane_store:: app::persistence`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/approvals/mod.rs hosts/rust-daemon/src/approvals/registry.rs hosts/rust-daemon/src/sessions/mod.rs hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/app/persistence.rs hosts/rust-daemon/src/agent_runs/live_tests.rs
git commit -m "feat(daemon): keep approvals, policies, rules, and session allowances in a version-7 control plane"
```

Recommended implementer tier: standard (mechanical snapshot plumbing plus a registry with clear unit tests).

---

### Task 3: Settling approvals: decisions, rules, session allowances, and the run's status

**Files:**

- Create: `hosts/rust-daemon/src/state/approval_state.rs`
- Modify: `hosts/rust-daemon/src/state.rs` (`mod approval_state;` and its re-exports)
- Modify: `hosts/rust-daemon/src/approvals/mod.rs` (six constants)

**Interfaces:**

- Consumes: Task 1 `evaluate`, `validate_matcher`, `Verdict`; Task 2 `ApprovalRequest::{pending, resolve, is_pending}`, `ApprovalRegistry`, `SessionRecord.session_allowances`; `crate::agent_runs::{config_helper_parent(config: &AgentConfig) -> Option<&str>, is_helper_config(config: &AgentConfig) -> bool}`; `DaemonState::with_live_tools(record) -> RunRecord` (M3); `RunLedger::get_mut`; `SessionRegistry::get_mut(agent_id, session_id)`.
- Produces (`ApprovalAsk`, `ApprovalUndo`, `OwnerDecision`, `SettleRefusal`, and `Settlement` are re-exported from `crate::state`; the other types stay in `state::approval_state`, named only there):
  - `ApprovalAsk { run: RunLink, call: ToolCall, timeout_ms: u64 }`; `OpenedApproval { approval: ApprovalRequest, run: Option<RunRecord> }` (`run` is set when the request moved the run to `awaiting_approval`); `OpenRefusal { Stopped, Unavailable }` with `result(self) -> TaskResult<Content>` (`CANCELLED_TOOL_RESULT` / `APPROVAL_UNAVAILABLE`).
  - `OwnerDecision { kind: ApprovalDecisionKind, note: Option<String>, matcher: Option<ApprovalMatcher>, revision: u64 }`; `Settlement { Owner(OwnerDecision), TimedOut, Stopped }`; `SettleRefusal { NotFound, Resolved(ApprovalRequest), Stale, Invalid(&'static str), Conflict(&'static str) }`; `SettledApproval { approval: ApprovalRequest, run: Option<RunRecord>, undo: ApprovalUndo }` (`run` is set when the settlement moved the run back to `running`); `ApprovalUndo` (opaque).
  - `DaemonState::approval_verdict(&self, agent: &AgentState, session_id: &str, call: &ToolCall) -> Verdict`, `open_approval(&mut self, ask: &ApprovalAsk, now_ms: u64) -> Result<OpenedApproval, OpenRefusal>`, `revert_open_approval(&mut self, opened: &OpenedApproval)`, `settle_approval(&mut self, id: &str, settlement: Settlement, now_ms: u64) -> Result<SettledApproval, SettleRefusal>`, `revert_settled_approval(&mut self, undo: ApprovalUndo)`.
  - Constants in `approvals`: `MAX_APPROVAL_NOTE_CHARS = 1_000`, `APPROVAL_NOTE_TOO_LONG`, `APPROVAL_TIMED_OUT`, `APPROVAL_UNAVAILABLE`, `TOO_MANY_SESSION_ALLOWANCES`, `APPROVAL_SESSION_GONE`.
- Behavior: a helper (`workspaceRole: helper`) is judged by its companion's (`parentAgentId`'s) policy and rules and has no session allowances; everyone else by their own policy, rules, and the session's allowances. Opening needs the run in flight in the ledger and not stopped. Settling validates everything (revision, note ≤ 1,000 characters after trimming, the matcher for the tool, the caps) before changing anything, then resolves the record once, adds the allowance or rule (reusing an identical one), and moves the run back to `running` when it waits on nothing else. A revert puts the record, the rule, the allowance, and the run's status back exactly.

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/state/approval_state.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anima_core::{AgentConfig, AgentSettings, AgentState, DataValue, ToolCall};

    use super::*;
    use crate::approvals::{
        ApprovalDecisionKind, ApprovalMatcher, ApprovalPolicy, ApprovalRule, ApprovalStatus,
        MatcherKind, PolicyAction, ResolvedBy, RiskClass, SessionAllowance, Verdict,
        APPROVAL_NOTE_TOO_LONG, APPROVAL_SESSION_GONE, APPROVAL_TIMED_OUT, APPROVAL_UNAVAILABLE,
        MATCHER_KIND_NOT_FOR_TOOL, MAX_APPROVAL_RULES_PER_AGENT, MAX_SESSION_ALLOWANCES,
        TOO_MANY_RULES, TOO_MANY_SESSION_ALLOWANCES,
    };
    use crate::runs::{RunLink, RunRecord, RunSource, RunStart, RunStatus, RunStopRequest};
    use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
    use crate::state::DaemonState;

    fn config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }

    fn call(name: &str, key: &str, value: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: name.into(),
            args: BTreeMap::from([(key.to_string(), DataValue::String(value.into()))]),
        }
    }

    fn remember() -> ToolCall {
        call("memory_add", "content", "the plan")
    }

    fn fetch() -> ToolCall {
        call("web_fetch", "url", "https://docs.rs/serde")
    }

    /// A daemon with one agent, its chat `chat:a`, and a running run there.
    fn daemon() -> (DaemonState, RunLink) {
        let mut state = DaemonState::new();
        let agent_id = state.create_agent(config("companion")).unwrap().state.id;
        state.sessions.insert(SessionRecord::new(
            &agent_id,
            "chat:a",
            SessionKind::Chat,
            SessionOrigin::Web,
            "A".into(),
            TitleSource::Owner,
            1,
        ));
        let run = RunRecord::running(
            RunStart {
                agent_id: agent_id.clone(),
                session_id: "chat:a".into(),
                source: RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "remember".into(),
                model: "deterministic".into(),
                provider: None,
                parent_run_id: None,
            },
            1,
        );
        let link = RunLink {
            run_id: run.id.clone(),
            session_id: "chat:a".into(),
            agent_id,
        };
        state.runs.insert(run);
        (state, link)
    }

    fn ask(link: &RunLink, call: ToolCall) -> ApprovalAsk {
        ApprovalAsk {
            run: link.clone(),
            call,
            timeout_ms: 60_000,
        }
    }

    fn owner(kind: ApprovalDecisionKind, revision: u64) -> Settlement {
        Settlement::Owner(OwnerDecision {
            kind,
            note: None,
            matcher: None,
            revision,
        })
    }

    fn agent_state(state: &DaemonState, agent_id: &str) -> AgentState {
        state.get_agent(agent_id).unwrap().state
    }

    fn run_status(state: &DaemonState, link: &RunLink) -> RunStatus {
        state.runs.get(&link.run_id).unwrap().status
    }

    #[test]
    fn a_verdict_follows_the_policy_rules_and_session_allowances() {
        let (mut state, link) = daemon();
        let agent = agent_state(&state, &link.agent_id);
        assert_eq!(state.approval_verdict(&agent, "chat:a", &remember()), Verdict::Allow);

        state.approvals.set_policy(
            &link.agent_id,
            ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
        );
        assert_eq!(state.approval_verdict(&agent, "chat:a", &remember()), Verdict::Ask);

        state
            .sessions
            .get_mut(&link.agent_id, "chat:a")
            .unwrap()
            .session_allowances
            .push(SessionAllowance {
                tool: "memory_add".into(),
                matcher: ApprovalMatcher::any(),
                created_at_ms: 1,
                from_approval_id: "apr_1".into(),
            });
        assert_eq!(state.approval_verdict(&agent, "chat:a", &remember()), Verdict::Allow);
        assert_eq!(
            state.approval_verdict(&agent, "chat:b", &remember()),
            Verdict::Ask,
            "an allowance covers its own session only"
        );
    }

    #[test]
    fn a_helper_answers_to_its_companion_without_session_allowances() {
        let (mut state, link) = daemon();
        let mut helper = config("helper");
        let settings = helper.settings.as_mut().unwrap();
        settings
            .additional
            .insert("workspaceRole".into(), DataValue::String("helper".into()));
        settings.additional.insert(
            "parentAgentId".into(),
            DataValue::String(link.agent_id.clone()),
        );
        let helper = state.create_agent(helper).unwrap().state;
        let mut room = SessionRecord::new(
            &helper.id,
            "room-helper",
            SessionKind::Helper,
            SessionOrigin::Delegation,
            "Helper".into(),
            TitleSource::System,
            1,
        );
        room.session_allowances.push(SessionAllowance {
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: 1,
            from_approval_id: "apr_1".into(),
        });
        state.sessions.insert(room);
        state.approvals.set_policy(
            &link.agent_id,
            ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
        );

        assert_eq!(
            state.approval_verdict(&helper, "room-helper", &remember()),
            Verdict::Ask,
            "the companion's policy, and no allowance counts for a helper"
        );
        state
            .approvals
            .add_rule(ApprovalRule {
                id: "rule_1".into(),
                agent_id: link.agent_id.clone(),
                tool: "memory_add".into(),
                matcher: ApprovalMatcher::any(),
                created_at_ms: 1,
                from_approval_id: None,
            })
            .unwrap();
        assert_eq!(
            state.approval_verdict(&helper, "room-helper", &remember()),
            Verdict::Allow,
            "the companion's rules cover its helpers"
        );
    }

    #[test]
    fn opening_a_request_moves_the_run_to_awaiting_approval_once() {
        let (mut state, link) = daemon();
        let first = state.open_approval(&ask(&link, remember()), 10).unwrap();
        assert_eq!(first.approval.status, ApprovalStatus::Pending);
        assert_eq!(first.approval.expires_at_ms, 60_010);
        assert_eq!(
            first.run.as_ref().map(|run| run.status),
            Some(RunStatus::AwaitingApproval)
        );
        let second = state.open_approval(&ask(&link, fetch()), 11).unwrap();
        assert!(second.run.is_none(), "the run already awaits approval");
        assert_eq!(state.approvals.pending_ids_for_run(&link.run_id).len(), 2);

        state.revert_open_approval(&second);
        assert_eq!(run_status(&state, &link), RunStatus::AwaitingApproval);
        state.revert_open_approval(&first);
        assert_eq!(run_status(&state, &link), RunStatus::Running);
        assert!(state.approvals.pending().is_empty());
    }

    #[test]
    fn a_stopped_or_missing_run_cannot_ask() {
        let (mut state, link) = daemon();
        state.runs.get_mut(&link.run_id).unwrap().stop = Some(RunStopRequest {
            requested_at_ms: 5,
        });
        let refusal = state.open_approval(&ask(&link, remember()), 10).unwrap_err();
        assert_eq!(refusal, OpenRefusal::Stopped);
        assert_eq!(
            refusal.result().error.as_deref(),
            Some(anima_core::CANCELLED_TOOL_RESULT)
        );
        let missing = RunLink {
            run_id: "run_missing".into(),
            ..link.clone()
        };
        let refusal = state.open_approval(&ask(&missing, remember()), 10).unwrap_err();
        assert_eq!(refusal, OpenRefusal::Unavailable);
        assert_eq!(refusal.result().error.as_deref(), Some(APPROVAL_UNAVAILABLE));
        assert!(state.approvals.pending().is_empty());
    }

    #[test]
    fn an_owner_decision_settles_once_and_needs_the_current_revision() {
        let (mut state, link) = daemon();
        let id = state
            .open_approval(&ask(&link, remember()), 10)
            .unwrap()
            .approval
            .id;
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowOnce, 2), 20)
                .unwrap_err(),
            SettleRefusal::Stale
        );
        let settled = state
            .settle_approval(&id, owner(ApprovalDecisionKind::AllowOnce, 1), 20)
            .unwrap();
        assert_eq!(settled.approval.status, ApprovalStatus::Allowed);
        assert_eq!(settled.approval.revision, 2);
        let resolution = settled.approval.resolution.clone().unwrap();
        assert_eq!(resolution.resolved_by, ResolvedBy::Owner);
        assert_eq!(resolution.decision, Some(ApprovalDecisionKind::AllowOnce));
        assert_eq!(resolution.resolved_at_ms, 20);
        assert_eq!(
            settled.run.map(|run| run.status),
            Some(RunStatus::Running)
        );

        match state.settle_approval(&id, owner(ApprovalDecisionKind::Deny, 2), 30) {
            Err(SettleRefusal::Resolved(record)) => {
                assert!(record.was_decided_as(ApprovalDecisionKind::AllowOnce));
                assert!(!record.was_decided_as(ApprovalDecisionKind::Deny));
            }
            other => panic!("expected the resolved record, got {other:?}"),
        }
        assert_eq!(
            state
                .settle_approval("apr_missing", owner(ApprovalDecisionKind::AllowOnce, 1), 30)
                .unwrap_err(),
            SettleRefusal::NotFound
        );
    }

    #[test]
    fn allow_session_adds_one_allowance_and_allow_always_one_rule() {
        let (mut state, link) = daemon();
        let first = state.open_approval(&ask(&link, fetch()), 10).unwrap().approval;
        let settled = state
            .settle_approval(&first.id, owner(ApprovalDecisionKind::AllowSession, 1), 11)
            .unwrap();
        let domain = ApprovalMatcher {
            kind: MatcherKind::Domain,
            value: "docs.rs".into(),
        };
        assert_eq!(settled.approval.resolution.unwrap().matcher, Some(domain));
        let again = state.open_approval(&ask(&link, fetch()), 12).unwrap().approval;
        state
            .settle_approval(&again.id, owner(ApprovalDecisionKind::AllowSession, 1), 13)
            .unwrap();
        {
            let allowances = &state
                .sessions
                .get(&link.agent_id, "chat:a")
                .unwrap()
                .session_allowances;
            assert_eq!(allowances.len(), 1, "the same grant is kept once");
            assert_eq!(allowances[0].from_approval_id, first.id);
        }

        let third = state.open_approval(&ask(&link, fetch()), 14).unwrap().approval;
        let always = state
            .settle_approval(
                &third.id,
                Settlement::Owner(OwnerDecision {
                    kind: ApprovalDecisionKind::AllowAlways,
                    note: Some("  docs are fine  ".into()),
                    matcher: Some(ApprovalMatcher {
                        kind: MatcherKind::Any,
                        value: "ignored".into(),
                    }),
                    revision: 1,
                }),
                15,
            )
            .unwrap()
            .approval;
        let resolution = always.resolution.unwrap();
        assert_eq!(resolution.note.as_deref(), Some("docs are fine"));
        let rule_id = {
            let rules = state.approvals.rules_for(&link.agent_id);
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].matcher, ApprovalMatcher::any());
            assert_eq!(rules[0].tool, "web_fetch");
            assert_eq!(rules[0].from_approval_id.as_deref(), Some(third.id.as_str()));
            rules[0].id.clone()
        };
        assert_eq!(resolution.rule_id.as_deref(), Some(rule_id.as_str()));

        let fourth = state.open_approval(&ask(&link, fetch()), 16).unwrap().approval;
        let repeat = state
            .settle_approval(
                &fourth.id,
                Settlement::Owner(OwnerDecision {
                    kind: ApprovalDecisionKind::AllowAlways,
                    note: None,
                    matcher: Some(ApprovalMatcher::any()),
                    revision: 1,
                }),
                17,
            )
            .unwrap()
            .approval;
        assert_eq!(
            repeat.resolution.unwrap().rule_id.as_deref(),
            Some(rule_id.as_str()),
            "an identical rule is reused"
        );
        assert_eq!(state.approvals.rules_for(&link.agent_id).len(), 1);
    }

    #[test]
    fn a_decision_is_validated_before_anything_changes() {
        let (mut state, link) = daemon();
        let id = state
            .open_approval(&ask(&link, remember()), 10)
            .unwrap()
            .approval
            .id;
        let long = Settlement::Owner(OwnerDecision {
            kind: ApprovalDecisionKind::Deny,
            note: Some("x".repeat(1_001)),
            matcher: None,
            revision: 1,
        });
        assert_eq!(
            state.settle_approval(&id, long, 11).unwrap_err(),
            SettleRefusal::Invalid(APPROVAL_NOTE_TOO_LONG)
        );
        let misfit = Settlement::Owner(OwnerDecision {
            kind: ApprovalDecisionKind::AllowAlways,
            note: None,
            matcher: Some(ApprovalMatcher {
                kind: MatcherKind::PathGlob,
                value: "**".into(),
            }),
            revision: 1,
        });
        assert_eq!(
            state.settle_approval(&id, misfit, 11).unwrap_err(),
            SettleRefusal::Invalid(MATCHER_KIND_NOT_FOR_TOOL)
        );
        assert!(state.approvals.get(&id).unwrap().is_pending());
        assert!(state.approvals.rules_for(&link.agent_id).is_empty());

        let exactly = Settlement::Owner(OwnerDecision {
            kind: ApprovalDecisionKind::Deny,
            note: Some("y".repeat(1_000)),
            matcher: None,
            revision: 1,
        });
        let denied = state.settle_approval(&id, exactly, 12).unwrap().approval;
        assert_eq!(denied.status, ApprovalStatus::Denied);
        assert_eq!(denied.resolution.unwrap().note.unwrap().chars().count(), 1_000);
    }

    #[test]
    fn a_timeout_is_a_denial_and_a_stop_is_stopped() {
        let (mut state, link) = daemon();
        let timed = state.open_approval(&ask(&link, remember()), 10).unwrap().approval;
        let stopped = state.open_approval(&ask(&link, fetch()), 10).unwrap().approval;

        let timed = state
            .settle_approval(&timed.id, Settlement::TimedOut, 20)
            .unwrap()
            .approval;
        assert_eq!(timed.status, ApprovalStatus::Denied);
        let resolution = timed.resolution.unwrap();
        assert_eq!(resolution.decision, Some(ApprovalDecisionKind::Deny));
        assert_eq!(resolution.note.as_deref(), Some(APPROVAL_TIMED_OUT));
        assert_eq!(resolution.resolved_by, ResolvedBy::Timeout);

        let stopped = state
            .settle_approval(&stopped.id, Settlement::Stopped, 21)
            .unwrap()
            .approval;
        assert_eq!(stopped.status, ApprovalStatus::Stopped);
        let resolution = stopped.resolution.unwrap();
        assert_eq!(resolution.decision, None);
        assert_eq!(resolution.resolved_by, ResolvedBy::Stop);
    }

    #[test]
    fn the_run_resumes_only_when_its_last_approval_settles() {
        let (mut state, link) = daemon();
        let first = state.open_approval(&ask(&link, remember()), 10).unwrap().approval;
        let second = state.open_approval(&ask(&link, fetch()), 11).unwrap().approval;

        let settled = state
            .settle_approval(&first.id, owner(ApprovalDecisionKind::AllowOnce, 1), 12)
            .unwrap();
        assert!(settled.run.is_none());
        assert_eq!(run_status(&state, &link), RunStatus::AwaitingApproval);
        let settled = state
            .settle_approval(&second.id, owner(ApprovalDecisionKind::Deny, 1), 13)
            .unwrap();
        assert_eq!(settled.run.map(|run| run.status), Some(RunStatus::Running));
        assert_eq!(run_status(&state, &link), RunStatus::Running);
    }

    #[test]
    fn a_reverted_settlement_puts_everything_back() {
        let (mut state, link) = daemon();
        let always = state.open_approval(&ask(&link, fetch()), 10).unwrap().approval;
        let settled = state
            .settle_approval(&always.id, owner(ApprovalDecisionKind::AllowAlways, 1), 11)
            .unwrap();
        assert_eq!(state.approvals.rules_for(&link.agent_id).len(), 1);
        state.revert_settled_approval(settled.undo);
        let restored = state.approvals.get(&always.id).unwrap();
        assert!(restored.is_pending());
        assert_eq!(restored.revision, 1);
        assert!(state.approvals.rules_for(&link.agent_id).is_empty());
        assert_eq!(run_status(&state, &link), RunStatus::AwaitingApproval);

        let settled = state
            .settle_approval(&always.id, owner(ApprovalDecisionKind::AllowSession, 1), 12)
            .unwrap();
        state.revert_settled_approval(settled.undo);
        assert!(state
            .sessions
            .get(&link.agent_id, "chat:a")
            .unwrap()
            .session_allowances
            .is_empty());
        assert!(state.approvals.get(&always.id).unwrap().is_pending());
    }

    #[test]
    fn full_rules_and_allowances_refuse_the_next_grant() {
        let (mut state, link) = daemon();
        for index in 0..MAX_APPROVAL_RULES_PER_AGENT {
            state
                .approvals
                .add_rule(ApprovalRule {
                    id: format!("rule_{index}"),
                    agent_id: link.agent_id.clone(),
                    tool: "web_fetch".into(),
                    matcher: ApprovalMatcher {
                        kind: MatcherKind::Domain,
                        value: format!("d{index}.example"),
                    },
                    created_at_ms: 1,
                    from_approval_id: None,
                })
                .unwrap();
        }
        let id = state.open_approval(&ask(&link, fetch()), 10).unwrap().approval.id;
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowAlways, 1), 11)
                .unwrap_err(),
            SettleRefusal::Conflict(TOO_MANY_RULES)
        );

        let session = state.sessions.get_mut(&link.agent_id, "chat:a").unwrap();
        for index in 0..MAX_SESSION_ALLOWANCES {
            session.session_allowances.push(SessionAllowance {
                tool: "web_fetch".into(),
                matcher: ApprovalMatcher {
                    kind: MatcherKind::Domain,
                    value: format!("s{index}.example"),
                },
                created_at_ms: 1,
                from_approval_id: format!("apr_{index}"),
            });
        }
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowSession, 1), 12)
                .unwrap_err(),
            SettleRefusal::Conflict(TOO_MANY_SESSION_ALLOWANCES)
        );
        assert!(state.approvals.get(&id).unwrap().is_pending());

        state.sessions.remove(&link.agent_id, "chat:a");
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowSession, 1), 13)
                .unwrap_err(),
            SettleRefusal::Conflict(APPROVAL_SESSION_GONE)
        );
    }
}
```

In `hosts/rust-daemon/src/state.rs`, add `mod approval_state;` as the first line of the module list (before `mod live_state;`) and, after the `pub(crate) use self::session_state::RunSessionRequest;` line:

```text
#[allow(unused_imports)] // M4 Tasks 5 and 6 use these; Task 8 removes the allow.
pub(crate) use self::approval_state::{
    ApprovalAsk, ApprovalUndo, OwnerDecision, SettleRefusal, Settlement,
};
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- state::approval_state`
Expected: FAIL to compile — `ApprovalAsk`, `OwnerDecision`, `Settlement`, `DaemonState::open_approval`, the new constants, and the other names are not defined.

- [ ] **Step 3: Add the constants**

In `hosts/rust-daemon/src/approvals/mod.rs`, after `TOO_MANY_RULES`, add:

```text
/// The longest owner note (spec §7.3, §16), in characters after trimming.
pub(crate) const MAX_APPROVAL_NOTE_CHARS: usize = 1_000;
pub(crate) const APPROVAL_NOTE_TOO_LONG: &str = "note must be at most 1,000 characters";
/// A timeout's note (spec §7.3): the call's result reads
/// "Denied by owner: Approval timed out".
pub(crate) const APPROVAL_TIMED_OUT: &str = "Approval timed out";
/// The tool result when a run that is not in flight tries to ask.
pub(crate) const APPROVAL_UNAVAILABLE: &str =
    "Needs owner approval, but this run cannot ask for it; the tool did not run";
pub(crate) const TOO_MANY_SESSION_ALLOWANCES: &str = "This session already has 50 allowances";
/// `allow_session` for a session that no longer exists.
pub(crate) const APPROVAL_SESSION_GONE: &str = "This approval's session no longer exists";
```

- [ ] **Step 4: Implement the state changes**

In `hosts/rust-daemon/src/state/approval_state.rs`, above the test module, add:

```rust
//! Approval state changes (spec §7.3), made under the control-plane
//! transaction and the state write lock: opening a request, settling it,
//! and putting either back when its save fails. Nothing here awaits.
#![allow(dead_code)] // M4 Task 8 removes this once the gate and the routes use every item.

use anima_core::{AgentState, Content, TaskResult, ToolCall, CANCELLED_TOOL_RESULT};

use super::DaemonState;
use crate::agent_runs::{config_helper_parent, is_helper_config};
use crate::approvals::{
    evaluate, validate_matcher, ApprovalDecisionKind, ApprovalMatcher, ApprovalRequest,
    ApprovalResolution, ApprovalRule, ApprovalStatus, PendingApprovalStart, ResolvedBy,
    SessionAllowance, Verdict, APPROVAL_NOTE_TOO_LONG, APPROVAL_SESSION_GONE, APPROVAL_TIMED_OUT,
    APPROVAL_UNAVAILABLE, MAX_APPROVAL_NOTE_CHARS, MAX_SESSION_ALLOWANCES,
    TOO_MANY_SESSION_ALLOWANCES,
};
use crate::runs::{RunLink, RunRecord, RunStatus};

/// A call that needs the owner (spec §7.3).
#[derive(Clone, Debug)]
pub(crate) struct ApprovalAsk {
    pub(crate) run: RunLink,
    pub(crate) call: ToolCall,
    pub(crate) timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenedApproval {
    pub(crate) approval: ApprovalRequest,
    /// The run as it now is, when this request moved it to `awaiting_approval`.
    pub(crate) run: Option<RunRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenRefusal {
    /// The run's stop is saved: the call never runs.
    Stopped,
    /// The run is not in flight in the ledger.
    Unavailable,
}

impl OpenRefusal {
    pub(crate) fn result(self) -> TaskResult<Content> {
        TaskResult::error(
            match self {
                Self::Stopped => CANCELLED_TOOL_RESULT,
                Self::Unavailable => APPROVAL_UNAVAILABLE,
            },
            0,
        )
    }
}

/// The owner's decision as the decision route received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnerDecision {
    pub(crate) kind: ApprovalDecisionKind,
    pub(crate) note: Option<String>,
    pub(crate) matcher: Option<ApprovalMatcher>,
    pub(crate) revision: u64,
}

/// What settles a pending approval (spec §7.3).
#[derive(Clone, Debug)]
pub(crate) enum Settlement {
    Owner(OwnerDecision),
    TimedOut,
    Stopped,
}

/// Why a settlement changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettleRefusal {
    /// The control plane holds no such approval.
    NotFound,
    /// Something settled it first: the record as it stands.
    Resolved(ApprovalRequest),
    /// The decision carried an old revision.
    Stale,
    /// A 400: the note or the matcher.
    Invalid(&'static str),
    /// A 409: a cap, or the session is gone.
    Conflict(&'static str),
}

/// What a settlement changed, as it was, so a failed save can put it back.
#[derive(Clone, Debug)]
pub(crate) struct ApprovalUndo {
    previous: ApprovalRequest,
    added_rule: Option<String>,
    added_allowance: bool,
    resumed_run: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SettledApproval {
    pub(crate) approval: ApprovalRequest,
    /// The run as it now is, when this settlement moved it back to `running`.
    pub(crate) run: Option<RunRecord>,
    pub(crate) undo: ApprovalUndo,
}

/// A trimmed note of at most `MAX_APPROVAL_NOTE_CHARS`, `None` when blank.
fn normalized_note(note: Option<&str>) -> Result<Option<String>, SettleRefusal> {
    let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) else {
        return Ok(None);
    };
    if note.chars().count() > MAX_APPROVAL_NOTE_CHARS {
        return Err(SettleRefusal::Invalid(APPROVAL_NOTE_TOO_LONG));
    }
    Ok(Some(note.to_string()))
}

impl DaemonState {
    /// What `call` needs in `session_id` (spec §7.2). A helper answers to
    /// its companion's policy and rules and has no session allowances.
    pub(crate) fn approval_verdict(
        &self,
        agent: &AgentState,
        session_id: &str,
        call: &ToolCall,
    ) -> Verdict {
        let owner = config_helper_parent(&agent.config).unwrap_or(agent.id.as_str());
        let policy = self.approvals.policy(owner);
        let rules = self.approvals.rules_for(owner);
        let allowances: &[SessionAllowance] = if is_helper_config(&agent.config) {
            &[]
        } else {
            self.sessions
                .get(&agent.id, session_id)
                .map(|session| session.session_allowances.as_slice())
                .unwrap_or(&[])
        };
        evaluate(&policy, &rules, allowances, call)
    }

    /// Records a pending approval for `ask` and moves its run to
    /// `awaiting_approval` (spec §7.3). The caller saves, then announces.
    pub(crate) fn open_approval(
        &mut self,
        ask: &ApprovalAsk,
        now_ms: u64,
    ) -> Result<OpenedApproval, OpenRefusal> {
        let run = self
            .runs
            .get_mut(&ask.run.run_id)
            .filter(|run| run.agent_id == ask.run.agent_id)
            .ok_or(OpenRefusal::Unavailable)?;
        if run.stop.is_some() {
            return Err(OpenRefusal::Stopped);
        }
        if !run.status.is_in_flight() {
            return Err(OpenRefusal::Unavailable);
        }
        let moved = run.status == RunStatus::Running;
        if moved {
            run.status = RunStatus::AwaitingApproval;
        }
        let moved = moved.then(|| run.clone());
        let approval = ApprovalRequest::pending(
            PendingApprovalStart {
                agent_id: &ask.run.agent_id,
                session_id: &ask.run.session_id,
                run_id: &ask.run.run_id,
                call: &ask.call,
                timeout_ms: ask.timeout_ms,
            },
            now_ms,
        );
        self.approvals.insert(approval.clone());
        Ok(OpenedApproval {
            approval,
            run: moved.map(|record| self.with_live_tools(record)),
        })
    }

    /// Takes back a request whose save failed.
    pub(crate) fn revert_open_approval(&mut self, opened: &OpenedApproval) {
        self.approvals.remove(&opened.approval.id);
        self.resume_if_unblocked(&opened.approval.run_id);
    }

    /// Moves `run_id` back to `running` once it waits on no approval.
    fn resume_if_unblocked(&mut self, run_id: &str) -> Option<RunRecord> {
        if !self.approvals.pending_ids_for_run(run_id).is_empty() {
            return None;
        }
        let run = self
            .runs
            .get_mut(run_id)
            .filter(|run| run.status == RunStatus::AwaitingApproval)?;
        run.status = RunStatus::Running;
        let record = run.clone();
        Some(self.with_live_tools(record))
    }

    /// Settles pending approval `id` (spec §7.3): validates, then resolves it
    /// once, adds the allowance or rule, and resumes the run when it waits on
    /// nothing else. The caller saves, and reverts with `undo` if that fails.
    pub(crate) fn settle_approval(
        &mut self,
        id: &str,
        settlement: Settlement,
        now_ms: u64,
    ) -> Result<SettledApproval, SettleRefusal> {
        let previous = self
            .approvals
            .get(id)
            .cloned()
            .ok_or(SettleRefusal::NotFound)?;
        if !previous.is_pending() {
            return Err(SettleRefusal::Resolved(previous));
        }
        let (status, mut resolution, grant) = match settlement {
            Settlement::Owner(decision) => {
                if decision.revision != previous.revision {
                    return Err(SettleRefusal::Stale);
                }
                let note = normalized_note(decision.note.as_deref())?;
                let matcher = match decision.kind {
                    ApprovalDecisionKind::AllowSession | ApprovalDecisionKind::AllowAlways => {
                        Some(
                            validate_matcher(
                                &previous.tool,
                                decision
                                    .matcher
                                    .as_ref()
                                    .unwrap_or(&previous.suggested_matcher),
                            )
                            .map_err(SettleRefusal::Invalid)?,
                        )
                    }
                    ApprovalDecisionKind::AllowOnce | ApprovalDecisionKind::Deny => None,
                };
                let status = if decision.kind == ApprovalDecisionKind::Deny {
                    ApprovalStatus::Denied
                } else {
                    ApprovalStatus::Allowed
                };
                (
                    status,
                    ApprovalResolution {
                        decision: Some(decision.kind),
                        note,
                        matcher: matcher.clone(),
                        rule_id: None,
                        resolved_by: ResolvedBy::Owner,
                        resolved_at_ms: now_ms,
                    },
                    matcher.map(|matcher| (decision.kind, matcher)),
                )
            }
            Settlement::TimedOut => (
                ApprovalStatus::Denied,
                ApprovalResolution {
                    decision: Some(ApprovalDecisionKind::Deny),
                    note: Some(APPROVAL_TIMED_OUT.to_string()),
                    matcher: None,
                    rule_id: None,
                    resolved_by: ResolvedBy::Timeout,
                    resolved_at_ms: now_ms,
                },
                None,
            ),
            Settlement::Stopped => (
                ApprovalStatus::Stopped,
                ApprovalResolution {
                    decision: None,
                    note: None,
                    matcher: None,
                    rule_id: None,
                    resolved_by: ResolvedBy::Stop,
                    resolved_at_ms: now_ms,
                },
                None,
            ),
        };
        let mut undo = ApprovalUndo {
            previous: previous.clone(),
            added_rule: None,
            added_allowance: false,
            resumed_run: false,
        };
        match grant {
            Some((ApprovalDecisionKind::AllowSession, matcher)) => {
                let session = self
                    .sessions
                    .get_mut(&previous.agent_id, &previous.session_id)
                    .ok_or(SettleRefusal::Conflict(APPROVAL_SESSION_GONE))?;
                let held = session
                    .session_allowances
                    .iter()
                    .any(|allowance| allowance.tool == previous.tool && allowance.matcher == matcher);
                if !held {
                    if session.session_allowances.len() >= MAX_SESSION_ALLOWANCES {
                        return Err(SettleRefusal::Conflict(TOO_MANY_SESSION_ALLOWANCES));
                    }
                    session.session_allowances.push(SessionAllowance {
                        tool: previous.tool.clone(),
                        matcher,
                        created_at_ms: now_ms,
                        from_approval_id: previous.id.clone(),
                    });
                    undo.added_allowance = true;
                }
            }
            Some((_, matcher)) => {
                let existing = self
                    .approvals
                    .find_rule(&previous.agent_id, &previous.tool, &matcher)
                    .map(|rule| rule.id.clone());
                match existing {
                    Some(rule_id) => resolution.rule_id = Some(rule_id),
                    None => {
                        let rule = ApprovalRule {
                            id: format!("rule_{}", uuid::Uuid::new_v4()),
                            agent_id: previous.agent_id.clone(),
                            tool: previous.tool.clone(),
                            matcher,
                            created_at_ms: now_ms,
                            from_approval_id: Some(previous.id.clone()),
                        };
                        let rule_id = rule.id.clone();
                        self.approvals
                            .add_rule(rule)
                            .map_err(SettleRefusal::Conflict)?;
                        resolution.rule_id = Some(rule_id.clone());
                        undo.added_rule = Some(rule_id);
                    }
                }
            }
            None => {}
        }
        let approval = {
            let record = self
                .approvals
                .get_mut(id)
                .expect("the approval was found above");
            record.resolve(status, resolution);
            record.clone()
        };
        let run = self.resume_if_unblocked(&approval.run_id);
        undo.resumed_run = run.is_some();
        Ok(SettledApproval {
            approval,
            run,
            undo,
        })
    }

    /// Puts back what a settlement changed after its save failed. Nothing
    /// else changes the approval meanwhile: every settlement holds the
    /// control-plane transaction until it is saved or reverted.
    pub(crate) fn revert_settled_approval(&mut self, undo: ApprovalUndo) {
        let ApprovalUndo {
            previous,
            added_rule,
            added_allowance,
            resumed_run,
        } = undo;
        if let Some(rule_id) = added_rule {
            self.approvals.remove_rule(&previous.agent_id, &rule_id);
        }
        if added_allowance {
            if let Some(session) = self
                .sessions
                .get_mut(&previous.agent_id, &previous.session_id)
            {
                session
                    .session_allowances
                    .retain(|allowance| allowance.from_approval_id != previous.id);
            }
        }
        if resumed_run {
            if let Some(run) = self
                .runs
                .get_mut(&previous.run_id)
                .filter(|run| run.status == RunStatus::Running)
            {
                run.status = RunStatus::AwaitingApproval;
            }
        }
        self.approvals.insert(previous);
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- state::approval_state approvals::`
Expected: PASS — the 11 new tests and Tasks 1–2's.

- [ ] **Step 6: Format and commit**

Run: `cargo fmt --all && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- state::approval_state`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/state/approval_state.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/approvals/mod.rs
git commit -m "feat(daemon): open and settle approvals with rules, session allowances, and exact reverts"
```

Recommended implementer tier: standard (synchronous state logic with exhaustive unit tests; the revert paths are where a reviewer should look hardest).

---

### Task 4: Decided approvals in the history store and the outbox

**Files:**

- Modify: `hosts/rust-daemon/src/history/mod.rs` (`ApprovalPageQuery`; three trait methods; deletion docs)
- Modify: `hosts/rust-daemon/src/history/memory.rs`, `hosts/rust-daemon/src/history/sqlite.rs`, `hosts/rust-daemon/src/history/postgres.rs` (the methods; deletions remove approvals; conformance calls)
- Modify: `hosts/rust-daemon/src/history/conformance.rs` (`decided_approval`, `assert_history_store_approval_conformance`, `FlakyHistoryStore`)
- Modify: `hosts/rust-daemon/src/history/outbox.rs` (`HISTORY_APPROVAL_BATCH`, `write_approvals`, `FlushReport.approvals`, tests)
- Modify: `hosts/rust-daemon/src/state/approval_state.rs` (`unmirrored_decided_approvals`)

**Interfaces:**

- Consumes: Task 2 `ApprovalRequest`, `ApprovalRegistry::{retain_decided, unmirrored_decided, mark_mirrored}`; the tables the M2 schema already created (`approvals` in SQLite schema v1, `history_approvals` in migration `20260923000000_history_store.sql`, both `(id, agent_id, session_id, created_at_ms, record)`); `HistoryService::flush_once` (M2).
- Produces:
  - `history::ApprovalPageQuery { agent_id: Option<String>, since_ms: u64, before: Option<(u64, String)>, limit: usize }` (newest first by `(createdAtMs, id)`; `before` is exclusive).
  - `HistoryStore::upsert_approvals(&self, approvals: &[ApprovalRequest]) -> Result<(), HistoryError>`, `get_approval(&self, approval_id: &str) -> Result<Option<ApprovalRequest>, HistoryError>`, `page_approvals(&self, query: &ApprovalPageQuery) -> Result<Vec<ApprovalRequest>, HistoryError>`.
  - `delete_session` also removes the session's approvals; `delete_agent` also removes the agent's approvals (usage rows still stay).
  - `DaemonState::unmirrored_decided_approvals(&mut self, limit: usize) -> Vec<ApprovalRequest>`.
  - `FlushReport.approvals: usize`; `HISTORY_APPROVAL_BATCH = 200`.
- Behavior: after messages and terminal runs, each flush writes the control plane's decided approvals in batches, read under the control-plane transaction (so only saved state is mirrored), and removes the ones the store now holds unchanged. A failing store keeps them in the control plane until it recovers. Decided approvals of deleted agents or sessions are dropped instead of written, so a deleted session's approval rows (its tool arguments) never come back. Pending approvals are never written.

- [ ] **Step 1: Write the failing tests**

In `hosts/rust-daemon/src/history/conformance.rs`:

1. Add to the imports:

```text
use std::collections::BTreeMap;

use anima_core::{DataValue, ToolCall};

use super::ApprovalPageQuery;
use crate::approvals::{
    ApprovalDecisionKind, ApprovalRequest, ApprovalResolution, ApprovalStatus,
    PendingApprovalStart, ResolvedBy,
};
```

2. After `terminal_run`, add the fixture and the conformance case:

```rust
/// An approval the owner allowed once, created at `created_at_ms`.
pub(crate) fn decided_approval(
    id: &str,
    agent_id: &str,
    session_id: &str,
    created_at_ms: u64,
) -> ApprovalRequest {
    let call = ToolCall {
        id: format!("call-{id}"),
        name: "memory_add".into(),
        args: BTreeMap::from([(
            "content".to_string(),
            DataValue::String("the plan".into()),
        )]),
    };
    let mut approval = ApprovalRequest::pending(
        PendingApprovalStart {
            agent_id,
            session_id,
            run_id: "run_conformance",
            call: &call,
            timeout_ms: 1_000,
        },
        created_at_ms,
    );
    approval.id = id.to_string();
    approval.resolve(
        ApprovalStatus::Allowed,
        ApprovalResolution {
            decision: Some(ApprovalDecisionKind::AllowOnce),
            note: None,
            matcher: None,
            rule_id: None,
            resolved_by: ResolvedBy::Owner,
            resolved_at_ms: created_at_ms + 5,
        },
    );
    approval
}

/// Every store keeps decided approvals by id, pages them newest first
/// within a window, and removes them with their session or agent. Ids and
/// timestamps are unique per call, so a shared Postgres database can run it
/// repeatedly.
pub(crate) async fn assert_history_store_approval_conformance(store: &dyn HistoryStore) {
    let agent = format!("agent-{}", uuid::Uuid::new_v4());
    let other = format!("agent-{}", uuid::Uuid::new_v4());
    let base = (uuid::Uuid::new_v4().as_u128() % 1_000_000_000) as u64 * 1_000;
    let id = |n: u32| format!("apr_{base}_{n}");
    let first = decided_approval(&id(1), &agent, "chat:a", base + 100);
    let second = decided_approval(&id(2), &agent, "chat:b", base + 200);
    // Same time as `second`: the id breaks the tie.
    let third = decided_approval(&id(3), &agent, "chat:a", base + 200);
    let elsewhere = decided_approval(&id(4), &other, "chat:a", base + 300);
    store
        .upsert_approvals(&[
            first.clone(),
            second.clone(),
            third.clone(),
            elsewhere.clone(),
        ])
        .await
        .unwrap();

    let mut changed = first.clone();
    changed.resolution.as_mut().unwrap().note = Some("rewritten".into());
    store.upsert_approvals(&[changed.clone()]).await.unwrap();
    assert_eq!(store.get_approval(&first.id).await.unwrap(), Some(changed));
    assert_eq!(store.get_approval(&id(99)).await.unwrap(), None);

    let page = |agent_id: Option<&str>, since_ms: u64, before: Option<&ApprovalRequest>, limit| {
        ApprovalPageQuery {
            agent_id: agent_id.map(str::to_string),
            since_ms,
            before: before.map(|approval| (approval.created_at_ms, approval.id.clone())),
            limit,
        }
    };
    let ids = |rows: Vec<ApprovalRequest>| {
        rows.into_iter()
            .map(|approval| approval.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(store.page_approvals(&page(Some(&agent), base, None, 10)).await.unwrap()),
        [third.id.clone(), second.id.clone(), first.id.clone()]
    );
    let first_page = store
        .page_approvals(&page(Some(&agent), base, None, 2))
        .await
        .unwrap();
    assert_eq!(ids(first_page.clone()), [third.id.clone(), second.id.clone()]);
    assert_eq!(
        ids(store
            .page_approvals(&page(Some(&agent), base, first_page.last(), 2))
            .await
            .unwrap()),
        [first.id.clone()]
    );
    assert_eq!(
        ids(store
            .page_approvals(&page(Some(&agent), base + 150, None, 10))
            .await
            .unwrap()),
        [third.id.clone(), second.id.clone()],
        "only approvals created inside the window"
    );
    let everyone = store
        .page_approvals(&page(None, base, None, 1_000))
        .await
        .unwrap()
        .into_iter()
        .filter(|approval| approval.agent_id == agent || approval.agent_id == other)
        .collect::<Vec<_>>();
    assert_eq!(
        ids(everyone),
        [
            elsewhere.id.clone(),
            third.id.clone(),
            second.id.clone(),
            first.id.clone()
        ]
    );

    store.delete_session(&agent, "chat:b").await.unwrap();
    assert_eq!(store.get_approval(&second.id).await.unwrap(), None);
    assert!(store.get_approval(&first.id).await.unwrap().is_some());
    store.delete_agent(&agent).await.unwrap();
    assert_eq!(store.get_approval(&first.id).await.unwrap(), None);
    assert_eq!(store.get_approval(&third.id).await.unwrap(), None);
    assert_eq!(
        store.get_approval(&elsewhere.id).await.unwrap(),
        Some(elsewhere)
    );
}
```

3. In `impl HistoryStore for FlakyHistoryStore`, add after `upsert_runs`:

```rust
    async fn upsert_approvals(&self, approvals: &[ApprovalRequest]) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.upsert_approvals(approvals).await
    }

    async fn get_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<ApprovalRequest>, HistoryError> {
        self.check()?;
        self.inner.get_approval(approval_id).await
    }

    async fn page_approvals(
        &self,
        query: &ApprovalPageQuery,
    ) -> Result<Vec<ApprovalRequest>, HistoryError> {
        self.check()?;
        self.inner.page_approvals(query).await
    }
```

In each store's conformance test, call the new case after `assert_history_store_diacritics_conformance(&store).await;`, and add `assert_history_store_approval_conformance` to that test module's `use crate::history::conformance::{...}` list:

- `hosts/rust-daemon/src/history/memory.rs` (the memory store's conformance test),
- `hosts/rust-daemon/src/history/sqlite.rs` (`sqlite_store_meets_the_conformance_suite`),
- `hosts/rust-daemon/src/history/postgres.rs` (`postgres_store_meets_the_conformance_suite`, still `#[ignore]`).

```text
        assert_history_store_approval_conformance(&store).await;
```

In `hosts/rust-daemon/src/history/outbox.rs`'s `tests` module:

1. In `committed_turns_and_terminal_runs_reach_the_store_and_runs_are_marked_mirrored`, add `approvals: 0,` to the expected `FlushReport { … }` literal.
2. Add these tests at the end of the module:

```rust
    /// `agent_id`'s chat `session_id` with one decided and one pending approval.
    async fn with_approvals(state: &SharedDaemonState, agent_id: &str, session_id: &str) -> String {
        use crate::history::conformance::decided_approval;
        let mut guard = state.write().await;
        guard
            .sessions
            .insert(crate::sessions::SessionRecord::new(
                agent_id,
                session_id,
                crate::sessions::SessionKind::Chat,
                crate::sessions::SessionOrigin::Web,
                "Chat".into(),
                crate::sessions::TitleSource::Owner,
                1,
            ));
        let decided = decided_approval(&format!("apr_done_{session_id}"), agent_id, session_id, 10);
        let mut pending = decided_approval(&format!("apr_wait_{session_id}"), agent_id, session_id, 20);
        pending.status = crate::approvals::ApprovalStatus::Pending;
        pending.resolution = None;
        guard.approvals.insert(decided.clone());
        guard.approvals.insert(pending);
        decided.id
    }

    #[tokio::test]
    async fn decided_approvals_move_to_the_store_and_leave_the_control_plane() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let decided = with_approvals(&state, &agent_id, "chat:one").await;
        let history = state.read().await.history.clone();

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!(report.approvals, 1);
        assert_eq!(store.get_approval(&decided).await.unwrap().unwrap().id, decided);
        let guard = state.read().await;
        assert!(guard.approvals.get(&decided).is_none(), "the store holds it now");
        assert!(
            guard.approvals.get("apr_wait_chat:one").is_some(),
            "a pending approval is never written"
        );
        assert!(store.get_approval("apr_wait_chat:one").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_failing_store_keeps_decided_approvals_and_a_deleted_session_drops_its_own() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let kept = with_approvals(&state, &agent_id, "chat:kept").await;
        let orphan = with_approvals(&state, &agent_id, "chat:gone").await;
        state.write().await.sessions.remove(&agent_id, "chat:gone");
        let history = state.read().await.history.clone();
        store.set_failing(true);

        assert!(history
            .flush_once(&state, &transactions, now_millis())
            .await
            .is_err());
        assert!(state.read().await.approvals.get(&kept).is_some());

        store.set_failing(false);
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(report.approvals, 1);
        assert!(store.get_approval(&kept).await.unwrap().is_some());
        assert!(
            store.get_approval(&orphan).await.unwrap().is_none(),
            "a deleted session's approval is never written"
        );
        assert!(state.read().await.approvals.get(&orphan).is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::`
Expected: FAIL to compile — `ApprovalPageQuery`, `upsert_approvals`, `get_approval`, `page_approvals`, and `FlushReport.approvals` do not exist.

- [ ] **Step 3: Add the query and the trait methods**

In `hosts/rust-daemon/src/history/mod.rs`:

1. Add `use crate::approvals::ApprovalRequest;` after `use crate::runs::RunRecord;`.
2. After `MessagePageQuery`, add:

```rust
/// One page of decided approvals, newest first by `(createdAtMs, id)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalPageQuery {
    /// Only this agent's; every agent's when `None`.
    pub(crate) agent_id: Option<String>,
    /// Only approvals created at or after this time (spec §7.3's 30 days).
    pub(crate) since_ms: u64,
    /// Only approvals strictly older than this `(createdAtMs, id)`.
    pub(crate) before: Option<(u64, String)>,
    pub(crate) limit: usize,
}
```

3. In `trait HistoryStore`, after `upsert_runs`, add:

```rust
    /// Decided approvals (spec §13.1), idempotent by id.
    async fn upsert_approvals(&self, approvals: &[ApprovalRequest]) -> Result<(), HistoryError>;

    async fn get_approval(&self, approval_id: &str)
        -> Result<Option<ApprovalRequest>, HistoryError>;

    async fn page_approvals(
        &self,
        query: &ApprovalPageQuery,
    ) -> Result<Vec<ApprovalRequest>, HistoryError>;
```

4. Replace the two deletion doc comments with:

```text
    /// Removes a session's messages, runs, approvals, and attachment records.
```

```text
    /// Removes an agent's messages, runs, approvals, and attachment records
    /// in every session; usage rows stay (spec §3.3).
```

- [ ] **Step 4: Implement the memory store**

In `hosts/rust-daemon/src/history/memory.rs`:

1. Add `use crate::approvals::ApprovalRequest;` and add `ApprovalPageQuery` to the `use super::{…}` list.
2. Add to `Tables`:

```text
    approvals: HashMap<String, (u64, ApprovalRequest)>,
    approval_seqs: BTreeMap<u64, String>,
```

3. Add to the impl, after `upsert_runs`:

```rust
    async fn upsert_approvals(&self, approvals: &[ApprovalRequest]) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        for approval in approvals {
            upsert(
                &mut tables.approvals,
                &mut tables.approval_seqs,
                &mut tables.next_seq,
                approval.id.clone(),
                approval.clone(),
                self.max_rows,
            );
        }
        Ok(())
    }

    async fn get_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<ApprovalRequest>, HistoryError> {
        Ok(self
            .tables()
            .approvals
            .get(approval_id)
            .map(|(_, approval)| approval.clone()))
    }

    async fn page_approvals(
        &self,
        query: &ApprovalPageQuery,
    ) -> Result<Vec<ApprovalRequest>, HistoryError> {
        let tables = self.tables();
        let mut rows = tables
            .approvals
            .values()
            .map(|(_, approval)| approval)
            .filter(|approval| {
                query
                    .agent_id
                    .as_deref()
                    .is_none_or(|agent_id| approval.agent_id == agent_id)
                    && approval.created_at_ms >= query.since_ms
                    && query.before.as_ref().is_none_or(|(at_ms, id)| {
                        (approval.created_at_ms, approval.id.as_str()) < (*at_ms, id.as_str())
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            (right.created_at_ms, &right.id).cmp(&(left.created_at_ms, &left.id))
        });
        rows.truncate(query.limit);
        Ok(rows)
    }
```

4. In `delete_session`, add after the runs' `remove_where`:

```text
        remove_where(&mut tables.approvals, &mut tables.approval_seqs, |approval| {
            approval.agent_id == agent_id && approval.session_id == session_id
        });
```

and in `delete_agent`:

```text
        remove_where(&mut tables.approvals, &mut tables.approval_seqs, |approval| {
            approval.agent_id == agent_id
        });
```

- [ ] **Step 5: Implement the SQLite and Postgres stores**

In `hosts/rust-daemon/src/history/sqlite.rs`:

1. Add `use crate::approvals::ApprovalRequest;` and add `ApprovalPageQuery` to the `use super::{…}` list.
2. After `UPSERT_RUN`, add:

```rust
const UPSERT_APPROVAL: &str = "
INSERT INTO approvals (id, agent_id, session_id, created_at_ms, record)
VALUES (?1, ?2, ?3, ?4, ?5)
ON CONFLICT (id) DO UPDATE SET
    agent_id = excluded.agent_id,
    session_id = excluded.session_id,
    created_at_ms = excluded.created_at_ms,
    record = excluded.record";

const PAGE_APPROVALS: &str = "
SELECT record FROM approvals
WHERE (?1 IS NULL OR agent_id = ?1) AND created_at_ms >= ?2
  AND (?3 IS NULL OR created_at_ms < ?3 OR (created_at_ms = ?3 AND id < ?4))
ORDER BY created_at_ms DESC, id DESC
LIMIT ?5";
```

3. Add to the impl, after `upsert_runs`:

```rust
    async fn upsert_approvals(&self, approvals: &[ApprovalRequest]) -> Result<(), HistoryError> {
        if approvals.is_empty() {
            return Ok(());
        }
        let rows = approvals
            .iter()
            .map(|approval| -> Result<(String, String, String, i64, String), HistoryError> {
                Ok((
                    approval.id.clone(),
                    approval.agent_id.clone(),
                    approval.session_id.clone(),
                    to_i64(approval.created_at_ms)?,
                    serde_json::to_string(approval)?,
                ))
            })
            .collect::<Result<Vec<_>, HistoryError>>()?;
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            {
                let mut statement = transaction.prepare_cached(UPSERT_APPROVAL)?;
                for (id, agent_id, session_id, created_at_ms, record) in &rows {
                    statement.execute(params![id, agent_id, session_id, created_at_ms, record])?;
                }
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn get_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<ApprovalRequest>, HistoryError> {
        let approval_id = approval_id.to_string();
        self.run(move |connection| {
            let record = connection
                .query_row(
                    "SELECT record FROM approvals WHERE id = ?1",
                    params![approval_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            record
                .map(|record| serde_json::from_str(&record).map_err(HistoryError::from))
                .transpose()
        })
        .await
    }

    async fn page_approvals(
        &self,
        query: &ApprovalPageQuery,
    ) -> Result<Vec<ApprovalRequest>, HistoryError> {
        let query = query.clone();
        self.run(move |connection| {
            let (before_at, before_id) = match &query.before {
                Some((at_ms, id)) => (Some(to_i64(*at_ms)?), id.clone()),
                None => (None, String::new()),
            };
            let mut statement = connection.prepare_cached(PAGE_APPROVALS)?;
            let records = statement
                .query_map(
                    params![
                        query.agent_id,
                        to_i64(query.since_ms)?,
                        before_at,
                        before_id,
                        i64::try_from(query.limit).unwrap_or(i64::MAX)
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

4. In `delete_session` and `delete_agent`, replace `for table in ["messages", "runs", "attachments"]` with `for table in ["messages", "runs", "attachments", "approvals"]` (both loops), and in `delete_agent` replace `// Usage rows stay (spec §3.3).` with `// Usage rows stay (spec §3.3); approvals go with their agent.`.

In `hosts/rust-daemon/src/history/postgres.rs`:

1. Add `use crate::approvals::ApprovalRequest;` and add `ApprovalPageQuery` to the `use super::{…}` list.
2. After `UPSERT_RUN`, add:

```rust
const UPSERT_APPROVAL: &str = "
INSERT INTO history_approvals (id, agent_id, session_id, created_at_ms, record)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (id) DO UPDATE SET
    agent_id = EXCLUDED.agent_id,
    session_id = EXCLUDED.session_id,
    created_at_ms = EXCLUDED.created_at_ms,
    record = EXCLUDED.record";

const PAGE_APPROVALS: &str = "
SELECT record FROM history_approvals
WHERE ($1::text IS NULL OR agent_id = $1) AND created_at_ms >= $2
  AND ($3::bigint IS NULL OR created_at_ms < $3 OR (created_at_ms = $3 AND id < $4))
ORDER BY created_at_ms DESC, id DESC
LIMIT $5";
```

3. Add to the impl, after `upsert_runs`:

```rust
    async fn upsert_approvals(&self, approvals: &[ApprovalRequest]) -> Result<(), HistoryError> {
        if approvals.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for approval in approvals {
            sqlx::query(UPSERT_APPROVAL)
                .bind(&approval.id)
                .bind(&approval.agent_id)
                .bind(&approval.session_id)
                .bind(to_i64(approval.created_at_ms)?)
                .bind(serde_json::to_value(approval)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn get_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<ApprovalRequest>, HistoryError> {
        let row = sqlx::query("SELECT record FROM history_approvals WHERE id = $1")
            .bind(approval_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| -> Result<ApprovalRequest, HistoryError> {
            let record: serde_json::Value = row.try_get("record")?;
            Ok(serde_json::from_value(record)?)
        })
        .transpose()
    }

    async fn page_approvals(
        &self,
        query: &ApprovalPageQuery,
    ) -> Result<Vec<ApprovalRequest>, HistoryError> {
        let (before_at, before_id) = match &query.before {
            Some((at_ms, id)) => (Some(to_i64(*at_ms)?), id.clone()),
            None => (None, String::new()),
        };
        let rows = sqlx::query(PAGE_APPROVALS)
            .bind(query.agent_id.as_deref())
            .bind(to_i64(query.since_ms)?)
            .bind(before_at)
            .bind(before_id)
            .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| -> Result<ApprovalRequest, HistoryError> {
                let record: serde_json::Value = row.try_get("record")?;
                Ok(serde_json::from_value(record)?)
            })
            .collect()
    }
```

4. In `delete_session` and `delete_agent`, replace `["history_messages", "history_runs", "history_attachments"]` with `["history_messages", "history_runs", "history_attachments", "history_approvals"]` (both loops), and in `delete_agent` replace `// Usage rows stay (spec §3.3).` with `// Usage rows stay (spec §3.3); approvals go with their agent.`.

- [ ] **Step 6: Flush decided approvals**

In `hosts/rust-daemon/src/state/approval_state.rs`, add to the `impl DaemonState` block:

```rust
    /// Decided approvals for the history store (spec §13.1), the `limit`
    /// oldest resolutions first. Those of deleted agents or sessions are
    /// dropped first: their history went with them, and writing them would
    /// bring it back. The snapshot never saves them, so nothing needs saving.
    pub(crate) fn unmirrored_decided_approvals(&mut self, limit: usize) -> Vec<ApprovalRequest> {
        let live_agents = self.live_agent_ids();
        let sessions = &self.sessions;
        self.approvals.retain_decided(|approval| {
            live_agents.contains(&approval.agent_id)
                && sessions.contains(&approval.agent_id, &approval.session_id)
        });
        self.approvals.unmirrored_decided(limit)
    }
```

In `hosts/rust-daemon/src/history/outbox.rs`:

1. Replace the module doc's first sentence `//! History outbox (spec §13.1): committed messages and terminal runs reach the` / `//! history store within about a second,` with `//! History outbox (spec §13.1): committed messages, terminal runs, and decided` / `//! approvals reach the history store within about a second,`.
2. After `HISTORY_RUN_BATCH`, add:

```rust
/// Decided approvals per store write.
pub(crate) const HISTORY_APPROVAL_BATCH: usize = 200;
```

3. Add `pub(crate) approvals: usize,` to `FlushReport` after `runs`.
4. In `flush_locked`, replace the last line `self.write_runs(state, transactions, report).await` with:

```text
        self.write_runs(state, transactions, report).await?;
        self.write_approvals(state, transactions, report).await
```

5. After `write_runs`, add:

```rust
    /// Writes decided approvals in batches, read under the control-plane
    /// transaction so only saved decisions are mirrored; each one the store
    /// then holds unchanged leaves the control plane (spec §7.3).
    async fn write_approvals(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        loop {
            let approvals = {
                let _transaction = transactions.lock().await;
                state
                    .write()
                    .await
                    .unmirrored_decided_approvals(HISTORY_APPROVAL_BATCH)
            };
            if approvals.is_empty() {
                return Ok(());
            }
            self.store.upsert_approvals(&approvals).await?;
            let removed = state.write().await.approvals.mark_mirrored(&approvals);
            report.approvals += removed;
            if removed == 0 || approvals.len() < HISTORY_APPROVAL_BATCH {
                return Ok(());
            }
        }
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history:: approvals:: state::approval_state`
Expected: PASS — the memory and SQLite conformance tests (now with approvals), the two new outbox tests, and every existing history test; the Postgres test stays ignored.

- [ ] **Step 8: Format and commit**

Run: `cargo fmt --all && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/history/mod.rs hosts/rust-daemon/src/history/memory.rs hosts/rust-daemon/src/history/sqlite.rs hosts/rust-daemon/src/history/postgres.rs hosts/rust-daemon/src/history/conformance.rs hosts/rust-daemon/src/history/outbox.rs hosts/rust-daemon/src/state/approval_state.rs
git commit -m "feat(daemon): move decided approvals to the history store through the outbox"
```

Recommended implementer tier: standard (follows the terminal-run mirroring pattern closely; the Postgres SQL is hand-checked only).

---

### Task 5: The gate in `execute_tool`: verdicts, the waiting call, decisions, timeouts, and helpers

**Files:**

- Create: `hosts/rust-daemon/src/approvals/gate.rs`, `hosts/rust-daemon/src/agent_runs/approvals.rs`, `hosts/rust-daemon/src/agent_runs/approval_tests.rs`, `hosts/rust-daemon/src/routes/contracts/approvals.rs`
- Modify: `hosts/rust-daemon/src/approvals/mod.rs` (module, re-exports, eight constants)
- Modify: `hosts/rust-daemon/src/tools.rs` (the `approvals` field, `with_approvals`, `live_checks`, the gate in `execute_tool`)
- Modify: `hosts/rust-daemon/src/agent_runs.rs` (module lines, two coordinator fields, `with_approval_timeouts`, one builder call in `run_locked`)
- Modify: `hosts/rust-daemon/src/live/events.rs` (`ApprovalRequested`, `ApprovalResolved`, `approval_json`)
- Modify: `hosts/rust-daemon/src/state/approval_state.rs` (`publish_approval`, `publish_run_status`, `publish_settled`)
- Modify: `hosts/rust-daemon/src/routes/contracts/mod.rs`, `hosts/rust-daemon/src/routes/mod.rs` (`ApprovalResponse` export)
- Modify: `hosts/rust-daemon/src/agent_runs/test_support.rs` (approval fixtures), `hosts/rust-daemon/src/agent_runs/stop_tests.rs` (the bash stop test allows `exec`)

**Interfaces:**

- Consumes: Task 3 `DaemonState::{approval_verdict, open_approval, revert_open_approval, settle_approval, revert_settled_approval}`, `ApprovalAsk`, `OwnerDecision`, `Settlement`, `SettleRefusal`; `AgentRunCoordinator::control_plane_transaction() -> OwnedMutexGuard<()>`; `ControlPlanePersistRequest::save()`; `HistoryStore::get_approval` (Task 4); `anima_core::CancelSignal::{cancelled(), is_cancelled()}`; `LiveHub::publish(event, parent: Option<&str>)`, `run_status_event(record)`, `DaemonState::live_parent_agent(agent_id, session_id)`; `ToolExecutionContext` (M3) and `crate::runs::RunLink`.
- Produces:
  - `approvals::gate::{ApprovalGate, GateOutcome, ApprovalTimeouts, ApprovalWaiters, PendingApproval, denial_text}` (all but `denial_text` re-exported from `approvals`):
    - `ApprovalGate::new(coordinator: AgentRunCoordinator, run: RunLink, source: RunSource, cancel: CancelSignal) -> Self`; `async fn check(&self, agent: &AgentState, call: &ToolCall) -> GateOutcome`.
    - `GateOutcome { Proceed, Approved, Refuse(TaskResult<Content>) }`.
    - `ApprovalTimeouts { default: Duration, telegram: Duration }` (`Default` = 30 min / 15 min), `for_source(self, source: RunSource) -> Duration`.
    - `ApprovalWaiters::{register(&self, id) -> oneshot::Receiver<ApprovalRequest>, forget(&self, id), wake(&self, approval: &ApprovalRequest)}`.
    - `PendingApproval { id: String, woken: oneshot::Receiver<ApprovalRequest> }`; `denial_text(note: Option<&str>) -> String`.
  - `AgentRunCoordinator` (in `agent_runs/approvals.rs`): `approval_verdict(&self, agent, session_id, call) -> Verdict`, `approval_timeout(&self, source) -> Duration`, `open_approval(&self, ask: ApprovalAsk) -> Result<PendingApproval, TaskResult<Content>>`, `settle_approval(&self, id: &str, settlement: Settlement) -> Option<ApprovalRequest>` (timeout or stop; `None` once the record left the control plane), `decide_approval(&self, id: &str, decision: OwnerDecision) -> Result<ApprovalRequest, ApiError>`; test-only `with_approval_timeouts(self, ApprovalTimeouts) -> Self`; fields `approval_waiters: ApprovalWaiters`, `approval_timeouts: ApprovalTimeouts`.
  - `ToolExecutionContext::with_approvals(self, gate: Option<ApprovalGate>) -> Self` (`run_locked` passes one for every coordinator run; swarm and direct contexts pass none).
  - `LiveEventBody::{ApprovalRequested(ApprovalRequest), ApprovalResolved(ApprovalRequest)}` (`approval.requested`, `approval.resolved`, with `approval`); `live::events::approval_json(&ApprovalRequest) -> Value`.
  - `DaemonState::{publish_approval(&self, approval), publish_run_status(&self, record), publish_settled(&self, settled: &SettledApproval)}`.
  - `routes::contracts::{ApprovalResponse, ApprovalMatcherResponse, ApprovalResolutionResponse}` with `From<&ApprovalRequest>`; `routes::ApprovalResponse` exported.
  - Constants in `approvals`: `APPROVAL_TIMEOUT_MS`, `TELEGRAM_APPROVAL_TIMEOUT_MS`, `DENIED_BY_OWNER`, `HELPER_NEEDS_APPROVAL`, `APPROVAL_NOT_SAVED`, `APPROVAL_LOST`, `APPROVAL_ALREADY_RESOLVED`, `APPROVAL_REVISION_STALE`.
- Behavior: `execute_tool` runs the live checks, refuses an unregistered tool, then asks the gate (spec §7.3). `Allow` dispatches; `Deny` answers `Denied by owner policy`; `Ask` from a helper answers `Needs owner approval; not available to helpers`; any other `Ask` opens a request (saved, then `run.awaiting_approval` if the run moved there, then `approval.requested`) and waits, holding no lock, for the first of: the owner's decision (woken through its oneshot), the run's stop signal, or the deadline (30 minutes, 15 for `RunSource::Telegram`). A timeout or a stop the stop path did not settle is settled by the waiter itself under the transaction; whoever flips the record first wins and the others see it resolved. `allowed` re-runs the live checks and dispatches unless the run is being stopped; `denied` answers `Denied by owner: <note>` (a timeout's note is `Approval timed out`); `stopped` answers `Cancelled before running (stopped by owner)`. The owner's decision is saved before its waiter is woken (a failed save reverts it and answers 503), is idempotent (the same decision again returns the record, from the history store once mirrored), conflicts on anything else (409), and needs the current revision. The M3 stop test that runs `bash` now allows `exec` for its agent, because `exec` asks by default.

- [ ] **Step 1: Write the failing tests**

Add to `hosts/rust-daemon/src/agent_runs/test_support.rs`:

1. Extend the imports: add `CancelSignal`, `Message`, `MessageRole` to the `anima_core::{…}` list, and add:

```text
use crate::approvals::{
    ApprovalDecisionKind, ApprovalGate, ApprovalPolicy, ApprovalRequest, ApprovalTimeouts,
};
use crate::runs::{RunLink, RunRecord, RunStart};
use crate::state::OwnerDecision;
use crate::tools::ToolExecutionContext;
```

2. At the end of the file, add:

```rust
/// A `memory_add` call (write class, spec §7.1) storing `text`.
pub(crate) fn remember_call(id: &str, text: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "memory_add".into(),
        args: BTreeMap::from([("content".to_string(), DataValue::String(text.into()))]),
    }
}

/// A coordinator whose one agent may use `calculate` and `memory_add`,
/// under `policy`, waiting at most `timeouts` for the owner.
pub(crate) async fn approving_coordinator(
    model: Arc<dyn ModelAdapter>,
    policy: ApprovalPolicy,
    timeouts: ApprovalTimeouts,
) -> (AgentRunCoordinator, String) {
    let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(model)));
    let agent_id = {
        let mut guard = state.write().await;
        let mut config = companion_config("companion");
        config.tools = Some(
            crate::tools::ToolRegistry::new()
                .resolve_descriptors(["calculate", "memory_add"])
                .unwrap(),
        );
        let agent_id = guard.create_agent(config).unwrap().state.id;
        guard.approvals.set_policy(&agent_id, policy);
        agent_id
    };
    (
        AgentRunCoordinator::new(state, Arc::new(Semaphore::new(8)))
            .with_approval_timeouts(timeouts),
        agent_id,
    )
}

/// Waits (up to five seconds) until at least `count` approvals are pending;
/// returns them oldest first.
pub(crate) async fn pending_approvals(
    coordinator: &AgentRunCoordinator,
    count: usize,
) -> Vec<ApprovalRequest> {
    for _ in 0..500 {
        let pending = coordinator
            .state
            .read()
            .await
            .approvals
            .pending()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if pending.len() >= count {
            return pending;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{count} approvals never became pending");
}

/// The owner's `kind` at `revision`, without a note or a matcher.
pub(crate) fn decision(kind: ApprovalDecisionKind, revision: u64) -> OwnerDecision {
    OwnerDecision {
        kind,
        note: None,
        matcher: None,
        revision,
    }
}

/// The texts of `agent_id`'s committed tool results, oldest first.
pub(crate) async fn tool_results(coordinator: &AgentRunCoordinator, agent_id: &str) -> Vec<String> {
    coordinator.state.read().await.agents[agent_id]
        .messages()
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .map(|message| message.content.text.clone())
        .collect()
}

/// A running ledger run of `agent_id` in a new chat `session_id`, for tool
/// calls made outside a coordinator run.
pub(crate) async fn ledger_run(
    coordinator: &AgentRunCoordinator,
    agent_id: &str,
    session_id: &str,
) -> RunLink {
    add_chat(coordinator, agent_id, session_id).await;
    let record = RunRecord::running(
        RunStart {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            source: RunSource::Web,
            source_ref: None,
            idempotency_key: None,
            text: "use a tool".into(),
            model: "gpt-5.4".into(),
            provider: None,
            parent_run_id: None,
        },
        anima_core::primitives::now_millis(),
    );
    let link = RunLink {
        run_id: record.id.clone(),
        session_id: session_id.into(),
        agent_id: agent_id.into(),
    };
    coordinator.state.write().await.runs.insert(record);
    link
}

/// A tool context for `run` with its approval gate, as `run_locked` builds one.
pub(crate) async fn gated_context(
    coordinator: &AgentRunCoordinator,
    run: &RunLink,
    cancel: CancelSignal,
) -> ToolExecutionContext {
    coordinator
        .state
        .read()
        .await
        .tool_execution_context()
        .with_team(coordinator.clone(), false)
        .with_run_link(Some(run.clone()))
        .with_cancel(Some(cancel.clone()))
        .with_approvals(Some(ApprovalGate::new(
            coordinator.clone(),
            run.clone(),
            RunSource::Web,
            cancel,
        )))
}

/// The user message a direct tool call is made for.
pub(crate) fn tool_input(agent_id: &str, room_id: &str) -> Message {
    Message {
        id: "msg-tool-input".into(),
        agent_id: agent_id.into(),
        room_id: room_id.into(),
        content: Content {
            text: "use a tool".into(),
            ..Content::default()
        },
        role: MessageRole::User,
        created_at_ms: 1,
    }
}
```

Create `hosts/rust-daemon/src/agent_runs/approval_tests.rs`:

```rust
//! Approvals in coordinator runs (spec §7.3): the gate, the waiting call,
//! decisions, timeouts, and helpers.

use std::time::Duration;

use anima_core::{AgentConfigUpdate, CancelSignal, MessageRole};
use axum::http::StatusCode;
use serde_json::{json, Value};

use super::test_support::{
    approving_coordinator, calculate_call, chat_request, companion_config, decision,
    events_until, gated_context, lead_config, ledger_run, pending_approvals, remember_call,
    tool_input, tool_results, Gate, ScriptedModel, Step,
};
use super::AgentRunCoordinator;
use crate::approvals::{
    ApprovalDecisionKind, ApprovalMatcher, ApprovalPolicy, ApprovalRule, ApprovalStatus,
    ApprovalTimeouts, MatcherKind, PolicyAction, ResolvedBy, RiskClass, Verdict,
    APPROVAL_ALREADY_RESOLVED, APPROVAL_NOT_SAVED, APPROVAL_REVISION_STALE, APPROVAL_TIMEOUT_MS,
    DENIED_BY_POLICY, HELPER_NEEDS_APPROVAL, TELEGRAM_APPROVAL_TIMEOUT_MS,
};
use crate::runs::{RunSource, RunStatus};
use crate::state::OwnerDecision;

fn ask_before_writes() -> ApprovalPolicy {
    ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask)
}

fn patient() -> ApprovalTimeouts {
    ApprovalTimeouts {
        default: Duration::from_secs(30),
        telegram: Duration::from_secs(15),
    }
}

fn quick() -> ApprovalTimeouts {
    ApprovalTimeouts {
        default: Duration::from_millis(250),
        telegram: Duration::from_millis(250),
    }
}

fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_string())
        .collect()
}

fn spawn_run(
    coordinator: &AgentRunCoordinator,
    request: super::AgentRunRequest,
) -> tokio::task::JoinHandle<Result<crate::routes::AgentRunEnvelope, crate::routes::ApiError>> {
    let coordinator = coordinator.clone();
    tokio::spawn(async move { coordinator.run(request).await })
}

/// The one tool result `agent_id` committed.
async fn only_result(coordinator: &AgentRunCoordinator, agent_id: &str) -> String {
    let mut results = tool_results(coordinator, agent_id).await;
    assert_eq!(results.len(), 1, "{results:?}");
    results.remove(0)
}

#[tokio::test]
async fn a_denied_class_never_runs_and_the_model_hears_why() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Understood"]),
    ]);
    let deny_writes = ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Deny);
    let (coordinator, agent_id) = approving_coordinator(model.clone(), deny_writes, patient()).await;

    coordinator
        .run(chat_request(&agent_id, "chat:ask", "remember the plan"))
        .await
        .unwrap();

    assert!(only_result(&coordinator, &agent_id).await.contains(DENIED_BY_POLICY));
    assert!(coordinator.state.read().await.approvals.pending().is_empty());
    assert!(model.requests()[1].messages.iter().any(|message| {
        message.role == MessageRole::Tool && message.content.text.contains(DENIED_BY_POLICY)
    }));
}

#[tokio::test]
async fn read_tools_never_ask_even_when_every_class_is_denied() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![calculate_call("call-1", "2+2")]),
        Step::Text(vec!["Four"]),
    ]);
    let deny_all = ApprovalPolicy {
        write: PolicyAction::Deny,
        exec: PolicyAction::Deny,
        network: PolicyAction::Deny,
        delegate: PolicyAction::Deny,
    };
    let (coordinator, agent_id) = approving_coordinator(model, deny_all, patient()).await;

    coordinator
        .run(chat_request(&agent_id, "chat:math", "add"))
        .await
        .unwrap();

    let result = only_result(&coordinator, &agent_id).await;
    assert!(result.contains('4'), "{result}");
    assert!(!result.contains(DENIED_BY_POLICY));
}

#[tokio::test]
async fn an_asked_call_waits_for_the_owner_and_runs_once_allowed() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Saved"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:ask", "remember"));

    let asked = events_until(&mut subscription, "approval.requested").await;
    let asked_types = types(&asked);
    let awaiting = asked_types
        .iter()
        .position(|kind| kind == "run.awaiting_approval")
        .expect("the run awaits approval");
    assert_eq!(awaiting, asked_types.len() - 2, "then the request is announced");
    assert!(asked_types[..awaiting].contains(&"tool.started".to_string()));
    assert_eq!(asked[awaiting]["run"]["status"], "awaiting_approval");
    let announced = &asked[asked.len() - 1];
    assert_eq!(announced["sessionId"], "chat:ask");
    let approval = &announced["approval"];
    assert_eq!(approval["tool"], "memory_add");
    assert_eq!(approval["class"], "write");
    assert_eq!(approval["status"], "pending");
    assert_eq!(approval["revision"], 1);
    assert_eq!(approval["toolCallId"], "call-1");
    assert_eq!(approval["arguments"], "{\"content\":\"the plan\"}");
    assert_eq!(approval["argumentsTruncated"], false);
    assert_eq!(approval["matcherKinds"], json!(["any"]));
    assert_eq!(approval["suggestedMatcher"], json!({"kind": "any", "value": ""}));
    assert_eq!(approval["resolution"], Value::Null);

    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(pending.expires_at_ms - pending.created_at_ms, 30_000);
    assert_eq!(
        coordinator.state.read().await.runs.get(&pending.run_id).unwrap().status,
        RunStatus::AwaitingApproval
    );

    let allowed = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    assert_eq!(allowed.status, ApprovalStatus::Allowed);
    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error, None);
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );

    let after = events_until(&mut subscription, "run.completed").await;
    let after_types = types(&after);
    let resolved = after_types
        .iter()
        .position(|kind| kind == "approval.resolved")
        .unwrap();
    assert_eq!(after[resolved]["approval"]["status"], "allowed");
    assert_eq!(after[resolved]["approval"]["resolution"]["decision"], "allow_once");
    assert_eq!(after_types[resolved + 1], "run.started", "the run resumes");
    assert_eq!(after[resolved + 1]["run"]["status"], "running");
    assert_eq!(
        after_types
            .iter()
            .filter(|kind| *kind == "approval.resolved")
            .count(),
        1
    );
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test]
async fn a_denial_reaches_the_model_with_the_owners_note() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["I won't"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:ask", "remember"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    coordinator
        .decide_approval(
            &pending.id,
            OwnerDecision {
                kind: ApprovalDecisionKind::Deny,
                note: Some("not now".into()),
                matcher: None,
                revision: 1,
            },
        )
        .await
        .unwrap();
    running.await.unwrap().unwrap();

    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner: not now"));
    assert!(model.requests()[1].messages.iter().any(|message| {
        message.role == MessageRole::Tool && message.content.text.contains("Denied by owner: not now")
    }));
}

#[tokio::test]
async fn allow_for_the_session_skips_the_next_ask_there_and_only_there() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "one")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-2", "two")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-3", "three")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), patient()).await;

    let first = spawn_run(&coordinator, chat_request(&agent_id, "chat:one", "first"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowSession, 1))
        .await
        .unwrap();
    first.await.unwrap().unwrap();

    tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.run(chat_request(&agent_id, "chat:one", "second")),
    )
    .await
    .expect("the session's allowance covers the second call")
    .unwrap();

    let third = spawn_run(&coordinator, chat_request(&agent_id, "chat:two", "third"));
    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.session_id, "chat:two", "another session still asks");
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    third.await.unwrap().unwrap();
}

#[tokio::test]
async fn always_allow_creates_a_rule_that_later_runs_follow() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "one")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-2", "two")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), patient()).await;

    let first = spawn_run(&coordinator, chat_request(&agent_id, "chat:one", "first"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    let always = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowAlways, 1))
        .await
        .unwrap();
    first.await.unwrap().unwrap();
    {
        let guard = coordinator.state.read().await;
        let rules = guard.approvals.rules_for(&agent_id);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].tool, "memory_add");
        assert_eq!(rules[0].matcher, ApprovalMatcher::any());
        assert_eq!(rules[0].from_approval_id.as_deref(), Some(pending.id.as_str()));
        assert_eq!(
            always.resolution.unwrap().rule_id.as_deref(),
            Some(rules[0].id.as_str())
        );
    }

    tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.run(chat_request(&agent_id, "chat:two", "second")),
    )
    .await
    .expect("the rule covers every session")
    .unwrap();
}

#[tokio::test]
async fn a_timeout_denies_the_call_and_a_late_decision_conflicts() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), quick()).await;

    tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.run(chat_request(&agent_id, "chat:ask", "remember")),
    )
    .await
    .expect("the timeout ends the wait")
    .unwrap();

    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner: Approval timed out"));
    let timed_out = coordinator.state.read().await.approvals.decided()[0].clone();
    assert_eq!(timed_out.status, ApprovalStatus::Denied);
    assert_eq!(
        timed_out.resolution.as_ref().unwrap().resolved_by,
        ResolvedBy::Timeout
    );
    let late = coordinator
        .decide_approval(&timed_out.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap_err();
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(late.message(), APPROVAL_ALREADY_RESOLVED);
}

#[tokio::test]
async fn telegram_started_runs_wait_fifteen_minutes_and_others_thirty() {
    assert_eq!(APPROVAL_TIMEOUT_MS, 30 * 60 * 1000);
    assert_eq!(TELEGRAM_APPROVAL_TIMEOUT_MS, 15 * 60 * 1000);
    assert_eq!(
        ApprovalTimeouts::default(),
        ApprovalTimeouts {
            default: Duration::from_millis(APPROVAL_TIMEOUT_MS),
            telegram: Duration::from_millis(TELEGRAM_APPROVAL_TIMEOUT_MS),
        }
    );
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "one")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-2", "two")]),
        Step::Text(vec!["ok"]),
    ]);
    let timeouts = ApprovalTimeouts {
        default: Duration::from_secs(60),
        telegram: Duration::from_secs(20),
    };
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), timeouts).await;

    let mut telegram = chat_request(&agent_id, "chat:telegram", "from telegram");
    telegram.source = RunSource::Telegram;
    let running = spawn_run(&coordinator, telegram);
    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.expires_at_ms - asked.created_at_ms, 20_000);
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();

    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:api", "from the api"));
    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.expires_at_ms - asked.created_at_ms, 60_000);
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_timeout_that_queued_first_beats_a_later_decision() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), quick()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:race", "remember"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // The waiter's timeout queues on the transaction first (tokio's mutex is
    // fair), then the owner's decision.
    let held = coordinator.control_plane_transaction().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let deciding = {
        let coordinator = coordinator.clone();
        let id = pending.id.clone();
        tokio::spawn(async move {
            coordinator
                .decide_approval(&id, decision(ApprovalDecisionKind::AllowOnce, 1))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(held);

    let refused = deciding.await.unwrap().unwrap_err();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(refused.message(), APPROVAL_ALREADY_RESOLVED);
    running.await.unwrap().unwrap();
    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner: Approval timed out"));
    let events = events_until(&mut subscription, "run.completed").await;
    assert_eq!(
        types(&events)
            .iter()
            .filter(|kind| *kind == "approval.resolved")
            .count(),
        1
    );
}

#[tokio::test]
async fn a_decision_that_queued_first_beats_the_timeout() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), quick()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:race", "remember"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let held = coordinator.control_plane_transaction().await;
    let deciding = {
        let coordinator = coordinator.clone();
        let id = pending.id.clone();
        tokio::spawn(async move {
            coordinator
                .decide_approval(&id, decision(ApprovalDecisionKind::AllowOnce, 1))
                .await
        })
    };
    // The decision queues first; the timeout fires meanwhile and queues
    // behind it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(held);

    let allowed = deciding.await.unwrap().unwrap();
    assert_eq!(allowed.status, ApprovalStatus::Allowed);
    running.await.unwrap().unwrap();
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );
    let events = events_until(&mut subscription, "run.completed").await;
    assert_eq!(
        types(&events)
            .iter()
            .filter(|kind| *kind == "approval.resolved")
            .count(),
        1
    );
}

#[tokio::test]
async fn helpers_are_denied_instead_of_waiting() {
    let (coordinator, companion) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), patient()).await;
    let helper = {
        let mut guard = coordinator.state.write().await;
        let parent = guard.get_agent(&companion).unwrap().state;
        guard
            .create_agent(super::helper_config(&parent, "Helper".into()))
            .unwrap()
            .state
    };
    let run = ledger_run(&coordinator, &helper.id, "room-helper").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new()).await;

    let result = context
        .execute_tool(
            helper.clone(),
            tool_input(&helper.id, "room-helper"),
            remember_call("call-1", "x"),
        )
        .await;

    assert_eq!(result.error.as_deref(), Some(HELPER_NEEDS_APPROVAL));
    assert!(coordinator.state.read().await.approvals.pending().is_empty());
}

#[tokio::test]
async fn an_approved_call_is_checked_again_before_it_runs() {
    let (coordinator, _) =
        approving_coordinator(ScriptedModel::new(vec![]), ApprovalPolicy::default(), patient())
            .await;
    let (lead_id, specialist) = {
        let mut guard = coordinator.state.write().await;
        let mut lead = lead_config("Lead");
        lead.tools = Some(
            crate::tools::ToolRegistry::new()
                .resolve_descriptors(["memory_add"])
                .unwrap(),
        );
        let lead_id = guard.create_agent(lead).unwrap().state.id;
        let mut specialist = companion_config("Specialist");
        specialist.tools = lead_tools(&guard, &lead_id);
        let specialist = guard.create_agent(specialist).unwrap().state;
        guard.approvals.set_policy(&specialist.id, ask_before_writes());
        (lead_id, specialist)
    };
    let run = ledger_run(&coordinator, &specialist.id, "room-delegated").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new())
        .await
        .with_delegated_parent(Some(lead_id.clone()));
    let calling = {
        let specialist = specialist.clone();
        tokio::spawn(async move {
            context
                .execute_tool(
                    specialist.clone(),
                    tool_input(&specialist.id, "room-delegated"),
                    remember_call("call-1", "x"),
                )
                .await
        })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // While the owner decides, the manager loses the tool (spec §7.3: the
    // live checks run again after an approval).
    coordinator
        .state
        .write()
        .await
        .update_agent(
            &lead_id,
            AgentConfigUpdate {
                tools: Some(vec![]),
                ..Default::default()
            },
        )
        .unwrap();
    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();

    let result = calling.await.unwrap();
    assert_eq!(
        result.error.as_deref(),
        Some("The manager no longer has permission for this delegated tool")
    );
}

fn lead_tools(
    guard: &crate::state::DaemonState,
    lead_id: &str,
) -> Option<Vec<anima_core::ToolDescriptor>> {
    guard.get_agent(lead_id).unwrap().state.config.tools
}

#[tokio::test]
async fn a_request_that_cannot_be_saved_never_runs_its_tool() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![remember_call("call-1", "the plan")]),
            Step::Text(vec!["ok"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), patient()).await;
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:ask", "remember"));
    // The run's start is saved; the next save is the request's, and it fails.
    gate.entered().await;
    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    gate.release();
    tokio::time::timeout(Duration::from_secs(5), save.entered.acquire())
        .await
        .expect("the request is saved")
        .unwrap()
        .forget();
    save.release.add_permits(1);
    gate.entered().await;
    gate.release();

    running.await.unwrap().unwrap();
    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains(APPROVAL_NOT_SAVED));
    let guard = coordinator.state.read().await;
    assert!(guard.approvals.pending().is_empty());
    assert!(guard.approvals.decided().is_empty());
}

#[tokio::test]
async fn repeating_a_decision_is_idempotent_and_a_different_one_conflicts() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), patient()).await;
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:ask", "remember"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let stale = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 2))
        .await
        .unwrap_err();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(stale.message(), APPROVAL_REVISION_STALE);

    let first = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    let replay = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    assert_eq!(replay, first, "the same decision again changes nothing");
    let other = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap_err();
    assert_eq!(other.status(), StatusCode::CONFLICT);
    assert_eq!(other.message(), APPROVAL_ALREADY_RESOLVED);
    let missing = coordinator
        .decide_approval("apr_missing", decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap_err();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn two_calls_in_one_batch_wait_together_and_the_run_resumes_after_both() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![
            remember_call("call-1", "one"),
            remember_call("call-2", "two"),
        ]),
        Step::Text(vec!["both"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:batch", "remember both"));
    let pending = pending_approvals(&coordinator, 2).await;

    coordinator
        .decide_approval(&pending[0].id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&pending[0].run_id)
            .unwrap()
            .status,
        RunStatus::AwaitingApproval,
        "the other call still waits"
    );
    coordinator
        .decide_approval(&pending[1].id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    assert_eq!(results.len(), 2);
    assert_eq!(
        results
            .iter()
            .filter(|text| text.starts_with("stored memory: "))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|text| text.contains("Denied by owner"))
            .count(),
        1
    );
    let events = types(&events_until(&mut subscription, "run.completed").await);
    let count = |kind: &str| events.iter().filter(|event| *event == kind).count();
    assert_eq!(count("run.awaiting_approval"), 1);
    assert_eq!(count("approval.requested"), 2);
    assert_eq!(count("approval.resolved"), 2);
    assert_eq!(count("run.started"), 2, "the start, then one resume");
}

#[tokio::test]
async fn a_rule_never_covers_a_command_with_shell_operators() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![anima_core::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            args: std::collections::BTreeMap::from([(
                "command".to_string(),
                anima_core::DataValue::String("git status; rm -rf ~".into()),
            )]),
        }]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ApprovalPolicy::default(), patient()).await;
    {
        let mut guard = coordinator.state.write().await;
        guard
            .update_agent(
                &agent_id,
                AgentConfigUpdate {
                    tools: Some(
                        crate::tools::ToolRegistry::new()
                            .resolve_descriptors(["bash"])
                            .unwrap(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap();
        guard
            .approvals
            .add_rule(ApprovalRule {
                id: "rule_git".into(),
                agent_id: agent_id.clone(),
                tool: "bash".into(),
                matcher: ApprovalMatcher {
                    kind: MatcherKind::CommandPrefix,
                    value: "git status".into(),
                },
                created_at_ms: 1,
                from_approval_id: None,
            })
            .unwrap();
        let agent = guard.get_agent(&agent_id).unwrap().state;
        let plain = anima_core::ToolCall {
            id: "call-0".into(),
            name: "bash".into(),
            args: std::collections::BTreeMap::from([(
                "command".to_string(),
                anima_core::DataValue::String("git status --short".into()),
            )]),
        };
        assert_eq!(
            guard.approval_verdict(&agent, "chat:shell", &plain),
            Verdict::Allow,
            "the rule covers the plain command"
        );
    }
    let running = spawn_run(&coordinator, chat_request(&agent_id, "chat:shell", "status"));

    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.tool, "bash");
    assert!(asked.arguments.contains("git status; rm -rf ~"));
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();
    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner"));
}
```

In `hosts/rust-daemon/src/agent_runs.rs`, add next to the other test modules (first, alphabetically):

```text
#[cfg(test)]
mod approval_tests;
```

In `hosts/rust-daemon/src/agent_runs/stop_tests.rs`, in `a_stop_kills_the_bash_command_its_run_waits_for`, replace the last line of the block that creates the agent, `guard.create_agent(config).unwrap().state.id`, with:

```text
        let agent_id = guard.create_agent(config).unwrap().state.id;
        // `exec` asks by default (spec §7.2); this owner lets it run.
        guard.approvals.set_policy(
            &agent_id,
            crate::approvals::ApprovalPolicy::default().with(
                crate::approvals::RiskClass::Exec,
                crate::approvals::PolicyAction::Allow,
            ),
        );
        agent_id
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::approval_tests`
Expected: FAIL to compile — `ApprovalGate`, `ApprovalTimeouts`, `with_approval_timeouts`, `with_approvals`, `decide_approval`, and the new constants do not exist.

- [ ] **Step 3: Add the constants, the response body, and the events**

In `hosts/rust-daemon/src/approvals/mod.rs`:

1. After `pub(crate) mod registry;` add `pub(crate) mod gate;`, and after the `registry` re-export add:

```text
#[allow(unused_imports)] // M4 Tasks 6 and 8 use the rest.
pub(crate) use gate::{
    ApprovalGate, ApprovalTimeouts, ApprovalWaiters, GateOutcome, PendingApproval,
};
```

2. After `APPROVAL_SESSION_GONE` add:

```text
/// How long a call waits for the owner (spec §7.3, §16)...
pub(crate) const APPROVAL_TIMEOUT_MS: u64 = 30 * 60 * 1000;
/// ...or, for a Telegram-started run, whose connector handles one message
/// at a time.
pub(crate) const TELEGRAM_APPROVAL_TIMEOUT_MS: u64 = 15 * 60 * 1000;
/// A denial's tool result, followed by `": <note>"` when there is a note.
pub(crate) const DENIED_BY_OWNER: &str = "Denied by owner";
/// Helpers never wait (spec §7.3).
pub(crate) const HELPER_NEEDS_APPROVAL: &str = "Needs owner approval; not available to helpers";
pub(crate) const APPROVAL_NOT_SAVED: &str =
    "Needs owner approval, but the request could not be saved; the tool did not run";
pub(crate) const APPROVAL_LOST: &str = "The approval request was lost; the tool did not run";
pub(crate) const APPROVAL_ALREADY_RESOLVED: &str = "This approval was already resolved";
pub(crate) const APPROVAL_REVISION_STALE: &str =
    "This approval changed; reload it and decide again";
```

Create `hosts/rust-daemon/src/routes/contracts/approvals.rs`:

```rust
//! Approval bodies (spec §7.3).

use serde::Serialize;
use utoipa::ToSchema;

use crate::approvals::{matcher_kinds, ApprovalMatcher, ApprovalRequest, ApprovalResolution};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalMatcherResponse {
    /// `command_prefix`, `path_glob`, `domain`, or `any`.
    pub(crate) kind: String,
    /// Empty for `any`.
    pub(crate) value: String,
}

impl From<&ApprovalMatcher> for ApprovalMatcherResponse {
    fn from(matcher: &ApprovalMatcher) -> Self {
        Self {
            kind: matcher.kind.as_str().into(),
            value: matcher.value.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalResolutionResponse {
    /// `allow_once`, `allow_session`, `allow_always`, or `deny`; `null` for
    /// a stopped or expired request.
    pub(crate) decision: Option<String>,
    pub(crate) note: Option<String>,
    /// The allowance's or rule's matcher.
    pub(crate) matcher: Option<ApprovalMatcherResponse>,
    pub(crate) rule_id: Option<String>,
    /// `owner`, `timeout`, `stop`, or `restart`.
    pub(crate) resolved_by: String,
    pub(crate) resolved_at_ms: u64,
}

impl From<&ApprovalResolution> for ApprovalResolutionResponse {
    fn from(resolution: &ApprovalResolution) -> Self {
        Self {
            decision: resolution.decision.map(|decision| decision.as_str().into()),
            note: resolution.note.clone(),
            matcher: resolution.matcher.as_ref().map(ApprovalMatcherResponse::from),
            rule_id: resolution.rule_id.clone(),
            resolved_by: resolution.resolved_by.as_str().into(),
            resolved_at_ms: resolution.resolved_at_ms,
        }
    }
}

/// A call waiting for, or decided by, the owner (spec §7.3).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool: String,
    /// `write`, `exec`, `network`, or `delegate`.
    pub(crate) class: String,
    /// The call's arguments as JSON text, at most 16 KiB. The model wrote
    /// them: show them as text, never as markup.
    pub(crate) arguments: String,
    pub(crate) arguments_truncated: bool,
    pub(crate) suggested_matcher: ApprovalMatcherResponse,
    /// The matcher kinds an allowance or rule for this tool may use.
    pub(crate) matcher_kinds: Vec<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) expires_at_ms: u64,
    /// `pending`, `allowed`, `denied`, `stopped`, or `expired`.
    pub(crate) status: String,
    /// Send it back with a decision.
    pub(crate) revision: u64,
    pub(crate) resolution: Option<ApprovalResolutionResponse>,
}

impl From<&ApprovalRequest> for ApprovalResponse {
    fn from(approval: &ApprovalRequest) -> Self {
        Self {
            id: approval.id.clone(),
            agent_id: approval.agent_id.clone(),
            session_id: approval.session_id.clone(),
            run_id: approval.run_id.clone(),
            tool_call_id: approval.tool_call_id.clone(),
            tool: approval.tool.clone(),
            class: approval.class.as_str().into(),
            arguments: approval.arguments.clone(),
            arguments_truncated: approval.arguments_truncated,
            suggested_matcher: ApprovalMatcherResponse::from(&approval.suggested_matcher),
            matcher_kinds: matcher_kinds(&approval.tool)
                .iter()
                .map(|kind| kind.as_str().to_string())
                .collect(),
            created_at_ms: approval.created_at_ms,
            expires_at_ms: approval.expires_at_ms,
            status: approval.status.as_str().into(),
            revision: approval.revision,
            resolution: approval
                .resolution
                .as_ref()
                .map(ApprovalResolutionResponse::from),
        }
    }
}
```

In `hosts/rust-daemon/src/routes/contracts/mod.rs`, add `mod approvals;` first in the module list and `pub(crate) use approvals::*;` before `pub(crate) use connectors::*;`. In `hosts/rust-daemon/src/routes/mod.rs`, replace

```text
pub(crate) use self::contracts::{
    AgentRunEnvelope, AgentRuntimeSnapshotResponse, RunResponse, TaskResultResponse,
};
```

with

```text
pub(crate) use self::contracts::{
    AgentRunEnvelope, AgentRuntimeSnapshotResponse, ApprovalResponse, RunResponse,
    TaskResultResponse,
};
```

In `hosts/rust-daemon/src/live/events.rs`:

1. Replace `use crate::routes::RunResponse;` with `use crate::routes::{ApprovalResponse, RunResponse};` and add `use crate::approvals::ApprovalRequest;`.
2. Add to `LiveEventBody`, after `ToolFinished { … }`:

```text
    ApprovalRequested(ApprovalRequest),
    ApprovalResolved(ApprovalRequest),
```

3. Add to `type_name`'s match: `Self::ApprovalRequested(_) => "approval.requested",` and `Self::ApprovalResolved(_) => "approval.resolved",`.
4. Add to `to_json`'s match, after the `ToolFinished` arm:

```text
            LiveEventBody::ApprovalRequested(approval) | LiveEventBody::ApprovalResolved(approval) => {
                value["approval"] = approval_json(approval);
            }
```

5. After `run_json`, add:

```rust
/// An approval as streams and routes show it (spec §6, §7.3).
pub(crate) fn approval_json(approval: &ApprovalRequest) -> Value {
    serde_json::to_value(ApprovalResponse::from(approval)).unwrap_or(Value::Null)
}
```

In `hosts/rust-daemon/src/state/approval_state.rs`, add `use crate::live::{run_status_event, LiveEvent, LiveEventBody};` and add to the `impl DaemonState` block:

```rust
    /// `approval.requested` for a pending approval, otherwise
    /// `approval.resolved`, to its agent's stream and to the stream that
    /// carries its session (spec §6).
    pub(crate) fn publish_approval(&self, approval: &ApprovalRequest) {
        let body = if approval.is_pending() {
            LiveEventBody::ApprovalRequested(approval.clone())
        } else {
            LiveEventBody::ApprovalResolved(approval.clone())
        };
        let parent = self.live_parent_agent(&approval.agent_id, &approval.session_id);
        self.live.publish(
            LiveEvent::new(&approval.agent_id, body)
                .session(&approval.session_id)
                .run(&approval.run_id),
            parent.as_deref(),
        );
    }

    /// `record`'s lifecycle event for its current status: a request's
    /// `run.awaiting_approval`, or a resume's `run.started`.
    pub(crate) fn publish_run_status(&self, record: &RunRecord) {
        let parent = self.live_parent_agent(&record.agent_id, &record.session_id);
        self.live.publish(run_status_event(record), parent.as_deref());
    }

    /// `approval.resolved`, then `run.started` when the run resumed.
    pub(crate) fn publish_settled(&self, settled: &SettledApproval) {
        self.publish_approval(&settled.approval);
        if let Some(run) = &settled.run {
            self.publish_run_status(run);
        }
    }
```

- [ ] **Step 4: Implement the gate and the waiting call**

Create `hosts/rust-daemon/src/approvals/gate.rs`:

```rust
//! The gate in `ToolExecutionContext::execute_tool` (spec §7.3): judges each
//! call, and for one that needs the owner saves a request and waits for its
//! decision, the run's stop, or the timeout, whichever settles it first. The
//! waiting call holds no lock; every settlement takes the control-plane
//! transaction, so exactly one of them wins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use anima_core::{
    AgentState, CancelSignal, Content, TaskResult, ToolCall, CANCELLED_TOOL_RESULT,
};
use tokio::sync::oneshot;

use super::{
    ApprovalRequest, ApprovalStatus, Verdict, APPROVAL_LOST, APPROVAL_TIMEOUT_MS,
    DENIED_BY_OWNER, DENIED_BY_POLICY, HELPER_NEEDS_APPROVAL, TELEGRAM_APPROVAL_TIMEOUT_MS,
};
use crate::agent_runs::{is_helper_config, AgentRunCoordinator};
use crate::runs::{RunLink, RunSource};
use crate::state::{ApprovalAsk, Settlement};

/// How long a call waits for the owner (spec §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalTimeouts {
    pub(crate) default: Duration,
    /// For Telegram-started runs: the connector handles one message at a time.
    pub(crate) telegram: Duration,
}

impl Default for ApprovalTimeouts {
    fn default() -> Self {
        Self {
            default: Duration::from_millis(APPROVAL_TIMEOUT_MS),
            telegram: Duration::from_millis(TELEGRAM_APPROVAL_TIMEOUT_MS),
        }
    }
}

impl ApprovalTimeouts {
    pub(crate) fn for_source(self, source: RunSource) -> Duration {
        if source == RunSource::Telegram {
            self.telegram
        } else {
            self.default
        }
    }
}

type Senders = HashMap<String, oneshot::Sender<ApprovalRequest>>;

/// The calls waiting for the owner, by approval id. Whoever settles an
/// approval sends the settled record to its one waiter. A leaf lock, never
/// held across `.await` or while taking the state lock.
#[derive(Clone, Default)]
pub(crate) struct ApprovalWaiters {
    senders: Arc<StdMutex<Senders>>,
}

impl ApprovalWaiters {
    fn senders(&self) -> MutexGuard<'_, Senders> {
        self.senders
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The wake-up of approval `id`; registered before the request is saved.
    pub(crate) fn register(&self, id: &str) -> oneshot::Receiver<ApprovalRequest> {
        let (sender, receiver) = oneshot::channel();
        self.senders().insert(id.to_string(), sender);
        receiver
    }

    pub(crate) fn forget(&self, id: &str) {
        self.senders().remove(id);
    }

    /// Sends the settled record to its waiter, once; a later call finds none.
    pub(crate) fn wake(&self, approval: &ApprovalRequest) {
        let sender = self.senders().remove(&approval.id);
        if let Some(sender) = sender {
            let _ = sender.send(approval.clone());
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.senders().len()
    }
}

/// A saved and announced request, and the wake-up its settlement sends.
pub(crate) struct PendingApproval {
    pub(crate) id: String,
    pub(crate) woken: oneshot::Receiver<ApprovalRequest>,
}

/// A denial's tool result: `Denied by owner: <note>`, or `Denied by owner`.
pub(crate) fn denial_text(note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{DENIED_BY_OWNER}: {note}"),
        None => DENIED_BY_OWNER.to_string(),
    }
}

/// What `execute_tool` does with a call once the gate has judged it.
pub(crate) enum GateOutcome {
    /// Allowed without asking: dispatch.
    Proceed,
    /// The owner allowed it after a wait: check again, then dispatch.
    Approved,
    /// Answer the model with this instead of running the tool.
    Refuse(TaskResult<Content>),
}

/// One coordinator run's gate (spec §7.3).
#[derive(Clone)]
pub(crate) struct ApprovalGate {
    coordinator: AgentRunCoordinator,
    run: RunLink,
    source: RunSource,
    cancel: CancelSignal,
}

impl ApprovalGate {
    pub(crate) fn new(
        coordinator: AgentRunCoordinator,
        run: RunLink,
        source: RunSource,
        cancel: CancelSignal,
    ) -> Self {
        Self {
            coordinator,
            run,
            source,
            cancel,
        }
    }

    pub(crate) async fn check(&self, agent: &AgentState, call: &ToolCall) -> GateOutcome {
        match self
            .coordinator
            .approval_verdict(agent, &self.run.session_id, call)
            .await
        {
            Verdict::Allow => GateOutcome::Proceed,
            Verdict::Deny => GateOutcome::Refuse(TaskResult::error(DENIED_BY_POLICY, 0)),
            // Helpers never wait (spec §7.3).
            Verdict::Ask if is_helper_config(&agent.config) => {
                GateOutcome::Refuse(TaskResult::error(HELPER_NEEDS_APPROVAL, 0))
            }
            Verdict::Ask => self.ask(call).await,
        }
    }

    async fn ask(&self, call: &ToolCall) -> GateOutcome {
        let timeout = self.coordinator.approval_timeout(self.source);
        let ask = ApprovalAsk {
            run: self.run.clone(),
            call: call.clone(),
            timeout_ms: timeout.as_millis() as u64,
        };
        let pending = match self.coordinator.open_approval(ask).await {
            Ok(pending) => pending,
            Err(refused) => return GateOutcome::Refuse(refused),
        };
        match self.wait(pending, timeout).await {
            Some(approval) => self.outcome(&approval),
            None => GateOutcome::Refuse(TaskResult::error(APPROVAL_LOST, 0)),
        }
    }

    /// The approval as it was settled, by whoever settled it first.
    async fn wait(&self, pending: PendingApproval, timeout: Duration) -> Option<ApprovalRequest> {
        let PendingApproval { id, mut woken } = pending;
        let deadline = tokio::time::Instant::now() + timeout;
        let settled = tokio::select! {
            biased;
            settled = &mut woken => settled.ok(),
            () = self.cancel.cancelled() => None,
            () = tokio::time::sleep_until(deadline) => None,
        };
        if settled.is_some() {
            return settled;
        }
        let settlement = if self.cancel.is_cancelled() {
            Settlement::Stopped
        } else {
            Settlement::TimedOut
        };
        match self.coordinator.settle_approval(&id, settlement).await {
            Some(approval) => Some(approval),
            // Settled first by someone whose record already left the control
            // plane; that settlement sent it here before it let go of the
            // transaction this settle waited for.
            None => woken.try_recv().ok(),
        }
    }

    fn outcome(&self, approval: &ApprovalRequest) -> GateOutcome {
        match approval.status {
            ApprovalStatus::Allowed if !self.cancel.is_cancelled() => GateOutcome::Approved,
            ApprovalStatus::Denied => GateOutcome::Refuse(TaskResult::error(
                denial_text(
                    approval
                        .resolution
                        .as_ref()
                        .and_then(|resolution| resolution.note.as_deref()),
                ),
                0,
            )),
            // Allowed, but the run is being stopped: nothing new starts.
            ApprovalStatus::Allowed | ApprovalStatus::Stopped => {
                GateOutcome::Refuse(TaskResult::error(CANCELLED_TOOL_RESULT, 0))
            }
            ApprovalStatus::Pending | ApprovalStatus::Expired => {
                GateOutcome::Refuse(TaskResult::error(APPROVAL_LOST, 0))
            }
        }
    }
}
```

Create `hosts/rust-daemon/src/agent_runs/approvals.rs`:

```rust
//! The coordinator's side of approvals (spec §7.3): asking, settling, and
//! the owner's decision, each under the control-plane transaction, saved
//! before the waiting call is woken and before anything is announced.

use std::time::Duration;

use anima_core::primitives::now_millis;
use anima_core::{AgentState, Content, TaskResult, ToolCall};
use tracing::warn;

use super::AgentRunCoordinator;
use crate::approvals::{
    ApprovalRequest, PendingApproval, Verdict, APPROVAL_ALREADY_RESOLVED, APPROVAL_NOT_SAVED,
    APPROVAL_REVISION_STALE,
};
use crate::routes::ApiError;
use crate::runs::RunSource;
use crate::state::{ApprovalAsk, OwnerDecision, SettleRefusal, Settlement};

/// A decided approval: the same owner decision again is answered with it
/// (spec §7.3, idempotent); anything else conflicts.
fn replay_or_conflict(
    record: ApprovalRequest,
    decision: &OwnerDecision,
) -> Result<ApprovalRequest, ApiError> {
    if record.was_decided_as(decision.kind) {
        Ok(record)
    } else {
        Err(ApiError::conflict(APPROVAL_ALREADY_RESOLVED))
    }
}

impl AgentRunCoordinator {
    pub(crate) async fn approval_verdict(
        &self,
        agent: &AgentState,
        session_id: &str,
        call: &ToolCall,
    ) -> Verdict {
        self.state
            .read()
            .await
            .approval_verdict(agent, session_id, call)
    }

    /// How long a run from `source` waits for the owner (spec §7.3).
    pub(crate) fn approval_timeout(&self, source: RunSource) -> Duration {
        self.approval_timeouts.for_source(source)
    }

    /// Saves a request for `ask`, then announces it: `run.awaiting_approval`
    /// when the run moved there, then `approval.requested` (spec §7.3). A
    /// request that cannot be saved is taken back and the tool does not run.
    pub(crate) async fn open_approval(
        &self,
        ask: ApprovalAsk,
    ) -> Result<PendingApproval, TaskResult<Content>> {
        let transaction = self.control_plane_transaction().await;
        let (opened, persist) = {
            let mut guard = self.state.write().await;
            let opened = guard
                .open_approval(&ask, now_millis())
                .map_err(|refusal| refusal.result())?;
            (opened, guard.control_plane_persist_request())
        };
        // Registered before the request is durable or announced, so a
        // decision always finds its waiter.
        let woken = self.approval_waiters.register(&opened.approval.id);
        if let Err(error) = persist.save().await {
            warn!(approval_id = %opened.approval.id, error = %error, "could not save an approval request; the tool does not run");
            self.approval_waiters.forget(&opened.approval.id);
            self.state.write().await.revert_open_approval(&opened);
            drop(transaction);
            return Err(TaskResult::error(APPROVAL_NOT_SAVED, 0));
        }
        {
            let guard = self.state.read().await;
            if let Some(run) = &opened.run {
                guard.publish_run_status(run);
            }
            guard.publish_approval(&opened.approval);
        }
        drop(transaction);
        Ok(PendingApproval {
            id: opened.approval.id,
            woken,
        })
    }

    /// Settles `id` as timed out or stopped unless something settled it
    /// first, and returns the record as it stands; `None` once it has left
    /// the control plane. An unsaved timeout or stop is kept: it only keeps
    /// the call from running, and a restart before the next save expires
    /// the request anyway (spec §4.8).
    pub(crate) async fn settle_approval(
        &self,
        id: &str,
        settlement: Settlement,
    ) -> Option<ApprovalRequest> {
        let transaction = self.control_plane_transaction().await;
        let (settled, persist) = {
            let mut guard = self.state.write().await;
            match guard.settle_approval(id, settlement, now_millis()) {
                Ok(settled) => (settled, guard.control_plane_persist_request()),
                Err(SettleRefusal::Resolved(record)) => return Some(record),
                Err(_) => return None,
            }
        };
        if let Err(error) = persist.save().await {
            warn!(approval_id = %id, error = %error, "could not save a timed-out or stopped approval; the next save keeps it");
        }
        // Announced before any waiter goes on, so a stream hears the
        // resolution before the tool's own events.
        self.state.read().await.publish_settled(&settled);
        self.approval_waiters.wake(&settled.approval);
        drop(transaction);
        Some(settled.approval)
    }

    /// The owner's decision (spec §7.3): saved before its waiter is woken; a
    /// failed save changes nothing (503). The same decision again answers
    /// with the record, from the history store once it moved there.
    pub(crate) async fn decide_approval(
        &self,
        id: &str,
        decision: OwnerDecision,
    ) -> Result<ApprovalRequest, ApiError> {
        let transaction = self.control_plane_transaction().await;
        let outcome = {
            let mut guard = self.state.write().await;
            match guard.settle_approval(id, Settlement::Owner(decision.clone()), now_millis()) {
                Ok(settled) => Ok((settled, guard.control_plane_persist_request())),
                Err(refusal) => Err(refusal),
            }
        };
        let (settled, persist) = match outcome {
            Ok(settled) => settled,
            Err(SettleRefusal::Resolved(record)) => return replay_or_conflict(record, &decision),
            Err(SettleRefusal::Stale) => return Err(ApiError::conflict(APPROVAL_REVISION_STALE)),
            Err(SettleRefusal::Invalid(message)) => {
                return Err(ApiError::bad_request_static(message))
            }
            Err(SettleRefusal::Conflict(message)) => return Err(ApiError::conflict(message)),
            Err(SettleRefusal::NotFound) => {
                drop(transaction);
                // Moved to the history store, or never known. Read outside
                // the state lock (M2 lock rule).
                let history = self.state.read().await.history.clone();
                return match history.store().get_approval(id).await {
                    Ok(Some(record)) => replay_or_conflict(record, &decision),
                    Ok(None) => Err(ApiError::not_found()),
                    Err(error) => Err(ApiError::service_unavailable(error.message())),
                };
            }
        };
        if let Err(error) = persist.save().await {
            self.state.write().await.revert_settled_approval(settled.undo);
            return Err(ApiError::service_unavailable(error.to_string()));
        }
        // Durable: the streams hear it first, then the waiting call goes on,
        // so `approval.resolved` precedes the tool's own events.
        self.state.read().await.publish_settled(&settled);
        self.approval_waiters.wake(&settled.approval);
        drop(transaction);
        Ok(settled.approval)
    }
}
```

In `hosts/rust-daemon/src/agent_runs.rs`:

1. Add `mod approvals;` first in the module list (before `mod compact;`).
2. In `AgentRunCoordinator`, after `title_timeout`, add:

```text
    /// Calls waiting for the owner, by approval id (spec §7.3).
    approval_waiters: crate::approvals::ApprovalWaiters,
    /// How long a call waits for the owner (spec §7.3).
    approval_timeouts: crate::approvals::ApprovalTimeouts,
```

and in `AgentRunCoordinator::new`, after the `title_timeout` field:

```text
            approval_waiters: crate::approvals::ApprovalWaiters::default(),
            approval_timeouts: crate::approvals::ApprovalTimeouts::default(),
```

3. After `with_title_timeout`, add:

```rust
    /// Shorter approval waits, so tests need not wait thirty minutes.
    #[cfg(test)]
    pub(crate) fn with_approval_timeouts(
        mut self,
        timeouts: crate::approvals::ApprovalTimeouts,
    ) -> Self {
        self.approval_timeouts = timeouts;
        self
    }
```

4. In `run_locked`, replace `.with_cancel(Some(live_run.control().cancel));` at the end of the `tool_context` builder chain with:

```text
            .with_cancel(Some(live_run.control().cancel))
            // Spec §7.3: the gate between the live checks and dispatch.
            .with_approvals(Some(crate::approvals::ApprovalGate::new(
                self.clone(),
                crate::runs::RunLink {
                    run_id: run_id.clone(),
                    session_id: session_id.clone(),
                    agent_id: agent_id.clone(),
                },
                source,
                live_run.control().cancel,
            )));
```

In `hosts/rust-daemon/src/tools.rs`:

1. Add to `ToolExecutionContext`, after `cancel`:

```text
    /// The run's approval gate (spec §7.3); `None` for swarm runs and direct
    /// tool calls, which have no session or owner to ask.
    pub(super) approvals: Option<crate::approvals::ApprovalGate>,
```

and `approvals: None,` to the struct literal in `ToolExecutionContext::new`.

2. After `with_cancel`, add:

```rust
    /// Puts the run's approval gate between the live checks and dispatch.
    pub(crate) fn with_approvals(mut self, gate: Option<crate::approvals::ApprovalGate>) -> Self {
        self.approvals = gate;
        self
    }
```

3. Replace `execute_tool` with these two methods:

```rust
    pub(crate) async fn execute_tool(
        self,
        agent: AgentState,
        user_message: Message,
        tool_call: ToolCall,
    ) -> TaskResult<Content> {
        if let Err(refused) = self.live_checks(&agent, &tool_call).await {
            return refused;
        }
        // No approval is asked for a tool that does not exist.
        let Some(handler) = self.tool_registry.lookup(&tool_call.name) else {
            return TaskResult::error(format!("Unknown tool: {}", tool_call.name), 0);
        };
        // Spec §7.3: after the live checks, before dispatch. An approval can
        // take minutes, so what it approved is checked again first.
        if let Some(gate) = &self.approvals {
            match gate.check(&agent, &tool_call).await {
                crate::approvals::GateOutcome::Proceed => {}
                crate::approvals::GateOutcome::Approved => {
                    if let Err(refused) = self.live_checks(&agent, &tool_call).await {
                        return refused;
                    }
                }
                crate::approvals::GateOutcome::Refuse(result) => return result,
            }
        }
        handler(self, agent, user_message, tool_call).await
    }

    /// The checks every call passes before it may run, and again after an
    /// approval: the tool is configured, a helper runs no process tool, and a
    /// delegating manager or peer still permits it.
    async fn live_checks(
        &self,
        agent: &AgentState,
        tool_call: &ToolCall,
    ) -> Result<(), TaskResult<Content>> {
        if !agent.config.allows_tool(&tool_call.name) {
            return Err(TaskResult::error(
                tool_not_configured_error(&tool_call.name),
                0,
            ));
        }
        if is_process_tool(&tool_call.name)
            && agent
                .config
                .settings
                .as_ref()
                .and_then(|settings| settings.additional.get("workspaceRole"))
                == Some(&DataValue::String("helper".into()))
        {
            return Err(TaskResult::error(
                "Process tools are unavailable to helpers until process cancellation is supported",
                0,
            ));
        }
        if let Some(parent_id) = &self.delegated_parent {
            let Some(coordinator) = &self.team else {
                return Err(TaskResult::error(
                    "Delegated permission context is unavailable",
                    0,
                ));
            };
            if !coordinator
                .parent_allows_tool(parent_id, &tool_call.name)
                .await
            {
                return Err(TaskResult::error(
                    "The manager no longer has permission for this delegated tool",
                    0,
                ));
            }
        }
        for source in &self.peer_sources {
            let Some(coordinator) = &self.team else {
                return Err(TaskResult::error("Peer permission context is unavailable", 0));
            };
            if !coordinator.peer_allows_tool(source, &tool_call.name).await {
                return Err(TaskResult::error(
                    "An originating agent no longer permits this peer tool action",
                    0,
                ));
            }
        }
        Ok(())
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::approval_tests agent_runs::stop_tests tools:: live::`
Expected: PASS — the 16 approval tests, the M3 stop tests (the bash one with `exec` allowed), and the tool and live tests.

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs:: routes::tests::runs connectors::`
Expected: PASS — no other coordinator test asks: their tools are read, write, or delegate class, which `allow` by default.

- [ ] **Step 6: Format and commit**

Run: `cargo fmt --all && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::approval_tests`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/approvals/mod.rs hosts/rust-daemon/src/approvals/gate.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/agent_runs/approvals.rs hosts/rust-daemon/src/agent_runs/approval_tests.rs hosts/rust-daemon/src/agent_runs/test_support.rs hosts/rust-daemon/src/agent_runs/stop_tests.rs hosts/rust-daemon/src/tools.rs hosts/rust-daemon/src/live/events.rs hosts/rust-daemon/src/state/approval_state.rs hosts/rust-daemon/src/routes/contracts/approvals.rs hosts/rust-daemon/src/routes/contracts/mod.rs hosts/rust-daemon/src/routes/mod.rs
git commit -m "feat(daemon): wait for the owner's approval before risky tool calls"
```

Recommended implementer tier: most capable (async waiting, transaction ordering, and exactly-once settlement across three racing sources).

---

### Task 6: Stop while awaiting approval, abandoned waits, and steers that wait

**Files:**

- Create: `hosts/rust-daemon/src/agent_runs/approval_stop_tests.rs`
- Modify: `hosts/rust-daemon/src/state/run_stop.rs` (settle the run's approvals in the stop's own change; undo)
- Modify: `hosts/rust-daemon/src/agent_runs/stop.rs` (announce and wake after the save, before the signals)
- Modify: `hosts/rust-daemon/src/approvals/gate.rs` (`AbandonGuard` in `wait`)
- Modify: `hosts/rust-daemon/src/agent_runs.rs` (`#[cfg(test)] mod approval_stop_tests;`)

**Interfaces:**

- Consumes: Task 3 `DaemonState::{settle_approval, revert_settled_approval}`, `Settlement::Stopped`, `ApprovalUndo`; Task 5 `ApprovalWaiters::wake`, `DaemonState::{publish_approval, publish_run_status}`, `AgentRunCoordinator::settle_approval`, the test fixtures; M3 `DaemonState::request_run_stop`, `revert_run_stop`, `RunStopPlan`, `RunStopUndo`, `AgentRunCoordinator::{stop_run, accept_run, web_start}`, `AcceptRun`, `AcceptedRun::Steered`, `SessionRunMode::Steer`.
- Produces:
  - `RunStopPlan.approvals: Vec<ApprovalRequest>` (settled `stopped` by this stop, as they now are) and `RunStopPlan.resumed: Vec<RunRecord>` (runs those settlements moved back to `running`); `RunStopUndo.approvals: Vec<ApprovalUndo>` (`is_empty` counts it; `revert_run_stop` reverts them first).
  - No new public API in `gate.rs`: `wait` keeps a private `AbandonGuard` armed until it has an outcome.
- Behavior: spec §4.6 — a stop of a run (or a helper it started) that awaits approval settles each pending approval as `stopped` in the stop's own change, so it is saved with the stop; after the save the stop announces `approval.resolved` (and `run.started` for the resume), wakes each waiting call with the stopped record, and then cancels the runs. The call answers `Cancelled before running (stopped by owner)` and the run ends `cancelled`; a later decision is 409. A stop whose save fails is reverted whole: the approval is pending again and the run awaits approval again. A cancel without a saved stop (a helper's deadline, a shutdown) is settled `stopped` by the waiting call itself. A waiting call that is dropped (an aborted run task) settles its approval `stopped` from a spawned task. Steers sent while a run awaits approval join the run (it is in flight) and are taken in at the next model call, after the tool's result (spec §4.7): the waiter never touches the steering inbox.

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/agent_runs/approval_stop_tests.rs`:

```rust
//! Stopping a run that awaits approval (spec §4.6), waits that are
//! abandoned, and steers that wait for the next model call (spec §4.7).

use std::time::Duration;

use anima_core::{CancelSignal, MessageRole, CANCELLED_TOOL_RESULT};
use axum::http::StatusCode;

use super::test_support::{
    accept, accept_web, add_chat, approving_coordinator, chat_request, decision, events_until,
    gated_context, ledger_run, pending_approvals, remember_call, tool_input, tool_results,
    wait_for, ScriptedModel, Step,
};
use super::{AcceptRun, AcceptedRun, SessionRunMode};
use crate::approvals::{
    ApprovalDecisionKind, ApprovalPolicy, ApprovalStatus, ApprovalTimeouts, PolicyAction,
    ResolvedBy, RiskClass, APPROVAL_ALREADY_RESOLVED,
};
use crate::runs::RunStatus;

fn ask_before_writes() -> ApprovalPolicy {
    ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask)
}

fn patient() -> ApprovalTimeouts {
    ApprovalTimeouts {
        default: Duration::from_secs(30),
        telegram: Duration::from_secs(15),
    }
}

#[tokio::test]
async fn stopping_a_run_that_awaits_approval_resolves_it_as_stopped() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["never sent"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:stop", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let stopped = coordinator.stop_run(&agent_id, &pending.run_id).await.unwrap();
    assert!(stopped.stop.is_some());
    {
        let guard = coordinator.state.read().await;
        let approval = guard.approvals.get(&pending.id).unwrap();
        assert_eq!(approval.status, ApprovalStatus::Stopped);
        assert_eq!(
            approval.resolution.as_ref().unwrap().resolved_by,
            ResolvedBy::Stop
        );
    }

    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    let results = tool_results(&coordinator, &agent_id).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].contains(CANCELLED_TOOL_RESULT), "{}", results[0]);
    assert_eq!(model.requests().len(), 1, "a stopped run makes no more calls");
    assert_eq!(
        coordinator.state.read().await.runs.get(&pending.run_id).unwrap().status,
        RunStatus::Cancelled
    );
    let late = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap_err();
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(late.message(), APPROVAL_ALREADY_RESOLVED);

    let events = events_until(&mut subscription, "run.cancelled").await;
    let resolved = events
        .iter()
        .filter(|event| event["type"] == "approval.resolved")
        .collect::<Vec<_>>();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["approval"]["status"], "stopped");
    assert_eq!(resolved[0]["approval"]["resolution"]["resolvedBy"], "stop");
}

#[tokio::test]
async fn a_stop_that_cannot_be_saved_leaves_the_approval_waiting() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), patient()).await;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:stop", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    let stopping = {
        let coordinator = coordinator.clone();
        let (agent_id, run_id) = (agent_id.clone(), pending.run_id.clone());
        tokio::spawn(async move { coordinator.stop_run(&agent_id, &run_id).await })
    };
    tokio::time::timeout(Duration::from_secs(5), save.entered.acquire())
        .await
        .expect("the stop saves")
        .unwrap()
        .forget();
    save.release.add_permits(1);

    let error = stopping.await.unwrap().unwrap_err();
    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    {
        let guard = coordinator.state.read().await;
        assert!(guard.approvals.get(&pending.id).unwrap().is_pending());
        let run = guard.runs.get(&pending.run_id).unwrap();
        assert_eq!(run.status, RunStatus::AwaitingApproval);
        assert_eq!(run.stop, None);
    }

    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );
}

#[tokio::test]
async fn a_cancel_without_a_saved_stop_still_resolves_the_approval() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["never sent"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), patient()).await;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:cancel", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // What a helper's deadline or a shutdown does: the signal, no saved stop.
    coordinator
        .state
        .read()
        .await
        .live
        .runs()
        .control(&pending.run_id)
        .expect("the run is live")
        .cancel
        .cancel();

    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    let guard = coordinator.state.read().await;
    let approval = guard.approvals.get(&pending.id).unwrap();
    assert_eq!(approval.status, ApprovalStatus::Stopped);
    assert_eq!(
        approval.resolution.as_ref().unwrap().resolved_by,
        ResolvedBy::Stop
    );
    drop(guard);
    assert!(tool_results(&coordinator, &agent_id).await[0].contains(CANCELLED_TOOL_RESULT));
}

#[tokio::test]
async fn an_abandoned_wait_resolves_its_approval_as_stopped() {
    let (coordinator, agent_id) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), patient()).await;
    let agent = coordinator
        .state
        .read()
        .await
        .get_agent(&agent_id)
        .unwrap()
        .state;
    let run = ledger_run(&coordinator, &agent_id, "chat:abandon").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new()).await;
    let input = tool_input(&agent_id, "chat:abandon");
    let calling = tokio::spawn(async move {
        context
            .execute_tool(agent, input, remember_call("call-1", "x"))
            .await
    });
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    calling.abort();
    let _ = calling.await;

    for _ in 0..500 {
        {
            let guard = coordinator.state.read().await;
            let approval = guard.approvals.get(&pending.id).unwrap();
            if approval.status == ApprovalStatus::Stopped {
                assert_eq!(
                    approval.resolution.as_ref().unwrap().resolved_by,
                    ResolvedBy::Stop
                );
                assert_eq!(
                    guard.runs.get(&run.run_id).unwrap().status,
                    RunStatus::Running,
                    "the run waits on nothing any more"
                );
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("an abandoned approval never resolved");
}

#[tokio::test]
async fn a_steer_sent_while_awaiting_approval_waits_for_the_next_model_call() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Saved, and noted"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let first = accept_web(&coordinator, &agent_id, "chat:s", "remember the plan").await;
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(pending.run_id, first);

    let steer = AcceptRun {
        mode: SessionRunMode::Steer,
        text: "also note the date".into(),
        ..accept(&agent_id, "chat:s", "key-steer")
    };
    let start = coordinator.web_start(
        steer.agent_id.clone(),
        steer.session_id.clone(),
        steer.text.clone(),
        steer.idempotency_key.clone(),
    );
    let AcceptedRun::Steered(joined) = coordinator.accept_run(steer, start).await.unwrap() else {
        panic!("the steer joins the run that awaits approval");
    };
    assert_eq!(joined.id, first);
    assert_eq!(joined.status, RunStatus::AwaitingApproval);

    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    wait_for(&coordinator, &first, RunStatus::Completed).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0]
        .messages
        .iter()
        .any(|message| message.content.text == "also note the date"));
    let second = &requests[1].messages;
    let result_at = second
        .iter()
        .position(|message| {
            message.role == MessageRole::Tool && message.content.text.contains("stored memory")
        })
        .expect("the tool ran");
    let steer_at = second
        .iter()
        .position(|message| {
            message.role == MessageRole::User && message.content.text == "also note the date"
        })
        .expect("the steer joined");
    assert!(steer_at > result_at, "the steer comes after the tool's result");
}
```

In `hosts/rust-daemon/src/agent_runs.rs`, add next to the other test modules:

```text
#[cfg(test)]
mod approval_stop_tests;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::approval_stop_tests`
Expected: FAIL — `an_abandoned_wait_resolves_its_approval_as_stopped` times out with the approval still pending, and `stopping_a_run_that_awaits_approval_resolves_it_as_stopped` usually finds the approval still pending when `stop_run` returns (before this task only the waiting call settles it, after the stop lets go of the transaction, so the outcome is a race). The other three tests already pass: they pin behavior this task must keep (a failed stop save leaves the approval waiting, a bare cancel still resolves it, a steer waits for the next model call).

- [ ] **Step 3: Settle the run's approvals with the stop**

In `hosts/rust-daemon/src/state/run_stop.rs`:

1. Add the imports:

```text
use crate::approvals::ApprovalRequest;
use crate::state::{ApprovalUndo, Settlement};
```

2. Add to `RunStopUndo`, after `jobs`:

```text
    /// Approvals the stop settled, as they were (spec §4.6).
    pub(crate) approvals: Vec<ApprovalUndo>,
```

and to its `is_empty`: `&& self.approvals.is_empty()`.

3. Add to `RunStopPlan`, after `cancelled`:

```text
    /// Approvals this stop settled as `stopped`, as they now are.
    pub(crate) approvals: Vec<ApprovalRequest>,
    /// Runs those settlements moved back to `running`.
    pub(crate) resumed: Vec<RunRecord>,
```

and `approvals: Vec::new(), resumed: Vec::new(),` to both `RunStopPlan { … }` literals in `request_run_stop` (the terminal-run return and `let mut plan`).

4. In `request_run_stop`, add `let mut stopping: Vec<String> = Vec::new();` next to `let mut sources`, and in the `RunStatus::Running | RunStatus::AwaitingApproval if target.stop.is_none()` arm, after the `sources.push` block, add `stopping.push(id.clone());`.
5. After the `for (source, source_ref) in sources { … }` loop, add:

```text
        // Spec §4.6: a stopped run's pending approvals resolve as `stopped`
        // in this same save, so a decision that comes later finds them
        // settled and the waiting calls hear it from the stop.
        for run_id in &stopping {
            for approval_id in self.approvals.pending_ids_for_run(run_id) {
                if let Ok(settled) = self.settle_approval(&approval_id, Settlement::Stopped, now_ms) {
                    plan.approvals.push(settled.approval);
                    plan.resumed.extend(settled.run);
                    plan.undo.approvals.push(settled.undo);
                }
            }
        }
```

6. At the start of `revert_run_stop`, add:

```text
        // Newest first, so each run goes back to awaiting its approvals.
        for approval in undo.approvals.into_iter().rev() {
            self.revert_settled_approval(approval);
        }
```

In `hosts/rust-daemon/src/agent_runs/stop.rs`, in `stop_run`, at the start of the block that takes `let guard = self.state.read().await;` after the save, before `for id in &plan.signal`, add:

```text
            // Settled with the stop (spec §4.6): the streams hear it, then
            // each waiting call, before any run is signalled.
            for approval in &plan.approvals {
                guard.publish_approval(approval);
            }
            for record in &plan.resumed {
                guard.publish_run_status(record);
            }
            for approval in &plan.approvals {
                self.approval_waiters.wake(approval);
            }
```

- [ ] **Step 4: Settle an abandoned wait**

In `hosts/rust-daemon/src/approvals/gate.rs`, add after `PendingApproval`:

```rust
/// Settles an approval as `stopped` when its waiting call is dropped before
/// it had an outcome (an aborted run task), so no request waits forever.
struct AbandonGuard {
    coordinator: AgentRunCoordinator,
    id: Option<String>,
}

impl AbandonGuard {
    fn disarm(&mut self) {
        self.id = None;
    }
}

impl Drop for AbandonGuard {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let coordinator = self.coordinator.clone();
        handle.spawn(async move {
            coordinator.settle_approval(&id, Settlement::Stopped).await;
        });
    }
}
```

and replace `wait` with:

```rust
    /// The approval as it was settled, by whoever settled it first.
    async fn wait(&self, pending: PendingApproval, timeout: Duration) -> Option<ApprovalRequest> {
        let PendingApproval { id, mut woken } = pending;
        let mut abandoned = AbandonGuard {
            coordinator: self.coordinator.clone(),
            id: Some(id.clone()),
        };
        let deadline = tokio::time::Instant::now() + timeout;
        let settled = tokio::select! {
            biased;
            settled = &mut woken => settled.ok(),
            () = self.cancel.cancelled() => None,
            () = tokio::time::sleep_until(deadline) => None,
        };
        let settled = match settled {
            Some(approval) => Some(approval),
            None => {
                let settlement = if self.cancel.is_cancelled() {
                    Settlement::Stopped
                } else {
                    Settlement::TimedOut
                };
                match self.coordinator.settle_approval(&id, settlement).await {
                    Some(approval) => Some(approval),
                    // Settled first by someone whose record already left the
                    // control plane; that settlement sent it here before it
                    // let go of the transaction this settle waited for.
                    None => woken.try_recv().ok(),
                }
            }
        };
        abandoned.disarm();
        settled
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::approval_stop_tests agent_runs::approval_tests agent_runs::stop_tests state::`
Expected: PASS — the 5 new tests, Task 5's, and the M3 stop tests.

- [ ] **Step 6: Format and commit**

Run: `cargo fmt --all && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::approval_stop_tests`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/state/run_stop.rs hosts/rust-daemon/src/agent_runs/stop.rs hosts/rust-daemon/src/approvals/gate.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/agent_runs/approval_stop_tests.rs
git commit -m "feat(daemon): resolve a stopped run's approvals with its stop"
```

Recommended implementer tier: most capable (the stop's save-then-signal protocol, revert on a failed save, and the drop guard).

---

### Task 7: Approval reads: the stream snapshot, session `pendingApprovals`, and `GET /api/approvals`

**Files:**

- Create: `hosts/rust-daemon/src/routes/approvals.rs`, `hosts/rust-daemon/src/routes/tests/approvals.rs`
- Modify: `hosts/rust-daemon/src/live/events.rs` (`snapshot_json` takes the approvals), `hosts/rust-daemon/src/live/tests.rs`
- Modify: `hosts/rust-daemon/src/state/live_state.rs` (`live_snapshot_approvals`), `hosts/rust-daemon/src/routes/events.rs`, `hosts/rust-daemon/src/routes/tests/events.rs`
- Modify: `hosts/rust-daemon/src/sessions/views.rs` (`SessionView.pending_approvals`), `hosts/rust-daemon/src/routes/contracts/sessions.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/approvals.rs` (`ApprovalsEnvelope`), `hosts/rust-daemon/src/routes/mod.rs` (module, route, `ApiDoc` path and tag, test module), `hosts/rust-daemon/src/approvals/mod.rs` (three constants)
- Modify: `hosts/rust-daemon/README.md` (the Approvals section and its first row)

**Interfaces:**

- Consumes: Task 2 `ApprovalRegistry::{pending, decided, pending_count_for_session}`; Task 4 `HistoryStore::page_approvals`, `ApprovalPageQuery`; Task 5 `ApprovalResponse`, `approval_json`; M3 `DaemonState::live_snapshot_runs`, `SnapshotRun`; `routes::jobs::{authorize, no_store}`, `routes::sessions::rejected`, `routes::http::{json_response, request_query}`.
- Produces:
  - `live::snapshot_json(agent_id: &str, seq: u64, runs: &[SnapshotRun], approvals: &[ApprovalRequest]) -> Value` (the `approvals` array now holds `ApprovalResponse` JSON).
  - `DaemonState::live_snapshot_approvals(&self, runs: &[SnapshotRun]) -> Vec<ApprovalRequest>` (pending approvals of those runs, oldest first).
  - `SessionView.pending_approvals: usize`; `SessionResponse.pendingApprovals` reports it (spec §3.2 derived field).
  - `GET /api/approvals` (`routes::approvals::list_approvals`, tag `approvals`) → `ApprovalsEnvelope { approvals: Vec<ApprovalResponse>, next_cursor: Option<String> }` (`nextCursor` is `<createdAtMs>:<id>`).
  - Constants in `approvals`: `DECIDED_APPROVAL_WINDOW_MS = 30 * 24 * 60 * 60 * 1000`, `DEFAULT_APPROVAL_PAGE = 50`, `MAX_APPROVAL_PAGE = 100`.
- Behavior: a new stream's first event lists the pending approvals of its runs, so a reconnecting browser shows them again (spec §6). `pending` lists every pending approval (or one agent's), oldest first, without paging. `decided` merges the history store (read outside the state lock) with the decided approvals the control plane still holds (its copy wins), keeps the last 30 days, sorts newest first by `(createdAtMs, id)`, and pages with an exclusive cursor. `agentId` matches the approval's own agent exactly.

- [ ] **Step 1: Write the failing tests**

In `hosts/rust-daemon/src/live/tests.rs`, in `a_snapshot_lists_runs_with_their_live_state`, pass `&[]` as `snapshot_json`'s new last argument (after the runs slice).

In `hosts/rust-daemon/src/routes/tests/events.rs`, add after `the_first_event_is_a_snapshot_of_the_active_runs`:

```rust
#[tokio::test]
async fn the_snapshot_lists_the_pending_approvals_of_its_runs() {
    use crate::approvals::{ApprovalRequest, PendingApprovalStart};

    let (state, agent) = state_with_agent();
    let active = running(&agent, "chat:a");
    let call = anima_core::ToolCall {
        id: "call-1".into(),
        name: "memory_add".into(),
        args: std::collections::BTreeMap::from([(
            "content".to_string(),
            anima_core::DataValue::String("the plan".into()),
        )]),
    };
    {
        let mut guard = state.write().await;
        guard.runs.insert(active.clone());
        guard.live.runs().register(&active.id);
        for (session, run) in [("chat:a", active.id.as_str()), ("chat:b", "run_elsewhere")] {
            guard.approvals.insert(ApprovalRequest::pending(
                PendingApprovalStart {
                    agent_id: &agent,
                    session_id: session,
                    run_id: run,
                    call: &call,
                    timeout_ms: 60_000,
                },
                5,
            ));
        }
    }
    let app = router(state, DaemonConfig::default());

    let response = app
        .oneshot(events_request(&agent, OWNER_ORIGIN))
        .await
        .unwrap();
    let snapshot = SseReader::new(response).next().await;

    let approvals = snapshot.data["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1, "only the snapshot's runs' approvals");
    assert_eq!(approvals[0]["runId"], active.id.as_str());
    assert_eq!(approvals[0]["status"], "pending");
    assert_eq!(approvals[0]["tool"], "memory_add");
    assert_eq!(approvals[0]["matcherKinds"], serde_json::json!(["any"]));
}
```

Create `hosts/rust-daemon/src/routes/tests/approvals.rs`:

```rust
use super::*;
use std::collections::BTreeMap;

use anima_core::primitives::now_millis;
use anima_core::{DataValue, ToolCall};
use serde_json::{json, Value};

use crate::approvals::{
    ApprovalDecisionKind, ApprovalRequest, ApprovalResolution, ApprovalStatus,
    PendingApprovalStart, ResolvedBy,
};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

const OWNER_ORIGIN: &str = "http://localhost:4200";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

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

fn remember() -> ToolCall {
    ToolCall {
        id: "call-1".into(),
        name: "memory_add".into(),
        args: BTreeMap::from([(
            "content".to_string(),
            DataValue::String("the plan".into()),
        )]),
    }
}

fn pending(agent: &str, session: &str, run: &str, at_ms: u64) -> ApprovalRequest {
    ApprovalRequest::pending(
        PendingApprovalStart {
            agent_id: agent,
            session_id: session,
            run_id: run,
            call: &remember(),
            timeout_ms: 60_000,
        },
        at_ms,
    )
}

fn decided(agent: &str, session: &str, at_ms: u64, note: &str) -> ApprovalRequest {
    let mut approval = pending(agent, session, "run_done", at_ms);
    approval.resolve(
        ApprovalStatus::Allowed,
        ApprovalResolution {
            decision: Some(ApprovalDecisionKind::AllowOnce),
            note: Some(note.into()),
            matcher: None,
            rule_id: None,
            resolved_by: ResolvedBy::Owner,
            resolved_at_ms: at_ms + 1,
        },
    );
    approval
}

/// A daemon whose agent has the chat `chat:plans`.
fn daemon() -> (Arc<RwLock<DaemonState>>, String) {
    let mut daemon = DaemonState::new();
    let agent = daemon
        .create_agent(test_config("companion"))
        .unwrap()
        .state
        .id;
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        "chat:plans",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Plans".into(),
        TitleSource::Owner,
        1,
    ));
    (Arc::new(RwLock::new(daemon)), agent)
}

fn ids(body: &Value) -> Vec<String> {
    body["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|approval| approval["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn listing_approvals_requires_the_owner() {
    let (state, _) = daemon();
    let app = router(state, DaemonConfig::default());

    let refused = app
        .oneshot(request(
            "GET",
            "/api/approvals",
            "https://untrusted.example",
            None,
        ))
        .await
        .unwrap();

    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn pending_approvals_are_listed_oldest_first_and_filtered_by_agent() {
    let (state, agent) = daemon();
    let (later, earlier, other) = (
        pending(&agent, "chat:plans", "run_a", 20),
        pending(&agent, "chat:plans", "run_a", 10),
        pending("agent-other", "chat:x", "run_x", 5),
    );
    {
        let mut guard = state.write().await;
        for approval in [later.clone(), earlier.clone(), other.clone()] {
            guard.approvals.insert(approval);
        }
        guard
            .approvals
            .insert(decided(&agent, "chat:plans", 30, "done"));
    }
    let app = router(state, DaemonConfig::default());

    let mine = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/approvals?status=pending&agentId={agent}"),
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(mine.status(), StatusCode::OK);
    assert_eq!(mine.headers()["cache-control"], "no-store");
    let mine = json_body(mine).await;
    assert_eq!(ids(&mine), [earlier.id.clone(), later.id.clone()]);
    assert_eq!(mine["nextCursor"], Value::Null);
    assert_eq!(mine["approvals"][0]["matcherKinds"], json!(["any"]));

    let everyone = json_body(
        app.oneshot(request("GET", "/api/approvals", OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(ids(&everyone), [other.id, earlier.id, later.id]);
}

#[tokio::test]
async fn decided_approvals_merge_the_store_and_the_control_plane_within_thirty_days() {
    let (state, agent) = daemon();
    let now = now_millis();
    let old = decided(&agent, "chat:plans", now - 31 * DAY_MS, "too old");
    let recent = decided(&agent, "chat:plans", now - 2 * DAY_MS, "stored");
    let mut both = decided(&agent, "chat:plans", now - 3 * DAY_MS, "stored copy");
    let held = decided(&agent, "chat:plans", now - DAY_MS, "held");
    // Written outside the state lock, as the outbox writes.
    let history = state.read().await.history.clone();
    history
        .store()
        .upsert_approvals(&[old, recent.clone(), both.clone()])
        .await
        .unwrap();
    both.resolution.as_mut().unwrap().note = Some("control plane copy".into());
    {
        let mut guard = state.write().await;
        guard.approvals.insert(held.clone());
        guard.approvals.insert(both.clone());
    }
    let app = router(state, DaemonConfig::default());

    let first = json_body(
        app.clone()
            .oneshot(request(
                "GET",
                &format!("/api/approvals?status=decided&agentId={agent}&limit=2"),
                OWNER_ORIGIN,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(ids(&first), [held.id.clone(), recent.id.clone()]);
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    assert_eq!(cursor, format!("{}:{}", recent.created_at_ms, recent.id));

    let second = json_body(
        app.oneshot(request(
            "GET",
            &format!("/api/approvals?status=decided&agentId={agent}&limit=2&cursor={cursor}"),
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(ids(&second), [both.id.clone()], "the 31-day-old one is left out");
    assert_eq!(
        second["approvals"][0]["resolution"]["note"],
        "control plane copy"
    );
    assert_eq!(second["nextCursor"], Value::Null);
}

#[tokio::test]
async fn invalid_approval_queries_are_rejected() {
    let (state, _) = daemon();
    let app = router(state, DaemonConfig::default());
    for (query, message) in [
        ("status=bogus", "status must be pending or decided"),
        ("status=decided&limit=0", "limit must be between 1 and 100"),
        ("status=decided&limit=101", "limit must be between 1 and 100"),
        ("status=decided&cursor=nonsense", "cursor is not valid"),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "GET",
                &format!("/api/approvals?{query}"),
                OWNER_ORIGIN,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message, "{query}");
    }
}

#[tokio::test]
async fn a_session_counts_its_pending_approvals() {
    let (state, agent) = daemon();
    {
        let mut guard = state.write().await;
        guard
            .approvals
            .insert(pending(&agent, "chat:plans", "run_a", 10));
        guard
            .approvals
            .insert(decided(&agent, "chat:plans", 5, "done"));
    }
    let app = router(state, DaemonConfig::default());

    let body = json_body(
        app.oneshot(request(
            "GET",
            &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap(),
    )
    .await;

    assert_eq!(body["session"]["pendingApprovals"], 1);
}
```

In `hosts/rust-daemon/src/routes/mod.rs`, add `mod approvals;` to the `mod tests { … }` module list (first).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::approvals routes::tests::events live::tests`
Expected: FAIL — `snapshot_json` takes three arguments, `GET /api/approvals` answers 404, and `pendingApprovals` is always 0.

- [ ] **Step 3: List pending approvals in the snapshot and on sessions**

In `hosts/rust-daemon/src/live/events.rs`, replace `snapshot_json` and its doc comment with:

```rust
/// The first event of every stream (spec §6): the active runs with their
/// current step, text so far, and tool cards, and their pending approvals.
pub(crate) fn snapshot_json(
    agent_id: &str,
    seq: u64,
    runs: &[SnapshotRun],
    approvals: &[ApprovalRequest],
) -> Value {
    let runs = runs
        .iter()
        .map(|run| {
            let live = run.live.clone().unwrap_or_default();
            json!({
                "run": run_json(&run.record),
                "stepId": live.step_id,
                "text": live.text,
                "textOffset": live.text_offset,
                "tools": live.tools,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "type": "stream.snapshot",
        "agentId": agent_id,
        "seq": seq,
        "at": now_millis(),
        "runs": runs,
        "approvals": approvals.iter().map(approval_json).collect::<Vec<_>>(),
    })
}
```

In `hosts/rust-daemon/src/state/live_state.rs`, add `use crate::approvals::ApprovalRequest;` and, after `live_snapshot_runs`:

```rust
    /// The pending approvals of a new stream's runs (spec §6), oldest first,
    /// so a client that reconnects shows them again.
    pub(crate) fn live_snapshot_approvals(&self, runs: &[SnapshotRun]) -> Vec<ApprovalRequest> {
        let run_ids = runs
            .iter()
            .map(|run| run.record.id.as_str())
            .collect::<HashSet<_>>();
        self.approvals
            .pending()
            .into_iter()
            .filter(|approval| run_ids.contains(approval.run_id.as_str()))
            .cloned()
            .collect()
    }
```

In `hosts/rust-daemon/src/routes/events.rs`, replace the block from `let (subscription, runs) = {` through `let snapshot = live::snapshot_json(&agent_id, 1, &runs);` with:

```text
    let (subscription, runs, approvals) = {
        let guard = state.daemon.read().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        // Subscribed before the snapshot is taken, so nothing published after
        // it is missed; the client drops the overlap by offset and id.
        let Ok(subscription) = guard.live.subscribe(&agent_id) else {
            return rejected(ApiError::too_many_requests(TOO_MANY_STREAMS));
        };
        let runs = guard.live_snapshot_runs(&agent_id);
        let approvals = guard.live_snapshot_approvals(&runs);
        (subscription, runs, approvals)
    };
    let snapshot = live::snapshot_json(&agent_id, 1, &runs, &approvals);
```

and in its `#[utoipa::path]` 200 description, replace `the session, run, step, message, and tool events` with `the session, run, step, message, tool, and approval events`.

In `hosts/rust-daemon/src/sessions/views.rs`:

1. Add `pending_approvals: usize,` to `Candidate` after `active_runs`, and to `SessionView` after `active_runs`: `pub(crate) pending_approvals: usize,`.
2. In `candidate`, after the `active_runs: …` field, add:

```text
        pending_approvals: state
            .approvals
            .pending_count_for_session(&record.agent_id, &record.id),
```

3. Where `SessionView { … }` is built from a candidate, add `pending_approvals: candidate.pending_approvals,` after `active_runs`.

In `hosts/rust-daemon/src/routes/contracts/sessions.rs`, replace

```text
    /// Always 0 until approvals exist (M4).
    pub(crate) pending_approvals: usize,
```

with

```text
    /// Approvals waiting for the owner in this session (spec §3.2).
    pub(crate) pending_approvals: usize,
```

and `pending_approvals: 0,` with `pending_approvals: view.pending_approvals,`.

- [ ] **Step 4: Add the list route**

In `hosts/rust-daemon/src/approvals/mod.rs`, after `APPROVAL_REVISION_STALE`, add:

```text
/// `GET /api/approvals?status=decided` reads this far back (spec §7.3).
pub(crate) const DECIDED_APPROVAL_WINDOW_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// Decided approvals per page, by default and at most.
pub(crate) const DEFAULT_APPROVAL_PAGE: usize = 50;
pub(crate) const MAX_APPROVAL_PAGE: usize = 100;
```

In `hosts/rust-daemon/src/routes/contracts/approvals.rs`, add:

```rust
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalsEnvelope {
    pub(crate) approvals: Vec<ApprovalResponse>,
    /// `decided` only: pass it as `cursor` for the next, older page.
    pub(crate) next_cursor: Option<String>,
}
```

Create `hosts/rust-daemon/src/routes/approvals.rs`:

```rust
//! Approvals (spec §7.3): the pending and decided lists, the owner's
//! decision, and each agent's policy and rules. Every route requires the
//! local owner and answers `Cache-Control: no-store`.

use std::collections::HashMap;

use anima_core::primitives::now_millis;
use axum::extract::{Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::Response;

use super::contracts::{ApprovalResponse, ApprovalsEnvelope, ErrorBody};
use super::http::{json_response, request_query};
use super::jobs::{authorize, no_store};
use super::sessions::rejected;
use super::{ApiError, AppState};
use crate::approvals::{
    ApprovalRequest, DECIDED_APPROVAL_WINDOW_MS, DEFAULT_APPROVAL_PAGE, MAX_APPROVAL_PAGE,
};
use crate::history::ApprovalPageQuery;

const STATUS_INVALID: &str = "status must be pending or decided";
const CURSOR_INVALID: &str = "cursor is not valid";
const LIMIT_INVALID: &str = "limit must be between 1 and 100";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListStatus {
    Pending,
    Decided,
}

struct ListQuery {
    status: ListStatus,
    agent_id: Option<String>,
    before: Option<(u64, String)>,
    limit: usize,
}

/// `<createdAtMs>:<id>`: where the next page of decided approvals starts.
fn cursor_of(approval: &ApprovalRequest) -> String {
    format!("{}:{}", approval.created_at_ms, approval.id)
}

fn parse_cursor(cursor: &str) -> Option<(u64, String)> {
    let (at_ms, id) = cursor.split_once(':')?;
    let at_ms = at_ms.parse().ok()?;
    (!id.is_empty()).then(|| (at_ms, id.to_string()))
}

fn list_query(uri: &Uri) -> Result<ListQuery, ApiError> {
    let params =
        request_query(uri).map_err(|()| ApiError::bad_request_static("malformed query"))?;
    let status = match params.get("status").map(String::as_str) {
        None | Some("") | Some("pending") => ListStatus::Pending,
        Some("decided") => ListStatus::Decided,
        Some(_) => return Err(ApiError::bad_request_static(STATUS_INVALID)),
    };
    let before = match params.get("cursor").map(String::as_str) {
        None | Some("") => None,
        Some(cursor) => {
            Some(parse_cursor(cursor).ok_or_else(|| ApiError::bad_request_static(CURSOR_INVALID))?)
        }
    };
    let limit = match params.get("limit").map(String::as_str) {
        None | Some("") => DEFAULT_APPROVAL_PAGE,
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_APPROVAL_PAGE).contains(limit))
            .ok_or_else(|| ApiError::bad_request_static(LIMIT_INVALID))?,
    };
    Ok(ListQuery {
        status,
        agent_id: params.get("agentId").filter(|id| !id.is_empty()).cloned(),
        before,
        limit,
    })
}

fn newest_first(left: &ApprovalRequest, right: &ApprovalRequest) -> std::cmp::Ordering {
    (right.created_at_ms, &right.id).cmp(&(left.created_at_ms, &left.id))
}

#[utoipa::path(get, path = "/api/approvals", tag = "approvals",
    params(
        ("status" = Option<String>, Query, description = "`pending` (the default): waiting for the owner, oldest first. `decided`: resolved in the last 30 days, newest first"),
        ("agentId" = Option<String>, Query, description = "Only this agent's approvals"),
        ("cursor" = Option<String>, Query, description = "`decided` only: the previous page's `nextCursor`"),
        ("limit" = Option<usize>, Query, description = "`decided` only: 1–100, default 50")
    ),
    responses(
        (status = 200, description = "Approvals, and for `decided` the next page's cursor", body = ApprovalsEnvelope),
        (status = 400, description = "Invalid status, cursor, or limit", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 503, description = "The history store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn list_approvals(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let query = match list_query(request.uri()) {
        Ok(query) => query,
        Err(error) => return rejected(error),
    };
    let for_agent = |approval: &ApprovalRequest| {
        query
            .agent_id
            .as_deref()
            .is_none_or(|agent_id| approval.agent_id == agent_id)
    };
    if query.status == ListStatus::Pending {
        let approvals = state
            .daemon
            .read()
            .await
            .approvals
            .pending()
            .into_iter()
            .filter(|approval| for_agent(*approval))
            .map(ApprovalResponse::from)
            .collect();
        return no_store(json_response(
            StatusCode::OK,
            &ApprovalsEnvelope {
                approvals,
                next_cursor: None,
            },
        ));
    }
    let since_ms = now_millis().saturating_sub(DECIDED_APPROVAL_WINDOW_MS);
    let (held, history) = {
        let guard = state.daemon.read().await;
        let held = guard
            .approvals
            .decided()
            .into_iter()
            .filter(|approval| {
                for_agent(*approval)
                    && approval.created_at_ms >= since_ms
                    && query.before.as_ref().is_none_or(|(at_ms, id)| {
                        (approval.created_at_ms, approval.id.as_str()) < (*at_ms, id.as_str())
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        (held, guard.history.clone())
    };
    // Read outside the state lock (M2 lock rule); one more than a page, so
    // the merge below knows whether another page follows.
    let stored = match history
        .store()
        .page_approvals(&ApprovalPageQuery {
            agent_id: query.agent_id.clone(),
            since_ms,
            before: query.before.clone(),
            limit: query.limit + 1,
        })
        .await
    {
        Ok(stored) => stored,
        Err(error) => return rejected(ApiError::service_unavailable(error.message())),
    };
    // Both may hold an approval just being mirrored: the control plane's
    // copy is the current one.
    let mut merged = stored
        .into_iter()
        .map(|approval| (approval.id.clone(), approval))
        .collect::<HashMap<_, _>>();
    for approval in held {
        merged.insert(approval.id.clone(), approval);
    }
    let mut approvals = merged.into_values().collect::<Vec<_>>();
    approvals.sort_by(newest_first);
    let next_cursor =
        (approvals.len() > query.limit).then(|| cursor_of(&approvals[query.limit - 1]));
    approvals.truncate(query.limit);
    no_store(json_response(
        StatusCode::OK,
        &ApprovalsEnvelope {
            approvals: approvals.iter().map(ApprovalResponse::from).collect(),
            next_cursor,
        },
    ))
}
```

In `hosts/rust-daemon/src/routes/mod.rs`:

1. Add `mod approvals;` first in the top module list.
2. Add `approvals::list_approvals,` to `ApiDoc`'s `paths(…)` after `runs::start_session_run, runs::list_session_runs, runs::get_run, runs::stop_run,`.
3. Add to `ApiDoc`'s `tags(…)`, after the `runs` tag:

```text
        (name = "approvals", description = "Tool approvals: pending requests, decisions, policies, and rules"),
```

4. Add the route after the `/api/agents/{agent_id}/runs/{run_id}/stop` route:

```text
        .route("/api/approvals", get(approvals::list_approvals))
```

In `hosts/rust-daemon/README.md`, insert before `### Agencies`:

```markdown
### Approvals

Every approval route requires local-owner authorization and answers `Cache-Control: no-store`, errors included. A coordinator run's tool call is judged by its agent's policy (`write`, `exec`, `network`, and `delegate` each `allow`, `ask`, or `deny`; by default only `exec` asks), then its rules and the session's allowances; read-class tools never ask, and a helper answers to its companion's policy and rules. A call that needs the owner waits up to 30 minutes (15 for a Telegram-started run) with its run `awaiting_approval`, announced as `run.awaiting_approval` and `approval.requested`. A decision, a Stop, or the timeout (a denial noted `Approval timed out`) settles it once, announced as `approval.resolved` and, when the run waits on nothing else, `run.started`. Helpers never wait: a call that would ask is denied (`Needs owner approval; not available to helpers`). A restart expires every pending approval. Decided approvals move to the history store.

| Method | Path             | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| ------ | ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET`  | `/api/approvals` | `?status=pending` (the default): approvals waiting for the owner, oldest first. `?status=decided`: approvals resolved in the last 30 days, newest first, with `?cursor=` and `?limit=` (1–100, default 50). Optional `?agentId=`. Returns `{ approvals, nextCursor }`; each approval carries its `tool`, `class`, `arguments` (JSON text, at most 16 KiB, with `argumentsTruncated`), `suggestedMatcher`, `matcherKinds`, `createdAtMs`, `expiresAtMs`, `status` (`pending`, `allowed`, `denied`, `stopped`, or `expired`), `revision`, and `resolution`. `400` for an invalid status, cursor, or limit; `503` when the history store cannot be read. |
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::approvals routes::tests::events routes::tests::sessions live:: sessions::`
Expected: PASS — the 5 new route tests, the new snapshot test, and the existing events, sessions, and live tests.

- [ ] **Step 6: Format and commit**

Run: `cargo fmt --all && bun x nx format:write --files=hosts/rust-daemon/README.md && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::approvals`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/routes/approvals.rs hosts/rust-daemon/src/routes/tests/approvals.rs hosts/rust-daemon/src/live/events.rs hosts/rust-daemon/src/live/tests.rs hosts/rust-daemon/src/state/live_state.rs hosts/rust-daemon/src/routes/events.rs hosts/rust-daemon/src/routes/tests/events.rs hosts/rust-daemon/src/sessions/views.rs hosts/rust-daemon/src/routes/contracts/sessions.rs hosts/rust-daemon/src/routes/contracts/approvals.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/approvals/mod.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): list pending and decided approvals and show them in snapshots and sessions"
```

Recommended implementer tier: standard (route and read-path plumbing with a merge whose paging edge cases are tested).

---

### Task 8: Decision, policy, and rule routes

**Files:**

- Modify: `hosts/rust-daemon/src/routes/approvals.rs` (five handlers and their request bodies)
- Modify: `hosts/rust-daemon/src/routes/contracts/approvals.rs` (envelopes for one approval, the policy, rules, and the tool catalog)
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (routes and `ApiDoc` paths)
- Modify: `hosts/rust-daemon/src/routes/tests/approvals.rs` (tests)
- Modify: `hosts/rust-daemon/src/approvals/mod.rs` (three constants; remove the temporary allows), `hosts/rust-daemon/src/state/approval_state.rs`, `hosts/rust-daemon/src/state.rs` (remove the temporary allows)
- Modify: `hosts/rust-daemon/README.md` (six rows)

**Interfaces:**

- Consumes: Task 5 `AgentRunCoordinator::decide_approval(id, OwnerDecision) -> Result<ApprovalRequest, ApiError>`, `ApprovalResponse`; Task 2 `ApprovalRegistry::{policy, set_policy, restore_policy, rules_for, find_rule, add_rule, remove_rule}`; Task 1 `validate_matcher`, `matcher_kinds`, `risk_class`; `crate::agent_runs::{config_helper_parent, is_helper_config}`; `DaemonState.tool_registry: ToolRegistry` (`lookup`, `tool_names`); `routes::jobs::body` (a JSON body within the daemon's size limit, errors `no_store`d); `AgentRunCoordinator::control_plane_transaction()`; `DeleteResponse`.
- Produces:
  - `POST /api/approvals/{approval_id}/decision` with `{ decision, note?, matcher?, revision }` (`deny_unknown_fields`) → 200 `{ approval }`; 400, 404, 409, 503 as the Global Constraints list.
  - `GET /api/agents/{agent_id}/approval-policy` → `{ policy: { write, exec, network, delegate } }` (a helper's is its companion's); `PUT` with all four fields → 200 `{ policy }`, 409 for a helper, saved (a failed save puts the old one back, 503).
  - `GET /api/agents/{agent_id}/approval-rules` → `{ rules, tools }` (`tools`: every registered tool that is not read class, with `class` and `matcherKinds`); `POST` `{ tool, matcher }` → 201 `{ rule }` (200 with the existing rule for an identical tool and matcher), 400 for an unknown or read-class tool or a bad matcher, 409 for a helper or past 100 rules; `DELETE /api/agents/{agent_id}/approval-rules/{rule_id}` → 200 `{ deleted: true }`, 404 when the agent has no such rule.
  - `routes::contracts::{ApprovalEnvelope, ApprovalPolicyResponse, ApprovalPolicyEnvelope, ApprovalRuleResponse, ApprovalRuleEnvelope, ApprovalToolResponse, ApprovalRulesEnvelope}`.
  - Constants in `approvals`: `READ_TOOLS_NEED_NO_RULE`, `HELPERS_USE_COMPANION_APPROVALS`, `UNKNOWN_RULE_TOOL`.
- Behavior: every route requires the local owner (reads `authorize_read`, writes `authorize`) and answers `no-store`, errors included; nothing else writes policies or rules (spec §7.2), and no tool reaches these routes' state. Policy and rule writes take the control-plane transaction and save before answering. A decision on an approval that already expired, timed out, or was stopped is 409; the same owner decision again is 200 with the record, from the history store once mirrored. This task also removes the temporary `dead_code`/`unused_imports` allowances Tasks 1–5 added, now that every item has a caller.

- [ ] **Step 1: Write the failing tests**

Add to `hosts/rust-daemon/src/routes/tests/approvals.rs`:

1. Extend the imports:

```text
use crate::agent_runs::test_support::{companion_config, remember_call, ScriptedModel, Step};
use crate::approvals::{
    ApprovalPolicy, PolicyAction, RiskClass, APPROVAL_ALREADY_RESOLVED, APPROVAL_NOTE_TOO_LONG,
    APPROVAL_REVISION_STALE, HELPERS_USE_COMPANION_APPROVALS, MATCHER_KIND_NOT_FOR_TOOL,
    MATCHER_VALUE_INVALID, MAX_APPROVAL_RULES_PER_AGENT, READ_TOOLS_NEED_NO_RULE, TOO_MANY_RULES,
    UNKNOWN_RULE_TOOL,
};
use crate::runs::RunStatus;
```

2. Add these tests:

```rust
#[test]
fn the_openapi_document_lists_every_approval_route() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    for path in [
        "/api/approvals",
        "/api/approvals/{approval_id}/decision",
        "/api/agents/{agent_id}/approval-policy",
        "/api/agents/{agent_id}/approval-rules",
        "/api/agents/{agent_id}/approval-rules/{rule_id}",
    ] {
        assert!(document.paths.paths.contains_key(path), "{path}");
    }
}

#[tokio::test]
async fn every_approval_route_requires_the_owner() {
    let (state, agent) = daemon();
    let app = router(state, DaemonConfig::default());
    let policy = json!({"write": "allow", "exec": "ask", "network": "allow", "delegate": "allow"});
    for (method, uri, body) in [
        (
            "POST",
            "/api/approvals/apr_1/decision".to_string(),
            Some(json!({"decision": "allow_once", "revision": 1})),
        ),
        ("GET", format!("/api/agents/{agent}/approval-policy"), None),
        (
            "PUT",
            format!("/api/agents/{agent}/approval-policy"),
            Some(policy),
        ),
        ("GET", format!("/api/agents/{agent}/approval-rules"), None),
        (
            "POST",
            format!("/api/agents/{agent}/approval-rules"),
            Some(json!({"tool": "bash", "matcher": {"kind": "any"}})),
        ),
        (
            "DELETE",
            format!("/api/agents/{agent}/approval-rules/rule_1"),
            None,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, &uri, "https://untrusted.example", body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn a_decision_resolves_the_waiting_call_and_repeats_idempotently() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Saved"]),
    ]);
    let mut daemon = DaemonState::with_model_adapter(model);
    let mut config = companion_config("companion");
    config.tools = Some(
        crate::tools::ToolRegistry::new()
            .resolve_descriptors(["memory_add"])
            .unwrap(),
    );
    let agent = daemon.create_agent(config).unwrap().state.id;
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        "chat:plans",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Plans".into(),
        TitleSource::Owner,
        1,
    ));
    daemon.approvals.set_policy(
        &agent,
        ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
    );
    let state = Arc::new(RwLock::new(daemon));
    let app = router(state.clone(), DaemonConfig::default());

    let mut start = request(
        "POST",
        &format!("/api/agents/{agent}/sessions/chat%3Aplans/runs"),
        OWNER_ORIGIN,
        Some(json!({"text": "remember the plan"})),
    );
    start
        .headers_mut()
        .insert("idempotency-key", "key-1".parse().unwrap());
    let accepted = app.clone().oneshot(start).await.unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let run_id = json_body(accepted).await["run"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut approval = None;
    for _ in 0..500 {
        approval = state.read().await.approvals.pending().first().map(|found| (*found).clone());
        if approval.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let approval = approval.expect("the run asks");
    let decide = |body: Value| {
        request(
            "POST",
            &format!("/api/approvals/{}/decision", approval.id),
            OWNER_ORIGIN,
            Some(body),
        )
    };

    let first = app
        .clone()
        .oneshot(decide(json!({"decision": "allow_once", "revision": 1})))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["cache-control"], "no-store");
    let first = json_body(first).await;
    assert_eq!(first["approval"]["status"], "allowed");
    assert_eq!(first["approval"]["revision"], 2);
    let replay = app
        .clone()
        .oneshot(decide(json!({"decision": "allow_once", "revision": 1})))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await, first);
    let other = app
        .clone()
        .oneshot(decide(json!({"decision": "deny", "revision": 2})))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(other).await["error"], APPROVAL_ALREADY_RESOLVED);

    for _ in 0..500 {
        if state
            .read()
            .await
            .runs
            .get(&run_id)
            .is_some_and(|run| run.status == RunStatus::Completed)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the allowed run never completed");
}

#[tokio::test]
async fn a_decision_needs_the_current_revision_a_short_note_and_a_fitting_matcher() {
    let (state, agent) = daemon();
    let waiting = pending(&agent, "chat:plans", "run_a", 10);
    state.write().await.approvals.insert(waiting.clone());
    let app = router(state.clone(), DaemonConfig::default());
    let uri = format!("/api/approvals/{}/decision", waiting.id);
    for (body, status, message) in [
        (
            json!({"decision": "allow_once", "revision": 2}),
            StatusCode::CONFLICT,
            APPROVAL_REVISION_STALE,
        ),
        (
            json!({"decision": "deny", "note": "x".repeat(1_001), "revision": 1}),
            StatusCode::BAD_REQUEST,
            APPROVAL_NOTE_TOO_LONG,
        ),
        (
            json!({"decision": "allow_always", "matcher": {"kind": "path_glob", "value": "**"}, "revision": 1}),
            StatusCode::BAD_REQUEST,
            MATCHER_KIND_NOT_FOR_TOOL,
        ),
        (
            json!({"decision": "allow_once", "revision": 1, "extra": true}),
            StatusCode::BAD_REQUEST,
            "request body must be valid JSON",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request("POST", &uri, OWNER_ORIGIN, Some(body.clone())))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{body}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message, "{body}");
    }
    assert!(state.read().await.approvals.get(&waiting.id).unwrap().is_pending());
    let missing = app
        .oneshot(request(
            "POST",
            "/api/approvals/apr_missing/decision",
            OWNER_ORIGIN,
            Some(json!({"decision": "allow_once", "revision": 1})),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_expired_or_mirrored_approval_answers_from_its_record() {
    let (state, agent) = daemon();
    let mut expired = pending(&agent, "chat:plans", "run_a", 10);
    expired.resolve(
        ApprovalStatus::Expired,
        ApprovalResolution {
            decision: None,
            note: None,
            matcher: None,
            rule_id: None,
            resolved_by: ResolvedBy::Restart,
            resolved_at_ms: 20,
        },
    );
    let mirrored = decided(&agent, "chat:plans", 30, "done");
    state.write().await.approvals.insert(expired.clone());
    let history = state.read().await.history.clone();
    history
        .store()
        .upsert_approvals(&[mirrored.clone()])
        .await
        .unwrap();
    let app = router(state, DaemonConfig::default());
    let decide = |id: &str, body: Value| {
        request(
            "POST",
            &format!("/api/approvals/{id}/decision"),
            OWNER_ORIGIN,
            Some(body),
        )
    };

    let late = app
        .clone()
        .oneshot(decide(&expired.id, json!({"decision": "allow_once", "revision": 1})))
        .await
        .unwrap();
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(late).await["error"], APPROVAL_ALREADY_RESOLVED);

    let replay = app
        .clone()
        .oneshot(decide(&mirrored.id, json!({"decision": "allow_once", "revision": 1})))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK, "answered from the history store");
    assert_eq!(json_body(replay).await["approval"]["id"], mirrored.id.as_str());
    let other = app
        .oneshot(decide(&mirrored.id, json!({"decision": "deny", "revision": 1})))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_policy_round_trips_and_is_saved() {
    let (state, agent) = daemon();
    let app = router(state.clone(), DaemonConfig::default());
    let uri = format!("/api/agents/{agent}/approval-policy");

    let initial = json_body(
        app.clone()
            .oneshot(request("GET", &uri, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        initial["policy"],
        json!({"write": "allow", "exec": "ask", "network": "allow", "delegate": "allow"})
    );

    let strict = json!({"write": "ask", "exec": "deny", "network": "allow", "delegate": "ask"});
    let put = app
        .clone()
        .oneshot(request("PUT", &uri, OWNER_ORIGIN, Some(strict.clone())))
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(put.headers()["cache-control"], "no-store");
    assert_eq!(json_body(put).await["policy"], strict);
    let read = json_body(
        app.clone()
            .oneshot(request("GET", &uri, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(read["policy"], strict);
    let saved = state.read().await.control_plane_snapshot();
    assert_eq!(saved.approval_policies[0].agent_id, agent);
    assert_eq!(saved.approval_policies[0].policy.exec, PolicyAction::Deny);

    let partial = app
        .clone()
        .oneshot(request(
            "PUT",
            &uri,
            OWNER_ORIGIN,
            Some(json!({"write": "ask"})),
        ))
        .await
        .unwrap();
    assert_eq!(partial.status(), StatusCode::BAD_REQUEST);
    let unknown = app
        .oneshot(request(
            "GET",
            "/api/agents/agent-missing/approval-policy",
            OWNER_ORIGIN,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_helper_has_no_policy_or_rules_of_its_own() {
    let (state, agent) = daemon();
    let helper = {
        let mut guard = state.write().await;
        let mut config = test_config("helper");
        let settings = config.settings.as_mut().unwrap();
        settings
            .additional
            .insert("workspaceRole".into(), DataValue::String("helper".into()));
        settings
            .additional
            .insert("parentAgentId".into(), DataValue::String(agent.clone()));
        guard.approvals.set_policy(
            &agent,
            ApprovalPolicy::default().with(RiskClass::Network, PolicyAction::Deny),
        );
        guard.create_agent(config).unwrap().state.id
    };
    let app = router(state, DaemonConfig::default());

    let read = json_body(
        app.clone()
            .oneshot(request(
                "GET",
                &format!("/api/agents/{helper}/approval-policy"),
                OWNER_ORIGIN,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(read["policy"]["network"], "deny", "the companion's policy");
    for (method, uri, body) in [
        (
            "PUT",
            format!("/api/agents/{helper}/approval-policy"),
            json!({"write": "allow", "exec": "allow", "network": "allow", "delegate": "allow"}),
        ),
        (
            "POST",
            format!("/api/agents/{helper}/approval-rules"),
            json!({"tool": "memory_add", "matcher": {"kind": "any"}}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, &uri, OWNER_ORIGIN, Some(body)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{method} {uri}");
        assert_eq!(
            json_body(response).await["error"],
            HELPERS_USE_COMPANION_APPROVALS
        );
    }
}

#[tokio::test]
async fn rules_are_created_listed_and_deleted() {
    let (state, agent) = daemon();
    let app = router(state, DaemonConfig::default());
    let rules = format!("/api/agents/{agent}/approval-rules");
    let body = json!({"tool": "bash", "matcher": {"kind": "command_prefix", "value": "  git   status "}});

    let created = app
        .clone()
        .oneshot(request("POST", &rules, OWNER_ORIGIN, Some(body.clone())))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = json_body(created).await["rule"].clone();
    assert_eq!(created["tool"], "bash");
    assert_eq!(
        created["matcher"],
        json!({"kind": "command_prefix", "value": "git status"})
    );
    assert_eq!(created["fromApprovalId"], Value::Null);
    let again = app
        .clone()
        .oneshot(request("POST", &rules, OWNER_ORIGIN, Some(body)))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK, "an identical rule is returned");
    assert_eq!(json_body(again).await["rule"]["id"], created["id"]);

    let listed = json_body(
        app.clone()
            .oneshot(request("GET", &rules, OWNER_ORIGIN, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["rules"].as_array().unwrap().len(), 1);
    let tools = listed["tools"].as_array().unwrap();
    assert!(tools.contains(&json!({
        "name": "bash",
        "class": "exec",
        "matcherKinds": ["command_prefix", "any"]
    })));
    assert!(!tools.iter().any(|tool| tool["class"] == "read"));

    let delete = format!("{rules}/{}", created["id"].as_str().unwrap());
    let deleted = app
        .clone()
        .oneshot(request("DELETE", &delete, OWNER_ORIGIN, None))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(json_body(deleted).await, json!({"deleted": true}));
    let gone = app
        .oneshot(request("DELETE", &delete, OWNER_ORIGIN, None))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rules_refuse_unknown_and_read_tools_bad_matchers_and_the_101st() {
    let (state, agent) = daemon();
    let app = router(state, DaemonConfig::default());
    let rules = format!("/api/agents/{agent}/approval-rules");
    for (body, status, message) in [
        (
            json!({"tool": "teleport", "matcher": {"kind": "any"}}),
            StatusCode::BAD_REQUEST,
            UNKNOWN_RULE_TOOL,
        ),
        (
            json!({"tool": "calculate", "matcher": {"kind": "any"}}),
            StatusCode::BAD_REQUEST,
            READ_TOOLS_NEED_NO_RULE,
        ),
        (
            json!({"tool": "bash", "matcher": {"kind": "path_glob", "value": "**"}}),
            StatusCode::BAD_REQUEST,
            MATCHER_KIND_NOT_FOR_TOOL,
        ),
        (
            json!({"tool": "bash", "matcher": {"kind": "command_prefix", "value": "git; rm"}}),
            StatusCode::BAD_REQUEST,
            MATCHER_VALUE_INVALID,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request("POST", &rules, OWNER_ORIGIN, Some(body.clone())))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{body}");
        assert_eq!(json_body(response).await["error"], message, "{body}");
    }
    for index in 0..MAX_APPROVAL_RULES_PER_AGENT {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &rules,
                OWNER_ORIGIN,
                Some(json!({"tool": "web_fetch", "matcher": {"kind": "domain", "value": format!("d{index}.example")}})),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }
    let full = app
        .oneshot(request(
            "POST",
            &rules,
            OWNER_ORIGIN,
            Some(json!({"tool": "web_fetch", "matcher": {"kind": "any"}})),
        ))
        .await
        .unwrap();
    assert_eq!(full.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(full).await["error"], TOO_MANY_RULES);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::approvals`
Expected: FAIL to compile — `READ_TOOLS_NEED_NO_RULE`, `HELPERS_USE_COMPANION_APPROVALS`, and `UNKNOWN_RULE_TOOL` do not exist; once they do, the new routes answer 404.

- [ ] **Step 3: Add the constants and the bodies**

In `hosts/rust-daemon/src/approvals/mod.rs`, after `MAX_APPROVAL_PAGE`, add:

```text
/// A rule for a read-class tool would never be consulted.
pub(crate) const READ_TOOLS_NEED_NO_RULE: &str = "Read-class tools never ask, so they need no rule";
/// Helpers are judged by their companion's policy and rules.
pub(crate) const HELPERS_USE_COMPANION_APPROVALS: &str =
    "Helpers use their companion's approval policy and rules";
pub(crate) const UNKNOWN_RULE_TOOL: &str = "unknown tool";
```

In `hosts/rust-daemon/src/routes/contracts/approvals.rs`, change the import to `use crate::approvals::{matcher_kinds, ApprovalMatcher, ApprovalPolicy, ApprovalRequest, ApprovalResolution, ApprovalRule};` and add:

```rust
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalEnvelope {
    pub(crate) approval: ApprovalResponse,
}

/// What each class does: `allow`, `ask`, or `deny` (spec §7.2).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalPolicyResponse {
    pub(crate) write: String,
    pub(crate) exec: String,
    pub(crate) network: String,
    pub(crate) delegate: String,
}

impl From<&ApprovalPolicy> for ApprovalPolicyResponse {
    fn from(policy: &ApprovalPolicy) -> Self {
        Self {
            write: policy.write.as_str().into(),
            exec: policy.exec.as_str().into(),
            network: policy.network.as_str().into(),
            delegate: policy.delegate.as_str().into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalPolicyEnvelope {
    pub(crate) policy: ApprovalPolicyResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRuleResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) tool: String,
    pub(crate) matcher: ApprovalMatcherResponse,
    pub(crate) created_at_ms: u64,
    /// The approval whose "Always allow" created it; `null` for one the
    /// owner added on the Approvals page.
    pub(crate) from_approval_id: Option<String>,
}

impl From<&ApprovalRule> for ApprovalRuleResponse {
    fn from(rule: &ApprovalRule) -> Self {
        Self {
            id: rule.id.clone(),
            agent_id: rule.agent_id.clone(),
            tool: rule.tool.clone(),
            matcher: ApprovalMatcherResponse::from(&rule.matcher),
            created_at_ms: rule.created_at_ms,
            from_approval_id: rule.from_approval_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRuleEnvelope {
    pub(crate) rule: ApprovalRuleResponse,
}

/// A tool a rule can cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalToolResponse {
    pub(crate) name: String,
    /// `write`, `exec`, `network`, or `delegate`.
    pub(crate) class: String,
    pub(crate) matcher_kinds: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRulesEnvelope {
    pub(crate) rules: Vec<ApprovalRuleResponse>,
    /// Every registered tool that is not read class, by name.
    pub(crate) tools: Vec<ApprovalToolResponse>,
}
```

- [ ] **Step 4: Add the handlers and the routes**

In `hosts/rust-daemon/src/routes/approvals.rs`:

1. Extend the imports:

```text
use axum::extract::Path;
use serde::Deserialize;
use utoipa::ToSchema;

use super::contracts::{
    ApprovalEnvelope, ApprovalPolicyEnvelope, ApprovalPolicyResponse, ApprovalRuleEnvelope,
    ApprovalRuleResponse, ApprovalRulesEnvelope, ApprovalToolResponse, DeleteResponse,
};
use super::jobs::body;
use crate::agent_runs::{config_helper_parent, is_helper_config};
use crate::approvals::{
    matcher_kinds, risk_class, validate_matcher, ApprovalDecisionKind, ApprovalMatcher,
    ApprovalPolicy, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
    HELPERS_USE_COMPANION_APPROVALS, READ_TOOLS_NEED_NO_RULE, UNKNOWN_RULE_TOOL,
};
use crate::state::OwnerDecision;
use crate::tools::ToolRegistry;
```

2. Add at the end of the file:

```rust
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MatcherRequest {
    kind: MatcherKind,
    /// Ignored for `any`.
    #[serde(default)]
    value: String,
}

impl From<MatcherRequest> for ApprovalMatcher {
    fn from(matcher: MatcherRequest) -> Self {
        Self {
            kind: matcher.kind,
            value: matcher.value,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DecisionRequest {
    decision: ApprovalDecisionKind,
    /// At most 1,000 characters; a denial's reaches the model.
    #[serde(default)]
    note: Option<String>,
    /// For `allow_session` and `allow_always`; the suggestion when absent.
    #[serde(default)]
    matcher: Option<MatcherRequest>,
    /// The approval's current revision.
    revision: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PolicyRequest {
    write: PolicyAction,
    exec: PolicyAction,
    network: PolicyAction,
    delegate: PolicyAction,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RuleRequest {
    tool: String,
    matcher: MatcherRequest,
}

#[utoipa::path(post, path = "/api/approvals/{approval_id}/decision", tag = "approvals",
    params(("approval_id" = String, Path)),
    request_body = DecisionRequest,
    responses(
        (status = 200, description = "The approval as decided; the same decision again returns it unchanged", body = ApprovalEnvelope),
        (status = 400, description = "An invalid body, a note over 1,000 characters, or a matcher that does not fit the tool", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such approval", body = ErrorBody),
        (status = 409, description = "Resolved another way already (another decision, the timeout, a Stop, or a restart), a stale revision, a full rule or allowance list, or a session that no longer exists", body = ErrorBody),
        (status = 503, description = "The decision could not be saved, or the history store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn decide_approval(
    State(state): State<AppState>,
    Path(approval_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: DecisionRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let decision = OwnerDecision {
        kind: input.decision,
        note: input.note,
        matcher: input.matcher.map(ApprovalMatcher::from),
        revision: input.revision,
    };
    match state.agent_runs.decide_approval(&approval_id, decision).await {
        Ok(approval) => no_store(json_response(
            StatusCode::OK,
            &ApprovalEnvelope {
                approval: ApprovalResponse::from(&approval),
            },
        )),
        Err(error) => rejected(error),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/approval-policy", tag = "approvals",
    params(("agent_id" = String, Path)),
    responses(
        (status = 200, description = "The agent's policy (a helper's is its companion's)", body = ApprovalPolicyEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody)
    ))]
pub(super) async fn get_approval_policy(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let guard = state.daemon.read().await;
    let Some(runtime) = guard.agents.get(&agent_id) else {
        return rejected(ApiError::not_found());
    };
    let owner = config_helper_parent(runtime.config())
        .unwrap_or(&agent_id)
        .to_string();
    let policy = guard.approvals.policy(&owner);
    no_store(json_response(
        StatusCode::OK,
        &ApprovalPolicyEnvelope {
            policy: ApprovalPolicyResponse::from(&policy),
        },
    ))
}

#[utoipa::path(put, path = "/api/agents/{agent_id}/approval-policy", tag = "approvals",
    params(("agent_id" = String, Path)),
    request_body = PolicyRequest,
    responses(
        (status = 200, description = "The policy, saved", body = ApprovalPolicyEnvelope),
        (status = 400, description = "An invalid body: all four classes are required", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody),
        (status = 409, description = "A helper, which uses its companion's policy", body = ErrorBody),
        (status = 503, description = "The policy could not be saved; the old one stays", body = ErrorBody)
    ))]
pub(super) async fn put_approval_policy(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: PolicyRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let policy = ApprovalPolicy {
        write: input.write,
        exec: input.exec,
        network: input.network,
        delegate: input.delegate,
    };
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (previous, persist) = {
        let mut guard = state.daemon.write().await;
        match guard.agents.get(&agent_id) {
            None => return rejected(ApiError::not_found()),
            Some(runtime) if is_helper_config(runtime.config()) => {
                return rejected(ApiError::conflict(HELPERS_USE_COMPANION_APPROVALS))
            }
            Some(_) => {}
        }
        let previous = guard.approvals.set_policy(&agent_id, policy);
        (previous, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state
            .daemon
            .write()
            .await
            .approvals
            .restore_policy(&agent_id, previous);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    no_store(json_response(
        StatusCode::OK,
        &ApprovalPolicyEnvelope {
            policy: ApprovalPolicyResponse::from(&policy),
        },
    ))
}

/// Every registered tool a rule can cover (all but the read class), by name.
fn rule_tools(registry: &ToolRegistry) -> Vec<ApprovalToolResponse> {
    registry
        .tool_names()
        .into_iter()
        .filter(|name| risk_class(name) != RiskClass::Read)
        .map(|name| ApprovalToolResponse {
            class: risk_class(&name).as_str().into(),
            matcher_kinds: matcher_kinds(&name)
                .iter()
                .map(|kind| kind.as_str().to_string())
                .collect(),
            name,
        })
        .collect()
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/approval-rules", tag = "approvals",
    params(("agent_id" = String, Path)),
    responses(
        (status = 200, description = "The agent's rules, oldest first (a helper's are its companion's), and the tools a rule can cover", body = ApprovalRulesEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody)
    ))]
pub(super) async fn list_approval_rules(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let guard = state.daemon.read().await;
    let Some(runtime) = guard.agents.get(&agent_id) else {
        return rejected(ApiError::not_found());
    };
    let owner = config_helper_parent(runtime.config())
        .unwrap_or(&agent_id)
        .to_string();
    no_store(json_response(
        StatusCode::OK,
        &ApprovalRulesEnvelope {
            rules: guard
                .approvals
                .rules_for(&owner)
                .into_iter()
                .map(ApprovalRuleResponse::from)
                .collect(),
            tools: rule_tools(&guard.tool_registry),
        },
    ))
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/approval-rules", tag = "approvals",
    params(("agent_id" = String, Path)),
    request_body = RuleRequest,
    responses(
        (status = 201, description = "The rule, saved", body = ApprovalRuleEnvelope),
        (status = 200, description = "The agent already had this exact rule", body = ApprovalRuleEnvelope),
        (status = 400, description = "An invalid body, an unknown or read-class tool, or a matcher that does not fit the tool", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody),
        (status = 409, description = "A helper, or the agent already has 100 rules", body = ErrorBody),
        (status = 503, description = "The rule could not be saved", body = ErrorBody)
    ))]
pub(super) async fn create_approval_rule(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: RuleRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let tool = input.tool.trim().to_string();
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (rule, persist) = {
        let mut guard = state.daemon.write().await;
        match guard.agents.get(&agent_id) {
            None => return rejected(ApiError::not_found()),
            Some(runtime) if is_helper_config(runtime.config()) => {
                return rejected(ApiError::conflict(HELPERS_USE_COMPANION_APPROVALS))
            }
            Some(_) => {}
        }
        if guard.tool_registry.lookup(&tool).is_none() {
            return rejected(ApiError::bad_request_static(UNKNOWN_RULE_TOOL));
        }
        if risk_class(&tool) == RiskClass::Read {
            return rejected(ApiError::bad_request_static(READ_TOOLS_NEED_NO_RULE));
        }
        let matcher = match validate_matcher(&tool, &ApprovalMatcher::from(input.matcher)) {
            Ok(matcher) => matcher,
            Err(message) => return rejected(ApiError::bad_request_static(message)),
        };
        if let Some(existing) = guard.approvals.find_rule(&agent_id, &tool, &matcher) {
            return no_store(json_response(
                StatusCode::OK,
                &ApprovalRuleEnvelope {
                    rule: ApprovalRuleResponse::from(existing),
                },
            ));
        }
        let rule = ApprovalRule {
            id: format!("rule_{}", uuid::Uuid::new_v4()),
            agent_id: agent_id.clone(),
            tool,
            matcher,
            created_at_ms: now_millis(),
            from_approval_id: None,
        };
        if let Err(message) = guard.approvals.add_rule(rule.clone()) {
            return rejected(ApiError::conflict(message));
        }
        (rule, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state
            .daemon
            .write()
            .await
            .approvals
            .remove_rule(&agent_id, &rule.id);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    no_store(json_response(
        StatusCode::CREATED,
        &ApprovalRuleEnvelope {
            rule: ApprovalRuleResponse::from(&rule),
        },
    ))
}

#[utoipa::path(delete, path = "/api/agents/{agent_id}/approval-rules/{rule_id}", tag = "approvals",
    params(("agent_id" = String, Path), ("rule_id" = String, Path)),
    responses(
        (status = 200, description = "The rule is gone", body = DeleteResponse),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or rule not found", body = ErrorBody),
        (status = 503, description = "The removal could not be saved; the rule stays", body = ErrorBody)
    ))]
pub(super) async fn delete_approval_rule(
    State(state): State<AppState>,
    Path((agent_id, rule_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (removed, persist) = {
        let mut guard = state.daemon.write().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        let Some(removed) = guard.approvals.remove_rule(&agent_id, &rule_id) else {
            return rejected(ApiError::not_found());
        };
        (removed, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        // The removal left room, so putting it back cannot hit the cap.
        let _ = state.daemon.write().await.approvals.add_rule(removed);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    no_store(json_response(
        StatusCode::OK,
        &DeleteResponse { deleted: true },
    ))
}
```

In `hosts/rust-daemon/src/routes/mod.rs`:

1. Replace `approvals::list_approvals,` in `ApiDoc`'s `paths(…)` with:

```text
        approvals::list_approvals, approvals::decide_approval, approvals::get_approval_policy,
        approvals::put_approval_policy, approvals::list_approval_rules, approvals::create_approval_rule,
        approvals::delete_approval_rule,
```

2. After `.route("/api/approvals", get(approvals::list_approvals))`, add:

```text
        .route(
            "/api/approvals/{approval_id}/decision",
            axum::routing::post(approvals::decide_approval),
        )
        .route(
            "/api/agents/{agent_id}/approval-policy",
            get(approvals::get_approval_policy).put(approvals::put_approval_policy),
        )
        .route(
            "/api/agents/{agent_id}/approval-rules",
            get(approvals::list_approval_rules).post(approvals::create_approval_rule),
        )
        .route(
            "/api/agents/{agent_id}/approval-rules/{rule_id}",
            axum::routing::delete(approvals::delete_approval_rule),
        )
```

In `hosts/rust-daemon/README.md`, add these rows to the Approvals table after the `/api/approvals` row:

```markdown
| `POST` | `/api/approvals/{approval_id}/decision` | Decide a pending approval: `{ "decision": "allow_once" \| "allow_session" \| "allow_always" \| "deny", "note"?, "matcher"?, "revision" }` (`note` at most 1,000 characters; `matcher` `{ "kind": "command_prefix" \| "path_glob" \| "domain" \| "any", "value" }` for `allow_session` and `allow_always`, the suggestion when absent). Saved before the waiting call goes on; returns `{ approval }`. `allow_session` adds a session allowance (at most 50); `allow_always` creates a rule. A denial answers the model `Denied by owner: <note>`. The same decision again returns the approval unchanged, from the history store once it moved there. `400` for an invalid body, note, or matcher; `404` for an unknown approval; `409` (`This approval was already resolved`) once it was resolved another way, including by the timeout, a Stop, or a restart, and (`This approval changed; reload it and decide again`) for a stale revision; `503` when the decision cannot be saved. |
| `GET` | `/api/agents/{agent_id}/approval-policy` | `{ policy: { write, exec, network, delegate } }`, each `allow`, `ask`, or `deny`; a helper's is its companion's. |
| `PUT` | `/api/agents/{agent_id}/approval-policy` | Replace the policy (all four classes required); returns `{ policy }`. `409` for a helper; `503` when it cannot be saved (the old policy stays). |
| `GET` | `/api/agents/{agent_id}/approval-rules` | `{ rules, tools }`: the agent's rules oldest first (a helper's are its companion's), and every tool a rule can cover with its `class` and `matcherKinds`. |
| `POST` | `/api/agents/{agent_id}/approval-rules` | Add a rule `{ "tool", "matcher" }`; returns `201` with `{ rule }`, or `200` with the agent's identical rule. A command-prefix rule never covers a command holding a shell operator, and a path rule never covers an absolute path or one with `..`. `400` for an unknown or read-class tool or an invalid matcher; `409` for a helper or past 100 rules; `503` when it cannot be saved. |
| `DELETE` | `/api/agents/{agent_id}/approval-rules/{rule_id}` | Remove a rule; returns `{ deleted: true }`. `404` for an unknown rule; `503` when it cannot be saved (the rule stays). |
```

- [ ] **Step 5: Remove the temporary allowances**

Every approval item has a caller now. Delete these lines:

- In `hosts/rust-daemon/src/approvals/mod.rs`: `#![allow(dead_code)] // M4 Task 8 removes this once the gate and the routes use every item.` and the three `#[allow(unused_imports)] // M4 …` lines above the `policy`, `registry`, and `gate` re-exports.
- In `hosts/rust-daemon/src/state/approval_state.rs`: `#![allow(dead_code)] // M4 Task 8 removes this once the gate and the routes use every item.`
- In `hosts/rust-daemon/src/state.rs`: `#[allow(unused_imports)] // M4 Tasks 5 and 6 use these; Task 8 removes the allow.`

Run: `CARGO_INCREMENTAL=0 cargo check -p anima-daemon 2>&1 | grep -n "approval" ; CARGO_INCREMENTAL=0 cargo check -p anima-daemon --tests 2>&1 | grep -n "approval"`
Expected: no output from either (no warning mentions an approval file or item). If a warning names an unused item, delete that item or its re-export rather than restoring an allow.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::approvals approvals:: agent_runs::approval state::approval_state`
Expected: PASS — the 9 new route tests and every earlier approval test.

- [ ] **Step 7: Format and commit**

Run: `cargo fmt --all && bun x nx format:write --files=hosts/rust-daemon/README.md && CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- routes::tests::approvals`
Expected: PASS.

```bash
git add hosts/rust-daemon/src/routes/approvals.rs hosts/rust-daemon/src/routes/contracts/approvals.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/routes/tests/approvals.rs hosts/rust-daemon/src/approvals/mod.rs hosts/rust-daemon/src/state/approval_state.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): decide approvals and manage approval policies and rules over owner-only routes"
```

Recommended implementer tier: standard (route plumbing over tested state; the security surface is the owner check on every route, which the tests pin).

---

### Task 9: SDK approvals client and typed approval events

**Files:**

- Create: `packages/sdk/src/approvals.ts`, `packages/sdk/src/approvals.spec.ts`
- Modify: `packages/sdk/src/events.ts`, `packages/sdk/src/events.spec.ts`, `packages/sdk/src/client.ts`, `packages/sdk/src/index.ts`

**Interfaces:**

- Consumes: Tasks 7–8 routes and JSON (`ApprovalResponse`, `{ approvals, nextCursor }`, `{ approval }`, `{ policy }`, `{ rules, tools }`, `{ rule }`, `{ deleted }`); Task 5 events `approval.requested` / `approval.resolved` with `approval`; `DaemonClient.requestJson<T>(path, { method?, body?, signal? })`.
- Produces (exported from `@animaOS-SWARM/sdk`):
  - Types: `RiskClass`, `PolicyClass`, `ApprovalPolicyAction`, `ApprovalPolicy`, `ApprovalMatcherKind`, `ApprovalMatcher`, `ApprovalStatus`, `ApprovalDecision`, `ApprovalResolvedBy`, `ApprovalResolution`, `Approval`, `ApprovalRule`, `ApprovalTool`, `ApprovalRules`, `ApprovalPage`, `ApprovalListOptions`, `ApprovalDecisionInput`, `ApprovalRuleInput`.
  - Values: `POLICY_CLASSES: readonly PolicyClass[]` (`['write', 'exec', 'network', 'delegate']`), `DEFAULT_APPROVAL_POLICY` (`exec: 'ask'`, others `'allow'`), `MAX_APPROVAL_NOTE_CHARS = 1_000`, `ApprovalsClient`, `isApprovalEvent(event)`.
  - `ApprovalsClient`: `list(options: ApprovalListOptions): Promise<ApprovalPage>`, `decide(approvalId: string, input: ApprovalDecisionInput): Promise<Approval>`, `policy(agentId: string, options?: { signal?: AbortSignal }): Promise<ApprovalPolicy>`, `setPolicy(agentId: string, policy: ApprovalPolicy): Promise<ApprovalPolicy>`, `rules(agentId: string, options?: { signal?: AbortSignal }): Promise<ApprovalRules>`, `addRule(agentId: string, input: ApprovalRuleInput): Promise<ApprovalRule>`, `removeRule(agentId: string, ruleId: string): Promise<void>`.
  - `DaemonClient.approvals: ApprovalsClient`.
  - `AgentEvent` gains `{ type: 'approval.requested' | 'approval.resolved'; approval: Approval }`; `stream.snapshot`'s `approvals` becomes `Approval[]` (was `unknown[]`).
- Behavior: additive only (spec §13.4). Path segments are percent-encoded; `list` always sends `status`.

- [ ] **Step 1: Write the failing tests**

Create `packages/sdk/src/approvals.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';

import {
  createDaemonClient,
  DaemonHttpError,
  DEFAULT_APPROVAL_POLICY,
  MAX_APPROVAL_NOTE_CHARS,
  POLICY_CLASSES,
  type Approval,
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
  return { approvals: client.approvals, requests };
}

const approval: Approval = {
  id: 'apr_1',
  agentId: 'agent/a',
  sessionId: 'chat:1',
  runId: 'run_1',
  toolCallId: 'call-1',
  tool: 'bash',
  class: 'exec',
  arguments: '{"command":"git status"}',
  argumentsTruncated: false,
  suggestedMatcher: { kind: 'command_prefix', value: 'git status' },
  matcherKinds: ['command_prefix', 'any'],
  createdAtMs: 10,
  expiresAtMs: 1_800_010,
  status: 'pending',
  revision: 1,
  resolution: null,
};

describe('approvals client', () => {
  it('lists pending and decided approvals', async () => {
    const { approvals, requests } = transport(() =>
      Response.json({ approvals: [approval], nextCursor: '10:apr_1' }),
    );

    const pending = await approvals.list({
      status: 'pending',
      agentId: 'agent/a',
    });
    const decided = await approvals.list({
      status: 'decided',
      cursor: '20:apr_2',
      limit: 10,
    });

    expect(pending.approvals).toEqual([approval]);
    expect(decided.nextCursor).toBe('10:apr_1');
    expect(requests.map(({ url }) => url)).toEqual([
      '/api/approvals?status=pending&agentId=agent%2Fa',
      '/api/approvals?status=decided&cursor=20%3Aapr_2&limit=10',
    ]);
  });

  it('sends a decision with its revision and returns the decided approval', async () => {
    const decided = { ...approval, status: 'allowed', revision: 2 };
    const { approvals, requests } = transport(() =>
      Response.json({ approval: decided }),
    );

    const result = await approvals.decide('apr/1', {
      decision: 'allow_always',
      note: 'fine',
      matcher: { kind: 'command_prefix', value: 'git' },
      revision: 1,
    });

    expect(result).toEqual(decided);
    const [request] = requests;
    expect(request.url).toBe('/api/approvals/apr%2F1/decision');
    expect(request.init?.method).toBe('POST');
    expect(JSON.parse(String(request.init?.body))).toEqual({
      decision: 'allow_always',
      note: 'fine',
      matcher: { kind: 'command_prefix', value: 'git' },
      revision: 1,
    });
  });

  it('reads and replaces the policy', async () => {
    const strict = { ...DEFAULT_APPROVAL_POLICY, write: 'ask' as const };
    const { approvals, requests } = transport((_url, init) =>
      Response.json({
        policy: init?.method === 'PUT' ? strict : DEFAULT_APPROVAL_POLICY,
      }),
    );

    expect(await approvals.policy('agent/a')).toEqual({
      write: 'allow',
      exec: 'ask',
      network: 'allow',
      delegate: 'allow',
    });
    expect(await approvals.setPolicy('agent/a', strict)).toEqual(strict);
    expect(
      requests.map(({ url, init }) => [init?.method ?? 'GET', url]),
    ).toEqual([
      ['GET', '/api/agents/agent%2Fa/approval-policy'],
      ['PUT', '/api/agents/agent%2Fa/approval-policy'],
    ]);
    expect(JSON.parse(String(requests[1].init?.body))).toEqual(strict);
    expect(POLICY_CLASSES).toEqual(['write', 'exec', 'network', 'delegate']);
    expect(MAX_APPROVAL_NOTE_CHARS).toBe(1_000);
  });

  it('lists, adds, and removes rules', async () => {
    const rule = {
      id: 'rule_1',
      agentId: 'agent/a',
      tool: 'bash',
      matcher: { kind: 'command_prefix', value: 'git status' },
      createdAtMs: 5,
      fromApprovalId: null,
    };
    const { approvals, requests } = transport((_url, init) =>
      init?.method === 'DELETE'
        ? Response.json({ deleted: true })
        : init?.method === 'POST'
          ? Response.json({ rule }, { status: 201 })
          : Response.json({
              rules: [rule],
              tools: [
                {
                  name: 'bash',
                  class: 'exec',
                  matcherKinds: ['command_prefix', 'any'],
                },
              ],
            }),
    );

    const listed = await approvals.rules('agent/a');
    expect(listed.rules).toEqual([rule]);
    expect(listed.tools[0].class).toBe('exec');
    expect(
      await approvals.addRule('agent/a', {
        tool: 'bash',
        matcher: { kind: 'command_prefix', value: 'git status' },
      }),
    ).toEqual(rule);
    await approvals.removeRule('agent/a', 'rule/1');
    expect(
      requests.map(({ url, init }) => [init?.method ?? 'GET', url]),
    ).toEqual([
      ['GET', '/api/agents/agent%2Fa/approval-rules'],
      ['POST', '/api/agents/agent%2Fa/approval-rules'],
      ['DELETE', '/api/agents/agent%2Fa/approval-rules/rule%2F1'],
    ]);
  });

  it('surfaces a decision that lost a race as a daemon error', async () => {
    const { approvals } = transport(() =>
      Response.json(
        { error: 'This approval was already resolved' },
        { status: 409 },
      ),
    );

    const failure = approvals.decide('apr_1', {
      decision: 'deny',
      revision: 1,
    });
    await expect(failure).rejects.toBeInstanceOf(DaemonHttpError);
    await expect(failure).rejects.toMatchObject({
      status: 409,
      message: 'This approval was already resolved',
    });
  });
});
```

In `packages/sdk/src/events.spec.ts`, add `isApprovalEvent` to the `./index.js` import and this test inside `describe('agent events client', …)`:

```ts
it('types approval events and the approvals a snapshot carries', async () => {
  const approval = {
    id: 'apr_1',
    agentId: 'agent/a',
    sessionId: 'chat:1',
    runId: 'run_1',
    toolCallId: 'call-1',
    tool: 'bash',
    class: 'exec',
    arguments: '{"command":"ls"}',
    argumentsTruncated: false,
    suggestedMatcher: { kind: 'command_prefix', value: 'ls' },
    matcherKinds: ['command_prefix', 'any'],
    createdAtMs: 5,
    expiresAtMs: 1_800_005,
    status: 'pending',
    revision: 1,
    resolution: null,
  };
  const resolved = {
    ...approval,
    status: 'allowed',
    revision: 2,
    resolution: {
      decision: 'allow_once',
      note: null,
      matcher: null,
      ruleId: null,
      resolvedBy: 'owner',
      resolvedAtMs: 9,
    },
  };
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async () =>
      sseResponse([
        `id: 1\nevent: stream.snapshot\ndata: ${JSON.stringify({ type: 'stream.snapshot', agentId: 'agent/a', seq: 1, at: 5, runs: [], approvals: [approval] })}\n\n`,
        `id: 2\nevent: approval.resolved\ndata: ${JSON.stringify({ type: 'approval.resolved', agentId: 'agent/a', sessionId: 'chat:1', runId: 'run_1', seq: 2, at: 9, approval: resolved })}\n\n`,
      ]),
  });

  const received: AgentEvent[] = [];
  for await (const event of client.events.stream('agent/a'))
    received.push(event);

  const [snapshot, decided] = received;
  expect(snapshot.type === 'stream.snapshot' && snapshot.approvals[0].id).toBe(
    'apr_1',
  );
  expect(isApprovalEvent(decided)).toBe(true);
  expect(isApprovalEvent(snapshot)).toBe(false);
  expect(
    decided.type === 'approval.resolved' &&
      decided.approval.resolution?.resolvedBy,
  ).toBe('owner');
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `bun x nx test @animaOS-SWARM/sdk`
Expected: FAIL — `client.approvals`, `DEFAULT_APPROVAL_POLICY`, `POLICY_CLASSES`, `MAX_APPROVAL_NOTE_CHARS`, and `isApprovalEvent` are not exported.

- [ ] **Step 3: Implement the client and the event types**

Create `packages/sdk/src/approvals.ts`:

```ts
import type { DaemonClient } from './client.js';

/** How much a tool can change (spec §7.1). Read-class tools never ask. */
export type RiskClass = 'read' | 'write' | 'exec' | 'network' | 'delegate';
/** The classes a policy sets. */
export type PolicyClass = Exclude<RiskClass, 'read'>;
export const POLICY_CLASSES: readonly PolicyClass[] = [
  'write',
  'exec',
  'network',
  'delegate',
];

export type ApprovalPolicyAction = 'allow' | 'ask' | 'deny';
export type ApprovalPolicy = Record<PolicyClass, ApprovalPolicyAction>;
/** The daemon's default (spec §7.2): ask only before `exec`. */
export const DEFAULT_APPROVAL_POLICY: ApprovalPolicy = {
  write: 'allow',
  exec: 'ask',
  network: 'allow',
  delegate: 'allow',
};

export type ApprovalMatcherKind =
  | 'command_prefix'
  | 'path_glob'
  | 'domain'
  | 'any';
export interface ApprovalMatcher {
  kind: ApprovalMatcherKind;
  /** Empty for `any`. */
  value: string;
}

export type ApprovalStatus =
  | 'pending'
  | 'allowed'
  | 'denied'
  | 'stopped'
  | 'expired';
export type ApprovalDecision =
  | 'allow_once'
  | 'allow_session'
  | 'allow_always'
  | 'deny';
export type ApprovalResolvedBy = 'owner' | 'timeout' | 'stop' | 'restart';

export interface ApprovalResolution {
  /** `null` for a stopped or expired request. */
  decision: ApprovalDecision | null;
  note: string | null;
  /** The allowance's or rule's matcher. */
  matcher: ApprovalMatcher | null;
  ruleId: string | null;
  resolvedBy: ApprovalResolvedBy;
  resolvedAtMs: number;
}

/** A tool call waiting for, or decided by, the owner (spec §7.3). */
export interface Approval {
  id: string;
  agentId: string;
  sessionId: string;
  runId: string;
  toolCallId: string;
  tool: string;
  class: RiskClass;
  /** The call's arguments as JSON text, at most 16 KiB. The model wrote
   *  them: show them as text, never as markup. */
  arguments: string;
  argumentsTruncated: boolean;
  suggestedMatcher: ApprovalMatcher;
  /** The matcher kinds an allowance or rule for this tool may use. */
  matcherKinds: ApprovalMatcherKind[];
  createdAtMs: number;
  expiresAtMs: number;
  status: ApprovalStatus;
  /** Send it back with a decision. */
  revision: number;
  resolution: ApprovalResolution | null;
}

export interface ApprovalRule {
  id: string;
  agentId: string;
  tool: string;
  matcher: ApprovalMatcher;
  createdAtMs: number;
  /** The approval whose "Always allow" created it. */
  fromApprovalId: string | null;
}

/** A tool a rule can cover. */
export interface ApprovalTool {
  name: string;
  class: PolicyClass;
  matcherKinds: ApprovalMatcherKind[];
}

export interface ApprovalRules {
  rules: ApprovalRule[];
  tools: ApprovalTool[];
}

export interface ApprovalPage {
  approvals: Approval[];
  /** `decided` only: pass it as `cursor` for the next, older page. */
  nextCursor: string | null;
}

export interface ApprovalListOptions {
  /** `pending`: oldest first. `decided`: the last 30 days, newest first. */
  status: 'pending' | 'decided';
  agentId?: string;
  cursor?: string;
  /** `decided` only: 1–100; the daemon default is 50. */
  limit?: number;
  signal?: AbortSignal;
}

export interface ApprovalDecisionInput {
  decision: ApprovalDecision;
  /** At most `MAX_APPROVAL_NOTE_CHARS`; a denial's reaches the model. */
  note?: string;
  /** For `allow_session` and `allow_always`; the suggestion when absent. */
  matcher?: ApprovalMatcher;
  revision: number;
}

export interface ApprovalRuleInput {
  tool: string;
  matcher: ApprovalMatcher;
}

/** The longest note the daemon keeps (spec §16). */
export const MAX_APPROVAL_NOTE_CHARS = 1_000;

function agentPath(agentId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}`;
}

export class ApprovalsClient {
  constructor(private readonly client: DaemonClient) {}

  async list(options: ApprovalListOptions): Promise<ApprovalPage> {
    const search = new URLSearchParams({ status: options.status });
    if (options.agentId) search.set('agentId', options.agentId);
    if (options.cursor) search.set('cursor', options.cursor);
    if (options.limit !== undefined) search.set('limit', String(options.limit));
    return this.client.requestJson<ApprovalPage>(
      `/api/approvals?${search.toString()}`,
      { signal: options.signal },
    );
  }

  /** The owner's decision (spec §7.3); the same decision again returns the
   *  approval unchanged. */
  async decide(
    approvalId: string,
    input: ApprovalDecisionInput,
  ): Promise<Approval> {
    const response = await this.client.requestJson<{ approval: Approval }>(
      `/api/approvals/${encodeURIComponent(approvalId)}/decision`,
      { method: 'POST', body: input },
    );
    return response.approval;
  }

  async policy(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<ApprovalPolicy> {
    const response = await this.client.requestJson<{
      policy: ApprovalPolicy;
    }>(`${agentPath(agentId)}/approval-policy`, { signal: options.signal });
    return response.policy;
  }

  async setPolicy(
    agentId: string,
    policy: ApprovalPolicy,
  ): Promise<ApprovalPolicy> {
    const response = await this.client.requestJson<{
      policy: ApprovalPolicy;
    }>(`${agentPath(agentId)}/approval-policy`, {
      method: 'PUT',
      body: policy,
    });
    return response.policy;
  }

  async rules(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<ApprovalRules> {
    return this.client.requestJson<ApprovalRules>(
      `${agentPath(agentId)}/approval-rules`,
      { signal: options.signal },
    );
  }

  async addRule(
    agentId: string,
    input: ApprovalRuleInput,
  ): Promise<ApprovalRule> {
    const response = await this.client.requestJson<{ rule: ApprovalRule }>(
      `${agentPath(agentId)}/approval-rules`,
      { method: 'POST', body: input },
    );
    return response.rule;
  }

  async removeRule(agentId: string, ruleId: string): Promise<void> {
    await this.client.requestJson<{ deleted: boolean }>(
      `${agentPath(agentId)}/approval-rules/${encodeURIComponent(ruleId)}`,
      { method: 'DELETE' },
    );
  }
}
```

In `packages/sdk/src/events.ts`:

1. Add `import type { Approval } from './approvals.js';`.
2. In the `stream.snapshot` member of `AgentEvent`, replace `approvals: unknown[];` with:

```text
      /** The pending approvals of the snapshot's runs (spec §6). */
      approvals: Approval[];
```

3. Add a member to the `AgentEvent` union, after the `tool.finished` one:

```text
  | (EventBase & {
      type: 'approval.requested' | 'approval.resolved';
      approval: Approval;
    })
```

4. After `isRunLifecycleEvent`, add:

```ts
export function isApprovalEvent(
  event: AgentEvent,
): event is Extract<
  AgentEvent,
  { type: 'approval.requested' | 'approval.resolved' }
> {
  return (
    event.type === 'approval.requested' || event.type === 'approval.resolved'
  );
}
```

In `packages/sdk/src/client.ts`: add `import { ApprovalsClient } from './approvals.js';`, the field `readonly approvals: ApprovalsClient;` after `readonly events: AgentEventsClient;`, and `this.approvals = new ApprovalsClient(this);` after `this.events = new AgentEventsClient(this);`.

In `packages/sdk/src/index.ts`, replace `export { AgentEventsClient, isRunLifecycleEvent } from './events.js';` with `export { AgentEventsClient, isApprovalEvent, isRunLifecycleEvent } from './events.js';` and add after the events type exports:

```ts
export {
  ApprovalsClient,
  DEFAULT_APPROVAL_POLICY,
  MAX_APPROVAL_NOTE_CHARS,
  POLICY_CLASSES,
} from './approvals.js';
export type {
  Approval,
  ApprovalDecision,
  ApprovalDecisionInput,
  ApprovalListOptions,
  ApprovalMatcher,
  ApprovalMatcherKind,
  ApprovalPage,
  ApprovalPolicy,
  ApprovalPolicyAction,
  ApprovalResolution,
  ApprovalResolvedBy,
  ApprovalRule,
  ApprovalRuleInput,
  ApprovalRules,
  ApprovalStatus,
  ApprovalTool,
  PolicyClass,
  RiskClass,
} from './approvals.js';
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `bun x nx test @animaOS-SWARM/sdk`
Expected: PASS — 5 new approvals tests, the new events test, and the existing 56.

Run: `bun x nx run @animaOS-SWARM/sdk:typecheck`
Expected: PASS.

- [ ] **Step 5: Format, build, and commit**

Run: `bun x nx format:write --files=packages/sdk/src/approvals.ts,packages/sdk/src/approvals.spec.ts,packages/sdk/src/events.ts,packages/sdk/src/events.spec.ts,packages/sdk/src/client.ts,packages/sdk/src/index.ts && bun x nx test @animaOS-SWARM/sdk && bun x nx run @animaOS-SWARM/sdk:build`
Expected: PASS, and the build emits `dist/approvals.js` so the web's direct Vitest runs resolve the new exports.

```bash
git add packages/sdk/src/approvals.ts packages/sdk/src/approvals.spec.ts packages/sdk/src/events.ts packages/sdk/src/events.spec.ts packages/sdk/src/client.ts packages/sdk/src/index.ts
git commit -m "feat(sdk): add the approvals client and typed approval events"
```

Recommended implementer tier: cheap (typed wrappers over routes with request-shape tests).

---

### Task 10: Web live state: pending approvals per run

**Files:**

- Modify: `apps/web/src/lib/session-events.ts`, `apps/web/src/lib/session-events.test.ts`
- Modify: `apps/web/src/lib/transcript.ts` (`NON_TERMINAL_ORDER`), `apps/web/src/lib/transcript.test.ts`
- Modify: `apps/web/src/test/live.ts` (`approvalFixture`, `approvalEvent`, `snapshotEvent`'s approvals)

**Interfaces:**

- Consumes: Task 9 `Approval`, `AgentEvent`'s `approval.requested` / `approval.resolved`, `stream.snapshot.approvals: Approval[]`.
- Produces:
  - `LiveRun.approvals: Approval[]` (the run's pending approvals, oldest first); `LiveState.approvals: Readonly<Record<string, Approval>>` (every pending approval this stream knows); `EMPTY_LIVE_STATE.approvals = {}`; `emptyLiveRun(run)` sets `approvals: []`.
  - `pendingApprovals(approvals: LiveState['approvals']): Approval[]` (oldest first).
  - Test fixtures: `approvalFixture(id: string, overrides?: Partial<Approval>): Approval` (a pending `bash` approval of `run_1` in `chat:1`), `approvalEvent(type, approval, seq): AgentEvent`, `snapshotEvent(runs?, seq?, agentId?, approvals?: Approval[])`.
- Behavior: a snapshot replaces the pending approvals with its own (so a reconnect shows each exactly once); `approval.requested` adds a pending approval once (a repeat is ignored); `approval.resolved` removes it; a run that finishes drops whatever approvals the stream still holds for it; an approval that arrives before its run is attached when the run arrives. In the transcript's merge of stream and ledger, `running` and `awaiting_approval` rank the same, so the stream's view wins and a stale ledger read never puts a resumed run back to "awaiting approval" (Review Focus 5); `queued` still yields to either.

- [ ] **Step 1: Write the failing tests**

In `apps/web/src/test/live.ts`:

1. Add `Approval` to the `@animaOS-SWARM/sdk` type import.
2. Replace `snapshotEvent` with:

```ts
export function snapshotEvent(
  runs: SnapshotRun[] = [],
  seq = 1,
  agentId = 'agent-main',
  approvals: Approval[] = [],
): AgentEvent {
  return { type: 'stream.snapshot', agentId, seq, at: 1, runs, approvals };
}
```

3. Add after `toolFinishedEvent`:

```ts
/** A pending `bash` approval of `run_1` in `chat:1`, unless overridden. */
export function approvalFixture(
  id: string,
  overrides: Partial<Approval> = {},
): Approval {
  return {
    id,
    agentId: 'agent-main',
    sessionId: 'chat:1',
    runId: 'run_1',
    toolCallId: 'call_1',
    tool: 'bash',
    class: 'exec',
    arguments: '{"command":"git status"}',
    argumentsTruncated: false,
    suggestedMatcher: { kind: 'command_prefix', value: 'git status' },
    matcherKinds: ['command_prefix', 'any'],
    createdAtMs: 1,
    expiresAtMs: 1_800_001,
    status: 'pending',
    revision: 1,
    resolution: null,
    ...overrides,
  };
}

export function approvalEvent(
  type: 'approval.requested' | 'approval.resolved',
  approval: Approval,
  seq: number,
): AgentEvent {
  return {
    type,
    agentId: approval.agentId,
    sessionId: approval.sessionId,
    runId: approval.runId,
    seq,
    at: 1,
    approval,
  };
}
```

In `apps/web/src/lib/session-events.test.ts`, add `pendingApprovals` to the `./session-events` import and `approvalEvent`, `approvalFixture` to the `../test/live` import, and add at the end:

```ts
describe('approvals', () => {
  const awaiting = runFixture('run_1', {
    status: 'awaiting_approval',
    startedAtMs: 2,
  });

  it('keeps the snapshot’s pending approvals on their runs, once', () => {
    const first = approvalFixture('apr_1', { createdAtMs: 5 });
    const second = approvalFixture('apr_2', { createdAtMs: 3 });
    const decided = approvalFixture('apr_3', { status: 'allowed' });
    const state = applyAll([
      snapshotEvent([snapshotRun(awaiting)], 1, 'agent-main', [
        first,
        second,
        decided,
      ]),
      approvalEvent('approval.requested', first, 2),
    ]);

    expect(pendingApprovals(state.approvals).map((item) => item.id)).toEqual([
      'apr_2',
      'apr_1',
    ]);
    expect(state.runs.run_1.approvals.map((item) => item.id)).toEqual([
      'apr_2',
      'apr_1',
    ]);

    const reconnected = applyEvent(
      state,
      snapshotEvent([snapshotRun(awaiting)], 1, 'agent-main', [first]),
    );
    expect(Object.keys(reconnected.approvals)).toEqual(['apr_1']);
    expect(reconnected.runs.run_1.approvals).toEqual([first]);
  });

  it('adds a requested approval and removes it once resolved', () => {
    const approval = approvalFixture('apr_1');
    const requested = applyAll([
      snapshotEvent([]),
      runEvent('run.awaiting_approval', awaiting, 2),
      approvalEvent('approval.requested', approval, 3),
    ]);
    expect(requested.runs.run_1.approvals).toEqual([approval]);

    const resolved = applyAll(
      [
        approvalEvent(
          'approval.resolved',
          { ...approval, status: 'allowed', revision: 2 },
          4,
        ),
        runEvent('run.started', { ...awaiting, status: 'running' }, 5),
      ],
      requested,
    );
    expect(resolved.approvals).toEqual({});
    expect(resolved.runs.run_1.approvals).toEqual([]);
    expect(resolved.runs.run_1.run.status).toBe('running');
  });

  it('attaches an approval that arrived before its run', () => {
    const approval = approvalFixture('apr_1');
    const state = applyAll([
      snapshotEvent([]),
      approvalEvent('approval.requested', approval, 2),
      runEvent('run.awaiting_approval', awaiting, 3),
    ]);

    expect(state.runs.run_1.approvals).toEqual([approval]);
  });

  it('drops a finished run’s approvals and ignores ones it never saw', () => {
    const approval = approvalFixture('apr_1');
    const state = applyAll([
      snapshotEvent([snapshotRun(awaiting)], 1, 'agent-main', [approval]),
      approvalEvent(
        'approval.resolved',
        approvalFixture('apr_unknown', { status: 'denied' }),
        2,
      ),
    ]);
    expect(Object.keys(state.approvals)).toEqual(['apr_1']);

    const cancelled = applyEvent(
      state,
      runEvent(
        'run.cancelled',
        { ...awaiting, status: 'cancelled', finishedAtMs: 9 },
        3,
      ),
    );
    expect(cancelled.approvals).toEqual({});
    expect(cancelled.runs.run_1.approvals).toEqual([]);
  });
});
```

In `apps/web/src/lib/transcript.test.ts`, in `prefers the more advanced status when live and ledger are both mid-flight`, replace the last assertion (the one expecting `'awaiting_approval'` from `mergeSessionRuns([runningLive], [awaitingLedger])`) with:

```ts
// Running and awaiting approval are both in flight: the stream's view
// wins, so a stale ledger read never puts a resumed run back to
// "awaiting approval" (M4 Review Focus 5).
const awaitingLedger = { ...queued, status: 'awaiting_approval' as const };
expect(mergeSessionRuns([runningLive], [awaitingLedger])[0].run.status).toBe(
  'running',
);
expect(
  mergeSessionRuns(
    [emptyLiveRun(awaitingLedger)],
    [{ ...queued, status: 'running' as const }],
  )[0].run.status,
).toBe('awaiting_approval');
expect(
  mergeSessionRuns([staleQueuedLive], [awaitingLedger])[0].run.status,
).toBe('awaiting_approval');
```

(delete the old `const awaitingLedger = …` line and its `expect`, which this replaces).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/web && bun x vitest run src/lib/session-events.test.ts src/lib/transcript.test.ts`
Expected: FAIL — `pendingApprovals` is not exported, runs have no `approvals`, and the merge still prefers the ledger's `awaiting_approval`.

- [ ] **Step 3: Track pending approvals**

In `apps/web/src/lib/session-events.ts`:

1. Add `type Approval,` to the `@animaOS-SWARM/sdk` import.
2. Add to `LiveRun`, after `steers`:

```text
  /** The run's calls waiting for the owner, oldest first (spec §7.3). */
  approvals: Approval[];
```

3. Replace `LiveState`, `EMPTY_LIVE_STATE`, and `emptyLiveRun` with:

```ts
export interface LiveState {
  /** The `seq` of the newest event applied from the current stream. */
  seq: number;
  runs: Readonly<Record<string, LiveRun>>;
  /** Every pending approval this stream knows, by id (spec §7.3). */
  approvals: Readonly<Record<string, Approval>>;
  /** Bumped by every snapshot and resync, so views refetch what they show. */
  epoch: number;
}

export const EMPTY_LIVE_STATE: LiveState = {
  seq: 0,
  runs: {},
  approvals: {},
  epoch: 0,
};
```

```ts
export function emptyLiveRun(run: Run): LiveRun {
  return { run, steps: [], tools: [], phase: null, steers: [], approvals: [] };
}

function byRequest(left: Approval, right: Approval): number {
  return (
    left.createdAtMs - right.createdAtMs || left.id.localeCompare(right.id)
  );
}

/** Pending approvals, oldest first. */
export function pendingApprovals(
  approvals: LiveState['approvals'],
): Approval[] {
  return Object.values(approvals).sort(byRequest);
}

function runApprovals(
  approvals: LiveState['approvals'],
  runId: string,
): Approval[] {
  return pendingApprovals(approvals).filter(
    (approval) => approval.runId === runId,
  );
}

/** Adds a pending approval once, on its run if the stream has the run. */
function withApproval(state: LiveState, approval: Approval): LiveState {
  if (approval.status !== 'pending' || state.approvals[approval.id])
    return state;
  const approvals = { ...state.approvals, [approval.id]: approval };
  const live = state.runs[approval.runId];
  return {
    ...state,
    approvals,
    runs: live
      ? {
          ...state.runs,
          [approval.runId]: {
            ...live,
            approvals: runApprovals(approvals, approval.runId),
          },
        }
      : state.runs,
  };
}

/** Removes approvals the stream holds, from their runs too. */
function withoutApprovals(state: LiveState, ids: readonly string[]): LiveState {
  const gone = ids.filter((id) => state.approvals[id]);
  if (gone.length === 0) return state;
  const approvals = { ...state.approvals };
  const runs = { ...state.runs };
  for (const id of gone) {
    const { runId } = approvals[id];
    delete approvals[id];
    const live = runs[runId];
    if (live)
      runs[runId] = {
        ...live,
        approvals: live.approvals.filter((approval) => approval.id !== id),
      };
  }
  return { ...state, approvals, runs };
}
```

4. Replace `withRun` with:

```ts
function withRun(state: LiveState, run: Run): LiveState {
  const current = state.runs[run.id];
  // A finished run never goes back to an earlier status.
  if (
    current &&
    isTerminalRunStatus(current.run.status) &&
    !isTerminalRunStatus(run.status)
  )
    return state;
  const next: LiveRun = current
    ? {
        ...current,
        run,
        phase: isTerminalRunStatus(run.status) ? null : current.phase,
      }
    : // An approval that arrived before its run joins it now.
      {
        ...emptyLiveRun(run),
        approvals: runApprovals(state.approvals, run.id),
      };
  const updated: LiveState = {
    ...state,
    runs: withoutOldFinished({ ...state.runs, [run.id]: next }),
  };
  if (!isTerminalRunStatus(run.status)) return updated;
  // A finished run waits on nothing: drop what the stream still holds.
  return withoutApprovals(
    updated,
    Object.values(updated.approvals)
      .filter((approval) => approval.runId === run.id)
      .map((approval) => approval.id),
  );
}
```

5. In `applyEvent`, replace the `stream.snapshot` branch with:

```ts
if (event.type === 'stream.snapshot') {
  const approvals: Record<string, Approval> = {};
  for (const approval of event.approvals)
    if (approval.status === 'pending') approvals[approval.id] = approval;
  const runs: Record<string, LiveRun> = {};
  for (const item of event.runs) {
    runs[item.run.id] = {
      ...emptyLiveRun(item.run),
      steps: item.stepId
        ? [
            capped({
              stepId: item.stepId,
              text: item.text,
              textOffset: item.textOffset,
            }),
          ]
        : [],
      tools: item.tools,
      approvals: runApprovals(approvals, item.run.id),
    };
  }
  return { seq: event.seq, runs, approvals, epoch: state.epoch + 1 };
}
```

and after `if (isRunLifecycleEvent(event)) return withRun(next, event.run);` add:

```ts
if (event.type === 'approval.requested')
  return withApproval(next, event.approval);
if (event.type === 'approval.resolved')
  return withoutApprovals(next, [event.approval.id]);
```

In `apps/web/src/lib/transcript.ts`, replace

```text
const NON_TERMINAL_ORDER: Record<string, number> = {
  queued: 0,
  running: 1,
  awaiting_approval: 2,
};
```

with

```text
/** Running and awaiting approval are both in flight and rank the same: the
 *  stream's view of either wins (M4 Review Focus 5). */
const NON_TERMINAL_ORDER: Record<string, number> = {
  queued: 0,
  running: 1,
  awaiting_approval: 1,
};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd apps/web && bun x vitest run src/lib/session-events.test.ts src/lib/transcript.test.ts src/hooks/useAgentEvents.test.tsx src/hooks/useLiveSession.test.tsx`
Expected: PASS — the 4 new reducer tests, the updated merge test, and the stream hooks' existing tests.

- [ ] **Step 5: Format and commit**

Run: `bun x nx format:write --files=apps/web/src/lib/session-events.ts,apps/web/src/lib/session-events.test.ts,apps/web/src/lib/transcript.ts,apps/web/src/lib/transcript.test.ts,apps/web/src/test/live.ts && cd apps/web && bun x vitest run src/lib/session-events.test.ts src/lib/transcript.test.ts`
Expected: PASS.

```bash
git add apps/web/src/lib/session-events.ts apps/web/src/lib/session-events.test.ts apps/web/src/lib/transcript.ts apps/web/src/lib/transcript.test.ts apps/web/src/test/live.ts
git commit -m "feat(web): track pending approvals per run from the event stream"
```

Recommended implementer tier: standard (a pure reducer with tolerant ordering rules).

---

### Task 11: Web inline approval cards

**Files:**

- Create: `apps/web/src/lib/approvals.ts`, `apps/web/src/lib/approvals.test.ts`, `apps/web/src/components/sessions/ApprovalCard.tsx`, `apps/web/src/components/sessions/ApprovalCard.test.tsx`, `apps/web/src/approvals.css`
- Modify: `apps/web/src/styles.css` (import), `apps/web/src/lib/daemon-api.ts` (`decideApproval`)
- Modify: `apps/web/src/lib/transcript.ts` (`TranscriptActions.onDecideApproval`), `apps/web/src/hooks/useTranscriptActions.ts`, `apps/web/src/hooks/useTranscriptActions.test.tsx`
- Modify: `apps/web/src/components/sessions/RunActivity.tsx`, `apps/web/src/components/sessions/RunActivity.test.tsx`

**Interfaces:**

- Consumes: Task 9 `Approval`, `ApprovalDecision`, `ApprovalDecisionInput`, `ApprovalMatcher`, `ApprovalMatcherKind`, `RiskClass`, `MAX_APPROVAL_NOTE_CHARS`, `DaemonHttpError`, `daemon.approvals` (through `setupClient`); Task 10 `LiveRun.approvals`, `approvalFixture`; M3 `RunActivity`, `TranscriptActions`, `useTranscriptActions`, `formatTime` (`components/ui-bits`).
- Produces:
  - `lib/approvals.ts`: `ApprovalDecide = (approval: Approval, input: Omit<ApprovalDecisionInput, 'revision'>) => Promise<string | null>` (null when it went through, else the message to show); `decideApproval: ApprovalDecide` (sends the approval's own `revision`); `CLASS_LABELS: Record<RiskClass, string>`, `DECISION_LABELS: Record<ApprovalDecision, string>`, `MATCHER_KIND_LABELS: Record<ApprovalMatcherKind, string>`, `describeMatcher(tool: string, matcher: ApprovalMatcher): string`, `MAX_MATCHER_VALUE_CHARS = 512`.
  - `daemon.decideApproval(approvalId: string, input: ApprovalDecisionInput) => Promise<Approval>`.
  - `ApprovalCard({ approval, onDecide?, context? })`: a `region` named `Approval needed: <tool>` with the arguments in a `<pre>` labelled `Arguments`, a `Note (optional)` textarea (at most 1,000 characters), the scope line `For this session or always: <describeMatcher>` with `Edit` (a `Match by` select of `approval.matcherKinds` and a `Match value` input), and the buttons `Allow once`, `Allow for this session`, `Always allow`, `Deny`.
  - `TranscriptActions.onDecideApproval?: ApprovalDecide`; `TranscriptActionOptions.decideApproval?: ApprovalDecide` (defaults to `decideApproval`), with one stable `onDecideApproval` identity.
  - `RunActivity` renders an `ApprovalCard` for each of `live.approvals` after its tool cards and says `Waiting for your approval…` in its status region while the run awaits approval.
- Behavior: spec §15.2's inline cards with the four decisions. The arguments are untrusted text (spec §14): a text node inside `<pre>`, never Markdown or HTML. The note is trimmed and sent only when not blank; the matcher is sent only with `Allow for this session` and `Always allow` (the suggestion unless edited; choosing `Any call of this tool` clears the value). After a decision goes through the card says what was sent until the stream's `approval.resolved` removes it; a failure (409 already resolved, 404 gone, offline) shows the message and leaves the buttons usable.

- [ ] **Step 1: Write the failing tests**

Create `apps/web/src/lib/approvals.test.ts`:

```ts
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonConnectionError, DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from './daemon-api';
import { decideApproval, describeMatcher } from './approvals';
import { approvalFixture } from '../test/live';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('describeMatcher', () => {
  it('says what a rule or allowance covers', () => {
    expect(
      describeMatcher('bash', { kind: 'command_prefix', value: 'git status' }),
    ).toBe('bash commands starting with “git status”');
    expect(
      describeMatcher('write_file', { kind: 'path_glob', value: 'notes/**' }),
    ).toBe('write_file on files matching “notes/**”');
    expect(
      describeMatcher('web_fetch', { kind: 'domain', value: 'docs.rs' }),
    ).toBe('web_fetch on docs.rs and its subdomains');
    expect(describeMatcher('memory_add', { kind: 'any', value: '' })).toBe(
      'every memory_add call',
    );
  });
});

describe('decideApproval', () => {
  it('sends the approval’s own revision and answers null once it went through', async () => {
    const approval = approvalFixture('apr_1', { revision: 1 });
    const decide = vi
      .spyOn(daemon, 'decideApproval')
      .mockResolvedValue({ ...approval, status: 'allowed', revision: 2 });

    await expect(
      decideApproval(approval, { decision: 'allow_once' }),
    ).resolves.toBeNull();
    expect(decide).toHaveBeenCalledWith('apr_1', {
      decision: 'allow_once',
      revision: 1,
    });
  });

  it('answers what to show when it did not go through', async () => {
    const approval = approvalFixture('apr_1');
    const decide = vi.spyOn(daemon, 'decideApproval');

    decide.mockRejectedValueOnce(
      new DaemonHttpError(409, { error: 'This approval was already resolved' }),
    );
    await expect(decideApproval(approval, { decision: 'deny' })).resolves.toBe(
      'This approval was already resolved',
    );
    decide.mockRejectedValueOnce(
      new DaemonHttpError(404, { error: 'not found' }),
    );
    await expect(decideApproval(approval, { decision: 'deny' })).resolves.toBe(
      'This approval is no longer waiting.',
    );
    decide.mockRejectedValueOnce(new DaemonConnectionError('', new Error('x')));
    await expect(decideApproval(approval, { decision: 'deny' })).resolves.toBe(
      'Could not reach your companion. Try again.',
    );
  });
});
```

Create `apps/web/src/components/sessions/ApprovalCard.test.tsx`:

```tsx
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { approvalFixture } from '../../test/live';
import { ApprovalCard } from './ApprovalCard';

describe('ApprovalCard', () => {
  it('shows the call’s arguments as text, never as markup', () => {
    const approval = approvalFixture('apr_1', {
      arguments: '{"command":"<img src=x onerror=alert(1)> **bold**"}',
      argumentsTruncated: true,
    });
    const { container } = render(<ApprovalCard approval={approval} />);

    const card = screen.getByRole('region', { name: 'Approval needed: bash' });
    expect(within(card).getByLabelText('Arguments')).toHaveTextContent(
      '{"command":"<img src=x onerror=alert(1)> **bold**"}',
    );
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('strong')?.textContent).toBe('bash');
    expect(
      within(card).getByText('Arguments shortened to 16 KiB.'),
    ).toBeVisible();
    expect(within(card).getByText('Runs commands')).toBeVisible();
    expect(within(card).getByLabelText('Note (optional)')).toHaveAttribute(
      'maxLength',
      '1000',
    );
    for (const name of [
      'Allow once',
      'Allow for this session',
      'Always allow',
      'Deny',
    ])
      expect(within(card).getByRole('button', { name })).toBeDisabled();
  });

  it('sends a trimmed note with a denial and no matcher', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const onDecide = vi.fn().mockResolvedValue(null);
    render(<ApprovalCard approval={approval} onDecide={onDecide} />);

    await user.type(screen.getByLabelText('Note (optional)'), '  not now  ');
    await user.click(screen.getByRole('button', { name: 'Deny' }));

    expect(onDecide).toHaveBeenCalledWith(approval, {
      decision: 'deny',
      note: 'not now',
    });
    expect(
      await screen.findByText('Denied. The companion carries on without it.'),
    ).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Allow once' }),
    ).not.toBeInTheDocument();
  });

  it('sends the suggested scope, or the edited one, with the scoped decisions', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const onDecide = vi.fn().mockResolvedValue(null);
    const { unmount } = render(
      <ApprovalCard approval={approval} onDecide={onDecide} />,
    );
    expect(
      screen.getByText(/bash commands starting with “git status”/),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Edit' }));
    const value = screen.getByLabelText('Match value');
    await user.clear(value);
    await user.type(value, 'git');
    await user.click(screen.getByRole('button', { name: 'Always allow' }));
    expect(onDecide).toHaveBeenLastCalledWith(approval, {
      decision: 'allow_always',
      matcher: { kind: 'command_prefix', value: 'git' },
    });
    unmount();

    render(<ApprovalCard approval={approval} onDecide={onDecide} />);
    await user.click(screen.getByRole('button', { name: 'Edit' }));
    await user.selectOptions(
      screen.getByLabelText('Match by'),
      'Any call of this tool',
    );
    expect(screen.queryByLabelText('Match value')).not.toBeInTheDocument();
    await user.click(
      screen.getByRole('button', { name: 'Allow for this session' }),
    );
    expect(onDecide).toHaveBeenLastCalledWith(approval, {
      decision: 'allow_session',
      matcher: { kind: 'any', value: '' },
    });
  });

  it('shows why a decision did not go through and lets the owner try again', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const onDecide = vi
      .fn()
      .mockResolvedValueOnce('This approval was already resolved')
      .mockResolvedValueOnce(null);
    render(<ApprovalCard approval={approval} onDecide={onDecide} />);

    await user.click(screen.getByRole('button', { name: 'Allow once' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'This approval was already resolved',
    );
    await user.click(screen.getByRole('button', { name: 'Allow once' }));
    expect(await screen.findByText('Allowed once. Continuing…')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
```

In `apps/web/src/components/sessions/RunActivity.test.tsx`, add `within` to the Testing Library import and `approvalFixture` to the `../../test/live` import, and add inside `describe('RunActivity', …)`:

```tsx
it('shows the run’s approval cards inline and decides through the transcript actions', async () => {
  const user = userEvent.setup();
  const run = runFixture('run_1', {
    status: 'awaiting_approval',
    startedAtMs: Date.now(),
  });
  const approval = approvalFixture('apr_1');
  const onDecideApproval = vi.fn().mockResolvedValue(null);
  render(
    <RunActivity
      agentName="Nova"
      renderMessage={renderMessage}
      live={{ ...emptyLiveRun(run), approvals: [approval] }}
      actions={{ onDecideApproval }}
    />,
  );

  expect(screen.getByText('Waiting for your approval…')).toBeVisible();
  const card = screen.getByRole('region', { name: 'Approval needed: bash' });
  await user.click(within(card).getByRole('button', { name: 'Allow once' }));
  expect(onDecideApproval).toHaveBeenCalledWith(approval, {
    decision: 'allow_once',
  });
  expect(
    await within(card).findByText('Allowed once. Continuing…'),
  ).toBeVisible();
});
```

In `apps/web/src/hooks/useTranscriptActions.test.tsx`, add `approvalFixture` to the `../test/live` import and add inside `describe('useTranscriptActions', …)`:

```tsx
it('decides approvals through its handler with one identity across renders', async () => {
  const decideApproval = vi.fn().mockResolvedValue(null);
  const { result, rerender } = actionsFor({ decideApproval });
  const first = result.current;
  rerender();
  expect(result.current).toBe(first);

  const approval = approvalFixture('apr_1');
  await expect(
    result.current.onDecideApproval?.(approval, { decision: 'deny' }),
  ).resolves.toBeNull();
  expect(decideApproval).toHaveBeenCalledWith(approval, {
    decision: 'deny',
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/web && bun x vitest run src/lib/approvals.test.ts src/components/sessions/ApprovalCard.test.tsx src/components/sessions/RunActivity.test.tsx src/hooks/useTranscriptActions.test.tsx`
Expected: FAIL — `lib/approvals`, `ApprovalCard`, `daemon.decideApproval`, and `onDecideApproval` do not exist.

- [ ] **Step 3: Add the helpers and the daemon call**

In `apps/web/src/lib/daemon-api.ts`, add `type ApprovalDecisionInput,` to the `@animaOS-SWARM/sdk` import and, after `agentEvents`, add:

```ts
  /** The owner's decision on a pending approval (spec §7.3). */
  decideApproval: (approvalId: string, input: ApprovalDecisionInput) =>
    setupClient.approvals.decide(approvalId, input),
```

Create `apps/web/src/lib/approvals.ts`:

```ts
import {
  DaemonHttpError,
  type Approval,
  type ApprovalDecision,
  type ApprovalDecisionInput,
  type ApprovalMatcher,
  type ApprovalMatcherKind,
  type RiskClass,
} from '@animaOS-SWARM/sdk';

import { daemon } from './daemon-api';

/** The daemon's bound on a matcher value (`MAX_MATCHER_VALUE_CHARS`). */
export const MAX_MATCHER_VALUE_CHARS = 512;

/** Sends the owner's decision with the approval's own revision; answers
 *  null when it went through, otherwise the message to show. */
export type ApprovalDecide = (
  approval: Approval,
  input: Omit<ApprovalDecisionInput, 'revision'>,
) => Promise<string | null>;

export const CLASS_LABELS: Record<RiskClass, string> = {
  read: 'Reads',
  write: 'Changes files and records',
  exec: 'Runs commands',
  network: 'Uses the internet',
  delegate: 'Asks other agents',
};

export const DECISION_LABELS: Record<ApprovalDecision, string> = {
  allow_once: 'Allow once',
  allow_session: 'Allow for this session',
  allow_always: 'Always allow',
  deny: 'Deny',
};

export const MATCHER_KIND_LABELS: Record<ApprovalMatcherKind, string> = {
  command_prefix: 'Command starts with',
  path_glob: 'File path matches',
  domain: 'Website domain is',
  any: 'Any call of this tool',
};

/** What a rule or allowance covers, in words. */
export function describeMatcher(
  tool: string,
  matcher: ApprovalMatcher,
): string {
  switch (matcher.kind) {
    case 'command_prefix':
      return `${tool} commands starting with “${matcher.value}”`;
    case 'path_glob':
      return `${tool} on files matching “${matcher.value}”`;
    case 'domain':
      return `${tool} on ${matcher.value} and its subdomains`;
    case 'any':
      return `every ${tool} call`;
  }
}

export const decideApproval: ApprovalDecide = async (approval, input) => {
  try {
    await daemon.decideApproval(approval.id, {
      ...input,
      revision: approval.revision,
    });
    return null;
  } catch (error) {
    if (error instanceof DaemonHttpError)
      return error.status === 404
        ? 'This approval is no longer waiting.'
        : error.message;
    return 'Could not reach your companion. Try again.';
  }
};
```

- [ ] **Step 4: Add the card and render it inline**

Create `apps/web/src/components/sessions/ApprovalCard.tsx`:

```tsx
import { useId, useState, type ReactNode } from 'react';
import {
  MAX_APPROVAL_NOTE_CHARS,
  type Approval,
  type ApprovalDecision,
  type ApprovalMatcher,
  type ApprovalMatcherKind,
} from '@animaOS-SWARM/sdk';

import {
  CLASS_LABELS,
  DECISION_LABELS,
  MATCHER_KIND_LABELS,
  MAX_MATCHER_VALUE_CHARS,
  describeMatcher,
  type ApprovalDecide,
} from '../../lib/approvals';
import { formatTime } from '../ui-bits';

const DECISIONS: readonly ApprovalDecision[] = [
  'allow_once',
  'allow_session',
  'allow_always',
  'deny',
];

const SENT: Record<ApprovalDecision, string> = {
  allow_once: 'Allowed once. Continuing…',
  allow_session: 'Allowed for this session. Continuing…',
  allow_always: 'Always allowed. Continuing…',
  deny: 'Denied. The companion carries on without it.',
};

function MatcherEditor({
  kinds,
  matcher,
  onChange,
  onDone,
}: {
  kinds: readonly ApprovalMatcherKind[];
  matcher: ApprovalMatcher;
  onChange: (matcher: ApprovalMatcher) => void;
  onDone: () => void;
}) {
  return (
    <fieldset className="approval-card-matcher">
      <legend className="approval-card-label">
        What “for this session” and “always” cover
      </legend>
      <select
        aria-label="Match by"
        className="approval-card-input"
        value={matcher.kind}
        onChange={(event) => {
          const kind = event.target.value as ApprovalMatcherKind;
          onChange({ kind, value: kind === 'any' ? '' : matcher.value });
        }}
      >
        {kinds.map((kind) => (
          <option key={kind} value={kind}>
            {MATCHER_KIND_LABELS[kind]}
          </option>
        ))}
      </select>
      {matcher.kind !== 'any' && (
        <input
          aria-label="Match value"
          className="approval-card-input"
          maxLength={MAX_MATCHER_VALUE_CHARS}
          value={matcher.value}
          onChange={(event) =>
            onChange({ ...matcher, value: event.target.value })
          }
        />
      )}
      <button type="button" className="studio-tool-button" onClick={onDone}>
        Done
      </button>
    </fieldset>
  );
}

/**
 * A call waiting for the owner (spec §7.3, §15.2): what it wants to do,
 * shown as plain text, and the four decisions with an optional note and an
 * editable scope for "for this session" and "always".
 */
export function ApprovalCard({
  approval,
  onDecide,
  context,
}: {
  approval: Approval;
  onDecide?: ApprovalDecide;
  /** Shown in the header, such as the session the call came from. */
  context?: ReactNode;
}) {
  const [note, setNote] = useState('');
  const [matcher, setMatcher] = useState<ApprovalMatcher>(
    approval.suggestedMatcher,
  );
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState<ApprovalDecision | null>(null);
  const [sent, setSent] = useState<ApprovalDecision | null>(null);
  const [error, setError] = useState<string | null>(null);
  const noteId = useId();

  const decide = async (decision: ApprovalDecision) => {
    if (!onDecide || busy) return;
    setBusy(decision);
    setError(null);
    const scoped = decision === 'allow_session' || decision === 'allow_always';
    const trimmed = note.trim();
    const failure = await onDecide(approval, {
      decision,
      ...(trimmed ? { note: trimmed } : {}),
      ...(scoped ? { matcher } : {}),
    });
    setBusy(null);
    if (failure) setError(failure);
    else setSent(decision);
  };

  return (
    <section
      className="approval-card"
      aria-label={`Approval needed: ${approval.tool}`}
    >
      <header className="approval-card-header">
        <strong className="approval-card-tool">{approval.tool}</strong>
        <span className="approval-card-class">
          {CLASS_LABELS[approval.class]}
        </span>
        {context}
      </header>
      <p className="approval-card-lead">This call waits for your approval.</p>
      {/* The model wrote these: a text node, never markup (spec §14). */}
      <pre className="approval-card-arguments" aria-label="Arguments">
        {approval.arguments}
      </pre>
      {approval.argumentsTruncated && (
        <p className="approval-card-note">Arguments shortened to 16 KiB.</p>
      )}
      {sent ? (
        <p className="approval-card-sent" role="status">
          {SENT[sent]}
        </p>
      ) : (
        <>
          <label className="approval-card-label" htmlFor={noteId}>
            Note (optional)
          </label>
          <textarea
            id={noteId}
            className="approval-card-input"
            rows={2}
            maxLength={MAX_APPROVAL_NOTE_CHARS}
            value={note}
            onChange={(event) => setNote(event.target.value)}
          />
          {editing ? (
            <MatcherEditor
              kinds={approval.matcherKinds}
              matcher={matcher}
              onChange={setMatcher}
              onDone={() => setEditing(false)}
            />
          ) : (
            <p className="approval-card-scope">
              For this session or always:{' '}
              {describeMatcher(approval.tool, matcher)}{' '}
              <button
                type="button"
                className="studio-tool-button"
                onClick={() => setEditing(true)}
              >
                Edit
              </button>
            </p>
          )}
          <div className="approval-card-actions">
            {DECISIONS.map((decision) => (
              <button
                key={decision}
                type="button"
                className={
                  decision === 'deny'
                    ? 'studio-tool-button approval-card-deny'
                    : 'studio-tool-button'
                }
                disabled={!onDecide || busy !== null}
                onClick={() => void decide(decision)}
              >
                {DECISION_LABELS[decision]}
              </button>
            ))}
          </div>
          {error && (
            <p className="approval-card-error" role="alert">
              {error}
            </p>
          )}
        </>
      )}
      <p className="approval-card-expiry">
        Waits until {formatTime(approval.expiresAtMs)}
      </p>
    </section>
  );
}
```

In `apps/web/src/lib/transcript.ts`, add `import type { ApprovalDecide } from './approvals';` and to `TranscriptActions`, after `onOpenSession`:

```text
  /** Sends the owner's decision from an inline approval card (spec §7.3). */
  onDecideApproval?: ApprovalDecide;
```

In `apps/web/src/hooks/useTranscriptActions.ts`:

1. Add `import { decideApproval as sendApprovalDecision, type ApprovalDecide } from '../lib/approvals';`.
2. Add to `TranscriptActionOptions`, after `openSession`:

```text
  /** Sends an approval decision; the daemon call by default. */
  decideApproval?: ApprovalDecide;
```

3. Destructure `decideApproval` in `useTranscriptActions`'s parameters, add it to `latest` (`const latest = { stopRun, cancelPending, sendAgain, compact, openSession, decideApproval };`), and add to the object `useMemo` returns, after `onOpenSession`:

```text
      onDecideApproval: (approval, input) =>
        (latestRef.current.decideApproval ?? sendApprovalDecision)(
          approval,
          input,
        ),
```

In `apps/web/src/components/sessions/RunActivity.tsx`:

1. Add `import { ApprovalCard } from './ApprovalCard';`.
2. After the `{(active || steps.length > 0) && ( <ToolBlock … /> )}` element, add:

```text
      {live.approvals.map((approval) => (
        <ApprovalCard
          key={approval.id}
          approval={approval}
          onDecide={actions?.onDecideApproval}
        />
      ))}
```

3. Inside the `<div role="status">` region, before the compacting paragraph, add:

```text
        {run.status === 'awaiting_approval' && (
          <p className="run-phase">Waiting for your approval…</p>
        )}
```

- [ ] **Step 5: Style the card**

Create `apps/web/src/approvals.css`:

```css
/* Approval cards and the Approvals page (spec §15.2, §15.4). */
.approval-card {
  display: flex;
  max-width: 85%;
  flex-direction: column;
  gap: 8px;
  align-self: flex-start;
  border: 1px solid var(--color-amber);
  border-radius: 12px;
  padding: 10px 12px;
  background: rgb(255 255 255 / 0.02);
}
.approval-card-header {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}
.approval-card-tool {
  color: var(--color-ink);
  font-family: var(--font-mono);
  font-size: 12px;
}
.approval-card-class,
.approval-card-lead,
.approval-card-note,
.approval-card-scope,
.approval-card-expiry {
  color: var(--color-ink-3);
  font-size: 12px;
}
.approval-card-arguments {
  max-height: 16rem;
  overflow: auto;
  border-radius: 8px;
  padding: 8px;
  background: var(--color-abyss);
  color: var(--color-ink-2);
  font-family: var(--font-mono);
  font-size: 11px;
  white-space: pre-wrap;
  word-break: break-word;
}
.approval-card-label {
  color: var(--color-ink-2);
  font-size: 11px;
}
.approval-card-input {
  width: 100%;
  border: 1px solid var(--color-line);
  border-radius: 8px;
  padding: 6px 8px;
  background: var(--color-panel);
  color: var(--color-ink);
  font-size: 12px;
}
.approval-card-matcher {
  display: flex;
  flex-direction: column;
  gap: 6px;
  border: 1px solid var(--color-line);
  border-radius: 8px;
  padding: 8px;
}
.approval-card-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.approval-card-deny,
.approval-card-error {
  color: var(--color-danger);
}
.approval-card-sent {
  color: var(--color-mint);
  font-size: 12px;
}
```

In `apps/web/src/styles.css`, add `@import './approvals.css';` after `@import './live-runs.css';`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd apps/web && bun x vitest run src/lib/approvals.test.ts src/components/sessions/ApprovalCard.test.tsx src/components/sessions/RunActivity.test.tsx src/hooks/useTranscriptActions.test.tsx src/visual-tokens.test.ts src/components/ChatScreen.test.tsx src/components/ChatScreen.memo.test.tsx`
Expected: PASS, with no `act()` warnings in the output.

- [ ] **Step 7: Format and commit**

Run: `bun x nx format:write --files=apps/web/src/lib/approvals.ts,apps/web/src/lib/approvals.test.ts,apps/web/src/components/sessions/ApprovalCard.tsx,apps/web/src/components/sessions/ApprovalCard.test.tsx,apps/web/src/approvals.css,apps/web/src/styles.css,apps/web/src/lib/daemon-api.ts,apps/web/src/lib/transcript.ts,apps/web/src/hooks/useTranscriptActions.ts,apps/web/src/hooks/useTranscriptActions.test.tsx,apps/web/src/components/sessions/RunActivity.tsx,apps/web/src/components/sessions/RunActivity.test.tsx && cd apps/web && bun x vitest run src/components/sessions/ApprovalCard.test.tsx src/components/sessions/RunActivity.test.tsx`
Expected: PASS.

```bash
git add apps/web/src/lib/approvals.ts apps/web/src/lib/approvals.test.ts apps/web/src/components/sessions/ApprovalCard.tsx apps/web/src/components/sessions/ApprovalCard.test.tsx apps/web/src/approvals.css apps/web/src/styles.css apps/web/src/lib/daemon-api.ts apps/web/src/lib/transcript.ts apps/web/src/hooks/useTranscriptActions.ts apps/web/src/hooks/useTranscriptActions.test.tsx apps/web/src/components/sessions/RunActivity.tsx apps/web/src/components/sessions/RunActivity.test.tsx
git commit -m "feat(web): decide approvals from inline cards in the transcript"
```

Recommended implementer tier: standard (a self-contained component with its own tests; the untrusted-text rendering is the part to review).

---

### Task 12: Web Approvals page: pending, rules and policy, decided history

**Files:**

- Create: `apps/web/src/hooks/useApprovals.ts`, `apps/web/src/hooks/useApprovals.test.tsx`, `apps/web/src/pages/ApprovalsPage.tsx`, `apps/web/src/pages/ApprovalsPage.test.tsx`
- Modify: `apps/web/src/lib/daemon-api.ts` (six approvals calls), `apps/web/src/lib/approvals.ts` (`approvalOutcome`, `resolvedAt`, `formatWhen`), `apps/web/src/lib/approvals.test.ts`, `apps/web/src/approvals.css` (page styles)

**Interfaces:**

- Consumes: Task 9 `ApprovalListOptions`, `ApprovalPolicy`, `ApprovalPolicyAction`, `ApprovalRule`, `ApprovalRuleInput`, `ApprovalTool`, `PolicyClass`, `POLICY_CLASSES`, `DEFAULT_APPROVAL_POLICY`, `DaemonHttpError`; Task 10 `pendingApprovals`, `LiveState`; Task 11 `ApprovalCard`, `decideApproval`, `ApprovalDecide`, `CLASS_LABELS`, `MATCHER_KIND_LABELS`, `MAX_MATCHER_VALUE_CHARS`, `describeMatcher`; `sessionKey` (`lib/session-groups`).
- Produces:
  - `daemon.{listApprovals(options: ApprovalListOptions), approvalPolicy(agentId, options?), setApprovalPolicy(agentId, policy), approvalRules(agentId, options?), addApprovalRule(agentId, input: ApprovalRuleInput), removeApprovalRule(agentId, ruleId)}`.
  - `useApprovals({ agentId, streamApprovals, streamOpen, epoch }): ApprovalsView` with `pending`, `decided`, `hasMoreDecided`, `loadMoreDecided()`, `policy`, `rules`, `tools`, `error`, `setPolicyAction(klass, action): Promise<void>`, `addRule(tool, matcher): Promise<boolean>`, `removeRule(rule): Promise<void>`, `refresh()`.
  - `ApprovalsPage({ agentId, live, streamOpen, sessions, onOpenSession })` (Task 13 mounts it at `#/approvals`).
  - `lib/approvals.ts`: `approvalOutcome(approval: Approval): string`, `resolvedAt(approval: Approval): number`, `formatWhen(ms: number): string`.
- Behavior: spec §15.4's Approvals page. "Waiting for you" shows the stream's pending approvals while it is open (read from `GET /api/approvals?status=pending` otherwise, and again after a decision made here) as `ApprovalCard`s with an "Open <session title>" button. "Rules" has one group per class (write, exec, network, delegate) with the class's policy select (`Allow`, `Ask me first`, `Deny`), its rules with Remove, and an Add-rule form (tool, `Match by`, `Match value`). "Decided in the last 30 days" lists outcomes newest first, notes as text, and "Show older" pages on. The decided list is read again whenever the stream's pending set changes or a snapshot arrives. Reads are aborted on unmount; failures show one alert.

- [ ] **Step 1: Write the failing tests**

Add to `apps/web/src/lib/approvals.test.ts` (and `approvalOutcome` to its `./approvals` import):

```ts
describe('approvalOutcome', () => {
  it('says how each approval ended', () => {
    const resolution = {
      decision: null,
      note: null,
      matcher: null,
      ruleId: null,
      resolvedBy: 'owner' as const,
      resolvedAtMs: 5,
    };
    const ended = (
      status: 'allowed' | 'denied' | 'stopped' | 'expired',
      overrides: Partial<typeof resolution> = {},
    ) =>
      approvalOutcome(
        approvalFixture('apr_1', {
          status,
          resolution: { ...resolution, ...overrides },
        }),
      );

    expect(ended('allowed', { decision: 'allow_once' })).toBe('Allowed once');
    expect(ended('allowed', { decision: 'allow_session' })).toBe(
      'Allowed for the session',
    );
    expect(ended('allowed', { decision: 'allow_always' })).toBe(
      'Always allowed',
    );
    expect(ended('denied', { decision: 'deny' })).toBe('Denied');
    expect(ended('denied', { decision: 'deny', resolvedBy: 'timeout' })).toBe(
      'Timed out',
    );
    expect(ended('stopped', { resolvedBy: 'stop' })).toBe(
      'Stopped with its run',
    );
    expect(ended('expired', { resolvedBy: 'restart' })).toBe(
      'Expired at a restart',
    );
    expect(approvalOutcome(approvalFixture('apr_2'))).toBe('Waiting');
  });
});
```

Create `apps/web/src/hooks/useApprovals.test.tsx`:

```tsx
import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_APPROVAL_POLICY } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { approvalFixture } from '../test/live';
import { useApprovals } from './useApprovals';

beforeEach(() => {
  vi.spyOn(daemon, 'listApprovals').mockResolvedValue({
    approvals: [],
    nextCursor: null,
  });
  vi.spyOn(daemon, 'approvalPolicy').mockResolvedValue(DEFAULT_APPROVAL_POLICY);
  vi.spyOn(daemon, 'approvalRules').mockResolvedValue({ rules: [], tools: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useApprovals', () => {
  it('reads the decided list again once a pending approval resolves', async () => {
    const approval = approvalFixture('apr_1');
    const { result, rerender } = renderHook(
      (props: { approvals: Record<string, typeof approval> }) =>
        useApprovals({
          agentId: 'agent-main',
          streamApprovals: props.approvals,
          streamOpen: true,
          epoch: 1,
        }),
      { initialProps: { approvals: { apr_1: approval } } },
    );
    await waitFor(() => expect(result.current.policy).not.toBeNull());
    expect(result.current.pending).toEqual([approval]);
    const decidedReads = () =>
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.filter(([options]) => options.status === 'decided').length;
    expect(decidedReads()).toBe(1);
    expect(
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.some(([options]) => options.status === 'pending'),
    ).toBe(false);

    rerender({ approvals: {} });

    await waitFor(() => expect(decidedReads()).toBe(2));
    expect(result.current.pending).toEqual([]);
  });

  it('reads pending approvals itself while the stream is closed, and aborts on unmount', async () => {
    const approval = approvalFixture('apr_1');
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) => ({
      approvals: options.status === 'pending' ? [approval] : [],
      nextCursor: null,
    }));
    const { result, unmount } = renderHook(() =>
      useApprovals({
        agentId: 'agent-main',
        streamApprovals: {},
        streamOpen: false,
        epoch: 0,
      }),
    );

    await waitFor(() => expect(result.current.pending).toEqual([approval]));
    const signals = vi
      .mocked(daemon.listApprovals)
      .mock.calls.map(([options]) => options.signal);
    unmount();
    expect(signals.every((signal) => signal?.aborted)).toBe(true);
  });
});
```

Create `apps/web/src/pages/ApprovalsPage.test.tsx`:

```tsx
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DaemonHttpError,
  DEFAULT_APPROVAL_POLICY,
  type Approval,
  type ApprovalRule,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { EMPTY_LIVE_STATE, type LiveState } from '../lib/session-events';
import { approvalFixture } from '../test/live';
import { sessionFixture } from '../test/sessions';
import { ApprovalsPage } from './ApprovalsPage';

const gitRule: ApprovalRule = {
  id: 'rule_1',
  agentId: 'agent-main',
  tool: 'bash',
  matcher: { kind: 'command_prefix', value: 'git status' },
  createdAtMs: 1,
  fromApprovalId: null,
};

const tools = [
  {
    name: 'bash',
    class: 'exec' as const,
    matcherKinds: ['command_prefix' as const, 'any' as const],
  },
  {
    name: 'memory_add',
    class: 'write' as const,
    matcherKinds: ['any' as const],
  },
];

function streamWith(...approvals: Approval[]): LiveState {
  return {
    ...EMPTY_LIVE_STATE,
    approvals: Object.fromEntries(
      approvals.map((approval) => [approval.id, approval]),
    ),
  };
}

function renderPage(props: Partial<Parameters<typeof ApprovalsPage>[0]> = {}) {
  const onOpenSession = vi.fn();
  render(
    <ApprovalsPage
      agentId="agent-main"
      live={EMPTY_LIVE_STATE}
      streamOpen
      sessions={[sessionFixture('chat:1', { title: 'Weekend plans' })]}
      onOpenSession={onOpenSession}
      {...props}
    />,
  );
  return { onOpenSession };
}

beforeEach(() => {
  vi.spyOn(daemon, 'listApprovals').mockResolvedValue({
    approvals: [],
    nextCursor: null,
  });
  vi.spyOn(daemon, 'approvalPolicy').mockResolvedValue(DEFAULT_APPROVAL_POLICY);
  vi.spyOn(daemon, 'approvalRules').mockResolvedValue({
    rules: [gitRule],
    tools,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ApprovalsPage', () => {
  it('shows the stream’s pending approvals and decides them', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const decide = vi
      .spyOn(daemon, 'decideApproval')
      .mockResolvedValue({ ...approval, status: 'allowed', revision: 2 });
    const { onOpenSession } = renderPage({ live: streamWith(approval) });
    // The page's reads settle first, so no update lands outside act().
    expect(
      await screen.findByText(
        'Always allow bash commands starting with “git status”',
      ),
    ).toBeVisible();

    const waiting = screen.getByRole('region', { name: 'Waiting for you' });
    const card = within(waiting).getByRole('region', {
      name: 'Approval needed: bash',
    });
    await user.click(
      within(card).getByRole('button', { name: 'Open Weekend plans' }),
    );
    expect(onOpenSession).toHaveBeenCalledWith(approval);
    await user.click(within(card).getByRole('button', { name: 'Allow once' }));
    expect(decide).toHaveBeenCalledWith('apr_1', {
      decision: 'allow_once',
      revision: 1,
    });
    expect(
      await within(card).findByText('Allowed once. Continuing…'),
    ).toBeVisible();
    expect(
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.some(([options]) => options.status === 'pending'),
    ).toBe(false);
  });

  it('reads pending approvals when the stream is closed, and again after a decision', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) => ({
      approvals: options.status === 'pending' ? [approval] : [],
      nextCursor: null,
    }));
    vi.spyOn(daemon, 'decideApproval').mockResolvedValue({
      ...approval,
      status: 'denied',
      revision: 2,
    });
    renderPage({ streamOpen: false });

    const card = await screen.findByRole('region', {
      name: 'Approval needed: bash',
    });
    const pendingReads = () =>
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.filter(([options]) => options.status === 'pending').length;
    expect(pendingReads()).toBe(1);
    await user.click(within(card).getByRole('button', { name: 'Deny' }));
    await waitFor(() => expect(pendingReads()).toBe(2));
  });

  it('lists decided approvals newest first and loads older ones', async () => {
    const user = userEvent.setup();
    const resolution = (
      overrides: Partial<Approval['resolution'] & object>,
    ) => ({
      decision: null,
      note: null,
      matcher: null,
      ruleId: null,
      resolvedBy: 'owner' as const,
      resolvedAtMs: 10,
      ...overrides,
    });
    const always = approvalFixture('apr_new', {
      status: 'allowed',
      resolution: resolution({ decision: 'allow_always' }),
    });
    const timedOut = approvalFixture('apr_old', {
      tool: 'web_fetch',
      status: 'denied',
      resolution: resolution({
        decision: 'deny',
        resolvedBy: 'timeout',
        note: '<b>Approval timed out</b>',
      }),
    });
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) =>
      options.cursor
        ? { approvals: [timedOut], nextCursor: null }
        : { approvals: [always], nextCursor: '10:apr_new' },
    );
    renderPage();

    const decided = screen.getByRole('region', {
      name: 'Decided in the last 30 days',
    });
    expect(await within(decided).findByText('Always allowed')).toBeVisible();
    await user.click(
      within(decided).getByRole('button', { name: 'Show older' }),
    );
    expect(await within(decided).findByText('Timed out')).toBeVisible();
    expect(
      within(decided).getByText('“<b>Approval timed out</b>”'),
    ).toBeVisible();
    expect(
      within(decided).queryByRole('button', { name: 'Show older' }),
    ).not.toBeInTheDocument();
    expect(daemon.listApprovals).toHaveBeenCalledWith({
      status: 'decided',
      agentId: 'agent-main',
      cursor: '10:apr_new',
    });
  });

  it('sets each class’s policy and manages its rules', async () => {
    const user = userEvent.setup();
    const setPolicy = vi
      .spyOn(daemon, 'setApprovalPolicy')
      .mockImplementation(async (_agentId, policy) => policy);
    const remove = vi.spyOn(daemon, 'removeApprovalRule').mockResolvedValue();
    const added: ApprovalRule = {
      ...gitRule,
      id: 'rule_2',
      matcher: { kind: 'command_prefix', value: 'npm test' },
    };
    const add = vi.spyOn(daemon, 'addApprovalRule').mockResolvedValue(added);
    renderPage();

    const exec = await screen.findByRole('region', { name: 'Runs commands' });
    const policy = within(exec).getByRole('combobox', {
      name: 'Policy for Runs commands',
    });
    await waitFor(() => expect(policy).toBeEnabled());
    await user.selectOptions(policy, 'Deny');
    expect(setPolicy).toHaveBeenCalledWith('agent-main', {
      ...DEFAULT_APPROVAL_POLICY,
      exec: 'deny',
    });

    expect(
      within(exec).getByText(
        'Always allow bash commands starting with “git status”',
      ),
    ).toBeVisible();
    const form = within(exec).getByRole('form', {
      name: 'Add a rule for Runs commands',
    });
    await user.type(within(form).getByLabelText('Match value'), 'npm test');
    await user.click(within(form).getByRole('button', { name: 'Add rule' }));
    expect(add).toHaveBeenCalledWith('agent-main', {
      tool: 'bash',
      matcher: { kind: 'command_prefix', value: 'npm test' },
    });
    expect(
      await within(exec).findByText(
        'Always allow bash commands starting with “npm test”',
      ),
    ).toBeVisible();

    await user.click(
      within(exec).getByRole('button', {
        name: 'Remove rule: bash commands starting with “git status”',
      }),
    );
    expect(remove).toHaveBeenCalledWith('agent-main', 'rule_1');
    await waitFor(() =>
      expect(
        within(exec).queryByText(
          'Always allow bash commands starting with “git status”',
        ),
      ).not.toBeInTheDocument(),
    );
    const write = screen.getByRole('region', {
      name: 'Changes files and records',
    });
    expect(within(write).getByText('No rules yet.')).toBeVisible();
  });

  it('shows why a change did not go through', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'setApprovalPolicy').mockRejectedValue(
      new DaemonHttpError(503, { error: 'control plane save failed' }),
    );
    renderPage();

    const policy = await screen.findByRole('combobox', {
      name: 'Policy for Uses the internet',
    });
    await waitFor(() => expect(policy).toBeEnabled());
    await user.selectOptions(policy, 'Ask me first');

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'control plane save failed',
    );
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/web && bun x vitest run src/lib/approvals.test.ts src/hooks/useApprovals.test.tsx src/pages/ApprovalsPage.test.tsx`
Expected: FAIL — `approvalOutcome`, `useApprovals`, `ApprovalsPage`, and the `daemon` approvals calls do not exist.

- [ ] **Step 3: Add the daemon calls and the helpers**

In `apps/web/src/lib/daemon-api.ts`, add `type ApprovalListOptions, type ApprovalPolicy, type ApprovalRuleInput,` to the `@animaOS-SWARM/sdk` import and, after `decideApproval`:

```ts
  listApprovals: (options: ApprovalListOptions) =>
    setupClient.approvals.list(options),
  approvalPolicy: (agentId: string, options: { signal?: AbortSignal } = {}) =>
    setupClient.approvals.policy(agentId, options),
  setApprovalPolicy: (agentId: string, policy: ApprovalPolicy) =>
    setupClient.approvals.setPolicy(agentId, policy),
  approvalRules: (agentId: string, options: { signal?: AbortSignal } = {}) =>
    setupClient.approvals.rules(agentId, options),
  addApprovalRule: (agentId: string, input: ApprovalRuleInput) =>
    setupClient.approvals.addRule(agentId, input),
  removeApprovalRule: (agentId: string, ruleId: string) =>
    setupClient.approvals.removeRule(agentId, ruleId),
```

Add to `apps/web/src/lib/approvals.ts`:

```ts
/** How an approval ended, in a few words. */
export function approvalOutcome(approval: Approval): string {
  const resolution = approval.resolution;
  switch (approval.status) {
    case 'pending':
      return 'Waiting';
    case 'stopped':
      return 'Stopped with its run';
    case 'expired':
      return 'Expired at a restart';
    case 'denied':
      return resolution?.resolvedBy === 'timeout' ? 'Timed out' : 'Denied';
    case 'allowed':
      return resolution?.decision === 'allow_session'
        ? 'Allowed for the session'
        : resolution?.decision === 'allow_always'
          ? 'Always allowed'
          : 'Allowed once';
  }
}

/** When it was settled, or asked while it is still pending. */
export function resolvedAt(approval: Approval): number {
  return approval.resolution?.resolvedAtMs ?? approval.createdAtMs;
}

export function formatWhen(ms: number): string {
  return new Date(ms).toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}
```

- [ ] **Step 4: Implement the hook and the page**

Create `apps/web/src/hooks/useApprovals.ts`:

```ts
import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  DaemonHttpError,
  DEFAULT_APPROVAL_POLICY,
  type Approval,
  type ApprovalMatcher,
  type ApprovalPolicy,
  type ApprovalPolicyAction,
  type ApprovalRule,
  type ApprovalTool,
  type PolicyClass,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { pendingApprovals, type LiveState } from '../lib/session-events';

export interface ApprovalsOptions {
  agentId: string;
  /** The companion stream's pending approvals. */
  streamApprovals: LiveState['approvals'];
  /** While open, the stream's pending approvals are the ones shown. */
  streamOpen: boolean;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
}

export interface ApprovalsView {
  /** Waiting for the owner, oldest first. */
  pending: Approval[];
  /** Decided in the last 30 days, newest first. */
  decided: Approval[];
  hasMoreDecided: boolean;
  loadMoreDecided: () => void;
  /** Null until read. */
  policy: ApprovalPolicy | null;
  rules: ApprovalRule[];
  /** The tools a rule can cover. */
  tools: ApprovalTool[];
  error: string | null;
  setPolicyAction: (
    klass: PolicyClass,
    action: ApprovalPolicyAction,
  ) => Promise<void>;
  /** True when the daemon kept the rule. */
  addRule: (tool: string, matcher: ApprovalMatcher) => Promise<boolean>;
  removeRule: (rule: ApprovalRule) => Promise<void>;
  /** Reads the lists again, as a decision made without the stream needs. */
  refresh: () => void;
}

function message(error: unknown): string {
  return error instanceof DaemonHttpError
    ? error.message
    : 'Could not reach your companion. Try again.';
}

/** The Approvals page's data (spec §15.4, §15.5 `useApprovals`). */
export function useApprovals({
  agentId,
  streamApprovals,
  streamOpen,
  epoch,
}: ApprovalsOptions): ApprovalsView {
  const [readPending, setReadPending] = useState<Approval[]>([]);
  const [decided, setDecided] = useState<Approval[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [policy, setPolicy] = useState<ApprovalPolicy | null>(null);
  const [rules, setRules] = useState<ApprovalRule[]>([]);
  const [tools, setTools] = useState<ApprovalTool[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [reads, setReads] = useState(0);
  const refresh = useCallback(() => setReads((value) => value + 1), []);
  // A settled approval leaves the stream's pending set: the decided list
  // has a new entry.
  const pendingKey = Object.keys(streamApprovals).sort().join('\u0000');

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    void Promise.all([
      daemon.listApprovals({ status: 'decided', agentId, signal }),
      streamOpen
        ? null
        : daemon.listApprovals({ status: 'pending', agentId, signal }),
    ]).then(
      ([decidedPage, pendingPage]) => {
        if (signal.aborted) return;
        setDecided(decidedPage.approvals);
        setCursor(decidedPage.nextCursor);
        if (pendingPage) setReadPending(pendingPage.approvals);
        setError(null);
      },
      (caught: unknown) => {
        if (!signal.aborted) setError(message(caught));
      },
    );
    return () => controller.abort();
  }, [agentId, streamOpen, reads, pendingKey, epoch]);

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    void Promise.all([
      daemon.approvalPolicy(agentId, { signal }),
      daemon.approvalRules(agentId, { signal }),
    ]).then(
      ([nextPolicy, nextRules]) => {
        if (signal.aborted) return;
        setPolicy(nextPolicy);
        setRules(nextRules.rules);
        setTools(nextRules.tools);
      },
      (caught: unknown) => {
        if (!signal.aborted) setError(message(caught));
      },
    );
    return () => controller.abort();
  }, [agentId, reads]);

  const pending = useMemo(
    () => (streamOpen ? pendingApprovals(streamApprovals) : readPending),
    [streamOpen, streamApprovals, readPending],
  );

  const loadMoreDecided = useCallback(() => {
    if (!cursor) return;
    void daemon.listApprovals({ status: 'decided', agentId, cursor }).then(
      (page) => {
        setDecided((current) => [
          ...current,
          ...page.approvals.filter(
            (approval) => !current.some((known) => known.id === approval.id),
          ),
        ]);
        setCursor(page.nextCursor);
      },
      (caught: unknown) => setError(message(caught)),
    );
  }, [agentId, cursor]);

  const setPolicyAction = useCallback(
    async (klass: PolicyClass, action: ApprovalPolicyAction) => {
      const next = { ...(policy ?? DEFAULT_APPROVAL_POLICY), [klass]: action };
      try {
        setPolicy(await daemon.setApprovalPolicy(agentId, next));
        setError(null);
      } catch (caught) {
        setError(message(caught));
      }
    },
    [agentId, policy],
  );

  const addRule = useCallback(
    async (tool: string, matcher: ApprovalMatcher) => {
      try {
        const rule = await daemon.addApprovalRule(agentId, { tool, matcher });
        setRules((current) =>
          current.some((known) => known.id === rule.id)
            ? current
            : [...current, rule],
        );
        setError(null);
        return true;
      } catch (caught) {
        setError(message(caught));
        return false;
      }
    },
    [agentId],
  );

  const removeRule = useCallback(
    async (rule: ApprovalRule) => {
      try {
        await daemon.removeApprovalRule(agentId, rule.id);
        setRules((current) => current.filter((known) => known.id !== rule.id));
        setError(null);
      } catch (caught) {
        setError(message(caught));
      }
    },
    [agentId],
  );

  return {
    pending,
    decided,
    hasMoreDecided: cursor !== null,
    loadMoreDecided,
    policy,
    rules,
    tools,
    error,
    setPolicyAction,
    addRule,
    removeRule,
    refresh,
  };
}
```

Create `apps/web/src/pages/ApprovalsPage.tsx`:

```tsx
import { useState, type FormEvent } from 'react';
import {
  DEFAULT_APPROVAL_POLICY,
  POLICY_CLASSES,
  type Approval,
  type ApprovalMatcherKind,
  type ApprovalPolicyAction,
  type PolicyClass,
  type Session,
} from '@animaOS-SWARM/sdk';

import { ApprovalCard } from '../components/sessions/ApprovalCard';
import { useApprovals, type ApprovalsView } from '../hooks/useApprovals';
import {
  CLASS_LABELS,
  MATCHER_KIND_LABELS,
  MAX_MATCHER_VALUE_CHARS,
  approvalOutcome,
  decideApproval,
  describeMatcher,
  formatWhen,
  resolvedAt,
  type ApprovalDecide,
} from '../lib/approvals';
import type { LiveState } from '../lib/session-events';
import { sessionKey } from '../lib/session-groups';

const ACTIONS: readonly ApprovalPolicyAction[] = ['allow', 'ask', 'deny'];
const ACTION_LABELS: Record<ApprovalPolicyAction, string> = {
  allow: 'Allow',
  ask: 'Ask me first',
  deny: 'Deny',
};

function ClassRules({
  klass,
  view,
}: {
  klass: PolicyClass;
  view: ApprovalsView;
}) {
  const label = CLASS_LABELS[klass];
  const tools = view.tools.filter((tool) => tool.class === klass);
  const rules = view.rules.filter((rule) =>
    tools.some((tool) => tool.name === rule.tool),
  );
  const [toolName, setToolName] = useState('');
  const [kind, setKind] = useState<ApprovalMatcherKind | null>(null);
  const [value, setValue] = useState('');
  const tool = tools.find((item) => item.name === toolName) ?? tools[0];
  const matcherKind =
    tool && kind && tool.matcherKinds.includes(kind)
      ? kind
      : tool?.matcherKinds[0];

  const add = async (event: FormEvent) => {
    event.preventDefault();
    if (!tool || !matcherKind) return;
    const kept = await view.addRule(tool.name, {
      kind: matcherKind,
      value: matcherKind === 'any' ? '' : value.trim(),
    });
    if (kept) setValue('');
  };

  return (
    <section className="approvals-class" aria-label={label}>
      <h3>{label}</h3>
      <label className="approvals-policy">
        <span>When one of these tools runs</span>
        <select
          aria-label={`Policy for ${label}`}
          value={view.policy?.[klass] ?? DEFAULT_APPROVAL_POLICY[klass]}
          disabled={view.policy === null}
          onChange={(event) =>
            void view.setPolicyAction(
              klass,
              event.target.value as ApprovalPolicyAction,
            )
          }
        >
          {ACTIONS.map((action) => (
            <option key={action} value={action}>
              {ACTION_LABELS[action]}
            </option>
          ))}
        </select>
      </label>
      {rules.length === 0 ? (
        <p className="approvals-empty">No rules yet.</p>
      ) : (
        <ul className="approvals-rules">
          {rules.map((rule) => {
            const covers = describeMatcher(rule.tool, rule.matcher);
            return (
              <li key={rule.id}>
                <span>Always allow {covers}</span>
                <button
                  type="button"
                  className="studio-tool-button"
                  aria-label={`Remove rule: ${covers}`}
                  onClick={() => void view.removeRule(rule)}
                >
                  Remove
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {tool && matcherKind && (
        <form
          className="approvals-add-rule"
          aria-label={`Add a rule for ${label}`}
          onSubmit={(event) => void add(event)}
        >
          <select
            aria-label="Tool"
            value={tool.name}
            onChange={(event) => setToolName(event.target.value)}
          >
            {tools.map((item) => (
              <option key={item.name} value={item.name}>
                {item.name}
              </option>
            ))}
          </select>
          <select
            aria-label="Match by"
            value={matcherKind}
            onChange={(event) =>
              setKind(event.target.value as ApprovalMatcherKind)
            }
          >
            {tool.matcherKinds.map((item) => (
              <option key={item} value={item}>
                {MATCHER_KIND_LABELS[item]}
              </option>
            ))}
          </select>
          {matcherKind !== 'any' && (
            <input
              aria-label="Match value"
              value={value}
              maxLength={MAX_MATCHER_VALUE_CHARS}
              onChange={(event) => setValue(event.target.value)}
            />
          )}
          <button
            type="submit"
            className="studio-tool-button"
            disabled={matcherKind !== 'any' && !value.trim()}
          >
            Add rule
          </button>
        </form>
      )}
    </section>
  );
}

export interface ApprovalsPageProps {
  agentId: string;
  /** The companion's live stream state. */
  live: LiveState;
  streamOpen: boolean;
  /** The sessions list, for each card's "Open" button. */
  sessions: readonly Session[];
  onOpenSession: (approval: Approval) => void;
}

/** Spec §15.4: pending cards, rules and policy per class, and the 30-day
 *  decided history. */
export function ApprovalsPage({
  agentId,
  live,
  streamOpen,
  sessions,
  onOpenSession,
}: ApprovalsPageProps) {
  const view = useApprovals({
    agentId,
    streamApprovals: live.approvals,
    streamOpen,
    epoch: live.epoch,
  });
  const titles = new Map(
    sessions.map((session) => [sessionKey(session), session.title]),
  );
  const decide: ApprovalDecide = async (approval, input) => {
    const failure = await decideApproval(approval, input);
    // Without the stream nothing else tells this page it went through.
    if (!failure && !streamOpen) view.refresh();
    return failure;
  };

  return (
    <div className="approvals-page">
      {view.error && (
        <p className="approvals-error" role="alert">
          {view.error}
        </p>
      )}
      <section
        className="approvals-section"
        aria-labelledby="approvals-waiting"
      >
        <h2 id="approvals-waiting">Waiting for you</h2>
        {view.pending.length === 0 ? (
          <p className="approvals-empty">
            Nothing is waiting for your approval.
          </p>
        ) : (
          <div className="approvals-cards">
            {view.pending.map((approval) => (
              <ApprovalCard
                key={approval.id}
                approval={approval}
                onDecide={decide}
                context={
                  <button
                    type="button"
                    className="studio-tool-button"
                    onClick={() => onOpenSession(approval)}
                  >
                    Open{' '}
                    {titles.get(
                      sessionKey({
                        agentId: approval.agentId,
                        id: approval.sessionId,
                      }),
                    ) ?? 'chat'}
                  </button>
                }
              />
            ))}
          </div>
        )}
      </section>
      <section className="approvals-section" aria-labelledby="approvals-rules">
        <h2 id="approvals-rules">Rules</h2>
        <p className="approvals-help">
          Tools that only read never ask. For the others, choose what happens,
          and which calls are always allowed.
        </p>
        {POLICY_CLASSES.map((klass) => (
          <ClassRules key={klass} klass={klass} view={view} />
        ))}
      </section>
      <section
        className="approvals-section"
        aria-labelledby="approvals-decided"
      >
        <h2 id="approvals-decided">Decided in the last 30 days</h2>
        {view.decided.length === 0 ? (
          <p className="approvals-empty">No decisions in the last 30 days.</p>
        ) : (
          <ul className="approvals-decided">
            {view.decided.map((approval) => (
              <li key={approval.id}>
                <strong>{approval.tool}</strong>
                <span>{approvalOutcome(approval)}</span>
                {approval.resolution?.note && (
                  <span className="approvals-note">
                    “{approval.resolution.note}”
                  </span>
                )}
                <time dateTime={new Date(resolvedAt(approval)).toISOString()}>
                  {formatWhen(resolvedAt(approval))}
                </time>
              </li>
            ))}
          </ul>
        )}
        {view.hasMoreDecided && (
          <button
            type="button"
            className="studio-tool-button"
            onClick={view.loadMoreDecided}
          >
            Show older
          </button>
        )}
      </section>
    </div>
  );
}
```

Append to `apps/web/src/approvals.css`:

```css
.approvals-page {
  display: flex;
  flex-direction: column;
  gap: 24px;
  overflow-y: auto;
  padding: 24px;
}
.approvals-section {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.approvals-section h2 {
  color: var(--color-ink);
  font-size: 15px;
  font-weight: 600;
}
.approvals-cards {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.approvals-cards .approval-card {
  max-width: 100%;
}
.approvals-class {
  display: flex;
  flex-direction: column;
  gap: 8px;
  border: 1px solid var(--color-line);
  border-radius: 12px;
  padding: 12px;
}
.approvals-class h3 {
  color: var(--color-ink-2);
  font-size: 13px;
  font-weight: 600;
}
.approvals-policy,
.approvals-add-rule {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  color: var(--color-ink-3);
  font-size: 12px;
}
.approvals-policy select,
.approvals-add-rule select,
.approvals-add-rule input {
  border: 1px solid var(--color-line);
  border-radius: 8px;
  padding: 4px 8px;
  background: var(--color-panel);
  color: var(--color-ink);
  font-size: 12px;
}
.approvals-rules,
.approvals-decided {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.approvals-rules li,
.approvals-decided li {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  color: var(--color-ink-2);
  font-size: 12px;
}
.approvals-empty,
.approvals-help,
.approvals-note,
.approvals-decided time {
  color: var(--color-ink-3);
  font-size: 12px;
}
.approvals-error {
  color: var(--color-danger);
  font-size: 12px;
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd apps/web && bun x vitest run src/lib/approvals.test.ts src/hooks/useApprovals.test.tsx src/pages/ApprovalsPage.test.tsx src/visual-tokens.test.ts`
Expected: PASS, with no `act()` warnings.

- [ ] **Step 6: Format and commit**

Run: `bun x nx format:write --files=apps/web/src/hooks/useApprovals.ts,apps/web/src/hooks/useApprovals.test.tsx,apps/web/src/pages/ApprovalsPage.tsx,apps/web/src/pages/ApprovalsPage.test.tsx,apps/web/src/lib/daemon-api.ts,apps/web/src/lib/approvals.ts,apps/web/src/lib/approvals.test.ts,apps/web/src/approvals.css && cd apps/web && bun x vitest run src/pages/ApprovalsPage.test.tsx src/hooks/useApprovals.test.tsx`
Expected: PASS.

```bash
git add apps/web/src/hooks/useApprovals.ts apps/web/src/hooks/useApprovals.test.tsx apps/web/src/pages/ApprovalsPage.tsx apps/web/src/pages/ApprovalsPage.test.tsx apps/web/src/lib/daemon-api.ts apps/web/src/lib/approvals.ts apps/web/src/lib/approvals.test.ts apps/web/src/approvals.css
git commit -m "feat(web): add the Approvals page with pending cards, rules, policy, and history"
```

Recommended implementer tier: standard (a page over a hook with mocked daemon calls; watch for act() warnings from the async reads).

---

### Task 13: Web shell: the Approvals destination, badges, ⌘K additions, and wiring

**Files:**

- Modify: `apps/web/src/components/WorkspaceShell.tsx`, `apps/web/src/components/WorkspaceShell.test.tsx`
- Modify: `apps/web/src/components/sessions/SessionSidebar.tsx`, `apps/web/src/components/sessions/SessionSidebar.test.tsx`
- Modify: `apps/web/src/lib/approvals.ts` (`pendingApprovalCount`), `apps/web/src/lib/approvals.test.ts`, `apps/web/src/approvals.css` (badges)
- Modify: `apps/web/src/ViewHarness.tsx` (four `WorkspaceShell` props), `apps/web/src/ViewHarness.test.tsx`

**Interfaces:**

- Consumes: Task 12 `ApprovalsPage`; Task 10 `LiveState.approvals`; M3 `useLiveSession` (`live.state`, `live.status`), `useCompanionSessions` (`sessions.sessions`), `commands.openTarget(target: { agentId, sessionId })`, the harness's `openSession(session: Session)`; `Session.pendingApprovals` (derived by the daemon since Task 7); `sessionKey`; `ShieldIcon` (`components/icons`); the ⌘K `CommandMenu` and its `StudioCommand { id, title, description, group, run }`.
- Produces:
  - `AVAILABLE_PAGES` gains `'approvals'`; the Approvals destination (first primary destination, `ShieldIcon`) in the sidebar and the mobile dock, named `Approvals, N waiting` with a badge while N > 0.
  - `WorkspaceShell` props: `approvals?: ReactNode | null` (rendered at `#/approvals`), `pendingApprovals?: number`, `sessions?: readonly Session[]`, `onOpenSession?: (session: Session) => void`; `MAX_SESSION_COMMANDS = 50`.
  - ⌘K commands (spec §15.3, the M3 carry-forward): `New chat` (kept), `Review approvals` (`N waiting for you` / `Nothing is waiting`), `Go to Approvals` (from the destinations), and one `Sessions` command per listed, unarchived session title (at most 50, newest activity first).
  - `SessionSidebar` rows: a badge with the session's pending count and `needs approval` in the row's accessible name.
  - `pendingApprovalCount(approvals: LiveState['approvals'], streamOpen: boolean, sessions: readonly Session[]): number`.
- Behavior: the badge counts the stream's pending approvals while it is open (the companion's and its helpers'), else the listed sessions' `pendingApprovals`. The harness mounts `ApprovalsPage` for `#/approvals` (its "Open" button opens the approval's session through `openTarget`, so a helper's session opens too); a reload on `#/approvals` restores the page. The inline cards need no harness wiring: `useTranscriptActions` sends decisions itself (Task 11).

- [ ] **Step 1: Write the failing tests**

Add to `apps/web/src/lib/approvals.test.ts` (and `pendingApprovalCount` to its `./approvals` import, `sessionFixture` from `../test/sessions`):

```ts
describe('pendingApprovalCount', () => {
  it('counts the stream’s approvals while it is open, else the sessions’', () => {
    const approvals = {
      apr_1: approvalFixture('apr_1'),
      apr_2: approvalFixture('apr_2'),
    };
    const sessions = [
      sessionFixture('chat:1', { pendingApprovals: 1 }),
      sessionFixture('chat:2', { pendingApprovals: 3 }),
    ];
    expect(pendingApprovalCount(approvals, true, sessions)).toBe(2);
    expect(pendingApprovalCount(approvals, false, sessions)).toBe(4);
    expect(pendingApprovalCount({}, true, sessions)).toBe(0);
  });
});
```

In `apps/web/src/components/WorkspaceShell.test.tsx`:

1. In `shows the conversation for pages that arrive in later releases`, replace `{ kind: 'page', page: 'approvals' }` with `{ kind: 'page', page: 'automations' }`.
2. Add these tests inside `describe('WorkspaceShell', …)`:

```tsx
it('opens Approvals from the navigation with its waiting badge', async () => {
  const user = userEvent.setup();
  render(<Shell approvals={<div>Approvals page</div>} pendingApprovals={2} />);
  const nav = screen.getByRole('navigation', {
    name: 'Workspace navigation',
  });

  await user.click(
    within(nav).getByRole('button', { name: 'Approvals, 2 waiting' }),
  );

  expect(screen.getByText('Approvals page')).toBeVisible();
  expect(screen.getByText('Workspace canvas')).not.toBeVisible();
  expect(
    within(nav).getByRole('button', { name: 'Approvals, 2 waiting' }),
  ).toHaveAttribute('aria-current', 'page');
});

it('names the destination plainly when nothing waits', () => {
  render(<Shell approvals={<div>Approvals page</div>} />);
  expect(
    within(
      screen.getByRole('navigation', { name: 'Workspace navigation' }),
    ).getByRole('button', { name: 'Approvals' }),
  ).toBeVisible();
});

it('reviews approvals and opens sessions by title from commands', async () => {
  const user = userEvent.setup();
  const onOpenSession = vi.fn();
  const plans = sessionFixture('chat:plans', { title: 'Weekend plans' });
  render(
    <Shell
      approvals={<div>Approvals page</div>}
      pendingApprovals={1}
      sessions={[
        plans,
        sessionFixture('chat:old', { title: 'Old archive', archived: true }),
      ]}
      onOpenSession={onOpenSession}
    />,
  );

  await user.keyboard('{Control>}k{/Control}');
  expect(
    screen.getByRole('option', { name: /Review approvals/ }),
  ).toHaveTextContent('1 waiting for you');
  expect(screen.getByRole('option', { name: /Go to Approvals/ })).toBeVisible();
  expect(
    screen.queryByRole('option', { name: /Old archive/ }),
  ).not.toBeInTheDocument();
  await user.type(
    screen.getByRole('combobox', { name: 'Search commands' }),
    'weekend',
  );
  await user.keyboard('{Enter}');
  expect(onOpenSession).toHaveBeenCalledWith(plans);

  await user.keyboard('{Control>}k{/Control}');
  await user.type(
    screen.getByRole('combobox', { name: 'Search commands' }),
    'review',
  );
  await user.keyboard('{Enter}');
  expect(screen.getByText('Approvals page')).toBeVisible();
});
```

In `apps/web/src/components/sessions/SessionSidebar.test.tsx`, add inside `describe('SessionSidebar', …)`:

```tsx
it('marks a session that waits for an approval', () => {
  renderSidebar({
    sessions: [
      sessionFixture('chat:deploy', {
        title: 'Deploy',
        pendingApprovals: 2,
        lastActivityAtMs: NOW.getTime(),
      }),
    ],
  });

  const row = screen.getByRole('button', { name: 'Deploy, needs approval' });
  expect(within(row).getByText('2')).toBeVisible();
});
```

In `apps/web/src/ViewHarness.test.tsx`:

1. Add `approvalEvent` and `approvalFixture` to the `./test/live` import.
2. In `opens the new session on a page that still shows the conversation`, replace the comment and hash with:

```text
  // Automations arrives in a later release; until then it shows the chat.
  window.history.replaceState(null, '', '/#/automations');
```

3. After `streams a reply into the open session with its tool steps, then shows the committed reply`, add:

```tsx
it('shows a pending approval inline and on the Approvals badge, once, and decides it', async () => {
  const user = userEvent.setup();
  const { stream } = await openLiveSession();
  const run = { ...runningRun(), status: 'awaiting_approval' as const };
  const approval = approvalFixture('apr_7', {
    sessionId: 'room-7',
    runId: run.id,
  });
  const decide = vi
    .spyOn(daemon, 'decideApproval')
    .mockResolvedValue({ ...approval, status: 'allowed', revision: 2 });
  act(() =>
    stream.push(snapshotEvent([snapshotRun(run)], 1, 'agent-main', [approval])),
  );

  const card = await screen.findByRole('region', {
    name: 'Approval needed: bash',
  });
  expect(screen.getByText('Waiting for your approval…')).toBeVisible();
  expect(
    screen.getByRole('button', { name: 'Approvals, 1 waiting' }),
  ).toBeVisible();
  // A reconnect's snapshot shows it again, still once.
  act(() =>
    stream.push(snapshotEvent([snapshotRun(run)], 1, 'agent-main', [approval])),
  );
  expect(
    screen.getAllByRole('region', { name: 'Approval needed: bash' }),
  ).toHaveLength(1);

  await user.click(within(card).getByRole('button', { name: 'Allow once' }));
  expect(decide).toHaveBeenCalledWith('apr_7', {
    decision: 'allow_once',
    revision: 1,
  });
  act(() =>
    stream.push(
      approvalEvent(
        'approval.resolved',
        { ...approval, status: 'allowed', revision: 2 },
        2,
      ),
      runEvent('run.started', { ...run, status: 'running' }, 3),
    ),
  );

  await waitFor(() =>
    expect(
      screen.queryByRole('region', { name: 'Approval needed: bash' }),
    ).not.toBeInTheDocument(),
  );
  expect(screen.getByRole('button', { name: 'Approvals' })).toBeVisible();
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/web && bun x vitest run src/lib/approvals.test.ts src/components/WorkspaceShell.test.tsx src/components/sessions/SessionSidebar.test.tsx src/ViewHarness.test.tsx`
Expected: FAIL — `pendingApprovalCount` is missing, there is no Approvals destination or badge, ⌘K has no approval or session commands, rows have no approval badge, and the harness does not show the badge.

- [ ] **Step 3: Count pending approvals and badge the session rows**

Add to `apps/web/src/lib/approvals.ts` (with `import type { Session } from '@animaOS-SWARM/sdk';` merged into the SDK import and `import type { LiveState } from './session-events';`):

```ts
/** Approvals waiting for the owner: the stream's while it is open (every
 *  pending one of the companion and its helpers), else the listed
 *  sessions' counts. */
export function pendingApprovalCount(
  approvals: LiveState['approvals'],
  streamOpen: boolean,
  sessions: readonly Session[],
): number {
  if (streamOpen) return Object.keys(approvals).length;
  return sessions.reduce(
    (total, session) => total + session.pendingApprovals,
    0,
  );
}
```

In `apps/web/src/components/sessions/SessionSidebar.tsx`:

1. Replace `rowLabel` with:

```ts
function rowLabel(session: Session): string {
  return [
    session.title,
    session.activeRuns > 0 ? 'working' : null,
    session.pendingApprovals > 0 ? 'needs approval' : null,
    session.unread ? 'unread' : null,
  ]
    .filter(Boolean)
    .join(', ');
}
```

2. In `SessionRow`'s row button, after `<span className="session-title">{session.title}</span>`, add:

```text
          {session.pendingApprovals > 0 && (
            <span className="session-approval-badge" aria-hidden>
              {session.pendingApprovals}
            </span>
          )}
```

Append to `apps/web/src/approvals.css`:

```css
.nav-badge,
.session-approval-badge {
  min-width: 18px;
  border-radius: 9px;
  padding: 0 5px;
  background: var(--color-amber);
  color: var(--color-abyss);
  font-size: 10px;
  font-weight: 600;
  line-height: 18px;
  text-align: center;
}
.nav-badge {
  margin-left: auto;
}
```

- [ ] **Step 4: Add the destination, the badge, and the commands**

In `apps/web/src/components/WorkspaceShell.tsx`:

1. Add `import type { Session } from '@animaOS-SWARM/sdk';`, `import { sessionKey } from '../lib/session-groups';`, and `ShieldIcon` to the `./icons` import.
2. Replace `AVAILABLE_PAGES` and `PRIMARY_DESTINATIONS` with:

```tsx
/** Pages this release renders; the other hash pages open the conversation
 *  until their milestones build them. */
export const AVAILABLE_PAGES = [
  'approvals',
  'work',
  'files',
  'connectors',
  'capabilities',
] as const satisfies readonly HashPage[];
export type AvailablePage = (typeof AVAILABLE_PAGES)[number];

/** Session titles the command menu offers, newest activity first. */
export const MAX_SESSION_COMMANDS = 50;
```

```tsx
const PRIMARY_DESTINATIONS: Destination[] = [
  { page: 'approvals', label: 'Approvals', icon: <ShieldIcon size={16} /> },
  { page: 'work', label: 'Work', icon: <SparkIcon size={16} /> },
  { page: 'files', label: 'Files', icon: <PulseIcon size={16} /> },
  { page: 'connectors', label: 'Connectors', icon: <GearIcon size={16} /> },
];
```

(The `interface Destination` and `AvailablePage` stay as they are; move nothing else.)

3. Give `DestinationNavigation` a `pendingApprovals: number` prop (add it to the destructured props and their type), and replace its `destination` function with:

```tsx
const destination = (item: Destination) => {
  const waiting = item.page === 'approvals' ? pendingApprovals : 0;
  return (
    <button
      key={item.page}
      type="button"
      onClick={() => navigate({ kind: 'page', page: item.page })}
      aria-current={page === item.page ? 'page' : undefined}
      aria-label={
        waiting > 0 ? `${item.label}, ${waiting} waiting` : item.label
      }
      className={itemClass}
    >
      {item.icon}
      <span>{item.label}</span>
      {waiting > 0 && (
        <span className="nav-badge" aria-hidden>
          {waiting}
        </span>
      )}
    </button>
  );
};
```

4. Add to `WorkspaceShell`'s destructured props (after `connectors = null,`) `approvals = null, pendingApprovals = 0, sessions = [], onOpenSession,` and to its props type, after `connectors?: ReactNode | null;`:

```text
  /** The Approvals page, shown at `#/approvals`. */
  approvals?: ReactNode | null;
  /** Approvals waiting for the owner, for the destination's badge. */
  pendingApprovals?: number;
  /** The listed sessions the command menu offers by title. */
  sessions?: readonly Session[];
  onOpenSession?: (session: Session) => void;
```

5. Pass `pendingApprovals={pendingApprovals}` to both `<DestinationNavigation … />` elements.
6. In `commands`, after the `new-chat` entry, add:

```text
    {
      id: 'review-approvals',
      title: 'Review approvals',
      description:
        pendingApprovals > 0
          ? `${pendingApprovals} waiting for you`
          : 'Nothing is waiting',
      group: 'Navigate',
      run: () => navigate({ kind: 'page', page: 'approvals' }),
    },
```

and after the `...(desktopNavigation ? [ … focus … ] : [])` spread:

```text
    ...(onOpenSession
      ? sessions
          .filter((session) => !session.archived)
          .slice(0, MAX_SESSION_COMMANDS)
          .map((session) => ({
            id: `session:${sessionKey(session)}`,
            title: session.title,
            description: 'Open this conversation',
            group: 'Sessions',
            run: () => onOpenSession(session),
          }))
      : []),
```

7. In the page switch, replace `{page === 'connectors' ? (` with:

```text
              {page === 'approvals' ? (
                approvals
              ) : page === 'connectors' ? (
```

In `apps/web/src/ViewHarness.tsx`:

1. Add `import { ApprovalsPage } from './pages/ApprovalsPage';` and `import { pendingApprovalCount } from './lib/approvals';`.
2. Add these props to `<WorkspaceShell … />`, after `connectors={…}`:

```text
          approvals={
            <ApprovalsPage
              agentId={agent.id}
              live={live.state}
              streamOpen={live.status === 'open'}
              sessions={sessions.sessions}
              onOpenSession={(approval) =>
                commands.openTarget({
                  agentId: approval.agentId,
                  sessionId: approval.sessionId,
                })
              }
            />
          }
          pendingApprovals={pendingApprovalCount(
            live.state.approvals,
            live.status === 'open',
            sessions.sessions,
          )}
          sessions={sessions.sessions}
          onOpenSession={openSession}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd apps/web && bun x vitest run src/lib/approvals.test.ts src/components/WorkspaceShell.test.tsx src/components/sessions/SessionSidebar.test.tsx src/ViewHarness.test.tsx src/visual-tokens.test.ts`
Expected: PASS, with no `act()` warnings or console noise.

Run: `bun x nx run-many -t test,typecheck -p @animaOS-SWARM/web`
Expected: PASS — the whole web suite (703 tests at M3's end plus M4's).

- [ ] **Step 6: Format and commit**

Run: `bun x nx format:write --files=apps/web/src/components/WorkspaceShell.tsx,apps/web/src/components/WorkspaceShell.test.tsx,apps/web/src/components/sessions/SessionSidebar.tsx,apps/web/src/components/sessions/SessionSidebar.test.tsx,apps/web/src/lib/approvals.ts,apps/web/src/lib/approvals.test.ts,apps/web/src/approvals.css,apps/web/src/ViewHarness.tsx,apps/web/src/ViewHarness.test.tsx && cd apps/web && bun x vitest run src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx`
Expected: PASS.

```bash
git add apps/web/src/components/WorkspaceShell.tsx apps/web/src/components/WorkspaceShell.test.tsx apps/web/src/components/sessions/SessionSidebar.tsx apps/web/src/components/sessions/SessionSidebar.test.tsx apps/web/src/lib/approvals.ts apps/web/src/lib/approvals.test.ts apps/web/src/approvals.css apps/web/src/ViewHarness.tsx apps/web/src/ViewHarness.test.tsx
git commit -m "feat(web): add the Approvals destination, approval badges, and the command menu additions"
```

Recommended implementer tier: most capable (integration across the shell, the harness, and the shared stream, with the harness's large test suite to keep quiet).

---

### Task 14: M4 verification

**Files:**

- Modify: `docs/superpowers/plans/2026-09-23-companion-console.md` (the M4 status row; controller only)

- [ ] **Step 1: Check the contracts**

Run: `grep -n "APPROVAL_TIMEOUT_MS\|TELEGRAM_APPROVAL_TIMEOUT_MS\|MAX_APPROVAL_ARGUMENTS_BYTES\|MAX_APPROVAL_NOTE_CHARS\|DECIDED_APPROVAL_WINDOW_MS\|MAX_APPROVAL_RULES_PER_AGENT\|MAX_SESSION_ALLOWANCES\|MAX_MATCHER_VALUE_CHARS" hosts/rust-daemon/src/approvals/mod.rs`
Expected: each constant defined once, in `approvals/mod.rs`.

Run: `grep -rn '"/api/approvals"\|"/api/approvals/{approval_id}/decision"\|"/api/agents/{agent_id}/approval-policy"\|"/api/agents/{agent_id}/approval-rules"\|"/api/agents/{agent_id}/approval-rules/{rule_id}"' hosts/rust-daemon/src/routes`
Expected: each path in `routes/mod.rs` (the router) and in its handler's `#[utoipa::path]` in `routes/approvals.rs`.

Run: `grep -n "approval" hosts/rust-daemon/README.md | head -20`
Expected: the Approvals section and its seven rows.

Run: `grep -rn "allow(dead_code)\|allow(unused_imports)" hosts/rust-daemon/src/approvals hosts/rust-daemon/src/state/approval_state.rs`
Expected: no output (Task 8 removed the temporary allowances).

Run: `grep -rn "dangerouslySetInnerHTML\|MarkdownMessage" apps/web/src/components/sessions/ApprovalCard.tsx apps/web/src/pages/ApprovalsPage.tsx`
Expected: no output (arguments and notes render as text).

- [ ] **Step 2: Run the milestone gate**

Run: `df -h /System/Volumes/Data`

- With at least 12 GB available: run `bun x nx run rust-daemon:test --skipNxCache` (it also runs `core-rust:test`). Expected: PASS (M3 ended at 1,564 passed / 7 ignored; M4 adds about 75 tests and one more ignored Postgres case inside the existing ignored test).
- Otherwise run the fallback in the shared `target/` (no new `CARGO_TARGET_DIR`): `CARGO_INCREMENTAL=0 cargo test -p anima-core --lib`, `CARGO_INCREMENTAL=0 cargo test -p anima-model-adapters --lib`, `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib`, `CARGO_INCREMENTAL=0 cargo test -p anima-core --tests`, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --tests`. Expected: PASS. The fallback does not satisfy AGENTS.md's completion rule; record that the Nx gate is pending disk space.

Run: `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`
Expected: every target succeeds.

Run: `cargo fmt --all --check && bun x nx format:check --base=origin/main`
Expected: both succeed.

The Postgres conformance test stays `#[ignore]` without a database; hand-check `history/postgres.rs`'s approval SQL against `migrations/20260923000000_history_store.sql` (`history_approvals`).

- [ ] **Step 3: Update the master plan status**

In `docs/superpowers/plans/2026-09-23-companion-console.md`, replace the M4 row (match it by content; the table is padded)

```markdown
| M4 Approvals | (written before M4) | pending |
```

with the following only if every gate command passed (fill in the Nx test count and the head commit):

```markdown
| M4 Approvals | `2026-09-23-companion-console-m4.md` | done (Nx rust-daemon:test <count> passed; sdk + web test, typecheck, build green at <sha>) |
```

If the Rust gate ran only through the fallback, use `implemented — Nx gate pending (disk)` as the status. Then run `bun x nx format:write --files=docs/superpowers/plans/2026-09-23-companion-console.md` (it realigns the table). The controller commits this file:

```bash
git add docs/superpowers/plans/2026-09-23-companion-console.md
git commit -m "docs: mark the M4 approvals milestone complete"
```

Recommended implementer tier: the controller runs this task.

---

## Notes for the controller

**Task shape against the master plan.** Every master task is covered; T4.1 is split so each commit stays reviewable:

- T4.1 (risk table, policy, rules, gate, waiter, timeouts, helper denial, restart expiry) → Tasks 1 (table, matchers, evaluation), 2 (records, registry, snapshot v7, restart expiry), 3 (settling: decisions, rules, allowances, run status), 5 (the gate and the waiting call in `execute_tool`, timeouts, helpers, live re-check), and 6 (Stop, abandoned waits, steers). T4.2 (routes and history) → Tasks 4 (history store and outbox), 7 (reads: snapshot, `pendingApprovals`, `GET /api/approvals`), and 8 (decision, policy, and rule routes). T4.3 (SDK, card, page, badges) → Tasks 9 (SDK), 10 (reducer), 11 (inline card), 12 (page), and 13 (destination, badges, ⌘K, wiring). Task 14 is the gate.
- Master names kept: `hosts/rust-daemon/src/approvals/{mod.rs,policy.rs,gate.rs}`, `tools.rs`, `routes/approvals.rs`, `packages/sdk/src/approvals.ts`, `ApprovalCard.tsx` (in `components/sessions/`, next to the other transcript cards), `pages/ApprovalsPage.tsx` (a new `apps/web/src/pages/` folder later milestones' pages can share).
- Files the master plan did not list: `approvals/registry.rs`, `state/approval_state.rs`, `agent_runs/{approvals.rs,approval_tests.rs,approval_stop_tests.rs}`, `routes/contracts/approvals.rs`, `routes/tests/approvals.rs`; web `lib/approvals.ts`, `hooks/useApprovals.ts` (spec §15.5 names it), `approvals.css`.
- Order: 1 → 2 → 3 → 4 (4 needs 2's registry) → 5 (needs 3) → 6 (needs 5) → 7 (needs 4 and 5) → 8 (needs 7's module) → 9 (needs 7–8's JSON) → 10 → 11 → 12 → 13 → 14. Tasks 4 and 5 are independent of each other once 3 is in.

**Carry-forwards (every item of `.superpowers/sdd/2026-09-23-companion-console-m4/carry-forwards.md`).**

- `RunStatus::AwaitingApproval` (daemon, SDK, web `isActiveRun`) → used as is; Task 3 moves runs in and out of it, Task 5 publishes `run.awaiting_approval` (`LiveEventBody::RunAwaitingApproval`, already defined) and the resume's `run.started`, Task 10 makes the web merge rank it with `running`.
- `stream.snapshot`'s empty `approvals` → Task 7 fills it (`live_snapshot_approvals`), Task 9 types it `Approval[]`, Task 10 reduces it, Task 13's harness test reconnects with it.
- Stop while awaiting approval (§4.6; `agent_runs/stop.rs`, `state/run_stop.rs`, the run's `CancelSignal`) → Task 6: the stop settles the run's approvals in its own save and wakes the waiters before cancelling; the waiter also races the same `CancelSignal` for cancels without a saved stop.
- Steers wait (§4.7) → Task 6's `a_steer_sent_while_awaiting_approval_waits_for_the_next_model_call`: the waiter never touches the steering inbox, and `joinable_run`'s `is_in_flight` already accepts steers into an awaiting run.
- Restart expiry (§4.8) → Task 2: `ApprovalRegistry::restored` next to `RunLedger::restored` in `restore_control_plane_snapshot`.
- `sessionAllowances` and a snapshot bump → Task 2 adds the field and bumps to version 7 with `.pre-approvals.bak` / `control_plane.backup.6`, following M2 and M3.
- History store `approvals` table, conformance, outbox, 30-day reads → Task 4 (the tables already exist in schema v1 and the Postgres migration, so no migration is added), Task 7 (`decided`).
- Helpers never wait (`is_helper_config`, `helper_config`) → Task 5 (`helpers_are_denied_instead_of_waiting`, built with `helper_config`).
- The gate after the live checks, before dispatch; `search_conversations` read class; unknown tools `exec` → Tasks 1 and 5.
- Telegram 15 minutes (`RunSource::Telegram`) → Task 5 (`ApprovalTimeouts::for_source`).
- ⌘K additions (M3 audit M27) → Task 13.
- Inline card, `#/approvals`, badges, reducer events, SDK `approvals.ts` → Tasks 9–13.
- Snapshot approvals after a reconnect → Tasks 10 and 13.
- Process lessons → Global Constraints: `text` fences for fragments, the SDK build at the end of Task 9, the waiter's own 30/15-minute bound (M4 adds no model call), named string constants tested once, new code in new modules (the three large files only gain wiring), pristine web tests.
- Repo facts → Global Constraints and Task 14 (gate counts, CI, no Postgres, disk, lock order).
- Out of scope: none of the carry-forward items. (The master plan's "Carried from M3" items for M8 and M10 stay there.)

**Spec vs. code decisions.**

- Statuses and who resolved: spec §7.3 names `pending` and describes the rest; the plan uses `allowed`, `denied` (an owner deny or a timeout, `resolvedBy: timeout` with the note `Approval timed out`), `stopped` (the run's Stop, a bare cancel, or an abandoned wait), and `expired` (a restart), with `resolvedBy` `owner | timeout | stop | restart`.
- Helpers answer to their companion's policy and rules and have no session allowances (spec says only that each agent has a policy; a helper inherits the companion's tools, so its own default policy would let it bypass the companion's `ask`). They never wait. `PUT` policy and `POST` rules for a helper are 409; `GET` answers the companion's.
- Matchers, where the spec is silent: a command-prefix rule compares whole words and never matches a command holding `; & | $ > < ( )`, a backtick, or a newline; a path glob is workspace-relative (`*`, `?`, `**`) and never matches an absolute path or one with `..`; a domain covers its host and subdomains over http/https. Suggestions: `git status`-style first two words, the file's folder `/**`, the URL's host, else `any`.
- Plan bounds for spec §1's bounded growth: 100 rules per agent, 50 allowances per session, 512-character matcher values, decided pages of 50 (at most 100).
- An unregistered tool is refused before the gate (no approval is asked for a tool that cannot run); "unknown tools are exec" applies to registered tools missing from the table, and a test fails when a registered tool has no class.
- Snapshot version 7 with a backup (M3 precedent), rather than additive fields an M3 daemon would silently drop.
- Decided approvals leave the control plane once the history store holds them (spec §7.3 "moves the record"), so there is no `mirrored` flag; a decision on a mirrored approval is answered from the store.
- Session deletion removes the session's approvals from the history store, and agent deletion the agent's (approvals carry the tool arguments, which are transcript data); usage rows still stay. Decided approvals of a deleted session are dropped from the control plane at the next flush instead of being written.
- `agentId` in `GET /api/approvals` matches the approval's own agent exactly; a delegated specialist's approvals list under the specialist, while the companion's stream still carries them live (spec §6).
- A resume after approval is announced as `run.started` with the run at `running` (no new event type); `startedAtMs` is unchanged.
- The stop path settles a run's pending approvals as `stopped` inside the stop's own save (so a later Allow is 409 rather than a misleading "allowed" for a tool that never ran). An approval allowed just before a Stop still runs if the call is already past the gate; if the stop signal is set when the waiter wakes, the call is not dispatched.
- Saves: an owner decision is saved before the waiter wakes and is reverted on failure (503); a request is saved before it is announced and refused on failure (the tool does not run); a timeout or stop settlement that cannot be saved is kept in memory (it only keeps a call from running, and a restart expires it anyway).
- Announce before waking: `approval.resolved` (and the resume) are published before the waiting call is woken, so streams see the resolution before the tool's own events.
- Swarm runs (`/api/swarms/...`) are not gated: they have no session, ledger run, or owner UI; their tool context gets no gate.
- `GET /api/agents/{id}/approval-rules` also returns the tool catalog (every registered non-read tool with its class and matcher kinds), and `ApprovalResponse` carries `matcherKinds`, so the web never duplicates the daemon's table.
- Hot-tail pruning needs no change: a pending approval's tool call lives in its run's isolated runtime until the run commits, so it never references a committed message (spec §13.2's "not referenced by a pending approval" holds by construction).
- A stream that opens while a request's save is in flight can list that request in its snapshot; if the save then fails, the request is withdrawn without an event and a decision on it is 404 (the next snapshot drops it). M3 announces a run's failed start for the same window; the plan accepts this narrower one.

**Deferred to later milestones.** Showing and revoking session allowances in the UI (they end with the session; M10 or later if wanted); the Health page's pending-approvals card (M8); the Playwright approval-card flow of spec §17 (M10, T10.2); approving from Telegram (a spec §1 non-goal); gating swarm runs (outside the companion console); `load_skill`, `propose_skill`, `list_automations`, `create_automation`, `pause_automation` exist only as table entries until M5 and M6.

**Risks for the pre-flight audit.**

- Concurrency (Tasks 5 and 6): the waiter's `select!`, the transaction ordering between a decision, the timeout, and a Stop, the oneshot fallback (`try_recv` after a settle that found the record already mirrored), and the stop's settle-then-save-then-wake-then-cancel order. The race tests hold the control-plane transaction to force each order; they rely on tokio's fair mutex and on 250 ms / 500 ms timings.
- Self-approval through a broad rule: an owner who "always allows" `curl` (or any HTTP client) lets the model POST to the daemon's decision route from the same machine with a forged `Origin` header, since loopback owner authorization is origin-based. Approvals are the control for shell access (spec §1), so the plan does not add a decision nonce; the audit may want one (for example a per-approval token shown only in the UI) or a note in the README.
- Behavior change for existing clients: `exec: ask` by default means a CLI, API, Telegram, schedule, or job run that calls `bash` now waits up to 30 (or 15) minutes for a web approval; a legacy `POST /run` caller with its own HTTP timeout may give up first (the run still waits). The one M3 test that runs `bash` is updated; others use read, write, or delegate tools. Telegram's connector handles one message at a time, so a pending approval also holds that chat for up to 15 minutes.
- Rollback: an M3 binary refuses the version-7 snapshot; downgrading needs `.pre-approvals.bak` (or the `control_plane.backup.6` row), and M10's upgrade notes should say so.
- Size: Task 5 is the largest (about 1,800 plan lines, 16 tests); Task 13 touches `ViewHarness.tsx` with four props and adds one harness test.
- The Postgres approval SQL (`$1::text IS NULL`, `$3::bigint IS NULL`) is only hand-checked.
