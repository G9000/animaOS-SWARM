# Handover: companion console build (M0–M3 done, M4 planned)

This document is for the next agent picking up the companion console upgrade. Read it first; it tells you where everything is and what to do next.

## Update 2026-10-03: M5 done, PR open

- **M5 (Skills) is complete** on `feat/companion-console`, and a PR to `main` is open.
- **Work done:** all 13 tasks, then the final review (daemon and web+SDK reviewers) and one fix wave (`2060912` web, `0b08b2b` daemon).
- **Gates at `e9d57f9`, all green:**
  - `rust-daemon:test`: 1,775 passed, 7 ignored.
  - SDK and web test, typecheck and build (web 807 tests).
  - `cargo fmt --check` and `nx format:check`.
- **Merging:** only when the owner says "merge".
- **Next:** M6 (automations), following the master plan. Write the plan, run a pre-flight audit, write the rulings, then execute.
- **Ledger:** every M5 ruling is in `.superpowers/sdd/2026-09-23-companion-console-m5/progress.md`.
- **Post-M5 follow-up:** skip a scan while the previous scan's blocking work is still running (hung-drive case).

## Update 2026-10-02 (latest): M5 in progress (Tasks 1–7 committed)

- **Done:** the M5 audit rulings are committed in the plan (`be0029f`). Tasks 1–5 are implemented, reviewed and fixed. Tasks 6 (`5729eab`, skill routes) and 7 (`61c6123`, draft/import routes and the multipart reader) are committed but **not yet reviewed**.
- **Resume:** run one combined review of Tasks 6 and 7, a fix round, then Tasks 8–14, the final review, the fix wave and the gates, then a PR.
- **Working files:** the ledger, briefs and rules are in `.superpowers/sdd/2026-09-23-companion-console-m5/`. These are local and gitignored; regenerate the briefs from the plan if they're missing.
- Task 7 hardened the multipart reader now (per-part caps, a linear boundary search) instead of deferring that to M9 (audit m19). Drop that deferral note.

## Update 2026-10-02 (later): M5 planned and audited, not started

- M4 is merged to `main` (PR #9, merge commit `f34313b`).
- **M5 (Skills) plan:** `docs/superpowers/plans/2026-09-23-companion-console-m5.md`, 14 tasks. Snapshot moves to v8, with `.pre-skills.bak` / `control_plane.backup.7` written first.
- **Pre-flight audit:** `docs/superpowers/plans/2026-09-23-companion-console-m5-preflight-audit.md`, with 0 Blocker, 4 Important and 19 Minor findings.
- **Resume here:**
  1. Rule on the audit findings: insert a `#### Controller rulings from the pre-flight audit (binding)` block per affected task and commit the plan. The auditor's recommended rulings are in the audit, and none needs a question for the owner.
  2. Extract briefs to `.superpowers/sdd/2026-09-23-companion-console-m5/` and copy `rules.md` from the M4 folder.
  3. Run Tasks 1→14 as in M4.
- **The four Important findings:**
  - **I1:** hidden Unicode (tag characters, bidi overrides, zero-width characters, U+2028/2029) in drafts.
  - **I2:** the page's Edit button can approve text nobody reviewed.
  - **I3:** Windows device names (`con`, `nul`, `com1`…) pass as slugs.
  - **I4:** `act()` warnings in the Skills page tests and `useSkillCommands`.
- **Tiers:** the auditor recommends the most capable model for Tasks 4, 5, 9 and 13.

## Update 2026-10-02: M4 done, PR open

- **M4 (Approvals) is complete** on `feat/companion-console`, and [PR #9](https://github.com/G9000/animaOS-SWARM/pull/9) to `main` is ready for review.
  - All 13 tasks are done, then a whole-milestone final review (split into daemon and web+SDK reviewers) and one fix wave (`9de5ea3` web, `20c6221` daemon).
  - Gates at `20c6221`: `bun x nx run rust-daemon:test --skipNxCache` passed 1,654 with 7 ignored; SDK and web test, typecheck and build are green (web 751 tests, SDK 63); `cargo fmt --check` and `nx format:check` are clean.
  - Merge only when the owner says "merge".
- **Next:** M5 (skills) per the master plan. Write its plan with an opus subagent from the spec, the master plan and the carry-forwards, run a pre-flight audit, then execute it the same way.
- **Post-M4 follow-ups:**
  - Tell Telegram and CLI users that an approval is waiting in the web console.
  - When a request's save fails, its card can linger until the tab reconnects; this is accepted, as in M3.
- **Ledger:** `.superpowers/sdd/2026-09-23-companion-console-m4/progress.md` holds every ruling.
- **Commits are GPG-signed.** When gpg-agent's cache expires, a commit blocks on a pinentry dialog until the owner answers it.

## Where things stood (2026-09-28)

- **Done and merged to `main`:**
  - M0 (security groundwork), M1 (run coordinator), M2 (sessions), and M3 (live runs).
  - PR [#6](https://github.com/G9000/animaOS-SWARM/pull/6) merged M0–M2; PR [#7](https://github.com/G9000/animaOS-SWARM/pull/7) merged M3 (merge commit `51f1f5b`).
- **In progress: M4 (Approvals).** The plan is written and committed, and its pre-flight audit is done. **No M4 code exists yet.**
- **Branch:** keep working on `feat/companion-console`. Don't switch branches in the shared checkout. Open the next PR from this branch when M4 is done.
- **M3 gates at its end:**
  - `bun x nx run rust-daemon:test --skipNxCache`: 1,564 passed / 7 ignored.
  - Web: 59 files / 703 tests. SDK: 16 files / 56 tests.
  - Typecheck, build, `cargo fmt --all --check`, and `bun x nx format:check --base=origin/main` all green.

## Key documents

- Spec (the binding authority): `docs/superpowers/specs/2026-09-23-companion-console-design.md`
- Master plan (milestone table and the "Carried from M3" list): `docs/superpowers/plans/2026-09-23-companion-console.md`
- Milestone plans:
  - `docs/superpowers/plans/2026-09-23-companion-console-m0.md` through `-m3.md` (done)
  - `-m4.md` (next, 14 tasks)
- M4 pre-flight audit: `docs/superpowers/plans/2026-09-23-companion-console-m4-preflight-audit.md`. Its rulings are not yet written into the plan (see "Next steps").
- Local-only working files: `.superpowers/sdd/2026-09-23-companion-console-m*/`. They hold the ledgers (`progress.md`), task briefs, reports, and review diffs, and are gitignored. On the same machine, the M4 ledger is `.superpowers/sdd/2026-09-23-companion-console-m4/progress.md`. Another machine won't have these files; everything essential is in this document and the committed plans.

## How the work is run

The workflow is the `superpowers:subagent-driven-development` skill:

1. One implementer subagent per task. It reads a brief extracted from the plan and commits its work.
2. A task review covers spec compliance and quality.
3. Up to 5 fix rounds follow, then a scoped re-review.
4. After the last task comes a whole-milestone final review. M3 split it into three parallel area reviewers: core+live, daemon, and SDK+web.
5. Then one fix wave, done in sequential slices, followed by the gates.

Decisions are recorded in the ledger as `Ruling: <what> — <why> — <cost if wrong>`. Opus handles concurrency-heavy tasks and reviews; sonnet handles mechanical ones.

## Repo rules (every agent, every task)

- Prefix every cargo command with `CARGO_INCREMENTAL=0`; test filters go after `--`.
- Stage files by explicit path only. Never use `git add -A`, `git add .`, or `git commit -a`.
- Implementers never stage `docs/` or `.superpowers/`; the controller commits docs.
- Never use `git stash`, `git reset`, `git checkout -- <path>`, `git restore`, or `git worktree`, and never switch branches.
- Don't start a real daemon, dev server, database, or container. No Postgres is available here, so Postgres tests stay `#[ignore]`.
- Don't touch the Docker Postgres on port 55432; it belongs to another project.
- No new dependencies.
- Keep touched files `cargo fmt` and Prettier clean. Format only touched files, never the whole repo.
- Lock order in the daemon: control-plane transaction mutex → state lock → live registry or fanout mutex. Room leases come before the transaction. No std lock is held across `.await`.
- Put new code in new modules or hooks. `agent_runs.rs` (about 5,600 lines), `connectors/runtime.rs` (about 7,500), and `apps/web/src/ViewHarness.tsx` (about 1,200) are already large.

## Gotchas learned the hard way

- **Disk is tight** (about 13 GB free); the Nx Rust gate needs about 12 GB. Leftover rustc codegen files pile up in `target/debug/deps/*.rcgu.o`. When no cargo or rustc is running, deleting ones older than 30 minutes freed about 13 GB once. Don't add extra `CARGO_TARGET_DIR`s, and never delete anything outside the repo; the user frees space there.
- **CI always fails** at `nx start-ci-run` (an Nx Cloud setup issue), including on `main`. The local gates are the verification.
- **Merging PRs:** auto mode denies Claude's self-merge unless the user explicitly says "merge". `gh pr edit` fails (GraphQL Projects deprecation); use `gh api -X PATCH repos/G9000/animaOS-SWARM/pulls/N`, and `gh api -X PUT …/pulls/N/merge -f merge_method=merge` once the user says to merge.
- **Subagent stalls** (the 600 s watchdog) happened on long single responses. Tell implementers to work one item at a time, commit after each, and pipe test output through `tail -30`. If an agent stalls twice, start a fresh one on the remaining items.
- **Plans:** fence partial code fragments as `text`, because Prettier mangles partial JSX fenced as `tsx`. End every SDK-changing task with `bun x nx run @animaOS-SWARM/sdk:build`, so later web vitest runs resolve the new exports.
- The model adapters' HTTP client has no request timeout, so every new long wait or model call needs its own bound.

## Next steps (M4)

1. Read the M4 plan's header and Global Constraints, and the pre-flight audit.
2. Rule on the audit's findings: 0 Blocker, 6 Important, 14 Minor. Then insert a `#### Controller rulings from the pre-flight audit (binding)` block at the end of each affected task in the plan, as M3's plan does, and commit the plan. The six Important findings, with the auditor's recommended rulings:
   - **I1:** Task 5's two race tests depend on 250/500 ms real-time timing and can flake under the parallel Nx gate. Rewrite them with patient timeouts, an explicit `settle_approval(TimedOut)` task, and `yield_now()` ordering, without paused time.
   - **I2:** A broad exec rule (curl, an interpreter, git, npm) lets the companion rewrite its own approval policy with a forged-Origin loopback request, or run code through default-allowed writes to `.git/config` or `package.json`. The auditor recommends documenting the limit in the README and warning in the UI, with no per-approval token. **Ask the user** whether they want stronger protection.
   - **I3:** With `exec: ask` as the default, check-ins, jobs, Telegram and CLI runs all wait for a web decision. The spec intends this. The legacy `/run` answers 408 after 10 minutes while the approval keeps waiting. Add README and UX copy.
   - **I4:** Document the snapshot v7 rollback (`.pre-approvals.bak` / `control_plane.backup.6`) in the README, as M2 and M3 did, and keep the v7 bump.
   - **I5:** Task 8's `cargo check` runs build a second set of artifacts on a disk-tight machine. Use `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --no-run` plus a grep instead.
   - **I6:** "Always allow" on a delegated specialist's approval creates a rule the Approvals page can't show or remove. Hide "Always allow" when `approval.agentId !== companion.id`, or list those agents' rules.
3. Run Tasks 1→14 strictly in order. Task 14 is the gate plus the master-plan status row.
4. Final review, fix wave, gates, then a PR from `feat/companion-console` to `main`.
5. After M4, M5 through M10 follow the master plan: skills, automations, memory, usage/logs/health, attachments/voice, and deployment/docs. Each gets a plan written by an opus subagent from the spec, the master plan, and a carry-forwards file, then a pre-flight audit, then execution.

## Open items carried forward

These are listed in the master plan under "Carried from M3":

- **M8:** usage for stopped calls, compaction, and titles; the steer queue cap.
- **M10:** rewrite the Playwright specs that mock `/run`, split the large files, and add provider HTTP timeouts, SIGTERM handling, and shutdown producer ordering.
- **Manual acceptance:**
  - One real xAI call, which sends running usage totals.
  - One gpt-6 call, to check whether it accepts `temperature: 0.2` for compaction and titles.
  - Anthropic and OpenAI streaming end to end.

## Using what's built today

Run `bun dev --host rust`. It starts the Rust daemon on `127.0.0.1:8080` plus the web console and playground. Everything from M0–M3 works:

- Parallel sessions with a sidebar.
- Live streamed replies with tool cards.
- Stop, steering (Ctrl/⌘+Enter), and queued sends with safe retries.
- Slash commands, compaction, AI titles, and check-in replies.
- Helper sessions, and the `search_conversations` tool.

The first start after upgrading migrates the control-plane snapshot to v6, and the JSON store keeps a `.pre-live-runs.bak` backup. Approvals (M4) are not there yet, so tools run under the old rules with no approval prompt.
