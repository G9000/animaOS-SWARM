# Durable daemon jobs

The next approved workplace increment makes a one-off assignment a saved job instead of a long-lived browser request. This is a host queue around the existing AgentRunCoordinator, not a replacement for the portable durable engine or a claim of mid-tool checkpoint resumption.

## Contract

Jobs belong to one existing agent. Creation explicitly queues work and requires configured control-plane persistence. Saving a todo remains separate and never queues a job or enables a schedule. Store job records with the existing control-plane snapshot; old snapshots default to no jobs.

Lifecycle: queued -> running -> completed or failed. Restored/orphaned running jobs become needs_review and are not replayed. Queued jobs are dispatched when the daemon is active. A human can cancel queued jobs or explicitly retry failed/needs_review jobs using a revision check. Retry of uncertain work requires acknowledgement and can repeat external effects. At most three attempts; no automatic failed-job retry.

Creation is idempotent within an agent for an exact title/prompt tuple with an owner-supplied request key; reusing that agent's key for different input is a conflict. Keys are agent-scoped, so separate agents can use the same key. Persist the dispatch claim before invoking the existing runner. Use a job/attempt key for existing runner idempotency. Persist completion with the runner's final snapshot when possible and retain uncertainty on write failure. Existing live tool/peer/delegator authority checks remain authoritative.

Keep at most 200 job records, eight queued and running jobs combined, a 32 KiB prompt, a 160-character title, and a bounded 64 KiB result preview. Refuse new work at capacity instead of silently deleting history. Do not advertise running-job cancellation without a verified runner cancellation boundary.

## HTTP and web

GET/POST `/api/agents/{agent_id}/jobs` lists/queues jobs. POST `/api/agents/{agent_id}/jobs/{job_id}/cancel` and `/retry` apply revision-checked actions. All endpoints authorize local-owner access before reading payload or job state and use no-store responses. SDK mirrors the contract. Work hub gets an agent-scoped Runs section showing queued/running/completed/failed/review states, manual refresh/polling, explicit queue action, and safe retry/cancel controls. Errors and offline state must not appear as an empty queue.

## Acceptance

Prove idempotent create, queue persistence before dispatch, failed save rollback, successful daemon-only execution, interrupted work requiring review, queued work surviving restart, current authority on execution, revision conflict, and retry bounds. Keep tests deterministic and bounded; full Rust/web/SDK verification after integration, then rerun only changed/failing checks.
