# Independent agent profiles

Each agent has an editable identity and personality, a memory inspector, and a recent activity view accessible from the chat header. Existing PATCH config persistence applies personality changes on the next run without replacing conversation history or permissions.

- [x] Add per-agent avatar GET/PUT/DELETE routes, keyed by the runtime UUID under workspace `assets/agent-avatars`. Validate image type and size, replace atomically, and serve without caching.
- [x] Expose avatar upload/removal through the SDK and web client; retain existing SDK memory reads and agent update contracts.
- [x] Show avatars in direct messages, the chat profile button, agent cards, and profile identity.
- [x] Extend the settings drawer with Profile, Memory, and Activity sections. Preserve existing config edits and capability controls.
- [x] Add personality styles that supplement existing instructions. Do not grant tools, schedules, or external-action permission through personality.
- [x] Inspect and search the most recent 100 agent memories with scope, timestamps, refresh, and distinct error/empty states. Ignore stale responses after switching agents.
- [x] Verify binary SDK uploads, daemon asset isolation and invalid replacements, profile switching, personality persistence, and desktop/mobile flows.

Avatars require a configured workspace. The workspace avatar remains a separate existing asset. Memory is read-only in this profile; activity reflects recorded messages and runtime status, not simulated presence. No background autonomy is added.
