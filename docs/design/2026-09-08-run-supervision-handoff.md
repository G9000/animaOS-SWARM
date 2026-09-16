# Run supervision — continuation handoff

This increment adds supervision to **Work → Runs**. It builds on the durable queue and its existing runner, rather than introducing a separate goal execution engine.

## Delivered

- Save a proposal that waits for owner approval before dispatch. Approval is revision-checked and saved before the job is eligible to run; retries can require a fresh approval each time. A pending proposal may also be cancelled.
- Choose a maximum of one, two, or three attempts. The daemon enforces the budget across retries and restarts. This is an attempt budget, not a token, elapsed-time, or financial spend cap.
- Retain each settled attempt's bounded text output, error, and completion timestamps. Retry no longer discards prior available outputs. Truncated outputs are labelled; missing earlier legacy attempts are not invented.
- Accept a completed result or request changes with feedback. Decisions bind to the latest completed output with a revision check, persist once, and never start another run automatically. Explicit retry includes the previous review feedback and preserves its output snapshot.
- Extend SDK and owner-authorized, no-store HTTP contracts with `maxAttempts`, `requiresApproval`, `approvedAtMs`, saved `attempts`, and POST `/api/agents/{agent_id}/jobs/{job_id}/approve` and `/review`.
- Preserve old saved jobs and request defaults: three attempts, no approval requirement. Migrate their latest available settled output without changing tool permissions or enabling schedules.

## Validation

- Full Rust daemon Nx target: 1,016 passed, five ignored. Includes core dependency tests. Added tests cover proposal restart without dispatch, approval execution, bounded retries, request equivalence, history/feedback retention, one-time reviews, rollback on failed saves, invalid snapshots, and HTTP owner/revision/decision contracts.
- Full SDK suite: 38 passed. Full web suite: 347 passed; two additional regressions were added afterward, bringing the source suite to 349. Final focused Runs suite: 11 passed, including the feedback-draft fix. Web/SDK typechecks and builds passed; web build/typecheck passed again after the final UI fix.
- Disposable live browser with a deterministic mock agent: saved a two-attempt proposal, restarted the daemon, confirmed attempt zero still awaiting approval, approved and completed attempt one, requested changes, retried into a new pending approval, approved attempt two, observed the feedback in its output, and accepted the result. No paid model calls or external actions occurred.
- The browser pass found that feedback for attempt one appeared in attempt two's review form. Feedback drafts are now scoped to the output attempt; a regression verifies they do not carry into another attempt.
- Desktop and 390×844 browser checks confirmed usable controls and saved result layout. At phone width, document width equalled viewport width (390 pixels). The final stored record had two outputs with `changes_requested` then `accepted` decisions. Temporary viewport override was reset and the preview tab, daemon, and Vite server were stopped.

## Boundaries and next work

Approval authorizes starting an assignment; it does not grant tools or authorize individual external effects. Result acceptance reviews a saved text snapshot, not a workspace file with provenance. Existing permission checks remain authoritative. The history cap remains 200 jobs, with eight queued/running jobs combined. No automatic retries or mid-tool resumption were added.

The subsequent [workplace goals increment](2026-09-08-workplace-goals-handoff.md) adds linked assignments, aggregate attempt budgets, and grouped saved outputs. File artifact records, real resource metering, and portable capability execution remain next work. The broader visual overhaul, third-party modules, rich document engines, camera, voice, and home integrations remain outside these increments.

Restart the daemon using the normal `bun dev --host rust` workflow to load the new contracts. Changes are left uncommitted for review. Validation logs are under `target/supervision-*.log`; disposable preview files are under `target/supervision-preview`.
