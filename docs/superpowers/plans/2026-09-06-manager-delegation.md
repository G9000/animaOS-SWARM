# Workspace manager roster and delegation

The workspace manager must know which agents exist and be able to assign bounded work to existing specialists. The daemon owns this behavior, so web, connector, and scheduled manager runs share the same implementation.

- [x] Supply current roster data on each coordinator run and expose a live roster tool to the designated workspace manager.
- [x] Expose manager-only delegation to existing specialists, record each specialist conversation, and return actual results and failures.
- [x] Reject self-delegation, manager targets, recursive delegation, busy targets, missing targets, and tool permission escalation. Recheck manager permissions before delegated tool execution.
- [x] Reuse admitted, serialized, persisted agent execution. Remove temporary roster/tool context before saving the agent configuration.
- [x] Validate end-to-end tool dispatch, persisted specialist results, permission failures, busy targets, original config restoration, and the full Rust daemon target.
- [x] Preserve the durable manager role when recreating a manager in an existing workspace. All 37 onboarding tests and web typecheck pass.

Verification: `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache` passed (342 daemon unit tests passed, one ignored, plus all integration suites and core dependencies). Delegation tests use a deterministic test model; no paid model or external publishing call was made.

Delegation does not create agents, start schedules, or grant additional filesystem/process/integration capabilities. The existing main-agent role marker enables coordination on the next run; no stored profile rewrite is required. Deploying the changed daemon requires restarting it.
