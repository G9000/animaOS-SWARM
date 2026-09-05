# Workspace Cockpit Implementation Plan

**Goal:** Turn the existing Studio refresh into a faster, more useful web workspace, inline in the current checkout.

**Architecture:** Keep daemon state authoritative. Add client-side navigation and conversation tools through small components, without changing host contracts, access profiles, or schedule ownership. Keep drafts in memory and never silently send prompt templates.

**Tech stack:** React, TypeScript, existing Tailwind/CSS tokens, Vitest and Nx.

## Design

Build on the existing warm Studio identity rather than introducing an unrelated theme. The sidebar becomes operational, with real usage and agent status. A command dialog joins navigation and six reusable prompt starters. Conversation tools provide literal text search, result navigation, local Markdown download and individual copy actions. Search and scroll controls must not steal the reader's position as responses arrive. Focus mode temporarily removes the desktop sidebar but always provides an exit.

Onboarding becomes a split introduction/form layout on desktop, with a compact mobile flow. Agents gain search by name/model/provider and clear empty results. Activity exposes input/output token usage without invented cost estimates.

## Implementation

- [x] Add tests for command navigation, prompt insertion, focus mode, agent filtering, conversation search/copy/export and safe Enter handling.
- [x] Add `components/CommandMenu.tsx` for a focus-contained, searchable keyboard dialog and `lib/prompt-library.ts` for reusable starter content.
- [x] Integrate commands, focus mode and real status summaries in `components/WorkspaceShell.tsx`; pass prompt insertion from `ViewHarness.tsx`.
- [x] Add `components/ConversationTools.tsx` and `lib/conversation.ts` for literal search, selection and a local Markdown export.
- [x] Extend `components/ChatScreen.tsx` with copy actions, scroll anchoring and non-destructive jump controls; guard Enter during IME, disabled or sending states.
- [x] Preserve failed sends as an explicit recovery queue without overwriting a newer draft; block network sends while offline in `ViewHarness.tsx`.
- [x] Add agent filtering, token breakdown, onboarding composition and responsive styles in `cockpit.css`, alongside the existing `studio.css`.
- [x] Run focused and full web tests, typecheck, build, and desktop/mobile browser QA; review changed code. Remove temporary QA artifacts.

## Verification

Run `bun x nx test @animaOS-SWARM/web --run --skipNxCache`, `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache`, and `bun x nx build @animaOS-SWARM/web --skipNxCache`. Check narrow mobile and short desktop screens, modal keyboard behavior, and no horizontal page overflow. Do not create real workspace data just to reach configured screens.

### Results

- 228 tests passed across 23 files; typecheck and production build passed.
- Browser QA: desktop conversation search/jump, command filtering and draft insertion; mobile command menu, conversation, agent cards, activity; actual onboarding on desktop/mobile. Temporary sample-data entry points removed.
- Review found two recovery races. Both fixed and regression-tested: queued failures survive later sends and concurrent settings saves, while agent lifecycle isolation remains intact.
- Existing non-blocking React test `act` warning and Vite bundle-size warning remain. Drafts and recoverable messages are session-memory only; no backend or access-policy changes.
