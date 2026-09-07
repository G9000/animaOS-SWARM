# Agency onboarding implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development for bounded implementation and review. Preserve unrelated changes. User approved implementation on 2026-09-07.

**Goal:** Make agency setup goal-first, connect intelligence before folder setup, and create useful editable teams with explicit access.

**Architecture:** Keep generation and atomic workspace bootstrap in the Rust daemon. Add typed SDK agency/workspace contracts and consume them in the web adapter. The web owns draft editing; generated capabilities are suggestions, never automatic grants. Existing resume and manager-recreation paths remain supported.

**Tech Stack:** React/TypeScript, SDK HTTP client, Rust daemon, Nx/Vitest/Cargo.

## Approved design

Flow: Goal (name, brief, template) -> Model -> Team (agency only) -> Workspace (folder, manager, access) -> Launch. Existing workspace resume stays available at entry. Seven templates include concrete roles, deliverables, workflow, suggested capabilities, and a prepared first assignment. First assignment is saved in manager instructions, not run automatically. Service connections and recurring routines remain post-setup. No new runtime budget or approval UI until host integration is verified.

## Tasks

- [x] Shared contracts: add agencies/workspace SDK clients, exact-or-automatic size input, exports, and HTTP contract tests. Preserve nullable daemon types. Delegate this isolated SDK task while web work proceeds.
- [x] Rust generation: reject invalid exact counts and enforce returned exact count; preserve automatic ceiling validation. Regression tests first, then full rust-daemon:test with isolated validation target on Windows.
- [x] Templates and team draft: expand to software, research, operations, support; retain per-agent model/provider and suggested tool names. Add role add/edit/remove, per-role model and explicit access selection, editable first assignment. Test payload preservation and permission behavior.
- [x] Flow: split goal from folder setup, retain provider readiness guards and resume invalidation. Add automatic/exact sizing controls, draft validation, launch summary, instructions with first assignment and team workflow.
- [x] Integration: use SDK contracts in web wrapper; regression tests for generation mismatch, initial goal without folder, late folder validation, role settings, prepared assignment, resume, and duplicate submit.
- [x] Verify: web and SDK Nx tests/typecheck/build; rust-daemon:test --skipNxCache and formatting. Review combined diff and visually inspect desktop/mobile onboarding without modifying the user's workspace.

## Verification commands

- `bun x nx run @animaOS-SWARM/web:test --run --skipNxCache`
- `bun x nx run @animaOS-SWARM/sdk:test --run --skipNxCache`
- `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache`
- `bun x nx run @animaOS-SWARM/web:build --skipNxCache`
- `$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; bun x nx run rust-daemon:test --skipNxCache`
- `bun x nx run rust-daemon:lint --skipNxCache`

## Acceptance

Generation never creates agents. Failed generation preserves edits. Exact mode means exact total including manager; manual edits must satisfy chosen bounds. Unknown/generated tools are displayed for review and never granted merely because the model suggested them. Model overrides must reference configured providers. Bootstrap remains atomic. The first assignment is visibly prepared, editable, and not executed on creation.


## Result (2026-09-07)

- [x] Short-name follow-up: full personal-name renames also replace unambiguous first-name references, preserving shared first names, role-title wording, unchanged full names, and prose already using the new name. Specialist Edit now includes "Fix an old name in the text" with an explicit previous-name input and "Update name references" for stale drafts without rename history. Repair guards blank/duplicate roster names, another agent's name, and absent matches. Verified 324 web tests, build/typecheck, diff checks, and localhost serving the control. Review findings about repair matching and duplicate names resolved. No LLM calls or user workspace writes.

- [x] Team rename follow-up: completing a specialist or manager name edit synchronizes whole-name references in roles, instructions, teammate handoffs, workspace brief, priorities, and prepared assignment; Next also synchronizes pending edits. Blank/duplicate intermediate names keep the original reference for later resolution. One-pass Unicode-aware replacement preserves partial words and avoids cascading name swaps. Generated/template lead references follow the chosen manager name. No LLM request or automatic role rewrite. Verified 321 web tests, web build/typecheck, bootstrap payload regression, and diff checks; review finding about regenerated template lead names resolved.

- [x] Settings persistence follow-up: retain the original setup provider/model across catalog refreshes and disconnects, including a custom model when reselecting the same provider. Settings model/provider/name updates synchronize the matching anima.yaml entry and daemon state; preserve global defaults, other agents, and custom fields. Serialize concurrent writes and restore the previous configuration on save failure. Validation: 319 web tests, web build/typecheck, full Rust Nx test target including concurrent saves, legacy fallback, YAML-only resume, restart, and failure rollback; scoped formatting and diff checks passed. Independent reviews found no blockers. This supersedes the earlier limitation about Settings edits not synchronizing YAML. Existing user YAML remains untouched; restart the running daemon before saving Settings to load this backend change.

- [x] Generation timeout follow-up: agency generation was under the 30-second general API timeout. It now uses the bounded model-run timeout (600 seconds by default), preserving authentication/body limits and ordinary API timeouts. Starting regeneration clears stale success; 408 gets readable copy while the prior team remains usable. Validation: 318 web tests, web build/typecheck, full Rust Nx tests with slow-generation success/timeout HTTP regressions. Restart the running daemon to load the route change.

- [x] Model persistence follow-up: all template/generated agents inherit the setup model/provider unless the user explicitly overrides a specialist. Generated model suggestions no longer apply automatically (supersedes earlier generation-provider pinning). Bootstrap writes each agent's effective model/provider to anima.yaml; Rust inspect/resume and CLI launch honor per-agent overrides with legacy global fallback. Workspace readiness checks every effective provider. Existing root YAML inspected without modification: all four agents already use anthropic / claude-haiku-4-5. Validation: web 317 tests, CLI 113 tests, web/CLI builds/typechecks, full Rust Nx tests including 41 workspace integration tests and YAML -> fresh resume -> restart round trip. Runtime settings updates remain persisted in daemon state; this change does not add ongoing synchronization of later settings edits back into YAML.

- [x] Template feedback: keep the full picker visible; replace ambiguous custom cards with Create a custom agency and a secondary Manager only option. All seven templates populate name, structured goal/brief (deliverables and workflow), and values. Re-selecting the active template preserves edits. First assignment remains separately editable at Review. Validation: 316 web tests, web build/typecheck, desktop/mobile browser checks and diff whitespace checks passed.

- [x] Follow-up: native browser dictation for Workspace brief. Explicit mic start/cancel/stop, interim preview, final-only append preserving typed edits, standard/prefixed API detection, errors and unsupported fallback, abort on unmount. No backend changes. Validated 309 web tests, build/typecheck and desktop/mobile simulated speech browser checks; actual microphone recognition was not exercised.

Implemented all six tasks in the current checkout. Seven templates now carry workflow, deliverables, suggested connections and prepared first assignments. Generated model overrides remain paired with their generation provider. Per-role grants are explicitly chosen and restricted to the selected access profile. Agency drafts enforce 2-10 total members and exact counts where selected.

Validation: web 300 tests; SDK 34 tests including daemon integration; web/SDK build and typecheck; full Rust daemon/core Nx test target passed. Final focused agency tests passed 5/5 after the last summary-display adjustment. Headless Edge at 1440x1000 and 390x844 exercised Goal, Model, Team, Workspace, Review and role editing with mocked API responses; no overflow or page errors. Existing user workspace was not changed.

Code review issues resolved: preserve provider/model pairing, reject agencies without specialists. No remaining blocking review findings. `git diff --check` passed. Scoped Rust formatting passed; repository-wide rust-daemon:lint still reports existing formatting in routes/mod.rs and tests/agent_api.rs, which were not edited. The running daemon was not restarted; restart it to load server changes. No commits or deployment performed.
