# Durable jobs — continuation handoff

The next workplace increment adds a daemon-owned job queue and an agent-scoped **Work → Runs** view. The existing runner still owns model execution, tool permissions, admission, and per-agent serialization. No model-provider or paid-service integration was added.

## Delivered behavior

- Explicitly queue an assignment, inspect its status/result, cancel queued work, and retry failed or uncertain work with a revision check. Saving a task or configuring an agent does not implicitly start a job.
- Job creation and claims are saved before execution. Browser disconnection does not cancel admitted work. Queued jobs are restored and dispatched after daemon startup.
- A restored running job becomes `needs_review`; it is never automatically replayed. Retrying uncertain work requires acknowledgement because prior external effects may have happened.
- Completion is saved with the existing runner's final snapshot. A rejected completion save retains uncertainty rather than announcing a durable success. Reads use the same transaction boundary so provisional statuses are not exposed.
- Request keys deduplicate exact submissions per agent. The web retains the key and draft across uncertain responses and tab remounts. Agent switching aborts stale list reads; errors do not appear as empty histories.
- Owner-authorized, no-store GET/POST `/api/agents/{agent_id}/jobs`, plus revision-checked POST `/{job_id}/cancel` and `/retry`. SDK methods are `agents.jobs`, `createJob`, `cancelJob`, and `retryJob`.
- Limits: 200 retained records, eight queued/running jobs combined, three attempts, 32 KiB prompts, and 64 KiB result/error previews. History is not silently evicted. Creation requires configured control-plane persistence.

## Verification

- Final `bun x nx run rust-daemon:test --skipNxCache`: 1,010 passed, five ignored, across the core dependency and daemon suites. Deterministic tests cover fresh-state disk restore, queue execution without a browser, claim-before-model, failed completion persistence, idempotency, revision checks, capacity, snapshot validation, and the HTTP contract.
- Full web suite: 344 tests passed. Full SDK suite: 38 passed, including daemon integration. Web/SDK builds and typechecks passed.
- The web contrast contract caught the new primary button foreground; it now uses the existing `text-accent-fg` token. An existing swarm timeout test now limits execution to 50 ms while allowing fixture persistence a normal setup deadline.
- Final review identified backward clock corrections invalidating persisted lifecycle timestamps. A regression failed before the fix; transitions now preserve timestamp order, and the full Rust target passed afterward. No other significant integrated review findings remained.
- No manual browser/device acceptance or real power-loss test was performed in this increment. Existing compiler dead-code, React act, and bundle-size warnings remain. Logs are local under `target/jobs-*.log`.

## Remaining boundaries

This queue does not resume a partially executed tool or implement the portable execution journal. Running-job cancellation is intentionally unavailable until the runner has a verified cancellation boundary. Unexpected worker panics retain the saved running claim for review at restart. Graceful shutdown waits for admitted work.

The next product slice should add owner-visible goal budgets, approvals, and artifact records, then connect the portable capability execution boundary. Third-party modules, rich document engines, camera/voice/home control, and the full visual redesign remain broader product work. Record retention/archive controls are also needed before lifting the fixed history cap.

Restart the daemon with the normal `bun dev --host rust` workflow to load the new routes and worker. This change is left uncommitted for review.
