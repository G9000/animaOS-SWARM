# Run supervision

Continue the approved persistent workplace build through the existing daemon-owned Runs workflow. This increment adds owner-reviewed proposals, bounded attempts, and immutable output snapshots, without claiming aggregate goal budgets, spend caps, tool-level approvals, or file artifact provenance.

## Contract

Create jobs with optional `maxAttempts` (1..3, default 3) and `requiresApproval` (default false). Both participate in request-key equivalence. Approved-proposal jobs begin `awaiting_approval`; they do not dispatch until the owner posts an approval with the current revision. `approvedAtMs` records approval; every explicit retry resets it and waits again when required. Cancel supports awaiting approval and queued jobs. The existing eight queued/running cap applies at approval and immediate creation/retry; pending proposals consume the 200 record cap only.

POST `/api/agents/{agent_id}/jobs/{job_id}/approve` takes `{revision}`. POST `/review` takes `{revision,decision:"accepted"|"changes_requested",note}` (note defaults empty; max 4000 bytes; changes_requested requires nonblank feedback). Existing owner authorization before body/state, no-store responses, transaction persistence, 409 conflicts, and rollback guarantees apply.

Every settled attempt retains an entry in `attempts`: `{attempt,status,startedAtMs,finishedAtMs,result,error,resultTruncated,review}`. Status is completed/failed/needs_review only. Review is null or `{decision,note,reviewedAtMs}` and may be decided once on the latest completed attempt. Review is acceptance of the saved output, not permission to perform external effects. A changes_requested completed result becomes eligible for explicit retry within maxAttempts; original output/feedback remains retained. A retry includes the latest feedback as owner feedback in the next model request. No automatic revision run occurs.

The output text is the existing bounded 64 KiB result snapshot, not a discovered workspace file. Mark truncation. Historical entries remain immutable apart from their one review decision; attempts are capped by the existing maximum of three. Keep top-level result/error for compatibility. Old snapshots default new fields; when restoring old settled records or retrying them, preserve their available output as a historical attempt. Do not invent outputs for already-lost legacy attempts. Persist history with the runner completion transaction and recover uncertain running attempts without replay. Snapshot validation rejects invalid limits, inconsistent statuses, duplicate/future attempt numbers, and malformed reviews without rejecting old records lacking history.

## UI and acceptance

Runs exposes a maximum-attempt selector and approval toggle with retained drafts/request keys. Pending jobs display Approve and start / Cancel proposal. History displays saved attempts, truncation, review decisions and feedback. Completed outputs offer Accept result / Request changes; changed work can be explicitly retried and clearly returns to approval when configured. Distinguish execution status from output acceptance. Never render model-produced output as executable HTML or silently enable tools.

Verify pending jobs cannot dispatch (including restart); approved jobs run; stale/unauthorized decisions do nothing; failed saves roll back; attempt budgets hold; prior output survives retry/restart; changes_requested feedback reaches the next request; defaults preserve old jobs; unknown outcomes remain reviewable. Run relevant Nx Rust/web/SDK tests and builds. No paid calls or external effects in validation.
