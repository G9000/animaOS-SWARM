# Persistent workplace foundation — handoff

Work is on `codex/persistent-workplace-foundation`. The checkout was clean when work started. Changes are left uncommitted for review; no deployment, external messages, paid model calls, or third-party memory installation were performed.

## Delivered

- A source-grounded runtime audit covering saved tasks, execution/restart behavior, proactive schedules, peer messaging, delegated authority, memory, and native tools. Existing working safeguards were preserved rather than replaced.
- Memory mutation rollback across agent tools, task-result capture, evaluators, swarm relationship recording, and owner memory routes. Rejected pre-commit saves no longer leak into searches or later snapshots.
- Atomic JSON memory replacement using the existing dependency, with failure-injection coverage preserving the previous file. Commit acknowledgement ambiguity is treated separately from a rejected pre-commit write.
- An owner-authorized, no-store `/api/capabilities` endpoint derived from the actual native registry; typed SDK access; a searchable web view with requirements, configured storage, and explicitly unavailable future modules.
- A clearer workplace overview with useful next actions, counts that retain partial-data uncertainty, and direct work/files/capabilities navigation. Onboarding presentation emphasizes purpose and authority while preserving existing model setup, access choices, and workspace resume behavior.
- A more concrete Software Studio first assignment: saved product brief when write access exists, named owners, acceptance criteria, validation, and owner review.
- Extension-boundary documentation reusing existing core contracts and identifying the missing daemon execution bridge.

## Validation

- Full web suite: 336 tests passed. SDK suite: 37 tests passed, including daemon integration tests. Both packages built and typechecked.
- A later capabilities-only pass covered the unknown-category fallback and corrected storage label (4 tests, including one added regression). Web build/typecheck passed again after that change. Unchanged suites were not repeated.
- Final `bun x nx run rust-daemon:test --skipNxCache`: 997 checks passed across core/host unit, integration, and doc tests; 5 tests remain ignored. Independent review's postcommit persistence finding was corrected and re-reviewed with no further actionable findings. Logs are under `target/foundation-*.log` and `target/runtime-memory-*.log` (temporary, not committed).
- Browser automation inspected onboarding, overview, and the live 31-tool capability inventory against an isolated daemon with disposable stores. The capability page was checked at desktop and 390×844; document width was 390, with no horizontal overflow. Temporary viewport override was reset.
- A live preview restart preserved the agent ID, task content/revision, and private memory ID/content. This did not simulate a power failure or execute an interrupted autonomous job.
- Known validation limits: ignored Rust tests remain ignored; Unix-specific filesystem guarantees were not executed on this Windows machine; existing React act warnings and a Vite bundle-size warning remain.

## Still required for the larger product

This is a foundation increment, not the complete UI replacement or a finished autonomous startup operating system.

1. The subsequent [durable jobs increment](2026-09-08-durable-jobs-handoff.md) now wires queued job ownership and restart reconciliation into the daemon. Mid-tool checkpoints and arbitrary interrupted-work continuation remain unimplemented.
2. Add owner-visible goal budgets, approval/review lifecycle, and artifact records. Existing task lists and messaging limits are useful but not that complete workflow.
3. Implement the portable capability execution/installation bridge and process isolation before accepting third-party executable modules.
4. Deliver format-aware document processing and browser/computer-use engines. Text file and shell tools are already native; they are not a dedicated PDF/Word/spreadsheet engine.
5. Add camera, voice, and smart-home modules after the base contracts are executable. Inventory entries cannot be enabled yet.
6. Evaluate first-party memory correction/deletion/visibility/retrieval at realistic scale before choosing optional external adapters. Full snapshot costs and remote commit ambiguity remain architecture work.
7. Finish the broader visual overhaul and a complete startup acceptance scenario using the verified foundation.

## Run and review

Use the repository's normal `bun dev --host rust` workflow. Restart the daemon to load the new capability route; an old running daemon will return 404. The temporary visual QA daemon used port 18080 and disposable files under `target/foundation-preview`; it is not the user's configured workplace. The temporary daemon and Vite server were stopped after verification.

Read `2026-09-08-runtime-foundation-audit.md` for evidence and `2026-09-08-extension-boundary.md` for the extension implementation boundary.
