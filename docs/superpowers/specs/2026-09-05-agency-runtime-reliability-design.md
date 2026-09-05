# Agency runtime reliability

Approved direction: a persistent, daemon-owned agency with missions, recoverable tasks, budgets, safe autonomy, and mission-control UI. Implement inline; preserve existing UI edits. This is the first independently verifiable phase, not a claim that checkpointed missions are already integrated.

## This delivery

- Swarm budgets gate further agent runs and daemon model generations once observed total usage reaches the configured per-dispatch budget. Zero disables execution; None is unlimited. In-flight provider calls may overshoot; this is not a hard billing cap. Responses already obtained remain accounted for.
- Daemon swarm usage updates after every model response, including manager responses before delegation. Core custom factories retain run-boundary enforcement through their usage hooks. A new dispatch resets accounting.
- Completed swarm totals retain manager and worker usage after ephemeral agents retire. Live idle prewarmed workers still support usage inspection.
- The daemon scheduler admits bounded independent jobs without awaiting one job before scanning for another. At most one scheduled job per agent is active, preventing a backlog on that agent from consuming scheduler slots. Due work is ordered by due time then ID. Existing shared daemon admission, persistence-before-run, and connector commit rules remain.
- Claiming a schedule clears its previous outcome. On restart, a claimed occurrence with no terminal outcome is disabled and recorded as interrupted/requiring review. Never automatically replay an uncertain tool action. Users may explicitly re-enable the schedule after review. Legacy records whose outcome predates their latest claim are also reconciled.
- Startup admission stays closed until reconciliation is persisted successfully; persistence errors retry reconciliation without executing jobs. Per-agent exclusion lasts through the owned run and final durable commit, including graceful shutdown.

## Boundaries and follow-on

Budget coordination belongs in anima-swarm; model wrapping and scheduler I/O belong in rust-daemon. Preserve HTTP owner guards, capabilities, and persisted snapshots. No real model calls, credential changes, live workspace resets, or UI edits in this phase.

Persistent mission/task dependencies and DurableAgentEngine storage integration are the next phase. They require a concrete durable ExecutionStore host adapter and capability recovery bindings; do not label legacy snapshot restoration as checkpoint resume.

## Verification

Regression tests cover token totals, zero/exhausted/reset budgets, manager-to-worker shared usage, independent schedule progress, concurrency bounds, same-agent exclusion, and restart reconciliation without provider calls. Use Nx core and daemon tests with a separate daemon validation target directory. Check formatting and obtain independent review before reporting completion of this phase.
