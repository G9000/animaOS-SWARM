# Run supervision implementation plan

User approved continuing the workplace build. Use subagent-driven development for bounded independent daemon and web ownership; root integrates contracts and reviews evidence. Keep current branch and preserve unrelated edits.

- [x] Daemon jobs: add validated defaults, pending approval, per-job attempt budget, saved attempt output/review lifecycle, explicit change-request retries and feedback. Own `jobs.rs` and `jobs/*`; add behavior tests first.
- [x] HTTP integration: add approve/review routes and request schemas, extend creation contract, owner/no-store/CAS/roundtrip route regressions. Root owns `routes/jobs.rs`, `routes/mod.rs`, `routes/tests/jobs.rs`, any snapshot integration.
- [x] SDK/web: extend typed contracts and methods, Runs controls and saved outputs, retained input/idempotency behavior, accessible review feedback. Add focused tests first; preserve existing UI conventions.
- [x] Independent review of approval/persistence/history and contract integration. Verify with Rust daemon Nx test and full SDK/web test, typecheck, build; limit repeats to changes/failures.
- [x] Update handoff and mark verified scope, deferred aggregate goals/spend budgets, file artifact provenance and broader UI/modules.

Validation evidence and remaining limits: docs/design/2026-09-08-run-supervision-handoff.md. Root performed the integrated review; additional reviewer delegation was unavailable due the agent limit.
