# Companion Console M7: Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **This is a lean plan:** each task gives files, exact interfaces, rules, a test list, commands, and a commit message, and complete code only for the few tricky pieces. The implementer writes the tests and the rest of the code from the interfaces, test-first, against the code as it is.

**Goal:** Let the owner see and fix what the companion remembers (spec §10, §15.4 Memory, §15.2 "Save to memory"): edit and delete a memory (re-indexed and re-embedded; its citations cleaned up), list, replace, and forget temporal facts, delete an entity with its relationships, all behind owner authorization; a web Memory page with three tabs (Memories, About you, People & things) and a "Save to memory" action on chat messages. Model-written memory text is refused when it hides characters, and shown in the web only as text with hidden characters revealed.

**Architecture:** The memory manager is not part of the control-plane snapshot: it has its own store (`memory_store.rs`, `MemoryMutation`) and its own embeddings runtime (`memory_embeddings.rs`). So M7 touches no control-plane state. `anima-memory` gains three methods in a new `memory_manager/edit.rs` (`update_memory`, `delete_memory`, `delete_entity`). The daemon gains owner routes in a new `routes/memory_edits.rs` (the existing `routes/memories.rs` keeps its routes and their authorization) over a small shared module `memory_text.rs` (limits, the hidden-text check, tag cleaning). Every owner mutation runs in its own `tokio::spawn` under the memory write lock, saves through `MemoryMutation`, and syncs the embedding before releasing the lock. The SDK `MemoriesClient` gains the new calls; the web gets `lib/memory.ts`, `hooks/useMemory.ts`, `pages/MemoryPage.tsx`, and `hooks/useSaveToMemory.ts`.

**Tech Stack:** Rust 2021 (tokio, axum 0.8, serde, utoipa 5), TypeScript (React 19, Vite, Tailwind v4, Vitest, Testing Library), Nx with Bun.

**Spec:** `docs/superpowers/specs/2026-09-23-companion-console-design.md` (§10 Memory is the core; also §3 sessions, §6 events (no new event), §13 persistence, §14 owner authorization, §15.2 message actions, §15.4 Memory page, §15.5 `useMemory`, §16 limits, §17 tests). Master plan: `docs/superpowers/plans/2026-09-23-companion-console.md` (M7, T7.1–T7.3, and Global Constraints).

## Global Constraints

- Master plan Global Constraints apply. **No new third-party dependencies and no new dependency features** (Rust, SDK, web). `anima-memory` and `anima-core` gain no HTTP, DB, or host dependency (`anima-memory` stays `[dependencies] anima-core` only).
- **Precondition: M6 is merged.** Before Task 1 run `git log --oneline -1 && grep -n "CONTROL_PLANE_STORE_VERSION: u32 = 9" hosts/rust-daemon/src/control_plane_store.rs && grep -n "fn is_smuggling_character" hosts/rust-daemon/src/skills/mod.rs && ls apps/web/src/pages/AutomationsPage.tsx`. Expected: head at or after `d5cefa2`, and a match in each. Otherwise stop and report.
- **Snapshot version stays 9.** M7 changes no control-plane state: the memory manager persists through `memory_store.rs` (its own file or Postgres table, own format), and update, delete, and the fact and entity changes use fields the store already holds. No backup file, no version bump, no tool grants, no new event type (spec §6 lists none for memory; the page reads on open and after each of its own changes). Rolling back to an M6 daemon is safe. Task 9 greps that `CONTROL_PLANE_STORE_VERSION` is still 9.
- **Routes, exactly** (spec §10; all new, all `#[utoipa::path(... tag = "memories" ...)]` registered in `ApiDoc`, all answer `Cache-Control: no-store`; mutations call `state.local_owner.authorize` (403 `local owner authorization required` otherwise), reads `authorize_read`; the handlers live in `routes/memory_edits.rs` and reuse `routes::jobs::{authorize, no_store}` as `routes/skills.rs` does). The existing memory routes keep their shapes and authorization (spec §10).
  - `PATCH /api/memories/{memory_id}` with `{ content?, importance?, tags? }` → 200 `MemoryResponse` (the existing shape); 400, 403, 404, 503. `tags: null` clears the tags; an array replaces them; absent keeps them.
  - `DELETE /api/memories/{memory_id}` → 200 `{ id, removedRelationships, updatedRelationships, updatedFacts }` (counts); 403, 404, 503.
  - `GET /api/memories/facts?agentId=&subject=&includeInactive=&limit=` → 200 `{ facts: MemoryFact[] }`, newest first; 400 (`FACTS_LIMIT_INVALID`, bad `includeInactive`), 403.
  - `PATCH /api/memories/facts/{fact_id}` with `{ value }` → 200 `{ fact: MemoryFact, superseded: MemoryFact }`; 400, 403, 404, 409 (`FACT_NOT_EDITABLE`), 503.
  - `DELETE /api/memories/facts/{fact_id}` → 200 `{ id }`; 403, 404, 503.
  - `DELETE /api/memories/entities/{entity_id}?kind=agent|user|system|external` → 200 `{ kind, id, removedRelationships, removedFacts }`; 400 (`ENTITY_KIND_REQUIRED`), 403, 404, 409 (`ENTITY_OWNS_MEMORIES`), 503. `kind` is a query parameter because an entity's key is its kind plus its id (spec §10 names only the id; this is the one addition).
  - Static segments win over `{memory_id}` in the router (`/api/memories/facts` is registered with the other static routes); the existing `/api/memories/{memory_id}/trace` is untouched.
- **JSON, exactly.** `MemoryFact` (camelCase; absent values are `null`): `id`, `subjectKind`, `subjectId`, `subjectName`, `predicate`, `objectKind`, `objectId`, `objectName`, `value`, `validFrom`, `validTo`, `observedAt`, `confidence`, `evidenceMemoryIds`, `supersedesFactIds`, `status` (`"active" | "superseded" | "retracted"`), `tags`, `roomId`, `worldId`, `sessionId`, `createdAt`, `updatedAt`. Timestamps are epoch milliseconds.
- **Limits, named once** (`memory_text.rs`, mirrored in the SDK): `MAX_MEMORY_EDIT_CHARS = 8_000` (PATCH content; POST keeps its present behavior), `MAX_MEMORY_TAGS = 20`, `MAX_MEMORY_TAG_CHARS = 40`, `MAX_FACT_VALUE_CHARS = 500`; `routes/memory_edits.rs`: `DEFAULT_FACTS_LIMIT = 100`, `MAX_FACTS_LIMIT = 500`, `FACT_REPLACEMENT_CONFIDENCE = 1.0` (the owner stated it). Web: `MEMORY_PAGE_LIMIT = 200` (memories read at once).
- **Strings, exact** (named constants, each tested once):
  - `hosts/rust-daemon/src/memory_text.rs`: `MEMORY_TEXT_HIDDEN = "Memory text must not contain invisible tag or direction-override characters"`, `MEMORY_CONTENT_INVALID = "content must be 1 to 8000 characters"`, `MEMORY_TAGS_INVALID = "tags must be at most 20 non-empty tags of at most 40 characters"`, `FACT_VALUE_INVALID = "value must be 1 to 500 characters"`.
  - `hosts/rust-daemon/src/routes/memory_edits.rs`: `MEMORY_PATCH_EMPTY = "provide content, importance, or tags to change"`, `FACT_NOT_EDITABLE = "Only an active fact with a value can be edited"`, `ENTITY_KIND_REQUIRED = "kind query parameter must be one of agent, user, system, external"`, `ENTITY_OWNS_MEMORIES = "This entity still has memories; delete them first"`, `FACTS_LIMIT_INVALID = "limit must be from 1 to 500"`, `MEMORY_TASK_FAILED = "The memory change did not finish; check Memory and try again"` (503).
  - `anima-memory` messages (`MemoryError::message`): `InvalidMemoryContent => "content must not be empty"`, `EntityOwnsMemories => "entity still owns memories"`.
  - Web strings are named constants in `apps/web/src/lib/memory.ts` (Task 6) and `hooks/useSaveToMemory.ts` (Task 8).
- **Untrusted content (model-written memory text).** The companion writes memories (`memory_add` tool, the evaluator's extractions) and the owner can save a model message (Save to memory). Text that smuggles instructions must not enter memory, because memories are injected into later prompts. Rule: a memory's content, tags, and a fact's value refuse Unicode tag characters, bidirectional embeddings, overrides, isolates, and variation-selector supplements (`skills::is_smuggling_character`, already `pub(crate)`), through one function `memory_text::has_hidden_text`, in: the `memory_add` tool (Task 4), `POST /api/memories` (Task 4, a deliberate small change to an existing route), `PATCH /api/memories/{id}`, and `PATCH /api/memories/facts/{id}`. Restored records are not re-checked. Other invisible format characters (zero-width space and the like) are accepted by the daemon and **revealed** in the web: every model-written string the Memory page and the chat show goes through `revealInvisible` (`apps/web/src/lib/skills.ts`) and renders as a text node, never through `MarkdownMessage` or HTML. Task 9 greps for this.
- **Concurrency, every task.** Lock order: memory write lock (`DaemonState::memory_handle()`, a tokio `RwLock`) → embeddings write lock (a leaf). The memory store is separate from the control plane: no control-plane transaction is taken, and the state lock is held only long enough to clone the handles (`state.read().await.memory_handle()`), never while awaiting the memory lock. No `std::sync::Mutex` is held across `.await`. **Every owner mutation runs its whole body (lock, mutate, save, embedding sync) in its own `tokio::spawn`** (`memory_edits::locked`, Task 2), so a dropped request never leaves a half-applied change (`MemoryMutation` already restores the snapshot when dropped before a save; the spawned task finishes either way). The embedding is synced while the memory write lock is still held, so two concurrent edits cannot leave the vector of the older one.
- Existing behavior stays except where a task says so. Deliberate changes: `POST /api/memories` and the `memory_add` tool refuse hidden-Unicode text; nothing else of the existing routes changes. `MemoryManager::forget` keeps its present behavior (it does not clean citations); the new `delete_memory` is what the owner's delete calls.
- Commands. Rust iteration: `CARGO_INCREMENTAL=0 cargo test -p anima-memory --lib -- <filter>`; `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- <filter>`; integration: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test memory_api 2>&1 | tail -30` (pipe the others through `tail -30` too). SDK: `bun x nx test @animaOS-SWARM/sdk`, and **every SDK-changing task ends with `bun x nx run @animaOS-SWARM/sdk:build`** so later direct web Vitest runs resolve the new exports. Web: `cd apps/web && bun x vitest run <files>`. The milestone gate (Task 9): `bun x nx run rust-daemon:test --skipNxCache` and `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`.
- Formatting. Every task ends with `cargo fmt --all` when it touched Rust (then `git diff --stat` must show only the task's files; if `cargo fmt` reformatted unrelated files, tell the controller instead of staging them) and `bun x nx format:write --files=<each changed TS/TSX/CSS/MD file>` when it touched TypeScript, CSS, or Markdown, then re-runs its tests.
- Git. Stage files by explicit path only; never `git add -A`, `git add .`, or `git commit -a`. Never stage anything under `docs/` or `.superpowers/`, nor `nx.json` or `anima.yaml`. `hosts/rust-daemon/README.md` is not under `docs/` and is staged with the task that changes it. Never use `git stash`, `git reset`, `git checkout -- <path>`, `git restore`, or `git worktree`, and never switch branches. Do not start the daemon, a dev server, a database, or a container; tests start what they need. Commits are GPG-signed on this machine and can block on a pinentry dialog until the owner answers it. End each commit message with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Disk is tight: never set a new `CARGO_TARGET_DIR`. No Postgres is available (the memory store's Postgres path stays untested here, as before).
- Large files stay put: `agent_runs.rs`, `connectors/runtime.rs`, and `ViewHarness.tsx` only gain wiring lines (M7 adds about four to `ViewHarness.tsx` and none to the other two); new logic goes in new modules and components. Web tests stay pristine: no new `act()` warnings or console noise.
- Windows: no test builds a path by string concatenation; temp paths use `std::env::temp_dir().join(…)` with a UUID.
- Tests are deterministic: no sleeps and no wall-clock races. Web search is submitted (Enter or the button), not debounced, so no test needs timers.
- Code fences: complete functions keep their language; partial fragments are fenced as `text` so Prettier leaves them alone.
- Out of scope (do not build): adding a memory by hand on the page; bulk delete; editing relationships or entity names; merging duplicate entities; export; a memory event on the stream; embedding facts; memory scopes or sharing controls; Telegram memory commands; retention controls (the existing `POST /api/memories/retention` stays an API only).

## Review Focus

1. **Deleting and editing keep the indexes honest.** After `update_memory` the old words no longer find the memory and the new ones do; after `delete_memory` neither the text index nor the vector index holds it; no citation of a deleted memory remains. Tests: Task 1 (`update_memory_reindexes_the_text_index`, `delete_memory_removes_citations_from_facts_and_relationships`), Task 2 (`patch_re_embeds_changed_content_only`, `delete_removes_the_memory_its_embedding_and_citations`).
2. **The citation cleanup does not over-reach.** `delete_memory` removes a relationship only when it cited that memory and is left with none; relationships that never had evidence stay. (The existing `prune_relationship_evidence` removes every evidence-less relationship, so it must not be reused as is.) Test: Task 1 (`delete_memory_keeps_relationships_that_never_cited_it`).
3. **Entity deletion stays deleted.** The store rebuilds entities from memories, relationships, and facts on load, so deleting an entity must also remove what would recreate it, and refuse while memories belong to it. Tests: Task 1 (`delete_entity_survives_a_snapshot_round_trip`, `delete_entity_refuses_while_it_owns_memories`).
4. **Owner-only and drop-safe.** Every new route 403s a non-owner with nothing changed; a dropped request still finishes; a failed save restores the memory and answers 503. Tests: Task 2 (`the_memory_edit_routes_refuse_a_non_owner`, `a_dropped_patch_still_finishes`, `a_failed_save_restores_the_memory_and_answers_503`).
5. **Hidden text.** Refused where the model writes; revealed where the owner reads. Tests: Task 4, Task 6 (`reveals invisible characters in text`), Task 7 (`shows hidden characters as markers`).

## File map

| Area           | Files                                                                                                                                                                                                                                                                                                                                                                                                        |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Crate          | `packages/core-rust/crates/anima-memory/src`: create `memory_manager/edit.rs`, `memory_manager/edit_tests.rs`; modify `memory_manager.rs` (two module lines, one `pub use`), `memory_manager/types.rs` (two `MemoryError` variants), `lib.rs` (re-exports)                                                                                                                                                   |
| Daemon         | `hosts/rust-daemon/src`: create `memory_text.rs`, `routes/memory_edits.rs`, `routes/contracts/memory_edits.rs`, `routes/tests/memory_edits.rs`; modify `lib.rs` (one module line), `routes/mod.rs` (routes, `ApiDoc`, test module line), `routes/contracts/mod.rs`, `routes/memories.rs` (hidden-text check, one visibility change), `tools/memory.rs`, `tests/memory_api.rs`, `hosts/rust-daemon/README.md` |
| SDK            | `packages/sdk/src`: modify `memories.ts`, `index.ts`; create `memories.spec.ts`                                                                                                                                                                                                                                                                                                                              |
| Web data       | `apps/web/src`: create `lib/memory.ts`, `lib/memory.test.ts`, `components/memory/RevealedText.tsx`, `hooks/useMemory.ts`, `hooks/useMemory.test.tsx`, `test/memory.ts`; modify `lib/daemon-api.ts`, `lib/daemon-api.test.ts`                                                                                                                                                                                 |
| Web page       | create `pages/MemoryPage.tsx`, `pages/MemoryPage.test.tsx`, `components/memory/{MemoryList.tsx,FactList.tsx,EntityList.tsx}`, `memory.css`; modify `styles.css`, `components/WorkspaceShell.tsx` (+ test), `components/icons.tsx` (only if no icon fits), `ViewHarness.tsx` (wiring), `ViewHarness.test.tsx`                                                                                                 |
| Save to memory | create `hooks/useSaveToMemory.ts`, `hooks/useSaveToMemory.test.tsx`, `components/memory/SaveToMemory.tsx`; modify `lib/transcript.ts` (`TranscriptActions`), `hooks/useTranscriptActions.ts`, `components/ChatScreen.tsx` (+ tests)                                                                                                                                                                          |
| Docs           | `docs/superpowers/plans/2026-09-23-companion-console.md` (the M7 status row, Task 9, controller only)                                                                                                                                                                                                                                                                                                        |

## Task list

1. `anima-memory`: `update_memory`, `delete_memory` with citation cleanup, `delete_entity` (T7.1)
2. Daemon: owner routes to edit and delete a memory, with embedding sync (T7.2)
3. Daemon: facts routes and entity deletion, OpenAPI, README (T7.2)
4. Daemon: hidden-Unicode refusal in `memory_add` and `POST /api/memories` (T7.2)
5. SDK: memory edit, facts, and entity calls (T7.3)
6. Web data: memory helpers, `RevealedText`, the facade, and `useMemory` (T7.3)
7. Web Memory page: Memories, About you, People & things, and the destination (T7.3)
8. Web "Save to memory" on chat messages (T7.3)
9. M7 verification (controller)

---

### Task 1: `anima-memory`: `update_memory`, `delete_memory` with citation cleanup, `delete_entity`

**Model tier:** sonnet.

**Files:**

- Create: `packages/core-rust/crates/anima-memory/src/memory_manager/edit.rs`, `.../memory_manager/edit_tests.rs`
- Modify: `.../memory_manager.rs` (add `mod edit;`, `#[cfg(test)] mod edit_tests;`, and `pub use self::edit::{EntityDeletion, MemoryDeletion, MemoryPatch};`), `.../memory_manager/types.rs` (two `MemoryError` variants and their messages), `.../lib.rs` (re-export the three types)

`edit.rs` is a child module of `memory_manager`, so it reads the manager's private fields and the private helpers (`build_index_text`, `entity_key`, `unique_strings`, `validate_importance`) through `super::`. No other crate file changes; no dependency changes.

**Interfaces (exact):**

```text
// memory_manager/types.rs
pub enum MemoryError { ..., InvalidMemoryContent, EntityOwnsMemories }
//   message(): InvalidMemoryContent => "content must not be empty"
//              EntityOwnsMemories   => "entity still owns memories"

// memory_manager/edit.rs
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryPatch {
    pub content: Option<String>,            // trimmed; empty after trim => InvalidMemoryContent
    pub importance: Option<f64>,            // validated like add(): InvalidImportance
    pub tags: Option<Option<Vec<String>>>,  // None keep; Some(None) clear; Some(Some(v)) replace (unique_strings; empty vec => cleared)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryDeletion {
    pub removed_relationship_ids: Vec<String>,           // agent relationships that cited it and were left with none
    pub updated_relationship_ids: Vec<String>,           // agent relationships that cited it and keep other evidence
    pub updated_fact_ids: Vec<String>,                   // temporal facts that cited it (kept, citation removed)
    pub updated_temporal_relationship_ids: Vec<String>,  // temporal relationships that cited it (kept, citation removed)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityDeletion {
    pub removed_relationship_ids: Vec<String>,           // agent relationships with the entity as source or target
    pub removed_temporal_relationship_ids: Vec<String>,  // temporal relationships likewise
    pub removed_fact_ids: Vec<String>,                   // temporal facts with the entity as subject or object
}

impl MemoryManager {
    /// Ok(None): no such memory. Validation happens before anything changes.
    pub fn update_memory(&mut self, id: &str, patch: MemoryPatch) -> Result<Option<Memory>, MemoryError>;
    /// None: no such memory.
    pub fn delete_memory(&mut self, id: &str) -> Option<MemoryDeletion>;
    /// Ok(None): no such entity. Err(EntityOwnsMemories): an Agent-kind entity whose id is the agent_id of any memory.
    pub fn delete_entity(&mut self, kind: RelationshipEndpointKind, id: &str) -> Result<Option<EntityDeletion>, MemoryError>;
}
```

**Rules and edge cases:**

- `update_memory`: validate the whole patch first (an invalid field changes nothing). Replace `content`, `importance`, `tags` in place; keep `id`, `created_at`, `agent_*`, `memory_type`, `scope`, room, world, and session. Then re-index with `self.index.add_document(id, build_index_text(&memory))` (`add_document` replaces an existing id). An empty patch returns the memory unchanged (the route turns that into a 400, not the crate). The text index is rebuilt for tag-only changes too (tags are part of the indexed text).
- `delete_memory`: remove the memory and its index document. Then clean citations **by targeted loops, not by `prune_relationship_evidence`** (that helper removes every evidence-less relationship, including ones that never cited anything): for each agent relationship whose `evidence_memory_ids` contains the id, remove the id; if the list is now empty, remove the relationship (the same rule retention uses) and report it in `removed_relationship_ids`, else report it in `updated_relationship_ids`. For each temporal fact and temporal relationship whose evidence contains the id, remove the id and report it; **they are kept even when their evidence becomes empty** (a fact the owner stated stays until the owner removes it on the About you tab; the controller ruled this in Risks, see the end). Do not touch `supersedes_*` lists. Do not remove entities.
- `delete_entity`: look the entity up by `entity_key(kind, id)` (an unknown key is `Ok(None)`). If `kind == Agent` and any memory has `agent_id == id`, return `Err(EntityOwnsMemories)` and change nothing (loading would recreate the entity from its memories). Otherwise remove, in this order, every agent relationship whose source or target is `(kind, id)`; every temporal relationship likewise; every temporal fact whose subject is `(kind, id)` or whose object kind and id are; then the entity. Facts and relationships are removed, not edited, because loading recreates an entity from anything that names it. Other entities that those records named stay. Also drop the removed facts' ids from other facts' `supersedes_fact_ids` and the removed temporal relationships' ids from others' `supersedes_relationship_ids` (as `forget_temporal_fact` does).
- Ids in the three result lists are sorted ascending (deterministic tests).

**Tests** (`memory_manager/edit_tests.rs`, using the helpers pattern of `tests.rs`; copy the few builder helpers instead of exposing them):

- `update_memory_reindexes_the_text_index`: old content's distinctive word no longer finds the memory by `search`; the new one does; the id is unchanged.
- `update_memory_keeps_identity_fields`: `id`, `created_at`, `agent_id`, `agent_name`, `memory_type`, `scope`, `room_id`, `session_id` equal their pre-update values.
- `update_memory_changes_importance_and_tags`: importance replaced; `Some(Some(vec))` replaces tags (duplicates collapsed); `Some(None)` clears; `None` keeps; a tag-only change makes the new tag searchable.
- `update_memory_rejects_invalid_input_without_changing_anything`: importance `1.5` and `NaN` give `InvalidImportance`; content `"   "` gives `InvalidMemoryContent`; the stored memory is byte-for-byte unchanged after each, including when content is valid but importance is not.
- `update_memory_for_an_unknown_id_is_none`.
- `an_empty_patch_returns_the_memory_unchanged`.
- `delete_memory_removes_the_memory_and_its_index_entry`: `get` is `None`, `search` finds nothing, `size()` decreases.
- `delete_memory_removes_citations_from_facts_and_relationships`: seed two memories A and B, a relationship citing `[A, B]`, a relationship citing `[A]`, a fact citing `[A, B]`, a fact citing `[A]`, a temporal relationship citing `[A]`; delete A; assert the first relationship now cites `[B]` (listed under updated), the second is gone (listed under removed), the facts cite `[B]` and `[]`, the temporal relationship cites `[]` and still exists, and the three lists name exactly those ids.
- `delete_memory_keeps_relationships_that_never_cited_it`: a relationship with no evidence and one citing only B are untouched after deleting A.
- `delete_memory_for_an_unknown_id_is_none`.
- `delete_entity_removes_relationships_at_both_ends`: an agent relationship and a temporal relationship, each with the entity once as source and once as target, are gone; a relationship between two other entities stays; the other endpoints' entities stay.
- `delete_entity_removes_facts_with_it_as_subject_or_object`: two facts removed, an unrelated fact stays, and a fact that listed a removed fact in `supersedes_fact_ids` no longer does.
- `delete_entity_refuses_while_it_owns_memories`: an Agent-kind entity that is some memory's `agent_id` returns `EntityOwnsMemories` and nothing changed (counts equal); after `delete_memory` of all its memories, the same call succeeds.
- `delete_entity_survives_a_snapshot_round_trip`: delete a `User` entity that had a relationship and a fact, `replace_snapshot(manager.snapshot())` into a fresh manager, and assert the entity is still absent from `list_entities`.
- `delete_entity_for_an_unknown_key_is_none`, and `an_entity_is_matched_by_kind_and_id`: a `User` entity and an `Agent` entity with the same id are deleted independently.

**Steps:**

- [ ] **Step 1:** Run the precondition command (Global Constraints). Write the failing tests (they need only the three signatures and the two variants to compile). Run `CARGO_INCREMENTAL=0 cargo test -p anima-memory --lib -- edit_tests 2>&1 | tail -30`. Expected: compile errors, then (with stubs) FAIL.
- [ ] **Step 2:** Implement `edit.rs`, the two variants, the re-exports. Run the same command. Expected: PASS. Run `CARGO_INCREMENTAL=0 cargo test -p anima-memory 2>&1 | tail -15`. Expected: PASS (the existing suite and doc tests are unchanged; the crate still compiles with `--features locomo-eval` untouched, so also run `CARGO_INCREMENTAL=0 cargo build -p anima-memory --features locomo-eval 2>&1 | tail -5`).
- [ ] **Step 3:** `grep -n "InvalidTemporalValidityRange" -r packages/core-rust hosts --include=*.rs` and confirm no `match` over `MemoryError` elsewhere became non-exhaustive (the build above covers `anima-memory`; `CARGO_INCREMENTAL=0 cargo check -p anima-daemon 2>&1 | tail -10` covers the daemon).
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage the files above by path; commit:

```bash
git commit -m "feat(memory): edit a memory, delete one with its citations, and delete an entity

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Daemon: owner routes to edit and delete a memory, with embedding sync

**Model tier:** opus (concurrency and persistence under the memory lock).

**Files:**

- Create: `hosts/rust-daemon/src/memory_text.rs`, `hosts/rust-daemon/src/routes/memory_edits.rs`, `hosts/rust-daemon/src/routes/contracts/memory_edits.rs`, `hosts/rust-daemon/src/routes/tests/memory_edits.rs`
- Modify: `hosts/rust-daemon/src/lib.rs` (`mod memory_text;`), `routes/mod.rs` (`mod memory_edits;`, the two routes, `ApiDoc` paths, the test module line `mod memory_edits;`), `routes/contracts/mod.rs` (module and re-exports), `routes/memories.rs` (make `persist_memory_store` and `remove_memory_embeddings` `pub(super)`; nothing else here in this task)

**Interfaces (exact):**

```text
// memory_text.rs
pub(crate) const MAX_MEMORY_EDIT_CHARS: usize = 8_000;
pub(crate) const MAX_MEMORY_TAGS: usize = 20;
pub(crate) const MAX_MEMORY_TAG_CHARS: usize = 40;
pub(crate) const MAX_FACT_VALUE_CHARS: usize = 500;
pub(crate) const MEMORY_TEXT_HIDDEN / MEMORY_CONTENT_INVALID / MEMORY_TAGS_INVALID / FACT_VALUE_INVALID: &str  // exact text in Global Constraints
pub(crate) fn has_hidden_text(text: &str) -> bool;   // text.chars().any(crate::skills::is_smuggling_character)
/// trim each tag; drop empty; unique (first wins); Err(MEMORY_TAGS_INVALID) when > 20 remain or any is > 40 chars (chars, not bytes) or has_hidden_text.
pub(crate) fn clean_tags(tags: Vec<String>) -> Result<Vec<String>, &'static str>;

// routes/contracts/memory_edits.rs  (serde camelCase, ToSchema)
pub(crate) struct MemoryPatchRequest {           // #[serde(deny_unknown_fields)]
    content: Option<String>,
    importance: Option<f64>,
    #[serde(default, deserialize_with = "double_option")] tags: Option<Option<Vec<String>>>,  // absent / null / array
}
pub(crate) struct MemoryDeleteResponse { id: String, removed_relationships: usize, updated_relationships: usize, updated_facts: usize }

// routes/memory_edits.rs
pub(super) async fn patch_memory(State(AppState), Path(memory_id): Path<String>, request: Request) -> Response;
pub(super) async fn delete_memory(State(AppState), Path(memory_id): Path<String>, request: Request) -> Response;
/// Runs `work` in its own task and awaits it. A dropped caller does not cancel it; a panicked or cancelled task is a 503 MEMORY_TASK_FAILED.
async fn locked<T: Send + 'static>(work: impl Future<Output = Result<T, ApiError>> + Send + 'static) -> Result<T, ApiError>;
```

The `double_option` deserializer is the standard five-line helper (`Option<Option<T>>` where a present `null` becomes `Some(None)`); put it beside the request type.

**Behavior rules:**

- `PATCH`: owner `authorize(…, false)`; body through `skill_body`-style limited read (`state.config.max_request_bytes`, plus `parse_json_body`); at least one field present else 400 `MEMORY_PATCH_EMPTY`; `content` trimmed, 1 to `MAX_MEMORY_EDIT_CHARS` characters else 400 `MEMORY_CONTENT_INVALID`; `has_hidden_text` on content else 400 `MEMORY_TEXT_HIDDEN`; `importance` finite and in `0..=1` else 400 with the crate's `InvalidImportance` message; tags through `clean_tags` (an empty cleaned array clears). All validation runs before the task starts.
- The task body (inside `locked`): clone the handles under the state read lock, release it; `memory_handle.write().await`; `MemoryMutation::new`; `update_memory` (`Ok(None)` is 404, an `Err` is 400 with `error.message()`); `persist_memory_store(...)` (a failure is 503 with the existing `failed to persist memory: …` text, and `MemoryMutation`'s drop restores the previous memory and index); then, **still holding the memory write lock**, sync the embedding: when `content` changed, `embeddings.write().await.upsert_memory(&memory)`; on its error call `remove_memories(&[id])` (a stale vector must not outlive its text) and `warn!` (the response stays 200: the edit is saved and recall falls back to the text index). When only importance or tags changed, the embedding is left alone (it embeds content only).
- `DELETE`: owner `authorize(…, false)`; task body: `delete_memory` (`None` is 404), persist (503 restores), then `remove_memories(&[id])` under the same lock (a failure only warns); answer 200 `MemoryDeleteResponse` with the counts from `MemoryDeletion`.
- All answers pass through `no_store`; errors use `ApiError` through `skills::rejected`-style helpers (`routes::sessions::rejected` is `pub(super)`: use it as `routes/skills.rs` does).
- A write that returns 503 after a durability-uncertain save keeps the new state (existing `MemoryMutation` semantics); do not add handling for it.

**Tricky piece: `locked`** (complete):

```rust
async fn locked<T: Send + 'static>(
    work: impl std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
) -> Result<T, ApiError> {
    match tokio::spawn(work).await {
        Ok(result) => result,
        Err(_) => Err(ApiError::service_unavailable(MEMORY_TASK_FAILED.to_string())),
    }
}
```

Everything the body needs is cloned out of `AppState` before `locked(async move { … })` (the handles are `Arc`s; the parsed patch is owned).

**Tests** (`routes/tests/memory_edits.rs`; a `request(method, uri, origin, body)` helper and `OWNER_ORIGIN = "http://localhost:4200"` as in `routes/tests/skills.rs`; a foreign origin such as `http://evil.example` is the non-owner):

- `the_memory_edit_routes_refuse_a_non_owner`: PATCH and DELETE with a foreign origin answer 403 with `no-store`, and the memory is unchanged.
- `patch_changes_content_importance_and_tags_and_answers_the_memory`: 200; body is a `MemoryResponse` with the new values and the old `id`, `createdAt`, `agentId`; a following `GET /api/memories/search?q=<new word>` finds it and `q=<old word>` does not.
- `patch_validates_each_field`: importance `1.5`; content `""`; content of 8,001 characters; 21 tags; a 41-character tag; content containing U+E0041 (tag character) answers `MEMORY_TEXT_HIDDEN`; each answers 400 with the exact constant or the crate message and changes nothing. Content of exactly 8,000 characters is accepted.
- `patch_with_no_fields_is_a_400`: body `{}` answers `MEMORY_PATCH_EMPTY`; an unknown field answers 400.
- `patch_tags_null_clears_and_absent_keeps`.
- `patch_unknown_memory_is_404`.
- `patch_re_embeds_changed_content_only`: through the embeddings handle (`state.read().await.memory_embeddings_handle()`, `MemoryVectorIndex::search`), after a content PATCH the new text's top hit is the memory; after an importance-only PATCH the vector is the same object of work (assert `vectorCount` unchanged and the top hit unchanged). Skip the assertions that need a real vector when the test embeddings are disabled: construct the state with `MemoryEmbeddingRuntime::local_default()`.
- `a_failed_embedding_removes_the_stale_vector`: cover with a unit test of the sync helper (extract it as `sync_embedding(&mut runtime, &memory, content_changed)` taking a closure for the failing case) if the runtime offers no failure injection; otherwise state in the test why it is a unit-level check.
- `delete_removes_the_memory_its_embedding_and_citations`: seed two memories, a relationship citing both through `POST /api/memories/relationships`; DELETE one; 200 with `updatedRelationships: 1`; `GET /api/memories/recent` no longer lists it; the embedding index no longer returns it; `GET /api/memories/relationships` shows evidence `[other]`.
- `delete_unknown_memory_is_404`.
- `a_failed_save_restores_the_memory_and_answers_503`: a `MemoryStoreConfig` that cannot be written (see the existing failing-store tests in `memory_store.rs` for the fixture; if none exists, point the store at a path whose parent is a regular file); PATCH answers 503 with the `failed to persist memory` prefix, and `get` returns the original content and the original search hit; DELETE answers 503 and the memory is still listed.
- `a_dropped_patch_still_finishes`: hold the memory **read** lock, poll the PATCH future once (`futures::poll!`; `futures` is already a dependency), drop it, release the lock, then `memory.write().await` (tokio's lock is FIFO, so it queues behind the spawned task) and assert the new content is stored. No sleeps; use `tokio::task::yield_now` in a bounded loop only until the spawned task is queued, and fail with a message if it never is.
- `memory_text_helpers`: `has_hidden_text` flags U+E0041, U+202E, U+2066, U+E0100 and not U+200B or plain text; `clean_tags` trims, drops empties, deduplicates, and enforces both limits by characters (a 40-character non-ASCII tag passes, 41 fails). (Unit tests inside `memory_text.rs`.)

**Steps:**

- [ ] **Step 1:** Run the precondition command. Write `memory_text.rs` with its unit tests first and run `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- memory_text 2>&1 | tail -20`. Expected: PASS.
- [ ] **Step 2:** Write the route tests (compile-failing), then the contracts, handlers, `locked`, routes, `ApiDoc` entries. Register the routes with the static `/api/memories/...` routes: `.route("/api/memories/{memory_id}", patch(memory_edits::patch_memory).delete(memory_edits::delete_memory))`. Run `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- memory_edits 2>&1 | tail -30`. Expected: PASS. Then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test memory_api 2>&1 | tail -15` (the existing route tests still pass; the trace route still resolves).
- [ ] **Step 3:** `grep -n "memory_edits" hosts/rust-daemon/src/routes/mod.rs | head` shows the module, both routes, and both `ApiDoc` paths (a utoipa path missing from `ApiDoc` is the common slip; the OpenAPI test added in Task 3 asserts it).
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage the files above by path; commit:

```bash
git commit -m "feat(daemon): let the owner edit and delete a memory

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Daemon: facts routes and entity deletion, OpenAPI, README

**Model tier:** sonnet.

**Files:**

- Modify: `hosts/rust-daemon/src/routes/memory_edits.rs`, `routes/contracts/memory_edits.rs`, `routes/contracts/mod.rs`, `routes/mod.rs` (four routes, `ApiDoc` paths), `routes/tests/memory_edits.rs`, `hosts/rust-daemon/README.md`

No crate change: facts are replaced through the existing `add_temporal_fact` (its `supersedes_fact_ids` marks the old fact `Superseded` and closes its `valid_to`), and listed through `list_temporal_facts`.

**Interfaces (exact):**

```text
// contracts/memory_edits.rs
pub(crate) struct MemoryFactResponse { ...the MemoryFact JSON in Global Constraints; kinds and status as the lowercase strings of as_str() }
pub(crate) struct MemoryFactsEnvelope { facts: Vec<MemoryFactResponse> }
pub(crate) struct FactPatchRequest { value: String }                      // deny_unknown_fields
pub(crate) struct FactReplacedResponse { fact: MemoryFactResponse, superseded: MemoryFactResponse }
pub(crate) struct FactDeleteResponse { id: String }
pub(crate) struct EntityDeleteResponse { kind: String, id: String, removed_relationships: usize, removed_facts: usize }
impl From<&TemporalFact> for MemoryFactResponse

// routes/memory_edits.rs
pub(super) async fn list_facts(State(AppState), request: Request) -> Response;
pub(super) async fn patch_fact(State(AppState), Path(fact_id): Path<String>, request: Request) -> Response;
pub(super) async fn delete_fact(State(AppState), Path(fact_id): Path<String>, request: Request) -> Response;
pub(super) async fn delete_entity(State(AppState), Path(entity_id): Path<String>, request: Request) -> Response;
const DEFAULT_FACTS_LIMIT: usize = 100;  const MAX_FACTS_LIMIT: usize = 500;  const FACT_REPLACEMENT_CONFIDENCE: f64 = 1.0;
```

Routes: `.route("/api/memories/facts", get(memory_edits::list_facts))`, `.route("/api/memories/facts/{fact_id}", patch(memory_edits::patch_fact).delete(memory_edits::delete_fact))`, `.route("/api/memories/entities/{entity_id}", axum::routing::delete(memory_edits::delete_entity))` beside the existing `/api/memories/entities` route.

**Behavior rules:**

- `GET facts`: owner `authorize(…, true)`. Query via `request_query` (`agentId`, `subject`, `includeInactive` as `true|false`, `limit`). `limit` absent is 100; present must be a whole number 1 to 500 else 400 `FACTS_LIMIT_INVALID`; `includeInactive` other than `true`/`false` is 400 `"includeInactive must be true or false"` (one more named constant in this file). Read under the memory **read** lock: call `list_temporal_facts(TemporalFactOptions { include_inactive, limit: Some(usize::MAX), ..Default })` (the crate's own limit would cut before the filters below), then filter, then truncate to `limit`. The list is already newest first.
  - `agentId`: keep a fact only when at least one id in `evidence_memory_ids` is a memory whose `agent_id` equals it (`manager.get(id)`); a fact with no evidence is shown only when `agentId` is absent. (Facts carry no agent of their own; their evidence says whose memories they came from.)
  - `subject`: keep a fact whose `subject_id` equals it, or whose `subject_name` equals it ignoring ASCII case.
- `PATCH fact`: owner `authorize(…, false)`; `value` trimmed, 1 to `MAX_FACT_VALUE_CHARS` characters else 400 `FACT_VALUE_INVALID`; `has_hidden_text` else 400 `MEMORY_TEXT_HIDDEN`. Task body in `locked` (as Task 2): `get_temporal_fact` (`None` is 404); refuse with 409 `FACT_NOT_EDITABLE` unless `status == Active` and `value.is_some()`; then `add_temporal_fact(NewTemporalFact { ..old's subject, predicate, tags, room/world/session, evidence_memory_ids, object fields None, value: Some(new), valid_from: None, valid_to: None, observed_at: None (now), confidence: FACT_REPLACEMENT_CONFIDENCE, supersedes_fact_ids: vec![old.id], status: None })`; persist (503 restores through `MemoryMutation`); answer 200 `{ fact, superseded }` where `superseded` is the old fact re-read after the change (status `superseded`, `validTo` set). Same value as the old one still replaces it (the owner confirmed it), so no equality shortcut.
- `DELETE fact`: owner `authorize(…, false)`; `locked` body: `get_temporal_fact` (`None` is 404); `forget_temporal_fact`; persist; 200 `{ id }`.
- `DELETE entity`: owner `authorize(…, false)`; `kind` query is required and parsed with `RelationshipEndpointKind::from_str` (in `memory_manager/types.rs`; the contracts' private `parse_relationship_endpoint_kind` wraps it) else 400 `ENTITY_KIND_REQUIRED`; `locked` body: `delete_entity(kind, &id)` (`Ok(None)` is 404, `Err(EntityOwnsMemories)` is 409 `ENTITY_OWNS_MEMORIES`), persist, 200 `EntityDeleteResponse` with `removedRelationships` = agent plus temporal counts and `removedFacts`. The path id is percent-decoded by axum; ids with `:` work.
- Deleting facts and entities removes no embeddings (only memories are embedded).

**Tests** (same file and helpers as Task 2; seed through the real routes and the manager where easier):

- `the_fact_and_entity_routes_refuse_a_non_owner`: all four routes answer 403 `no-store`; nothing changed. (`GET facts` too: the read needs owner read authorization.)
- `facts_list_newest_first_and_hide_inactive_unless_asked`: three facts, one superseded; default shows two, `includeInactive=true` shows three; order newest first.
- `facts_filter_by_agent_through_their_evidence`: facts citing memories of two different agents; `agentId=` keeps only the matching agent's; a fact without evidence is absent under `agentId` and present without it.
- `facts_filter_by_subject_id_or_name`.
- `facts_limit_defaults_and_is_bounded`: 150 facts return 100 by default; `limit=500` returns all 150; `limit=0`, `limit=501`, `limit=x` answer 400 `FACTS_LIMIT_INVALID`; `includeInactive=maybe` answers 400.
- `replacing_a_fact_supersedes_it_with_the_new_value`: 200; `fact.value` is the new value, `status` active, `supersedesFactIds` is `[old]`, `confidence` 1.0, subject, predicate, evidence, and tags kept; `superseded.status` is `superseded` and its `validTo` is set; a later `GET facts` lists only the new one by default.
- `replacing_refuses_inactive_and_valueless_facts`: an already superseded fact and a fact with an object and no value answer 409 `FACT_NOT_EDITABLE`; unchanged.
- `replacing_validates_the_value`: empty, 501 characters, and U+202E answer 400 with `FACT_VALUE_INVALID` / `MEMORY_TEXT_HIDDEN`; unknown id is 404.
- `deleting_a_fact_removes_it_and_the_links_to_it`: 200 `{ id }`; a fact that superseded it no longer lists it; unknown id is 404.
- `deleting_an_entity_removes_its_relationships_and_answers_counts`: seed a `user` entity with one agent relationship and two facts; 200 with `removedRelationships: 1`, `removedFacts: 2`; the entity no longer appears in `GET /api/memories/entities`; other entities stay.
- `deleting_an_entity_needs_its_kind_and_refuses_one_that_owns_memories`: no `kind` or `kind=robot` answers 400 `ENTITY_KIND_REQUIRED`; the companion's `agent` entity with a memory answers 409 `ENTITY_OWNS_MEMORIES`; an unknown id is 404; the same id under another kind is a different entity.
- `the_new_memory_routes_are_in_the_openapi_document`: `ApiDoc::openapi()` has path items for all six routes of this milestone (`/api/memories/{memory_id}`, `/api/memories/facts`, `/api/memories/facts/{fact_id}`, `/api/memories/entities/{entity_id}`) with the expected methods. (Use whatever the existing OpenAPI test in `routes/mod.rs` does to read the document.)
- `a_failed_save_restores_a_fact_replacement`: the failing store from Task 2; the old fact is still active and no new fact exists; 503.

**README:** `hosts/rust-daemon/README.md` gains a **Memory editing (owner)** section with a table row per route above (method, path, auth, request, answers, errors), the note that `entities/{id}` takes `kind`, the note that facts carry no agent so `agentId` filters by evidence, and the "Rolling back" note: M7 changes no snapshot; the memory store format is unchanged.

**Steps:**

- [ ] **Step 1:** Write the tests; confirm they fail (`CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- memory_edits 2>&1 | tail -30`).
- [ ] **Step 2:** Implement contracts, handlers, routes, `ApiDoc`. Run the same command, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test memory_api 2>&1 | tail -15`. Expected: PASS.
- [ ] **Step 3:** Write the README section. `grep -n "Memory editing" hosts/rust-daemon/README.md` shows it.
- [ ] **Step 4:** `cargo fmt --all`; `git diff --stat`; stage by path (the README included); commit:

```bash
git commit -m "feat(daemon): list, replace, and forget facts and delete entities as the owner

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Daemon: hidden-Unicode refusal in `memory_add` and `POST /api/memories`

**Model tier:** sonnet.

**Files:**

- Modify: `hosts/rust-daemon/src/tools/memory.rs`, `hosts/rust-daemon/src/routes/memories.rs` (`handle_create_memory` only), `hosts/rust-daemon/src/tools/tests.rs` (or the file holding the existing `memory_add` tests; grep `execute_memory_add` in tests), `hosts/rust-daemon/tests/memory_api.rs`

**Interfaces and rules:**

- The `memory_add` tool: after the existing non-empty `content` check, `if memory_text::has_hidden_text(&content) { return TaskResult::error(MEMORY_TEXT_HIDDEN, 0); }`. Nothing is stored (return before the memory lock). The tool's other behavior and messages are unchanged.
- `POST /api/memories` (`handle_create_memory`): after `into_domain`, `has_hidden_text` on `content` and on every tag answers 400 `MEMORY_TEXT_HIDDEN` via `ApiError::bad_request_static`. A deliberate change to an existing route's validation; plain text, including other invisible format characters, is still accepted. `POST /api/memories/evaluated` is **not** changed (the evaluator derives its text from the user's message, and its duplicate handling is its own contract); the Task 9 notes record this.
- The constant `MEMORY_TEXT_HIDDEN` is the one from `memory_text.rs`; do not redefine it.
- Why only these: they are where model-written or model-saved text enters memory. The owner's own PATCH routes were done in Tasks 2 and 3.

**Tests:**

- `memory_add_refuses_hidden_text_and_stores_nothing` (tool test): content containing U+E0041 returns the error result with the exact constant; `memory.size()` unchanged; the same content with U+200B (zero-width space) is stored (the daemon reveals it later; it does not refuse it).
- `create_memory_refuses_hidden_text_in_content_and_tags` (integration, `tests/memory_api.rs`): 400 with the constant for a content with U+202E and for a tag with U+E0041; nothing stored (`GET /api/memories/recent` empty).
- `create_memory_still_accepts_plain_and_zero_width_text`: 201 for both.
- Keep the existing `memory_api.rs` tests passing unchanged.

**Steps:**

- [ ] **Step 1:** Write the tests and watch them fail. `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- memory_add 2>&1 | tail -20`, `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --test memory_api 2>&1 | tail -20`.
- [ ] **Step 2:** Implement; both pass. Run `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- tools 2>&1 | tail -15` (the tools suite still passes).
- [ ] **Step 3:** `cargo fmt --all`; `git diff --stat`; stage by path; commit:

```bash
git commit -m "fix(daemon): refuse hidden characters in memories the companion writes

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: SDK: memory edit, facts, and entity calls

**Model tier:** sonnet.

**Files:**

- Modify: `packages/sdk/src/memories.ts`, `packages/sdk/src/index.ts`
- Create: `packages/sdk/src/memories.spec.ts` (model it on `skills.spec.ts` for how a `DaemonClient` is faked)

**Interfaces (exact; additions to `memories.ts`, exported from `index.ts` with the existing memory exports):**

```ts
export const MAX_MEMORY_EDIT_CHARS = 8_000;
export const MAX_MEMORY_TAGS = 20;
export const MAX_MEMORY_TAG_CHARS = 40;
export const MAX_FACT_VALUE_CHARS = 500;
export const MAX_FACTS_SHOWN = 500;

export interface MemoryPatch {
  content?: string;
  importance?: number;
  /** `null` clears the tags; absent keeps them. */
  tags?: string[] | null;
}

export interface MemoryDeleteResult {
  id: string;
  removedRelationships: number;
  updatedRelationships: number;
  updatedFacts: number;
}

export type MemoryFactStatus = 'active' | 'superseded' | 'retracted';

/** A temporal fact. Untrusted text: the companion wrote it, so show it as text. */
export interface MemoryFact {
  id: string;
  subjectKind: RelationshipEndpointKind;
  subjectId: string;
  subjectName: string;
  predicate: string;
  objectKind: RelationshipEndpointKind | null;
  objectId: string | null;
  objectName: string | null;
  value: string | null;
  validFrom: number | null;
  validTo: number | null;
  observedAt: number;
  confidence: number;
  evidenceMemoryIds: string[];
  supersedesFactIds: string[];
  status: MemoryFactStatus;
  tags: string[] | null;
  roomId: string | null;
  worldId: string | null;
  sessionId: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface MemoryFactOptions {
  agentId?: string;
  subject?: string;
  includeInactive?: boolean;
  limit?: number;
}

export interface MemoryFactReplaced { fact: MemoryFact; superseded: MemoryFact }
export interface MemoryEntityDeleteResult {
  kind: RelationshipEndpointKind;
  id: string;
  removedRelationships: number;
  removedFacts: number;
}

// MemoriesClient additions
update(memoryId: string, patch: MemoryPatch): Promise<Memory>;                 // PATCH /api/memories/{id}
delete(memoryId: string): Promise<MemoryDeleteResult>;                         // DELETE /api/memories/{id}
facts(options?: MemoryFactOptions): Promise<MemoryFact[]>;                     // GET  /api/memories/facts?...  (only set params are sent)
replaceFact(factId: string, value: string): Promise<MemoryFactReplaced>;       // PATCH /api/memories/facts/{id} { value }
deleteFact(factId: string): Promise<{ id: string }>;                           // DELETE /api/memories/facts/{id}
deleteEntity(kind: RelationshipEndpointKind, entityId: string): Promise<MemoryEntityDeleteResult>;
                                                                               // DELETE /api/memories/entities/{id}?kind=...
```

Rules: ids are `encodeURIComponent`-encoded in paths; `facts` sends `includeInactive=true` only when true and never sends undefined params; errors surface as the client's existing `DaemonHttpError` (nothing new). The SDK does not validate lengths; the constants are for callers. `Memory` stays the existing type from `@animaOS-SWARM/memory`; its `type` field is the memory type, as the daemon's `MemoryResponse` already serializes it.

**Tests** (`memories.spec.ts`): `update sends a PATCH with only the given fields`; `update can clear tags with null`; `delete encodes the id and returns the counts`; `facts sends only the options that are set`; `facts includes includeInactive only when true`; `replaceFact patches the value`; `deleteFact deletes by id`; `deleteEntity sends the kind and encodes ids with colons`; `a daemon refusal reaches the caller as DaemonHttpError`; `the limits match the daemon` (the five constants equal 8000, 20, 40, 500, 500).

**Steps:**

- [ ] **Step 1:** Write the spec; run `bun x nx test @animaOS-SWARM/sdk 2>&1 | tail -20`. Expected: FAIL.
- [ ] **Step 2:** Implement; run it again. Expected: PASS. Run `bun x nx run @animaOS-SWARM/sdk:typecheck 2>&1 | tail -10` if that target exists (`bun x nx show projects --json` and `bun x nx show project @animaOS-SWARM/sdk` name it).
- [ ] **Step 3:** `bun x nx format:write --files=packages/sdk/src/memories.ts,packages/sdk/src/memories.spec.ts,packages/sdk/src/index.ts`; re-run the SDK tests; then `bun x nx run @animaOS-SWARM/sdk:build 2>&1 | tail -10`. Expected: build succeeds.
- [ ] **Step 4:** Stage the three files by path; commit:

```bash
git commit -m "feat(sdk): add memory edit, fact, and entity calls

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Web data: memory helpers, `RevealedText`, the facade, and `useMemory`

**Model tier:** sonnet.

**Files:**

- Create: `apps/web/src/lib/memory.ts`, `lib/memory.test.ts`, `components/memory/RevealedText.tsx`, `components/memory/RevealedText.test.tsx`, `hooks/useMemory.ts`, `hooks/useMemory.test.tsx`, `test/memory.ts`
- Modify: `apps/web/src/lib/daemon-api.ts`, `lib/daemon-api.test.ts`

All new daemon calls go through the SDK (`setupClient.memories`). Preconditions: Task 5's SDK build is done (`bun x nx run @animaOS-SWARM/sdk:build`).

**Interfaces (exact):**

```ts
// lib/daemon-api.ts: additions to the `daemon` object (reads take no signal: the SDK memory calls do not; the hook ignores stale answers by sequence number)
recentMemories: (agentId: string, limit: number) => setupClient.memories.recent({ agentId, limit }),
searchMemories: (query: string, agentId: string, limit: number) => setupClient.memories.search(query, { agentId, limit }),
traceMemory: (id: string) => setupClient.memories.trace(id),
updateMemory: (id: string, patch: MemoryPatch) => setupClient.memories.update(id, patch),
deleteMemory: (id: string) => setupClient.memories.delete(id),
saveMemory: (input: CreateMemoryInput) => setupClient.memories.create(input),
listFacts: (options: MemoryFactOptions) => setupClient.memories.facts(options),
replaceFact: (id: string, value: string) => setupClient.memories.replaceFact(id, value),
deleteFact: (id: string) => setupClient.memories.deleteFact(id),
listMemoryEntities: (limit: number) => setupClient.memories.entities({ limit }),
listMemoryRelationships: (agentId: string, limit: number) => setupClient.memories.relationships({ agentId, limit }),
deleteMemoryEntity: (kind: RelationshipEndpointKind, id: string) => setupClient.memories.deleteEntity(kind, id),

// lib/memory.ts
export const MEMORY_PAGE_LIMIT = 200;
export const MEMORY_TYPES = ['fact', 'observation', 'task_result', 'reflection'] as const;
export const MEMORY_TYPE_LABELS: Record<MemoryType, string> = { fact: 'Fact', observation: 'Observation', task_result: 'Task result', reflection: 'Reflection' };
export type MemorySort = 'relevance' | 'newest' | 'oldest' | 'importance';   // 'relevance' keeps the daemon's order and is offered only while a search is active
export const MEMORY_SORT_LABELS: Record<MemorySort, string> = { relevance: 'Most relevant', newest: 'Newest', oldest: 'Oldest', importance: 'Most important' };
export type MemoryEntry = Memory & { score?: number };          // a search hit carries its score
export function filterByType(list: readonly MemoryEntry[], type: MemoryType | 'all'): MemoryEntry[];
export function sortMemories(list: readonly MemoryEntry[], sort: MemorySort): MemoryEntry[];   // stable; ties newest first; 'relevance' returns the list as given
export function parseTagInput(text: string): string[];            // split on commas, trim, drop empties, unique
export function formatTags(tags: readonly string[] | null | undefined): string;   // 'a, b'
export function importanceLabel(importance: number): 'High' | 'Medium' | 'Low';  // >= 0.7, >= 0.4, else
export function humanizePredicate(predicate: string): string;     // 'communication_preference' -> 'communication preference'
export function describeFact(fact: MemoryFact): { label: string; value: string };   // value, else objectName, else ''
export function groupFacts(facts: readonly MemoryFact[]): { preferences: MemoryFact[]; about: MemoryFact[] };  // predicate contains 'preference' (any case)
export function stripInvisible(text: string): string;             // removes the characters revealInvisible marks (\p{Cf}, U+2028, U+2029)
export function memoryErrorMessage(error: unknown): { message: string; status: number | null };
//   DaemonHttpError: its own message (the daemon's constants are owner-readable), status kept; anything else: COMPANION_UNREACHABLE (lib/approvals), status null
// owner-facing strings, each a named export tested once
export const MEMORY_EMPTY = 'Nothing is remembered yet. Your companion saves what matters as you talk.';
export const MEMORY_SEARCH_EMPTY = 'No memories match that search.';
export const FACTS_EMPTY = 'Nothing about you is saved yet. Tell your companion what matters to you.';
export const ENTITIES_EMPTY = 'No people or things yet.';
export const MEMORY_GONE = 'That memory is already gone.';
export const FACT_GONE = 'That fact is already gone.';
export const ENTITY_GONE = 'That is already gone.';
export const DELETE_MEMORY_PROMPT = 'Delete this memory? Your companion will forget it. This can’t be undone.';
export const DELETE_FACT_PROMPT = 'Forget this? Your companion will stop using it.';
export const DELETE_ENTITY_PROMPT = 'Remove this from People & things? Its connections go with it.';
export const FACT_EDIT_HINT = 'Saving replaces this fact. The old value stays as history.';
export const REPLACED_LABEL = 'Replaced';
```

```tsx
// components/memory/RevealedText.tsx
/** Model-written text, as text: hidden characters are shown as ⟨U+XXXX⟩ markers and a note counts them (spec §14). Never markup. */
export function RevealedText({
  text,
  className,
}: {
  text: string;
  className?: string;
}): JSX.Element;
//   renders <span className={className}>{revealed.text}</span>, then, when count > 0, <small className="memory-hidden-note">{invisibleNote(count)}</small>; both from lib/skills' revealInvisible / invisibleNote

// hooks/useMemory.ts
export interface MemoryOptions {
  agentId: string | null;
  epoch: number;
  enabled: boolean;
}
export interface MemoryView {
  memories: MemoryEntry[];
  facts: MemoryFact[];
  entities: MemoryEntity[];
  relationships: AgentRelationship[];
  loaded: boolean;
  error: string | null;
  errorStatus: number | null;
  query: string;
  includeReplaced: boolean;
  /** Reads the list again with this search ('' is the recent list). */
  search: (query: string) => void;
  setIncludeReplaced: (value: boolean) => void;
  refresh: () => void;
  /** Each answers true when the daemon took it, after the lists were read again. */
  edit: (memory: Memory, patch: MemoryPatch) => Promise<boolean>;
  remove: (memory: Memory) => Promise<boolean>;
  trace: (memory: Memory) => Promise<MemoryEvidenceTrace | null>;
  replaceFact: (fact: MemoryFact, value: string) => Promise<boolean>;
  removeFact: (fact: MemoryFact) => Promise<boolean>;
  removeEntity: (entity: MemoryEntity) => Promise<boolean>;
}
export function useMemory(options: MemoryOptions): MemoryView;
```

```ts
// test/memory.ts: fixtures
memoryFixture(id: string, overrides?: Partial<Memory>): Memory         // type 'fact', importance 0.5, agentId 'agent-main'
factFixture(id: string, overrides?: Partial<MemoryFact>): MemoryFact   // active, value 'Prefers short answers', predicate 'communication_preference'
entityFixture(id: string, overrides?: Partial<MemoryEntity>): MemoryEntity
relationshipFixture(id: string, overrides?: Partial<AgentRelationship>): AgentRelationship
```

**Hook rules:**

- Reads (`recentMemories` or `searchMemories` by `query`, `listFacts({ agentId, includeInactive: includeReplaced, limit: MAX_FACTS_SHOWN })`, `listMemoryEntities(MEMORY_PAGE_LIMIT)`, `listMemoryRelationships(agentId, MEMORY_PAGE_LIMIT)`) run together with `Promise.allSettled` when `enabled && agentId`, on mount, when `epoch` changes, when `query` or `includeReplaced` changes, and after every action. Each list keeps its previous value when its own read failed; the first failure's message sets `error` (via `memoryErrorMessage`). A read answered after a newer one started is ignored (a sequence counter in a ref), and unmounting ignores late answers.
- **Empty reload bail-out (the M5 I4 lesson):** a list that was empty and is empty again keeps its previous array, so a harness that goes online with nothing remembered does not re-render.
- Actions: a failed action sets `error` and `errorStatus` (cleared at the next action) and returns false; 404 and 409 reload the lists. `edit`/`remove`/`replaceFact`/`removeFact`/`removeEntity` reload on success. `trace` returns `null` on failure (and sets `error`) and does not reload.
- A 404 answers with the exact `MEMORY_GONE` / `FACT_GONE` / `ENTITY_GONE` text instead of the daemon's generic one.
- `removeEntity` passes the entity's `kind` and `id` to `daemon.deleteMemoryEntity`.

**Tests:**

- `lib/memory.test.ts`: `filters by type and keeps the order`; `sorts newest, oldest, and by importance with newest breaking ties, and keeps relevance order as given`; `parses and formats tags`; `labels importance at the boundaries 0.7 and 0.4`; `humanizes predicates`; `describes a fact by its value, else its object`; `groups preference facts apart from the rest`; `strips the characters that are revealed`; `maps a daemon refusal to its message and a network failure to the unreachable text`; `owner-facing strings` (one assertion per exported constant).
- `RevealedText.test.tsx`: `reveals invisible characters in text` (a ZWSP shows `⟨U+200B⟩` and the note "This text contains 1 invisible character"); `renders plain text without a note`; `never renders markup` (`<b>x</b>` appears as literal text and no `b` element exists).
- `useMemory.test.tsx` (mock `daemon`): `loads the recent memories, facts, entities, and relationships for the companion`; `search reads the matching memories and an empty search returns to the recent list`; `includeReplaced reads the facts again with includeInactive`; `a stale read is ignored`; `a failed list keeps the others and sets the error`; `an empty reload keeps the empty arrays` (referential equality); `edit sends the patch and reloads`; `a 404 on remove says the memory is gone and reloads`; `remove, replaceFact, removeFact, and removeEntity call the daemon and reload`; `trace returns the evidence or null`; `nothing is read while offline or without an agent`.
- `daemon-api.test.ts`: one test that each new method calls the matching SDK method with the arguments above (follow the file's existing pattern).

**Steps:**

- [ ] **Step 1:** Write the tests; `cd apps/web && bun x vitest run src/lib/memory.test.ts src/components/memory src/hooks/useMemory.test.tsx src/lib/daemon-api.test.ts 2>&1 | tail -30`. Expected: FAIL.
- [ ] **Step 2:** Implement; run again. Expected: PASS, with no `act()` warnings or console output.
- [ ] **Step 3:** `bun x nx format:write --files=<each changed file>`; re-run; `bun x nx typecheck @animaOS-SWARM/web 2>&1 | tail -10` (confirm the target name with `bun x nx show project @animaOS-SWARM/web`).
- [ ] **Step 4:** Stage the files by path; commit:

```bash
git commit -m "feat(web): add the memory data layer and revealed text

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Web Memory page: Memories, About you, People & things, and the destination

**Model tier:** sonnet.

**Files:**

- Create: `apps/web/src/pages/MemoryPage.tsx`, `pages/MemoryPage.test.tsx`, `components/memory/MemoryList.tsx`, `components/memory/FactList.tsx`, `components/memory/EntityList.tsx`, `memory.css`
- Modify: `apps/web/src/styles.css` (import `memory.css`; it must pass `visual-tokens.test.ts`: use the existing palette variables only), `components/WorkspaceShell.tsx` and `components/WorkspaceShell.test.tsx`, `ViewHarness.tsx` (wiring only), `ViewHarness.test.tsx`

**Interfaces (exact):**

```tsx
// pages/MemoryPage.tsx
export interface MemoryPageProps {
  agentId: string;          // the companion
  online: boolean;
  /** LiveState.epoch: a snapshot or resync reads again. */
  epoch: number;
}
export function MemoryPage(props: MemoryPageProps): JSX.Element;   // calls useMemory itself

// components/WorkspaceShell.tsx
AVAILABLE_PAGES gains 'memory' (after 'automations');
PRIMARY_DESTINATIONS gains { page: 'memory', label: 'Memory', icon: <ChipIcon size={16} /> } between Automations and Skills;
WorkspaceShell gains the prop `memory?: ReactNode | null` and renders it at page === 'memory'.
// ViewHarness.tsx: <WorkspaceShell memory={<MemoryPage agentId={…} online={…} epoch={live.state.epoch} />} …/>  plus one import.
```

The ⌘K list and the mobile dock pick the destination up from `DESTINATIONS` (Go to Memory), as for the other pages.

**Page behavior (spec §15.4):**

- A `role="tablist"` with three tabs, **Memories**, **About you**, **People & things** (`aria-selected`, buttons; the selected tab is remembered in `sessionStorage` under `anima.memory.tab` inside try/catch and the page renders without it). Header text: "What your companion remembers". While `!online`, show `COMPANION_UNREACHABLE` and no lists. While `!loaded`, show "Loading memory…". A read error shows `error` in a `role="alert"` with a Refresh button.
- **Memories tab.** A search form (an input labelled "Search memories" and a Search button; submitting calls `view.search(text.trim())`; submitting empty returns to the recent list; a "Clear" button appears while a search is active); a type filter as buttons (All, Fact, Observation, Task result, Reflection; `aria-pressed`); a sort `select` labelled "Sort memories" (Most relevant is offered and selected by default only while a search is active; otherwise Newest). Each memory is a list item showing: its type label, its content through `RevealedText`, importance label ("High importance"), tags as small text chips each through `RevealedText`, and the date (`formatWhen` from `lib/approvals`). Actions per item: **Edit**, **Delete**, **Where it was used** (a disclosure button, `aria-expanded`). Empty states: `MEMORY_EMPTY`, or `MEMORY_SEARCH_EMPTY` while a search is active.
  - **Edit**: an inline form with a textarea (label "Memory text", counted against `MAX_MEMORY_EDIT_CHARS`: Save is disabled and the count turns into "n / 8,000" in an alert when over), an importance range 0 to 1 step 0.05 with its value shown, a tags input (comma separated, label "Tags") using `parseTagInput`, **Save** and **Cancel**. Save calls `view.edit(memory, patch)` with only the fields that changed (content trimmed; `tags: null` when the tag box is emptied, an array otherwise; none changed: Save is disabled). On success the form closes; on failure it stays open with `view.error` shown. When the memory text contains invisible characters the form shows their note and a "Remove invisible characters" button that applies `stripInvisible` to the textarea.
  - **Delete**: shows `DELETE_MEMORY_PROMPT` with **Delete memory** and **Keep**; Escape or Keep cancels; confirming calls `view.remove`. A 404 shows `MEMORY_GONE` (the hook already says so).
  - **Where it was used**: calls `view.trace(memory)` once per expansion; shows "Relationships" (each: `relationshipType`, summary, and the two names, through `RevealedText`) and "Facts" (the loaded facts whose `evidenceMemoryIds` include the id, each described with `describeFact`); "Nothing else cites this memory." when both are empty; "Couldn’t load the trail." on a failed trace.
- **About you tab.** Facts grouped under **Preferences** and **Things you’ve told me** (`groupFacts`); each shows the humanized predicate as its label and the value through `RevealedText`, plus the date. A checkbox "Show replaced facts" toggles `setIncludeReplaced`; a replaced (non-active) fact is labelled `REPLACED_LABEL` and has no actions. Active facts with a value have **Edit** (an inline textarea labelled "Fact", the `FACT_EDIT_HINT` text, **Save** disabled when unchanged, empty, or over 500 characters; calls `view.replaceFact`) and **Forget** (`DELETE_FACT_PROMPT`, **Forget** / **Keep**, calls `view.removeFact`). Facts with an object and no value (no Edit) show the object name and only Forget. Empty: `FACTS_EMPTY`.
- **People & things tab.** Entities (name, kind label "Person"/"Companion"/"System"/"Thing" for user/agent/system/external, aliases, summary, all through `RevealedText`), each with **Remove** (`DELETE_ENTITY_PROMPT`; **Remove** / **Keep**; calls `view.removeEntity`; on failure the daemon's message, such as the "still has memories" refusal, appears beside the entity). Relationships listed below as "A → B · type" with the summary (through `RevealedText`). Empty: `ENTITIES_EMPTY`.
- Model-written strings (content, tags, fact values, entity names and summaries, relationship text) render **only** through `RevealedText`. No `dangerouslySetInnerHTML`, no `MarkdownMessage`. After each successful action, move focus to the list heading so keyboard users are not dropped.
- Styles: `memory.css` with `.memory-*` classes only; reuse the `studio-*` buttons and panel classes the Skills and Automations pages use; mobile friendly (single column under 640px).

**Tests** (`MemoryPage.test.tsx`; mock `daemon` as the Skills page tests do; fixtures from `test/memory.ts`):

- `shows the empty state when nothing is remembered`.
- `lists memories with type, importance, tags, and date`.
- `filters by type and sorts by importance`.
- `searching shows matches and an empty search returns to the recent list` (submit via Enter and via the button; `MEMORY_SEARCH_EMPTY` for no hits).
- `editing saves only the changed fields and closes` (content change sends `{ content }`; emptying tags sends `{ tags: null }`; importance change sends `{ importance }`).
- `editing cannot save unchanged or oversized text` (8,001 characters disables Save and shows the count alert).
- `cancelling an edit discards it`.
- `a failed save keeps the form open with the daemon's message` (400 `Memory text must not contain invisible tag or direction-override characters`).
- `deleting asks first, Keep cancels, and confirming deletes`.
- `a 404 on delete says the memory is gone`.
- `shows hidden characters as markers` (a ZWSP in content, a tag, a fact value, an entity name, and a relationship summary each show `⟨U+200B⟩` and the note) and `the editor can remove invisible characters`.
- `renders model text as text, never markup` (`<img src=x onerror=alert(1)>` as content yields no `img` element).
- `the evidence trail shows relationships and the facts that cite the memory`, `an uncited memory says so`, `a failed trail says it could not load`.
- `About you groups preferences apart and edits a fact by replacing it` (`replaceFact` called with the trimmed value; the hint text is shown); `forgetting a fact asks first`; `replaced facts are labelled and read-only after "Show replaced facts"` (that toggle calls `listFacts` with `includeInactive: true`); `a fact with an object and no value cannot be edited`.
- `People & things lists entities and relationships and removes an entity after asking`; `an entity that still has memories shows the daemon's refusal`.
- `offline shows the unreachable text and no lists`; `the selected tab survives a remount through session storage` and `still renders when storage throws`.
- `WorkspaceShell.test.tsx` additions: `the sidebar offers Memory between Automations and Skills`, `Memory opens the memory page and the command menu offers Go to Memory`. `ViewHarness.test.tsx`: `#/memory shows the Memory page for the companion` (and the existing harness tests stay quiet).

**Steps:**

- [ ] **Step 1:** Write the tests; `cd apps/web && bun x vitest run src/pages/MemoryPage.test.tsx src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx 2>&1 | tail -30`. Expected: FAIL.
- [ ] **Step 2:** Implement the page, components, CSS, shell destination, and harness wiring. Run the same command, then `bun x vitest run src/visual-tokens.test.ts 2>&1 | tail -10` (find its path with `find src -name "visual-tokens.test.ts"`). Expected: PASS and silent.
- [ ] **Step 3:** `grep -rn "dangerouslySetInnerHTML\|MarkdownMessage\|innerHTML" apps/web/src/pages/MemoryPage.tsx apps/web/src/components/memory` prints nothing; `git diff --stat -- apps/web/src/ViewHarness.tsx` shows a handful of added lines.
- [ ] **Step 4:** `bun x nx format:write --files=<each changed file>`; re-run the tests; `bun x nx typecheck @animaOS-SWARM/web 2>&1 | tail -10`.
- [ ] **Step 5:** Stage by path; commit:

```bash
git commit -m "feat(web): add the Memory page with memories, facts, and people and things

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Web "Save to memory" on chat messages

**Model tier:** sonnet.

**Files:**

- Create: `apps/web/src/hooks/useSaveToMemory.ts`, `hooks/useSaveToMemory.test.tsx`, `components/memory/SaveToMemory.tsx`, `components/memory/SaveToMemory.test.tsx`
- Modify: `apps/web/src/lib/transcript.ts` (`TranscriptActions`), `hooks/useTranscriptActions.ts` and its test, `components/sessions/SessionView.tsx` (create the hook, pass it in the actions), `components/ChatScreen.tsx` (`Bubble` gets the action) and the ChatScreen tests

**Interfaces (exact):**

```ts
// hooks/useSaveToMemory.ts
export const SAVE_TO_MEMORY_LABEL = 'Save to memory';
export const SAVED_TO_MEMORY_LABEL = '✓ Saved to memory';
export const SAVING_LABEL = 'Saving…';
export const SAVE_FAILED = 'Couldn’t save that. Try again.';
export const SAVE_SHORTENED_NOTE = 'Saved the first 8,000 characters';
export const SAVED_FROM_CHAT_TAG = 'saved-from-chat';
export const SAVED_IMPORTANCE = 0.75;                 // spec §10

export type SaveOutcome =
  | { kind: 'saved'; shortened: boolean }
  | { kind: 'failed'; message: string };

export interface SaveToMemoryTarget { agentId: string; agentName: string; sessionId: string }
/** Saves a chat message as a Fact (spec §10). One save per message: a saved message answers 'saved' again without calling the daemon; a save in flight is shared. */
export function useSaveToMemory(target: SaveToMemoryTarget | null): {
  save: (message: ChatMessage) => Promise<SaveOutcome>;
  savedState: (messageId: string) => { shortened: boolean } | null;   // what the button shows after a remount
};

// lib/transcript.ts: TranscriptActions gains
onSaveToMemory?: (message: ChatMessage) => Promise<SaveOutcome>;
savedToMemory?: (messageId: string) => { shortened: boolean } | null;

// components/memory/SaveToMemory.tsx
export function SaveToMemory({ message, save, saved }: {
  message: ChatMessage;
  save: (message: ChatMessage) => Promise<SaveOutcome>;
  saved: { shortened: boolean } | null;
}): JSX.Element;      // a button next to Copy; states idle | saving | saved | failed
```

**Rules:**

- The request is `daemon.saveMemory({ agentId, agentName, type: 'fact', content, importance: 0.75, tags: ['saved-from-chat'], sessionId })` (`POST /api/memories`, spec §10). `content` is the message text trimmed; when longer than `MAX_MEMORY_EDIT_CHARS` it is cut to that length (so the memory can still be edited later) and the outcome says `shortened: true`. An empty text is never offered (no button).
- A `DaemonHttpError` answers `{ kind: 'failed', message: error.message }` (the daemon's hidden-character refusal reads well as is); anything else answers `SAVE_FAILED`. A failed message can be tried again.
- The button shows `SAVE_TO_MEMORY_LABEL`, then `SAVING_LABEL` (disabled), then `SAVED_TO_MEMORY_LABEL` (disabled; with the `SAVE_SHORTENED_NOTE` text beside it when shortened), or the failure message in a `role="status"` span with the button enabled. State lives in the component for the click, and in the hook's map so a remounted bubble (a re-render of the list) still shows "Saved". It is offered on **User and Assistant** bubbles only (not tool or system pills), and only when `actions.onSaveToMemory` is provided, so `ChatScreen` standalone tests and Telegram read-only sessions behave as before (saving is allowed in every session kind; it writes memory, not the session).
- `Bubble` stays memoized: the callbacks passed down are stable (the hook returns `useCallback`-stable functions keyed on `target`), and the saved map is a ref, so saving one message does not re-render the others (the existing `ChatScreen.memo.test.tsx` must pass unchanged, plus one new case below).
- `SessionView` passes `{ agentId: agent.id, agentName: agent.name, sessionId: session.id }` (the viewed session's agent, so a helper's session saves to that helper) and `null` while the session record is unknown.
- The text is shown nowhere new; the saved content is the message's own text, and the daemon refuses hidden-character text (Task 4).

**Tests:**

- `useSaveToMemory.test.tsx`: `sends the message as a Fact with importance 0.75, the saved-from-chat tag, and the session id`; `a saved message answers saved again without calling the daemon`; `two clicks in flight call the daemon once`; `a long message is cut to 8,000 characters and says so`; `a daemon refusal answers its message and can be tried again`; `an unreachable daemon answers the generic failure`; `saved state survives a new render of the hook` (via `savedState`); `nothing is sent without a target`.
- `SaveToMemory.test.tsx`: `shows Save to memory, then Saving…, then ✓ Saved to memory`; `shows the shortened note`; `shows the failure and lets the owner try again`; `announces the result politely` (`role="status"`).
- ChatScreen / SessionView tests: `offers Save to memory on user and assistant messages only when the action is provided`; `does not offer it on tool or system messages`; `saving one message does not re-render the other bubbles` (render-count probe as in `ChatScreen.memo.test.tsx`); `useTranscriptActions passes the save action and the saved lookup`.

**Steps:**

- [ ] **Step 1:** Write the tests; `cd apps/web && bun x vitest run src/hooks/useSaveToMemory.test.tsx src/components/memory src/components/ChatScreen.test.tsx src/components/ChatScreen.memo.test.tsx src/components/sessions/SessionView.test.tsx src/hooks/useTranscriptActions.test.tsx 2>&1 | tail -30`. Expected: FAIL.
- [ ] **Step 2:** Implement; run the same command, then the full web suite once: `bun x vitest run 2>&1 | tail -15`. Expected: PASS and silent.
- [ ] **Step 3:** `bun x nx format:write --files=<each changed file>`; re-run; `bun x nx typecheck @animaOS-SWARM/web 2>&1 | tail -10`.
- [ ] **Step 4:** Stage by path; commit:

```bash
git commit -m "feat(web): save a chat message to memory

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 9: M7 verification

**Controller only.**

**Files:**

- Modify: `docs/superpowers/plans/2026-09-23-companion-console.md` (the M7 status row)

- [ ] **Step 1: Check the contracts**

Run: `grep -rn "const MAX_MEMORY_EDIT_CHARS\|const MAX_MEMORY_TAGS\|const MAX_MEMORY_TAG_CHARS\|const MAX_FACT_VALUE_CHARS\|const DEFAULT_FACTS_LIMIT\|const MAX_FACTS_LIMIT\|const FACT_REPLACEMENT_CONFIDENCE" hosts/rust-daemon/src`
Expected: each constant defined once (the first four in `memory_text.rs`, the rest in `routes/memory_edits.rs`).

Run: `grep -n '"/api/memories/{memory_id}"\|"/api/memories/facts"\|"/api/memories/facts/{fact_id}"\|"/api/memories/entities/{entity_id}"' hosts/rust-daemon/src/routes/mod.rs`
Expected: each path in the router and in `ApiDoc` (via the handlers' `#[utoipa::path]`).

Run: `grep -n "Memory editing" hosts/rust-daemon/README.md`
Expected: the new section.

Run: `grep -n "CONTROL_PLANE_STORE_VERSION: u32 = 9" hosts/rust-daemon/src/control_plane_store.rs; git diff d5cefa2 --stat -- hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/state.rs`
Expected: the version is still 9 and neither file changed.

Run: `grep -rn "allow(dead_code)\|allow(unused_imports)" hosts/rust-daemon/src/memory_text.rs hosts/rust-daemon/src/routes/memory_edits.rs packages/core-rust/crates/anima-memory/src/memory_manager/edit.rs`
Expected: no output.

Run: `grep -rn "dangerouslySetInnerHTML\|MarkdownMessage\|innerHTML" apps/web/src/pages/MemoryPage.tsx apps/web/src/components/memory`
Expected: no output (model text renders as text).

Run: `grep -rLn "RevealedText" apps/web/src/components/memory/MemoryList.tsx apps/web/src/components/memory/FactList.tsx apps/web/src/components/memory/EntityList.tsx`
Expected: no output (each of the three renders model text through `RevealedText`).

Run: `git diff d5cefa2 --stat -- Cargo.lock hosts/rust-daemon/Cargo.toml packages/core-rust/crates/anima-memory/Cargo.toml packages/sdk/package.json apps/web/package.json bun.lock`
Expected: no output (no new dependencies or features).

- [ ] **Step 2: Run the milestone gate**

Run: `df -h .`

- With at least 12 GB available: `bun x nx run rust-daemon:test --skipNxCache` (it also runs `core-rust:test`). Expected: PASS (M6 ended at 1,874 passed; M7 adds about 60 Rust tests).
- Otherwise run the fallback in the shared `target/` (no new `CARGO_TARGET_DIR`): `CARGO_INCREMENTAL=0 cargo test -p anima-memory`, `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib`, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --tests`. Expected: PASS. The fallback does not satisfy AGENTS.md's completion rule; record that the Nx gate is pending disk space. On Windows, if a running daemon locks `target/debug/anima-daemon.exe`, use AGENTS.md's `CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon` rerun only with the owner's go-ahead.

Run: `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`
Expected: every target succeeds (M6 ended with the web at about 850 tests; M7 adds about 70 web and 10 SDK tests).

Run: `cargo fmt --all --check && bun x nx format:check --base=origin/main`
Expected: both succeed.

- [ ] **Step 3: Update the master plan status**

In `docs/superpowers/plans/2026-09-23-companion-console.md`, replace the M7 row (match it by content; the table is padded)

```markdown
| M7 Memory | (written before M7) | pending |
```

with the following only if every gate command passed (fill in the Nx test count and the head commit):

```markdown
| M7 Memory | `2026-09-23-companion-console-m7.md` | done (Nx rust-daemon:test <count> passed; sdk + web test, typecheck, build green at <sha>) |
```

If the Rust gate ran only through the fallback, use `implemented — Nx gate pending (disk)`. Also adjust the master plan's M7 task lines to what shipped (T7.2's routes are in `routes/memory_edits.rs`, not `routes/memories.rs`; the entity route takes a `kind` query). Then run `bun x nx format:write --files=docs/superpowers/plans/2026-09-23-companion-console.md` (it realigns the table). The controller commits this file:

```bash
git add docs/superpowers/plans/2026-09-23-companion-console.md
git commit -m "docs: mark the M7 memory milestone complete"
```

Recommended implementer tier: the controller runs this task.

---

## Notes for the controller

**Task shape against the master plan.** T7.1 → Task 1 (the three methods, in `memory_manager/edit.rs`). T7.2 → Tasks 2, 3, and 4 (routes in a new `routes/memory_edits.rs` rather than `routes/memories.rs`, which keeps the existing routes and their authorization; the owner routes share `memory_text.rs`). T7.3 → Tasks 5 (SDK), 6 to 8 (web data, page, Save to memory). Task 9 is the gate. Order is strictly 1 → 9. Tasks 6 to 8 need only the SDK's types (Task 5) and could run before the daemon tasks, but stay sequential for the SDK build.

**Spec vs. code decisions.**

- **No snapshot bump.** The memory manager has its own store; M7 adds no field to it and nothing to the control plane, so there is no `.bak`, no migration, no tool grant, and no event. The Memory page reads on open, on a stream snapshot or resync (`epoch`), and after its own changes.
- **Spec §10's `delete_entity` and `update_memory` names are used as written.** `delete_memory` is a third method the spec's "forgets the memory, removes its id from the evidence of facts and relationships" needs, because the existing `forget` leaves citations behind and is kept as is.
- **Citation cleanup removes relationships left with no evidence** (as retention does) but **keeps facts and temporal relationships left with none** (see Risks 1).
- **`kind` on the entity route.** Entities are keyed by kind and id; the spec's `DELETE /api/memories/entities/{id}` is ambiguous across kinds, so a required `kind` query parameter is added.
- **Facts carry no agent.** `GET /api/memories/facts?agentId=` therefore filters by the agents of the memories that evidence the fact; facts with no evidence appear only without `agentId`. `subject` matches the subject id, or the subject name ignoring ASCII case.
- **Fact edits supersede.** `PATCH /api/memories/facts/{id}` creates a new active fact (confidence 1.0, same subject, predicate, evidence, and tags) that supersedes the old one through the crate's existing `supersedes_fact_ids`, so history is kept; only active facts with a value can be edited.
- **Embedding sync.** Content edits re-embed under the memory write lock; a failed re-embed removes the old vector (a stale vector is worse than none) and the answer stays 200; deletes remove the vector. Importance and tag edits do not re-embed (the embedding is of the content only).
- **Save to memory** uses the existing `POST /api/memories` exactly as spec §10 says (Fact, importance 0.75, `saved-from-chat`, session id), which has no owner authorization today (spec §10: existing routes keep theirs). The web cuts an over-long message to 8,000 characters so the memory stays editable.
- **Web.** The page owns its hook (`useMemory`), unlike Automations, because nothing else needs its lists; `ViewHarness.tsx` gains one element and one import. Search is submitted, not debounced. Memory text, tags, fact values, entity names, and relationship text render only through `RevealedText`.

**Deferred.** Adding a memory by hand; bulk delete; editing entities or relationships; guarding the evaluator's extractions against hidden characters (its text comes from the user's message, not the model); a memory event on the stream; a Health card for memory (M8); the Playwright memory flow (M10, T10.2); `GET /api/memories/facts` paging beyond 500.

## Risks (top 5 for the controller to rule on)

1. **Facts and temporal relationships left without evidence after a memory is deleted.** The plan keeps them (they show under About you, where the owner can forget them) and removes only agent relationships left with no evidence (retention's rule). The alternative is to forget a fact when its last evidence memory is deleted, which matches "I deleted the memory because it was wrong" but silently removes owner-visible facts. Recommendation: keep, as planned. Note the evaluator will re-extract the same fact if the user repeats it.
2. **Entity deletion semantics.** The store rebuilds entities from memories, relationships, and facts on load, so `delete_entity` also removes facts that name the entity and refuses (409) an agent entity that still owns memories, which includes the companion itself. Deleting the `user` entity is soft in practice: the evaluator recreates it on the next message that names the user. Rule whether the refusal and the fact removal are acceptable, or whether the page should hide Remove for the companion's own entity.
3. **Authorization is split.** Spec §10 keeps the existing memory routes' authorization, so reads (`recent`, `search`, `entities`, `relationships`, `trace`) and the writes `POST /api/memories`, `/entities`, `/relationships`, `/evaluated`, and `/retention` stay as they were (open to whatever protects the daemon today), while the new edit, delete, and facts routes require the owner. A non-owner can still add memories or run retention through the old routes. Rule: accept per the spec, or add owner authorization to `/retention` (the only old route that deletes memories) as a one-line follow-up. Related: hidden-character refusal covers `memory_add` and `POST /api/memories` but not `/evaluated` or the evaluator's extractions.
4. **The page can be stale while the companion writes memories.** There is no memory event (spec §6 lists none); the page reads on open, on stream resync, and after its own actions, and has a Refresh button. Rule whether to add `memory.updated` (a spec change touching the event reducer) or accept the staleness; the plan accepts it.
5. **Cost and safety under the memory lock.** Each edit snapshots the whole manager (`MemoryMutation::new` clones every record) and re-embeds under the write lock, so a very large store makes edits slow and blocks recall meanwhile; and the drop-safety test relies on tokio's FIFO lock ordering rather than a hook. Both are acceptable for a personal companion's store; the Postgres memory-store path stays untested here (no database available), so hand-check that the saved snapshot shape needs no change (it does not: the same fields are stored).

## Controller rulings on the risks (binding)

1. **Facts and temporal relationships with no evidence left:** they are kept when a memory is deleted; only agent relationships left empty are removed. Deleting the owner's facts as a side effect would lose data they never asked to delete. The owner can remove a fact directly.
2. **Entity deletion:**
   - It removes the facts that name the entity.
   - It refuses with 409 for an agent entity that still owns memories, including the companion itself.
   - The `kind` query parameter is an accepted addition to the spec's path.
   - Deleting the user entity is soft, because the evaluator recreates it. The page says so in one line next to the action.
3. **Authorization split:** the older memory routes stay as the spec has them, and only the new edit and delete routes are owner-only. Hidden-character refusal covers `memory_add` and `POST /api/memories`. Recorded as a post-M7 follow-up: decide owner auth for the older memory routes, and refusal for evaluator extractions.
4. **No memory event on the stream:** accepted. The page reads on open, on resync and after its own actions, and has a Refresh button. A `memory.updated` event is a follow-up.
5. **Edits snapshot and re-embed under the memory write lock:**
   - Accepted.
   - The drop-safety test relies on tokio's FIFO lock order rather than on timing, which is acceptable.
   - The Postgres memory-store path stays `#[ignore]`; hand-check its SQL.
