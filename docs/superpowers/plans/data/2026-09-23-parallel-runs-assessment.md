# Parallel runs per companion: code assessment (2026-09-23)

Read-only investigation used to plan M1. Paths: AR = `hosts/rust-daemon/src/agent_runs.rs`, ST = `hosts/rust-daemon/src/state.rs`, CR = `hosts/rust-daemon/src/connectors/runtime.rs`, SC = `hosts/rust-daemon/src/schedules.rs`, JB = `hosts/rust-daemon/src/jobs.rs`, RA = `hosts/rust-daemon/src/routes/agents.rs`, RM = `hosts/rust-daemon/src/routes/mod.rs`, RT = `packages/core-rust/crates/anima-core/src/runtime.rs`. Line numbers are as of commit 0eac187 and drift.

## Current run lifecycle

- **Entry points.** Everything ends in `run_transaction_admitted` (AR:493-520), which `tokio::spawn`s the run so a dropped caller cannot cancel the commit. `run` and `run_with_commit` fail fast on the global semaphore (AR:385, 417); `run_with_commit_waiting` waits (AR:442-448); `*_admitted*` variants take a caller-held permit.
- **`run_serialized` (AR:522-555).** One `Mutex<()>` per agent id (AR:875-882); the map entry is removed on drop when `strong_count == 2` (AR:83-103). Delegated/Peer rooms use `try_lock` and fail with 503 "Specialist is busy" (prevents nested-run deadlock); other rooms wait.
- **`run_locked` phase A** (under the global `control_plane_transaction`, AR:364-368, plus the state write lock): helper/peer/delegation checks (AR:585-653); the rollback baseline is the full `get_agent` snapshot, taken only if a rollback closure exists (AR:654-662); `take_agent_runtime` removes the runtime from `agents` and leaves a placeholder snapshot with `status = Running` (ST:2409-2428); `running_persist_request` is a full control-plane snapshot at the next revision (ST:1401-1412), saved before model work (AR:676-680) as the durable in-flight marker (restart turns Running into Failed, ST:2347-2349). If that save fails, the runtime is restored and the caller gets 503.
- **Phase B** (agent lock and permit only): temporarily rewrites system prompt and tools (AR:683-737); room: Stable uses its id, Peer uses `peer:{sender}:{target}`, Generated/Delegated get an id generated inside core (AR:743-754); Stable/Peer history is messages filtered by room (AR:778-783); restores the original config with `replace_config(original_config)` (AR:819).
- **Phase C** (transaction + write lock): `restore_agent_runtime` (ST:2430-2472) consumes the `deleted_agent_ids` tombstone (ST:2442), re-applies the latest snapshot config so a mid-run PATCH wins (ST:2443-2455), re-inserts the runtime unless deleted; the commit hook gets `(&mut DaemonState, restored snapshot, result)` (AR:838); on hook error `apply_run_rollback` runs and the error returns with no save; final save (AR:849-856), on failure rollback and 503; `persist_task_result_memory` runs after the transaction drops (AR:859-867).
- **Rollback.** `AgentRunRollback = FnOnce(&mut DaemonState, AgentRuntimeSnapshot)` (AR:79-81); `apply_run_rollback` hands it the baseline (AR:901-913). Connector and schedule hooks call `rollback_agent_runtime(baseline)` (ST:2477-2489; CR:1032, CR:1345; SC:672), restoring the whole pre-run transcript, events, tokens, last_task, and step count. Jobs ignore the baseline (JB:554-557).
- **What the agent lock guards:** sole ownership of the single runtime, baseline validity, and helper slot reservation (AR:302-321, 331; the helper then calls `run_locked` directly, AR:342).

## Fields a run mutates and how to merge them

A second concurrent run cannot start today: `take` finds nothing and returns 404 (AR:672-674).

| Field       | Written at                                                                                   | Merge at commit                                                                                                                                                                                                                                                                                    |
| ----------- | -------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| messages    | RT:254-275 (user 362, tool-call assistant 532, tool 700, evaluator retry 457-472, final 735) | Append the run's new messages; ids are globally unique (RT:25-28, 1070-1073) so per-room order survives                                                                                                                                                                                            |
| events      | RT:754-766                                                                                   | Append; AgentTokens events carry the run's own cumulative totals (RT:941-956), so they stop being monotonic (cosmetic)                                                                                                                                                                             |
| status      | RT:723/734/743; placeholder ST:2415                                                          | Must become derived: any run in flight ⇒ Running                                                                                                                                                                                                                                                   |
| token_usage | RT:935-939                                                                                   | Add the delta (run total minus its starting value)                                                                                                                                                                                                                                                 |
| last_task   | RT:736/744                                                                                   | Last commit wins                                                                                                                                                                                                                                                                                   |
| step_count  | RT:540-548                                                                                   | Add the delta; concurrent runs reuse indices — Postgres keys on `(agent_id, idempotency_key)` (postgres.rs:25) and the step_index unique was dropped (migrations/20260508000000_relax_step_index_unique.sql); the test-only `InMemoryAdapter` still matches on step_index (persistence.rs:116-119) |
| config      | rewrite AR:724-737, restore AR:819, re-apply ST:2443-2455                                    | Apply the rewrite to a per-run copy; the shared runtime is never touched; PATCH goes to the canonical runtime (ST:2291-2296) and the checked-out fallback (ST:2299-2330) goes away                                                                                                                 |

## What other paths see during a run

- GET list/get (RA:39-70 → ST:2358-2379): the placeholder, Running, with the pre-run transcript; in-flight messages invisible.
- PATCH (RM:1241-1265 → RA:90-144): patches the placeholder; restore re-applies it; the running task keeps its starting config.
- DELETE (RM:1222 → CR:1514-1643): `remove_agent` sets the tombstone (ST:2381-2388); if the save fails, `restore_removed_agent` (CR:1626-1630) re-inserts the Running placeholder, then marks it Failed (ST:2347) while the real runtime is still out.
- Schedules: one worker per agent (SC:398, 418-429); reconciliation keyed by agent (SC:454-476); connector-target schedules run in the Telegram room (SC:561).
- Connectors: every Telegram path uses room `telegram:{id}` (CR:249, 927, 1225). Hooks find the reply by scanning backwards for the last assistant message in that room and build the outbound record from its id (CR:979-990, CR:1258-1269; SC:623-633). `owner_send_replay` finds the keyed user message and the next assistant message in the room (CR:2288-2336), single-flighted per (connector, key) (CR:864-865). On restore, every outbound record's assistant message must exist in the persisted room (ST:1815-1827).
- Jobs: one per agent via a local set plus `is_agent_busy` (JB:426-427, 468-471, 502); each job runs in room `job:{id}` (JB:528).
- Helpers: the idle check is a `try_lock` on each helper's agent lock (AR:303-307).
- Memory: evaluator (components/evaluators.rs:43-200) and `persist_task_result_memory` use the shared memory write guard, held across persist (memory_store.rs:75-92); neither depends on the agent lock. The recent-memories provider is agent-wide (components/providers.rs:28-36) and task results carry `room_id: None` (AR:938).
- Todo tool: replaces the whole list with no revision check (tools/todo.rs:44).

## Correctness that depends on per-agent serialization today

- The take/restore model itself (a second run gets 404).
- Whole-snapshot rollback: it would erase another room's committed turn, and could leave an outbound record pointing at a deleted assistant message — a snapshot that then fails validation at restart.
- The single tombstone: the second run to restore would resurrect a deleted agent.
- Hooks' backwards scan — safe only while same-room runs stay serialized.
- Helper idle detection.

## Places that assume "Running means exactly one run"

RM:3452 blocks the owner's PUT tasks while Running; ST:2415/2458 the first run to finish flips the agent to Completed while another runs; team roster status (AR:226); tests AR:2188, 2466, 2473; RA:428; RM:3236; the CR:4921 test comment "wait on the agent lock".

## Smallest safe design

1. **Locks:** a mutex per `(agent, room_key)` plus a per-agent `Semaphore(N)`. Compute the room key before the run: the Stable id, or `peer:{src}:{tgt}`; for Generated/Delegated, generate the id up front and call `run_in_room_with_context_and_tools(id, vec![], …)`. Take the room lock first, then an agent slot. Nested Peer/Delegated runs try-acquire both. Helper reservation takes all N permits because reusing a helper rewrites its config agent-wide (AR:311).
2. **Copy the runtime, don't remove it.** The shared runtime stays in `agents`; each run builds its own copy with `from_snapshot` plus providers, evaluators, and db, reusing the wiring at ST:2337-2346 without the Running→Failed conversion. An in-flight counter supplies status.
3. **Commit** merges the run's changes as in the table. The hook receives the run's reply message id instead of scanning.
4. **Rollback by change set:** remove only this run's message and event ids and subtract its tokens; drop the baseline parameter from `AgentRunRollback`; replace `rollback_agent_runtime` at CR:1032, CR:1345, SC:672.
5. **Delete:** a commit for a deleted agent is discarded, so `deleted_agent_ids` can go.
6. **Other:** `is_agent_busy` becomes "no free slot"; RM:3452 uses the in-flight count; reserve the `telegram:`, `job:`, and `schedule:` room prefixes at RA:181 (only `peer:` is blocked today).

## Main risks

Rollback clobbering other rooms' turns (can make a persisted snapshot unbootable); tombstone resurrection; status reporting Completed mid-run; per-agent side resources becoming last-writer-wins (todo list, workspace files, calendar/mail drafts); runs waiting on a room lock holding global permits (AR:442-448); the global control-plane mutex held across disk saves twice per run (AR:582-681, 821-857), capping throughput; cross-room memory leakage via agent-wide recent memories.

## Tests that need to change

- AR: `same_agent_runs_wait_then_both_execute` (2104; its `request()` helper uses Generated rooms, AR:2477-2487, so the two runs become concurrent — move it to one Stable room and add a cross-room concurrency test); `aborted_caller_does_not_cancel_restore_or_leave_a_stale_agent_lock` (2144; relies on `lock_count`); `delegation_rejects_self_missing_target_escalation_and_non_manager` (holds the agent lock at 2010); `failed_commit_leaves_runtime_restored_without_final_snapshot` (2432); `commit_runs_after_runtime_restore_and_before_final_snapshot` (2383); `spawn_helper_atomically_caps_busy_helpers_and_reuses_slots` (1614); `spawn_helper_timeout_releases_capacity_and_saves_failed_state` (1727).
- CR: `serialized_rollback_preserves_a_turn_committed_while_connector_waited` (4921) needs a rewrite; re-check the "equals baseline" rollback assertions at 4268, 4710, 4797.
- RA: tests at 394, 441, 499.
- ST: the fixture `add_persisted_room_assistant_message` (956-979), which simulates a checked-out runtime.
- gcalendar: tests calling `take_agent_runtime` (connectors/gcalendar/tests.rs:1093, 1134).
- `scheduler_starts_new_due_agent_while_another_is_running` (SC:1062) is unaffected unless the scheduler's one-per-agent rule is relaxed (the spec relaxes it to one run per automation).
