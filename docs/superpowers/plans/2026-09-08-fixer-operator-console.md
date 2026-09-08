# Fixer operator console implementation plan

**Goal:** Restructure the Anima web console around assignment dispatch, execution, and result review, using the live Fixer dashboard (127.0.0.1:3000) and worker site (127.0.0.1:3002) as the visual references.

**Architecture:** Keep daemon contracts and authority unchanged. Add a team-wide Operations queue that reads existing agent jobs and opens the existing run controls for one selected job. Reuse the tested submission and review logic. Keep goals, tasks, schedules, notes, files, chat, connectors, and settings reachable. This is a web console overhaul, not a physical-worker marketplace implementation.

**Visual design:** Fixer charcoal canvas (#111114), panels (#1c1c20), separators (#303038), pink accents (#e94dab), subdued metadata, system typography, compact controls, and a split queue/detail workspace. Mobile uses a queue-to-detail drill-down with a back action. Status labels always accompany colors.

**Tech stack:** React, TypeScript, Tailwind/CSS, existing daemon SDK, Vitest, browser QA.

- [x] Add regression coverage for the Operations landing, team queue, status filtering, selection, unavailable agents, and offline actions.
- [x] Build `WorkspaceOperations.tsx` and adapt `AgentRuns.tsx` for an optional selected run / new-assignment view, preserving existing consumers.
- [x] Reorganize `WorkspaceShell.tsx` around Operations, Goals, Work, Team, Files, and supporting surfaces.
- [x] Replace warm Studio colors and decorative styling across shell, chat, onboarding, settings, and shared surfaces with Fixer tokens and compact geometry.
- [x] Run web tests, typecheck, build, and desktop/mobile browser checks. Review the diff and record remaining limitations.

**Validation:** `bun x nx run @animaOS-SWARM/web:test --run --skipNxCache`, `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache`, and `bun x nx run @animaOS-SWARM/web:build --skipNxCache`. No Rust changes are planned.

## Review refinements

- The queue polls every 10 seconds for five minutes, supports manual refresh, aborts obsolete loads, and refreshes on reconnection. Run mutations and revision conflicts notify the queue. Failed agent loads retain prior rows with a stale label and disclose incomplete counts.
- Selection stores both agent ID and job ID independently of chat, filters, and new-assignment mode. A removed agent or missing job receives an explicit unavailable state. Agent-scoped submission drafts retain the existing session-storage and idempotency behavior.
- Offline state disables all run mutations and fetches. Finished but unreviewed outputs appear in Review; interrupted/uncertain execution retains its separate label and retry acknowledgment. Acceptance and requests for changes retain existing authority semantics.
- Overview, Capabilities, Activity, Telegram, chat, commands, and the prepared first-assignment action remain reachable. Goals receives a direct navigation entry; Work retains existing task, note, and schedule management.

## Validation results

- Web tests: 40 files, 364 tests passed.
- Web production build and typecheck passed. Build retains a non-blocking bundle-size warning.
- Browser QA at desktop and 390px mobile: queue, selection, saved output, new assignment, Team, chat, settings, and onboarding. Mobile document width matched its 390px viewport.
- The daemon was offline. Browser checks used isolated temporary fixtures, removed after QA; no real assignment or model call was dispatched.
- Code review findings on stale details, offline selection, removed owners, and goal retry were fixed with regression tests.

## Exact palette follow-up

The prototype HTML was the wrong reference. The user identified the live sites on ports 3000 and 3002. Read their computed CSS and verified the corrected Anima rendering: accent #e94dab, actions #e978be with #191119 text, canvas #111114, panels #1c1c20, separators #303038, text #f4f4f5, muted #a4a4b1, sidebar #17171b. Palette tests pass. No prototype-blue palette remains in the console styles.
