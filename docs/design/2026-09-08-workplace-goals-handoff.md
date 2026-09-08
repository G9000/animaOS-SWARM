# Workplace goals — continuation handoff

**Work → Goals** now groups assignments across agents under an owner-managed objective and shared execution-attempt budget. Runs can optionally link a goal when created. Prior uncommitted run-supervision work was preserved.

## Delivered

- Durable goals with explicit create, pause, resume, and completion. Creation never starts work. Completion requires at least one accepted latest output and no unfinished or unaccepted linked jobs (cancelled jobs are allowed).
- Shared budget of 1–100 attempts: each started attempt is consumed, including failures and uncertain outcomes. Each queued job reserves one slot. Pending approvals do not reserve slots; approving them rechecks the goal budget atomically. Cancelling queued work releases its reservation.
- Goal linkage is immutable and participates in job idempotency. Existing jobs and browser drafts default to no goal. Old control-plane snapshots default to no goals.
- Paused goals hold queued work across restart; admitted running attempts may finish. A read-only dispatcher prefilter avoids repeatedly cloning state for paused jobs, while the claim transaction still checks goal state.
- Goal cards show consumed, reserved, remaining, run count, and accepted-output count. Selected goals group saved text outputs and agent ownership. Runs offers an optional goal selector with status and remaining budget. Failed or stale reads are surfaced rather than shown as empty histories.
- Owner-authorized, no-store GET/POST `/api/goals`, POST `/api/goals/{goal_id}/status`, and GET `/api/goals/{goal_id}/jobs`; typed `client.goals` SDK methods. Existing job POST adds optional `goalId`.
- Goal/job changes share one persistence transaction and rollback boundary. Stored state validates goal identities, request keys, linkage, budgets, and completion invariants.

## Verification

- Final `bun x nx run rust-daemon:test --skipNxCache`: **1,023 passed, five ignored**, including the core dependency. Tests cover concurrent final-slot contention, cancellation refunds, uncertain-attempt accounting, failed-save rollback, paused restore, accepted-output completion, old records, and HTTP authorization/contracts.
- Full web suite: **354 passed**. SDK: **39 passed**. Builds and typechecks passed. The web typecheck caught unsupported `String.replaceAll` under the repository's JS library target; display normalization now uses regex replacement. Final focused Goals/Runs suite: **16 passed**.
- Live disposable browser with deterministic mock agent: create a one-attempt goal, pause/resume, link a run, consume the slot, accept its saved output, inspect grouped counts/output, then complete the goal. Final snapshot confirmed `completed` with budget one. Desktop layout inspected. No paid model calls or external effects.
- Preview tab, daemon, and Vite server stopped. Logs are local under `target/goals-*.log`; disposable stores are under `target/goals-preview`. Existing ignored Rust checks and build/runtime warnings remain.

## Boundaries and next work

This budget limits execution attempts, not tokens, elapsed time, tool calls, or spending. An individual run still uses the existing runner and permissions. Goals do not automatically decompose objectives or dispatch new assignments. Goal objective text organizes work; each assignment supplies its execution instructions.

Grouped deliverables are bounded saved text outputs, not file artifacts with hashes/provenance. Next implement explicit workspace file artifact records using the existing confined file APIs, then resource metering and portable capability execution. Broader UI redesign, rich document processing, third-party modules, voice/camera/home control remain subsequent work.

Caps remain 50 goals and 200 retained jobs; there is no archive/delete or budget increase flow yet. Completed goals cannot reopen. Missing-agent unfinished jobs retain conservative history and can block completion; full archive/reassignment/abandonment lifecycle remains future work.

Restart the daemon using `bun dev --host rust` to load these routes. Changes remain uncommitted for review.
