# Workspace agency overview

Approved: replace Dashboard with Overview and make it the workspace landing page. Chat remains accessible, rather than the default canvas.

- [x] Replace dashboard layout with goal/next step, attention, work, team responsibilities, recent outputs, and explicit first-assignment start. Preserve honest loading/offline/partial states.
- [x] Navigate via Overview, Work, Team, Files, and Chat; retain integrations/activity and responsive navigation. Hide chat-specific controls outside Chat.
- [x] Start a prepared assignment only on explicit click, targeting the manager through the existing durable run/idempotency path; preserve other chat drafts.
- [x] Add bounded read-only workspace file listing/text preview in daemon, SDK, and web, confined to the configured workspace.
- [x] Verify overview/navigation/start behavior and file API confinement with relevant Nx tests/build/typecheck, full Rust tests, and responsive visual inspection where available.

No invented approval states or claims that every assistant reply is a finished deliverable. Surface the actual data currently available, and link back to its owning conversation. No automatic work starts on landing.

Web verification: 332 tests, build/typecheck passed; SDK 35 tests, build/typecheck passed. Live desktop and 390px mobile overview/Work/Chat navigation inspected through CUA; mobile document width equals viewport. No assignment started in the user's workspace. Full rust-daemon Nx test target passed, including 365 unit tests and 47 workspace tests. File API owner-read authorization and capability-based filesystem confinement are verified, including Windows junction coverage. Unix-specific replacement tests require a Unix runner. The running daemon was not restarted; the new Files endpoints require a daemon restart. git diff --check passed.
