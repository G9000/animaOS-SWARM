# Per-agent tasks and proactive work

- [x] Give each agent a dedicated persisted task list under `.animaos-swarm/agent-tasks`, keyed by encoded runtime ID. Preserve the legacy shared todo file.
- [x] Route each agent's todo tools to the same store as the Tasks UI.
- [x] Expose GET/PUT tasks in the daemon and SDK. Use revision checks for stale edits and reject UI saves during an active agent run.
- [x] Add separate Tasks and Proactive profile sections with mobile scrolling navigation.
- [x] Reuse durable per-agent daemon schedules for opt-in proactive prompts, interval controls, pause/resume, and last-run outcomes. Do not enable schedules simply because tasks exist.
- [x] Add SDK methods for listing, creating, updating, and deleting per-agent schedules.
- [x] Verify task isolation, stale revisions, browser edit conflicts, schedule isolation and pause, desktop/mobile rendering, and SDK contracts.

The new task list starts empty for each agent because the old shared list has no ownership information. It is preserved for recovery. Tool permissions still govern proactive work; the UI can configure tasks without granting additional runtime tools.
