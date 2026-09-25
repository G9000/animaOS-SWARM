# Companion Console M2: Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every conversation room of an agent a session with a durable record, mirror every committed message and terminal run into a row-based history store fed by an outbox, expose owner-authorized session routes (list, get, messages, create, rename/archive/mark read, delete, export) plus `GET /api/agents?view=summary`, keep the control-plane snapshot bounded to a hot tail, and give the web console a sessions sidebar, hash routing, and a session view that works on today's blocking `POST /api/agents/{id}/run`.

**Architecture:** A new `sessions` module owns the `SessionRecord` registry (in the control-plane snapshot, bumped to version 5 behind a pre-upgrade backup), room-to-session mapping, titles, capabilities, the legacy migration, per-response views, and hot-tail pruning. A new `history` module owns the `HistoryStore` trait with SQLite (WAL + FTS5), Postgres (new migration), and bounded in-memory implementations, and a `HistoryService` outbox that a `HistoryWorker` flushes about once a second, idempotently by id, with backoff and a readiness issue after five minutes of failures. The run coordinator ensures a session record in the run-start save, advances it at commit, enqueues the committed messages only after the final save succeeds, links delegated/helper/peer runs to their parent run, and runs workspace check-ins in stable `schedule:<id>` rooms. The SDK gains a `SessionsClient`; the web shell becomes route-driven (`#/s/<id>`, `#/work`, …) with a sessions sidebar, and the chat becomes a `SessionView` whose messages come from the session messages route.

**Tech Stack:** Rust 2021 (tokio, axum 0.8, serde, rusqlite 0.32 bundled with FTS5, sqlx 0.8 Postgres, sha2, base64 0.22, chrono, utoipa 5), TypeScript (React 19, Vite, Tailwind v4, Vitest, Testing Library), Nx with Bun.

**Spec:** `docs/superpowers/specs/2026-09-23-companion-console-design.md` (§3 sessions, §13 persistence/migration/compatibility, §15.1–§15.2 and §15.5 web, §16 limits, §17 tests). Master plan: `docs/superpowers/plans/2026-09-23-companion-console.md` (M2, T2.1–T2.8). M1 plan and ledger for carry-forwards: `docs/superpowers/plans/2026-09-23-companion-console-m1.md`, `.superpowers/sdd/2026-09-23-companion-console-m1/progress.md`.

## Global Constraints

- Master plan Global Constraints apply. **No new dependencies**: rusqlite (bundled, FTS5 compiled in), sqlx, sha2, base64, chrono, uuid, and async-trait are already in `hosts/rust-daemon/Cargo.toml`; the SDK and web add no packages. `anima-core` gains only one dependency-free method (Task 1).
- **Precondition: M1 is complete.** Before Task 1, run `grep -n "RESERVED_ROOM_PREFIXES" hosts/rust-daemon/src/routes/agents.rs && grep -n "max_runs_per_agent" hosts/rust-daemon/src/app.rs && grep -n "with_todo_baseline" hosts/rust-daemon/src/tools.rs hosts/rust-daemon/src/agent_runs.rs`. Expected: matches in every file. If `with_todo_baseline` is missing, stop and report that M1 Task 9 has not landed (Task 9 of this plan appends to the builder chain that M1 Task 9 ends).
- Out of scope (M3+, do not build): the async runs route `POST …/sessions/{sid}/runs`, the event stream, streaming, stop/steer/queue, `/compact` and compaction, AI titles, `search_conversations`, approvals, skills, usage records and `usage` totals on sessions, attachments, the Telegram `Stopped`/`Suppressed` states, and the web tool cards (tool/system messages keep today's pills until M3).
- New env var, exact name: `ANIMAOS_RS_HISTORY_SQLITE_FILE` (default `history.sqlite` beside `ANIMAOS_RS_CONTROL_PLANE_FILE`).
- Session ids match `^[A-Za-z0-9._:-]{1,200}$`. New chats are `chat:<uuid-v4>`. A legacy room id that fails the pattern maps to `legacy-room:<first 32 hex chars of SHA-256(room id)>` and the record keeps the room (`roomId`); every ledger `sessionId` goes through the same mapping (M1 carry-forward F17).
- The control-plane snapshot version goes from 4 to 5 **once**, in Task 7, together with the pre-upgrade backup (JSON: `<file>.pre-sessions.bak`, fsynced; Postgres: `host_snapshots` row `control_plane.backup.<version>`), and the backup is written for every older version including unversioned JSON files (M1 carry-forward).
- Every new route: reads call `authorize_read` and answer `Cache-Control: no-store` (errors included); mutations call `authorize`; every route has a `#[utoipa::path]` entry registered in `ApiDoc`. The sessions routes reuse `routes::jobs::{authorize, no_store}`.
- History writes happen only after the control-plane save that made the data durable succeeded (no phantom rows). Records stay in the control plane until mirrored; pruning removes only messages the `HistoryService` knows are mirrored.
- Lock discipline: never hold the `DaemonState` lock across a history-store call; the outbox's internal `std::sync::Mutex`es are never held across `.await`; order is control-plane transaction → state lock → outbox mutex.
- Error strings in this plan are exact; tests assert them.
- Existing behavior stays: `POST /api/agents/{id}/run` request/response and reserved prefixes, `GET /api/agents` without `view`, the connector message routes, Telegram idempotency, schedules, jobs, the CLI and TUI. The only deliberate daemon changes are listed in each task's Interfaces block.
- Formatting: `state.rs` and `routes/mod.rs` are not rustfmt-clean. Do not run `cargo fmt` over the workspace; hand-format edits there to match the surrounding code. New Rust files may be formatted with `rustfmt --edition 2021 <file>`.
- Commands. Rust iteration: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- <filter> <filter>` (several filters go after `--`); core: `CARGO_INCREMENTAL=0 cargo test -p anima-core --lib -- <filter>`. SDK: `bun x nx test @animaOS-SWARM/sdk`. Web: from `apps/web`, `bun x vitest run <files>`, or `bun x nx test @animaOS-SWARM/web`. The milestone gate (Task 18) runs `bun x nx run rust-daemon:test --skipNxCache` and `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`.
- Stage files by explicit path only; never `git add -A`, `git add .`, or `git commit -a`. Never stage anything under `docs/` except where a step says so.

## File map

| Area        | Files                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Core        | `packages/core-rust/crates/anima-core/src/runtime/run_delta.rs`, `…/runtime/run_tests.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Sessions    | create `hosts/rust-daemon/src/sessions/{mod.rs,migration.rs,views.rs,pruning.rs,test_support.rs}`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| History     | create `hosts/rust-daemon/src/history/{mod.rs,memory.rs,conformance.rs,sqlite.rs,postgres.rs,outbox.rs}`, `hosts/rust-daemon/migrations/20260923000000_history_store.sql`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| State       | modify `state.rs`, `state/run_commit.rs`; create `state/session_state.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Persistence | modify `control_plane_store.rs`, `app/persistence.rs`, `app.rs`, `lib.rs`, `README.md`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| Runs        | modify `runs/mod.rs`, `runs/ledger.rs`, `agent_runs.rs`, `tools.rs`, `tools/team.rs`, `schedules.rs`, `components/evaluators.rs`, `connectors/mod.rs`, `connectors/runtime.rs`, `jobs.rs`, `jobs/tests.rs`, `connectors/gcalendar/mod.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Routes      | create `routes/sessions.rs`, `routes/contracts/sessions.rs`, `routes/tests/sessions.rs`; modify `routes/mod.rs`, `routes/agents.rs`, `routes/contracts/{mod.rs,agents.rs}`, `routes/health.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| SDK         | create `packages/sdk/src/sessions.ts`, `packages/sdk/src/sessions.spec.ts`; modify `client.ts`, `agents.ts`, `agents.spec.ts`, `index.ts`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Web         | create `apps/web/src/lib/{hash-route.ts,hash-route.test.ts,session-groups.ts,session-groups.test.ts}`, `apps/web/src/test/sessions.ts`, `apps/web/src/hooks/{useCompanionSessions.ts,useCompanionSessions.test.tsx,useSessionMessages.ts,useSessionMessages.test.tsx}`, `apps/web/src/components/sessions/{SessionSidebar.tsx,SessionSidebar.test.tsx,SessionView.tsx,SessionView.test.tsx}`, `apps/web/src/sessions.css`; modify `ViewHarness.tsx`, `ViewHarness.test.tsx`, `components/WorkspaceShell.tsx`, `components/WorkspaceShell.test.tsx`, `components/CompanionShell.test.tsx`, `components/ChatScreen.tsx`, `lib/daemon-api.ts`, `lib/daemon-api.test.ts`, `styles.css`; delete `components/{ActivityView.tsx,CheckinsView.tsx,CheckinsView.test.tsx,TelegramThread.tsx,TelegramThread.test.tsx}`; modify `apps/web-e2e/src/{companion.spec.ts,independent-agents.spec.ts,main-workspace-agent.spec.ts}` |

## Task list

1. Transcript maintenance primitive (anima-core)
2. Session records, ids, kinds, titles, and the registry
3. History store contract and the bounded in-memory store
4. SQLite history store (WAL, FTS5, versioned schema)
5. Postgres history store and migration
6. History outbox, worker, readiness, and wiring
7. Snapshot version 5 with the pre-upgrade backup
8. Legacy migration: check-in relabel, legacy sessions, ledger ids, tool grants
9. Session records at run time and helper linkage
10. Stable check-in rooms and the silent check-in memory skip
11. Session read routes and `GET /api/agents?view=summary`
12. Session mutation routes and Markdown export
13. Hot-tail pruning and `messagePruned`
14. SDK sessions client
15. Web hash routing and session data hooks
16. Sessions sidebar and session view components
17. Route-driven shell and session-based chat
18. M2 verification

---

### Task 1: Transcript maintenance primitive (anima-core)

**Files:**

- Modify: `packages/core-rust/crates/anima-core/src/runtime/run_delta.rs` (one method)
- Modify: `packages/core-rust/crates/anima-core/src/runtime/run_tests.rs` (one test)

**Interfaces:**

- Consumes: private `AgentRuntime.messages: Vec<Message>`.
- Produces: `pub fn AgentRuntime::retain_messages(&mut self, keep: impl FnMut(&Message) -> bool) -> Vec<Message>` — removes the messages `keep` rejects, returns them in transcript order, and leaves counters, events, usage, status, and the last task untouched. Used by session deletion (Task 12) and hot-tail pruning (Task 13).

- [ ] **Step 1: Write the failing test**

Append to `packages/core-rust/crates/anima-core/src/runtime/run_tests.rs`:

```rust
#[test]
fn retain_messages_removes_only_rejected_messages_and_keeps_counters() {
    let mut canonical = canonical();
    run_in(&mut canonical, "room-a", "first");
    run_in(&mut canonical, "room-b", "second");
    let before = canonical.snapshot();

    let removed = canonical.retain_messages(|message| message.room_id != "room-a");

    assert_eq!(removed.len(), 2);
    assert!(removed.iter().all(|message| message.room_id == "room-a"));
    assert_eq!(removed[0].role, MessageRole::User);
    assert_eq!(removed[1].role, MessageRole::Assistant);
    let after = canonical.snapshot();
    assert_eq!(after.messages.len(), 2);
    assert!(after.messages.iter().all(|message| message.room_id == "room-b"));
    assert_eq!(after.message_count, 2);
    assert_eq!(after.event_count, before.event_count);
    assert_eq!(after.events, before.events);
    assert_eq!(after.step_count, before.step_count);
    assert_eq!(after.state.token_usage, before.state.token_usage);
    assert_eq!(after.state.status, before.state.status);
    assert_eq!(after.last_task, before.last_task);
    assert!(
        canonical.retain_messages(|_| true).is_empty(),
        "keeping everything removes nothing"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-core --lib -- runtime::run_tests::retain_messages`
Expected: compile error `no method named retain_messages found for struct AgentRuntime`.

- [ ] **Step 3: Implement**

In `packages/core-rust/crates/anima-core/src/runtime/run_delta.rs`, add inside `impl AgentRuntime { … }`, after `revert_run_delta`:

```rust
    /// Removes the transcript messages `keep` rejects and returns them in
    /// transcript order. Hosts use this to drop messages they keep elsewhere
    /// (for example ones mirrored to a history store) or a deleted room.
    /// Counters, events, usage, status, and the last task are untouched.
    pub fn retain_messages(&mut self, mut keep: impl FnMut(&Message) -> bool) -> Vec<Message> {
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.messages.len());
        for message in self.messages.drain(..) {
            if keep(&message) {
                kept.push(message);
            } else {
                removed.push(message);
            }
        }
        self.messages = kept;
        removed
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-core --lib -- runtime::run_tests`
Expected: PASS (all run tests, including the new one).

- [ ] **Step 5: Commit**

```bash
git add packages/core-rust/crates/anima-core/src/runtime/run_delta.rs packages/core-rust/crates/anima-core/src/runtime/run_tests.rs
git commit -m "feat(core): let hosts drop transcript messages they keep elsewhere"
```

---

### Task 2: Session records, ids, kinds, titles, and the registry

**Files:**

- Create: `hosts/rust-daemon/src/sessions/mod.rs`
- Modify: `hosts/rust-daemon/src/lib.rs` (`mod sessions;`)
- Modify: `hosts/rust-daemon/src/schedules.rs` (`unwrap_checkin_prompt`)
- Modify: `hosts/rust-daemon/src/control_plane_store.rs` (`sessions` field, constructor, one assertion)
- Modify: `hosts/rust-daemon/src/state.rs` (`sessions` field and init; snapshot, validation, restore; two tests)

**Interfaces:**

- Consumes: `crate::runs::RunSource`; `crate::schedules::{is_silent_checkin_reply, wrap_checkin_prompt}` (existing).
- Produces (in `crate::sessions`):
  - constants `MAX_SESSION_ID_BYTES` (200), `MAX_SESSION_TITLE_CHARS` (120), `DERIVED_TITLE_CHARS` (60), `MAX_SESSION_PREVIEW_CHARS` (160), `DEFAULT_CHAT_TITLE` (`"New chat"`), `LEGACY_ROOM_SESSION_PREFIX` (`"legacy-room:"`), `MAX_SESSION_CREATIONS_PER_MINUTE` (60), `SCHEDULE_ROOM_PREFIX` (`"schedule:"`);
  - enums (snake_case JSON, `as_str()`): `SessionKind { Chat, Telegram, Checkin, Job, Helper }` (+ `parse(&str) -> Option<Self>`, `capabilities(self, schedule_exists: bool) -> SessionCapabilities`), `SessionOrigin { Web, Api, Telegram, Schedule, Job, Delegation, Peer }`, `TitleSource { FirstMessage, Generated, Owner, System }`;
  - `SessionCapabilities { send, steer, stop, rename, archive, delete, compact, export }` (all `bool`);
  - `SessionSummary { text, through_message_id, created_at_ms, source_message_count }`, `SessionContextTrimmed { dropped_through_message_id, at_ms }` (camelCase; always `None` in M2);
  - `SessionRecord { id, agent_id, kind, origin, title, title_source, created_at_ms, last_activity_at_ms, last_read_at_ms: Option<u64>, archived, parent_session_id, parent_run_id, parent_agent_id, summary, context_trimmed, room_id: Option<String> }` (camelCase; `roomId` only for mapped legacy rooms), `SessionRecord::new(agent_id: &str, room_id: &str, kind, origin, title: String, title_source, now_ms: u64) -> Self`, `SessionRecord::room_id(&self) -> &str`, `SessionRecord::capabilities(&self, schedule_exists: bool) -> SessionCapabilities`;
  - `is_valid_session_id(&str) -> bool`, `session_id_for_room(&str) -> String`, `new_chat_session_id() -> String`, `schedule_room_id(&str) -> String`, `schedule_id_of_room(&str) -> Option<&str>`, `connector_id_of_room(&str) -> Option<&str>`, `job_id_of_room(&str) -> Option<&str>`, `peer_sender_of_room(&str) -> Option<&str>`;
  - `kind_for_room(room_id: &str, source: Option<RunSource>, helper_agent: bool) -> (SessionKind, SessionOrigin)`;
  - `truncate_chars(&str, usize) -> String`, `derived_title(&str) -> Option<String>`, `clean_owner_title(&str) -> Result<String, &'static str>` (error `"title must be 1 to 120 characters"`), `preview_text(&str) -> Option<String>`, `delegated_task_text(&str) -> &str`, `delegating_agent_id(&str) -> Option<&str>`;
  - `TitleContext<'a> { first_user_text, schedule_prompt, job_title, bot_username, peer_sender_name }` (all `Option<&'a str>`, `Clone + Copy`), `session_title(kind, origin, &TitleContext) -> (String, TitleSource)`;
  - `is_checkin_message(&Message) -> bool`, `is_inbound_message(&Message) -> bool`, `is_owner_web_turn(&Message) -> bool`, `hidden_message_ids<'a>(impl IntoIterator<Item = &'a Message>) -> HashSet<String>`;
  - `SessionRegistry` (`Clone + Default`) with `get`, `get_mut`, `contains`, `insert`, `remove`, `records`, `len`, `snapshot_records(&HashSet<String>) -> Vec<SessionRecord>`, `validate(&[SessionRecord]) -> Result<(), String>`, `restored(Vec<SessionRecord>, &HashSet<String>) -> Self`, `record_commit(agent_id, session_id, &[Message], owner_authored: bool) -> Option<SessionCommitUndo>`, `revert_commit(SessionCommitUndo)`;
  - `SessionCommitUndo` (`Clone + Debug`), `SessionCreateLimiter::try_acquire(&mut self, agent_id: &str, now_ms: u64) -> bool` (`Default + Debug`).
- Produces: `crate::schedules::unwrap_checkin_prompt(&str) -> &str`; `ControlPlaneSnapshot::sessions: Vec<SessionRecord>` (`#[serde(default)]`); `DaemonState::sessions: SessionRegistry` (`pub(crate)`), saved for live agents, validated, and restored (records of missing agents are dropped).

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/sessions/mod.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use anima_core::Content;
    use std::collections::BTreeMap;

    fn message(
        id: &str,
        role: MessageRole,
        text: &str,
        metadata: &[(&str, &str)],
        created_at_ms: u64,
    ) -> Message {
        Message {
            id: id.into(),
            agent_id: "agent-1".into(),
            room_id: "room-a".into(),
            content: Content {
                text: text.into(),
                attachments: None,
                metadata: (!metadata.is_empty()).then(|| {
                    metadata
                        .iter()
                        .map(|(key, value)| (key.to_string(), DataValue::String(value.to_string())))
                        .collect::<BTreeMap<_, _>>()
                }),
            },
            role,
            created_at_ms,
        }
    }

    fn chat_record(agent_id: &str, room_id: &str, now_ms: u64) -> SessionRecord {
        SessionRecord::new(
            agent_id,
            room_id,
            SessionKind::Chat,
            SessionOrigin::Web,
            DEFAULT_CHAT_TITLE.into(),
            TitleSource::FirstMessage,
            now_ms,
        )
    }

    fn empty_context() -> TitleContext<'static> {
        TitleContext {
            first_user_text: None,
            schedule_prompt: None,
            job_title: None,
            bot_username: None,
            peer_sender_name: None,
        }
    }

    #[test]
    fn session_ids_follow_the_pattern_and_invalid_rooms_map_to_stable_legacy_ids() {
        let longest = "a".repeat(MAX_SESSION_ID_BYTES);
        let too_long = "a".repeat(MAX_SESSION_ID_BYTES + 1);
        for valid in [
            "chat:5b1d",
            "direct:agent-1",
            "room-1700-3",
            "telegram:telegram-1",
            "schedule:s_1.2",
            longest.as_str(),
        ] {
            assert!(is_valid_session_id(valid), "{valid}");
            assert_eq!(session_id_for_room(valid), valid);
        }
        for invalid in ["", "has space", "slash/room", "ünïcode", too_long.as_str()] {
            assert!(!is_valid_session_id(invalid), "{invalid}");
            let mapped = session_id_for_room(invalid);
            assert!(mapped.starts_with(LEGACY_ROOM_SESSION_PREFIX), "{mapped}");
            assert_eq!(mapped.len(), LEGACY_ROOM_SESSION_PREFIX.len() + 32);
            assert!(is_valid_session_id(&mapped));
            assert_eq!(session_id_for_room(invalid), mapped, "the mapping is stable");
        }
        assert_ne!(session_id_for_room("room one"), session_id_for_room("room two"));
        let chat = new_chat_session_id();
        let uuid = chat.strip_prefix("chat:").expect("new chats use the chat prefix");
        assert_eq!(uuid::Uuid::parse_str(uuid).unwrap().get_version_num(), 4);
    }

    #[test]
    fn room_prefixes_name_their_owners() {
        assert_eq!(schedule_room_id("schedule-1"), "schedule:schedule-1");
        assert_eq!(schedule_id_of_room("schedule:schedule-1"), Some("schedule-1"));
        assert_eq!(connector_id_of_room("telegram:telegram-1"), Some("telegram-1"));
        assert_eq!(job_id_of_room("job:job-1"), Some("job-1"));
        assert_eq!(peer_sender_of_room("peer:alice:bob"), Some("alice"));
        assert_eq!(peer_sender_of_room("chat:x"), None);
        assert_eq!(
            crate::schedules::unwrap_checkin_prompt(&crate::schedules::wrap_checkin_prompt(
                "  Check goals "
            )),
            "Check goals"
        );
        assert_eq!(crate::schedules::unwrap_checkin_prompt("plain"), "plain");
    }

    #[test]
    fn rooms_map_to_kinds_and_origins() {
        let cases = [
            ("telegram:telegram-1", None, false, SessionKind::Telegram, SessionOrigin::Telegram),
            ("schedule:schedule-1", Some(RunSource::Schedule), false, SessionKind::Checkin, SessionOrigin::Schedule),
            ("job:job-1", Some(RunSource::Job), false, SessionKind::Job, SessionOrigin::Job),
            ("peer:alice:bob", Some(RunSource::Peer), false, SessionKind::Helper, SessionOrigin::Peer),
            ("room-1-1", Some(RunSource::Delegation), false, SessionKind::Helper, SessionOrigin::Delegation),
            ("room-1-2", Some(RunSource::Api), true, SessionKind::Helper, SessionOrigin::Delegation),
            ("chat:abc", Some(RunSource::Api), false, SessionKind::Chat, SessionOrigin::Web),
            ("direct:agent-1", None, false, SessionKind::Chat, SessionOrigin::Web),
            ("room-1-3", Some(RunSource::Api), false, SessionKind::Chat, SessionOrigin::Api),
            ("custom-room", None, false, SessionKind::Chat, SessionOrigin::Api),
        ];
        for (room, source, helper, kind, origin) in cases {
            assert_eq!(kind_for_room(room, source, helper), (kind, origin), "{room}");
        }
    }

    #[test]
    fn capabilities_follow_the_kind_table() {
        let chat = SessionKind::Chat.capabilities(false);
        assert!(
            chat.send
                && chat.steer
                && chat.stop
                && chat.rename
                && chat.archive
                && chat.delete
                && chat.compact
                && chat.export
        );
        let telegram = SessionKind::Telegram.capabilities(false);
        assert!(telegram.send && !telegram.steer && telegram.rename && !telegram.delete);
        assert!(telegram.compact && telegram.export);
        assert!(
            !SessionKind::Checkin.capabilities(true).delete,
            "a check-in whose automation still exists stays"
        );
        assert!(SessionKind::Checkin.capabilities(false).delete);
        for kind in [SessionKind::Job, SessionKind::Helper] {
            let caps = kind.capabilities(false);
            assert!(
                !caps.send
                    && !caps.steer
                    && caps.stop
                    && !caps.rename
                    && caps.archive
                    && !caps.delete
                    && !caps.compact
                    && caps.export,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn titles_are_short_single_line_plain_text() {
        assert_eq!(
            derived_title("\n  Plan my   week\nwith details").as_deref(),
            Some("Plan my week")
        );
        assert_eq!(derived_title("   \n\t"), None);
        let long = derived_title(&"word ".repeat(40)).unwrap();
        assert_eq!(long.chars().count(), DERIVED_TITLE_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(clean_owner_title("  Trip\nplanning  "), Ok("Trip planning".to_string()));
        assert_eq!(clean_owner_title(" \n "), Err("title must be 1 to 120 characters"));
        assert_eq!(
            clean_owner_title(&"x".repeat(MAX_SESSION_TITLE_CHARS + 1)),
            Err("title must be 1 to 120 characters")
        );
        assert_eq!(
            clean_owner_title(&"x".repeat(MAX_SESSION_TITLE_CHARS)).map(|title| title.len()),
            Ok(MAX_SESSION_TITLE_CHARS)
        );
        let preview = preview_text(&format!("{}\nend", "y".repeat(200))).unwrap();
        assert_eq!(preview.chars().count(), MAX_SESSION_PREVIEW_CHARS);
        assert_eq!(preview_text("  "), None);
    }

    #[test]
    fn titles_follow_the_session_kind() {
        let chat = TitleContext {
            first_user_text: Some("Hello there\nmore"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Chat, SessionOrigin::Web, &chat),
            ("Hello there".to_string(), TitleSource::FirstMessage)
        );
        assert_eq!(
            session_title(SessionKind::Chat, SessionOrigin::Api, &empty_context()),
            (DEFAULT_CHAT_TITLE.to_string(), TitleSource::FirstMessage)
        );
        let telegram = TitleContext {
            bot_username: Some("anima_bot"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Telegram, SessionOrigin::Telegram, &telegram),
            ("Telegram · @anima_bot".to_string(), TitleSource::System)
        );
        assert_eq!(
            session_title(SessionKind::Telegram, SessionOrigin::Telegram, &empty_context()).0,
            "Telegram"
        );
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let checkin = TitleContext {
            first_user_text: Some(&wrapped),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Checkin, SessionOrigin::Schedule, &checkin).0,
            "Check-in · Review open tasks"
        );
        let scheduled = TitleContext {
            schedule_prompt: Some("Morning brief"),
            ..checkin
        };
        assert_eq!(
            session_title(SessionKind::Checkin, SessionOrigin::Schedule, &scheduled),
            ("Check-in · Morning brief".to_string(), TitleSource::System)
        );
        let job = TitleContext {
            job_title: Some("Prepare brief"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Job, SessionOrigin::Job, &job),
            ("Job · Prepare brief".to_string(), TitleSource::System)
        );
        let peer = TitleContext {
            peer_sender_name: Some("Beta"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Helper, SessionOrigin::Peer, &peer).0,
            "Messages from Beta"
        );
        let task = "Task delegated by workspace manager Anima (agent-7). Return the result and any blockers. Do not delegate further.\n\nCompare vendors";
        let delegated = TitleContext {
            first_user_text: Some(task),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Helper, SessionOrigin::Delegation, &delegated),
            ("Compare vendors".to_string(), TitleSource::System)
        );
        assert_eq!(delegating_agent_id(task), Some("agent-7"));
        assert_eq!(delegating_agent_id("Compare vendors"), None);
        assert_eq!(
            session_title(SessionKind::Helper, SessionOrigin::Delegation, &empty_context()).0,
            "Helper task"
        );
    }

    #[test]
    fn silent_checkin_groups_are_hidden_and_spoken_ones_are_not() {
        let messages = vec![
            message("u1", MessageRole::User, "Hi", &[], 1),
            message("a1", MessageRole::Assistant, "Hello", &[], 2),
            message("c1", MessageRole::User, "Check status", &[("kind", "checkin"), ("id", "s1")], 3),
            message("t1", MessageRole::Assistant, "", &[], 4),
            message("r1", MessageRole::Tool, "done", &[], 5),
            message("s1", MessageRole::Assistant, "CHECKIN_OK", &[], 6),
            message("c2", MessageRole::User, "Check status", &[("kind", "checkin"), ("id", "s1")], 7),
            message("s2", MessageRole::Assistant, "You have two overdue tasks", &[], 8),
            message("u2", MessageRole::User, "Say CHECKIN_OK", &[], 9),
            message("a2", MessageRole::Assistant, "CHECKIN_OK", &[], 10),
        ];
        let mut hidden = hidden_message_ids(messages.iter()).into_iter().collect::<Vec<_>>();
        hidden.sort();
        assert_eq!(hidden, ["c1", "r1", "s1", "t1"]);

        // A pruned transcript that starts mid-group hides only the bare sentinel.
        let tail = vec![
            message("t9", MessageRole::Tool, "x", &[], 1),
            message("s9", MessageRole::Assistant, " CHECKIN_OK ", &[], 2),
        ];
        assert_eq!(hidden_message_ids(tail.iter()), HashSet::from(["s9".to_string()]));
        assert!(is_checkin_message(&messages[2]));
        assert!(!is_checkin_message(&messages[0]));
        assert!(is_inbound_message(&message(
            "i",
            MessageRole::User,
            "hey",
            &[("source", "telegram")],
            1
        )));
        assert!(is_owner_web_turn(&message(
            "o",
            MessageRole::User,
            "hey",
            &[("source", "telegramThread")],
            1
        )));
    }

    #[test]
    fn commits_advance_activity_read_state_and_the_first_message_title_and_revert_exactly() {
        let mut registry = SessionRegistry::default();
        registry.insert(chat_record("agent-1", "chat:one", 10));
        let turn = vec![
            message("u1", MessageRole::User, "Plan the offsite\nsoon", &[], 20),
            message("a1", MessageRole::Assistant, "Sure", &[], 25),
        ];

        let undo = registry
            .record_commit("agent-1", "chat:one", &turn, true)
            .expect("the session exists");
        let record = registry.get("agent-1", "chat:one").unwrap();
        assert_eq!(record.last_activity_at_ms, 25);
        assert_eq!(
            record.last_read_at_ms,
            Some(20),
            "the owner's own message is read, the reply is not"
        );
        assert_eq!(record.title, "Plan the offsite");
        assert_eq!(record.title_source, TitleSource::FirstMessage);

        registry.revert_commit(undo);
        let record = registry.get("agent-1", "chat:one").unwrap();
        assert_eq!(record.last_activity_at_ms, 10);
        assert_eq!(record.last_read_at_ms, None);
        assert_eq!(record.title, DEFAULT_CHAT_TITLE);

        let undo = registry.record_commit("agent-1", "chat:one", &turn, false).unwrap();
        assert_eq!(
            registry.get("agent-1", "chat:one").unwrap().last_read_at_ms,
            None,
            "another source's turn leaves the read state alone"
        );
        registry.revert_commit(undo);
        registry.get_mut("agent-1", "chat:one").unwrap().title_source = TitleSource::Owner;
        registry.record_commit("agent-1", "chat:one", &turn, false).unwrap();
        assert_eq!(
            registry.get("agent-1", "chat:one").unwrap().title,
            DEFAULT_CHAT_TITLE,
            "owner titles are never replaced"
        );
        assert!(registry.record_commit("agent-1", "chat:missing", &turn, true).is_none());
    }

    #[test]
    fn snapshots_keep_live_agents_sorted_and_validation_rejects_bad_records() {
        let mut registry = SessionRegistry::default();
        registry.insert(chat_record("agent-b", "chat:two", 1));
        registry.insert(chat_record("agent-a", "chat:one", 1));
        registry.insert(chat_record("agent-gone", "chat:three", 1));
        let live = HashSet::from(["agent-a".to_string(), "agent-b".to_string()]);

        let saved = registry.snapshot_records(&live);
        assert_eq!(
            saved
                .iter()
                .map(|record| (record.agent_id.as_str(), record.id.as_str()))
                .collect::<Vec<_>>(),
            [("agent-a", "chat:one"), ("agent-b", "chat:two")]
        );
        assert!(SessionRegistry::validate(&saved).is_ok());
        let everyone = HashSet::from([
            "agent-a".to_string(),
            "agent-b".to_string(),
            "agent-gone".to_string(),
        ]);
        let restored = SessionRegistry::restored(registry.snapshot_records(&everyone), &live);
        assert_eq!(restored.len(), 2, "sessions of missing agents are dropped on restore");

        let mut duplicate = saved.clone();
        duplicate.push(saved[0].clone());
        assert!(SessionRegistry::validate(&duplicate).is_err());
        let mut bad_id = saved[0].clone();
        bad_id.id = "not valid".into();
        assert!(SessionRegistry::validate(&[bad_id]).is_err());
        let mut blank_title = saved[0].clone();
        blank_title.title = String::new();
        assert!(SessionRegistry::validate(&[blank_title]).is_err());
        let mut wrong_room = saved[0].clone();
        wrong_room.room_id = Some("other room".into());
        assert!(SessionRegistry::validate(&[wrong_room]).is_err());

        let legacy = chat_record("agent-a", "legacy room/1", 1);
        assert_eq!(legacy.room_id(), "legacy room/1");
        assert!(legacy.id.starts_with(LEGACY_ROOM_SESSION_PREFIX));
        assert!(SessionRegistry::validate(&[legacy]).is_ok());
    }

    #[test]
    fn records_use_the_documented_json_names() {
        let mut record = chat_record("agent-1", "legacy room/1", 5);
        record.parent_agent_id = Some("agent-0".into());
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["kind"], "chat");
        assert_eq!(value["origin"], "web");
        assert_eq!(value["titleSource"], "first_message");
        assert_eq!(value["createdAtMs"], 5);
        assert_eq!(value["lastActivityAtMs"], 5);
        assert_eq!(value["lastReadAtMs"], serde_json::Value::Null);
        assert_eq!(value["parentAgentId"], "agent-0");
        assert_eq!(value["roomId"], "legacy room/1");
        assert_eq!(
            serde_json::to_value(chat_record("agent-1", "chat:x", 1))
                .unwrap()
                .get("roomId"),
            None,
            "valid rooms carry no roomId"
        );
        let minimal: SessionRecord = serde_json::from_value(serde_json::json!({
            "id": "chat:x",
            "agentId": "agent-1",
            "kind": "helper",
            "origin": "peer",
            "title": "T",
            "titleSource": "system",
            "createdAtMs": 1,
            "lastActivityAtMs": 2
        }))
        .unwrap();
        assert!(!minimal.archived);
        assert_eq!(minimal.summary, None);
        assert_eq!(minimal.last_read_at_ms, None);
        assert_eq!(SessionKind::parse("checkin"), Some(SessionKind::Checkin));
        assert_eq!(SessionKind::parse("chats"), None);
        assert_eq!(SessionOrigin::Delegation.as_str(), "delegation");
        assert_eq!(TitleSource::Generated.as_str(), "generated");
    }

    #[test]
    fn session_creation_is_limited_per_agent_per_minute() {
        let mut limiter = SessionCreateLimiter::default();
        for _ in 0..MAX_SESSION_CREATIONS_PER_MINUTE {
            assert!(limiter.try_acquire("agent-1", 1_000));
        }
        assert!(!limiter.try_acquire("agent-1", 1_000 + 59_999));
        assert!(limiter.try_acquire("agent-2", 1_000), "limits are per agent");
        assert!(
            limiter.try_acquire("agent-1", 1_000 + 60_000),
            "the one-minute window slides"
        );
    }
}
```

In `hosts/rust-daemon/src/lib.rs`, add `mod sessions;` after `mod schedules;`.

Append to `mod tests` in `hosts/rust-daemon/src/state.rs` (hand-formatted, after the last test):

```rust
    #[test]
    fn session_records_are_saved_for_live_agents_and_restored() {
        use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

        let mut source = DaemonState::new();
        let agent_id = source
            .create_agent(test_config("session-owner"))
            .expect("agent should be created")
            .state
            .id;
        let chat = SessionRecord::new(
            &agent_id,
            "chat:one",
            SessionKind::Chat,
            SessionOrigin::Web,
            "Plans".into(),
            TitleSource::Owner,
            10,
        );
        let orphan = SessionRecord::new(
            "agent-deleted",
            "chat:two",
            SessionKind::Chat,
            SessionOrigin::Web,
            "Gone".into(),
            TitleSource::Owner,
            11,
        );
        source.sessions.insert(chat.clone());
        source.sessions.insert(orphan);

        let snapshot = source.control_plane_snapshot();
        assert_eq!(snapshot.sessions, vec![chat.clone()], "sessions of deleted agents are not saved");
        let snapshot: ControlPlaneSnapshot =
            serde_json::from_str(&serde_json::to_string(&snapshot).unwrap()).unwrap();

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(snapshot)
            .expect("sessions should restore");
        assert_eq!(restored.sessions.get(&agent_id, "chat:one"), Some(&chat));
        assert_eq!(restored.sessions.len(), 1);
    }

    #[test]
    fn restore_rejects_an_invalid_session_record_without_mutation() {
        use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

        let mut source = DaemonState::new();
        let agent_id = source
            .create_agent(test_config("session-owner"))
            .expect("agent should be created")
            .state
            .id;
        let mut snapshot = source.control_plane_snapshot();
        let mut record = SessionRecord::new(
            &agent_id,
            "chat:one",
            SessionKind::Chat,
            SessionOrigin::Web,
            "Plans".into(),
            TitleSource::Owner,
            10,
        );
        record.id = "bad id".into();
        snapshot.sessions.push(record);

        let mut state = DaemonState::new();
        assert!(state.restore_control_plane_snapshot(snapshot).is_err());
        assert_eq!(state.agent_count(), 0, "invalid restores cannot add agents");
        assert_eq!(state.sessions.len(), 0);
    }
```

In `hosts/rust-daemon/src/control_plane_store.rs`, in `snapshot_serializes_current_version_with_empty_connector_collections`, add after the `runs` assertion:

```rust
        assert_eq!(payload["sessions"], serde_json::json!([]));
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions::tests state::tests control_plane_store::tests`
Expected: compile errors such as `cannot find function session_id_for_room`, `cannot find type SessionRecord`, `no field sessions on type ControlPlaneSnapshot`, and `cannot find function unwrap_checkin_prompt`.

- [ ] **Step 3: Implement**

Put this above the test module in `hosts/rust-daemon/src/sessions/mod.rs`:

```rust
//! Sessions (spec §3): one conversation room of one agent plus its record.
//! A session id equals its room id, except for legacy rooms whose id is not a
//! valid session id; those map to a stable `legacy-room:<hash>` id and keep
//! their room on the record.

use std::collections::{HashMap, HashSet, VecDeque};

use anima_core::{DataValue, Message, MessageRole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::runs::RunSource;

/// Session ids are at most 200 bytes of `[A-Za-z0-9._:-]` (spec §3.1).
pub(crate) const MAX_SESSION_ID_BYTES: usize = 200;
/// Titles are 1–120 characters of plain text (spec §3.2).
pub(crate) const MAX_SESSION_TITLE_CHARS: usize = 120;
/// Titles derived from a message stay short enough for the sidebar.
pub(crate) const DERIVED_TITLE_CHARS: usize = 60;
/// `preview` is the last visible message, at most 160 characters (spec §3.2).
pub(crate) const MAX_SESSION_PREVIEW_CHARS: usize = 160;
/// The title of a chat created before its first message.
pub(crate) const DEFAULT_CHAT_TITLE: &str = "New chat";
/// Prefix of the ids that stand in for invalid legacy room ids.
pub(crate) const LEGACY_ROOM_SESSION_PREFIX: &str = "legacy-room:";
/// Session creations per agent per minute (spec §14).
pub(crate) const MAX_SESSION_CREATIONS_PER_MINUTE: usize = 60;
/// Workspace check-ins run in `schedule:<id>` (spec §3.1, §9.2).
pub(crate) const SCHEDULE_ROOM_PREFIX: &str = "schedule:";
const SESSION_CREATION_WINDOW_MS: u64 = 60_000;
const DELEGATED_TASK_PREFIX: &str = "Task delegated by workspace manager ";

/// What a session is (spec §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionKind {
    Chat,
    Telegram,
    Checkin,
    Job,
    Helper,
}

impl SessionKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Telegram => "telegram",
            Self::Checkin => "checkin",
            Self::Job => "job",
            Self::Helper => "helper",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "chat" => Some(Self::Chat),
            "telegram" => Some(Self::Telegram),
            "checkin" => Some(Self::Checkin),
            "job" => Some(Self::Job),
            "helper" => Some(Self::Helper),
            _ => None,
        }
    }

    /// The capability table of spec §3.2. A check-in session can be deleted
    /// only once its automation is gone.
    pub(crate) const fn capabilities(self, schedule_exists: bool) -> SessionCapabilities {
        let (send, steer, rename, delete, compact) = match self {
            Self::Chat => (true, true, true, true, true),
            Self::Telegram => (true, false, true, false, true),
            Self::Checkin => (true, true, true, !schedule_exists, true),
            Self::Job | Self::Helper => (false, false, false, false, false),
        };
        SessionCapabilities {
            send,
            steer,
            stop: true,
            rename,
            archive: true,
            delete,
            compact,
            export: true,
        }
    }
}

/// Where a session came from (spec §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionOrigin {
    Web,
    Api,
    Telegram,
    Schedule,
    Job,
    Delegation,
    Peer,
}

impl SessionOrigin {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Api => "api",
            Self::Telegram => "telegram",
            Self::Schedule => "schedule",
            Self::Job => "job",
            Self::Delegation => "delegation",
            Self::Peer => "peer",
        }
    }
}

/// Who set the title (spec §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TitleSource {
    FirstMessage,
    Generated,
    Owner,
    System,
}

impl TitleSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::FirstMessage => "first_message",
            Self::Generated => "generated",
            Self::Owner => "owner",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionCapabilities {
    pub(crate) send: bool,
    pub(crate) steer: bool,
    pub(crate) stop: bool,
    pub(crate) rename: bool,
    pub(crate) archive: bool,
    pub(crate) delete: bool,
    pub(crate) compact: bool,
    pub(crate) export: bool,
}

/// A compaction summary (spec §5.4); written from M3 on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSummary {
    pub(crate) text: String,
    pub(crate) through_message_id: String,
    pub(crate) created_at_ms: u64,
    pub(crate) source_message_count: usize,
}

/// The latest turn outside the model's view (spec §5.3); written from M3 on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionContextTrimmed {
    pub(crate) dropped_through_message_id: String,
    pub(crate) at_ms: u64,
}

/// The stored session record (spec §3.2). Derived fields are computed per
/// response and never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) kind: SessionKind,
    pub(crate) origin: SessionOrigin,
    pub(crate) title: String,
    pub(crate) title_source: TitleSource,
    pub(crate) created_at_ms: u64,
    pub(crate) last_activity_at_ms: u64,
    #[serde(default)]
    pub(crate) last_read_at_ms: Option<u64>,
    #[serde(default)]
    pub(crate) archived: bool,
    #[serde(default)]
    pub(crate) parent_session_id: Option<String>,
    #[serde(default)]
    pub(crate) parent_run_id: Option<String>,
    #[serde(default)]
    pub(crate) parent_agent_id: Option<String>,
    #[serde(default)]
    pub(crate) summary: Option<SessionSummary>,
    #[serde(default)]
    pub(crate) context_trimmed: Option<SessionContextTrimmed>,
    /// The transcript room when it differs from `id`: a legacy room whose id
    /// is not a valid session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) room_id: Option<String>,
}

impl SessionRecord {
    pub(crate) fn new(
        agent_id: &str,
        room_id: &str,
        kind: SessionKind,
        origin: SessionOrigin,
        title: String,
        title_source: TitleSource,
        now_ms: u64,
    ) -> Self {
        let id = session_id_for_room(room_id);
        let room = (id != room_id).then(|| room_id.to_string());
        Self {
            id,
            agent_id: agent_id.to_string(),
            kind,
            origin,
            title,
            title_source,
            created_at_ms: now_ms,
            last_activity_at_ms: now_ms,
            last_read_at_ms: None,
            archived: false,
            parent_session_id: None,
            parent_run_id: None,
            parent_agent_id: None,
            summary: None,
            context_trimmed: None,
            room_id: room,
        }
    }

    /// The transcript room this session reads and writes.
    pub(crate) fn room_id(&self) -> &str {
        self.room_id.as_deref().unwrap_or(&self.id)
    }

    pub(crate) fn capabilities(&self, schedule_exists: bool) -> SessionCapabilities {
        self.kind.capabilities(schedule_exists)
    }
}

pub(crate) fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// The session id of a transcript room: the room id itself when it is a valid
/// session id, otherwise a stable `legacy-room:<hash>` id (spec §3.1, F17).
pub(crate) fn session_id_for_room(room_id: &str) -> String {
    if is_valid_session_id(room_id) {
        return room_id.to_string();
    }
    let digest = Sha256::digest(room_id.as_bytes());
    let hex = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{LEGACY_ROOM_SESSION_PREFIX}{hex}")
}

/// A new web chat id, `chat:<uuid-v4>` (spec §3.1).
pub(crate) fn new_chat_session_id() -> String {
    format!("chat:{}", uuid::Uuid::new_v4())
}

pub(crate) fn schedule_room_id(schedule_id: &str) -> String {
    format!("{SCHEDULE_ROOM_PREFIX}{schedule_id}")
}

pub(crate) fn schedule_id_of_room(room_id: &str) -> Option<&str> {
    room_id.strip_prefix(SCHEDULE_ROOM_PREFIX)
}

pub(crate) fn connector_id_of_room(room_id: &str) -> Option<&str> {
    room_id.strip_prefix("telegram:")
}

pub(crate) fn job_id_of_room(room_id: &str) -> Option<&str> {
    room_id.strip_prefix("job:")
}

/// The sending agent of a `peer:<sender>:<recipient>` room.
pub(crate) fn peer_sender_of_room(room_id: &str) -> Option<&str> {
    room_id
        .strip_prefix("peer:")?
        .split(':')
        .next()
        .filter(|sender| !sender.is_empty())
}

/// Kind and origin of a room by the rules of spec §3.1.
pub(crate) fn kind_for_room(
    room_id: &str,
    source: Option<RunSource>,
    helper_agent: bool,
) -> (SessionKind, SessionOrigin) {
    if room_id.starts_with("telegram:") {
        return (SessionKind::Telegram, SessionOrigin::Telegram);
    }
    if room_id.starts_with(SCHEDULE_ROOM_PREFIX) {
        return (SessionKind::Checkin, SessionOrigin::Schedule);
    }
    if room_id.starts_with("job:") {
        return (SessionKind::Job, SessionOrigin::Job);
    }
    if room_id.starts_with("peer:") || source == Some(RunSource::Peer) {
        return (SessionKind::Helper, SessionOrigin::Peer);
    }
    if helper_agent || source == Some(RunSource::Delegation) {
        return (SessionKind::Helper, SessionOrigin::Delegation);
    }
    if room_id.starts_with("chat:")
        || room_id.starts_with("direct:")
        || source == Some(RunSource::Web)
    {
        return (SessionKind::Chat, SessionOrigin::Web);
    }
    (SessionKind::Chat, SessionOrigin::Api)
}

/// `text` cut to `max_chars` characters, ending in "…" when shortened.
pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut kept = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    while kept.ends_with(char::is_whitespace) {
        kept.pop();
    }
    kept.push('…');
    kept
}

fn single_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// A title from the first non-empty line of a message.
pub(crate) fn derived_title(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let cleaned = single_line(line);
    (!cleaned.is_empty()).then(|| truncate_chars(&cleaned, DERIVED_TITLE_CHARS))
}

/// An owner-supplied title as stored: one line of plain text, 1–120 characters.
pub(crate) fn clean_owner_title(title: &str) -> Result<String, &'static str> {
    let cleaned = single_line(title);
    let chars = cleaned.chars().count();
    if chars == 0 || chars > MAX_SESSION_TITLE_CHARS {
        return Err("title must be 1 to 120 characters");
    }
    Ok(cleaned)
}

/// The sidebar preview of a message (spec §3.2).
pub(crate) fn preview_text(text: &str) -> Option<String> {
    let cleaned = single_line(text);
    (!cleaned.is_empty()).then(|| truncate_chars(&cleaned, MAX_SESSION_PREVIEW_CHARS))
}

/// The owner's task inside a delegated run's input.
pub(crate) fn delegated_task_text(text: &str) -> &str {
    if text.starts_with(DELEGATED_TASK_PREFIX) {
        text.split_once("\n\n").map(|(_, task)| task).unwrap_or(text)
    } else {
        text
    }
}

/// The manager id in a delegated task's preamble ("… manager <name> (<id>). …").
pub(crate) fn delegating_agent_id(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(DELEGATED_TASK_PREFIX)?;
    let (head, _) = rest.split_once("). ")?;
    let (_, id) = head.rsplit_once(" (")?;
    (!id.trim().is_empty()).then_some(id)
}

/// What a session title can be derived from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TitleContext<'a> {
    pub(crate) first_user_text: Option<&'a str>,
    pub(crate) schedule_prompt: Option<&'a str>,
    pub(crate) job_title: Option<&'a str>,
    pub(crate) bot_username: Option<&'a str>,
    pub(crate) peer_sender_name: Option<&'a str>,
}

/// The initial title of a session by kind (spec §3.2 `titleSource`).
pub(crate) fn session_title(
    kind: SessionKind,
    origin: SessionOrigin,
    context: &TitleContext<'_>,
) -> (String, TitleSource) {
    let labelled = |label: &str, detail: Option<String>| {
        detail
            .map(|detail| truncate_chars(&format!("{label} · {detail}"), MAX_SESSION_TITLE_CHARS))
            .unwrap_or_else(|| label.to_string())
    };
    match kind {
        SessionKind::Chat => (
            context
                .first_user_text
                .and_then(derived_title)
                .unwrap_or_else(|| DEFAULT_CHAT_TITLE.to_string()),
            TitleSource::FirstMessage,
        ),
        SessionKind::Telegram => (
            labelled(
                "Telegram",
                context
                    .bot_username
                    .map(|name| format!("@{}", name.trim_start_matches('@'))),
            ),
            TitleSource::System,
        ),
        SessionKind::Checkin => (
            labelled(
                "Check-in",
                context
                    .schedule_prompt
                    .or(context
                        .first_user_text
                        .map(crate::schedules::unwrap_checkin_prompt))
                    .and_then(derived_title),
            ),
            TitleSource::System,
        ),
        SessionKind::Job => (
            labelled("Job", context.job_title.and_then(derived_title)),
            TitleSource::System,
        ),
        SessionKind::Helper if origin == SessionOrigin::Peer => (
            context
                .peer_sender_name
                .and_then(derived_title)
                .map(|name| truncate_chars(&format!("Messages from {name}"), MAX_SESSION_TITLE_CHARS))
                .unwrap_or_else(|| "Agent messages".to_string()),
            TitleSource::System,
        ),
        SessionKind::Helper => (
            context
                .first_user_text
                .map(delegated_task_text)
                .and_then(derived_title)
                .unwrap_or_else(|| "Helper task".to_string()),
            TitleSource::System,
        ),
    }
}

fn metadata_str<'a>(message: &'a Message, key: &str) -> Option<&'a str> {
    match message.content.metadata.as_ref()?.get(key) {
        Some(DataValue::String(value)) => Some(value),
        _ => None,
    }
}

/// A scheduled check-in prompt (tagged by the scheduler).
pub(crate) fn is_checkin_message(message: &Message) -> bool {
    message.role == MessageRole::User && metadata_str(message, "kind") == Some("checkin")
}

/// A message that arrived from a Telegram chat.
pub(crate) fn is_inbound_message(message: &Message) -> bool {
    message.role == MessageRole::User && metadata_str(message, "source") == Some("telegram")
}

/// A Telegram owner turn sent from the web console.
pub(crate) fn is_owner_web_turn(message: &Message) -> bool {
    message.role == MessageRole::User && metadata_str(message, "source") == Some("telegramThread")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupStart {
    Missing,
    Checkin,
    Other,
}

/// Messages hidden from session views: every message of a check-in turn
/// whose final reply is the silent sentinel (spec §3.3 "silent check-in
/// pairs"). A turn whose opening message was pruned hides only a bare
/// sentinel reply. `messages` are one room's messages in transcript order.
pub(crate) fn hidden_message_ids<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
) -> HashSet<String> {
    let mut hidden = HashSet::new();
    let mut group: Vec<&Message> = Vec::new();
    let mut start = GroupStart::Missing;
    for message in messages {
        if message.role == MessageRole::User {
            close_group(&group, start, &mut hidden);
            group.clear();
            start = if is_checkin_message(message) {
                GroupStart::Checkin
            } else {
                GroupStart::Other
            };
        }
        group.push(message);
    }
    close_group(&group, start, &mut hidden);
    hidden
}

fn close_group(group: &[&Message], start: GroupStart, hidden: &mut HashSet<String>) {
    let Some(reply) = group
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Assistant)
    else {
        return;
    };
    if !crate::schedules::is_silent_checkin_reply(&reply.content.text) {
        return;
    }
    match start {
        GroupStart::Checkin => hidden.extend(group.iter().map(|message| message.id.clone())),
        GroupStart::Missing => {
            hidden.insert(reply.id.clone());
        }
        GroupStart::Other => {}
    }
}

/// What `record_commit` changed, so a rolled-back commit restores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionCommitUndo {
    agent_id: String,
    session_id: String,
    last_activity_at_ms: u64,
    last_read_at_ms: Option<u64>,
    title: String,
    title_source: TitleSource,
}

/// Session records keyed by `(agentId, id)` (spec §3.2).
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionRegistry {
    records: HashMap<(String, String), SessionRecord>,
}

impl SessionRegistry {
    fn key(agent_id: &str, session_id: &str) -> (String, String) {
        (agent_id.to_string(), session_id.to_string())
    }

    pub(crate) fn get(&self, agent_id: &str, session_id: &str) -> Option<&SessionRecord> {
        self.records.get(&Self::key(agent_id, session_id))
    }

    pub(crate) fn get_mut(&mut self, agent_id: &str, session_id: &str) -> Option<&mut SessionRecord> {
        self.records.get_mut(&Self::key(agent_id, session_id))
    }

    pub(crate) fn contains(&self, agent_id: &str, session_id: &str) -> bool {
        self.records.contains_key(&Self::key(agent_id, session_id))
    }

    pub(crate) fn insert(&mut self, record: SessionRecord) -> Option<SessionRecord> {
        self.records
            .insert((record.agent_id.clone(), record.id.clone()), record)
    }

    pub(crate) fn remove(&mut self, agent_id: &str, session_id: &str) -> Option<SessionRecord> {
        self.records.remove(&Self::key(agent_id, session_id))
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = &SessionRecord> {
        self.records.values()
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// Records to save, sorted, without sessions of agents that no longer exist.
    pub(crate) fn snapshot_records(&self, live_agents: &HashSet<String>) -> Vec<SessionRecord> {
        let mut records = self
            .records
            .values()
            .filter(|record| live_agents.contains(&record.agent_id))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.agent_id
                .cmp(&right.agent_id)
                .then_with(|| left.id.cmp(&right.id))
        });
        records
    }

    pub(crate) fn validate(records: &[SessionRecord]) -> Result<(), String> {
        let mut keys = HashSet::new();
        for record in records {
            if !is_valid_session_id(&record.id) {
                return Err(format!("session id '{}' is invalid", record.id));
            }
            if record.agent_id.trim().is_empty() {
                return Err(format!("session '{}' has an empty agent id", record.id));
            }
            if !keys.insert((record.agent_id.as_str(), record.id.as_str())) {
                return Err(format!(
                    "duplicate session '{}' for agent '{}'",
                    record.id, record.agent_id
                ));
            }
            let title_chars = record.title.chars().count();
            if title_chars == 0 || title_chars > MAX_SESSION_TITLE_CHARS {
                return Err(format!("session '{}' has an invalid title", record.id));
            }
            if let Some(room) = &record.room_id {
                if room.is_empty() || session_id_for_room(room) != record.id {
                    return Err(format!(
                        "session '{}' has a room that does not map to it",
                        record.id
                    ));
                }
            }
        }
        Ok(())
    }

    /// The registry after a restart: sessions of missing agents are dropped.
    pub(crate) fn restored(records: Vec<SessionRecord>, live_agents: &HashSet<String>) -> Self {
        let mut registry = Self::default();
        for record in records {
            if live_agents.contains(&record.agent_id) {
                registry.insert(record);
            }
        }
        registry
    }

    /// Advances a session for a committed run: activity moves to the newest
    /// message, the owner's own message marks the session read up to itself,
    /// and a placeholder chat title becomes the first message's title.
    pub(crate) fn record_commit(
        &mut self,
        agent_id: &str,
        session_id: &str,
        messages: &[Message],
        owner_authored: bool,
    ) -> Option<SessionCommitUndo> {
        let record = self.records.get_mut(&Self::key(agent_id, session_id))?;
        let undo = SessionCommitUndo {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            last_activity_at_ms: record.last_activity_at_ms,
            last_read_at_ms: record.last_read_at_ms,
            title: record.title.clone(),
            title_source: record.title_source,
        };
        if let Some(latest) = messages.iter().map(|message| message.created_at_ms).max() {
            record.last_activity_at_ms = record.last_activity_at_ms.max(latest);
        }
        let first_user = messages
            .iter()
            .find(|message| message.role == MessageRole::User);
        if owner_authored {
            if let Some(first_user) = first_user {
                record.last_read_at_ms = Some(
                    record
                        .last_read_at_ms
                        .unwrap_or(0)
                        .max(first_user.created_at_ms),
                );
            }
        }
        if record.kind == SessionKind::Chat
            && record.title_source == TitleSource::FirstMessage
            && record.title == DEFAULT_CHAT_TITLE
        {
            if let Some(title) = first_user.and_then(|message| derived_title(&message.content.text)) {
                record.title = title;
            }
        }
        Some(undo)
    }

    pub(crate) fn revert_commit(&mut self, undo: SessionCommitUndo) {
        if let Some(record) = self.records.get_mut(&Self::key(&undo.agent_id, &undo.session_id)) {
            record.last_activity_at_ms = undo.last_activity_at_ms;
            record.last_read_at_ms = undo.last_read_at_ms;
            record.title = undo.title;
            record.title_source = undo.title_source;
        }
    }
}

/// Session creations per agent per minute (spec §14); not persisted.
#[derive(Debug, Default)]
pub(crate) struct SessionCreateLimiter {
    windows: HashMap<String, VecDeque<u64>>,
}

impl SessionCreateLimiter {
    pub(crate) fn try_acquire(&mut self, agent_id: &str, now_ms: u64) -> bool {
        let window = self.windows.entry(agent_id.to_string()).or_default();
        while window
            .front()
            .is_some_and(|at| now_ms.saturating_sub(*at) >= SESSION_CREATION_WINDOW_MS)
        {
            window.pop_front();
        }
        if window.len() >= MAX_SESSION_CREATIONS_PER_MINUTE {
            return false;
        }
        window.push_back(now_ms);
        true
    }
}
```

In `hosts/rust-daemon/src/schedules.rs`, add directly after `pub(crate) fn wrap_checkin_prompt(...) { ... }`:

```rust
/// The owner's prompt inside a wrapped check-in input.
pub(crate) fn unwrap_checkin_prompt(text: &str) -> &str {
    text.strip_suffix(CHECKIN_SUFFIX)
        .map(str::trim_end)
        .unwrap_or(text)
        .trim()
}
```

In `hosts/rust-daemon/src/control_plane_store.rs`:

- add to `ControlPlaneSnapshot`, after `runs`:

```rust
    #[serde(default)]
    pub(crate) sessions: Vec<crate::sessions::SessionRecord>,
```

- in `with_connector_state_and_cleanup`, add `sessions: vec![],` after `runs: vec![],`.

In `hosts/rust-daemon/src/state.rs` (hand-formatted):

- add `pub(crate) sessions: crate::sessions::SessionRegistry,` to `DaemonState` after `pub(crate) runs: crate::runs::RunLedger,`, and `sessions: crate::sessions::SessionRegistry::default(),` after `runs: crate::runs::RunLedger::default(),` in `with_model_adapter_and_events_and_limits`;
- in `control_plane_snapshot`, after `snapshot.runs = self.runs.snapshot_records(&self.live_agent_ids());`, add `snapshot.sessions = self.sessions.snapshot_records(&self.live_agent_ids());`;
- in `validate_control_plane_snapshot`, after `crate::runs::RunLedger::validate(&snapshot.runs)?;`, add `crate::sessions::SessionRegistry::validate(&snapshot.sessions)?;`;
- in `restore_control_plane_snapshot`, directly before `self.runs = crate::runs::RunLedger::restored(`, add:

```rust
        self.sessions = crate::sessions::SessionRegistry::restored(
            snapshot.sessions,
            &self.live_agent_ids(),
        );
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions::tests state::tests control_plane_store::tests schedules::tests`
Expected: PASS (11 sessions tests, every `state::tests` test including the two new ones, the control-plane store tests, and the schedule tests).

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/sessions/mod.rs hosts/rust-daemon/src/lib.rs hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/state.rs
git commit -m "feat(daemon): add session records, room mapping, titles, and the session registry"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. Add `legacy-room:` to `RESERVED_ROOM_PREFIXES` in `hosts/rust-daemon/src/routes/agents.rs`, so a client `roomId` can never alias a mapped `legacy-room:<hash>` session id. Test: `POST /api/agents/{id}/run` with `roomId: "legacy-room:abc"` returns the existing reserved-prefix 400.
2. A session's `lastActivityAtMs` advances only for visible messages: silent check-in pairs (`CHECKIN_OK` exchanges) do not move a heartbeat session to the top of the list. Test.

---

### Task 3: History store contract and the bounded in-memory store

**Files:**

- Create: `hosts/rust-daemon/src/history/mod.rs`, `hosts/rust-daemon/src/history/memory.rs`, `hosts/rust-daemon/src/history/conformance.rs`
- Modify: `hosts/rust-daemon/src/lib.rs` (`mod history;`)

**Interfaces:**

- Consumes: `anima_core::Message`; `crate::runs::{RunRecord, RunStart, RunSource, RunStatus}`.
- Produces (in `crate::history`):
  - constants `EPHEMERAL_HISTORY_MAX_ROWS` (100,000), `MAX_SEARCH_TOKENS` (8), `MAX_SNIPPET_CHARS` (160);
  - `HistoryMessage { agent_id, session_id, hidden: bool, message: Message }` (+ `order(&self) -> MessageOrder`);
  - `MessageOrder { created_at_ms, ordinal, id }` (`Ord`, the transcript order within a session; `MessageOrder::of(&Message)`), `message_ordinal(&str) -> u64` (the counter of a runtime `msg-<ms>-<n>` id, else 0);
  - `MessagePageQuery { agent_id, session_id, before: Option<MessageOrder>, limit, include_hidden }`;
  - `HistoryError` (`new(impl Display)`, `message()`, `Display`, `From<std::io::Error>`, `From<serde_json::Error>`);
  - `#[async_trait] trait HistoryStore: Send + Sync` with `label() -> &'static str`, `is_ephemeral() -> bool` (default `false`), `upsert_messages(&[HistoryMessage])`, `upsert_runs(&[RunRecord])`, `existing_message_ids(&[String]) -> HashSet<String>`, `get_message(agent_id, session_id, message_id) -> Option<HistoryMessage>`, `get_run(run_id) -> Option<RunRecord>`, `page_messages(&MessagePageQuery) -> Vec<HistoryMessage>` (newest first, strictly older than `before`), `visible_message_counts(agent_id, &[String]) -> HashMap<String, usize>`, `search_messages(&[String] agent ids, query, limit) -> Vec<HistoryMessage>` (visible only, newest first, every word must match as a word prefix), `delete_session(agent_id, session_id)` (messages, runs, attachments) — all `Result<_, HistoryError>`, all idempotent by id;
  - `search_tokens(&str) -> Vec<String>`, `text_matches(&str, &[String]) -> bool`, `search_snippet(&str, &[String]) -> String`;
  - `MemoryHistoryStore::new()` / `with_max_rows(usize)` (label `"memory"`, ephemeral, oldest rows dropped beyond the cap);
  - test-only `history::conformance::{assert_history_store_conformance(&dyn HistoryStore), history_message(id, agent_id, session_id, role, text, created_at_ms) -> HistoryMessage, terminal_run(agent_id, session_id) -> RunRecord}` (Tasks 4–6 reuse them).

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/history/conformance.rs`:

```rust
//! Behaviour every history store shares (memory, SQLite, Postgres).

use std::collections::HashSet;

use anima_core::{Content, Message, MessageRole};

use super::{HistoryMessage, HistoryStore, MessagePageQuery};
use crate::runs::{RunRecord, RunSource, RunStart, RunStatus};

pub(crate) fn history_message(
    id: &str,
    agent_id: &str,
    session_id: &str,
    role: MessageRole,
    text: &str,
    created_at_ms: u64,
) -> HistoryMessage {
    HistoryMessage {
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        hidden: false,
        message: Message {
            id: id.into(),
            agent_id: agent_id.into(),
            room_id: session_id.into(),
            content: Content {
                text: text.into(),
                ..Content::default()
            },
            role,
            created_at_ms,
        },
    }
}

pub(crate) fn terminal_run(agent_id: &str, session_id: &str) -> RunRecord {
    let mut run = RunRecord::running(
        RunStart {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            source: RunSource::Api,
            source_ref: None,
            idempotency_key: None,
            text: "Deploy the build tonight".into(),
            model: "test-model".into(),
            provider: None,
            parent_run_id: None,
        },
        100,
    );
    run.finish(RunStatus::Completed, None, 110);
    run
}

fn ids(rows: &[HistoryMessage]) -> Vec<String> {
    rows.iter().map(|row| row.message.id.clone()).collect()
}

fn page(
    agent_id: &str,
    session_id: &str,
    before: Option<&HistoryMessage>,
    limit: usize,
    include_hidden: bool,
) -> MessagePageQuery {
    MessagePageQuery {
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        before: before.map(HistoryMessage::order),
        limit,
        include_hidden,
    }
}

/// Every store must pass this. Agent ids, timestamps, and message ids are
/// unique per call, so a shared Postgres database can run it repeatedly.
pub(crate) async fn assert_history_store_conformance(store: &dyn HistoryStore) {
    let agent = format!("agent-{}", uuid::Uuid::new_v4());
    let other = format!("agent-{}", uuid::Uuid::new_v4());
    let base = (uuid::Uuid::new_v4().as_u128() % 1_000_000_000) as u64 * 1_000;
    let at = |offset: u64| base + offset;
    let id = |offset: u64, ordinal: u64| format!("msg-{}-{ordinal}", base + offset);
    let mut silent = history_message(
        &id(300, 12),
        &agent,
        "chat:a",
        MessageRole::Assistant,
        "CHECKIN_OK deploy",
        at(300),
    );
    silent.hidden = true;
    // Two messages in the same millisecond keep their creation order through
    // the id counter (9 before 10, which string order would reverse).
    let rows = vec![
        history_message(&id(100, 9), &agent, "chat:a", MessageRole::User, "Deploy the build tonight", at(100)),
        history_message(&id(100, 10), &agent, "chat:a", MessageRole::Assistant, "Build deployed.", at(100)),
        history_message(&id(200, 11), &agent, "chat:a", MessageRole::User, "Thanks", at(200)),
        silent,
        history_message(&id(150, 13), &agent, "chat:b", MessageRole::User, "deployment notes for later", at(150)),
        history_message(&id(160, 14), &other, "chat:a", MessageRole::User, "deploy elsewhere", at(160)),
    ];
    store.upsert_messages(&rows).await.expect("messages upsert");
    let mut edited = rows[2].clone();
    edited.message.content.text = "Thanks!".into();
    store
        .upsert_messages(&[edited.clone(), edited])
        .await
        .expect("an id written twice stays one row");

    let newest = store
        .page_messages(&page(&agent, "chat:a", None, 10, false))
        .await
        .unwrap();
    assert_eq!(ids(&newest), [id(200, 11), id(100, 10), id(100, 9)]);
    assert_eq!(newest[0].message.content.text, "Thanks!");
    assert_eq!(newest[0].agent_id, agent);
    assert_eq!(newest[0].session_id, "chat:a");
    let everything = store
        .page_messages(&page(&agent, "chat:a", None, 10, true))
        .await
        .unwrap();
    assert_eq!(
        ids(&everything),
        [id(300, 12), id(200, 11), id(100, 10), id(100, 9)]
    );
    assert!(everything[0].hidden);
    let second = store
        .page_messages(&page(&agent, "chat:a", Some(&rows[2]), 1, false))
        .await
        .unwrap();
    assert_eq!(ids(&second), [id(100, 10)]);
    let last = store
        .page_messages(&page(&agent, "chat:a", Some(&rows[1]), 5, false))
        .await
        .unwrap();
    assert_eq!(ids(&last), [id(100, 9)]);
    assert!(store
        .page_messages(&page(&agent, "chat:a", Some(&rows[0]), 5, false))
        .await
        .unwrap()
        .is_empty());

    let counts = store
        .visible_message_counts(
            &agent,
            &["chat:a".to_string(), "chat:b".to_string(), "chat:none".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(counts.get("chat:a"), Some(&3));
    assert_eq!(counts.get("chat:b"), Some(&1));
    assert_eq!(counts.get("chat:none").copied().unwrap_or(0), 0);
    assert!(store
        .visible_message_counts(&agent, &[])
        .await
        .unwrap()
        .is_empty());

    let agents = [agent.clone()];
    assert_eq!(
        ids(&store.search_messages(&agents, "deploy", 10).await.unwrap()),
        [id(150, 13), id(100, 10), id(100, 9)],
        "word prefixes, newest first, hidden rows and other agents excluded"
    );
    assert_eq!(
        ids(&store.search_messages(&agents, "DEPL", 10).await.unwrap()),
        [id(150, 13), id(100, 10), id(100, 9)]
    );
    assert_eq!(
        ids(&store.search_messages(&agents, "deploy tonight", 10).await.unwrap()),
        [id(100, 9)],
        "every word must match"
    );
    assert_eq!(
        ids(&store.search_messages(&agents, "deploy", 1).await.unwrap()),
        [id(150, 13)]
    );
    assert!(store.search_messages(&agents, "!!", 10).await.unwrap().is_empty());
    assert!(store.search_messages(&[], "deploy", 10).await.unwrap().is_empty());
    let both = [agent.clone(), other.clone()];
    assert_eq!(
        ids(&store.search_messages(&both, "elsewhere", 10).await.unwrap()),
        [id(160, 14)]
    );

    let known = store
        .existing_message_ids(&[id(100, 9), format!("missing-{base}")])
        .await
        .unwrap();
    assert_eq!(known, HashSet::from([id(100, 9)]));
    assert!(store.existing_message_ids(&[]).await.unwrap().is_empty());
    assert_eq!(
        store
            .get_message(&agent, "chat:a", &id(100, 10))
            .await
            .unwrap()
            .map(|row| row.message.content.text),
        Some("Build deployed.".to_string())
    );
    assert_eq!(
        store.get_message(&agent, "chat:b", &id(100, 10)).await.unwrap(),
        None,
        "a message belongs to one session"
    );

    let run = terminal_run(&agent, "chat:a");
    let kept_run = terminal_run(&agent, "chat:b");
    store
        .upsert_runs(&[run.clone(), kept_run.clone()])
        .await
        .unwrap();
    store.upsert_runs(std::slice::from_ref(&run)).await.unwrap();
    assert_eq!(store.get_run(&run.id).await.unwrap(), Some(run.clone()));

    store.delete_session(&agent, "chat:a").await.unwrap();
    assert!(store
        .page_messages(&page(&agent, "chat:a", None, 10, true))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        ids(&store.search_messages(&agents, "deploy", 10).await.unwrap()),
        [id(150, 13)]
    );
    assert_eq!(store.get_run(&run.id).await.unwrap(), None);
    assert_eq!(store.get_run(&kept_run.id).await.unwrap(), Some(kept_run));
    assert_eq!(
        store
            .page_messages(&page(&other, "chat:a", None, 10, false))
            .await
            .unwrap()
            .len(),
        1,
        "other agents' sessions are untouched"
    );
    store
        .delete_session(&agent, "chat:a")
        .await
        .expect("deleting twice is harmless");
}
```

Create `hosts/rust-daemon/src/history/memory.rs` with only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::{assert_history_store_conformance, history_message};
    use anima_core::MessageRole;

    #[tokio::test]
    async fn memory_store_meets_the_conformance_suite() {
        assert_history_store_conformance(&MemoryHistoryStore::new()).await;
    }

    #[tokio::test]
    async fn ephemeral_tables_drop_the_oldest_rows_beyond_the_cap() {
        let store = MemoryHistoryStore::with_max_rows(2);
        let rows = (0..3u64)
            .map(|n| {
                history_message(
                    &format!("msg-{n}-{n}"),
                    "agent-1",
                    "chat:a",
                    MessageRole::User,
                    "hello",
                    n,
                )
            })
            .collect::<Vec<_>>();
        let all_ids = rows
            .iter()
            .map(|row| row.message.id.clone())
            .collect::<Vec<_>>();
        store.upsert_messages(&rows).await.unwrap();
        assert_eq!(
            store.existing_message_ids(&all_ids).await.unwrap(),
            HashSet::from(["msg-1-1".to_string(), "msg-2-2".to_string()])
        );

        // Rewriting a kept row neither makes it newer nor evicts another row.
        store.upsert_messages(&rows[1..2]).await.unwrap();
        assert_eq!(store.existing_message_ids(&all_ids).await.unwrap().len(), 2);
        assert!(store.is_ephemeral());
        assert_eq!(store.label(), "memory");
    }
}
```

Create `hosts/rust-daemon/src/history/mod.rs` with only the module declarations and tests for now:

```rust
#[cfg(test)]
pub(crate) mod conformance;
mod memory;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_order_uses_the_runtime_counter_within_a_millisecond() {
        assert_eq!(message_ordinal("msg-1700-9"), 9);
        assert_eq!(message_ordinal("msg-1700-10"), 10);
        assert_eq!(message_ordinal("message-1"), 0);
        assert_eq!(message_ordinal("msg-x"), 0);
        let nine = MessageOrder {
            created_at_ms: 1_700,
            ordinal: 9,
            id: "msg-1700-9".into(),
        };
        let ten = MessageOrder {
            created_at_ms: 1_700,
            ordinal: 10,
            id: "msg-1700-10".into(),
        };
        assert!(nine < ten, "string order would put -10 first");
        assert!(
            ten < MessageOrder {
                created_at_ms: 1_701,
                ordinal: 0,
                id: "a".into(),
            }
        );
    }

    #[test]
    fn search_tokens_are_lowercase_words() {
        assert_eq!(search_tokens("  Deploy, the BUILD! "), ["deploy", "the", "build"]);
        assert!(search_tokens("!!! ...").is_empty());
        assert_eq!(search_tokens(&"word ".repeat(20)).len(), MAX_SEARCH_TOKENS);
        let tokens = search_tokens("build deploy");
        assert!(text_matches("We deployed the Build.", &tokens));
        assert!(!text_matches("We deployed it.", &tokens));
        assert!(!text_matches("anything", &[]));
    }

    #[test]
    fn snippets_center_on_the_first_match() {
        let text = format!("{}needle here{}", "a ".repeat(100), " b".repeat(100));
        let snippet = search_snippet(&text, &search_tokens("NEEDLE"));
        assert!(snippet.contains("needle here"), "{snippet}");
        assert!(snippet.starts_with('…') && snippet.ends_with('…'), "{snippet}");
        assert!(snippet.chars().count() <= MAX_SNIPPET_CHARS + 2);
        assert_eq!(search_snippet("short\ntext", &search_tokens("short")), "short text");
        assert_eq!(search_snippet("no match here", &search_tokens("zebra")), "no match here");
    }
}
```

In `hosts/rust-daemon/src/lib.rs`, add `mod history;` after `mod events;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::`
Expected: compile errors such as `cannot find type HistoryMessage`, `cannot find function message_ordinal`, and `cannot find type MemoryHistoryStore`.

- [ ] **Step 3: Implement**

Replace `hosts/rust-daemon/src/history/mod.rs` above its test module with:

```rust
//! History store (spec §13.1): the row-based, append-only record of every
//! committed message and terminal run. The outbox feeds it; session views
//! merge it with the control plane's hot tail.

#[cfg(test)]
pub(crate) mod conformance;
mod memory;

pub(crate) use memory::MemoryHistoryStore;

use std::collections::{HashMap, HashSet};

use anima_core::Message;
use async_trait::async_trait;

use crate::runs::RunRecord;

/// Rows per in-memory table in ephemeral mode (spec §13.1).
pub(crate) const EPHEMERAL_HISTORY_MAX_ROWS: usize = 100_000;
/// Searches use at most this many query words.
pub(crate) const MAX_SEARCH_TOKENS: usize = 8;
/// Search snippets hold at most this many characters, plus ellipses.
pub(crate) const MAX_SNIPPET_CHARS: usize = 160;

/// One committed transcript message as the history store keeps it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoryMessage {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    /// Part of a silent check-in turn (spec §3.3); excluded unless asked for.
    pub(crate) hidden: bool,
    pub(crate) message: Message,
}

impl HistoryMessage {
    pub(crate) fn order(&self) -> MessageOrder {
        MessageOrder::of(&self.message)
    }
}

/// Transcript order within a session: creation time, then the runtime's
/// per-process message counter (messages of one session are created one at
/// a time), then the id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MessageOrder {
    pub(crate) created_at_ms: u64,
    pub(crate) ordinal: u64,
    pub(crate) id: String,
}

impl MessageOrder {
    pub(crate) fn of(message: &Message) -> Self {
        Self {
            created_at_ms: message.created_at_ms,
            ordinal: message_ordinal(&message.id),
            id: message.id.clone(),
        }
    }
}

/// The counter of a runtime message id (`msg-<ms>-<n>`); 0 for other ids.
pub(crate) fn message_ordinal(id: &str) -> u64 {
    id.rsplit_once('-')
        .filter(|(prefix, _)| prefix.starts_with("msg-"))
        .and_then(|(_, counter)| counter.parse().ok())
        .unwrap_or(0)
}

/// One page of a session's messages, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MessagePageQuery {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    /// Only messages strictly older than this one.
    pub(crate) before: Option<MessageOrder>,
    pub(crate) limit: usize,
    pub(crate) include_hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryError(String);

impl HistoryError {
    pub(crate) fn new(message: impl std::fmt::Display) -> Self {
        Self(message.to_string())
    }

    pub(crate) fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for HistoryError {}

impl From<std::io::Error> for HistoryError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error)
    }
}

impl From<serde_json::Error> for HistoryError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error)
    }
}

/// The history store (spec §13.1). Every write is idempotent by id.
#[async_trait]
pub(crate) trait HistoryStore: Send + Sync {
    /// `memory`, `sqlite`, or `postgres`.
    fn label(&self) -> &'static str;

    /// In-memory stores lose everything on restart, so hot-tail pruning stays off.
    fn is_ephemeral(&self) -> bool {
        false
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError>;

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError>;

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError>;

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError>;

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError>;

    /// Newest first, strictly older than `query.before`.
    async fn page_messages(&self, query: &MessagePageQuery)
        -> Result<Vec<HistoryMessage>, HistoryError>;

    /// Visible (non-hidden) messages per session.
    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError>;

    /// Visible messages of these agents in which every query word appears as
    /// a word prefix, newest first.
    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError>;

    /// Removes a session's messages, runs, and attachment records.
    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError>;
}

/// Lowercase query words, at most `MAX_SEARCH_TOKENS`.
pub(crate) fn search_tokens(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .take(MAX_SEARCH_TOKENS)
        .collect()
}

/// Whether every token occurs in `text`, ignoring case.
pub(crate) fn text_matches(text: &str, tokens: &[String]) -> bool {
    let lowered = text.to_lowercase();
    !tokens.is_empty() && tokens.iter().all(|token| lowered.contains(token.as_str()))
}

/// A single-line excerpt around the first matching token.
pub(crate) fn search_snippet(text: &str, tokens: &[String]) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = collapsed.chars().collect::<Vec<_>>();
    let lowered = chars
        .iter()
        .map(|character| character.to_lowercase().next().unwrap_or(*character))
        .collect::<Vec<_>>();
    let position = tokens
        .iter()
        .filter_map(|token| {
            let needle = token.chars().collect::<Vec<_>>();
            if needle.is_empty() || needle.len() > lowered.len() {
                return None;
            }
            lowered
                .windows(needle.len())
                .position(|window| window == needle.as_slice())
        })
        .min()
        .unwrap_or(0);
    let start = position.saturating_sub(MAX_SNIPPET_CHARS / 3);
    let end = (start + MAX_SNIPPET_CHARS).min(chars.len());
    let mut snippet = chars[start..end].iter().collect::<String>();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < chars.len() {
        snippet.push('…');
    }
    snippet
}
```

Put this above the test module in `hosts/rust-daemon/src/history/memory.rs`:

```rust
//! Bounded in-memory history store for ephemeral mode (spec §13.1): each
//! table keeps at most `EPHEMERAL_HISTORY_MAX_ROWS` rows, dropping the oldest.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;

use super::{
    search_tokens, text_matches, HistoryError, HistoryMessage, HistoryStore, MessagePageQuery,
    EPHEMERAL_HISTORY_MAX_ROWS,
};
use crate::runs::RunRecord;

pub(crate) struct MemoryHistoryStore {
    max_rows: usize,
    tables: Mutex<Tables>,
}

#[derive(Default)]
struct Tables {
    next_seq: u64,
    messages: HashMap<String, (u64, HistoryMessage)>,
    message_seqs: BTreeMap<u64, String>,
    runs: HashMap<String, (u64, RunRecord)>,
    run_seqs: BTreeMap<u64, String>,
}

impl MemoryHistoryStore {
    pub(crate) fn new() -> Self {
        Self::with_max_rows(EPHEMERAL_HISTORY_MAX_ROWS)
    }

    pub(crate) fn with_max_rows(max_rows: usize) -> Self {
        Self {
            max_rows: max_rows.max(1),
            tables: Mutex::new(Tables::default()),
        }
    }

    fn tables(&self) -> MutexGuard<'_, Tables> {
        self.tables
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Inserts or replaces a row by id; a new row evicts the oldest past the cap.
fn upsert<T>(
    rows: &mut HashMap<String, (u64, T)>,
    order: &mut BTreeMap<u64, String>,
    next_seq: &mut u64,
    id: String,
    value: T,
    max_rows: usize,
) {
    if let Some(existing) = rows.get_mut(&id) {
        existing.1 = value;
        return;
    }
    *next_seq += 1;
    order.insert(*next_seq, id.clone());
    rows.insert(id, (*next_seq, value));
    while rows.len() > max_rows {
        let Some((_, oldest)) = order.pop_first() else {
            break;
        };
        rows.remove(&oldest);
    }
}

fn remove_where<T>(
    rows: &mut HashMap<String, (u64, T)>,
    order: &mut BTreeMap<u64, String>,
    mut matches: impl FnMut(&T) -> bool,
) {
    let doomed = rows
        .iter()
        .filter(|(_, (_, value))| matches(value))
        .map(|(id, (seq, _))| (id.clone(), *seq))
        .collect::<Vec<_>>();
    for (id, seq) in doomed {
        rows.remove(&id);
        order.remove(&seq);
    }
}

#[async_trait]
impl HistoryStore for MemoryHistoryStore {
    fn label(&self) -> &'static str {
        "memory"
    }

    fn is_ephemeral(&self) -> bool {
        true
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        for row in messages {
            upsert(
                &mut tables.messages,
                &mut tables.message_seqs,
                &mut tables.next_seq,
                row.message.id.clone(),
                row.clone(),
                self.max_rows,
            );
        }
        Ok(())
    }

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        for run in runs {
            upsert(
                &mut tables.runs,
                &mut tables.run_seqs,
                &mut tables.next_seq,
                run.id.clone(),
                run.clone(),
                self.max_rows,
            );
        }
        Ok(())
    }

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError> {
        let tables = self.tables();
        Ok(ids
            .iter()
            .filter(|id| tables.messages.contains_key(*id))
            .cloned()
            .collect())
    }

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError> {
        Ok(self
            .tables()
            .messages
            .get(message_id)
            .map(|(_, row)| row)
            .filter(|row| row.agent_id == agent_id && row.session_id == session_id)
            .cloned())
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError> {
        Ok(self.tables().runs.get(run_id).map(|(_, run)| run.clone()))
    }

    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tables = self.tables();
        let mut rows = tables
            .messages
            .values()
            .map(|(_, row)| row)
            .filter(|row| {
                row.agent_id == query.agent_id
                    && row.session_id == query.session_id
                    && (query.include_hidden || !row.hidden)
                    && query
                        .before
                        .as_ref()
                        .is_none_or(|before| row.order() < *before)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.order().cmp(&left.order()));
        rows.truncate(query.limit);
        Ok(rows)
    }

    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError> {
        let wanted = session_ids.iter().collect::<HashSet<_>>();
        let mut counts = HashMap::new();
        for (_, row) in self.tables().messages.values() {
            if row.agent_id == agent_id && !row.hidden && wanted.contains(&row.session_id) {
                *counts.entry(row.session_id.clone()).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }

    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tokens = search_tokens(query);
        if tokens.is_empty() || agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        let tables = self.tables();
        let mut rows = tables
            .messages
            .values()
            .map(|(_, row)| row)
            .filter(|row| {
                !row.hidden
                    && agent_ids.contains(&row.agent_id)
                    && text_matches(&row.message.content.text, &tokens)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.order().cmp(&left.order()));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        remove_where(&mut tables.messages, &mut tables.message_seqs, |row| {
            row.agent_id == agent_id && row.session_id == session_id
        });
        remove_where(&mut tables.runs, &mut tables.run_seqs, |run| {
            run.agent_id == agent_id && run.session_id == session_id
        });
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::`
Expected: PASS (3 `history::tests` and 2 `history::memory::tests`).

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/history hosts/rust-daemon/src/lib.rs
git commit -m "feat(daemon): add the history store contract and the bounded in-memory store"
```

---

### Task 4: SQLite history store (WAL, FTS5, versioned schema)

**Files:**

- Create: `hosts/rust-daemon/src/history/sqlite.rs`
- Modify: `hosts/rust-daemon/src/history/mod.rs` (`mod sqlite;` and re-export)

**Interfaces:**

- Consumes: Task 3's `HistoryStore`, `HistoryMessage`, `MessagePageQuery`, `HistoryError`, `message_ordinal`, `search_tokens`, and `conformance::{assert_history_store_conformance, history_message}`.
- Produces: `crate::history::SqliteHistoryStore` with `async fn open(path: PathBuf) -> Result<Self, HistoryError>` (creates parent directories, enables WAL, `synchronous = FULL`, creates or checks the schema) and `path(&self) -> &Path`; label `"sqlite"`; `SQLITE_HISTORY_SCHEMA_VERSION` (1) in `PRAGMA user_version`; tables `messages` (+ external-content FTS5 `messages_fts` kept in sync by triggers), `runs`, `usage`, `approvals`, `schedule_runs`, `attachments` (the last four are created now and filled by M4/M6/M8/M9); `impl From<rusqlite::Error> for HistoryError`. A newer `user_version` is refused with `history store schema version <n> is newer than this daemon supports (1)`.

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/history/sqlite.rs` with only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::{assert_history_store_conformance, history_message};

    struct TempHistory(PathBuf);

    impl TempHistory {
        fn new(label: &str) -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("anima-history-{label}-{}", uuid::Uuid::new_v4()))
                    .join("history.sqlite"),
            )
        }
    }

    impl Drop for TempHistory {
        fn drop(&mut self) {
            if let Some(directory) = self.0.parent() {
                let _ = std::fs::remove_dir_all(directory);
            }
        }
    }

    #[tokio::test]
    async fn sqlite_store_meets_the_conformance_suite() {
        let temp = TempHistory::new("conformance");
        let store = SqliteHistoryStore::open(temp.0.clone())
            .await
            .expect("the store opens and creates its directory");
        assert_history_store_conformance(&store).await;
        assert_eq!(store.label(), "sqlite");
        assert!(!store.is_ephemeral());
    }

    #[tokio::test]
    async fn rows_survive_reopening_and_the_schema_is_versioned_in_wal_mode() {
        let temp = TempHistory::new("reopen");
        {
            let store = SqliteHistoryStore::open(temp.0.clone()).await.unwrap();
            store
                .upsert_messages(&[history_message(
                    "msg-5-1",
                    "agent-1",
                    "chat:a",
                    MessageRole::User,
                    "persisted words",
                    5,
                )])
                .await
                .unwrap();
        }
        let reopened = SqliteHistoryStore::open(temp.0.clone()).await.unwrap();
        assert_eq!(reopened.path(), temp.0.as_path());
        assert_eq!(
            reopened
                .search_messages(&["agent-1".to_string()], "persisted", 10)
                .await
                .unwrap()
                .len(),
            1,
            "rows and the FTS index survive a reopen"
        );
        let connection = Connection::open(&temp.0).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SQLITE_HISTORY_SCHEMA_VERSION);
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[tokio::test]
    async fn a_newer_schema_is_refused() {
        let temp = TempHistory::new("newer");
        std::fs::create_dir_all(temp.0.parent().unwrap()).unwrap();
        Connection::open(&temp.0)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let error = SqliteHistoryStore::open(temp.0.clone())
            .await
            .err()
            .expect("a newer schema must not be opened");
        assert_eq!(
            error.message(),
            "history store schema version 99 is newer than this daemon supports (1)"
        );
    }
}
```

In `hosts/rust-daemon/src/history/mod.rs`, add `mod sqlite;` after `mod memory;` and `pub(crate) use sqlite::SqliteHistoryStore;` after `pub(crate) use memory::MemoryHistoryStore;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::sqlite::tests`
Expected: compile errors `cannot find type SqliteHistoryStore` and `cannot find value SQLITE_HISTORY_SCHEMA_VERSION`.

- [ ] **Step 3: Implement**

Put this above the test module in `hosts/rust-daemon/src/history/sqlite.rs`:

```rust
//! SQLite history store (spec §13.1): WAL mode, FTS5 search, a versioned
//! schema, and one dedicated connection driven through `spawn_blocking`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anima_core::MessageRole;
use async_trait::async_trait;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use super::{
    message_ordinal, search_tokens, HistoryError, HistoryMessage, HistoryStore, MessagePageQuery,
};
use crate::runs::RunRecord;

/// `PRAGMA user_version` of the schema this daemon writes.
pub(crate) const SQLITE_HISTORY_SCHEMA_VERSION: i64 = 1;
const ID_CHUNK: usize = 500;

const SCHEMA_V1: &str = "
BEGIN;
CREATE TABLE messages (
    id TEXT NOT NULL PRIMARY KEY,
    agent_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    text TEXT NOT NULL,
    hidden INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    record TEXT NOT NULL
);
CREATE INDEX messages_session_order ON messages (agent_id, session_id, created_at_ms, ordinal, id);
CREATE VIRTUAL TABLE messages_fts USING fts5(text, content = 'messages', content_rowid = 'rowid');
CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts (rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER messages_fts_update AFTER UPDATE OF text ON messages BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO messages_fts (rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TABLE runs (
    id TEXT NOT NULL PRIMARY KEY,
    agent_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER,
    record TEXT NOT NULL
);
CREATE INDEX runs_session ON runs (agent_id, session_id, created_at_ms);
CREATE TABLE usage (
    id TEXT NOT NULL PRIMARY KEY,
    agent_id TEXT NOT NULL,
    session_id TEXT,
    run_id TEXT,
    created_at_ms INTEGER NOT NULL,
    record TEXT NOT NULL
);
CREATE INDEX usage_created ON usage (created_at_ms);
CREATE TABLE approvals (
    id TEXT NOT NULL PRIMARY KEY,
    agent_id TEXT NOT NULL,
    session_id TEXT,
    created_at_ms INTEGER NOT NULL,
    record TEXT NOT NULL
);
CREATE INDEX approvals_created ON approvals (agent_id, created_at_ms);
CREATE TABLE schedule_runs (
    id TEXT NOT NULL PRIMARY KEY,
    schedule_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    fired_at_ms INTEGER NOT NULL,
    record TEXT NOT NULL
);
CREATE INDEX schedule_runs_schedule ON schedule_runs (schedule_id, fired_at_ms);
CREATE TABLE attachments (
    id TEXT NOT NULL PRIMARY KEY,
    agent_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    record TEXT NOT NULL
);
CREATE INDEX attachments_session ON attachments (agent_id, session_id);
PRAGMA user_version = 1;
COMMIT;
";

const UPSERT_MESSAGE: &str = "
INSERT INTO messages (id, agent_id, session_id, role, text, hidden, created_at_ms, ordinal, record)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT (id) DO UPDATE SET
    agent_id = excluded.agent_id,
    session_id = excluded.session_id,
    role = excluded.role,
    text = excluded.text,
    hidden = excluded.hidden,
    created_at_ms = excluded.created_at_ms,
    ordinal = excluded.ordinal,
    record = excluded.record";

const UPSERT_RUN: &str = "
INSERT INTO runs (id, agent_id, session_id, status, created_at_ms, finished_at_ms, record)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
ON CONFLICT (id) DO UPDATE SET
    agent_id = excluded.agent_id,
    session_id = excluded.session_id,
    status = excluded.status,
    created_at_ms = excluded.created_at_ms,
    finished_at_ms = excluded.finished_at_ms,
    record = excluded.record";

const PAGE_MESSAGES: &str = "
SELECT agent_id, session_id, hidden, record FROM messages
WHERE agent_id = ?1 AND session_id = ?2 AND (?3 OR hidden = 0)
  AND (?4 IS NULL OR created_at_ms < ?4
       OR (created_at_ms = ?4 AND (ordinal < ?5 OR (ordinal = ?5 AND id < ?6))))
ORDER BY created_at_ms DESC, ordinal DESC, id DESC
LIMIT ?7";

impl From<rusqlite::Error> for HistoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new(error)
    }
}

pub(crate) struct SqliteHistoryStore {
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl SqliteHistoryStore {
    pub(crate) async fn open(path: PathBuf) -> Result<Self, HistoryError> {
        let opening = path.clone();
        let connection = tokio::task::spawn_blocking(move || open_connection(&opening))
            .await
            .map_err(|error| HistoryError::new(format!("history store worker stopped: {error}")))??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Runs `work` on the dedicated connection off the async runtime.
    async fn run<T, F>(&self, work: F) -> Result<T, HistoryError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, HistoryError> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let mut connection = connection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            work(&mut connection)
        })
        .await
        .map_err(|error| HistoryError::new(format!("history store worker stopped: {error}")))?
    }
}

fn open_connection(path: &Path) -> Result<Connection, HistoryError> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(HistoryError::new(format!(
            "history store could not enable WAL mode (got {mode})"
        )));
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    match version {
        0 => connection.execute_batch(SCHEMA_V1)?,
        SQLITE_HISTORY_SCHEMA_VERSION => {}
        newer => {
            return Err(HistoryError::new(format!(
                "history store schema version {newer} is newer than this daemon supports ({SQLITE_HISTORY_SCHEMA_VERSION})"
            )))
        }
    }
    Ok(connection)
}

fn to_i64(value: u64) -> Result<i64, HistoryError> {
    i64::try_from(value).map_err(|_| HistoryError::new("a timestamp or counter is out of range"))
}

fn role_name(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

fn placeholders(first: usize, count: usize) -> String {
    (first..first + count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

type RawRow = (String, String, bool, String);

fn raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn decode(rows: Vec<RawRow>) -> Result<Vec<HistoryMessage>, HistoryError> {
    rows.into_iter()
        .map(|(agent_id, session_id, hidden, record)| -> Result<HistoryMessage, HistoryError> {
            Ok(HistoryMessage {
                agent_id,
                session_id,
                hidden,
                message: serde_json::from_str(&record)?,
            })
        })
        .collect()
}

struct MessageRow {
    id: String,
    agent_id: String,
    session_id: String,
    role: &'static str,
    text: String,
    hidden: bool,
    created_at_ms: i64,
    ordinal: i64,
    record: String,
}

struct RunRow {
    id: String,
    agent_id: String,
    session_id: String,
    status: String,
    created_at_ms: i64,
    finished_at_ms: Option<i64>,
    record: String,
}

#[async_trait]
impl HistoryStore for SqliteHistoryStore {
    fn label(&self) -> &'static str {
        "sqlite"
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError> {
        if messages.is_empty() {
            return Ok(());
        }
        let rows = messages
            .iter()
            .map(|row| -> Result<MessageRow, HistoryError> {
                Ok(MessageRow {
                    id: row.message.id.clone(),
                    agent_id: row.agent_id.clone(),
                    session_id: row.session_id.clone(),
                    role: role_name(row.message.role),
                    text: row.message.content.text.clone(),
                    hidden: row.hidden,
                    created_at_ms: to_i64(row.message.created_at_ms)?,
                    ordinal: to_i64(message_ordinal(&row.message.id))?,
                    record: serde_json::to_string(&row.message)?,
                })
            })
            .collect::<Result<Vec<_>, HistoryError>>()?;
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            {
                let mut statement = transaction.prepare_cached(UPSERT_MESSAGE)?;
                for row in &rows {
                    statement.execute(params![
                        row.id,
                        row.agent_id,
                        row.session_id,
                        row.role,
                        row.text,
                        row.hidden,
                        row.created_at_ms,
                        row.ordinal,
                        row.record
                    ])?;
                }
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError> {
        if runs.is_empty() {
            return Ok(());
        }
        let rows = runs
            .iter()
            .map(|run| -> Result<RunRow, HistoryError> {
                Ok(RunRow {
                    id: run.id.clone(),
                    agent_id: run.agent_id.clone(),
                    session_id: run.session_id.clone(),
                    status: serde_json::to_value(run.status)?
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string(),
                    created_at_ms: to_i64(run.created_at_ms)?,
                    finished_at_ms: run.finished_at_ms.map(to_i64).transpose()?,
                    record: serde_json::to_string(run)?,
                })
            })
            .collect::<Result<Vec<_>, HistoryError>>()?;
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            {
                let mut statement = transaction.prepare_cached(UPSERT_RUN)?;
                for row in &rows {
                    statement.execute(params![
                        row.id,
                        row.agent_id,
                        row.session_id,
                        row.status,
                        row.created_at_ms,
                        row.finished_at_ms,
                        row.record
                    ])?;
                }
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError> {
        let ids = ids.to_vec();
        self.run(move |connection| {
            let mut found = HashSet::new();
            for chunk in ids.chunks(ID_CHUNK) {
                let sql = format!(
                    "SELECT id FROM messages WHERE id IN ({})",
                    placeholders(1, chunk.len())
                );
                let mut statement = connection.prepare(&sql)?;
                let rows =
                    statement.query_map(params_from_iter(chunk.iter()), |row| row.get::<_, String>(0))?;
                for row in rows {
                    found.insert(row?);
                }
            }
            Ok(found)
        })
        .await
    }

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError> {
        let (agent_id, session_id, message_id) =
            (agent_id.to_string(), session_id.to_string(), message_id.to_string());
        self.run(move |connection| {
            let row = connection
                .query_row(
                    "SELECT agent_id, session_id, hidden, record FROM messages
                     WHERE id = ?1 AND agent_id = ?2 AND session_id = ?3",
                    params![message_id, agent_id, session_id],
                    raw_row,
                )
                .optional()?;
            Ok(decode(row.into_iter().collect())?.pop())
        })
        .await
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError> {
        let run_id = run_id.to_string();
        self.run(move |connection| {
            let record = connection
                .query_row(
                    "SELECT record FROM runs WHERE id = ?1",
                    params![run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            record
                .map(|record| serde_json::from_str(&record).map_err(HistoryError::from))
                .transpose()
        })
        .await
    }

    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let query = query.clone();
        self.run(move |connection| {
            let before = query
                .before
                .as_ref()
                .map(|order| {
                    Ok::<_, HistoryError>((
                        to_i64(order.created_at_ms)?,
                        to_i64(order.ordinal)?,
                        order.id.clone(),
                    ))
                })
                .transpose()?;
            let mut statement = connection.prepare_cached(PAGE_MESSAGES)?;
            let rows = statement
                .query_map(
                    params![
                        query.agent_id,
                        query.session_id,
                        query.include_hidden,
                        before.as_ref().map(|before| before.0),
                        before.as_ref().map(|before| before.1),
                        before.as_ref().map(|before| before.2.as_str()),
                        i64::try_from(query.limit).unwrap_or(i64::MAX)
                    ],
                    raw_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            decode(rows)
        })
        .await
    }

    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError> {
        if session_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut values = vec![agent_id.to_string()];
        values.extend(session_ids.iter().cloned());
        self.run(move |connection| {
            let sql = format!(
                "SELECT session_id, COUNT(*) FROM messages
                 WHERE agent_id = ?1 AND hidden = 0 AND session_id IN ({})
                 GROUP BY session_id",
                placeholders(2, values.len() - 1)
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut counts = HashMap::new();
            for row in rows {
                let (session_id, count) = row?;
                counts.insert(session_id, usize::try_from(count).unwrap_or(0));
            }
            Ok(counts)
        })
        .await
    }

    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tokens = search_tokens(query);
        if tokens.is_empty() || agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        let fts = tokens
            .iter()
            .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        let mut values = vec![fts];
        values.extend(agent_ids.iter().cloned());
        self.run(move |connection| {
            let sql = format!(
                "SELECT messages.agent_id, messages.session_id, messages.hidden, messages.record
                 FROM messages_fts JOIN messages ON messages.rowid = messages_fts.rowid
                 WHERE messages_fts MATCH ?1 AND messages.hidden = 0 AND messages.agent_id IN ({})
                 ORDER BY messages.created_at_ms DESC, messages.ordinal DESC, messages.id DESC
                 LIMIT {limit}",
                placeholders(2, values.len() - 1)
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = statement
                .query_map(params_from_iter(values.iter()), raw_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            decode(rows)
        })
        .await
    }

    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError> {
        let (agent_id, session_id) = (agent_id.to_string(), session_id.to_string());
        self.run(move |connection| {
            let transaction = connection.transaction()?;
            for table in ["messages", "runs", "attachments"] {
                transaction.execute(
                    &format!("DELETE FROM {table} WHERE agent_id = ?1 AND session_id = ?2"),
                    params![agent_id, session_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::`
Expected: PASS (the 3 SQLite tests plus the Task 3 history tests).

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/history/sqlite.rs hosts/rust-daemon/src/history/mod.rs
git commit -m "feat(daemon): add the SQLite history store with WAL and full-text search"
```

---

### Task 5: Postgres history store and migration

**Files:**

- Create: `hosts/rust-daemon/migrations/20260923000000_history_store.sql`
- Create: `hosts/rust-daemon/src/history/postgres.rs`
- Modify: `hosts/rust-daemon/src/history/mod.rs` (`mod postgres;` and re-export)

**Interfaces:**

- Consumes: Task 3's contract and conformance suite.
- Produces: `crate::history::PostgresHistoryStore::new(pool: PgPool) -> Self` (label `"postgres"`); tables `history_messages` (generated `search tsvector` over `to_tsvector('simple', text)` with a GIN index), `history_runs`, `history_usage`, `history_approvals`, `history_schedule_runs`, `history_attachments` (the `history_` prefix keeps them apart from `step_log` and `host_snapshots` in the shared database; requires Postgres 12+ for the generated column); `impl From<sqlx::Error> for HistoryError`; `prefix_tsquery(&[String]) -> String` (`deploy:* & build:*`).

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/history/postgres.rs` with only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::assert_history_store_conformance;

    #[test]
    fn prefix_queries_join_every_word() {
        assert_eq!(
            prefix_tsquery(&["deploy".to_string(), "build".to_string()]),
            "deploy:* & build:*"
        );
    }

    #[ignore = "requires DATABASE_URL-backed Postgres"]
    #[sqlx::test(migrations = "./migrations")]
    async fn postgres_store_meets_the_conformance_suite(pool: PgPool) {
        let store = PostgresHistoryStore::new(pool);
        assert_history_store_conformance(&store).await;
        assert_eq!(store.label(), "postgres");
    }
}
```

In `hosts/rust-daemon/src/history/mod.rs`, add `mod postgres;` after `mod memory;` and `pub(crate) use postgres::PostgresHistoryStore;` after `pub(crate) use memory::MemoryHistoryStore;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::postgres::tests`
Expected: compile errors `cannot find function prefix_tsquery` and `cannot find type PostgresHistoryStore`.

- [ ] **Step 3: Implement**

Create `hosts/rust-daemon/migrations/20260923000000_history_store.sql`:

```sql
-- History store (companion console spec §13.1): committed session messages,
-- finished runs, and the tables later milestones fill (usage, approvals,
-- automation runs, attachments). The daemon's outbox writes every row
-- idempotently by id; `record` holds the full JSON record.

CREATE TABLE IF NOT EXISTS history_messages (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    role          TEXT NOT NULL,
    text          TEXT NOT NULL,
    hidden        BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms BIGINT NOT NULL,
    ordinal       BIGINT NOT NULL,
    record        JSONB NOT NULL,
    search        tsvector GENERATED ALWAYS AS (to_tsvector('simple', text)) STORED
);
CREATE INDEX IF NOT EXISTS history_messages_session_order_idx
    ON history_messages (agent_id, session_id, created_at_ms, ordinal, id);
CREATE INDEX IF NOT EXISTS history_messages_search_idx
    ON history_messages USING GIN (search);

CREATE TABLE IF NOT EXISTS history_runs (
    id             TEXT PRIMARY KEY,
    agent_id       TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    status         TEXT NOT NULL,
    created_at_ms  BIGINT NOT NULL,
    finished_at_ms BIGINT,
    record         JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_runs_session_idx
    ON history_runs (agent_id, session_id, created_at_ms);

CREATE TABLE IF NOT EXISTS history_usage (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT,
    run_id        TEXT,
    created_at_ms BIGINT NOT NULL,
    record        JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_usage_created_idx ON history_usage (created_at_ms);

CREATE TABLE IF NOT EXISTS history_approvals (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT,
    created_at_ms BIGINT NOT NULL,
    record        JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_approvals_created_idx
    ON history_approvals (agent_id, created_at_ms);

CREATE TABLE IF NOT EXISTS history_schedule_runs (
    id          TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL,
    agent_id    TEXT NOT NULL,
    fired_at_ms BIGINT NOT NULL,
    record      JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_schedule_runs_schedule_idx
    ON history_schedule_runs (schedule_id, fired_at_ms);

CREATE TABLE IF NOT EXISTS history_attachments (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    created_at_ms BIGINT NOT NULL,
    record        JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_attachments_session_idx
    ON history_attachments (agent_id, session_id);
```

Put this above the test module in `hosts/rust-daemon/src/history/postgres.rs`:

```rust
//! Postgres history store (spec §13.1) over the tables of migration
//! `20260923000000_history_store.sql`.

use std::collections::{HashMap, HashSet};

use anima_core::MessageRole;
use async_trait::async_trait;
use sqlx::{PgPool, Row};

use super::{
    message_ordinal, search_tokens, HistoryError, HistoryMessage, HistoryStore, MessagePageQuery,
};
use crate::runs::RunRecord;

const UPSERT_MESSAGE: &str = "
INSERT INTO history_messages (id, agent_id, session_id, role, text, hidden, created_at_ms, ordinal, record)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (id) DO UPDATE SET
    agent_id = EXCLUDED.agent_id,
    session_id = EXCLUDED.session_id,
    role = EXCLUDED.role,
    text = EXCLUDED.text,
    hidden = EXCLUDED.hidden,
    created_at_ms = EXCLUDED.created_at_ms,
    ordinal = EXCLUDED.ordinal,
    record = EXCLUDED.record";

const UPSERT_RUN: &str = "
INSERT INTO history_runs (id, agent_id, session_id, status, created_at_ms, finished_at_ms, record)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (id) DO UPDATE SET
    agent_id = EXCLUDED.agent_id,
    session_id = EXCLUDED.session_id,
    status = EXCLUDED.status,
    created_at_ms = EXCLUDED.created_at_ms,
    finished_at_ms = EXCLUDED.finished_at_ms,
    record = EXCLUDED.record";

impl From<sqlx::Error> for HistoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::new(error)
    }
}

pub(crate) struct PostgresHistoryStore {
    pool: PgPool,
}

impl PostgresHistoryStore {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// A prefix query in which every word must match: `deploy:* & build:*`.
pub(crate) fn prefix_tsquery(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| format!("{token}:*"))
        .collect::<Vec<_>>()
        .join(" & ")
}

fn to_i64(value: u64) -> Result<i64, HistoryError> {
    i64::try_from(value).map_err(|_| HistoryError::new("a timestamp or counter is out of range"))
}

fn role_name(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

fn decode_row(row: &sqlx::postgres::PgRow) -> Result<HistoryMessage, HistoryError> {
    let record: serde_json::Value = row.try_get("record")?;
    Ok(HistoryMessage {
        agent_id: row.try_get("agent_id")?,
        session_id: row.try_get("session_id")?,
        hidden: row.try_get("hidden")?,
        message: serde_json::from_value(record)?,
    })
}

#[async_trait]
impl HistoryStore for PostgresHistoryStore {
    fn label(&self) -> &'static str {
        "postgres"
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError> {
        if messages.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for row in messages {
            sqlx::query(UPSERT_MESSAGE)
                .bind(&row.message.id)
                .bind(&row.agent_id)
                .bind(&row.session_id)
                .bind(role_name(row.message.role))
                .bind(&row.message.content.text)
                .bind(row.hidden)
                .bind(to_i64(row.message.created_at_ms)?)
                .bind(to_i64(message_ordinal(&row.message.id))?)
                .bind(serde_json::to_value(&row.message)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError> {
        if runs.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for run in runs {
            let status = serde_json::to_value(run.status)?
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            sqlx::query(UPSERT_RUN)
                .bind(&run.id)
                .bind(&run.agent_id)
                .bind(&run.session_id)
                .bind(status)
                .bind(to_i64(run.created_at_ms)?)
                .bind(run.finished_at_ms.map(to_i64).transpose()?)
                .bind(serde_json::to_value(run)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows = sqlx::query("SELECT id FROM history_messages WHERE id = ANY($1)")
            .bind(ids.to_vec())
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| row.try_get::<String, _>("id").map_err(HistoryError::from))
            .collect()
    }

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError> {
        let row = sqlx::query(
            "SELECT agent_id, session_id, hidden, record FROM history_messages
             WHERE id = $1 AND agent_id = $2 AND session_id = $3",
        )
        .bind(message_id)
        .bind(agent_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(decode_row).transpose()
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError> {
        let row = sqlx::query("SELECT record FROM history_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| -> Result<RunRecord, HistoryError> {
            let record: serde_json::Value = row.try_get("record")?;
            Ok(serde_json::from_value(record)?)
        })
        .transpose()
    }

    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let limit = i64::try_from(query.limit).unwrap_or(i64::MAX);
        let rows = match &query.before {
            None => {
                sqlx::query(
                    "SELECT agent_id, session_id, hidden, record FROM history_messages
                     WHERE agent_id = $1 AND session_id = $2 AND ($3 OR NOT hidden)
                     ORDER BY created_at_ms DESC, ordinal DESC, id DESC
                     LIMIT $4",
                )
                .bind(&query.agent_id)
                .bind(&query.session_id)
                .bind(query.include_hidden)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            Some(before) => {
                sqlx::query(
                    "SELECT agent_id, session_id, hidden, record FROM history_messages
                     WHERE agent_id = $1 AND session_id = $2 AND ($3 OR NOT hidden)
                       AND (created_at_ms, ordinal, id) < ($5, $6, $7)
                     ORDER BY created_at_ms DESC, ordinal DESC, id DESC
                     LIMIT $4",
                )
                .bind(&query.agent_id)
                .bind(&query.session_id)
                .bind(query.include_hidden)
                .bind(limit)
                .bind(to_i64(before.created_at_ms)?)
                .bind(to_i64(before.ordinal)?)
                .bind(&before.id)
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(decode_row).collect()
    }

    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError> {
        if session_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query(
            "SELECT session_id, COUNT(*) AS total FROM history_messages
             WHERE agent_id = $1 AND NOT hidden AND session_id = ANY($2)
             GROUP BY session_id",
        )
        .bind(agent_id)
        .bind(session_ids.to_vec())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| -> Result<(String, usize), HistoryError> {
                let total: i64 = row.try_get("total")?;
                Ok((
                    row.try_get::<String, _>("session_id")?,
                    usize::try_from(total).unwrap_or(0),
                ))
            })
            .collect()
    }

    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tokens = search_tokens(query);
        if tokens.is_empty() || agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT agent_id, session_id, hidden, record FROM history_messages
             WHERE search @@ to_tsquery('simple', $1) AND NOT hidden AND agent_id = ANY($2)
             ORDER BY created_at_ms DESC, ordinal DESC, id DESC
             LIMIT $3",
        )
        .bind(prefix_tsquery(&tokens))
        .bind(agent_ids.to_vec())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode_row).collect()
    }

    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError> {
        let mut transaction = self.pool.begin().await?;
        for table in ["history_messages", "history_runs", "history_attachments"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE agent_id = $1 AND session_id = $2"
            ))
            .bind(agent_id)
            .bind(session_id)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history::`
Expected: PASS; `postgres_store_meets_the_conformance_suite` is reported as ignored. With a Postgres available, also run `DATABASE_URL=postgres://… CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- --ignored history::postgres` (expected: PASS); record in the task report whether it was run.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/migrations/20260923000000_history_store.sql hosts/rust-daemon/src/history/postgres.rs hosts/rust-daemon/src/history/mod.rs
git commit -m "feat(daemon): add the Postgres history store and its migration"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. Document in `hosts/rust-daemon/README.md` (Postgres persistence) that the history tables require Postgres 12+, and that rolling back to a pre-M2 daemon in Postgres mode also requires `DELETE FROM _sqlx_migrations WHERE version = 20260923000000;` (optionally dropping the `history_*` tables) besides restoring `control_plane.backup.<version>`, because sqlx refuses to start when an applied migration is missing (`VersionMissing`).

---

### Task 6: History outbox, worker, readiness, and wiring

**Files:**

- Create: `hosts/rust-daemon/src/history/outbox.rs`
- Modify: `hosts/rust-daemon/src/history/mod.rs` (`mod outbox;` and re-exports), `hosts/rust-daemon/src/history/conformance.rs` (`FlakyHistoryStore`)
- Modify: `hosts/rust-daemon/src/runs/ledger.rs` (`prune` only once mirrored, `unmirrored_terminal`, `mark_mirrored`, tests)
- Modify: `hosts/rust-daemon/src/state.rs` (`history` field, init, `set_history`)
- Modify: `hosts/rust-daemon/src/agent_runs.rs` (Phase C enqueue after the final save; one test)
- Modify: `hosts/rust-daemon/src/app/persistence.rs` (history store selection and configuration; tests)
- Modify: `hosts/rust-daemon/src/app.rs` (`DaemonRuntime::history`, start and shutdown)
- Modify: `hosts/rust-daemon/src/routes/health.rs` (readiness issue; test)
- Modify: `hosts/rust-daemon/README.md` (env table row)

**Interfaces:**

- Consumes: Tasks 3–5 stores; `crate::sessions::{hidden_message_ids, session_id_for_room}` (Task 2); `AgentRunCoordinator::control_plane_transactions()` (existing).
- Produces (in `crate::history`):
  - constants `HISTORY_FLUSH_INTERVAL` (1 s), `HISTORY_FLUSH_BATCH` (500), `HISTORY_RUN_BATCH` (200), `HISTORY_MAX_BACKOFF` (30 s), `HISTORY_READINESS_GRACE_MS` (5 min), `MAX_OUTBOX_ITEMS` (100,000);
  - `HistoryService` / `SharedHistory = Arc<HistoryService>` with `new(Arc<dyn HistoryStore>) -> SharedHistory`, `with_capacity(store, max_items) -> SharedHistory`, `ephemeral() -> SharedHistory`, `store() -> Arc<dyn HistoryStore>`, `is_ephemeral()`, `reconciled()`, `enqueue_committed(agent_id, session_id, &[Message])` (computes `hidden`), `enqueue_session_deletion(agent_id, session_id)`, `is_mirrored(&str) -> bool`, `forget_mirrored(impl IntoIterator<Item = &str>)`, `pending_count() -> usize`, `retry_delay() -> Duration`, `readiness_issue(now_ms) -> Option<String>` (text starts `history store writes have failed for <n> minutes; records stay in the control plane until it recovers (<error>)`), `async flush_once(&SharedDaemonState, now_ms) -> Result<FlushReport, HistoryError>`;
  - `FlushReport { messages, runs, deletions, reconciled }`;
  - `HistoryWorker` (`Clone`): `new(SharedDaemonState, Arc<tokio::sync::Mutex<()>>)`, `start(&self)` (needs a Tokio runtime; second call is a no-op), `async shutdown(&self)` (stops the loop, one final flush). Task 13 adds pruning to its loop and uses the stored transaction mutex;
  - test-only `conformance::FlakyHistoryStore::{new, set_failing}` (label `"flaky"`, not ephemeral).
- Produces: `DaemonState::history: SharedHistory` (default `HistoryService::ephemeral()`), `DaemonState::set_history(SharedHistory)`; `RunLedger::unmirrored_terminal(limit) -> Vec<RunRecord>` (oldest finished first), `RunLedger::mark_mirrored(&[RunRecord]) -> usize` (marks only records still equal to what was written); `RunLedger::prune` removes a terminal run only when `mirrored` (M1 carry-forward); `app::persistence::{HISTORY_SQLITE_FILE_ENV, history_store_for, default_history_sqlite_path}`; `GET /api/ready` reports the history readiness issue.
- Behavior: `run_locked` enqueues a run's messages only after its final control-plane save succeeded. The worker starts in `serve_with_state`, `app_with_configured_persistence`, and (when a Tokio runtime is present) `app_with_state`, and flushes once more on graceful shutdown. Store selection: JSON control plane → SQLite at `ANIMAOS_RS_HISTORY_SQLITE_FILE` or `history.sqlite` beside the control-plane file; Postgres control plane → Postgres tables unless the variable is set (then SQLite there); no durable control plane → memory (the variable is ignored with a warning). A store that cannot be opened fails startup like the other stores.

- [ ] **Step 1: Write the failing tests**

Append to `hosts/rust-daemon/src/history/conformance.rs`:

```rust
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use super::{HistoryError, MemoryHistoryStore};

/// A memory store whose every call fails while `failing` is set. Unlike the
/// memory store it is not ephemeral, so pruning tests can use it.
pub(crate) struct FlakyHistoryStore {
    inner: MemoryHistoryStore,
    failing: AtomicBool,
}

impl FlakyHistoryStore {
    pub(crate) fn new() -> Self {
        Self {
            inner: MemoryHistoryStore::new(),
            failing: AtomicBool::new(false),
        }
    }

    pub(crate) fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), HistoryError> {
        if self.failing.load(Ordering::SeqCst) {
            Err(HistoryError::new("injected history store failure"))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl HistoryStore for FlakyHistoryStore {
    fn label(&self) -> &'static str {
        "flaky"
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.upsert_messages(messages).await
    }

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.upsert_runs(runs).await
    }

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError> {
        self.check()?;
        self.inner.existing_message_ids(ids).await
    }

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError> {
        self.check()?;
        self.inner.get_message(agent_id, session_id, message_id).await
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError> {
        self.check()?;
        self.inner.get_run(run_id).await
    }

    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        self.check()?;
        self.inner.page_messages(query).await
    }

    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<std::collections::HashMap<String, usize>, HistoryError> {
        self.check()?;
        self.inner.visible_message_counts(agent_id, session_ids).await
    }

    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        self.check()?;
        self.inner.search_messages(agent_ids, query, limit).await
    }

    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.delete_session(agent_id, session_id).await
    }
}
```

Create `hosts/rust-daemon/src/history/outbox.rs` with only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::MessagePageQuery;
    use crate::runs::RunSource;
    use anima_core::{AgentConfig, AgentSettings, Content, MessageRole};
    use tokio::sync::{RwLock, Semaphore};

    fn config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }

    fn request(agent_id: &str, room: &str, text: &str) -> AgentRunRequest {
        AgentRunRequest {
            agent_id: agent_id.into(),
            content: Content {
                text: text.into(),
                ..Content::default()
            },
            room: RunRoom::Stable(room.into()),
            idempotency_key: None,
            source: RunSource::Api,
            source_ref: None,
        }
    }

    fn page(agent_id: &str, session_id: &str) -> MessagePageQuery {
        MessagePageQuery {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            before: None,
            limit: 100,
            include_hidden: true,
        }
    }

    async fn state_with(history: SharedHistory) -> (SharedDaemonState, AgentRunCoordinator, String) {
        let mut daemon = DaemonState::new();
        daemon.set_history(history);
        let agent_id = daemon.create_agent(config("historian")).unwrap().state.id;
        let state = Arc::new(tokio::sync::RwLock::new(daemon));
        let coordinator = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
        (state, coordinator, agent_id)
    }

    #[tokio::test]
    async fn committed_turns_and_terminal_runs_reach_the_store_and_runs_are_marked_mirrored() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        coordinator.run(request(&agent_id, "chat:one", "hello")).await.unwrap();
        let history = state.read().await.history.clone();
        assert_eq!(history.pending_count(), 2, "the committed turn waits in the outbox");

        let report = history.flush_once(&state, now_millis()).await.unwrap();
        assert_eq!(
            report,
            FlushReport {
                messages: 2,
                runs: 1,
                deletions: 0,
                reconciled: 0
            }
        );
        assert_eq!(history.pending_count(), 0);
        assert!(history.reconciled());
        let rows = store.page_messages(&page(&agent_id, "chat:one")).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| history.is_mirrored(&row.message.id)));
        let run = state.read().await.runs.for_agent(&agent_id)[0].clone();
        assert!(run.mirrored);
        let mut stored = store
            .get_run(&run.id)
            .await
            .unwrap()
            .expect("the terminal run is stored");
        stored.mirrored = true;
        assert_eq!(stored, run);
        assert_eq!(
            history.flush_once(&state, now_millis()).await.unwrap(),
            FlushReport::default(),
            "nothing is written twice"
        );
    }

    #[tokio::test]
    async fn a_failing_store_keeps_records_until_it_recovers_and_reports_after_five_minutes() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        coordinator.run(request(&agent_id, "chat:one", "hello")).await.unwrap();
        let history = state.read().await.history.clone();
        store.set_failing(true);
        let started = 1_000_000;

        assert!(history.flush_once(&state, started).await.is_err());
        assert_eq!(history.pending_count(), 2);
        assert!(!state.read().await.runs.for_agent(&agent_id)[0].mirrored);
        assert_eq!(
            history.readiness_issue(started + HISTORY_READINESS_GRACE_MS - 1),
            None
        );
        assert!(history.flush_once(&state, started + 60_000).await.is_err());
        assert_eq!(history.retry_delay(), HISTORY_FLUSH_INTERVAL * 2);
        let issue = history
            .readiness_issue(started + HISTORY_READINESS_GRACE_MS)
            .expect("five minutes of failures is a readiness issue");
        assert!(
            issue.starts_with("history store writes have failed for 5 minutes; records stay in the control plane until it recovers (injected history store failure)"),
            "{issue}"
        );

        store.set_failing(false);
        let report = history
            .flush_once(&state, started + HISTORY_READINESS_GRACE_MS + 1)
            .await
            .unwrap();
        assert_eq!((report.messages, report.runs), (2, 1));
        assert_eq!(history.readiness_issue(started + 2 * HISTORY_READINESS_GRACE_MS), None);
        assert_eq!(history.retry_delay(), HISTORY_FLUSH_INTERVAL);
        assert!(state.read().await.runs.for_agent(&agent_id)[0].mirrored);
    }

    #[tokio::test]
    async fn a_restart_mirrors_hot_messages_the_store_is_missing() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        coordinator
            .run(request(&agent_id, "chat:one", "before the crash"))
            .await
            .unwrap();
        // The process died before the outbox flushed: a fresh service, same store.
        let restarted = HistoryService::new(store.clone());
        state.write().await.set_history(Arc::clone(&restarted));
        assert!(!restarted.reconciled());

        let report = restarted.flush_once(&state, now_millis()).await.unwrap();
        assert_eq!((report.reconciled, report.messages, report.runs), (2, 2, 1));
        assert!(restarted.reconciled());
        assert_eq!(store.page_messages(&page(&agent_id, "chat:one")).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_overflowing_queue_falls_back_to_reconciling_the_hot_transcript() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) =
            state_with(HistoryService::with_capacity(store.clone(), 3)).await;
        coordinator.run(request(&agent_id, "chat:one", "first")).await.unwrap();
        let history = state.read().await.history.clone();
        history.flush_once(&state, now_millis()).await.unwrap();

        coordinator.run(request(&agent_id, "chat:one", "second")).await.unwrap();
        coordinator.run(request(&agent_id, "chat:one", "third")).await.unwrap();
        assert_eq!(history.pending_count(), 0, "four queued messages overflowed three slots");

        let report = history.flush_once(&state, now_millis()).await.unwrap();
        assert_eq!((report.reconciled, report.messages), (4, 4));
        assert_eq!(store.page_messages(&page(&agent_id, "chat:one")).await.unwrap().len(), 6);
    }

    #[tokio::test]
    async fn a_session_deletion_removes_the_rows_queued_before_it() {
        let store = Arc::new(MemoryHistoryStore::new());
        let history = HistoryService::new(store.clone());
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let turn = [
            history_message("msg-1-1", "agent-1", "chat:one", MessageRole::User, "hello", 1).message,
            history_message("msg-2-2", "agent-1", "chat:one", MessageRole::Assistant, "hi", 2).message,
        ];
        history.enqueue_committed("agent-1", "chat:one", &turn);
        history.enqueue_session_deletion("agent-1", "chat:one");
        history.enqueue_committed(
            "agent-1",
            "chat:two",
            &[history_message("msg-3-3", "agent-1", "chat:two", MessageRole::User, "other", 3).message],
        );

        let report = history.flush_once(&state, now_millis()).await.unwrap();

        assert_eq!((report.messages, report.deletions), (3, 1));
        assert!(store.page_messages(&page("agent-1", "chat:one")).await.unwrap().is_empty());
        assert_eq!(store.page_messages(&page("agent-1", "chat:two")).await.unwrap().len(), 1);
        assert_eq!(history.pending_count(), 0);
    }

    #[tokio::test]
    async fn silent_checkin_turns_are_stored_hidden() {
        let store = Arc::new(MemoryHistoryStore::new());
        let history = HistoryService::new(store.clone());
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let mut prompt =
            history_message("msg-1-1", "agent-1", "schedule:s1", MessageRole::User, "Check", 1).message;
        prompt.content.metadata = Some(std::collections::BTreeMap::from([(
            "kind".to_string(),
            anima_core::DataValue::String("checkin".into()),
        )]));
        let reply =
            history_message("msg-2-2", "agent-1", "schedule:s1", MessageRole::Assistant, "CHECKIN_OK", 2).message;
        history.enqueue_committed("agent-1", "schedule:s1", &[prompt, reply]);
        history.flush_once(&state, now_millis()).await.unwrap();

        let rows = store.page_messages(&page("agent-1", "schedule:s1")).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.hidden));
    }
}
```

Replace the test `retention_keeps_in_flight_runs_and_the_newest_terminal_runs_of_the_last_day` in `hosts/rust-daemon/src/runs/ledger.rs` with the following three tests, and add the `mirrored` helper after the `finished` helper:

```rust
    fn mirrored(agent_id: &str, at_ms: u64) -> RunRecord {
        let mut record = finished(agent_id, at_ms);
        record.mirrored = true;
        record
    }
```

```rust
    #[test]
    fn retention_keeps_in_flight_runs_and_the_newest_mirrored_terminal_runs_of_the_last_day() {
        let now = 10 * TERMINAL_RUN_RETENTION_MS;
        let mut ledger = RunLedger::default();
        let old_running = record("agent-a", now - 2 * TERMINAL_RUN_RETENTION_MS);
        ledger.insert(old_running.clone());
        let stale = mirrored("agent-a", now - TERMINAL_RUN_RETENTION_MS - 1);
        ledger.insert(stale.clone());
        let mut recent = Vec::new();
        for offset in 0..(MAX_TERMINAL_RUNS_PER_AGENT as u64 + 5) {
            let run = mirrored("agent-a", now - offset);
            recent.push(run.id.clone());
            ledger.insert(run);
        }
        let other = mirrored("agent-b", now - 10);
        ledger.insert(other.clone());

        ledger.prune(now);

        assert!(
            ledger.get(&old_running.id).is_some(),
            "in-flight runs are never pruned"
        );
        assert!(
            ledger.get(&stale.id).is_none(),
            "terminal runs older than a day are pruned"
        );
        assert!(ledger.get(&other.id).is_some(), "limits apply per agent");
        let kept = recent.iter().filter(|id| ledger.get(id).is_some()).count();
        assert_eq!(kept, MAX_TERMINAL_RUNS_PER_AGENT);
        assert!(ledger.get(&recent[0]).is_some(), "the newest run is kept");
        assert!(
            ledger.get(recent.last().unwrap()).is_none(),
            "the oldest excess run is pruned"
        );
    }

    #[test]
    fn terminal_runs_leave_the_control_plane_only_once_mirrored() {
        let now = 10 * TERMINAL_RUN_RETENTION_MS;
        let mut ledger = RunLedger::default();
        let stale = finished("agent-a", now - 2 * TERMINAL_RUN_RETENTION_MS);
        ledger.insert(stale.clone());
        let mut excess = Vec::new();
        for offset in 0..(MAX_TERMINAL_RUNS_PER_AGENT as u64 + 3) {
            let run = finished("agent-a", now - offset);
            excess.push(run.id.clone());
            ledger.insert(run);
        }

        ledger.prune(now);
        assert!(
            ledger.get(&stale.id).is_some(),
            "an old run waits for the history store"
        );
        assert!(
            excess.iter().all(|id| ledger.get(id).is_some()),
            "so do runs beyond the count limit"
        );

        ledger.get_mut(&stale.id).unwrap().mirrored = true;
        ledger.prune(now);
        assert!(ledger.get(&stale.id).is_none());
    }

    #[test]
    fn unmirrored_terminal_runs_are_listed_oldest_first_and_only_unchanged_records_are_marked() {
        let mut ledger = RunLedger::default();
        let running = record("agent-a", 1);
        let newer = finished("agent-a", 30);
        let older = finished("agent-a", 20);
        let already = mirrored("agent-a", 10);
        for run in [running.clone(), newer.clone(), older.clone(), already] {
            ledger.insert(run);
        }

        let pending = ledger.unmirrored_terminal(10);
        assert_eq!(
            pending.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
            [older.id.as_str(), newer.id.as_str()]
        );
        assert_eq!(ledger.unmirrored_terminal(1).len(), 1);

        // `newer` changes after it was read (a rolled-back commit, say).
        ledger.get_mut(&newer.id).unwrap().finish(
            RunStatus::Failed,
            Some(RunError::new(COMMIT_FAILED, "disk full")),
            31,
        );
        assert_eq!(ledger.mark_mirrored(&pending), 1);
        assert!(ledger.get(&older.id).unwrap().mirrored);
        assert!(
            !ledger.get(&newer.id).unwrap().mirrored,
            "a changed record is written again"
        );
        assert_eq!(
            ledger
                .unmirrored_terminal(10)
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            [newer.id.as_str()]
        );
        assert!(!ledger.get(&running.id).unwrap().mirrored);
    }
```

Append to `mod tests` in `hosts/rust-daemon/src/agent_runs.rs`:

```rust
    #[tokio::test]
    async fn only_durable_commits_reach_the_history_outbox() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let (coordinator, agent_id) = coordinator_with_agent(
            Arc::new(GateModelAdapter {
                calls: AtomicUsize::new(0),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
            4,
        )
        .await;
        release.add_permits(1);
        coordinator
            .run(room_request(&agent_id, "room-kept", "kept turn"))
            .await
            .expect("an ordinary run commits");
        entered.acquire().await.unwrap().forget();
        let history = coordinator.state.read().await.history.clone();
        assert_eq!(history.pending_count(), 2, "the committed turn is queued");

        let failed = {
            let coordinator = coordinator.clone();
            let request = room_request(&agent_id, "room-lost", "lost turn");
            tokio::spawn(async move { coordinator.run(request).await })
        };
        fail_the_next_final_save(&coordinator, &entered, &release).await;
        failed
            .await
            .unwrap()
            .expect_err("the final save failed");

        assert_eq!(
            history.pending_count(),
            2,
            "a commit whose save failed never reaches the history store"
        );
    }
```

Append to `hosts/rust-daemon/src/app/persistence.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("anima-persistence-{label}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn the_default_history_file_sits_beside_the_control_plane_file() {
        assert_eq!(
            default_history_sqlite_path(Path::new("/data/control-plane.json")),
            PathBuf::from("/data/history.sqlite")
        );
        assert_eq!(
            default_history_sqlite_path(Path::new("control-plane.json")),
            PathBuf::from("history.sqlite")
        );
    }

    #[tokio::test]
    async fn the_history_store_follows_the_control_plane_store() {
        let dir = temp_dir("history");
        let control = ControlPlaneStoreConfig::Json(dir.join("control-plane.json"));

        let beside = history_store_for(Some(&control), None).await.unwrap();
        assert_eq!(beside.label(), "sqlite");
        assert!(dir.join("history.sqlite").exists());
        let custom = dir.join("custom").join("history.db");
        let chosen = history_store_for(Some(&control), Some(custom.clone()))
            .await
            .unwrap();
        assert_eq!(chosen.label(), "sqlite");
        assert!(custom.exists());
        let ephemeral = history_store_for(None, Some(custom)).await.unwrap();
        assert!(
            ephemeral.is_ephemeral(),
            "without a durable control plane history stays in memory"
        );
        assert_eq!(history_store_for(None, None).await.unwrap().label(), "memory");
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://anima@127.0.0.1:1/anima")
            .unwrap();
        assert_eq!(
            history_store_for(Some(&ControlPlaneStoreConfig::Postgres(pool)), None)
                .await
                .unwrap()
                .label(),
            "postgres"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
```

Append to `hosts/rust-daemon/src/routes/health.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::{HistoryService, HISTORY_READINESS_GRACE_MS};
    use crate::state::DaemonState;
    use anima_core::MessageRole;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn readiness_reports_a_history_store_that_has_failed_for_five_minutes() {
        let store = Arc::new(FlakyHistoryStore::new());
        store.set_failing(true);
        let history = HistoryService::new(store);
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let config = DaemonConfig::default();
        assert_eq!(handle_readiness(&state, &config).await.status, "ready");

        history.enqueue_committed(
            "agent-1",
            "chat:one",
            &[history_message("msg-1-1", "agent-1", "chat:one", MessageRole::User, "hi", 1).message],
        );
        let long_ago = anima_core::primitives::now_millis() - HISTORY_READINESS_GRACE_MS - 1_000;
        assert!(history.flush_once(&state, long_ago).await.is_err());

        let response = handle_readiness(&state, &config).await;
        assert_eq!(response.status, "not_ready");
        assert!(
            response
                .issues
                .iter()
                .any(|issue| issue.starts_with("history store writes have failed for 5 minutes")),
            "{:?}",
            response.issues
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history:: runs::ledger::tests agent_runs::tests::only_durable_commits app::persistence::tests routes::health::tests`
Expected: compile errors such as `cannot find type HistoryService`, `no method named set_history`, `no method named unmirrored_terminal`, and `cannot find function history_store_for`.

- [ ] **Step 3: Implement the outbox and worker**

Put this above the test module in `hosts/rust-daemon/src/history/outbox.rs`:

```rust
//! History outbox (spec §13.1): committed messages and terminal runs reach the
//! history store within about a second, in batches, idempotently by id, with
//! retries and backoff. Records stay in the control plane until mirrored.
//! After a restart or a queue overflow the hot transcript is reconciled
//! against the store; five minutes of failures become a readiness issue.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use anima_core::primitives::now_millis;
use anima_core::Message;
use tokio::sync::{watch, Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::warn;

use super::{HistoryError, HistoryMessage, HistoryStore, MemoryHistoryStore};
use crate::app::SharedDaemonState;
use crate::sessions::{hidden_message_ids, session_id_for_room};
use crate::state::DaemonState;

/// The outbox flushes at least this often (spec §13.1).
pub(crate) const HISTORY_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// Messages per store write.
pub(crate) const HISTORY_FLUSH_BATCH: usize = 500;
/// Terminal runs per store write.
pub(crate) const HISTORY_RUN_BATCH: usize = 200;
/// The longest wait between retries while the store fails.
pub(crate) const HISTORY_MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Failing this long is a readiness issue (spec §13.1).
pub(crate) const HISTORY_READINESS_GRACE_MS: u64 = 5 * 60 * 1000;
/// Queued items before the queue gives way to a reconciliation.
pub(crate) const MAX_OUTBOX_ITEMS: usize = 100_000;

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug)]
enum OutboxItem {
    Message(HistoryMessage),
    DeleteSession { agent_id: String, session_id: String },
}

#[derive(Debug)]
struct Queued {
    seq: u64,
    item: OutboxItem,
}

#[derive(Debug, Default)]
struct OutboxState {
    next_seq: u64,
    items: VecDeque<Queued>,
    /// The queue overflowed and dropped its message copies; the next flush
    /// reads the hot transcript for what is still unmirrored.
    needs_reconcile: bool,
    failing_since_ms: Option<u64>,
    consecutive_failures: u32,
    last_error: Option<String>,
}

impl OutboxState {
    fn push(&mut self, item: OutboxItem) {
        self.next_seq += 1;
        self.items.push_back(Queued {
            seq: self.next_seq,
            item,
        });
    }
}

enum Batch {
    Empty,
    Messages { through: u64, rows: Vec<HistoryMessage> },
    Deletion { through: u64, agent_id: String, session_id: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FlushReport {
    pub(crate) messages: usize,
    pub(crate) runs: usize,
    pub(crate) deletions: usize,
    pub(crate) reconciled: usize,
}

pub(crate) struct HistoryService {
    store: Arc<dyn HistoryStore>,
    max_items: usize,
    outbox: StdMutex<OutboxState>,
    /// Hot message ids the store is known to hold; only these may be pruned.
    mirrored: StdMutex<HashSet<String>>,
    reconciled: AtomicBool,
    wake: Notify,
    flushing: Mutex<()>,
}

pub(crate) type SharedHistory = Arc<HistoryService>;

impl HistoryService {
    pub(crate) fn new(store: Arc<dyn HistoryStore>) -> SharedHistory {
        Self::with_capacity(store, MAX_OUTBOX_ITEMS)
    }

    pub(crate) fn with_capacity(store: Arc<dyn HistoryStore>, max_items: usize) -> SharedHistory {
        Arc::new(Self {
            store,
            max_items: max_items.max(1),
            outbox: StdMutex::new(OutboxState::default()),
            mirrored: StdMutex::new(HashSet::new()),
            reconciled: AtomicBool::new(false),
            wake: Notify::new(),
            flushing: Mutex::new(()),
        })
    }

    /// The ephemeral default: bounded in-memory tables (spec §13.1).
    pub(crate) fn ephemeral() -> SharedHistory {
        Self::new(Arc::new(MemoryHistoryStore::new()))
    }

    pub(crate) fn store(&self) -> Arc<dyn HistoryStore> {
        Arc::clone(&self.store)
    }

    pub(crate) fn is_ephemeral(&self) -> bool {
        self.store.is_ephemeral()
    }

    /// The hot transcript has been checked against the store since startup.
    pub(crate) fn reconciled(&self) -> bool {
        self.reconciled.load(Ordering::Acquire)
    }

    fn outbox(&self) -> MutexGuard<'_, OutboxState> {
        lock(&self.outbox)
    }

    fn mirrored(&self) -> MutexGuard<'_, HashSet<String>> {
        lock(&self.mirrored)
    }

    /// Queues one durable commit's messages. Call only after the control-plane
    /// save that made them durable succeeded.
    pub(crate) fn enqueue_committed(&self, agent_id: &str, session_id: &str, messages: &[Message]) {
        if messages.is_empty() {
            return;
        }
        let hidden = hidden_message_ids(messages.iter());
        {
            let mut outbox = self.outbox();
            for message in messages {
                outbox.push(OutboxItem::Message(HistoryMessage {
                    agent_id: agent_id.to_string(),
                    session_id: session_id.to_string(),
                    hidden: hidden.contains(&message.id),
                    message: message.clone(),
                }));
            }
            self.enforce_capacity(&mut outbox);
        }
        self.wake.notify_one();
    }

    /// Queues the removal of a deleted session's rows, after its deletion was saved.
    pub(crate) fn enqueue_session_deletion(&self, agent_id: &str, session_id: &str) {
        {
            let mut outbox = self.outbox();
            outbox.push(OutboxItem::DeleteSession {
                agent_id: agent_id.to_string(),
                session_id: session_id.to_string(),
            });
            self.enforce_capacity(&mut outbox);
        }
        self.wake.notify_one();
    }

    fn enforce_capacity(&self, outbox: &mut OutboxState) {
        if outbox.items.len() > self.max_items {
            // The hot transcript still holds every message; deletions must survive.
            outbox
                .items
                .retain(|queued| matches!(queued.item, OutboxItem::DeleteSession { .. }));
            outbox.needs_reconcile = true;
        }
    }

    pub(crate) fn is_mirrored(&self, message_id: &str) -> bool {
        self.mirrored().contains(message_id)
    }

    /// Drops ids that are no longer hot (pruned or deleted).
    pub(crate) fn forget_mirrored<'a>(&self, message_ids: impl IntoIterator<Item = &'a str>) {
        let mut mirrored = self.mirrored();
        for id in message_ids {
            mirrored.remove(id);
        }
    }

    fn mark_mirrored<'a>(&self, message_ids: impl IntoIterator<Item = &'a str>) {
        let mut mirrored = self.mirrored();
        for id in message_ids {
            mirrored.insert(id.to_string());
        }
    }

    /// Queued items not yet written.
    pub(crate) fn pending_count(&self) -> usize {
        self.outbox().items.len()
    }

    fn is_failing(&self) -> bool {
        self.outbox().failing_since_ms.is_some()
    }

    /// The wait before the next attempt: the flush interval, doubled per
    /// consecutive failure up to `HISTORY_MAX_BACKOFF`.
    pub(crate) fn retry_delay(&self) -> Duration {
        let failures = self.outbox().consecutive_failures;
        if failures == 0 {
            return HISTORY_FLUSH_INTERVAL;
        }
        HISTORY_FLUSH_INTERVAL
            .saturating_mul(2u32.saturating_pow(failures - 1))
            .min(HISTORY_MAX_BACKOFF)
    }

    pub(crate) fn readiness_issue(&self, now_ms: u64) -> Option<String> {
        let outbox = self.outbox();
        let since = outbox.failing_since_ms?;
        let failing_for = now_ms.saturating_sub(since);
        (failing_for >= HISTORY_READINESS_GRACE_MS).then(|| {
            format!(
                "history store writes have failed for {} minutes; records stay in the control plane until it recovers ({})",
                failing_for / 60_000,
                outbox.last_error.as_deref().unwrap_or("unknown error")
            )
        })
    }

    async fn wait_for_work(&self) {
        if self.is_failing() {
            tokio::time::sleep(self.retry_delay()).await;
        } else {
            let _ = tokio::time::timeout(HISTORY_FLUSH_INTERVAL, self.wake.notified()).await;
        }
    }

    /// Reconciles when needed, writes queued items in order, then writes
    /// terminal runs the ledger has not mirrored yet.
    pub(crate) async fn flush_once(
        &self,
        state: &SharedDaemonState,
        now_ms: u64,
    ) -> Result<FlushReport, HistoryError> {
        let _flushing = self.flushing.lock().await;
        let mut report = FlushReport::default();
        let result = self.flush_locked(state, &mut report).await;
        self.record_result(&result, now_ms);
        result.map(|()| report)
    }

    async fn flush_locked(
        &self,
        state: &SharedDaemonState,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        if !self.reconciled() || self.outbox().needs_reconcile {
            report.reconciled = self.reconcile(state).await?;
        }
        loop {
            match self.next_batch() {
                Batch::Empty => break,
                Batch::Messages { through, rows } => {
                    self.store.upsert_messages(&rows).await?;
                    self.mark_mirrored(rows.iter().map(|row| row.message.id.as_str()));
                    self.complete_through(through);
                    report.messages += rows.len();
                }
                Batch::Deletion {
                    through,
                    agent_id,
                    session_id,
                } => {
                    self.store.delete_session(&agent_id, &session_id).await?;
                    self.complete_through(through);
                    report.deletions += 1;
                }
            }
        }
        loop {
            let runs = state.read().await.runs.unmirrored_terminal(HISTORY_RUN_BATCH);
            if runs.is_empty() {
                break;
            }
            self.store.upsert_runs(&runs).await?;
            let marked = state.write().await.runs.mark_mirrored(&runs);
            report.runs += marked;
            if marked == 0 || runs.len() < HISTORY_RUN_BATCH {
                break;
            }
        }
        Ok(())
    }

    /// The queue prefix to write next: up to a batch of messages, or one deletion.
    fn next_batch(&self) -> Batch {
        let outbox = self.outbox();
        let Some(first) = outbox.items.front() else {
            return Batch::Empty;
        };
        if let OutboxItem::DeleteSession {
            agent_id,
            session_id,
        } = &first.item
        {
            return Batch::Deletion {
                through: first.seq,
                agent_id: agent_id.clone(),
                session_id: session_id.clone(),
            };
        }
        let mut rows = Vec::new();
        let mut through = first.seq;
        for queued in outbox.items.iter().take(HISTORY_FLUSH_BATCH) {
            match &queued.item {
                OutboxItem::Message(row) => {
                    rows.push(row.clone());
                    through = queued.seq;
                }
                OutboxItem::DeleteSession { .. } => break,
            }
        }
        Batch::Messages { through, rows }
    }

    fn complete_through(&self, seq: u64) {
        let mut outbox = self.outbox();
        while outbox
            .items
            .front()
            .is_some_and(|queued| queued.seq <= seq)
        {
            outbox.items.pop_front();
        }
    }

    /// Marks hot messages the store already holds as mirrored and queues the
    /// ones it is missing (spec §13.1 restart rule, §13.3 step 3).
    async fn reconcile(&self, state: &SharedDaemonState) -> Result<usize, HistoryError> {
        let hot = hot_messages_by_session(&*state.read().await);
        let queued = self
            .outbox()
            .items
            .iter()
            .filter_map(|queued| match &queued.item {
                OutboxItem::Message(row) => Some(row.message.id.clone()),
                OutboxItem::DeleteSession { .. } => None,
            })
            .collect::<HashSet<_>>();
        let mut missing = Vec::new();
        for (agent_id, session_id, messages) in hot {
            let hidden = hidden_message_ids(messages.iter());
            for chunk in messages.chunks(HISTORY_FLUSH_BATCH) {
                let ids = chunk
                    .iter()
                    .map(|message| message.id.clone())
                    .collect::<Vec<_>>();
                let existing = self.store.existing_message_ids(&ids).await?;
                self.mark_mirrored(existing.iter().map(String::as_str));
                for message in chunk
                    .iter()
                    .filter(|message| !existing.contains(&message.id) && !queued.contains(&message.id))
                {
                    missing.push(HistoryMessage {
                        agent_id: agent_id.clone(),
                        session_id: session_id.clone(),
                        hidden: hidden.contains(&message.id),
                        message: message.clone(),
                    });
                }
            }
        }
        let count = missing.len();
        {
            let mut outbox = self.outbox();
            for row in missing {
                outbox.push(OutboxItem::Message(row));
            }
            outbox.needs_reconcile = false;
        }
        self.reconciled.store(true, Ordering::Release);
        Ok(count)
    }

    fn record_result(&self, result: &Result<(), HistoryError>, now_ms: u64) {
        let mut outbox = self.outbox();
        match result {
            Ok(()) => {
                outbox.failing_since_ms = None;
                outbox.consecutive_failures = 0;
                outbox.last_error = None;
            }
            Err(error) => {
                outbox.failing_since_ms.get_or_insert(now_ms);
                outbox.consecutive_failures = outbox.consecutive_failures.saturating_add(1);
                outbox.last_error = Some(error.to_string());
                warn!(
                    error = %error,
                    pending = outbox.items.len(),
                    "history store write failed; records stay in the control plane"
                );
            }
        }
    }
}

/// Every hot message, grouped by (agent, session) in transcript order.
fn hot_messages_by_session(state: &DaemonState) -> Vec<(String, String, Vec<Message>)> {
    let mut sessions = Vec::new();
    for (agent_id, runtime) in &state.agents {
        let mut rooms: HashMap<&str, Vec<Message>> = HashMap::new();
        for message in runtime.messages() {
            rooms
                .entry(message.room_id.as_str())
                .or_default()
                .push(message.clone());
        }
        for (room_id, messages) in rooms {
            sessions.push((agent_id.clone(), session_id_for_room(room_id), messages));
        }
    }
    sessions
}

struct WorkerHandle {
    cancel: watch::Sender<bool>,
    join: JoinHandle<()>,
}

/// Runs the outbox flush loop; Task 13 adds hot-tail pruning to it.
#[derive(Clone)]
pub(crate) struct HistoryWorker {
    state: SharedDaemonState,
    /// The control-plane transaction; hot-tail pruning (Task 13) takes it.
    #[allow(dead_code)]
    transactions: Arc<Mutex<()>>,
    running: Arc<StdMutex<Option<WorkerHandle>>>,
}

impl HistoryWorker {
    pub(crate) fn new(state: SharedDaemonState, transactions: Arc<Mutex<()>>) -> Self {
        Self {
            state,
            transactions,
            running: Arc::new(StdMutex::new(None)),
        }
    }

    /// Starts the flush loop. Needs a Tokio runtime; a second call is a no-op.
    pub(crate) fn start(&self) {
        let mut running = lock(&self.running);
        if running.is_some() {
            return;
        }
        let (cancel, mut cancelled) = watch::channel(false);
        let state = Arc::clone(&self.state);
        let join = tokio::spawn(async move {
            loop {
                let history = state.read().await.history.clone();
                tokio::select! {
                    changed = cancelled.changed() => {
                        if changed.is_err() || *cancelled.borrow() {
                            break;
                        }
                    }
                    () = history.wait_for_work() => {}
                }
                let _ = history.flush_once(&state, now_millis()).await;
            }
        });
        *running = Some(WorkerHandle { cancel, join });
    }

    /// Stops the loop and makes one final flush attempt.
    pub(crate) async fn shutdown(&self) {
        let handle = lock(&self.running).take();
        if let Some(handle) = handle {
            let _ = handle.cancel.send(true);
            let _ = handle.join.await;
        }
        let history = self.state.read().await.history.clone();
        if let Err(error) = history.flush_once(&self.state, now_millis()).await {
            warn!(error = %error, "final history flush failed; records stay in the control plane");
        }
    }
}
```

In `hosts/rust-daemon/src/history/mod.rs`, add `mod outbox;` after `mod memory;` and:

```rust
pub(crate) use outbox::{
    FlushReport, HistoryService, HistoryWorker, SharedHistory, HISTORY_FLUSH_INTERVAL,
    HISTORY_READINESS_GRACE_MS,
};
```

- [ ] **Step 4: Implement the ledger, state, and coordinator changes**

`hosts/rust-daemon/src/runs/ledger.rs` — replace `prune` with:

```rust
    /// Keeps every non-terminal run plus, per agent, the terminal runs from the
    /// last 24 hours up to 50; a terminal run leaves only once the history
    /// store holds it (spec §4.1).
    pub(crate) fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(TERMINAL_RUN_RETENTION_MS);
        let expired: Vec<String> = {
            let mut terminal: HashMap<&str, Vec<(u64, &str, bool)>> = HashMap::new();
            for record in self
                .records
                .values()
                .filter(|record| record.status.is_terminal())
            {
                terminal.entry(record.agent_id.as_str()).or_default().push((
                    record.finished_at_ms.unwrap_or(record.created_at_ms),
                    record.id.as_str(),
                    record.mirrored,
                ));
            }
            let mut expired = Vec::new();
            for runs in terminal.values_mut() {
                runs.sort_unstable_by(|left, right| (right.0, right.1).cmp(&(left.0, left.1)));
                for (index, (finished_at_ms, run_id, mirrored)) in runs.iter().enumerate() {
                    if *mirrored
                        && (index >= MAX_TERMINAL_RUNS_PER_AGENT || *finished_at_ms < cutoff)
                    {
                        expired.push((*run_id).to_string());
                    }
                }
            }
            expired
        };
        for run_id in expired {
            self.records.remove(&run_id);
        }
    }

    /// Terminal runs the history store does not hold yet, oldest finished first.
    pub(crate) fn unmirrored_terminal(&self, limit: usize) -> Vec<RunRecord> {
        let mut records = self
            .records
            .values()
            .filter(|record| record.status.is_terminal() && !record.mirrored)
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.finished_at_ms
                .cmp(&right.finished_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        records.truncate(limit);
        records
    }

    /// Marks written runs mirrored, but only where the ledger still holds
    /// exactly what was written; a record changed meanwhile is written again.
    pub(crate) fn mark_mirrored(&mut self, written: &[RunRecord]) -> usize {
        let mut marked = 0;
        for run in written {
            if let Some(current) = self.records.get_mut(&run.id) {
                if !current.mirrored && *current == *run {
                    current.mirrored = true;
                    marked += 1;
                }
            }
        }
        marked
    }
```

Also change the `mirrored` field's doc comment on `RunRecord` to `/// Set once the history store holds this terminal record (spec §4.1).`

`hosts/rust-daemon/src/state.rs` (hand-formatted):

- add `pub(crate) history: crate::history::SharedHistory,` to `DaemonState` after `pub(crate) sessions: crate::sessions::SessionRegistry,`, and `history: crate::history::HistoryService::ephemeral(),` after `sessions: crate::sessions::SessionRegistry::default(),`;
- add after `set_memory_store`:

```rust
    pub(crate) fn set_history(&mut self, history: crate::history::SharedHistory) {
        self.history = history;
    }
```

`hosts/rust-daemon/src/agent_runs.rs`, in `run_locked` Phase C:

- change `let (snapshot, change_set, memory, memory_embeddings, memory_store, persist_request) = {` to `let (snapshot, change_set, memory, memory_embeddings, memory_store, history_outbox, persist_request) = {` (Phase B already has a local `history` for the room transcript, so use this distinct name);
- in the tuple at the end of that block, add `guard.history.clone(),` directly before `guard.control_plane_persist_request(),`;
- directly after `in_flight.disarm();` (which follows the successful save and `drop(transaction);`), add:

```rust
        // Only a durable commit reaches the history store (spec §13.1).
        history_outbox.enqueue_committed(
            &agent_id,
            &crate::sessions::session_id_for_room(&room_id),
            &change_set.delta.messages,
        );
```

- [ ] **Step 5: Implement persistence, startup, shutdown, and readiness**

`hosts/rust-daemon/src/app/persistence.rs`:

- change `use std::path::PathBuf;` to `use std::path::{Path, PathBuf};`, add `use crate::history::{HistoryService, HistoryStore, MemoryHistoryStore, PostgresHistoryStore, SqliteHistoryStore};`, and extend `use crate::control_plane_store::{...}` unchanged;
- in `configure_persistence`, replace `configure_control_plane_store(state, control_plane_store).await?;` with:

```rust
    configure_control_plane_store(state, control_plane_store.clone()).await?;
    configure_history_store(state, control_plane_store.as_ref()).await?;
```

- add after `configure_control_plane_store`:

```rust
/// The SQLite history file (spec §13.1); defaults beside the control plane.
pub(crate) const HISTORY_SQLITE_FILE_ENV: &str = "ANIMAOS_RS_HISTORY_SQLITE_FILE";

async fn configure_history_store(
    state: &SharedDaemonState,
    control_plane: Option<&ControlPlaneStoreConfig>,
) -> io::Result<()> {
    let store = history_store_for(control_plane, non_empty_env_path(HISTORY_SQLITE_FILE_ENV)?).await?;
    let label = store.label();
    state.write().await.set_history(HistoryService::new(store));
    info!(history_store = label, "runtime history store configured");
    Ok(())
}

/// The history store that goes with the control-plane store: SQLite beside a
/// JSON control plane (or at the explicit path), Postgres tables in Postgres
/// mode, and bounded memory tables in ephemeral mode.
pub(crate) async fn history_store_for(
    control_plane: Option<&ControlPlaneStoreConfig>,
    sqlite_override: Option<PathBuf>,
) -> io::Result<Arc<dyn HistoryStore>> {
    let sqlite_path = match (control_plane, sqlite_override) {
        (None, Some(_)) => {
            warn!("ANIMAOS_RS_HISTORY_SQLITE_FILE is ignored without a durable control plane; history stays in memory");
            return Ok(Arc::new(MemoryHistoryStore::new()));
        }
        (None, None) => return Ok(Arc::new(MemoryHistoryStore::new())),
        (Some(_), Some(path)) => path,
        (Some(ControlPlaneStoreConfig::Json(file)), None) => default_history_sqlite_path(file),
        (Some(ControlPlaneStoreConfig::Postgres(pool)), None) => {
            return Ok(Arc::new(PostgresHistoryStore::new(pool.clone())))
        }
    };
    let store = SqliteHistoryStore::open(sqlite_path)
        .await
        .map_err(|error| io::Error::new(io::ErrorKind::Other, format!("failed to open the history store: {error}")))?;
    Ok(Arc::new(store))
}

pub(crate) fn default_history_sqlite_path(control_plane_file: &Path) -> PathBuf {
    control_plane_file.with_file_name("history.sqlite")
}
```

`hosts/rust-daemon/src/app.rs`:

- add `history: crate::history::HistoryWorker,` to `struct DaemonRuntime` after `jobs: JobService,`;
- in `daemon_runtime` and in `deterministic_daemon_runtime_with_mail_transport`, add directly before `let scheduler = SchedulerService::new(state, agent_runs.clone(), connectors.clone());`:

```rust
    let history =
        crate::history::HistoryWorker::new(Arc::clone(&state), agent_runs.control_plane_transactions());
```

and add `history,` to both `DaemonRuntime { ... }` literals after `jobs,`;

- in `app_with_state`, add after `let runtime = deterministic_daemon_runtime(Arc::clone(&state), &config);`:

```rust
    // Embedded and test routers flush history as well when a runtime is present.
    if tokio::runtime::Handle::try_current().is_ok() {
        runtime.history.start();
    }
```

- in `app_with_configured_persistence`, add `runtime.history.start();` after `runtime.scheduler.start().await;`;
- in `serve_with_state`, add `runtime.history.start();` after `runtime.scheduler.start().await;`, add `let history = runtime.history.clone();` after `let jobs = runtime.jobs.clone();`, and add `history.shutdown().await;` after `connectors.shutdown().await;` inside the graceful-shutdown closure.

`hosts/rust-daemon/src/routes/health.rs`, in `handle_readiness`: replace the state read with

```rust
    let (database_configured, background_process_count, control_plane_durability, history) = {
        let guard = state.read().await;
        (
            guard.database_configured(),
            guard.background_process_count(),
            guard.control_plane_durability(),
            guard.history.clone(),
        )
    };
```

and add after the background-process issue:

```rust
    if let Some(issue) = history.readiness_issue(anima_core::primitives::now_millis()) {
        issues.push(issue);
    }
```

`hosts/rust-daemon/README.md`: add this row to the environment table after the `ANIMAOS_RS_CONTROL_PLANE_FILE` row:

```markdown
| `ANIMAOS_RS_HISTORY_SQLITE_FILE` | No | SQLite history store for session messages and finished runs (default `history.sqlite` beside `ANIMAOS_RS_CONTROL_PLANE_FILE`). Postgres mode uses Postgres history tables unless this is set. Without a control-plane file or Postgres, history stays in memory (100,000 rows per table). |
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- history:: runs:: agent_runs::tests app:: routes::health state::tests`
Expected: PASS (6 outbox tests, the ledger tests including the 3 rewritten or new ones, `only_durable_commits_reach_the_history_outbox`, the persistence and health tests, and the existing coordinator and state tests).

- [ ] **Step 7: Commit**

```bash
git add hosts/rust-daemon/src/history hosts/rust-daemon/src/runs/ledger.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/app/persistence.rs hosts/rust-daemon/src/app.rs hosts/rust-daemon/src/routes/health.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): mirror committed turns and finished runs through the history outbox"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. Mirror only durable state: `flush_once` reads `unmirrored_terminal` runs and `reconcile` takes its hot snapshot while holding the control-plane transaction mutex (`HistoryWorker.transactions`); `rollback_run` sets the record's `mirrored = false` (in `RunRecord::finish` or explicitly) so a rolled-back run is re-mirrored with its final status. Test: a flush that mirrors a completed run followed by a failed final save leaves the store row rewritten as `failed` after the next flush, with no phantom message rows.
2. Reconcile cannot resurrect deleted sessions: after its store round-trip, `reconcile` recomputes `missing` against the current hot state under `state.read()` and enqueues while still holding that read guard. Test: a `DeleteSession` enqueued while a reconcile is between its snapshot and its push leaves the deleted session with no rows after flushing.
3. Durable deletions: the control-plane snapshot gains `pendingHistoryDeletions` (`#[serde(default)]`, empty when absent), written in the same save as a session deletion (Task 12) or an agent deletion; boot replays pending deletions before `reconcile`; each entry is cleared in a normal save after the store's delete succeeds. Tests: a deletion survives a restart before the flush; a replayed deletion removes the rows.
4. Keep the worker alive in every router: store the `HistoryWorker` handle (or its cancel sender) in `AppState` or `DaemonState` so `router_with_runtime`, `app_with_state`, and `app_with_configured_persistence` keep flushing and pruning; the loop stops only on an explicit shutdown signal, never because a sender was dropped. Test: an `app_with_state` router mirrors a committed message within the flush interval.
5. Open or probe the history store before `configure_control_plane_store` writes anything, so a missing, corrupt, or unwritable history file refuses boot before the snapshot is upgraded and a pre-M2 daemon can still start on the untouched file. Test: an unwritable history path fails startup and leaves the control-plane file byte-identical.
6. Add a code comment at `reconcile` noting its first-boot memory cost (it clones hot messages into the outbox before capacity enforcement).

---

### Task 7: Snapshot version 5 with the pre-upgrade backup

**Files:**

- Modify: `hosts/rust-daemon/src/control_plane_store.rs` (version 5, unversioned files load as version 1, backup functions, tests)
- Modify: `hosts/rust-daemon/src/app/persistence.rs` (backup before the first save of an older snapshot; tests)
- Modify: `hosts/rust-daemon/src/state.rs` (one version assertion)

**Interfaces:**

- Consumes: the existing JSON (`AtomicFile`, `sync_snapshot_parent`) and Postgres (`host_snapshots`) helpers in `control_plane_store.rs`.
- Produces: `pub(crate) const CONTROL_PLANE_STORE_VERSION: u32 = 5` (the single M2 bump, M1 carry-forward); `pub(crate) const PRE_SESSIONS_BACKUP_SUFFIX: &str = ".pre-sessions.bak"`; `pre_sessions_backup_path(&Path) -> PathBuf`; `postgres_backup_key(version: u32) -> String` (`control_plane.backup.<version>`); `async write_pre_upgrade_backup(&ControlPlaneStoreConfig, loaded_version: u32) -> io::Result<String>` (JSON: exact file bytes to `<file>.pre-sessions.bak`, written atomically with `sync_all` and a synced parent directory; Postgres: server-side copy of the `control_plane` row, upserted). `configure_control_plane_store` writes the backup whenever the loaded version is below 5 — including unversioned JSON files, which now load as version 1 instead of the current version — before restoring and before the first save.

- [ ] **Step 1: Write the failing tests**

In `hosts/rust-daemon/src/control_plane_store.rs` tests:

- in `json_snapshot_replaces_an_existing_snapshot_only_after_a_synced_temp_write`, change `assert_eq!(loaded.version, 4);` to `assert_eq!(loaded.version, 5);`;
- in `snapshot_serializes_current_version_with_empty_connector_collections`, change `assert_eq!(payload["version"], 4);` to `assert_eq!(payload["version"], 5);`;
- append:

```rust
    #[test]
    fn unversioned_json_snapshots_load_as_legacy_version_one() {
        let path = test_snapshot_path("unversioned");
        std::fs::write(&path, r#"{"agents":[],"swarms":[]}"#).unwrap();

        let loaded = super::load_json_snapshot(&path).unwrap().unwrap();

        assert_eq!(loaded.version, 1, "an unversioned file predates sessions");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_backup_path_appends_the_suffix_to_the_file_name() {
        assert_eq!(
            super::pre_sessions_backup_path(std::path::Path::new("/data/control-plane.json")),
            std::path::PathBuf::from("/data/control-plane.json.pre-sessions.bak")
        );
        assert_eq!(super::postgres_backup_key(4), "control_plane.backup.4");
    }

    #[tokio::test]
    async fn the_pre_upgrade_backup_copies_the_exact_file_bytes_and_a_later_upgrade_replaces_it() {
        let path = test_snapshot_path("backup");
        let original = "{\n  \"version\": 4,\n  \"agents\": [],\n  \"swarms\": []\n}\n";
        std::fs::write(&path, original).unwrap();
        let config = super::ControlPlaneStoreConfig::Json(path.clone());

        let location = super::write_pre_upgrade_backup(&config, 4).await.unwrap();

        let backup = super::pre_sessions_backup_path(&path);
        assert_eq!(location, backup.display().to_string());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        std::fs::write(&path, "{\"version\":3}").unwrap();
        super::write_pre_upgrade_backup(&config, 3).await.unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{\"version\":3}");
        assert_no_temp_residue(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[ignore = "requires DATABASE_URL-backed Postgres"]
    #[sqlx::test(migrations = "./migrations")]
    async fn the_postgres_pre_upgrade_backup_copies_the_snapshot_row(pool: sqlx::PgPool) {
        use sqlx::Row;

        sqlx::query(
            "INSERT INTO host_snapshots (key, version, payload) VALUES ('control_plane', 4, '{\"version\":4,\"agents\":[]}')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let config = super::ControlPlaneStoreConfig::Postgres(pool.clone());

        let location = super::write_pre_upgrade_backup(&config, 4).await.unwrap();

        assert_eq!(location, "postgres:host_snapshots/control_plane.backup.4");
        let row = sqlx::query(
            "SELECT version, payload FROM host_snapshots WHERE key = 'control_plane.backup.4'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<i32, _>("version"), 4);
        assert_eq!(
            row.get::<serde_json::Value, _>("payload"),
            serde_json::json!({"version": 4, "agents": []})
        );
    }
```

In `hosts/rust-daemon/src/state.rs`, in `run_ledger_is_saved_in_the_snapshot_and_restart_interrupts_unfinished_runs`, change `assert_eq!(snapshot.version, 4);` to `assert_eq!(snapshot.version, 5);`.

Append to `mod tests` in `hosts/rust-daemon/src/app/persistence.rs` (created in Task 6):

```rust
    fn upgrader() -> anima_core::AgentConfig {
        anima_core::AgentConfig {
            name: "upgrader".into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        }
    }

    /// A snapshot file as an older daemon wrote it: no sessions, and the
    /// given version field (or none).
    fn older_snapshot_file(version: Option<u32>) -> String {
        let mut source = crate::state::DaemonState::new();
        source.create_agent(upgrader()).unwrap();
        let mut value = serde_json::to_value(source.control_plane_snapshot()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("sessions");
        match version {
            Some(version) => {
                object.insert("version".into(), version.into());
            }
            None => {
                object.remove("version");
            }
        }
        serde_json::to_string_pretty(&value).unwrap()
    }

    #[tokio::test]
    async fn upgrading_any_older_snapshot_writes_the_backup_before_saving_version_five() {
        for version in [None, Some(1), Some(2), Some(3), Some(4)] {
            let dir = temp_dir("upgrade");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("control-plane.json");
            let original = older_snapshot_file(version);
            std::fs::write(&path, &original).unwrap();
            let state = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));

            configure_control_plane_store(&state, Some(ControlPlaneStoreConfig::Json(path.clone())))
                .await
                .unwrap();

            let backup = crate::control_plane_store::pre_sessions_backup_path(&path);
            assert_eq!(
                std::fs::read_to_string(&backup).unwrap(),
                original,
                "{version:?}: the backup is the untouched original"
            );
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["version"], 5, "{version:?}");
            assert_eq!(state.read().await.agent_count(), 1);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[tokio::test]
    async fn a_current_snapshot_loads_without_a_backup() {
        let dir = temp_dir("current");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control-plane.json");
        let config = ControlPlaneStoreConfig::Json(path.clone());
        let fresh = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_store(&fresh, Some(config.clone()))
            .await
            .unwrap();
        assert!(path.exists(), "a fresh start saves a version-5 snapshot");

        let restarted = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_store(&restarted, Some(config)).await.unwrap();

        assert!(!crate::control_plane_store::pre_sessions_backup_path(&path).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- control_plane_store::tests app::persistence::tests state::tests::run_ledger_is_saved`
Expected: compile errors `cannot find function pre_sessions_backup_path`, `cannot find function write_pre_upgrade_backup`, and `cannot find function postgres_backup_key`; after those exist, the version assertions fail with `left: 4, right: 5` until the constant changes.

- [ ] **Step 3: Implement**

`hosts/rust-daemon/src/control_plane_store.rs`:

- replace `const CONTROL_PLANE_STORE_VERSION: u32 = 4;` with:

```rust
/// Snapshot format version. Version 5 adds sessions (companion console M2);
/// older daemons refuse it, so the first start writes a backup (spec §13.3).
pub(crate) const CONTROL_PLANE_STORE_VERSION: u32 = 5;
/// JSON snapshots written before the format was versioned.
const UNVERSIONED_SNAPSHOT_VERSION: u32 = 1;
/// Suffix of the JSON backup taken before the sessions upgrade (spec §13.3).
pub(crate) const PRE_SESSIONS_BACKUP_SUFFIX: &str = ".pre-sessions.bak";
```

- in `load_json_snapshot`, replace `snapshot.version = CONTROL_PLANE_STORE_VERSION;` (inside `if snapshot.version == 0`) with `snapshot.version = UNVERSIONED_SNAPSHOT_VERSION;`;
- add after `load_control_plane_snapshot`:

```rust
/// Where the JSON snapshot is backed up before the sessions upgrade.
pub(crate) fn pre_sessions_backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(PRE_SESSIONS_BACKUP_SUFFIX);
    path.with_file_name(name)
}

/// The `host_snapshots` key of the Postgres backup of a `version` snapshot.
pub(crate) fn postgres_backup_key(version: u32) -> String {
    format!("{CONTROL_PLANE_SNAPSHOT_KEY}.backup.{version}")
}

/// Saves the loaded snapshot, unchanged, before the upgrade rewrites it
/// (spec §13.3 step 1), and returns where the backup is.
pub(crate) async fn write_pre_upgrade_backup(
    config: &ControlPlaneStoreConfig,
    loaded_version: u32,
) -> io::Result<String> {
    match config {
        ControlPlaneStoreConfig::Json(path) => {
            backup_json_snapshot(path).map(|backup| backup.display().to_string())
        }
        ControlPlaneStoreConfig::Postgres(pool) => {
            backup_postgres_snapshot(pool, loaded_version).await
        }
    }
}

fn backup_json_snapshot(path: &Path) -> io::Result<PathBuf> {
    let bytes = fs::read(path)?;
    let backup = pre_sessions_backup_path(path);
    AtomicFile::new(&backup, AllowOverwrite)
        .write(|file| {
            file.write_all(&bytes)?;
            file.sync_all()
        })
        .map_err(atomic_write_error)?;
    sync_snapshot_parent(&backup)?;
    Ok(backup)
}

async fn backup_postgres_snapshot(pool: &PgPool, version: u32) -> io::Result<String> {
    let key = postgres_backup_key(version);
    sqlx::query(
        r#"
        INSERT INTO host_snapshots (key, version, payload, updated_at)
        SELECT $1, version, payload, now() FROM host_snapshots WHERE key = $2
        ON CONFLICT (key)
        DO UPDATE SET
            version = EXCLUDED.version,
            payload = EXCLUDED.payload,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(&key)
    .bind(CONTROL_PLANE_SNAPSHOT_KEY)
    .execute(pool)
    .await
    .map_err(postgres_error)?;
    Ok(format!("postgres:host_snapshots/{key}"))
}
```

`hosts/rust-daemon/src/app/persistence.rs`:

- change the import to `use crate::control_plane_store::{load_control_plane_snapshot, write_pre_upgrade_backup, ControlPlaneStoreConfig, CONTROL_PLANE_STORE_VERSION};`;
- in `configure_control_plane_store`, directly after `let snapshot = load_control_plane_snapshot(&config).await?;`, add:

```rust
    if let Some(loaded) = &snapshot {
        if loaded.version < CONTROL_PLANE_STORE_VERSION {
            // Spec §13.3 step 1: back up the untouched snapshot before anything
            // is written in the new version.
            let backup = write_pre_upgrade_backup(&config, loaded.version).await?;
            info!(
                backup = %backup,
                from_version = loaded.version,
                to_version = CONTROL_PLANE_STORE_VERSION,
                "saved the pre-upgrade control-plane backup"
            );
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- control_plane_store::tests app::persistence::tests state::tests`
Expected: PASS; the Postgres backup test is reported as ignored.

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/app/persistence.rs hosts/rust-daemon/src/state.rs
git commit -m "feat(daemon): bump the control plane to version 5 behind a pre-upgrade backup"
```

#### Controller rulings from the pre-flight audit (binding)

1. Write the JSON backup `<file>.pre-sessions.bak` (and the Postgres `control_plane.backup.<version>` row) only when the loaded version is below 5, so a later version bump can never overwrite it (unversioned files still count as version 1). Test: loading an already-v5 snapshot writes no backup and leaves an existing `.pre-sessions.bak` untouched.

---

### Task 8: Legacy migration — check-in relabel, legacy sessions, ledger ids, tool grants

**Files:**

- Create: `hosts/rust-daemon/src/sessions/migration.rs`
- Create: `hosts/rust-daemon/src/state/session_state.rs`
- Modify: `hosts/rust-daemon/src/sessions/mod.rs` (`pub(crate) mod migration;`)
- Modify: `hosts/rust-daemon/src/state.rs` (`mod session_state;`, `tool_grants_applied` field, snapshot, restore wiring)
- Modify: `hosts/rust-daemon/src/control_plane_store.rs` (`tool_grants_applied` field, constructor)
- Modify: `hosts/rust-daemon/src/agent_runs.rs` (`config_helper_parent` becomes `pub(crate)`)
- Modify: `hosts/rust-daemon/src/app/persistence.rs` (apply pending tool grants before the first save)

**Interfaces:**

- Consumes: Task 2's `kind_for_room`, `session_title`, `TitleContext`, `session_id_for_room`, `schedule_room_id`, `schedule_id_of_room`, `connector_id_of_room`, `job_id_of_room`, `peer_sender_of_room`, `delegating_agent_id`, `is_checkin_message`, `SessionRecord::new`, `SessionRegistry::{contains, insert}`; `ToolRegistry::{descriptor, resolve_descriptors}`.
- Produces (in `crate::sessions::migration`): `relabel_legacy_checkin_rooms(&mut [AgentRuntimeSnapshot], &mut [RunRecord]) -> usize` (messages moved); `map_ledger_session_ids(&mut [RunRecord]) -> usize` (F17); `LegacyAgent<'a> { agent_id: &'a str, config: &'a AgentConfig, messages: &'a [Message] }`; `LegacySessionContext<'a> { schedules, jobs, connectors, agent_names }` (maps by id); `derive_sessions_for_legacy_rooms(&SessionRegistry, &[LegacyAgent], &LegacySessionContext) -> Vec<SessionRecord>` (only rooms without a record; `lastReadAtMs` = last activity so upgraded history is not unread; helper sessions get `parentAgentId` from the helper's settings, a delegation preamble, or the peer room); `ToolGrantSet { id, read_class, write_class }` (all `&'static`); `TOOL_GRANTS: &[ToolGrantSet]` (empty in M2 — M3, M5, and M6 append theirs).
- Produces: `crate::agent_runs::config_helper_parent` (`pub(crate)`); `DaemonState::tool_grants_applied: BTreeSet<String>` (saved as `ControlPlaneSnapshot::tool_grants_applied: Vec<String>`, `#[serde(default)]`); `DaemonState::derive_legacy_sessions(&self) -> Vec<SessionRecord>`; `DaemonState::apply_pending_tool_grants(&mut self, &[ToolGrantSet]) -> Vec<String>` (agents that gained tools). Grants go to non-helper agents that have a tool list: read-class tools to all of them, write-class tools to those with `write_file`; unregistered names are skipped; each set applies once. `restore_control_plane_snapshot` relabels and maps before restoring agents and derives missing sessions after; `configure_control_plane_store` applies pending grants before its first save (also on a fresh start, so later agents never get retroactive grants).

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/sessions/migration.rs` with only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{RunStart, RunStatus};
    use crate::sessions::{SessionOrigin, TitleSource};
    use crate::state::DaemonState;
    use anima_core::{AgentSettings, Content};
    use std::collections::{BTreeMap, HashSet};

    fn message(
        id: &str,
        room: &str,
        role: MessageRole,
        text: &str,
        metadata: &[(&str, &str)],
        at: u64,
    ) -> Message {
        Message {
            id: id.into(),
            agent_id: "agent".into(),
            room_id: room.into(),
            content: Content {
                text: text.into(),
                attachments: None,
                metadata: (!metadata.is_empty()).then(|| {
                    metadata
                        .iter()
                        .map(|(key, value)| (key.to_string(), DataValue::String(value.to_string())))
                        .collect::<BTreeMap<_, _>>()
                }),
            },
            role,
            created_at_ms: at,
        }
    }

    fn run_in(agent_id: &str, session_id: &str) -> RunRecord {
        let mut run = RunRecord::running(
            RunStart {
                agent_id: agent_id.into(),
                session_id: session_id.into(),
                source: RunSource::Schedule,
                source_ref: None,
                idempotency_key: None,
                text: "tick".into(),
                model: "deterministic".into(),
                provider: None,
                parent_run_id: None,
            },
            10,
        );
        run.finish(RunStatus::Completed, None, 11);
        run
    }

    fn config(name: &str, additional: &[(&str, &str)]) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings {
                additional: additional
                    .iter()
                    .map(|(key, value)| (key.to_string(), DataValue::String(value.to_string())))
                    .collect(),
                ..AgentSettings::default()
            }),
        }
    }

    fn agent_snapshot(name: &str, messages: Vec<Message>) -> AgentRuntimeSnapshot {
        let mut snapshot = anima_core::AgentRuntime::new(
            config(name, &[]),
            std::sync::Arc::new(crate::model::DeterministicModelAdapter),
        )
        .snapshot();
        let agent_id = snapshot.state.id.clone();
        snapshot.messages = messages
            .into_iter()
            .map(|mut message| {
                message.agent_id = agent_id.clone();
                message
            })
            .collect();
        snapshot.message_count = snapshot.messages.len();
        snapshot
    }

    #[test]
    fn relabels_only_legacy_checkin_rooms_and_their_ledger_runs() {
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let tagged = [("kind", "checkin"), ("id", "schedule-1")];
        let mut messages = Vec::new();
        for (room, id, at) in [("room-100-1", "a", 100), ("room-200-2", "b", 200)] {
            messages.push(message(&format!("{id}-prompt"), room, MessageRole::User, &wrapped, &tagged, at));
            messages.push(message(&format!("{id}-reply"), room, MessageRole::Assistant, "CHECKIN_OK", &[], at + 1));
        }
        messages.push(message("chat-user", "room-300-3", MessageRole::User, "Summarize", &[], 300));
        messages.push(message(
            "telegram-checkin",
            "telegram:t1",
            MessageRole::User,
            &wrapped,
            &[("kind", "checkin"), ("id", "schedule-2")],
            400,
        ));
        let mut agents = vec![agent_snapshot("companion", messages)];
        let agent_id = agents[0].state.id.clone();
        let mut runs = vec![run_in(&agent_id, "room-100-1"), run_in(&agent_id, "room-300-3")];

        let moved = relabel_legacy_checkin_rooms(&mut agents, &mut runs);

        assert_eq!(moved, 4);
        let rooms = agents[0]
            .messages
            .iter()
            .map(|message| (message.id.as_str(), message.room_id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            rooms,
            [
                ("a-prompt", "schedule:schedule-1"),
                ("a-reply", "schedule:schedule-1"),
                ("b-prompt", "schedule:schedule-1"),
                ("b-reply", "schedule:schedule-1"),
                ("chat-user", "room-300-3"),
                ("telegram-checkin", "telegram:t1"),
            ]
        );
        assert_eq!(runs[0].session_id, "schedule:schedule-1");
        assert_eq!(runs[1].session_id, "room-300-3");
        assert_eq!(
            relabel_legacy_checkin_rooms(&mut agents, &mut runs),
            0,
            "relabeling is idempotent"
        );
    }

    #[test]
    fn ledger_session_ids_follow_the_room_mapping() {
        let mut runs = vec![run_in("agent", "weird room/1"), run_in("agent", "chat:fine")];
        assert_eq!(map_ledger_session_ids(&mut runs), 1);
        assert_eq!(runs[0].session_id, session_id_for_room("weird room/1"));
        assert_eq!(runs[1].session_id, "chat:fine");
        assert_eq!(map_ledger_session_ids(&mut runs), 0);
    }

    #[test]
    fn derives_a_session_for_every_legacy_room_kind() {
        let companion_config = config("Anima", &[("workspaceRole", "lead")]);
        let specialist_config = config("Specialist", &[]);
        let helper_config = config(
            "Research helper",
            &[("workspaceRole", "helper"), ("parentAgentId", "companion-1")],
        );
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let companion_messages = vec![
            message("d1", "direct:companion-1", MessageRole::User, "Plan my week\nwith details", &[], 10),
            message("d2", "direct:companion-1", MessageRole::Assistant, "Sure", &[], 11),
            message("g1", "room-20-1", MessageRole::User, "Summarize the report", &[], 20),
            message("c1", "schedule:schedule-1", MessageRole::User, &wrapped, &[("kind", "checkin"), ("id", "schedule-1")], 30),
            message("t1", "telegram:telegram-a", MessageRole::User, "hello from the phone", &[("source", "telegram")], 40),
            message("j1", "job:job-1", MessageRole::User, "Prepare the brief", &[], 50),
            message("w1", "weird room/1", MessageRole::User, "Odd room", &[], 60),
            message("w2", "weird room/1", MessageRole::Assistant, "Reply", &[], 65),
        ];
        let delegated = "Task delegated by workspace manager Anima (companion-1). Return the result and any blockers. Do not delegate further.\n\nCompare vendors";
        let specialist_messages = vec![
            message("s1", "room-70-2", MessageRole::User, delegated, &[], 70),
            message("p1", "peer:companion-1:specialist-1", MessageRole::User, "Can you check this?", &[], 80),
        ];
        let helper_messages = vec![message("h1", "room-90-3", MessageRole::User, "Find sources", &[], 90)];
        let schedules = HashMap::from([(
            "schedule-1".to_string(),
            serde_json::from_value::<ScheduledPromptRecord>(serde_json::json!({
                "id": "schedule-1",
                "agentId": "companion-1",
                "prompt": "Morning brief",
                "trigger": {"interval": {"intervalMs": 60000}},
                "target": "workspace",
                "nextDueAtMs": 1,
                "createdAtMs": 1,
                "updatedAtMs": 1
            }))
            .unwrap(),
        )]);
        let jobs = HashMap::from([(
            "job-1".to_string(),
            serde_json::from_value::<AgentJobRecord>(serde_json::json!({
                "id": "job-1",
                "agentId": "companion-1",
                "title": "Prepare brief",
                "prompt": "Prepare the brief",
                "requestKey": "brief",
                "status": "completed",
                "revision": 1,
                "attempt": 1,
                "createdAtMs": 1,
                "updatedAtMs": 2
            }))
            .unwrap(),
        )]);
        let connectors = HashMap::from([(
            "telegram-a".to_string(),
            serde_json::from_value::<TelegramConnectorRecord>(serde_json::json!({
                "id": "telegram-a",
                "agentId": "companion-1",
                "roomId": "telegram:telegram-a",
                "bot": {"id": "1", "username": "anima_bot"},
                "createdAtMs": 1,
                "updatedAtMs": 1
            }))
            .unwrap(),
        )]);
        let agent_names = HashMap::from([
            ("companion-1".to_string(), "Anima".to_string()),
            ("specialist-1".to_string(), "Specialist".to_string()),
            ("helper-1".to_string(), "Research helper".to_string()),
        ]);
        let mut registry = SessionRegistry::default();
        registry.insert(SessionRecord::new(
            "companion-1",
            "room-20-1",
            SessionKind::Chat,
            SessionOrigin::Api,
            "Existing".into(),
            TitleSource::Owner,
            1,
        ));
        let agents = [
            LegacyAgent { agent_id: "companion-1", config: &companion_config, messages: &companion_messages },
            LegacyAgent { agent_id: "specialist-1", config: &specialist_config, messages: &specialist_messages },
            LegacyAgent { agent_id: "helper-1", config: &helper_config, messages: &helper_messages },
        ];
        let context = LegacySessionContext {
            schedules: &schedules,
            jobs: &jobs,
            connectors: &connectors,
            agent_names: &agent_names,
        };

        let derived = derive_sessions_for_legacy_rooms(&registry, &agents, &context);

        let by_key = derived
            .iter()
            .map(|record| ((record.agent_id.as_str(), record.id.as_str()), record))
            .collect::<HashMap<_, _>>();
        assert_eq!(derived.len(), 8, "every room except the registered one");
        let direct = by_key[&("companion-1", "direct:companion-1")];
        assert_eq!((direct.kind, direct.origin), (SessionKind::Chat, SessionOrigin::Web));
        assert_eq!((direct.title.as_str(), direct.title_source), ("Plan my week", TitleSource::FirstMessage));
        assert_eq!((direct.created_at_ms, direct.last_activity_at_ms), (10, 11));
        assert_eq!(direct.last_read_at_ms, Some(11), "upgraded history is not unread");
        assert!(!by_key.contains_key(&("companion-1", "room-20-1")));
        let checkin = by_key[&("companion-1", "schedule:schedule-1")];
        assert_eq!((checkin.kind, checkin.title.as_str()), (SessionKind::Checkin, "Check-in · Morning brief"));
        assert_eq!(by_key[&("companion-1", "telegram:telegram-a")].title, "Telegram · @anima_bot");
        assert_eq!(by_key[&("companion-1", "job:job-1")].title, "Job · Prepare brief");
        let legacy_id = session_id_for_room("weird room/1");
        let legacy = by_key[&("companion-1", legacy_id.as_str())];
        assert_eq!(legacy.room_id(), "weird room/1");
        assert_eq!((legacy.kind, legacy.origin, legacy.title.as_str()), (SessionKind::Chat, SessionOrigin::Api, "Odd room"));
        assert_eq!(legacy.last_activity_at_ms, 65);
        let delegated_session = by_key[&("specialist-1", "room-70-2")];
        assert_eq!((delegated_session.kind, delegated_session.origin), (SessionKind::Helper, SessionOrigin::Delegation));
        assert_eq!(delegated_session.title, "Compare vendors");
        assert_eq!(delegated_session.parent_agent_id.as_deref(), Some("companion-1"));
        let peer = by_key[&("specialist-1", "peer:companion-1:specialist-1")];
        assert_eq!((peer.kind, peer.origin, peer.title.as_str()), (SessionKind::Helper, SessionOrigin::Peer, "Messages from Anima"));
        assert_eq!(peer.parent_agent_id.as_deref(), Some("companion-1"));
        let helper = by_key[&("helper-1", "room-90-3")];
        assert_eq!((helper.kind, helper.title.as_str()), (SessionKind::Helper, "Find sources"));
        assert_eq!(helper.parent_agent_id.as_deref(), Some("companion-1"));
    }

    #[test]
    fn restoring_an_older_snapshot_gives_every_room_a_session_exactly_once() {
        let mut source = DaemonState::new();
        let agent_id = source
            .create_agent(config("companion", &[("workspaceRole", "lead")]))
            .unwrap()
            .state
            .id;
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let mut snapshot = source.control_plane_snapshot();
        snapshot.version = 4;
        snapshot.agents[0].messages = vec![
            message("d1", &format!("direct:{agent_id}"), MessageRole::User, "Plan my week", &[], 10),
            message("c1", "room-20-1", MessageRole::User, &wrapped, &[("kind", "checkin"), ("id", "schedule-9")], 20),
            message("c2", "room-20-1", MessageRole::Assistant, "CHECKIN_OK", &[], 21),
        ]
        .into_iter()
        .map(|mut message| {
            message.agent_id = agent_id.clone();
            message
        })
        .collect();
        snapshot.agents[0].message_count = 3;
        snapshot.runs = vec![run_in(&agent_id, "room-20-1")];

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(snapshot)
            .expect("an older snapshot restores");

        let direct = restored
            .sessions
            .get(&agent_id, &format!("direct:{agent_id}"))
            .expect("the legacy web chat is a session");
        assert_eq!(direct.title, "Plan my week");
        let checkin = restored
            .sessions
            .get(&agent_id, "schedule:schedule-9")
            .expect("the per-tick room became the automation's session");
        assert_eq!(checkin.kind, SessionKind::Checkin);
        assert_eq!(checkin.title, "Check-in · Review open tasks");
        assert_eq!(restored.sessions.len(), 2);
        assert!(restored
            .get_agent(&agent_id)
            .unwrap()
            .messages
            .iter()
            .filter(|message| message.id.starts_with('c'))
            .all(|message| message.room_id == "schedule:schedule-9"));
        assert_eq!(restored.runs.for_agent(&agent_id)[0].session_id, "schedule:schedule-9");

        let saved = restored.control_plane_snapshot();
        let mut again = DaemonState::new();
        again.restore_control_plane_snapshot(saved.clone()).unwrap();
        assert_eq!(
            again.control_plane_snapshot().sessions,
            saved.sessions,
            "a second restore derives nothing new"
        );
    }

    #[test]
    fn tool_grant_sets_reach_existing_non_helper_agents_once() {
        const GRANTS: &[ToolGrantSet] = &[ToolGrantSet {
            id: "test-grant",
            read_class: &["todo_read", "not_a_tool"],
            write_class: &["todo_write"],
        }];
        let registry = crate::tools::ToolRegistry::new();
        let with_tools = |name: &str, tools: &[&str], additional: &[(&str, &str)]| {
            let mut agent = config(name, additional);
            agent.tools = Some(registry.resolve_descriptors(tools.iter().copied()).unwrap());
            agent
        };
        let mut state = DaemonState::new();
        let writer = state.create_agent(with_tools("writer", &["write_file"], &[])).unwrap().state.id;
        let reader = state.create_agent(with_tools("reader", &["read_file"], &[])).unwrap().state.id;
        let toolless = state.create_agent(config("toolless", &[])).unwrap().state.id;
        let helper = state
            .create_agent(with_tools(
                "helper",
                &["write_file"],
                &[("workspaceRole", "helper"), ("parentAgentId", writer.as_str())],
            ))
            .unwrap()
            .state
            .id;

        let mut changed = state.apply_pending_tool_grants(GRANTS);
        changed.sort();
        let mut expected = vec![writer.clone(), reader.clone()];
        expected.sort();
        assert_eq!(changed, expected);
        let names = |id: &str| {
            state
                .get_agent(id)
                .unwrap()
                .state
                .config
                .tools
                .unwrap_or_default()
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&writer), ["write_file", "todo_read", "todo_write"]);
        assert_eq!(names(&reader), ["read_file", "todo_read"]);
        assert_eq!(names(&helper), ["write_file"], "helpers never gain tools");
        assert!(
            state.get_agent(&toolless).unwrap().state.config.tools.is_none(),
            "an agent created without tools keeps none"
        );
        assert!(state.apply_pending_tool_grants(GRANTS).is_empty(), "a grant set applies once");
        assert!(state
            .control_plane_snapshot()
            .tool_grants_applied
            .contains(&"test-grant".to_string()));
    }

    #[test]
    fn every_listed_tool_grant_names_a_registered_tool() {
        let registry = crate::tools::ToolRegistry::new();
        let mut ids = HashSet::new();
        for grant in TOOL_GRANTS {
            assert!(ids.insert(grant.id), "duplicate grant id {}", grant.id);
            for name in grant.read_class.iter().chain(grant.write_class) {
                assert!(registry.descriptor(name).is_some(), "{name} is not registered");
            }
        }
    }
}
```

In `hosts/rust-daemon/src/sessions/mod.rs`, add `pub(crate) mod migration;` after the module doc comment.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions::migration::tests`
Expected: compile errors such as `cannot find function relabel_legacy_checkin_rooms`, `cannot find struct LegacyAgent`, `no method named apply_pending_tool_grants`, and `no field tool_grants_applied`.

- [ ] **Step 3: Implement the migration module**

Put this above the test module in `hosts/rust-daemon/src/sessions/migration.rs`:

```rust
//! Upgrading older control planes to sessions (spec §13.3): relabel legacy
//! per-tick check-in rooms, give every room a session record, map ledger
//! session ids, and grant tools added after agents already existed.

use std::collections::HashMap;

use anima_core::{AgentConfig, AgentRuntimeSnapshot, DataValue, Message, MessageRole};

use super::{
    connector_id_of_room, delegating_agent_id, is_checkin_message, job_id_of_room,
    kind_for_room, peer_sender_of_room, schedule_id_of_room, schedule_room_id,
    session_id_for_room, session_title, SessionKind, SessionRecord, SessionRegistry,
    TitleContext,
};
use crate::connectors::TelegramConnectorRecord;
use crate::jobs::AgentJobRecord;
use crate::runs::{RunRecord, RunSource};
use crate::schedules::ScheduledPromptRecord;

/// Tools added after agents existed, granted once to the agents of that time
/// the way the web access profiles grant them (spec §13.3 step 5).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ToolGrantSet {
    /// Recorded in the control plane once applied.
    pub(crate) id: &'static str,
    /// Granted to every non-helper agent that has a tool list.
    pub(crate) read_class: &'static [&'static str],
    /// Granted to those agents that already have `write_file`.
    pub(crate) write_class: &'static [&'static str],
}

/// M2 adds no tools. M3 (`search_conversations`), M5 (`load_skill`,
/// `propose_skill`), and M6 (`list_automations`, `create_automation`,
/// `pause_automation`) append their grant sets here.
pub(crate) const TOOL_GRANTS: &[ToolGrantSet] = &[];

/// Legacy check-ins ran in a fresh `room-*` room per tick. Those rooms become
/// the automation's `schedule:<id>` session, and so do their ledger runs;
/// nothing else references them. Returns how many messages moved.
pub(crate) fn relabel_legacy_checkin_rooms(
    agents: &mut [AgentRuntimeSnapshot],
    runs: &mut [RunRecord],
) -> usize {
    let mut moved = 0;
    let mut relabelled: HashMap<(String, String), String> = HashMap::new();
    for agent in agents.iter_mut() {
        let mut targets: HashMap<String, String> = HashMap::new();
        for message in &agent.messages {
            if !message.room_id.starts_with("room-") || targets.contains_key(&message.room_id) {
                continue;
            }
            if let Some(schedule_id) = checkin_schedule_id(message) {
                targets.insert(message.room_id.clone(), schedule_room_id(schedule_id));
            }
        }
        if targets.is_empty() {
            continue;
        }
        for message in agent.messages.iter_mut() {
            if let Some(room) = targets.get(&message.room_id) {
                message.room_id = room.clone();
                moved += 1;
            }
        }
        for (old, new) in targets {
            relabelled.insert((agent.state.id.clone(), old), new);
        }
    }
    for run in runs.iter_mut() {
        if let Some(room) = relabelled.get(&(run.agent_id.clone(), run.session_id.clone())) {
            run.session_id = room.clone();
        }
    }
    moved
}

fn checkin_schedule_id(message: &Message) -> Option<&str> {
    if !is_checkin_message(message) {
        return None;
    }
    match message.content.metadata.as_ref()?.get("id") {
        Some(DataValue::String(id)) if !id.trim().is_empty() => Some(id),
        _ => None,
    }
}

/// Ledger session ids that were written as raw room ids take the room's
/// session id (M1 carry-forward F17). Returns how many changed.
pub(crate) fn map_ledger_session_ids(runs: &mut [RunRecord]) -> usize {
    let mut mapped = 0;
    for run in runs {
        let session_id = session_id_for_room(&run.session_id);
        if session_id != run.session_id {
            run.session_id = session_id;
            mapped += 1;
        }
    }
    mapped
}

/// One agent's transcript, as the migration reads it.
pub(crate) struct LegacyAgent<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) config: &'a AgentConfig,
    pub(crate) messages: &'a [Message],
}

/// Records a session title can come from.
pub(crate) struct LegacySessionContext<'a> {
    pub(crate) schedules: &'a HashMap<String, ScheduledPromptRecord>,
    pub(crate) jobs: &'a HashMap<String, AgentJobRecord>,
    pub(crate) connectors: &'a HashMap<String, TelegramConnectorRecord>,
    pub(crate) agent_names: &'a HashMap<String, String>,
}

/// Session records for the rooms that have none, by the rules of spec §3.1
/// (spec §13.3 step 2). Upgraded history counts as read.
pub(crate) fn derive_sessions_for_legacy_rooms(
    registry: &SessionRegistry,
    agents: &[LegacyAgent<'_>],
    context: &LegacySessionContext<'_>,
) -> Vec<SessionRecord> {
    let mut derived = Vec::new();
    for agent in agents {
        let helper_parent = crate::agent_runs::config_helper_parent(agent.config);
        let mut rooms: Vec<(&str, Vec<&Message>)> = Vec::new();
        let mut index: HashMap<&str, usize> = HashMap::new();
        for message in agent.messages {
            let slot = *index.entry(message.room_id.as_str()).or_insert_with(|| {
                rooms.push((message.room_id.as_str(), Vec::new()));
                rooms.len() - 1
            });
            rooms[slot].1.push(message);
        }
        for (room_id, messages) in rooms {
            if registry.contains(agent.agent_id, &session_id_for_room(room_id)) {
                continue;
            }
            derived.push(legacy_session(agent.agent_id, helper_parent, room_id, &messages, context));
        }
    }
    derived
}

fn legacy_session(
    agent_id: &str,
    helper_parent: Option<&str>,
    room_id: &str,
    messages: &[&Message],
    context: &LegacySessionContext<'_>,
) -> SessionRecord {
    let first_user = messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.content.text.as_str());
    let delegated_by = first_user.and_then(delegating_agent_id);
    let source = delegated_by.map(|_| RunSource::Delegation);
    let (kind, origin) = kind_for_room(room_id, source, helper_parent.is_some());
    let peer_sender = peer_sender_of_room(room_id);
    let title_context = TitleContext {
        first_user_text: first_user,
        schedule_prompt: schedule_id_of_room(room_id)
            .and_then(|id| context.schedules.get(id))
            .map(|schedule| schedule.prompt.as_str()),
        job_title: job_id_of_room(room_id)
            .and_then(|id| context.jobs.get(id))
            .map(|job| job.title.as_str()),
        bot_username: connector_id_of_room(room_id)
            .and_then(|id| context.connectors.get(id))
            .and_then(|connector| connector.bot.username.as_deref()),
        peer_sender_name: peer_sender
            .and_then(|id| context.agent_names.get(id))
            .map(String::as_str),
    };
    let (title, title_source) = session_title(kind, origin, &title_context);
    let first_at = messages.first().map(|message| message.created_at_ms).unwrap_or(0);
    let last_at = messages
        .iter()
        .map(|message| message.created_at_ms)
        .max()
        .unwrap_or(first_at);
    let mut record = SessionRecord::new(agent_id, room_id, kind, origin, title, title_source, first_at);
    record.last_activity_at_ms = last_at;
    record.last_read_at_ms = Some(last_at);
    if kind == SessionKind::Helper {
        record.parent_agent_id = helper_parent
            .or(delegated_by)
            .or(peer_sender)
            .map(str::to_string);
    }
    record
}
```

- [ ] **Step 4: Implement the state, snapshot, and startup wiring**

`hosts/rust-daemon/src/agent_runs.rs`: change `fn config_helper_parent(config: &AgentConfig) -> Option<&str> {` to `pub(crate) fn config_helper_parent(config: &AgentConfig) -> Option<&str> {`.

`hosts/rust-daemon/src/control_plane_store.rs`: add to `ControlPlaneSnapshot`, after `sessions`,

```rust
    /// Tool grant sets already applied (spec §13.3 step 5).
    #[serde(default)]
    pub(crate) tool_grants_applied: Vec<String>,
```

and add `tool_grants_applied: vec![],` after `sessions: vec![],` in `with_connector_state_and_cleanup`.

Create `hosts/rust-daemon/src/state/session_state.rs`:

```rust
//! Session bookkeeping on the daemon state (spec §3, §13.3).

use std::collections::HashMap;

use anima_core::AgentConfigUpdate;

use super::DaemonState;
use crate::agent_runs::config_helper_parent;
use crate::sessions::migration::{
    derive_sessions_for_legacy_rooms, LegacyAgent, LegacySessionContext, ToolGrantSet,
};
use crate::sessions::SessionRecord;

impl DaemonState {
    /// Session records for rooms that have none yet (spec §13.3 step 2).
    pub(crate) fn derive_legacy_sessions(&self) -> Vec<SessionRecord> {
        let agent_names = self
            .agents
            .iter()
            .map(|(id, runtime)| (id.clone(), runtime.config().name.clone()))
            .collect::<HashMap<_, _>>();
        let agents = self
            .agents
            .iter()
            .map(|(id, runtime)| LegacyAgent {
                agent_id: id,
                config: runtime.config(),
                messages: runtime.messages(),
            })
            .collect::<Vec<_>>();
        derive_sessions_for_legacy_rooms(
            &self.sessions,
            &agents,
            &LegacySessionContext {
                schedules: &self.schedules,
                jobs: &self.jobs,
                connectors: &self.connectors,
                agent_names: &agent_names,
            },
        )
    }

    /// Applies the grant sets not applied before; returns the agents that
    /// gained tools. Helpers and agents without a tool list never gain any.
    pub(crate) fn apply_pending_tool_grants(&mut self, grants: &[ToolGrantSet]) -> Vec<String> {
        let mut changed: Vec<String> = Vec::new();
        for grant in grants {
            if self.tool_grants_applied.contains(grant.id) {
                continue;
            }
            for (agent_id, runtime) in self.agents.iter_mut() {
                let config = runtime.config();
                if config_helper_parent(config).is_some() {
                    continue;
                }
                let Some(current) = config.tools.as_ref() else {
                    continue;
                };
                let has_write_file = current.iter().any(|tool| tool.name == "write_file");
                let mut names = current
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>();
                let wanted = grant
                    .read_class
                    .iter()
                    .chain(grant.write_class.iter().filter(|_| has_write_file));
                let mut added = false;
                for name in wanted {
                    if self.tool_registry.descriptor(name).is_some()
                        && !names.iter().any(|known| known == name)
                    {
                        names.push((*name).to_string());
                        added = true;
                    }
                }
                if !added {
                    continue;
                }
                let tools = self
                    .tool_registry
                    .resolve_descriptors(names)
                    .expect("only registered tools are granted");
                runtime.update_config(AgentConfigUpdate {
                    tools: Some(tools),
                    ..AgentConfigUpdate::default()
                });
                if !changed.contains(agent_id) {
                    changed.push(agent_id.clone());
                }
            }
            self.tool_grants_applied.insert(grant.id.to_string());
        }
        for agent_id in &changed {
            if let Some(runtime) = self.agents.get(agent_id) {
                self.agent_snapshots
                    .insert(agent_id.clone(), runtime.snapshot());
            }
        }
        changed
    }
}
```

`hosts/rust-daemon/src/state.rs` (hand-formatted):

- add `mod session_state;` after `mod run_commit;`;
- add `pub(crate) tool_grants_applied: std::collections::BTreeSet<String>,` to `DaemonState` after `pub(crate) history: crate::history::SharedHistory,`, and `tool_grants_applied: std::collections::BTreeSet::new(),` after `history: crate::history::HistoryService::ephemeral(),`;
- in `control_plane_snapshot`, after the `snapshot.sessions = …` line, add `snapshot.tool_grants_applied = self.tool_grants_applied.iter().cloned().collect();`;
- change `restore_control_plane_snapshot(&mut self, snapshot: ControlPlaneSnapshot)` to take `mut snapshot: ControlPlaneSnapshot`, and directly after `self.validate_control_plane_snapshot(&snapshot)?;` add:

```rust
        // Spec §13.3 step 2: legacy per-tick check-in rooms become their
        // automation's session, and ledger session ids follow the room mapping.
        crate::sessions::migration::relabel_legacy_checkin_rooms(
            &mut snapshot.agents,
            &mut snapshot.runs,
        );
        crate::sessions::migration::map_ledger_session_ids(&mut snapshot.runs);
```

- directly after the `self.sessions = crate::sessions::SessionRegistry::restored(…);` statement (Task 2), add:

```rust
        for record in self.derive_legacy_sessions() {
            self.sessions.insert(record);
        }
        self.tool_grants_applied = snapshot.tool_grants_applied.into_iter().collect();
```

`hosts/rust-daemon/src/app/persistence.rs`, in `configure_control_plane_store`, replace the block

```rust
    {
        let mut guard = state.write().await;
        guard.set_control_plane_store(Some(config.clone()));
        guard.control_plane_persist_request()
    }
```

with

```rust
    {
        let mut guard = state.write().await;
        // Spec §13.3 step 5: tools added since these agents were created.
        let granted = guard.apply_pending_tool_grants(crate::sessions::migration::TOOL_GRANTS);
        if !granted.is_empty() {
            info!(agents = granted.len(), "granted newly added tools to existing agents");
        }
        guard.set_control_plane_store(Some(config.clone()));
        guard.control_plane_persist_request()
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions:: state::tests app::persistence::tests control_plane_store::tests`
Expected: PASS (6 migration tests plus the existing state, persistence, and store tests).

- [ ] **Step 6: Commit**

```bash
git add hosts/rust-daemon/src/sessions/migration.rs hosts/rust-daemon/src/sessions/mod.rs hosts/rust-daemon/src/state/session_state.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/app/persistence.rs
git commit -m "feat(daemon): migrate legacy rooms to sessions and add the one-time tool grant helper"
```

---

### Task 9: Session records at run time and helper linkage

**Files:**

- Modify: `hosts/rust-daemon/src/runs/mod.rs` (`RunLink`, `RunChangeSet::session_undo`)
- Modify: `hosts/rust-daemon/src/state/session_state.rs` (`RunSessionRequest`, `ensure_run_session`)
- Modify: `hosts/rust-daemon/src/state.rs` (re-export `RunSessionRequest`)
- Modify: `hosts/rust-daemon/src/state/run_commit.rs` (`commit_run` advances the session; `rollback_run` reverts it)
- Modify: `hosts/rust-daemon/src/agent_runs.rs` (`AgentRunRequest::parent`; `run_locked` Phase A/B/C; `send_peer`, `delegate`, `spawn_helper` take the parent; tests)
- Modify: `hosts/rust-daemon/src/tools.rs` (`ToolExecutionContext::run_link`, `with_run_link`)
- Modify: `hosts/rust-daemon/src/tools/team.rs` (pass the calling run's link)
- Modify (mechanical, add `parent: None,`): `hosts/rust-daemon/src/schedules.rs`, `hosts/rust-daemon/src/jobs.rs`, `hosts/rust-daemon/src/jobs/tests.rs`, `hosts/rust-daemon/src/connectors/runtime.rs`, `hosts/rust-daemon/src/connectors/gcalendar/mod.rs`, `hosts/rust-daemon/src/routes/agents.rs`, `hosts/rust-daemon/src/routes/mod.rs`, `hosts/rust-daemon/src/history/outbox.rs`

**Interfaces:**

- Consumes: Task 2's `SessionRegistry::{contains, insert, remove, record_commit, revert_commit}`, `SessionRecord::new`, `kind_for_room`, `session_title`, `TitleContext`, room helpers, `is_owner_web_turn`, `SessionCommitUndo`; Task 8's `config_helper_parent`; Task 6's `history_outbox.enqueue_committed`.
- Produces: `crate::runs::RunLink { run_id, session_id, agent_id }` (`Clone + Debug + PartialEq + Eq`); `RunChangeSet::session_undo: Option<SessionCommitUndo>` (set by `commit_run`, used by `rollback_run`); `AgentRunRequest::parent: Option<RunLink>`; `ToolExecutionContext::with_run_link(self, Option<RunLink>) -> Self` and field `run_link`; `crate::state::RunSessionRequest<'a> { agent_id, room_id, source, delegated_parent, peer_sender, parent: Option<&RunLink>, first_text, now_ms }`; `DaemonState::ensure_run_session(&mut self, RunSessionRequest) -> bool` (true when it created the record); new signatures `AgentRunCoordinator::send_peer(sender, target, message, route, parent: Option<RunLink>)`, `delegate(&caller, target, task, parent: Option<RunLink>)`, `spawn_helper(parent_id, name, task, parent_run: Option<RunLink>)`.
- Behavior: every coordinator run makes sure its room has a session record inside the run-start save (no extra save), removes a record it created if that save fails, records the mapped session id and `parentRunId` on its ledger record, and gives its tools a `RunLink` to itself. Helper sessions record `parentSessionId`, `parentRunId`, and `parentAgentId` (falling back to the delegating agent, the peer sender, or the helper's companion). `commit_run` advances `lastActivityAtMs`, marks the owner's own turn read (source `api` or `web`, or a Telegram owner turn from the web), and titles placeholder chats; a rolled-back commit reverts exactly that. Committed messages are enqueued under the mapped session id.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `hosts/rust-daemon/src/agent_runs.rs`:

```rust
    #[tokio::test]
    async fn a_run_saves_its_session_with_the_run_start_and_advances_it_at_commit() {
        use crate::sessions::{SessionKind, SessionOrigin, TitleSource};

        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let (coordinator, agent_id) = coordinator_with_agent(
            Arc::new(GateModelAdapter {
                calls: AtomicUsize::new(0),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
            4,
        )
        .await;
        let path = snapshot_path("session-start");
        let store = ControlPlaneStoreConfig::Json(path.clone());
        coordinator
            .state
            .write()
            .await
            .set_control_plane_store(Some(store.clone()));
        let running = {
            let coordinator = coordinator.clone();
            let request = room_request(&agent_id, "chat:plan", "Plan the offsite\nsoon");
            tokio::spawn(async move { coordinator.run(request).await })
        };
        entered.acquire().await.unwrap().forget();

        let saved = load_control_plane_snapshot(&store).await.unwrap().unwrap();
        let started = saved
            .sessions
            .iter()
            .find(|session| session.id == "chat:plan")
            .expect("the run-start save carries the new session");
        assert_eq!((started.kind, started.origin), (SessionKind::Chat, SessionOrigin::Web));
        assert_eq!(started.title, "Plan the offsite");
        assert_eq!(started.title_source, TitleSource::FirstMessage);
        assert_eq!(saved.runs[0].session_id, "chat:plan");

        release.add_permits(1);
        running.await.unwrap().unwrap();
        let guard = coordinator.state.read().await;
        let session = guard.sessions.get(&agent_id, "chat:plan").unwrap();
        let messages = guard.get_agent(&agent_id).unwrap().messages;
        assert_eq!(
            session.last_activity_at_ms,
            messages.iter().map(|message| message.created_at_ms).max().unwrap()
        );
        assert_eq!(
            session.last_read_at_ms,
            Some(messages[0].created_at_ms),
            "the owner's own message is read; the reply is not"
        );
        drop(guard);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn a_failed_start_save_leaves_no_session_record() {
        let (coordinator, agent_id) = coordinator_with_agent(
            Arc::new(CapturingModelAdapter {
                requests: Arc::new(StdMutex::new(Vec::new())),
            }),
            2,
        )
        .await;
        let gate = coordinator
            .state
            .write()
            .await
            .install_test_control_plane_save_gate(true);
        gate.release.add_permits(1);

        coordinator
            .run(room_request(&agent_id, "chat:unsaved", "unsaved"))
            .await
            .expect_err("the run-start save failed");

        assert!(coordinator
            .state
            .read()
            .await
            .sessions
            .get(&agent_id, "chat:unsaved")
            .is_none());
    }

    #[tokio::test]
    async fn a_rolled_back_commit_restores_the_session_it_advanced() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let (coordinator, agent_id) = coordinator_with_agent(
            Arc::new(GateModelAdapter {
                calls: AtomicUsize::new(0),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
            4,
        )
        .await;
        let failed = {
            let coordinator = coordinator.clone();
            let request = room_request(&agent_id, "chat:lost", "lost turn");
            tokio::spawn(async move { coordinator.run(request).await })
        };
        fail_the_next_final_save(&coordinator, &entered, &release).await;
        failed.await.unwrap().expect_err("the final save failed");

        let guard = coordinator.state.read().await;
        let session = guard
            .sessions
            .get(&agent_id, "chat:lost")
            .expect("the session was saved with the run start");
        assert_eq!(
            session.last_activity_at_ms, session.created_at_ms,
            "the rolled-back turn no longer counts as activity"
        );
        assert_eq!(session.last_read_at_ms, None);
    }

    #[tokio::test]
    async fn a_delegated_run_links_its_parent_run_and_session() {
        use crate::sessions::{SessionKind, SessionOrigin};

        let adapter = Arc::new(TeamModelAdapter {
            target: StdMutex::new(String::new()),
            configs: StdMutex::new(vec![]),
        });
        let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(adapter.clone())));
        let mut manager = test_config("Manager");
        manager.tools = Some(vec![crate::tools::ToolRegistry::new()
            .descriptor("send_message")
            .unwrap()]);
        manager
            .settings
            .as_mut()
            .unwrap()
            .additional
            .insert("workspaceRole".into(), DataValue::String("lead".into()));
        let manager = state.write().await.create_agent(manager).unwrap().state;
        let mut worker_config = test_config("Alice");
        worker_config.tools = manager.config.tools.clone();
        let worker = state.write().await.create_agent(worker_config).unwrap().state;
        *adapter.target.lock().unwrap() = worker.id.clone();
        let coordinator = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(4)));

        coordinator
            .run(request(&manager.id, "Ask Alice to draft a plan"))
            .await
            .unwrap();

        let guard = state.read().await;
        let manager_run = guard.runs.for_agent(&manager.id)[0].clone();
        let worker_run = guard.runs.for_agent(&worker.id)[0].clone();
        assert_eq!(manager_run.parent_run_id, None);
        assert_eq!(worker_run.source, RunSource::Delegation);
        assert_eq!(worker_run.parent_run_id.as_deref(), Some(manager_run.id.as_str()));
        let helper = guard
            .sessions
            .get(&worker.id, &worker_run.session_id)
            .expect("the delegated room is a session");
        assert_eq!((helper.kind, helper.origin), (SessionKind::Helper, SessionOrigin::Delegation));
        assert_eq!(helper.parent_run_id.as_deref(), Some(manager_run.id.as_str()));
        assert_eq!(helper.parent_session_id.as_deref(), Some(manager_run.session_id.as_str()));
        assert_eq!(helper.parent_agent_id.as_deref(), Some(manager.id.as_str()));
        assert_eq!(helper.title, "Draft a content plan");
        let chat = guard
            .sessions
            .get(&manager.id, &manager_run.session_id)
            .expect("the manager's generated room is a session too");
        assert_eq!(chat.kind, SessionKind::Chat);
    }

    #[tokio::test]
    async fn spawned_helpers_record_the_companion_run_that_started_them() {
        use crate::sessions::{SessionKind, SessionOrigin};

        let (coordinator, _) = coordinator_with_agent(
            Arc::new(CapturingModelAdapter {
                requests: Arc::new(StdMutex::new(Vec::new())),
            }),
            4,
        )
        .await;
        let lead = helper_lead(&coordinator).await;
        let link = crate::runs::RunLink {
            run_id: "run_companion".into(),
            session_id: "chat:plan".into(),
            agent_id: lead.id.clone(),
        };

        let text = coordinator
            .spawn_helper(lead.id.clone(), "Scout".into(), "Find three sources".into(), Some(link))
            .await
            .expect("the helper runs");

        let helper_id = serde_json::from_str::<serde_json::Value>(&text).unwrap()["agentId"]
            .as_str()
            .unwrap()
            .to_string();
        let guard = coordinator.state.read().await;
        let run = guard.runs.for_agent(&helper_id)[0].clone();
        assert_eq!(run.parent_run_id.as_deref(), Some("run_companion"));
        let session = guard
            .sessions
            .get(&helper_id, &run.session_id)
            .expect("the helper room is a session");
        assert_eq!((session.kind, session.origin), (SessionKind::Helper, SessionOrigin::Delegation));
        assert_eq!(session.parent_session_id.as_deref(), Some("chat:plan"));
        assert_eq!(session.parent_agent_id.as_deref(), Some(lead.id.as_str()));
        assert_eq!(session.title, "Find three sources");
    }

    #[tokio::test]
    async fn a_peer_request_is_a_helper_session_of_the_recipient() {
        use crate::sessions::{SessionKind, SessionOrigin};

        let (coordinator, sender) = coordinator_with_agent(
            Arc::new(CapturingModelAdapter {
                requests: Arc::new(StdMutex::new(Vec::new())),
            }),
            4,
        )
        .await;
        let recipient = coordinator
            .state
            .write()
            .await
            .create_agent(test_config("Recipient"))
            .unwrap()
            .state
            .id;

        coordinator
            .send_peer(
                sender.clone(),
                recipient.clone(),
                "Check this".into(),
                anima_core::AgentCommunicationRoute::start(sender.clone()),
                None,
            )
            .await
            .expect("the peer request runs");

        let guard = coordinator.state.read().await;
        let session = guard
            .sessions
            .get(&recipient, &format!("peer:{sender}:{recipient}"))
            .expect("the peer room is a session");
        assert_eq!((session.kind, session.origin), (SessionKind::Helper, SessionOrigin::Peer));
        assert_eq!(session.parent_agent_id.as_deref(), Some(sender.as_str()));
        assert_eq!(session.parent_run_id, None);
        assert_eq!(session.title, "Messages from operator");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::tests`
Expected: compile errors such as `no field parent on type AgentRunRequest`, `this method takes 4 arguments but 5 arguments were supplied` (`send_peer`), and `cannot find struct RunLink in module crate::runs`.

- [ ] **Step 3: Add the link, session request, and commit bookkeeping**

`hosts/rust-daemon/src/runs/mod.rs`:

- add after the `pub(crate) use ledger::{…};` block:

```rust
/// The run that started another run (spec §3.2 parent fields, §4.1 `parentRunId`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunLink {
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) agent_id: String,
}
```

- add to `RunChangeSet`, after `pub(crate) undo: Option<RuntimeRunUndo>,`:

```rust
    /// Set by `DaemonState::commit_run`; read by `rollback_run`.
    pub(crate) session_undo: Option<crate::sessions::SessionCommitUndo>,
```

and `session_undo: None,` after `undo: None,` in `RunChangeSet::new`. (`RunChangeSet::session_id` stays the room id: the reply detection compares it with `Message::room_id`.)

Append to `hosts/rust-daemon/src/state/session_state.rs`:

```rust
use crate::runs::{RunLink, RunSource};
use crate::sessions::{
    connector_id_of_room, job_id_of_room, kind_for_room, schedule_id_of_room,
    session_id_for_room, session_title, SessionKind, TitleContext,
};

/// What `ensure_run_session` needs to know about a starting run.
pub(crate) struct RunSessionRequest<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) room_id: &'a str,
    pub(crate) source: RunSource,
    /// The delegating agent of a `RunRoom::Delegated` run.
    pub(crate) delegated_parent: Option<&'a str>,
    /// The sending agent of a `RunRoom::Peer` run.
    pub(crate) peer_sender: Option<&'a str>,
    pub(crate) parent: Option<&'a RunLink>,
    pub(crate) first_text: &'a str,
    pub(crate) now_ms: u64,
}

impl DaemonState {
    /// Makes sure the run's room has a session record (spec §3); returns
    /// whether it created one, so a failed run-start save can remove it.
    pub(crate) fn ensure_run_session(&mut self, request: RunSessionRequest<'_>) -> bool {
        let session_id = session_id_for_room(request.room_id);
        if self.sessions.contains(request.agent_id, &session_id) {
            return false;
        }
        let helper_parent = self
            .agents
            .get(request.agent_id)
            .and_then(|runtime| config_helper_parent(runtime.config()))
            .map(str::to_string);
        let (kind, origin) = kind_for_room(request.room_id, Some(request.source), helper_parent.is_some());
        let peer_sender_name = request
            .peer_sender
            .and_then(|id| self.agents.get(id))
            .map(|runtime| runtime.config().name.clone());
        let title_context = TitleContext {
            first_user_text: Some(request.first_text),
            schedule_prompt: schedule_id_of_room(request.room_id)
                .and_then(|id| self.schedules.get(id))
                .map(|schedule| schedule.prompt.as_str()),
            job_title: job_id_of_room(request.room_id)
                .and_then(|id| self.jobs.get(id))
                .map(|job| job.title.as_str()),
            bot_username: connector_id_of_room(request.room_id)
                .and_then(|id| self.connectors.get(id))
                .and_then(|connector| connector.bot.username.as_deref()),
            peer_sender_name: peer_sender_name.as_deref(),
        };
        let (title, title_source) = session_title(kind, origin, &title_context);
        let mut record = SessionRecord::new(
            request.agent_id,
            request.room_id,
            kind,
            origin,
            title,
            title_source,
            request.now_ms,
        );
        if kind == SessionKind::Helper {
            record.parent_session_id = request.parent.map(|link| link.session_id.clone());
            record.parent_run_id = request.parent.map(|link| link.run_id.clone());
            record.parent_agent_id = request
                .parent
                .map(|link| link.agent_id.clone())
                .or_else(|| request.delegated_parent.map(str::to_string))
                .or_else(|| request.peer_sender.map(str::to_string))
                .or(helper_parent);
        }
        self.sessions.insert(record);
        true
    }
}
```

`hosts/rust-daemon/src/state.rs`: add `pub(crate) use self::session_state::RunSessionRequest;` after `mod session_state;`.

`hosts/rust-daemon/src/state/run_commit.rs`:

- change the imports to `use anima_core::{AgentRuntime, AgentRuntimeSnapshot, AgentStatus, MessageRole, RuntimeRunBase};` and `use crate::runs::{RunChangeSet, RunError, RunOutcome, RunSource, RunStatus, AGENT_DELETED};`;
- in `commit_run`, directly after `change_set.undo = Some(runtime.apply_run_delta(&change_set.delta));`, add:

```rust
        // Spec §3.2: activity follows the commit, and the owner's own turn is read.
        let owner_authored = self
            .runs
            .get(&change_set.run_id)
            .is_some_and(|record| matches!(record.source, RunSource::Api | RunSource::Web))
            || change_set
                .delta
                .messages
                .iter()
                .find(|message| message.role == MessageRole::User)
                .is_some_and(crate::sessions::is_owner_web_turn);
        change_set.session_undo = self.sessions.record_commit(
            &change_set.agent_id,
            &crate::sessions::session_id_for_room(&change_set.session_id),
            &change_set.delta.messages,
            owner_authored,
        );
```

- at the end of `rollback_run`, add:

```rust
        if let Some(undo) = change_set.session_undo.clone() {
            self.sessions.revert_commit(undo);
        }
```

- [ ] **Step 4: Thread the parent through the coordinator and tools**

`hosts/rust-daemon/src/agent_runs.rs`:

- add to `AgentRunRequest`, after `pub(crate) source_ref: Option<String>,`:

```rust
    /// The run that started this one (a delegation, helper, or peer request);
    /// recorded on the ledger and on the helper session (spec §3.2, §4.1).
    pub(crate) parent: Option<crate::runs::RunLink>,
```

- `send_peer`: add the parameter `parent: Option<crate::runs::RunLink>,` after `route: AgentCommunicationRoute,`, and `parent,` after `source_ref: None,` in its `AgentRunRequest { … }`;
- `delegate`: change the signature to `pub(crate) fn delegate(&self, caller: &AgentState, target: String, task: String, parent: Option<crate::runs::RunLink>) -> futures::future::BoxFuture<'static, Result<String, String>>` and add `parent,` after `source_ref: None,` in its `AgentRunRequest { … }`;
- `spawn_helper`: add the parameter `parent_run: Option<crate::runs::RunLink>,` after `task: String,` (its worker already binds `parent` to the companion's snapshot), and `parent: parent_run,` after `source_ref: None,` in its `let request = AgentRunRequest { … };`;
- `run_locked`: add `parent,` to the `let AgentRunRequest { … } = request;` destructuring after `source_ref,`;
- `run_locked` Phase A: change the block's result tuple and body as follows — the tuple becomes `let (mut runtime, tool_context, base, run_id, session_id, session_created, mut in_flight, running_persist_request) = {`; directly after the `let Some((runtime, tool_context, base)) = guard.build_run_runtime(&agent_id, &room_id) else { … };` statement add:

```rust
            // Spec §3: every room is a session; a new record is saved with the
            // run start below, so no extra save is added.
            let now_ms = anima_core::primitives::now_millis();
            let session_id = crate::sessions::session_id_for_room(&room_id);
            let session_created = guard.ensure_run_session(crate::state::RunSessionRequest {
                agent_id: &agent_id,
                room_id: &room_id,
                source,
                delegated_parent: match &room {
                    RunRoom::Delegated { parent_id } => Some(parent_id.as_str()),
                    _ => None,
                },
                peer_sender: match &room {
                    RunRoom::Peer { route } => route.participants().iter().rev().nth(1).map(String::as_str),
                    _ => None,
                },
                parent: parent.as_ref(),
                first_text: &content.text,
                now_ms,
            });
```

in the `RunRecord::running(RunStart { … }, …)` call change `session_id: room_id.clone(),` to `session_id: session_id.clone(),`, `parent_run_id: None,` to `parent_run_id: parent.as_ref().map(|link| link.run_id.clone()),`, and the timestamp argument `anima_core::primitives::now_millis(),` to `now_ms,`; and the block's final tuple becomes `(runtime, tool_context, base, run_id, session_id, session_created, in_flight, guard.control_plane_persist_request())`;

- replace the run-start save failure branch

```rust
        if let Err(error) = running_persist_request.save().await {
            self.state.write().await.runs.remove(&run_id);
            in_flight.disarm();
            return Err(ApiError::service_unavailable(error.to_string()));
        }
```

with

```rust
        if let Err(error) = running_persist_request.save().await {
            let mut guard = self.state.write().await;
            guard.runs.remove(&run_id);
            if session_created {
                guard.sessions.remove(&agent_id, &session_id);
            }
            drop(guard);
            in_flight.disarm();
            return Err(ApiError::service_unavailable(error.to_string()));
        }
```

- `run_locked` Phase B: append `.with_run_link(Some(crate::runs::RunLink { run_id: run_id.clone(), session_id: session_id.clone(), agent_id: agent_id.clone() }))` to the end of the `let tool_context = tool_context.with_team(…)…;` chain (after `.with_todo_baseline(todo_baseline)`);
- `run_locked` Phase C: in the `history_outbox.enqueue_committed(…)` call (Task 6), replace `&crate::sessions::session_id_for_room(&room_id),` with `&session_id,`.

`hosts/rust-daemon/src/tools.rs`:

- add to `ToolExecutionContext`, after `peer_sources: Vec<String>,`:

```rust
    /// The run executing these tools, so the runs they start can link to it.
    pub(super) run_link: Option<crate::runs::RunLink>,
```

- add `run_link: None,` after `peer_sources: vec![],` in `ToolExecutionContext::new`;
- add after `with_peer_route`:

```rust
    pub(crate) fn with_run_link(mut self, link: Option<crate::runs::RunLink>) -> Self {
        self.run_link = link;
        self
    }
```

`hosts/rust-daemon/src/tools/team.rs`:

- `send_message`: `coordinator.send_peer(agent.id, target.id.clone(), message.into(), route)` → `coordinator.send_peer(agent.id, target.id.clone(), message.into(), route, context.run_link.clone())`;
- `broadcast_message`: add `context.run_link.clone(),` as the last argument of the `coordinator.send_peer(agent.id.clone(), target.clone(), message.clone(), route.clone())` call;
- `delegate_to_agent`: `coordinator.delegate(&agent, target, task)` → `coordinator.delegate(&agent, target, task, context.run_link.clone())`;
- `spawn_helper`: `coordinator.spawn_helper(agent.id, name, task)` → `coordinator.spawn_helper(agent.id, name, task, context.run_link.clone())`.

(These functions destructure `context.team` and `context.peer_route` by value first; `context.run_link` is a separate field, so the partial moves still compile.)

`hosts/rust-daemon/src/routes/mod.rs`, `peer_message_entry`: add `None,` as the last argument of `.send_peer(sender_id.clone(), input.to_agent_id, input.message, anima_core::AgentCommunicationRoute::start(sender_id))`.

Add `parent: None,` after the `source_ref: …,` field of every other `AgentRunRequest { … }` literal:

- `hosts/rust-daemon/src/schedules.rs`: `execute_claimed`;
- `hosts/rust-daemon/src/jobs.rs`: `JobService::execute`;
- `hosts/rust-daemon/src/jobs/tests.rs`: `a_chat_run_in_another_room_does_not_hold_back_the_agents_job`;
- `hosts/rust-daemon/src/connectors/runtime.rs`: `send_from_owner_owned`, `process_pending_once_owned`, and the tests `cross_room_rollback_preserves_a_turn_committed_while_the_connector_ran` and `agent_deletion_is_rejected_while_a_run_is_in_flight`;
- `hosts/rust-daemon/src/connectors/gcalendar/mod.rs`: `notify_agent_write_applied`;
- `hosts/rust-daemon/src/routes/agents.rs`: `handle_run_agent` and the test `direct_run_for_a_helper_fails_fast_while_its_slot_is_held`;
- `hosts/rust-daemon/src/agent_runs.rs` tests: `stable_room_passes_only_that_rooms_history_to_model`, `idempotency_key_is_propagated_to_runtime_input_metadata`, and the `request` helper (`room_request` and the `..room_request(…)` updates need no change);
- `hosts/rust-daemon/src/history/outbox.rs` tests: the `request` helper.

Add `None` as the new last argument of each remaining direct call in `agent_runs.rs` tests: `.send_peer(…)` in `independent_agents_exchange_attributed_messages_in_separate_rooms`; `.delegate(&lead, helper.state.id, "Bypass start allowance".into())` in `spawn_helper_refreshes_reused_permissions_and_rejects_direct_helper_runs`; and the five `.delegate(…)` calls in `delegation_rejects_self_missing_target_escalation_and_non_manager`.

Run `grep -rn "AgentRunRequest {" hosts/rust-daemon/src` afterwards: every match is the struct definition, the `run_locked` destructuring, a literal with `parent`, or a `..room_request(…)`/`..request(…)` update.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- agent_runs::tests state:: history:: schedules::tests jobs:: connectors:: routes::agents::tests tools::`
Expected: PASS (the 6 new coordinator tests plus every existing test in those modules).

- [ ] **Step 6: Commit**

```bash
git add hosts/rust-daemon/src/runs/mod.rs hosts/rust-daemon/src/state/session_state.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/state/run_commit.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/tools.rs hosts/rust-daemon/src/tools/team.rs hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/jobs.rs hosts/rust-daemon/src/jobs/tests.rs hosts/rust-daemon/src/connectors/runtime.rs hosts/rust-daemon/src/connectors/gcalendar/mod.rs hosts/rust-daemon/src/routes/agents.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/history/outbox.rs
git commit -m "feat(daemon): give every run's room a session and link helper runs to their parent"
```

#### Controller rulings from the pre-flight audit (binding)

1. Calendar follow-up runs (`RunRoom::Generated`, source `api`, `sourceRef = calendar-write:<id>`) create sessions with `titleSource: system` and are not marked owner-read.

---

### Task 10: Stable check-in rooms and the silent check-in memory skip

**Files:**

- Modify: `hosts/rust-daemon/src/schedules.rs` (workspace check-ins run in `schedule:<id>`; `is_checkin_content`; one test rewritten)
- Modify: `hosts/rust-daemon/src/agent_runs.rs` (no task-result memory for silent check-ins; one test)
- Modify: `hosts/rust-daemon/src/components/evaluators.rs` (no reflection for silent check-ins; one test)

**Interfaces:**

- Consumes: `crate::sessions::{schedule_room_id, is_checkin_message, SessionKind}` (Task 2); Task 9's session creation.
- Produces: `crate::schedules::is_checkin_content(&Content) -> bool`. Workspace-target automations run in `RunRoom::Stable("schedule:<scheduleId>")` (spec §9.2; connector-target check-ins keep the Telegram room). A run whose input is a check-in and whose reply is exactly `CHECKIN_OK` (trimmed) stores no task-result memory and no evaluator reflection (spec §9.2); spoken check-ins and every other run are unchanged.

- [ ] **Step 1: Write the failing tests**

In `hosts/rust-daemon/src/schedules.rs`, replace the test `due_workspace_schedule_claims_before_running_and_tags_the_generated_room` with:

```rust
    #[tokio::test]
    async fn due_workspace_schedule_runs_in_its_stable_schedule_room() {
        let (service, state, agent_id, manager) = service();
        let (record, _) = service
            .create(
                agent_id.clone(),
                "Check status".into(),
                ScheduleTrigger::Interval { interval_ms: 1_000 },
                ScheduleTarget::Workspace,
                true,
                None,
                Some(2),
                Some(1),
            )
            .await
            .unwrap();
        assert_eq!(service.tick_at(2).await.unwrap(), 1);
        assert_eq!(service.tick_at(1_002).await.unwrap(), 1, "the next occurrence fires too");
        let guard = state.read().await;
        let schedule = &guard.schedules[&record.id];
        assert_eq!(schedule.next_due_at_ms, 2_002);
        assert_eq!(schedule.last_fired.as_ref().unwrap().fired_at_ms, 1_002);
        assert_eq!(
            schedule.last_safe_outcome.as_ref().unwrap().status,
            ScheduleOutcomeStatus::Spoke
        );
        let room = crate::sessions::schedule_room_id(&record.id);
        let snapshot = guard.get_agent(&agent_id).unwrap();
        assert_eq!(snapshot.messages.len(), 4);
        assert!(
            snapshot.messages.iter().all(|message| message.room_id == room),
            "both occurrences share the automation's room"
        );
        let input = snapshot
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .unwrap();
        assert_eq!(
            input.content.metadata.as_ref().unwrap().get("kind"),
            Some(&DataValue::String("checkin".into()))
        );
        assert_eq!(
            input.content.metadata.as_ref().unwrap().get("id"),
            Some(&DataValue::String(record.id.clone()))
        );
        let session = guard
            .sessions
            .get(&agent_id, &room)
            .expect("the automation's room is a check-in session");
        assert_eq!(session.kind, crate::sessions::SessionKind::Checkin);
        assert_eq!(session.title, "Check-in · Check status");
        drop(guard);
        manager.shutdown().await;
    }
```

Append to `mod tests` in `hosts/rust-daemon/src/agent_runs.rs`:

```rust
    struct CheckinModel;

    #[async_trait]
    impl ModelAdapter for CheckinModel {
        fn provider(&self) -> &str {
            "checkin"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            let input = request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::User)
                .map(|message| message.content.text.clone())
                .unwrap_or_default();
            Ok(model_response(if input.contains("quiet") {
                "CHECKIN_OK"
            } else {
                "Two tasks are overdue"
            }))
        }
    }

    #[tokio::test]
    async fn a_silent_checkin_stores_no_task_result_memory_or_reflection() {
        let (coordinator, agent_id) = coordinator_with_agent(Arc::new(CheckinModel), 4).await;
        let checkin = |text: &str| AgentRunRequest {
            content: Content {
                text: crate::schedules::wrap_checkin_prompt(text),
                attachments: None,
                metadata: Some(BTreeMap::from([
                    ("kind".to_string(), DataValue::String("checkin".into())),
                    ("id".to_string(), DataValue::String("schedule-1".into())),
                ])),
            },
            source: RunSource::Schedule,
            ..room_request(&agent_id, "schedule:schedule-1", "unused")
        };
        let memory = coordinator.state.read().await.memory_handle();

        coordinator.run(checkin("quiet check")).await.unwrap();
        assert_eq!(memory.read().await.size(), 0, "a silent check-in leaves no memory");

        coordinator.run(checkin("loud check")).await.unwrap();
        assert!(
            memory.read().await.size() >= 1,
            "a check-in that spoke is remembered as before"
        );
    }
```

Append to `mod tests` in `hosts/rust-daemon/src/components/evaluators.rs`:

```rust
    #[tokio::test]
    async fn silent_checkins_store_no_reflection() {
        let memory = Arc::new(AsyncRwLock::new(MemoryManager::new()));
        let evaluator = ReflectionMemoryEvaluator {
            memory: memory.clone(),
            memory_embeddings: Arc::new(AsyncRwLock::new(MemoryEmbeddingRuntime::disabled())),
            memory_store: None,
        };
        let runtime = AgentRuntime::new(
            AgentConfig {
                name: "operator".into(),
                model: "gpt-5.4".into(),
                bio: None,
                lore: None,
                knowledge: None,
                topics: None,
                adjectives: None,
                style: None,
                provider: None,
                system: None,
                tools: None,
                plugins: None,
                settings: None,
            },
            Arc::new(DeterministicModelAdapter),
        );
        let message = Message {
            id: "msg-1".into(),
            agent_id: runtime.id().to_string(),
            room_id: "schedule:schedule-1".into(),
            content: Content {
                text: "Check status".into(),
                attachments: None,
                metadata: Some(BTreeMap::from([(
                    "kind".to_string(),
                    DataValue::String("checkin".into()),
                )])),
            },
            role: MessageRole::User,
            created_at_ms: 0,
        };
        let silent = Content {
            text: " CHECKIN_OK ".into(),
            attachments: None,
            metadata: None,
        };

        let result = evaluator.evaluate(&runtime, &message, &silent).await.unwrap();

        assert_eq!(result.metadata, None);
        assert_eq!(memory.read().await.size(), 0);
        let spoken = Content {
            text: "Two tasks are overdue".into(),
            attachments: None,
            metadata: None,
        };
        evaluator.evaluate(&runtime, &message, &spoken).await.unwrap();
        assert!(memory.read().await.size() >= 1, "a spoken check-in still reflects");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::tests::due_workspace_schedule agent_runs::tests::a_silent_checkin components::evaluators::tests::silent_checkins`
Expected: FAIL — the schedule messages are in two generated `room-*` rooms (`both occurrences share the automation's room`), and both memory tests find memories after the silent check-in.

- [ ] **Step 3: Implement**

`hosts/rust-daemon/src/schedules.rs`:

- in `execute_claimed`, replace `ScheduleTarget::Workspace => RunRoom::Generated,` with:

```rust
        // Spec §9.2: a workspace automation runs in its own `schedule:<id>` session.
        ScheduleTarget::Workspace => RunRoom::Stable(crate::sessions::schedule_room_id(&record.id)),
```

- add after `unwrap_checkin_prompt`:

```rust
/// Input the scheduler tagged as a check-in prompt.
pub(crate) fn is_checkin_content(content: &Content) -> bool {
    matches!(
        content.metadata.as_ref().and_then(|metadata| metadata.get("kind")),
        Some(DataValue::String(kind)) if kind == "checkin"
    )
}
```

`hosts/rust-daemon/src/agent_runs.rs`, in `run_locked`:

- directly after `let retry_key = content_retry_key(&content).map(str::to_owned);`, add:

```rust
        let checkin_input = crate::schedules::is_checkin_content(&content);
```

- replace the call

```rust
        persist_task_result_memory(
            &result,
            &snapshot.state.id,
            &snapshot.state.name,
            memory,
            memory_embeddings,
            memory_store,
        )
        .await;
```

with

```rust
        // A silent check-in stores no task-result memory; otherwise silent
        // check-ins crowd real memories out of the recent-memory context (spec §9.2).
        let silent_checkin = checkin_input
            && result
                .data
                .as_ref()
                .is_some_and(|reply| crate::schedules::is_silent_checkin_reply(&reply.text));
        if !silent_checkin {
            persist_task_result_memory(
                &result,
                &snapshot.state.id,
                &snapshot.state.name,
                memory,
                memory_embeddings,
                memory_store,
            )
            .await;
        }
```

`hosts/rust-daemon/src/components/evaluators.rs`, at the start of `evaluate`, before the existing `if response.text.trim().is_empty()` check, add:

```rust
        // A silent check-in is plumbing, not conversation (spec §9.2).
        if crate::sessions::is_checkin_message(message)
            && crate::schedules::is_silent_checkin_reply(&response.text)
        {
            return Ok(EvaluatorResult::default());
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- schedules::tests agent_runs::tests components::`
Expected: PASS, including `ready_connector_schedule_uses_stable_room_and_queues_durable_delivery` (connector check-ins keep the Telegram room).

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/components/evaluators.rs
git commit -m "feat(daemon): run workspace check-ins in their session and skip memories for silent ones"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. Interim context guard (controller ruling): when building a run's history for a `schedule:` room, exclude silent check-in pairs (the same hidden-message rule the session views use) and keep only the newest 10 turns, where a turn starts at a user message and an assistant tool-call message is never separated from its tool results. Other rooms are unchanged in M2; M3's context selection replaces this. Tests: a schedule room with 30 prior ticks (half silent) gives the model at most 10 visible turns and no `CHECKIN_OK` pairs; a tool-call turn is kept whole.

---

### Task 11: Session read routes and `GET /api/agents?view=summary`

**Files:**

- Create: `hosts/rust-daemon/src/sessions/views.rs`, `hosts/rust-daemon/src/sessions/test_support.rs`
- Modify: `hosts/rust-daemon/src/sessions/mod.rs` (declare both modules)
- Modify: `hosts/rust-daemon/src/runs/ledger.rs` (`active_count_for_session`; one test)
- Modify: `hosts/rust-daemon/src/state/session_state.rs` (`agent_summaries`)
- Create: `hosts/rust-daemon/src/routes/contracts/sessions.rs`, `hosts/rust-daemon/src/routes/sessions.rs`, `hosts/rust-daemon/src/routes/tests/sessions.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/mod.rs`, `hosts/rust-daemon/src/routes/contracts/agents.rs` (summary contracts), `hosts/rust-daemon/src/routes/agents.rs` (`handle_list_agent_summaries`), `hosts/rust-daemon/src/routes/mod.rs` (module, `ApiDoc`, routes, `list_agents_entry`, test module)
- Modify: `hosts/rust-daemon/README.md` (the `GET /api/agents` row)

**Interfaces:**

- Consumes: Task 2's `SessionRecord`, `SessionKind`, `SessionCapabilities`, `hidden_message_ids`, `is_checkin_message`, `is_inbound_message`, `preview_text`, `schedule_id_of_room`, `is_valid_session_id`, `schedules::unwrap_checkin_prompt`; Task 3's `HistoryStore`, `MessageOrder`, `MessagePageQuery`, `search_tokens`, `text_matches`, `search_snippet`, `conformance::history_message`; Task 6's `DaemonState::history` (`store()`, `is_mirrored`, `flush_once`), `HistoryService::new`, `DaemonState::set_history`, `conformance::FlakyHistoryStore::{new, set_failing}`; Task 8's `config_helper_parent`; `routes::jobs::{authorize, no_store}`.
- Produces:
  - `crate::sessions::views`: constants `DEFAULT_SESSION_PAGE` (50), `MAX_SESSION_PAGE` (200), `DEFAULT_MESSAGE_PAGE` (50), `MAX_MESSAGE_PAGE` (200), `MAX_SEARCH_QUERY_CHARS` (200); `SessionListQuery { kind, archived, q, cursor, limit, include_helpers }` (`Default`: unarchived, 50, helpers included); `SessionCursor { last_activity_at_ms, agent_id, session_id }` with `encode()` / `decode(&str) -> Option<Self>`; `SessionMatch { message_id: Option<String>, snippet }`; `SessionView { record, message_count, preview, active_runs, unread, capabilities, matched }`; `SessionPage { sessions, next_cursor }`; `MessagePageRequest { before: Option<String>, limit, include_hidden }`; `PageMessage { message, hidden }`; `MessagePage { messages (oldest first), next_before }`; `MessagePageError { NotFound, BeforeNotFound, Unavailable }`; `async fn list_sessions(&SharedDaemonState, agent_id, &SessionListQuery) -> Option<SessionPage>` (`None`: unknown agent); `async fn session_view(&SharedDaemonState, agent_id, session_id) -> Option<SessionView>`; `async fn session_messages(&SharedDaemonState, agent_id, session_id, &MessagePageRequest) -> Result<MessagePage, MessagePageError>`;
  - test-only `crate::sessions::test_support::{agent_config(name), message(agent_id, id, room_id, role, text, created_at_ms), checkin_prompt(agent_id, id, room_id, schedule_id, prompt, created_at_ms), seed_messages(&mut DaemonState, agent_id, Vec<Message>)}` (Tasks 12–13 reuse them);
  - `RunLedger::active_count_for_session(agent_id, session_id) -> usize` (queued, running, or awaiting approval);
  - `DaemonState::agent_summaries() -> Vec<AgentRuntimeSnapshot>` (no messages or events; `message_count` kept; ordered like `list_agents`);
  - contracts `SessionResponse` (camelCase: `id, agentId, roomId, kind, origin, title, titleSource, createdAtMs, lastActivityAtMs, lastReadAtMs, archived, parentSessionId, parentRunId, parentAgentId, summary, contextTrimmed, messageCount, preview, activeRuns, pendingApprovals` (always 0 until M4)`, unread, capabilities, match?`), `SessionCapabilitiesResponse`, `SessionMatchResponse { messageId, snippet }`, `SessionsEnvelope { sessions, nextCursor }`, `SessionEnvelope { session }`, `SessionMessageResponse { id, role, text, attachments: [{ type, name }], metadata, createdAtMs, hidden? }`, `SessionMessagesEnvelope { messages, nextBefore }`, `EXPOSED_MESSAGE_METADATA`; `AgentSummaryResponse { state, messageCount, eventCount, lastTask }`, `AgentSummariesEnvelope { agents }`;
  - routes (owner `authorize_read`, `Cache-Control: no-store` on every answer, `#[utoipa::path]`, tag `sessions`): `GET /api/agents/{agent_id}/sessions` (`routes::sessions::list_sessions`), `GET /api/agents/{agent_id}/sessions/{session_id}` (`get_session`), `GET /api/agents/{agent_id}/sessions/{session_id}/messages` (`list_session_messages`); `GET /api/agents?view=summary`.
- Error strings: `malformed query`, `kind must be one of chat, telegram, checkin, job, helper`, `archived must be true or false`, `includeHelpers must be true or false`, `includeHidden must be true or false`, `limit must be between 1 and 200`, `cursor is invalid`, `q must be at most 200 characters`, `before message was not found`, `history store is unavailable` (503), `view must be summary`, and `not found` (404: unknown agent or session).
- Behavior: lists are sorted by `lastActivityAtMs` descending, then agent id and session id; the cursor is the base64url JSON `[lastActivityAtMs, agentId, sessionId]` of the last item. `archived=false` (default) lists unarchived sessions and `archived=true` only archived ones. `includeHelpers` (default true) adds sessions whose `parentAgentId` is the agent and sessions of the agent's helpers. `q` matches visible hot messages (substring), then history rows (word prefix), then the title (`match.messageId: null`); a query with no letters or digits matches nothing. `messageCount` is the store's visible count plus visible hot messages not yet mirrored (hot-only when the store cannot be read). `preview` is the newest visible user or assistant message (check-in prompts without the scheduler suffix), from the store when the hot tail has none. `unread` is a visible assistant or Telegram-inbound hot message newer than `lastReadAtMs`. Message pages merge the store page with the hot tail by id, hide silent check-in turns unless `includeHidden=true`, and fall back to the hot tail when the store cannot be read (a `before` that only the store could resolve then answers 503). Message metadata is limited to `toolCalls, toolCallId, stepId, runId, stopped, revised, steer, skill, clientRequestId, communication` (spec §3.3) plus `kind` and `source`.

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/sessions/test_support.rs`:

```rust
//! Helpers shared by the session, route, and pruning tests.

use std::collections::BTreeMap;

use anima_core::{
    AgentConfig, AgentSettings, Content, DataValue, Message, MessageRole, RuntimeRunDelta,
    TokenUsage,
};

use crate::state::DaemonState;

pub(crate) fn agent_config(name: &str) -> AgentConfig {
    AgentConfig {
        name: name.into(),
        model: "gpt-5.4".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some("openai".into()),
        system: None,
        tools: None,
        plugins: None,
        settings: Some(AgentSettings::default()),
    }
}

pub(crate) fn message(
    agent_id: &str,
    id: &str,
    room_id: &str,
    role: MessageRole,
    text: &str,
    created_at_ms: u64,
) -> Message {
    Message {
        id: id.into(),
        agent_id: agent_id.into(),
        room_id: room_id.into(),
        content: Content {
            text: text.into(),
            ..Content::default()
        },
        role,
        created_at_ms,
    }
}

/// A scheduler check-in prompt for `schedule_id`.
pub(crate) fn checkin_prompt(
    agent_id: &str,
    id: &str,
    room_id: &str,
    schedule_id: &str,
    prompt: &str,
    created_at_ms: u64,
) -> Message {
    let mut checkin = message(
        agent_id,
        id,
        room_id,
        MessageRole::User,
        &crate::schedules::wrap_checkin_prompt(prompt),
        created_at_ms,
    );
    checkin.content.metadata = Some(BTreeMap::from([
        ("kind".to_string(), DataValue::String("checkin".into())),
        ("id".to_string(), DataValue::String(schedule_id.into())),
    ]));
    checkin
}

/// Appends `messages` to the agent's canonical transcript, as a commit would.
pub(crate) fn seed_messages(state: &mut DaemonState, agent_id: &str, messages: Vec<Message>) {
    let runtime = state
        .agents
        .get_mut(agent_id)
        .expect("the seeded agent exists");
    let status = runtime.state().status;
    runtime.apply_run_delta(&RuntimeRunDelta {
        messages,
        events: Vec::new(),
        event_total: 0,
        token_usage: TokenUsage::default(),
        step_count: 0,
        last_task: None,
        status,
    });
}
```

In `hosts/rust-daemon/src/sessions/mod.rs`, add after `pub(crate) mod migration;`:

```rust
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod views;
```

Create `hosts/rust-daemon/src/sessions/views.rs` with only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anima_core::MessageRole;
    use tokio::sync::RwLock;

    use super::*;
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::{HistoryService, HistoryStore};
    use crate::sessions::test_support::{agent_config, checkin_prompt, message, seed_messages};
    use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
    use crate::state::DaemonState;

    fn session(agent_id: &str, room_id: &str, kind: SessionKind, title: &str, at: u64) -> SessionRecord {
        let origin = match kind {
            SessionKind::Chat => SessionOrigin::Web,
            SessionKind::Telegram => SessionOrigin::Telegram,
            SessionKind::Checkin => SessionOrigin::Schedule,
            SessionKind::Job => SessionOrigin::Job,
            SessionKind::Helper => SessionOrigin::Delegation,
        };
        SessionRecord::new(agent_id, room_id, kind, origin, title.into(), TitleSource::System, at)
    }

    fn ids(page: &MessagePage) -> Vec<String> {
        page.messages
            .iter()
            .map(|entry| entry.message.id.clone())
            .collect()
    }

    fn first_page(include_hidden: bool) -> MessagePageRequest {
        MessagePageRequest {
            before: None,
            limit: 10,
            include_hidden,
        }
    }

    #[tokio::test]
    async fn lists_sessions_newest_first_with_derived_fields_and_cursor_pages() {
        let mut daemon = DaemonState::new();
        let agent = daemon.create_agent(agent_config("companion")).unwrap().state.id;
        let mut plans = session(&agent, "chat:plans", SessionKind::Chat, "Plans", 10);
        plans.last_activity_at_ms = 30;
        plans.last_read_at_ms = Some(21);
        let mut bot = session(&agent, "telegram:bot", SessionKind::Telegram, "Telegram · @bot", 5);
        bot.last_activity_at_ms = 20;
        let mut old = session(&agent, "chat:old", SessionKind::Chat, "Old", 1);
        old.archived = true;
        for record in [plans, bot, old] {
            daemon.sessions.insert(record);
        }
        seed_messages(
            &mut daemon,
            &agent,
            vec![
                message(&agent, "p1", "chat:plans", MessageRole::User, "Plan the offsite", 21),
                message(&agent, "p2", "chat:plans", MessageRole::Assistant, "Here is a plan for the offsite", 30),
                message(&agent, "t1", "telegram:bot", MessageRole::Assistant, "Morning!", 20),
            ],
        );
        let state = Arc::new(RwLock::new(daemon));

        let first = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                limit: 1,
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(first.sessions.len(), 1);
        let plans = &first.sessions[0];
        assert_eq!(plans.record.id, "chat:plans");
        assert_eq!(plans.message_count, 2);
        assert_eq!(plans.preview.as_deref(), Some("Here is a plan for the offsite"));
        assert!(plans.unread, "the reply is newer than lastReadAtMs");
        assert!(plans.capabilities.delete);
        let cursor = SessionCursor::decode(first.next_cursor.as_deref().unwrap()).unwrap();

        let second = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                limit: 1,
                cursor: Some(cursor),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            second.sessions.iter().map(|view| view.record.id.as_str()).collect::<Vec<_>>(),
            ["telegram:bot"]
        );
        assert!(!second.sessions[0].capabilities.delete);
        assert_eq!(second.next_cursor, None, "archived sessions are listed separately");

        let archived = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                archived: true,
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            archived.sessions.iter().map(|view| view.record.id.as_str()).collect::<Vec<_>>(),
            ["chat:old"]
        );
        let telegram = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                kind: Some(SessionKind::Telegram),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(telegram.sessions.len(), 1);
        assert!(list_sessions(&state, "missing", &SessionListQuery::default())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn lists_include_helper_sessions_of_the_agent_unless_excluded() {
        let mut daemon = DaemonState::new();
        let companion = daemon.create_agent(agent_config("companion")).unwrap().state.id;
        let specialist = daemon.create_agent(agent_config("specialist")).unwrap().state.id;
        daemon
            .sessions
            .insert(session(&companion, "chat:plans", SessionKind::Chat, "Plans", 10));
        let mut delegated = session(&specialist, "room-9", SessionKind::Helper, "Draft a plan", 12);
        delegated.parent_agent_id = Some(companion.clone());
        delegated.parent_session_id = Some("chat:plans".into());
        daemon.sessions.insert(delegated);
        daemon
            .sessions
            .insert(session(&specialist, "chat:own", SessionKind::Chat, "Own chat", 11));
        let state = Arc::new(RwLock::new(daemon));

        let with_helpers = list_sessions(&state, &companion, &SessionListQuery::default())
            .await
            .unwrap();
        assert_eq!(
            with_helpers
                .sessions
                .iter()
                .map(|view| (view.record.agent_id.as_str(), view.record.id.as_str()))
                .collect::<Vec<_>>(),
            [(specialist.as_str(), "room-9"), (companion.as_str(), "chat:plans")]
        );
        assert!(!with_helpers.sessions[0].capabilities.send, "helper sessions are read-only");
        let own = list_sessions(
            &state,
            &companion,
            &SessionListQuery {
                include_helpers: false,
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(own.sessions.len(), 1);
    }

    #[tokio::test]
    async fn search_matches_hot_messages_then_the_history_store_then_titles() {
        let mut daemon = DaemonState::new();
        let agent = daemon.create_agent(agent_config("companion")).unwrap().state.id;
        for (room, title, at) in [
            ("chat:hot", "Groceries", 30),
            ("chat:cold", "Travel", 20),
            ("chat:title", "Budget review", 10),
            ("chat:none", "Nothing", 5),
        ] {
            daemon
                .sessions
                .insert(session(&agent, room, SessionKind::Chat, title, at));
        }
        seed_messages(
            &mut daemon,
            &agent,
            vec![message(&agent, "h1", "chat:hot", MessageRole::User, "Buy budget apples", 30)],
        );
        daemon
            .history
            .store()
            .upsert_messages(&[history_message(
                "c1",
                &agent,
                "chat:cold",
                MessageRole::Assistant,
                "The budget for Lisbon",
                20,
            )])
            .await
            .unwrap();
        let state = Arc::new(RwLock::new(daemon));

        let page = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                q: Some("budget".into()),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            page.sessions
                .iter()
                .map(|view| (
                    view.record.id.as_str(),
                    view.matched.as_ref().map(|found| found.message_id.as_deref())
                ))
                .collect::<Vec<_>>(),
            [
                ("chat:hot", Some(Some("h1"))),
                ("chat:cold", Some(Some("c1"))),
                ("chat:title", Some(None)),
            ]
        );
        assert_eq!(page.sessions[0].matched.as_ref().unwrap().snippet, "Buy budget apples");
        let wordless = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                q: Some("!!".into()),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert!(wordless.sessions.is_empty(), "a query without words matches nothing");
    }

    #[tokio::test]
    async fn message_pages_merge_the_store_with_the_hot_tail_without_duplicates() {
        let mut daemon = DaemonState::new();
        let agent = daemon.create_agent(agent_config("companion")).unwrap().state.id;
        daemon
            .sessions
            .insert(session(&agent, "chat:plans", SessionKind::Chat, "Plans", 1));
        // m1 and m2 were pruned: only the store has them.
        daemon
            .history
            .store()
            .upsert_messages(&[
                history_message("m1", &agent, "chat:plans", MessageRole::User, "one", 1),
                history_message("m2", &agent, "chat:plans", MessageRole::Assistant, "two", 2),
            ])
            .await
            .unwrap();
        seed_messages(
            &mut daemon,
            &agent,
            vec![
                message(&agent, "m3", "chat:plans", MessageRole::User, "three", 3),
                message(&agent, "m4", "chat:plans", MessageRole::Assistant, "four", 4),
            ],
        );
        let state = Arc::new(RwLock::new(daemon));
        let history = state.read().await.history.clone();
        history.flush_once(&state, 5).await.unwrap(); // m3 and m4 are now in both places
        seed_messages(
            &mut *state.write().await,
            &agent,
            vec![
                message(&agent, "m5", "chat:plans", MessageRole::User, "five", 5),
                message(&agent, "m6", "chat:plans", MessageRole::Assistant, "six", 6),
            ],
        );

        let newest = session_messages(
            &state,
            &agent,
            "chat:plans",
            &MessagePageRequest {
                before: None,
                limit: 3,
                include_hidden: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(ids(&newest), ["m4", "m5", "m6"]);
        assert_eq!(newest.next_before.as_deref(), Some("m4"));
        let older = session_messages(
            &state,
            &agent,
            "chat:plans",
            &MessagePageRequest {
                before: newest.next_before.clone(),
                limit: 3,
                include_hidden: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(ids(&older), ["m1", "m2", "m3"]);
        assert_eq!(older.next_before, None);
        let view = session_view(&state, &agent, "chat:plans").await.unwrap();
        assert_eq!(view.message_count, 6, "store rows plus unmirrored hot messages, each once");
        assert_eq!(
            session_messages(
                &state,
                &agent,
                "chat:plans",
                &MessagePageRequest {
                    before: Some("missing".into()),
                    limit: 3,
                    include_hidden: false,
                },
            )
            .await,
            Err(MessagePageError::BeforeNotFound)
        );
        assert_eq!(
            session_messages(&state, &agent, "chat:missing", &first_page(false)).await,
            Err(MessagePageError::NotFound)
        );
    }

    #[tokio::test]
    async fn silent_checkins_are_hidden_and_an_unreadable_store_leaves_the_hot_tail() {
        let flaky = Arc::new(FlakyHistoryStore::new());
        let mut daemon = DaemonState::new();
        daemon.set_history(HistoryService::new(flaky.clone()));
        let agent = daemon.create_agent(agent_config("companion")).unwrap().state.id;
        daemon.sessions.insert(session(
            &agent,
            "schedule:s1",
            SessionKind::Checkin,
            "Check-in · Check status",
            1,
        ));
        flaky
            .upsert_messages(&[history_message(
                "old",
                &agent,
                "schedule:s1",
                MessageRole::Assistant,
                "Earlier update",
                1,
            )])
            .await
            .unwrap();
        seed_messages(
            &mut daemon,
            &agent,
            vec![
                checkin_prompt(&agent, "c1", "schedule:s1", "s1", "Check status", 2),
                message(&agent, "c2", "schedule:s1", MessageRole::Assistant, "CHECKIN_OK", 3),
                checkin_prompt(&agent, "c3", "schedule:s1", "s1", "Check status", 4),
                message(&agent, "c4", "schedule:s1", MessageRole::Assistant, "Two tasks are overdue", 5),
            ],
        );
        let state = Arc::new(RwLock::new(daemon));

        let visible = session_messages(&state, &agent, "schedule:s1", &first_page(false))
            .await
            .unwrap();
        assert_eq!(ids(&visible), ["old", "c3", "c4"]);
        let everything = session_messages(&state, &agent, "schedule:s1", &first_page(true))
            .await
            .unwrap();
        assert_eq!(ids(&everything), ["old", "c1", "c2", "c3", "c4"]);
        assert!(everything.messages[1].hidden && everything.messages[2].hidden);

        flaky.set_failing(true);
        let hot_only = session_messages(&state, &agent, "schedule:s1", &first_page(false))
            .await
            .unwrap();
        assert_eq!(ids(&hot_only), ["c3", "c4"], "an unreadable store leaves the hot tail");
        assert_eq!(
            session_messages(
                &state,
                &agent,
                "schedule:s1",
                &MessagePageRequest {
                    before: Some("old".into()),
                    limit: 10,
                    include_hidden: false,
                },
            )
            .await,
            Err(MessagePageError::Unavailable)
        );
        let view = session_view(&state, &agent, "schedule:s1").await.unwrap();
        assert_eq!(view.message_count, 2, "counts fall back to the visible hot messages");
        assert_eq!(view.preview.as_deref(), Some("Two tasks are overdue"));
    }
}
```

Append to `mod tests` in `hosts/rust-daemon/src/runs/ledger.rs`:

```rust
    #[test]
    fn active_runs_are_counted_per_session() {
        let mut ledger = RunLedger::default();
        let running = record("agent-a", 1);
        let mut queued = record("agent-a", 2);
        queued.status = RunStatus::Queued;
        let done = finished("agent-a", 3);
        let mut elsewhere = record("agent-a", 4);
        elsewhere.session_id = "chat:other".into();
        for run in [running, queued, done, elsewhere] {
            ledger.insert(run);
        }

        assert_eq!(ledger.active_count_for_session("agent-a", "direct:test"), 2);
        assert_eq!(ledger.active_count_for_session("agent-a", "chat:other"), 1);
        assert_eq!(ledger.active_count_for_session("agent-b", "direct:test"), 0);
    }
```

Create `hosts/rust-daemon/src/routes/tests/sessions.rs`:

```rust
use super::*;
use anima_core::MessageRole;

use crate::sessions::test_support::{message, seed_messages};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

const OWNER_ORIGIN: &str = "http://localhost:4200";

fn get(uri: &str, origin: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap()
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn app_with_session() -> (axum::Router, Arc<RwLock<DaemonState>>, String) {
    let mut daemon = DaemonState::new();
    let agent = daemon.create_agent(test_config("companion")).unwrap().state.id;
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        "chat:plans",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Plans".into(),
        TitleSource::FirstMessage,
        1,
    ));
    seed_messages(
        &mut daemon,
        &agent,
        vec![
            message(&agent, "m1", "chat:plans", MessageRole::User, "Plan the offsite", 1),
            message(&agent, "m2", "chat:plans", MessageRole::Assistant, "Here is the plan", 2),
        ],
    );
    let state = Arc::new(RwLock::new(daemon));
    (router(state.clone(), DaemonConfig::default()), state, agent)
}

#[tokio::test]
async fn session_reads_require_the_owner_and_are_never_cached() {
    let (app, _, agent) = app_with_session().await;
    for path in [
        format!("/api/agents/{agent}/sessions"),
        format!("/api/agents/{agent}/sessions/chat%3Aplans"),
        format!("/api/agents/{agent}/sessions/chat%3Aplans/messages"),
    ] {
        let response = app
            .clone()
            .oneshot(get(&path, "https://untrusted.example"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let response = app.clone().oneshot(get(&path, OWNER_ORIGIN)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn session_routes_list_one_session_and_page_its_messages() {
    let (app, _, agent) = app_with_session().await;
    let list = json(
        app.clone()
            .oneshot(get(&format!("/api/agents/{agent}/sessions?limit=10"), OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    let session = &list["sessions"][0];
    assert_eq!(session["id"], "chat:plans");
    assert_eq!(session["agentId"], agent.as_str());
    assert_eq!(session["roomId"], "chat:plans");
    assert_eq!(session["kind"], "chat");
    assert_eq!(session["origin"], "web");
    assert_eq!(session["titleSource"], "first_message");
    assert_eq!(session["messageCount"], 2);
    assert_eq!(session["preview"], "Here is the plan");
    assert_eq!(session["unread"], true);
    assert_eq!(session["activeRuns"], 0);
    assert_eq!(session["pendingApprovals"], 0);
    assert_eq!(session["capabilities"]["delete"], true);
    assert!(session.get("match").is_none());
    assert_eq!(list["nextCursor"], serde_json::Value::Null);

    let one = json(
        app.clone()
            .oneshot(get(&format!("/api/agents/{agent}/sessions/chat%3Aplans"), OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(one["session"]["title"], "Plans");

    let page = json(
        app.clone()
            .oneshot(get(
                &format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?limit=1"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(page["messages"][0]["id"], "m2");
    assert_eq!(page["messages"][0]["role"], "assistant");
    assert_eq!(page["messages"][0]["text"], "Here is the plan");
    assert_eq!(page["nextBefore"], "m2");
    let older = json(
        app.clone()
            .oneshot(get(
                &format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?limit=1&before=m2"),
                OWNER_ORIGIN,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(older["messages"][0]["id"], "m1");
    assert_eq!(older["nextBefore"], serde_json::Value::Null);

    for (path, status, error) in [
        ("/api/agents/missing/sessions".to_string(), StatusCode::NOT_FOUND, "not found"),
        (format!("/api/agents/{agent}/sessions/chat%3Amissing"), StatusCode::NOT_FOUND, "not found"),
        (format!("/api/agents/{agent}/sessions?limit=0"), StatusCode::BAD_REQUEST, "limit must be between 1 and 200"),
        (format!("/api/agents/{agent}/sessions?kind=bogus"), StatusCode::BAD_REQUEST, "kind must be one of chat, telegram, checkin, job, helper"),
        (format!("/api/agents/{agent}/sessions?archived=maybe"), StatusCode::BAD_REQUEST, "archived must be true or false"),
        (format!("/api/agents/{agent}/sessions?cursor=%21"), StatusCode::BAD_REQUEST, "cursor is invalid"),
        (format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?before=nope"), StatusCode::BAD_REQUEST, "before message was not found"),
        (format!("/api/agents/{agent}/sessions/chat%3Aplans/messages?limit=201"), StatusCode::BAD_REQUEST, "limit must be between 1 and 200"),
    ] {
        let response = app.clone().oneshot(get(&path, OWNER_ORIGIN)).await.unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store", "{path}");
        assert_eq!(json(response).await["error"], error, "{path}");
    }
}

#[tokio::test]
async fn listing_agents_as_summaries_omits_their_messages() {
    let (app, _, agent) = app_with_session().await;
    let summaries = json(
        app.clone()
            .oneshot(get("/api/agents?view=summary", OWNER_ORIGIN))
            .await
            .unwrap(),
    )
    .await;
    let summary = &summaries["agents"][0];
    assert_eq!(summary["state"]["id"], agent.as_str());
    assert_eq!(summary["messageCount"], 2);
    assert!(summary.get("messages").is_none());
    let full = json(app.clone().oneshot(get("/api/agents", OWNER_ORIGIN)).await.unwrap()).await;
    assert_eq!(
        full["agents"][0]["messages"].as_array().unwrap().len(),
        2,
        "the default response is unchanged"
    );
    let response = app
        .clone()
        .oneshot(get("/api/agents?view=full", OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["error"], "view must be summary");
}

#[test]
fn the_openapi_document_lists_the_session_read_routes() {
    use utoipa::OpenApi;

    let paths = crate::routes::ApiDoc::openapi().paths.paths;
    for path in [
        "/api/agents/{agent_id}/sessions",
        "/api/agents/{agent_id}/sessions/{session_id}",
        "/api/agents/{agent_id}/sessions/{session_id}/messages",
    ] {
        assert!(paths.contains_key(path), "{path}");
    }
}
```

In `hosts/rust-daemon/src/routes/mod.rs`, in `mod tests`, add `mod sessions;` after `mod swarm_reliability;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions::views runs::ledger::tests::active_runs routes::tests::sessions`
Expected: compile errors such as `cannot find function list_sessions in this scope`, `cannot find struct, variant or union type SessionListQuery in this scope`, and `no method named active_count_for_session found for struct RunLedger`.

- [ ] **Step 3: Add the ledger count and agent summaries**

In `hosts/rust-daemon/src/runs/ledger.rs`, add to `impl RunLedger` after `in_flight_count`:

```rust
    /// Runs of this session that are queued, running, or awaiting approval.
    pub(crate) fn active_count_for_session(&self, agent_id: &str, session_id: &str) -> usize {
        self.records
            .values()
            .filter(|record| {
                record.agent_id == agent_id
                    && record.session_id == session_id
                    && !record.status.is_terminal()
            })
            .count()
    }
```

Append to `impl DaemonState` in `hosts/rust-daemon/src/state/session_state.rs` (and add `AgentRuntimeSnapshot` to its `use anima_core::{…};` line):

```rust
    /// Agents without their transcripts or events, ordered like `list_agents`
    /// (`GET /api/agents?view=summary`, spec §3.3).
    pub(crate) fn agent_summaries(&self) -> Vec<AgentRuntimeSnapshot> {
        let mut summaries = self
            .agent_snapshots
            .iter()
            .filter(|(agent_id, _)| !self.agents.contains_key(*agent_id))
            .map(|(_, snapshot)| AgentRuntimeSnapshot {
                state: snapshot.state.clone(),
                message_count: snapshot.message_count,
                messages: Vec::new(),
                event_count: snapshot.event_count,
                events: Vec::new(),
                last_task: snapshot.last_task.clone(),
                step_count: snapshot.step_count,
            })
            .collect::<Vec<_>>();
        summaries.extend(self.agents.values().map(|runtime| AgentRuntimeSnapshot {
            message_count: runtime.messages().len(),
            ..runtime.run_snapshot(Vec::new())
        }));
        let mut summaries = summaries
            .into_iter()
            .map(|summary| self.with_derived_status(summary))
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            left.state
                .created_at_ms
                .cmp(&right.state.created_at_ms)
                .then_with(|| left.state.id.cmp(&right.state.id))
        });
        summaries
    }
```

(The `use anima_core::AgentConfigUpdate;` line of Task 8 becomes `use anima_core::{AgentConfigUpdate, AgentRuntimeSnapshot};`.)

- [ ] **Step 4: Implement the views**

Put this above the test module in `hosts/rust-daemon/src/sessions/views.rs`:

```rust
//! Per-response session views (spec §3.2, §3.3): derived fields, lists with
//! search and cursors, and message pages that merge the history store with
//! the control plane's hot tail. The state lock is never held across a
//! history-store call, and an unreadable store degrades a view to the hot tail.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use anima_core::{Message, MessageRole};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use tracing::warn;

use super::{
    hidden_message_ids, is_checkin_message, is_inbound_message, preview_text,
    schedule_id_of_room, SessionCapabilities, SessionKind, SessionRecord,
};
use crate::agent_runs::config_helper_parent;
use crate::app::SharedDaemonState;
use crate::history::{
    search_snippet, search_tokens, text_matches, HistoryStore, MessageOrder, MessagePageQuery,
};
use crate::state::DaemonState;

/// Sessions per list page by default and at most (spec §3.3).
pub(crate) const DEFAULT_SESSION_PAGE: usize = 50;
pub(crate) const MAX_SESSION_PAGE: usize = 200;
/// Messages per page by default and at most.
pub(crate) const DEFAULT_MESSAGE_PAGE: usize = 50;
pub(crate) const MAX_MESSAGE_PAGE: usize = 200;
/// The longest accepted search query, in characters.
pub(crate) const MAX_SEARCH_QUERY_CHARS: usize = 200;
/// History rows one search reads at most.
const SEARCH_ROW_LIMIT: usize = 500;
/// History rows read for a preview when the hot tail has none.
const PREVIEW_ROW_LIMIT: usize = 20;

/// The filters of `GET /api/agents/{id}/sessions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionListQuery {
    pub(crate) kind: Option<SessionKind>,
    /// `false` lists unarchived sessions; `true` lists only archived ones.
    pub(crate) archived: bool,
    pub(crate) q: Option<String>,
    pub(crate) cursor: Option<SessionCursor>,
    pub(crate) limit: usize,
    /// Adds helper and delegated sessions whose `parentAgentId` is the agent.
    pub(crate) include_helpers: bool,
}

impl Default for SessionListQuery {
    fn default() -> Self {
        Self {
            kind: None,
            archived: false,
            q: None,
            cursor: None,
            limit: DEFAULT_SESSION_PAGE,
            include_helpers: true,
        }
    }
}

/// A position in the list order: newest activity first, then agent and id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionCursor {
    pub(crate) last_activity_at_ms: u64,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
}

impl SessionCursor {
    fn of(record: &SessionRecord) -> Self {
        Self {
            last_activity_at_ms: record.last_activity_at_ms,
            agent_id: record.agent_id.clone(),
            session_id: record.id.clone(),
        }
    }

    pub(crate) fn encode(&self) -> String {
        let value = serde_json::json!([self.last_activity_at_ms, self.agent_id, self.session_id]);
        URL_SAFE_NO_PAD.encode(value.to_string())
    }

    pub(crate) fn decode(value: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
        let (last_activity_at_ms, agent_id, session_id) =
            serde_json::from_slice::<(u64, String, String)>(&bytes).ok()?;
        Some(Self {
            last_activity_at_ms,
            agent_id,
            session_id,
        })
    }

    fn key(&self) -> (Reverse<u64>, &str, &str) {
        (
            Reverse(self.last_activity_at_ms),
            self.agent_id.as_str(),
            self.session_id.as_str(),
        )
    }
}

fn sort_key(record: &SessionRecord) -> (Reverse<u64>, &str, &str) {
    (
        Reverse(record.last_activity_at_ms),
        record.agent_id.as_str(),
        record.id.as_str(),
    )
}

/// Why a list item matched a search; `message_id` is `None` for a title match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionMatch {
    pub(crate) message_id: Option<String>,
    pub(crate) snippet: String,
}

/// A session record with its derived fields (spec §3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionView {
    pub(crate) record: SessionRecord,
    pub(crate) message_count: usize,
    pub(crate) preview: Option<String>,
    pub(crate) active_runs: usize,
    pub(crate) unread: bool,
    pub(crate) capabilities: SessionCapabilities,
    pub(crate) matched: Option<SessionMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionPage {
    pub(crate) sessions: Vec<SessionView>,
    pub(crate) next_cursor: Option<String>,
}

/// `GET …/sessions/{sid}/messages` parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MessagePageRequest {
    pub(crate) before: Option<String>,
    pub(crate) limit: usize,
    pub(crate) include_hidden: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PageMessage {
    pub(crate) message: Message,
    /// Part of a silent check-in turn.
    pub(crate) hidden: bool,
}

/// One page of a session's messages, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MessagePage {
    pub(crate) messages: Vec<PageMessage>,
    pub(crate) next_before: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessagePageError {
    NotFound,
    BeforeNotFound,
    /// The page needs the history store and it could not be read.
    Unavailable,
}

/// What the state lock yields for one session; the rest is added without it.
struct Candidate {
    record: SessionRecord,
    /// Visible hot messages the store is not known to hold.
    unmirrored_visible: usize,
    /// Every visible hot message (the count when the store cannot be read).
    hot_visible: usize,
    preview: Option<String>,
    unread: bool,
    active_runs: usize,
    capabilities: SessionCapabilities,
    matched: Option<SessionMatch>,
}

/// The text a person sees: a check-in prompt without the scheduler's suffix.
fn display_text(message: &Message) -> &str {
    if is_checkin_message(message) {
        crate::schedules::unwrap_checkin_prompt(&message.content.text)
    } else {
        &message.content.text
    }
}

fn preview_from_newest<'a>(newest_first: impl Iterator<Item = &'a Message>) -> Option<String> {
    newest_first
        .filter(|message| matches!(message.role, MessageRole::User | MessageRole::Assistant))
        .find_map(|message| preview_text(display_text(message)))
}

fn hot_rooms<'a>(state: &'a DaemonState, agent_id: &str) -> HashMap<&'a str, Vec<&'a Message>> {
    let mut rooms: HashMap<&str, Vec<&Message>> = HashMap::new();
    if let Some(runtime) = state.agents.get(agent_id) {
        for message in runtime.messages() {
            rooms
                .entry(message.room_id.as_str())
                .or_default()
                .push(message);
        }
    }
    rooms
}

fn candidate(
    state: &DaemonState,
    record: &SessionRecord,
    hot: &[&Message],
    tokens: &[String],
) -> Candidate {
    let hidden_ids = hidden_message_ids(hot.iter().copied());
    let visible = hot
        .iter()
        .copied()
        .filter(|message| !hidden_ids.contains(&message.id))
        .collect::<Vec<_>>();
    let read_through = record.last_read_at_ms.unwrap_or(0);
    let schedule_exists = record.kind == SessionKind::Checkin
        && schedule_id_of_room(record.room_id()).is_some_and(|id| state.schedules.contains_key(id));
    let matched = if tokens.is_empty() {
        None
    } else {
        visible
            .iter()
            .rev()
            .find(|message| text_matches(&message.content.text, tokens))
            .map(|message| SessionMatch {
                message_id: Some(message.id.clone()),
                snippet: search_snippet(&message.content.text, tokens),
            })
    };
    Candidate {
        record: record.clone(),
        unmirrored_visible: visible
            .iter()
            .filter(|message| !state.history.is_mirrored(&message.id))
            .count(),
        hot_visible: visible.len(),
        preview: preview_from_newest(visible.iter().rev().copied()),
        unread: visible.iter().any(|message| {
            (message.role == MessageRole::Assistant || is_inbound_message(message))
                && message.created_at_ms > read_through
        }),
        active_runs: state
            .runs
            .active_count_for_session(&record.agent_id, &record.id),
        capabilities: record.capabilities(schedule_exists),
        matched,
    }
}

fn collect_candidates(
    state: &DaemonState,
    agent_id: &str,
    include_helpers: bool,
    select: impl Fn(&SessionRecord) -> bool,
    tokens: &[String],
) -> Vec<Candidate> {
    let helpers = if include_helpers {
        state
            .agents
            .iter()
            .filter(|(_, runtime)| config_helper_parent(runtime.config()) == Some(agent_id))
            .map(|(id, _)| id.as_str())
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let records = state
        .sessions
        .records()
        .filter(|record| {
            state.agents.contains_key(&record.agent_id)
                && (record.agent_id == agent_id
                    || (include_helpers
                        && (record.parent_agent_id.as_deref() == Some(agent_id)
                            || helpers.contains(record.agent_id.as_str()))))
                && select(record)
        })
        .collect::<Vec<_>>();
    let mut rooms_by_agent: HashMap<&str, HashMap<&str, Vec<&Message>>> = HashMap::new();
    for record in &records {
        rooms_by_agent
            .entry(record.agent_id.as_str())
            .or_insert_with(|| hot_rooms(state, &record.agent_id));
    }
    records
        .into_iter()
        .map(|record| {
            let hot = rooms_by_agent
                .get(record.agent_id.as_str())
                .and_then(|rooms| rooms.get(record.room_id()))
                .map(Vec::as_slice)
                .unwrap_or_default();
            candidate(state, record, hot, tokens)
        })
        .collect()
}

/// The newest history match per session, keyed by `(agentId, sessionId)`.
async fn store_matches(
    store: &dyn HistoryStore,
    candidates: &[Candidate],
    query: &str,
    tokens: &[String],
) -> HashMap<(String, String), SessionMatch> {
    let mut agent_ids = candidates
        .iter()
        .map(|candidate| candidate.record.agent_id.clone())
        .collect::<Vec<_>>();
    agent_ids.sort();
    agent_ids.dedup();
    if agent_ids.is_empty() {
        return HashMap::new();
    }
    let rows = match store.search_messages(&agent_ids, query, SEARCH_ROW_LIMIT).await {
        Ok(rows) => rows,
        Err(error) => {
            warn!(error = %error, "session search could not read the history store; searching the hot tail only");
            return HashMap::new();
        }
    };
    let mut matches = HashMap::new();
    for row in rows {
        matches
            .entry((row.agent_id.clone(), row.session_id.clone()))
            .or_insert_with(|| SessionMatch {
                message_id: Some(row.message.id.clone()),
                snippet: search_snippet(&row.message.content.text, tokens),
            });
    }
    matches
}

async fn stored_preview(store: &dyn HistoryStore, record: &SessionRecord) -> Option<String> {
    let rows = store
        .page_messages(&MessagePageQuery {
            agent_id: record.agent_id.clone(),
            session_id: record.id.clone(),
            before: None,
            limit: PREVIEW_ROW_LIMIT,
            include_hidden: false,
        })
        .await
        .ok()?;
    preview_from_newest(rows.iter().map(|row| &row.message))
}

/// Adds the store's counts to the hot tail's and finds previews the hot tail lacks.
async fn complete(store: &dyn HistoryStore, candidates: Vec<Candidate>) -> Vec<SessionView> {
    let mut by_agent: HashMap<String, Vec<String>> = HashMap::new();
    for candidate in &candidates {
        by_agent
            .entry(candidate.record.agent_id.clone())
            .or_default()
            .push(candidate.record.id.clone());
    }
    let mut counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (agent_id, session_ids) in by_agent {
        match store.visible_message_counts(&agent_id, &session_ids).await {
            Ok(agent_counts) => {
                counts.insert(agent_id, agent_counts);
            }
            Err(error) => {
                warn!(agent_id = %agent_id, error = %error, "session counts could not read the history store; showing the hot tail");
            }
        }
    }
    let mut views = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let stored = counts
            .get(&candidate.record.agent_id)
            .map(|agent_counts| agent_counts.get(&candidate.record.id).copied().unwrap_or(0));
        let message_count = match stored {
            Some(stored) => stored + candidate.unmirrored_visible,
            None => candidate.hot_visible,
        };
        let mut preview = candidate.preview;
        if preview.is_none() && stored.unwrap_or(0) > 0 {
            preview = stored_preview(store, &candidate.record).await;
        }
        views.push(SessionView {
            record: candidate.record,
            message_count,
            preview,
            active_runs: candidate.active_runs,
            unread: candidate.unread,
            capabilities: candidate.capabilities,
            matched: candidate.matched,
        });
    }
    views
}

/// `GET /api/agents/{id}/sessions`; `None` when the agent does not exist.
pub(crate) async fn list_sessions(
    state: &SharedDaemonState,
    agent_id: &str,
    query: &SessionListQuery,
) -> Option<SessionPage> {
    let tokens = query.q.as_deref().map(search_tokens).unwrap_or_default();
    let (mut candidates, store) = {
        let guard = state.read().await;
        if !guard.agents.contains_key(agent_id) {
            return None;
        }
        let select = |record: &SessionRecord| {
            record.archived == query.archived && query.kind.map_or(true, |kind| record.kind == kind)
        };
        (
            collect_candidates(&guard, agent_id, query.include_helpers, select, &tokens),
            guard.history.store(),
        )
    };
    if let Some(q) = query.q.as_deref() {
        let mut stored = if tokens.is_empty() {
            HashMap::new()
        } else {
            store_matches(&*store, &candidates, q, &tokens).await
        };
        candidates.retain_mut(|candidate| {
            let key = (candidate.record.agent_id.clone(), candidate.record.id.clone());
            let found = candidate
                .matched
                .take()
                .or_else(|| stored.remove(&key))
                .or_else(|| {
                    text_matches(&candidate.record.title, &tokens).then(|| SessionMatch {
                        message_id: None,
                        snippet: candidate.record.title.clone(),
                    })
                });
            candidate.matched = found;
            candidate.matched.is_some()
        });
    }
    candidates.sort_by(|left, right| sort_key(&left.record).cmp(&sort_key(&right.record)));
    if let Some(cursor) = &query.cursor {
        let after = cursor.key();
        candidates.retain(|candidate| sort_key(&candidate.record) > after);
    }
    let has_more = candidates.len() > query.limit;
    candidates.truncate(query.limit);
    let next_cursor = if has_more {
        candidates
            .last()
            .map(|candidate| SessionCursor::of(&candidate.record).encode())
    } else {
        None
    };
    Some(SessionPage {
        sessions: complete(&*store, candidates).await,
        next_cursor,
    })
}

/// One session with its derived fields; `None` when the agent or session is missing.
pub(crate) async fn session_view(
    state: &SharedDaemonState,
    agent_id: &str,
    session_id: &str,
) -> Option<SessionView> {
    let (candidate, store) = {
        let guard = state.read().await;
        let runtime = guard.agents.get(agent_id)?;
        let record = guard.sessions.get(agent_id, session_id)?;
        let hot = runtime
            .messages()
            .iter()
            .filter(|message| message.room_id == record.room_id())
            .collect::<Vec<_>>();
        (candidate(&guard, record, &hot, &[]), guard.history.store())
    };
    complete(&*store, vec![candidate]).await.pop()
}

/// `GET …/sessions/{sid}/messages`: the history store's page merged with the
/// hot tail by id (spec §3.3).
pub(crate) async fn session_messages(
    state: &SharedDaemonState,
    agent_id: &str,
    session_id: &str,
    request: &MessagePageRequest,
) -> Result<MessagePage, MessagePageError> {
    let (hot, store) = {
        let guard = state.read().await;
        let runtime = guard
            .agents
            .get(agent_id)
            .ok_or(MessagePageError::NotFound)?;
        let record = guard
            .sessions
            .get(agent_id, session_id)
            .ok_or(MessagePageError::NotFound)?;
        let hot = runtime
            .messages()
            .iter()
            .filter(|message| message.room_id == record.room_id())
            .cloned()
            .collect::<Vec<_>>();
        (hot, guard.history.store())
    };
    let hidden_ids = hidden_message_ids(hot.iter());
    let before = match request.before.as_deref() {
        None => None,
        Some(before_id) => match hot.iter().find(|message| message.id == before_id) {
            Some(message) => Some(MessageOrder::of(message)),
            None => match store.get_message(agent_id, session_id, before_id).await {
                Ok(Some(row)) => Some(row.order()),
                Ok(None) => return Err(MessagePageError::BeforeNotFound),
                Err(error) => {
                    warn!(error = %error, "a message page could not read the history store");
                    return Err(MessagePageError::Unavailable);
                }
            },
        },
    };
    let hot_ids = hot
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    let mut page = hot
        .into_iter()
        .filter(|message| {
            before
                .as_ref()
                .map_or(true, |before| MessageOrder::of(message) < *before)
        })
        .map(|message| PageMessage {
            hidden: hidden_ids.contains(&message.id),
            message,
        })
        .filter(|entry| request.include_hidden || !entry.hidden)
        .collect::<Vec<_>>();
    let query = MessagePageQuery {
        agent_id: agent_id.to_string(),
        session_id: session_id.to_string(),
        before,
        limit: request.limit + 1,
        include_hidden: request.include_hidden,
    };
    match store.page_messages(&query).await {
        Ok(rows) => page.extend(
            rows.into_iter()
                .filter(|row| !hot_ids.contains(&row.message.id))
                .map(|row| PageMessage {
                    hidden: row.hidden,
                    message: row.message,
                }),
        ),
        Err(error) => {
            warn!(error = %error, "a message page could not read the history store; showing the hot tail");
        }
    }
    page.sort_by(|left, right| MessageOrder::of(&right.message).cmp(&MessageOrder::of(&left.message)));
    let has_more = page.len() > request.limit;
    page.truncate(request.limit);
    page.reverse();
    let next_before = if has_more {
        page.first().map(|entry| entry.message.id.clone())
    } else {
        None
    };
    Ok(MessagePage {
        messages: page,
        next_before,
    })
}
```

(`let (…) = { … }` blocks end the read guard before any history-store call; `?` inside them returns early with the guard dropped.)

- [ ] **Step 5: Add the contracts**

Create `hosts/rust-daemon/src/routes/contracts/sessions.rs`:

```rust
//! Session route bodies (spec §3.3).

use std::collections::BTreeMap;

use anima_core::{AttachmentType, MessageRole};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use super::shared::data_value_to_json;
use crate::sessions::views::{MessagePage, PageMessage, SessionPage, SessionView};
use crate::sessions::SessionCapabilities;

/// Message metadata the session routes expose: spec §3.3's list plus `kind`
/// (check-in prompts) and `source` (Telegram turns).
pub(crate) const EXPOSED_MESSAGE_METADATA: [&str; 12] = [
    "toolCalls",
    "toolCallId",
    "stepId",
    "runId",
    "stopped",
    "revised",
    "steer",
    "skill",
    "clientRequestId",
    "communication",
    "kind",
    "source",
];

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCapabilitiesResponse {
    pub(crate) send: bool,
    pub(crate) steer: bool,
    pub(crate) stop: bool,
    pub(crate) rename: bool,
    pub(crate) archive: bool,
    pub(crate) delete: bool,
    pub(crate) compact: bool,
    pub(crate) export: bool,
}

impl From<SessionCapabilities> for SessionCapabilitiesResponse {
    fn from(value: SessionCapabilities) -> Self {
        Self {
            send: value.send,
            steer: value.steer,
            stop: value.stop,
            rename: value.rename,
            archive: value.archive,
            delete: value.delete,
            compact: value.compact,
            export: value.export,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSummaryResponse {
    pub(crate) text: String,
    pub(crate) through_message_id: String,
    pub(crate) created_at_ms: u64,
    pub(crate) source_message_count: usize,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionContextTrimmedResponse {
    pub(crate) dropped_through_message_id: String,
    pub(crate) at_ms: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionMatchResponse {
    /// `null` when only the title matched.
    pub(crate) message_id: Option<String>,
    pub(crate) snippet: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    /// The transcript room: the id itself except for mapped legacy rooms.
    pub(crate) room_id: String,
    pub(crate) kind: String,
    pub(crate) origin: String,
    pub(crate) title: String,
    pub(crate) title_source: String,
    pub(crate) created_at_ms: u64,
    pub(crate) last_activity_at_ms: u64,
    pub(crate) last_read_at_ms: Option<u64>,
    pub(crate) archived: bool,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) parent_run_id: Option<String>,
    pub(crate) parent_agent_id: Option<String>,
    pub(crate) summary: Option<SessionSummaryResponse>,
    pub(crate) context_trimmed: Option<SessionContextTrimmedResponse>,
    pub(crate) message_count: usize,
    pub(crate) preview: Option<String>,
    pub(crate) active_runs: usize,
    /// Always 0 until approvals exist (M4).
    pub(crate) pending_approvals: usize,
    pub(crate) unread: bool,
    pub(crate) capabilities: SessionCapabilitiesResponse,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub(crate) matched: Option<SessionMatchResponse>,
}

impl From<&SessionView> for SessionResponse {
    fn from(view: &SessionView) -> Self {
        let record = &view.record;
        Self {
            id: record.id.clone(),
            agent_id: record.agent_id.clone(),
            room_id: record.room_id().to_string(),
            kind: record.kind.as_str().into(),
            origin: record.origin.as_str().into(),
            title: record.title.clone(),
            title_source: record.title_source.as_str().into(),
            created_at_ms: record.created_at_ms,
            last_activity_at_ms: record.last_activity_at_ms,
            last_read_at_ms: record.last_read_at_ms,
            archived: record.archived,
            parent_session_id: record.parent_session_id.clone(),
            parent_run_id: record.parent_run_id.clone(),
            parent_agent_id: record.parent_agent_id.clone(),
            summary: record.summary.as_ref().map(|summary| SessionSummaryResponse {
                text: summary.text.clone(),
                through_message_id: summary.through_message_id.clone(),
                created_at_ms: summary.created_at_ms,
                source_message_count: summary.source_message_count,
            }),
            context_trimmed: record
                .context_trimmed
                .as_ref()
                .map(|trimmed| SessionContextTrimmedResponse {
                    dropped_through_message_id: trimmed.dropped_through_message_id.clone(),
                    at_ms: trimmed.at_ms,
                }),
            message_count: view.message_count,
            preview: view.preview.clone(),
            active_runs: view.active_runs,
            pending_approvals: 0,
            unread: view.unread,
            capabilities: view.capabilities.into(),
            matched: view.matched.as_ref().map(|found| SessionMatchResponse {
                message_id: found.message_id.clone(),
                snippet: found.snippet.clone(),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionsEnvelope {
    pub(crate) sessions: Vec<SessionResponse>,
    pub(crate) next_cursor: Option<String>,
}

impl From<&SessionPage> for SessionsEnvelope {
    fn from(page: &SessionPage) -> Self {
        Self {
            sessions: page.sessions.iter().map(SessionResponse::from).collect(),
            next_cursor: page.next_cursor.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SessionEnvelope {
    pub(crate) session: SessionResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SessionAttachmentResponse {
    #[serde(rename = "type")]
    pub(crate) attachment_type: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionMessageResponse {
    pub(crate) id: String,
    pub(crate) role: String,
    pub(crate) text: String,
    /// Metadata only; attachment contents are not repeated here.
    pub(crate) attachments: Vec<SessionAttachmentResponse>,
    pub(crate) metadata: BTreeMap<String, Value>,
    pub(crate) created_at_ms: u64,
    /// Present (true) only on silent check-in messages, with `includeHidden=true`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) hidden: bool,
}

impl From<&PageMessage> for SessionMessageResponse {
    fn from(entry: &PageMessage) -> Self {
        let message = &entry.message;
        Self {
            id: message.id.clone(),
            role: match message.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
                MessageRole::Tool => "tool",
            }
            .into(),
            text: message.content.text.clone(),
            attachments: message
                .content
                .attachments
                .iter()
                .flatten()
                .map(|attachment| SessionAttachmentResponse {
                    attachment_type: match attachment.attachment_type {
                        AttachmentType::File => "file",
                        AttachmentType::Image => "image",
                        AttachmentType::Url => "url",
                    }
                    .into(),
                    name: attachment.name.clone(),
                })
                .collect(),
            metadata: message
                .content
                .metadata
                .iter()
                .flatten()
                .filter(|(key, _)| EXPOSED_MESSAGE_METADATA.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
            created_at_ms: message.created_at_ms,
            hidden: entry.hidden,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionMessagesEnvelope {
    pub(crate) messages: Vec<SessionMessageResponse>,
    pub(crate) next_before: Option<String>,
}

impl From<&MessagePage> for SessionMessagesEnvelope {
    fn from(page: &MessagePage) -> Self {
        Self {
            messages: page.messages.iter().map(SessionMessageResponse::from).collect(),
            next_before: page.next_before.clone(),
        }
    }
}
```

In `hosts/rust-daemon/src/routes/contracts/mod.rs`, add `mod sessions;` after `mod schedules;`, add `pub(crate) use sessions::*;` after `pub(crate) use schedules::*;`, and replace the `pub(crate) use agents::{…};` block with:

```rust
pub(crate) use agents::{
    AgentConfigRequest, AgentEnvelope, AgentProfileEnvelope, AgentProfileResponse,
    AgentRecentMemoriesQuery, AgentRunEnvelope, AgentRuntimeSnapshotResponse,
    AgentSummariesEnvelope, AgentSummaryResponse, AgentUpdateRequest, AgentsEnvelope,
    GenerateProfileRequest,
};
```

In `hosts/rust-daemon/src/routes/contracts/agents.rs`, add after `AgentsEnvelope`:

```rust
/// An agent without its transcript (`GET /api/agents?view=summary`).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentSummaryResponse {
    pub(crate) state: AgentStateResponse,
    pub(crate) message_count: usize,
    pub(crate) event_count: usize,
    pub(crate) last_task: Option<TaskResultResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentSummariesEnvelope {
    pub(crate) agents: Vec<AgentSummaryResponse>,
}

impl From<&AgentRuntimeSnapshot> for AgentSummaryResponse {
    fn from(value: &AgentRuntimeSnapshot) -> Self {
        Self {
            state: AgentStateResponse::from(&value.state),
            message_count: value.message_count,
            event_count: value.event_count,
            last_task: value.last_task.as_ref().map(TaskResultResponse::from),
        }
    }
}
```

In `hosts/rust-daemon/src/routes/agents.rs`, add `AgentSummariesEnvelope, AgentSummaryResponse,` to the `use super::contracts::{…};` list (after `AgentRuntimeSnapshotResponse,`) and add after `handle_list_agents`:

```rust
pub(crate) async fn handle_list_agent_summaries(state: &SharedDaemonState) -> AgentSummariesEnvelope {
    let summaries = state.read().await.agent_summaries();
    AgentSummariesEnvelope {
        agents: summaries.iter().map(AgentSummaryResponse::from).collect(),
    }
}
```

- [ ] **Step 6: Add the routes**

Create `hosts/rust-daemon/src/routes/sessions.rs`:

```rust
//! Session routes (spec §3.3). Every answer is owner-authorized and `no-store`.

use std::collections::HashMap;

use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use super::contracts::{
    ErrorBody, SessionEnvelope, SessionMessagesEnvelope, SessionResponse, SessionsEnvelope,
};
use super::http::{json_response, request_query};
use super::jobs::{authorize, no_store};
use super::{ApiError, AppState};
use crate::sessions::views::{
    self, MessagePageError, MessagePageRequest, SessionCursor, SessionListQuery,
    DEFAULT_MESSAGE_PAGE, DEFAULT_SESSION_PAGE, MAX_MESSAGE_PAGE, MAX_SEARCH_QUERY_CHARS,
    MAX_SESSION_PAGE,
};
use crate::sessions::{is_valid_session_id, SessionKind};

type QueryParams = HashMap<String, String>;

pub(super) fn rejected(error: ApiError) -> Response {
    no_store(error.into_response())
}

fn params(uri: &Uri) -> Result<QueryParams, ApiError> {
    request_query(uri).map_err(|()| ApiError::bad_request_static("malformed query"))
}

fn flag(params: &QueryParams, name: &str, default: bool) -> Result<bool, ApiError> {
    match params.get(name).map(String::as_str) {
        None | Some("") => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(_) => Err(ApiError::bad_request(format!("{name} must be true or false"))),
    }
}

fn limit(params: &QueryParams, default: usize, max: usize) -> Result<usize, ApiError> {
    match params.get("limit").map(String::as_str) {
        None | Some("") => Ok(default),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=max).contains(limit))
            .ok_or_else(|| ApiError::bad_request(format!("limit must be between 1 and {max}"))),
    }
}

fn list_query(uri: &Uri) -> Result<SessionListQuery, ApiError> {
    let params = params(uri)?;
    let kind = match params.get("kind").map(String::as_str) {
        None | Some("") => None,
        Some(value) => Some(SessionKind::parse(value).ok_or_else(|| {
            ApiError::bad_request_static("kind must be one of chat, telegram, checkin, job, helper")
        })?),
    };
    let q = params
        .get("q")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if q
        .as_ref()
        .is_some_and(|q| q.chars().count() > MAX_SEARCH_QUERY_CHARS)
    {
        return Err(ApiError::bad_request(format!(
            "q must be at most {MAX_SEARCH_QUERY_CHARS} characters"
        )));
    }
    let cursor = match params.get("cursor").map(String::as_str) {
        None | Some("") => None,
        Some(value) => Some(
            SessionCursor::decode(value)
                .ok_or_else(|| ApiError::bad_request_static("cursor is invalid"))?,
        ),
    };
    Ok(SessionListQuery {
        kind,
        archived: flag(&params, "archived", false)?,
        q,
        cursor,
        limit: limit(&params, DEFAULT_SESSION_PAGE, MAX_SESSION_PAGE)?,
        include_helpers: flag(&params, "includeHelpers", true)?,
    })
}

fn message_query(uri: &Uri) -> Result<MessagePageRequest, ApiError> {
    let params = params(uri)?;
    Ok(MessagePageRequest {
        before: params
            .get("before")
            .filter(|value| !value.is_empty())
            .cloned(),
        limit: limit(&params, DEFAULT_MESSAGE_PAGE, MAX_MESSAGE_PAGE)?,
        include_hidden: flag(&params, "includeHidden", false)?,
    })
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions", tag = "sessions",
    params(
        ("agent_id" = String, Path),
        ("kind" = Option<String>, Query, description = "chat, telegram, checkin, job, or helper"),
        ("archived" = Option<bool>, Query, description = "true lists only archived sessions (default false)"),
        ("q" = Option<String>, Query, description = "Search titles and message text (up to 200 characters)"),
        ("cursor" = Option<String>, Query, description = "nextCursor of the previous page"),
        ("limit" = Option<usize>, Query, description = "1-200, default 50"),
        ("includeHelpers" = Option<bool>, Query, description = "Include helper and delegated sessions whose parentAgentId is this agent (default true)")
    ),
    responses(
        (status = 200, description = "Sessions, newest activity first", body = SessionsEnvelope),
        (status = 400, description = "Invalid query", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody)
    ))]
pub(super) async fn list_sessions(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let query = match list_query(request.uri()) {
        Ok(query) => query,
        Err(error) => return rejected(error),
    };
    match views::list_sessions(&state.daemon, &agent_id, &query).await {
        Some(page) => no_store(json_response(StatusCode::OK, &SessionsEnvelope::from(&page))),
        None => rejected(ApiError::not_found()),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions/{session_id}", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    responses(
        (status = 200, description = "One session", body = SessionEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody)
    ))]
pub(super) async fn get_session(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    match views::session_view(&state.daemon, &agent_id, &session_id).await {
        Some(view) => no_store(json_response(
            StatusCode::OK,
            &SessionEnvelope {
                session: SessionResponse::from(&view),
            },
        )),
        None => rejected(ApiError::not_found()),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions/{session_id}/messages", tag = "sessions",
    params(
        ("agent_id" = String, Path),
        ("session_id" = String, Path, description = "Percent-encoded session id"),
        ("before" = Option<String>, Query, description = "Only messages older than this message id"),
        ("limit" = Option<usize>, Query, description = "1-200, default 50"),
        ("includeHidden" = Option<bool>, Query, description = "Include silent check-in turns (default false)")
    ),
    responses(
        (status = 200, description = "Messages oldest to newest within the page", body = SessionMessagesEnvelope),
        (status = 400, description = "Invalid query or unknown before message", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 503, description = "The page needs the history store and it cannot be read", body = ErrorBody)
    ))]
pub(super) async fn list_session_messages(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    let query = match message_query(request.uri()) {
        Ok(query) => query,
        Err(error) => return rejected(error),
    };
    match views::session_messages(&state.daemon, &agent_id, &session_id, &query).await {
        Ok(page) => no_store(json_response(
            StatusCode::OK,
            &SessionMessagesEnvelope::from(&page),
        )),
        Err(MessagePageError::NotFound) => rejected(ApiError::not_found()),
        Err(MessagePageError::BeforeNotFound) => {
            rejected(ApiError::bad_request_static("before message was not found"))
        }
        Err(MessagePageError::Unavailable) => {
            rejected(ApiError::service_unavailable("history store is unavailable"))
        }
    }
}
```

In `hosts/rust-daemon/src/routes/mod.rs` (hand-format; do not run rustfmt on this file):

- add `mod sessions;` after `mod schedules;` at the top;
- in `#[openapi(paths(…))]`, add `sessions::list_sessions, sessions::get_session, sessions::list_session_messages,` after `schedules::import_legacy_schedules,`; after the closing `),` of `paths(…)` add the line `    components(schemas(self::contracts::AgentSummariesEnvelope)),`; and add `(name = "sessions", description = "Agent sessions and their transcripts"),` to `tags(…)` after the `schedules` tag;
- in `timed_routes`, after the `.route("/api/agents/{agent_id}/jobs/{job_id}/review", …)` line, add:

```rust
        .route("/api/agents/{agent_id}/sessions", get(sessions::list_sessions))
        .route("/api/agents/{agent_id}/sessions/{session_id}", get(sessions::get_session))
        .route("/api/agents/{agent_id}/sessions/{session_id}/messages", get(sessions::list_session_messages))
```

- replace the `list_agents_entry` attribute and function with:

```rust
#[utoipa::path(
    get,
    path = "/api/agents",
    tag = "agents",
    params(("view" = Option<String>, Query, description = "summary returns AgentSummariesEnvelope: the agents without their messages")),
    responses(
        (status = 200, description = "List agents (AgentSummariesEnvelope with view=summary)", body = AgentsEnvelope),
        (status = 400, description = "Unknown view", body = ErrorBody)
    )
)]
async fn list_agents_entry(State(state): State<AppState>, uri: Uri) -> AxumResponse {
    let view = match request_query(&uri) {
        Ok(query) => query.get("view").filter(|view| !view.is_empty()).cloned(),
        Err(()) => return ApiError::bad_request_static("malformed query").into_response(),
    };
    match view.as_deref() {
        None => match agents::handle_list_agents(&state.daemon).await {
            Ok(response) => json_response(StatusCode::OK, &response),
            Err(error) => error.into_response(),
        },
        Some("summary") => json_response(
            StatusCode::OK,
            &agents::handle_list_agent_summaries(&state.daemon).await,
        ),
        Some(_) => ApiError::bad_request_static("view must be summary").into_response(),
    }
}
```

In `hosts/rust-daemon/README.md`, change the `GET /api/agents` row's description to `List all registered agent snapshots. \`?view=summary\` returns them without \`messages\`.`

- [ ] **Step 7: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions:: runs::ledger routes::tests history::`
Expected: PASS — the 5 view tests, the ledger test, the 4 route tests, and every existing test in those modules (`routes::tests` includes the unchanged `GET /api/agents` assertions).

- [ ] **Step 8: Commit**

```bash
git add hosts/rust-daemon/src/sessions/views.rs hosts/rust-daemon/src/sessions/test_support.rs hosts/rust-daemon/src/sessions/mod.rs hosts/rust-daemon/src/runs/ledger.rs hosts/rust-daemon/src/state/session_state.rs hosts/rust-daemon/src/routes/contracts/sessions.rs hosts/rust-daemon/src/routes/contracts/mod.rs hosts/rust-daemon/src/routes/contracts/agents.rs hosts/rust-daemon/src/routes/agents.rs hosts/rust-daemon/src/routes/sessions.rs hosts/rust-daemon/src/routes/tests/sessions.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): add owner-only session list, detail, and message routes plus agent summaries"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. `GET /api/agents` ignores unknown `view` values (default full response) instead of returning 400, preserving existing route behavior (spec §13.4); only `view=summary` changes the shape. Test. (Supersedes the earlier acceptance of 400.)
2. Session search ranks sessions by their newest matching message per session (a per-session grouping in SQLite and Postgres, and the equivalent in memory), so a session whose only matches are older than the newest 500 matching rows still appears. Test with more than 500 matches in one session and one older match in another.

---

### Task 12: Session mutation routes and Markdown export

**Files:**

- Modify: `hosts/rust-daemon/src/sessions/views.rs` (`full_transcript`, `transcript_markdown`, `export_file_stem`, `automation_exists`; one test)
- Modify: `hosts/rust-daemon/src/runs/ledger.rs` (`remove_terminal_for_session`; one test)
- Modify: `hosts/rust-daemon/src/agent_runs.rs` (`RoomReservation`, `try_reserve_room`)
- Modify: `hosts/rust-daemon/src/state.rs` (`session_limiter` field)
- Modify: `hosts/rust-daemon/src/routes/contracts/sessions.rs` (request bodies), `hosts/rust-daemon/src/routes/sessions.rs` (four handlers), `hosts/rust-daemon/src/routes/mod.rs` (routes, `ApiDoc`), `hosts/rust-daemon/src/routes/tests/sessions.rs` (tests)
- Modify: `hosts/rust-daemon/README.md` (Sessions route table)

**Interfaces:**

- Consumes: Task 1's `AgentRuntime::retain_messages`; Task 2's `SessionRecord::new`, `SessionCreateLimiter::try_acquire`, `clean_owner_title`, `new_chat_session_id`, `DEFAULT_CHAT_TITLE`, `TitleSource`, `SessionOrigin`; Task 6's `HistoryService::{enqueue_session_deletion, forget_mirrored, flush_once, store}`; Task 11's views, contracts, `routes::sessions::rejected`, `session_view`, `MessagePageError`, `test_support`, the route-test helpers `get`, `json`, `app_with_session`, `OWNER_ORIGIN`; `DaemonState::restore_removed_agent` (existing).
- Produces:
  - `crate::sessions::views::{full_transcript(&SharedDaemonState, agent_id, session_id) -> Result<(SessionRecord, String /* agent name */, Vec<Message>), MessagePageError>, transcript_markdown(&SessionRecord, agent_name, &[Message]) -> String, export_file_stem(title) -> String, automation_exists(&DaemonState, &SessionRecord) -> bool}`;
  - `RunLedger::remove_terminal_for_session(agent_id, session_id) -> Vec<RunRecord>`;
  - `crate::agent_runs::RoomReservation` and `AgentRunCoordinator::try_reserve_room(agent_id, room_id) -> Option<RoomReservation>` (holds the room's run lock; `None` while a run holds or waits for it);
  - `DaemonState::session_limiter: SessionCreateLimiter` (not persisted);
  - contracts `SessionCreateRequest { title? }`, `SessionUpdateRequest { title?, archived?, lastReadAtMs? }`;
  - routes: `POST /api/agents/{agent_id}/sessions` (`create_session`, 201 `{ session }`), `PATCH /api/agents/{agent_id}/sessions/{session_id}` (`update_session`, 200 `{ session }`), `DELETE /api/agents/{agent_id}/sessions/{session_id}` (`delete_session`, 200 `{ deleted: true }`), `GET /api/agents/{agent_id}/sessions/{session_id}/export` (`export_session`, `text/markdown; charset=utf-8`, `Content-Disposition: attachment; filename="<stem>.md"`). Mutations call `authorize`; export calls `authorize_read`; every answer is `no-store`.
- Error strings: `title must be 1 to 120 characters` (400), `Too many new chats; try again in a minute` (429), `at least one of title, archived, or lastReadAtMs is required` (400), `lastReadAtMs must not be in the future` (400), `This session cannot be renamed` (409), `This session cannot be deleted` (409), `A run in this session is still in progress` (409), `history store is unavailable` (503, export), `not found` (404).
- Behavior: create makes a `chat:<uuid>` chat titled "New chat" (`first_message`, retitled by its first message at commit) or the owner's title (`owner`), inside one control-plane transaction; a failed save removes it. PATCH applies every given field, sets `titleSource: owner` with a title, and restores the previous record if the save fails. DELETE follows the capability table, reserves the room so no run can start in it, refuses while the ledger has an active run in the session, removes the record, the room's hot messages, and the session's terminal ledger runs in one save, and only after that save succeeded queues the history deletion (messages, runs, attachment records) and forgets the mirrored ids; a failed save restores the agent, record, and runs. Export renders the record header and every visible message (hot and history store, silent check-ins excluded) oldest first.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `hosts/rust-daemon/src/sessions/views.rs`:

```rust
    #[tokio::test]
    async fn the_markdown_export_skips_silent_checkins_and_labels_speakers() {
        let mut daemon = DaemonState::new();
        let agent = daemon.create_agent(agent_config("companion")).unwrap().state.id;
        daemon.sessions.insert(session(
            &agent,
            "schedule:s1",
            SessionKind::Checkin,
            "Check-in · Check status",
            1,
        ));
        seed_messages(
            &mut daemon,
            &agent,
            vec![
                checkin_prompt(&agent, "c1", "schedule:s1", "s1", "Check status", 60_000),
                message(&agent, "c2", "schedule:s1", MessageRole::Assistant, "CHECKIN_OK", 60_001),
                checkin_prompt(&agent, "c3", "schedule:s1", "s1", "Check status", 120_000),
                message(&agent, "c4", "schedule:s1", MessageRole::Assistant, "Two tasks are overdue", 120_001),
            ],
        );
        let state = Arc::new(RwLock::new(daemon));

        let (record, agent_name, messages) =
            full_transcript(&state, &agent, "schedule:s1").await.unwrap();

        assert_eq!(
            messages.iter().map(|message| message.id.as_str()).collect::<Vec<_>>(),
            ["c3", "c4"]
        );
        assert_eq!(
            transcript_markdown(&record, &agent_name, &messages),
            "# Check-in · Check status\n\n- Agent: companion\n- Session: `schedule:s1` (checkin)\n- Messages: 2\n\n---\n\n**Check-in** · 1970-01-01 00:02 UTC\n\nCheck status\n\n---\n\n**companion** · 1970-01-01 00:02 UTC\n\nTwo tasks are overdue\n"
        );
        assert_eq!(export_file_stem("Check-in · Check status"), "check-in-check-status");
        assert_eq!(export_file_stem("···"), "session");
        assert_eq!(
            full_transcript(&state, &agent, "chat:missing").await.unwrap_err(),
            MessagePageError::NotFound
        );
    }
```

Append to `mod tests` in `hosts/rust-daemon/src/runs/ledger.rs`:

```rust
    #[test]
    fn a_deleted_session_takes_only_its_terminal_runs() {
        let mut ledger = RunLedger::default();
        let running = record("agent-a", 1);
        let running_id = running.id.clone();
        let done = finished("agent-a", 2);
        let done_id = done.id.clone();
        let mut elsewhere = finished("agent-a", 3);
        elsewhere.session_id = "chat:other".into();
        for run in [running, done, elsewhere] {
            ledger.insert(run);
        }

        let removed = ledger.remove_terminal_for_session("agent-a", "direct:test");

        assert_eq!(removed.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(), [done_id.as_str()]);
        assert!(ledger.get(&running_id).is_some());
        assert_eq!(ledger.for_agent("agent-a").len(), 2);
    }
```

Append to `hosts/rust-daemon/src/routes/tests/sessions.rs`:

```rust
fn send(method: &str, uri: &str, origin: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn session_mutations_and_export_require_the_owner() {
    let (app, _, agent) = app_with_session().await;
    let session = format!("/api/agents/{agent}/sessions/chat%3Aplans");
    for request in [
        send("POST", &format!("/api/agents/{agent}/sessions"), "https://untrusted.example", serde_json::json!({})),
        send("PATCH", &session, "https://untrusted.example", serde_json::json!({"archived": true})),
        send("DELETE", &session, "https://untrusted.example", serde_json::Value::Null),
        get(&format!("{session}/export"), "https://untrusted.example"),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn creating_a_chat_saves_it_returns_201_and_is_rate_limited() {
    use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};

    let (app, state, agent) = app_with_session().await;
    let path = std::env::temp_dir().join(format!(
        "anima-session-create-{}.json",
        uuid::Uuid::new_v4()
    ));
    let store = ControlPlaneStoreConfig::Json(path.clone());
    state.write().await.set_control_plane_store(Some(store.clone()));
    let create = format!("/api/agents/{agent}/sessions");

    let empty = Request::builder()
        .method("POST")
        .uri(&create)
        .header("host", "127.0.0.1:8080")
        .header("origin", OWNER_ORIGIN)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(empty).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let created = json(response).await;
    let id = created["session"]["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("chat:"), "{id}");
    assert_eq!(created["session"]["title"], "New chat");
    assert_eq!(created["session"]["titleSource"], "first_message");
    assert_eq!(created["session"]["kind"], "chat");
    let saved = load_control_plane_snapshot(&store).await.unwrap().unwrap();
    assert!(saved.sessions.iter().any(|session| session.id == id), "the new chat is durable");

    let titled = json(
        app.clone()
            .oneshot(send("POST", &create, OWNER_ORIGIN, serde_json::json!({"title": "  Trip   plans "})))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(titled["session"]["title"], "Trip plans");
    assert_eq!(titled["session"]["titleSource"], "owner");

    for (uri, body, status, error) in [
        (create.clone(), serde_json::json!({"title": ""}), StatusCode::BAD_REQUEST, "title must be 1 to 120 characters"),
        ("/api/agents/missing/sessions".to_string(), serde_json::json!({}), StatusCode::NOT_FOUND, "not found"),
    ] {
        let response = app.clone().oneshot(send("POST", &uri, OWNER_ORIGIN, body)).await.unwrap();
        assert_eq!(response.status(), status, "{uri}");
        assert_eq!(json(response).await["error"], error);
    }

    {
        let mut guard = state.write().await;
        let now = anima_core::primitives::now_millis();
        while guard.session_limiter.try_acquire(&agent, now) {}
    }
    let response = app
        .clone()
        .oneshot(send("POST", &create, OWNER_ORIGIN, serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(json(response).await["error"], "Too many new chats; try again in a minute");
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn patching_renames_archives_and_marks_a_session_read() {
    let (app, state, agent) = app_with_session().await;
    state.write().await.sessions.insert(SessionRecord::new(
        &agent,
        "job:job-1",
        SessionKind::Job,
        SessionOrigin::Job,
        "Job · Weekly report".into(),
        TitleSource::System,
        1,
    ));
    let chat = format!("/api/agents/{agent}/sessions/chat%3Aplans");
    let job = format!("/api/agents/{agent}/sessions/job%3Ajob-1");

    let renamed = json(
        app.clone()
            .oneshot(send("PATCH", &chat, OWNER_ORIGIN, serde_json::json!({"title": "Offsite", "lastReadAtMs": 2})))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(renamed["session"]["title"], "Offsite");
    assert_eq!(renamed["session"]["titleSource"], "owner");
    assert_eq!(renamed["session"]["lastReadAtMs"], 2);
    assert_eq!(renamed["session"]["unread"], false);
    let archived = json(
        app.clone()
            .oneshot(send("PATCH", &job, OWNER_ORIGIN, serde_json::json!({"archived": true})))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(archived["session"]["archived"], true);
    assert!(state.read().await.sessions.get(&agent, "job:job-1").unwrap().archived);

    for (uri, body, status, error) in [
        (chat.clone(), serde_json::json!({}), StatusCode::BAD_REQUEST, "at least one of title, archived, or lastReadAtMs is required"),
        (chat.clone(), serde_json::json!({"lastReadAtMs": u64::MAX}), StatusCode::BAD_REQUEST, "lastReadAtMs must not be in the future"),
        (chat.clone(), serde_json::json!({"title": " "}), StatusCode::BAD_REQUEST, "title must be 1 to 120 characters"),
        (job.clone(), serde_json::json!({"title": "Renamed"}), StatusCode::CONFLICT, "This session cannot be renamed"),
        (format!("/api/agents/{agent}/sessions/chat%3Amissing"), serde_json::json!({"archived": true}), StatusCode::NOT_FOUND, "not found"),
    ] {
        let response = app.clone().oneshot(send("PATCH", &uri, OWNER_ORIGIN, body)).await.unwrap();
        assert_eq!(response.status(), status, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json(response).await["error"], error, "{uri}");
    }
}

#[tokio::test]
async fn deleting_a_chat_removes_its_record_messages_and_history_rows() {
    use crate::history::HistoryStore as _;

    let (app, state, agent) = app_with_session().await;
    seed_messages(
        &mut *state.write().await,
        &agent,
        vec![message(&agent, "k1", "chat:keep", MessageRole::User, "keep me", 3)],
    );
    let history = state.read().await.history.clone();
    history.flush_once(&state, 10).await.unwrap();
    let session = format!("/api/agents/{agent}/sessions/chat%3Aplans");

    let response = app
        .clone()
        .oneshot(send("DELETE", &session, OWNER_ORIGIN, serde_json::Value::Null))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(json(response).await["deleted"], true);
    {
        let guard = state.read().await;
        assert!(guard.sessions.get(&agent, "chat:plans").is_none());
        let rooms = guard
            .get_agent(&agent)
            .unwrap()
            .messages
            .into_iter()
            .map(|message| message.room_id)
            .collect::<Vec<_>>();
        assert_eq!(rooms, ["chat:keep"], "other rooms keep their messages");
    }
    history.flush_once(&state, 11).await.unwrap();
    let rows = history
        .store()
        .page_messages(&crate::history::MessagePageQuery {
            agent_id: agent.clone(),
            session_id: "chat:plans".into(),
            before: None,
            limit: 10,
            include_hidden: true,
        })
        .await
        .unwrap();
    assert!(rows.is_empty(), "the history rows are deleted too");
    let response = app.clone().oneshot(get(&session, OWNER_ORIGIN)).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_is_refused_by_kind_and_while_a_run_is_active() {
    let (app, state, agent) = app_with_session().await;
    state.write().await.sessions.insert(SessionRecord::new(
        &agent,
        "telegram:bot",
        SessionKind::Telegram,
        SessionOrigin::Telegram,
        "Telegram · @bot".into(),
        TitleSource::System,
        1,
    ));
    state.write().await.runs.insert(crate::runs::RunRecord::running(
        crate::runs::RunStart {
            agent_id: agent.clone(),
            session_id: "chat:plans".into(),
            source: crate::runs::RunSource::Web,
            source_ref: None,
            idempotency_key: None,
            text: "still working".into(),
            model: "gpt-5.4".into(),
            provider: None,
            parent_run_id: None,
        },
        1,
    ));

    for (uri, error) in [
        (format!("/api/agents/{agent}/sessions/telegram%3Abot"), "This session cannot be deleted"),
        (format!("/api/agents/{agent}/sessions/chat%3Aplans"), "A run in this session is still in progress"),
    ] {
        let response = app
            .clone()
            .oneshot(send("DELETE", &uri, OWNER_ORIGIN, serde_json::Value::Null))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json(response).await["error"], error);
    }
    assert!(state.read().await.sessions.get(&agent, "chat:plans").is_some());
}

#[tokio::test]
async fn exporting_a_session_returns_its_full_markdown_transcript() {
    use crate::history::HistoryStore as _;

    let (app, state, agent) = app_with_session().await;
    state
        .read()
        .await
        .history
        .store()
        .upsert_messages(&[crate::history::conformance::history_message(
            "m0",
            &agent,
            "chat:plans",
            MessageRole::User,
            "An archived question",
            0,
        )])
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(get(&format!("/api/agents/{agent}/sessions/chat%3Aplans/export"), OWNER_ORIGIN))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/markdown; charset=utf-8");
    assert_eq!(response.headers()["content-disposition"], "attachment; filename=\"plans.md\"");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(body.starts_with("# Plans\n"), "{body}");
    let archived = body.find("An archived question").unwrap();
    let question = body.find("Plan the offsite").unwrap();
    let answer = body.find("Here is the plan").unwrap();
    assert!(archived < question && question < answer, "{body}");
    assert!(body.contains("**You** · 1970-01-01 00:00 UTC"), "{body}");
    assert!(body.contains("**companion** · 1970-01-01 00:00 UTC"), "{body}");
    let missing = app
        .clone()
        .oneshot(get(&format!("/api/agents/{agent}/sessions/chat%3Amissing/export"), OWNER_ORIGIN))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[test]
fn the_openapi_document_lists_the_session_mutation_routes() {
    use utoipa::OpenApi;

    let paths = crate::routes::ApiDoc::openapi().paths.paths;
    assert!(paths["/api/agents/{agent_id}/sessions"].post.is_some());
    let session = &paths["/api/agents/{agent_id}/sessions/{session_id}"];
    assert!(session.patch.is_some() && session.delete.is_some());
    assert!(paths["/api/agents/{agent_id}/sessions/{session_id}/export"].get.is_some());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions::views runs::ledger::tests::a_deleted_session routes::tests::sessions`
Expected: compile errors such as `cannot find function full_transcript in this scope`, `no method named remove_terminal_for_session found for struct RunLedger`, and `no field session_limiter on type DaemonState`.

- [ ] **Step 3: Add the ledger, coordinator, and state pieces**

In `hosts/rust-daemon/src/runs/ledger.rs`, add to `impl RunLedger` after `active_count_for_session`:

```rust
    /// Removes a deleted session's terminal runs and returns them, so a failed
    /// save can put them back.
    pub(crate) fn remove_terminal_for_session(
        &mut self,
        agent_id: &str,
        session_id: &str,
    ) -> Vec<RunRecord> {
        let ids = self
            .records
            .values()
            .filter(|record| {
                record.agent_id == agent_id
                    && record.session_id == session_id
                    && record.status.is_terminal()
            })
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| self.records.remove(&id))
            .collect()
    }
```

In `hosts/rust-daemon/src/agent_runs.rs`, add after the `RunTicket` impl block:

```rust
/// A room held by a non-run operation (session deletion); runs for the room
/// wait until it is dropped.
pub(crate) struct RoomReservation {
    _lease: SessionLease,
}
```

and add to `impl AgentRunCoordinator`, after `is_agent_busy`:

```rust
    /// Takes the room's run lock without waiting. `None` while a run holds or
    /// waits for it (spec §3.3: deleting a session with a run is 409).
    pub(crate) fn try_reserve_room(&self, agent_id: &str, room_id: &str) -> Option<RoomReservation> {
        let in_use = self
            .session_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&(agent_id.to_string(), room_id.to_string()));
        if in_use {
            return None;
        }
        self.try_session_lease(agent_id, room_id)
            .map(|lease| RoomReservation { _lease: lease })
    }
```

In `hosts/rust-daemon/src/state.rs` (hand-format), add `pub(crate) session_limiter: crate::sessions::SessionCreateLimiter,` to `DaemonState` after `pub(crate) history: crate::history::SharedHistory,`, and `session_limiter: crate::sessions::SessionCreateLimiter::default(),` after `history: crate::history::HistoryService::ephemeral(),` in `with_model_adapter_and_events_and_limits`.

- [ ] **Step 4: Add the export functions**

In `hosts/rust-daemon/src/sessions/views.rs`, change `use crate::history::{…};` to

```rust
use crate::history::{
    search_snippet, search_tokens, text_matches, HistoryMessage, HistoryStore, MessageOrder,
    MessagePageQuery,
};
```

add `const EXPORT_PAGE_ROWS: usize = 500;` after `PREVIEW_ROW_LIMIT`, and append above the test module:

```rust
/// Whether a check-in session's automation still exists; its session can be
/// deleted only once it is gone (spec §3.2).
pub(crate) fn automation_exists(state: &DaemonState, record: &SessionRecord) -> bool {
    record.kind == SessionKind::Checkin
        && schedule_id_of_room(record.room_id()).is_some_and(|id| state.schedules.contains_key(id))
}

/// Every visible message of a session oldest first, including messages only
/// the history store still holds (spec §3.3 export).
pub(crate) async fn full_transcript(
    state: &SharedDaemonState,
    agent_id: &str,
    session_id: &str,
) -> Result<(SessionRecord, String, Vec<Message>), MessagePageError> {
    let (record, agent_name, hot, store) = {
        let guard = state.read().await;
        let runtime = guard
            .agents
            .get(agent_id)
            .ok_or(MessagePageError::NotFound)?;
        let record = guard
            .sessions
            .get(agent_id, session_id)
            .ok_or(MessagePageError::NotFound)?
            .clone();
        let hot = runtime
            .messages()
            .iter()
            .filter(|message| message.room_id == record.room_id())
            .cloned()
            .collect::<Vec<_>>();
        (record, runtime.config().name.clone(), hot, guard.history.store())
    };
    let hidden_ids = hidden_message_ids(hot.iter());
    let hot_ids = hot
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    let mut messages = hot
        .into_iter()
        .filter(|message| !hidden_ids.contains(&message.id))
        .collect::<Vec<_>>();
    let mut before = None;
    loop {
        let query = MessagePageQuery {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            before: before.clone(),
            limit: EXPORT_PAGE_ROWS,
            include_hidden: false,
        };
        let rows = store.page_messages(&query).await.map_err(|error| {
            warn!(error = %error, "a session export could not read the history store");
            MessagePageError::Unavailable
        })?;
        let full_page = rows.len() == EXPORT_PAGE_ROWS;
        before = rows.last().map(HistoryMessage::order);
        messages.extend(
            rows.into_iter()
                .filter(|row| !hot_ids.contains(&row.message.id))
                .map(|row| row.message),
        );
        if !full_page {
            break;
        }
    }
    messages.sort_by_key(MessageOrder::of);
    Ok((record, agent_name, messages))
}

fn format_time(created_at_ms: u64) -> String {
    i64::try_from(created_at_ms)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|at| at.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

/// The Markdown export of a session.
pub(crate) fn transcript_markdown(
    record: &SessionRecord,
    agent_name: &str,
    messages: &[Message],
) -> String {
    let mut markdown = format!("# {}\n\n", record.title);
    markdown.push_str(&format!(
        "- Agent: {agent_name}\n- Session: `{}` ({})\n- Messages: {}\n",
        record.id,
        record.kind.as_str(),
        messages.len()
    ));
    for message in messages {
        let speaker = match message.role {
            MessageRole::User if is_checkin_message(message) => "Check-in",
            MessageRole::User if is_inbound_message(message) => "Telegram",
            MessageRole::User if record.kind == SessionKind::Helper => "Request",
            MessageRole::User => "You",
            MessageRole::Assistant => agent_name,
            MessageRole::Tool => "Tool",
            MessageRole::System => "System",
        };
        markdown.push_str(&format!(
            "\n---\n\n**{speaker}** · {}\n\n{}\n",
            format_time(message.created_at_ms),
            display_text(message).trim_end()
        ));
    }
    markdown
}

/// A download file name from a title: lowercase letters, digits, and dashes.
pub(crate) fn export_file_stem(title: &str) -> String {
    let mut stem = String::new();
    for character in title.chars() {
        if stem.len() >= 60 {
            break;
        }
        if character.is_ascii_alphanumeric() {
            stem.push(character.to_ascii_lowercase());
        } else if !stem.is_empty() && !stem.ends_with('-') {
            stem.push('-');
        }
    }
    let stem = stem.trim_end_matches('-');
    if stem.is_empty() {
        "session".to_string()
    } else {
        stem.to_string()
    }
}
```

- [ ] **Step 5: Add the request bodies and handlers**

In `hosts/rust-daemon/src/routes/contracts/sessions.rs`, change `use serde::Serialize;` to `use serde::{Deserialize, Serialize};` and append:

```rust
/// `POST /api/agents/{id}/sessions`; an empty body means `{}`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionCreateRequest {
    #[serde(default)]
    pub(crate) title: Option<String>,
}

/// `PATCH /api/agents/{id}/sessions/{sid}`; at least one field.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionUpdateRequest {
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) archived: Option<bool>,
    #[serde(default)]
    pub(crate) last_read_at_ms: Option<u64>,
}
```

In `hosts/rust-daemon/src/routes/sessions.rs`, replace the `use` block at the top (everything from `use std::collections::HashMap;` through `use crate::sessions::{is_valid_session_id, SessionKind};`) with:

```rust
use std::collections::HashMap;

use anima_core::primitives::now_millis;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tracing::warn;

use super::contracts::{
    DeleteResponse, ErrorBody, SessionCreateRequest, SessionEnvelope, SessionMessagesEnvelope,
    SessionResponse, SessionUpdateRequest, SessionsEnvelope,
};
use super::http::{json_response, read_limited_body, request_query};
use super::jobs::{authorize, body, no_store};
use super::{parse_json_body, ApiError, AppState};
use crate::sessions::views::{
    self, MessagePageError, MessagePageRequest, SessionCursor, SessionListQuery,
    DEFAULT_MESSAGE_PAGE, DEFAULT_SESSION_PAGE, MAX_MESSAGE_PAGE, MAX_SEARCH_QUERY_CHARS,
    MAX_SESSION_PAGE,
};
use crate::sessions::{
    clean_owner_title, is_valid_session_id, new_chat_session_id, SessionKind, SessionOrigin,
    SessionRecord, TitleSource, DEFAULT_CHAT_TITLE,
};

const TOO_MANY_NEW_CHATS: &str = "Too many new chats; try again in a minute";
const SESSION_RUN_IN_PROGRESS: &str = "A run in this session is still in progress";
```

and append at the end of the file:

```rust
async fn session_response(
    state: &AppState,
    agent_id: &str,
    session_id: &str,
    status: StatusCode,
) -> Response {
    match views::session_view(&state.daemon, agent_id, session_id).await {
        Some(view) => no_store(json_response(
            status,
            &SessionEnvelope {
                session: SessionResponse::from(&view),
            },
        )),
        None => rejected(ApiError::not_found()),
    }
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/sessions", tag = "sessions",
    params(("agent_id" = String, Path)), request_body = SessionCreateRequest,
    responses(
        (status = 201, description = "A new chat session", body = SessionEnvelope),
        (status = 400, description = "Invalid title", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody),
        (status = 429, description = "More than 60 new sessions this minute", body = ErrorBody),
        (status = 503, description = "The control plane could not be saved", body = ErrorBody)
    ))]
pub(super) async fn create_session(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let bytes = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(bytes) => bytes,
        Err(response) => return no_store(response),
    };
    let input = if bytes.iter().all(u8::is_ascii_whitespace) {
        SessionCreateRequest::default()
    } else {
        match parse_json_body::<SessionCreateRequest>(bytes) {
            Ok(input) => input,
            Err(error) => return rejected(error),
        }
    };
    let owner_title = match input.title.as_deref().map(clean_owner_title).transpose() {
        Ok(title) => title,
        Err(message) => return rejected(ApiError::bad_request_static(message)),
    };
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (session_id, persist) = {
        let mut guard = state.daemon.write().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        let now_ms = now_millis();
        if !guard.session_limiter.try_acquire(&agent_id, now_ms) {
            return rejected(ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                message: TOO_MANY_NEW_CHATS.into(),
            });
        }
        let (title, title_source) = match owner_title {
            Some(title) => (title, TitleSource::Owner),
            None => (DEFAULT_CHAT_TITLE.to_string(), TitleSource::FirstMessage),
        };
        let record = SessionRecord::new(
            &agent_id,
            &new_chat_session_id(),
            SessionKind::Chat,
            SessionOrigin::Web,
            title,
            title_source,
            now_ms,
        );
        let session_id = record.id.clone();
        guard.sessions.insert(record);
        (session_id, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state
            .daemon
            .write()
            .await
            .sessions
            .remove(&agent_id, &session_id);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    session_response(&state, &agent_id, &session_id, StatusCode::CREATED).await
}

#[utoipa::path(patch, path = "/api/agents/{agent_id}/sessions/{session_id}", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    request_body = SessionUpdateRequest,
    responses(
        (status = 200, description = "The updated session", body = SessionEnvelope),
        (status = 400, description = "Invalid or empty update", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 409, description = "This session cannot be renamed", body = ErrorBody),
        (status = 503, description = "The control plane could not be saved", body = ErrorBody)
    ))]
pub(super) async fn update_session(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    let input: SessionUpdateRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    if input.title.is_none() && input.archived.is_none() && input.last_read_at_ms.is_none() {
        return rejected(ApiError::bad_request_static(
            "at least one of title, archived, or lastReadAtMs is required",
        ));
    }
    let title = match input.title.as_deref().map(clean_owner_title).transpose() {
        Ok(title) => title,
        Err(message) => return rejected(ApiError::bad_request_static(message)),
    };
    if input
        .last_read_at_ms
        .is_some_and(|read_at| read_at > now_millis())
    {
        return rejected(ApiError::bad_request_static(
            "lastReadAtMs must not be in the future",
        ));
    }
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (previous, persist) = {
        let mut guard = state.daemon.write().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        let schedule_exists = guard
            .sessions
            .get(&agent_id, &session_id)
            .is_some_and(|record| views::automation_exists(&guard, record));
        let Some(record) = guard.sessions.get_mut(&agent_id, &session_id) else {
            return rejected(ApiError::not_found());
        };
        if title.is_some() && !record.capabilities(schedule_exists).rename {
            return rejected(ApiError::conflict("This session cannot be renamed"));
        }
        let previous = record.clone();
        if let Some(title) = title {
            record.title = title;
            record.title_source = TitleSource::Owner;
        }
        if let Some(archived) = input.archived {
            record.archived = archived;
        }
        if let Some(read_at) = input.last_read_at_ms {
            record.last_read_at_ms = Some(read_at);
        }
        (previous, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state.daemon.write().await.sessions.insert(previous);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    session_response(&state, &agent_id, &session_id, StatusCode::OK).await
}

#[utoipa::path(delete, path = "/api/agents/{agent_id}/sessions/{session_id}", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    responses(
        (status = 200, description = "Deleted: the record, hot and mirrored messages, and attachment records; memories are kept", body = DeleteResponse),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 409, description = "The kind does not allow deletion, or a run in the session is active", body = ErrorBody),
        (status = 503, description = "The control plane could not be saved", body = ErrorBody)
    ))]
pub(super) async fn delete_session(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    let room_id = {
        let guard = state.daemon.read().await;
        let record = match guard.sessions.get(&agent_id, &session_id) {
            Some(record) if guard.agents.contains_key(&agent_id) => record,
            _ => return rejected(ApiError::not_found()),
        };
        if !record
            .capabilities(views::automation_exists(&guard, record))
            .delete
        {
            return rejected(ApiError::conflict("This session cannot be deleted"));
        }
        record.room_id().to_string()
    };
    // Runs for this room wait behind the reservation, so none starts meanwhile.
    let Some(reservation) = state.agent_runs.try_reserve_room(&agent_id, &room_id) else {
        return rejected(ApiError::conflict(SESSION_RUN_IN_PROGRESS));
    };
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (previous_agent, record, removed_runs, removed_ids, persist) = {
        let mut guard = state.daemon.write().await;
        if guard.runs.active_count_for_session(&agent_id, &session_id) > 0 {
            return rejected(ApiError::conflict(SESSION_RUN_IN_PROGRESS));
        }
        let Some(runtime) = guard.agents.get_mut(&agent_id) else {
            return rejected(ApiError::not_found());
        };
        let previous_agent = runtime.snapshot();
        let removed_ids = runtime
            .retain_messages(|message| message.room_id != room_id)
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        let Some(record) = guard.sessions.remove(&agent_id, &session_id) else {
            if let Err(error) = guard.restore_removed_agent(previous_agent) {
                warn!(agent_id = %agent_id, error = %error, "could not restore an agent after a session vanished");
            }
            return rejected(ApiError::not_found());
        };
        let removed_runs = guard
            .runs
            .remove_terminal_for_session(&agent_id, &session_id);
        (
            previous_agent,
            record,
            removed_runs,
            removed_ids,
            guard.control_plane_persist_request(),
        )
    };
    if let Err(error) = persist.save().await {
        let mut guard = state.daemon.write().await;
        if let Err(restore_error) = guard.restore_removed_agent(previous_agent) {
            warn!(agent_id = %agent_id, error = %restore_error, "could not restore an agent after a failed session delete");
        }
        guard.sessions.insert(record);
        for run in removed_runs {
            guard.runs.insert(run);
        }
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    // Durable now: the history rows may go (spec §3.3).
    let history = state.daemon.read().await.history.clone();
    history.enqueue_session_deletion(&agent_id, &session_id);
    history.forget_mirrored(removed_ids.iter().map(String::as_str));
    drop(transaction);
    drop(reservation);
    no_store(json_response(StatusCode::OK, &DeleteResponse { deleted: true }))
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/sessions/{session_id}/export", tag = "sessions",
    params(("agent_id" = String, Path), ("session_id" = String, Path, description = "Percent-encoded session id")),
    responses(
        (status = 200, description = "The visible transcript, including messages kept only in the history store", body = String, content_type = "text/markdown"),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or session not found", body = ErrorBody),
        (status = 503, description = "The history store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn export_session(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    if !is_valid_session_id(&session_id) {
        return rejected(ApiError::not_found());
    }
    match views::full_transcript(&state.daemon, &agent_id, &session_id).await {
        Ok((record, agent_name, messages)) => {
            let markdown = views::transcript_markdown(&record, &agent_name, &messages);
            let mut response = (StatusCode::OK, markdown).into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/markdown; charset=utf-8"),
            );
            if let Ok(disposition) = HeaderValue::from_str(&format!(
                "attachment; filename=\"{}.md\"",
                views::export_file_stem(&record.title)
            )) {
                headers.insert(header::CONTENT_DISPOSITION, disposition);
            }
            no_store(response)
        }
        Err(MessagePageError::Unavailable) => {
            rejected(ApiError::service_unavailable("history store is unavailable"))
        }
        Err(MessagePageError::NotFound | MessagePageError::BeforeNotFound) => {
            rejected(ApiError::not_found())
        }
    }
}
```

(The delete handler removes the messages before the record so the borrow of `runtime` ends before `guard.sessions` is touched; the `let … else` restore path covers a record that vanished, which cannot happen while the transaction is held but keeps the state consistent if it did.)

In `hosts/rust-daemon/src/routes/mod.rs` (hand-format):

- in `#[openapi(paths(…))]`, after `sessions::list_session_messages,` add `sessions::create_session, sessions::update_session, sessions::delete_session, sessions::export_session,`;
- replace the two session route lines added in Task 11 for `/api/agents/{agent_id}/sessions` and `/api/agents/{agent_id}/sessions/{session_id}` with:

```rust
        .route("/api/agents/{agent_id}/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route("/api/agents/{agent_id}/sessions/{session_id}", get(sessions::get_session).patch(sessions::update_session).delete(sessions::delete_session))
        .route("/api/agents/{agent_id}/sessions/{session_id}/export", get(sessions::export_session))
```

In `hosts/rust-daemon/README.md`, add after the Agents table (before `### Agencies`):

```markdown
### Sessions

Every session route requires local-owner authorization, and reads answer `Cache-Control: no-store`.

| Method   | Path                                                    | Description                                                                                                                                                                                      |
| -------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `GET`    | `/api/agents/{agent_id}/sessions`                       | Sessions, newest activity first. Optional `?kind=`, `?archived=`, `?q=`, `?cursor=`, `?limit=` (1–200, default 50), and `?includeHelpers=` (default `true`). Returns `{ sessions, nextCursor }`. |
| `POST`   | `/api/agents/{agent_id}/sessions`                       | Create a chat. Body `{ "title"?: string }`. Returns `201` with `{ session }`; `429` beyond 60 per minute per agent.                                                                              |
| `GET`    | `/api/agents/{agent_id}/sessions/{session_id}`          | One session with `messageCount`, `preview`, `activeRuns`, `unread`, and `capabilities`.                                                                                                          |
| `PATCH`  | `/api/agents/{agent_id}/sessions/{session_id}`          | Rename, archive, or mark read: `{ "title"?, "archived"?, "lastReadAtMs"? }`.                                                                                                                     |
| `DELETE` | `/api/agents/{agent_id}/sessions/{session_id}`          | Delete a session its kind allows; `409` while a run in it is active. Memories are kept.                                                                                                          |
| `GET`    | `/api/agents/{agent_id}/sessions/{session_id}/messages` | Messages oldest to newest: `?before=<messageId>&limit=50&includeHidden=false`. Returns `{ messages, nextBefore }`.                                                                               |
| `GET`    | `/api/agents/{agent_id}/sessions/{session_id}/export`   | The visible transcript as `text/markdown`, including messages kept only in the history store.                                                                                                    |
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions:: runs::ledger routes::tests::sessions agent_runs::tests`
Expected: PASS — the new view, ledger, and 7 route tests plus the existing ones.

- [ ] **Step 7: Commit**

```bash
git add hosts/rust-daemon/src/sessions/views.rs hosts/rust-daemon/src/runs/ledger.rs hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/routes/contracts/sessions.rs hosts/rust-daemon/src/routes/sessions.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/routes/tests/sessions.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): create, rename, archive, delete, and export sessions"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. `POST /api/agents/{id}/sessions` returns 409 for helper agents (they can only run through their companion).
2. Session deletion and agent deletion (`ConnectorManager::delete_agent`) record their history deletions through Task 6's durable `pendingHistoryDeletions` (a per-agent deletion for agent delete).
3. Markdown export includes silent check-in turns, marked `(silent check-in)`, because spec §3.3 promises the full transcript.

---

### Task 13: Hot-tail pruning and `messagePruned`

**Files:**

- Create: `hosts/rust-daemon/src/sessions/pruning.rs`
- Modify: `hosts/rust-daemon/src/sessions/mod.rs` (`pub(crate) mod pruning;`)
- Modify: `hosts/rust-daemon/src/runs/ledger.rs` (`active_sessions`)
- Modify: `hosts/rust-daemon/src/connectors/mod.rs` (`TelegramOutboundRecord::message_pruned`)
- Modify: `hosts/rust-daemon/src/state.rs` (validation; one test; the `test_outbound` literal)
- Modify: `hosts/rust-daemon/src/history/outbox.rs` (prune tick in the worker loop)
- Modify (mechanical, add `message_pruned: false,`): `hosts/rust-daemon/src/schedules.rs`, `hosts/rust-daemon/src/connectors/runtime.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs` (OpenAPI descriptions), `hosts/rust-daemon/README.md` (hot-tail note)

**Interfaces:**

- Consumes: Task 1's `retain_messages`; Task 2's `session_id_for_room`; Task 6's `HistoryService::{is_ephemeral, reconciled, is_mirrored, forget_mirrored, flush_once}`, `HistoryWorker` (its `transactions` field), `FlakyHistoryStore`; Task 11's `test_support`; `DaemonState::{restore_removed_agent, install_test_control_plane_save_gate}` (existing).
- Produces: `crate::sessions::pruning::{HOT_TAIL_MESSAGES (200), HOT_TAIL_MIN_AGE_MS (24 h), PRUNE_INTERVAL_MS (10 min), PruneUndo { message_ids, .. }, async fn prune_once(&SharedDaemonState, &Arc<tokio::sync::Mutex<()>>, now_ms) -> Result<usize, String>}`; `DaemonState::prune_hot_tail(&mut self, now_ms) -> Option<PruneUndo>` and `DaemonState::revert_prune(&mut self, PruneUndo)`; `RunLedger::active_sessions() -> HashSet<(String, String)>`; `TelegramOutboundRecord::message_pruned: bool` (JSON `messagePruned`, omitted when false).
- Behavior (spec §13.2): a message leaves the control plane only when it is mirrored, outside its session's newest 200 (by transcript position), created more than 24 hours ago, not referenced by an undelivered Telegram outbound record (`Pending` or `Failed`; the delivery loop retries `Failed`), and not in a session with a queued, running, or awaiting-approval run. The history worker prunes every 10 minutes inside a control-plane transaction; it never prunes with an ephemeral store or before the outbox reconciled since startup. `Delivered` records whose message was pruned get `messagePruned: true`; snapshot validation accepts a missing assistant message only for such records and rejects `messagePruned` on an undelivered record with `outbound delivery '<id>' is marked messagePruned but was not delivered`. A failed prune save restores the agents and flags; a saved prune forgets the pruned ids in the mirrored set. `GET /api/agents` and `GET /api/agents/{id}` then carry only the hot tail (documented in OpenAPI and the README); `list_connector_messages` shows only the hot tail, and an owner-send retry older than 24 hours no longer finds its earlier turn (see Notes).

- [ ] **Step 1: Write the failing tests**

Create `hosts/rust-daemon/src/sessions/pruning.rs` with only its tests for now:

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anima_core::{Message, MessageRole};
    use tokio::sync::{Mutex, RwLock};

    use super::*;
    use crate::connectors::TelegramOutboundRecord;
    use crate::history::conformance::FlakyHistoryStore;
    use crate::history::HistoryService;
    use crate::runs::{RunRecord, RunSource, RunStart};
    use crate::sessions::test_support::{agent_config, message, seed_messages};

    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
    const NOW_MS: u64 = 10 * DAY_MS;

    fn room(agent_id: &str, room_id: &str, prefix: &str, count: usize) -> Vec<Message> {
        (0..count)
            .map(|index| {
                let role = if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                };
                message(agent_id, &format!("{prefix}{index:03}"), room_id, role, "old", 1_000 + index as u64)
            })
            .collect()
    }

    /// A non-ephemeral store holding every seeded message, reconciled.
    async fn mirrored_state(seed: impl FnOnce(&str) -> Vec<Message>) -> (SharedDaemonState, String) {
        let mut daemon = DaemonState::new();
        daemon.set_history(HistoryService::new(Arc::new(FlakyHistoryStore::new())));
        let agent = daemon.create_agent(agent_config("companion")).unwrap().state.id;
        let messages = seed(&agent);
        seed_messages(&mut daemon, &agent, messages);
        let state = Arc::new(RwLock::new(daemon));
        let history = state.read().await.history.clone();
        history.flush_once(&state, NOW_MS).await.unwrap();
        (state, agent)
    }

    fn outbound(
        agent_id: &str,
        id: &str,
        message_id: &str,
        delivery_state: OutboundDeliveryState,
    ) -> TelegramOutboundRecord {
        TelegramOutboundRecord {
            id: id.into(),
            connector_id: "telegram-a".into(),
            agent_id: agent_id.into(),
            room_id: "chat:a".into(),
            assistant_message_id: message_id.into(),
            text: "old".into(),
            created_at_ms: 1_000,
            delivered_at_ms: None,
            attempts: 1,
            delivery_state,
            message_pruned: false,
        }
    }

    #[tokio::test]
    async fn pruning_keeps_each_sessions_newest_recent_unmirrored_and_referenced_messages() {
        let (state, agent) = mirrored_state(|agent| {
            let mut messages = room(agent, "chat:a", "a", 207);
            messages[1].created_at_ms = NOW_MS - 1_000;
            messages.extend(room(agent, "chat:c", "c", 201));
            messages.extend(room(agent, "chat:b", "b", 3));
            messages
        })
        .await;
        let mut guard = state.write().await;
        guard.history.forget_mirrored(["a003"]);
        for (id, message_id, delivery_state) in [
            ("pending", "a004", OutboundDeliveryState::Pending),
            ("delivered", "a005", OutboundDeliveryState::Delivered),
            ("failed", "a006", OutboundDeliveryState::Failed),
        ] {
            guard
                .outbound
                .insert(id.into(), outbound(&agent, id, message_id, delivery_state));
        }
        guard.runs.insert(RunRecord::running(
            RunStart {
                agent_id: agent.clone(),
                session_id: "chat:c".into(),
                source: RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "still working".into(),
                model: "gpt-5.4".into(),
                provider: None,
                parent_run_id: None,
            },
            NOW_MS,
        ));

        let undo = guard
            .prune_hot_tail(NOW_MS)
            .expect("old mirrored messages leave the hot tail");

        let mut pruned = undo.message_ids.clone();
        pruned.sort();
        assert_eq!(pruned, ["a000", "a002", "a005"]);
        let hot = guard.get_agent(&agent).unwrap().messages;
        assert_eq!(hot.len(), 207 + 201 + 3 - 3);
        assert!(
            hot.iter().any(|message| message.id == "c000"),
            "a session with an active run keeps its whole transcript"
        );
        assert!(guard.outbound["delivered"].message_pruned);
        assert!(!guard.outbound["pending"].message_pruned);
        assert!(!guard.outbound["failed"].message_pruned);
        guard.revert_prune(undo);
        assert_eq!(guard.get_agent(&agent).unwrap().messages.len(), 207 + 201 + 3);
        assert!(!guard.outbound["delivered"].message_pruned);
    }

    #[tokio::test]
    async fn pruning_is_off_for_ephemeral_stores_and_until_reconciled() {
        let mut ephemeral = DaemonState::new();
        let agent = ephemeral.create_agent(agent_config("companion")).unwrap().state.id;
        seed_messages(&mut ephemeral, &agent, room(&agent, "chat:a", "a", 201));
        let ephemeral = Arc::new(RwLock::new(ephemeral));
        let history = ephemeral.read().await.history.clone();
        history.flush_once(&ephemeral, NOW_MS).await.unwrap();
        assert!(
            ephemeral.write().await.prune_hot_tail(NOW_MS).is_none(),
            "an ephemeral store keeps every message hot"
        );

        let mut unreconciled = DaemonState::new();
        unreconciled.set_history(HistoryService::new(Arc::new(FlakyHistoryStore::new())));
        let agent = unreconciled.create_agent(agent_config("companion")).unwrap().state.id;
        seed_messages(&mut unreconciled, &agent, room(&agent, "chat:a", "a", 201));
        assert!(
            unreconciled.prune_hot_tail(NOW_MS).is_none(),
            "nothing is pruned before the store was reconciled"
        );
    }

    #[tokio::test]
    async fn a_failed_prune_save_restores_the_hot_tail_and_a_saved_prune_forgets_mirrored_ids() {
        let (state, agent) = mirrored_state(|agent| room(agent, "chat:a", "a", 201)).await;
        let transactions = Arc::new(Mutex::new(()));
        let gate = state.write().await.install_test_control_plane_save_gate(true);
        gate.release.add_permits(1);

        let error = prune_once(&state, &transactions, NOW_MS)
            .await
            .expect_err("the save failed");
        assert_eq!(error, "injected control-plane save failure");
        assert_eq!(state.read().await.get_agent(&agent).unwrap().messages.len(), 201);
        assert!(state.read().await.history.is_mirrored("a000"));

        assert_eq!(prune_once(&state, &transactions, NOW_MS).await, Ok(1));
        let guard = state.read().await;
        let hot = guard.get_agent(&agent).unwrap().messages;
        assert_eq!(hot.len(), 200);
        assert_eq!(hot[0].id, "a001");
        assert!(!guard.history.is_mirrored("a000"), "pruned ids are no longer hot");
    }
}
```

In `hosts/rust-daemon/src/sessions/mod.rs`, add `pub(crate) mod pruning;` after `pub(crate) mod migration;`.

Add to `mod tests` in `hosts/rust-daemon/src/state.rs`, after `restore_rejects_an_invalid_session_record_without_mutation`:

```rust
    #[test]
    fn a_pruned_assistant_message_is_accepted_only_for_delivered_records() {
        let (snapshot, _) = valid_connector_snapshot();

        let mut delivered = snapshot.clone();
        delivered.outbound[0].delivery_state = OutboundDeliveryState::Delivered;
        delivered.outbound[0].delivered_at_ms = Some(14);
        delivered.outbound[0].assistant_message_id = "pruned-message".into();
        delivered.outbound[0].message_pruned = true;
        DaemonState::new()
            .restore_control_plane_snapshot(delivered)
            .expect("a delivered record may outlive its pruned message");

        for delivery_state in [OutboundDeliveryState::Pending, OutboundDeliveryState::Failed] {
            let mut undelivered = snapshot.clone();
            undelivered.outbound[0].delivery_state = delivery_state;
            undelivered.outbound[0].message_pruned = true;
            assert_eq!(
                DaemonState::new()
                    .restore_control_plane_snapshot(undelivered)
                    .unwrap_err(),
                "outbound delivery 'outbound-1' is marked messagePruned but was not delivered"
            );
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions::pruning state::tests::a_pruned_assistant_message`
Expected: compile errors such as `no field message_pruned on type TelegramOutboundRecord`, `no method named prune_hot_tail found for struct DaemonState`, and `cannot find function prune_once in this scope`.

- [ ] **Step 3: Add the flag, the ledger query, and the validation**

In `hosts/rust-daemon/src/connectors/mod.rs`, add to `TelegramOutboundRecord` after `pub(crate) delivery_state: OutboundDeliveryState,`:

```rust
    /// The assistant message left the control plane's hot tail after this
    /// record was delivered (spec §13.2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) message_pruned: bool,
```

Add `message_pruned: false,` after the `delivery_state` field of every `TelegramOutboundRecord { … }` literal:

- `hosts/rust-daemon/src/state.rs`: `test_outbound`;
- `hosts/rust-daemon/src/schedules.rs`: `execute_claimed`;
- `hosts/rust-daemon/src/connectors/runtime.rs`: `send_from_owner_owned`, `process_pending_once_owned`, and the tests `revoked_delivery_credential_stops_worker_and_requires_replacement`, `processing_commits_agent_message_inbound_and_outbox_then_delivers_stored_text`, `undelivered_outbox_backpressure_preserves_inbound_without_running_agent`, `inbound_backpressure_is_bounded_and_worker_recovers_without_losing_the_batch`, `failed_replacement_never_lets_old_worker_deliver_with_uncommitted_new_token`, `deletion_archives_completed_history_purges_pending_work_and_disables_schedules`, `outbox_compaction_removes_old_delivered_but_never_pending_or_failed`, and both literals in `outbox_compaction_caps_only_the_selected_connector`.

`grep -rn "TelegramOutboundRecord {" hosts/rust-daemon/src` then lists the struct definition plus 14 literals (the 13 above and the pruning test's `outbound` helper), each with `message_pruned`.

In `hosts/rust-daemon/src/runs/ledger.rs`, add to `impl RunLedger` after `active_count_for_session`:

```rust
    /// `(agentId, sessionId)` of every run that is queued, running, or awaiting approval.
    pub(crate) fn active_sessions(&self) -> HashSet<(String, String)> {
        self.records
            .values()
            .filter(|record| !record.status.is_terminal())
            .map(|record| (record.agent_id.clone(), record.session_id.clone()))
            .collect()
    }
```

In `hosts/rust-daemon/src/state.rs`, in the outbound loop of `validate_control_plane_snapshot`, directly before `if let Some(agent) = persisted_agents.get(&record.agent_id) {` add:

```rust
            if record.message_pruned && record.delivery_state != OutboundDeliveryState::Delivered {
                return Err(format!(
                    "outbound delivery '{}' is marked messagePruned but was not delivered",
                    record.id
                ));
            }
```

and change `if !assistant_message_exists {` in that block to `if !assistant_message_exists && !record.message_pruned {`.

- [ ] **Step 4: Implement pruning**

Put this above the test module in `hosts/rust-daemon/src/sessions/pruning.rs`:

```rust
//! Hot-tail pruning (spec §13.2): the control plane keeps each session's
//! newest messages and anything recent; older messages the history store
//! already holds leave the snapshot every ten minutes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anima_core::AgentRuntimeSnapshot;
use tokio::sync::Mutex;
use tracing::warn;

use super::session_id_for_room;
use crate::app::SharedDaemonState;
use crate::connectors::OutboundDeliveryState;
use crate::state::DaemonState;

/// Each session keeps at least its newest 200 messages (spec §16).
pub(crate) const HOT_TAIL_MESSAGES: usize = 200;
/// Messages created in the last 24 hours always stay.
pub(crate) const HOT_TAIL_MIN_AGE_MS: u64 = 24 * 60 * 60 * 1_000;
/// How often the history worker prunes.
pub(crate) const PRUNE_INTERVAL_MS: u64 = 10 * 60 * 1_000;

/// What one pruning pass changed, so a failed save can put it back.
#[derive(Debug, Default)]
pub(crate) struct PruneUndo {
    agents: Vec<AgentRuntimeSnapshot>,
    marked_outbound: Vec<String>,
    pub(crate) message_ids: Vec<String>,
}

impl DaemonState {
    /// Removes the hot messages that may leave the control plane: mirrored,
    /// outside their session's newest 200, older than 24 hours, not referenced
    /// by an undelivered Telegram record, and not in a session with an active
    /// run. Delivered records of pruned messages are marked `messagePruned`.
    /// `None` when nothing may be pruned (an ephemeral store, a store not yet
    /// reconciled since startup, or no candidates).
    pub(crate) fn prune_hot_tail(&mut self, now_ms: u64) -> Option<PruneUndo> {
        if self.history.is_ephemeral() || !self.history.reconciled() {
            return None;
        }
        let cutoff = now_ms.saturating_sub(HOT_TAIL_MIN_AGE_MS);
        let undelivered_references = self
            .outbound
            .values()
            .filter(|record| record.delivery_state != OutboundDeliveryState::Delivered)
            .map(|record| record.assistant_message_id.clone())
            .collect::<HashSet<_>>();
        let active_sessions = self.runs.active_sessions();
        let mut undo = PruneUndo::default();
        let agent_ids = self.agents.keys().cloned().collect::<Vec<_>>();
        for agent_id in agent_ids {
            let Some(runtime) = self.agents.get(&agent_id) else {
                continue;
            };
            let mut newer_in_room: HashMap<&str, usize> = HashMap::new();
            let mut prunable = HashSet::new();
            for message in runtime.messages().iter().rev() {
                let rank = newer_in_room.entry(message.room_id.as_str()).or_insert(0);
                *rank += 1;
                if *rank <= HOT_TAIL_MESSAGES
                    || message.created_at_ms > cutoff
                    || !self.history.is_mirrored(&message.id)
                    || undelivered_references.contains(&message.id)
                    || active_sessions
                        .contains(&(agent_id.clone(), session_id_for_room(&message.room_id)))
                {
                    continue;
                }
                prunable.insert(message.id.clone());
            }
            if prunable.is_empty() {
                continue;
            }
            let runtime = self
                .agents
                .get_mut(&agent_id)
                .expect("the agent was just read");
            undo.agents.push(runtime.snapshot());
            undo.message_ids.extend(
                runtime
                    .retain_messages(|message| !prunable.contains(&message.id))
                    .into_iter()
                    .map(|message| message.id),
            );
        }
        if undo.message_ids.is_empty() {
            return None;
        }
        let pruned = undo
            .message_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        for (id, record) in self.outbound.iter_mut() {
            if record.delivery_state == OutboundDeliveryState::Delivered
                && !record.message_pruned
                && pruned.contains(record.assistant_message_id.as_str())
            {
                record.message_pruned = true;
                undo.marked_outbound.push(id.clone());
            }
        }
        Some(undo)
    }

    /// Puts back what `prune_hot_tail` removed after its save failed.
    pub(crate) fn revert_prune(&mut self, undo: PruneUndo) {
        for snapshot in undo.agents {
            let agent_id = snapshot.state.id.clone();
            if let Err(error) = self.restore_removed_agent(snapshot) {
                warn!(agent_id = %agent_id, error = %error, "could not restore an agent's hot tail after a failed prune");
            }
        }
        for id in undo.marked_outbound {
            if let Some(record) = self.outbound.get_mut(&id) {
                record.message_pruned = false;
            }
        }
    }
}

/// One pruning pass inside a control-plane transaction; returns how many
/// messages left the hot tail. A failed save puts everything back.
pub(crate) async fn prune_once(
    state: &SharedDaemonState,
    transactions: &Arc<Mutex<()>>,
    now_ms: u64,
) -> Result<usize, String> {
    let _transaction = transactions.lock().await;
    let (undo, persist) = {
        let mut guard = state.write().await;
        let Some(undo) = guard.prune_hot_tail(now_ms) else {
            return Ok(0);
        };
        (undo, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state.write().await.revert_prune(undo);
        return Err(error.to_string());
    }
    let history = state.read().await.history.clone();
    history.forget_mirrored(undo.message_ids.iter().map(String::as_str));
    Ok(undo.message_ids.len())
}
```

In `hosts/rust-daemon/src/history/outbox.rs`:

- remove the `#[allow(dead_code)]` line above `transactions: Arc<Mutex<()>>,` in `HistoryWorker`;
- in `HistoryWorker::start`, replace

```rust
        let state = Arc::clone(&self.state);
        let join = tokio::spawn(async move {
            loop {
```

with

```rust
        let state = Arc::clone(&self.state);
        let transactions = Arc::clone(&self.transactions);
        let join = tokio::spawn(async move {
            let mut next_prune_at_ms =
                now_millis().saturating_add(crate::sessions::pruning::PRUNE_INTERVAL_MS);
            loop {
```

and replace the loop's last statement `let _ = history.flush_once(&state, now_millis()).await;` with:

```rust
                let _ = history.flush_once(&state, now_millis()).await;
                let now_ms = now_millis();
                if now_ms >= next_prune_at_ms {
                    next_prune_at_ms =
                        now_ms.saturating_add(crate::sessions::pruning::PRUNE_INTERVAL_MS);
                    match crate::sessions::pruning::prune_once(&state, &transactions, now_ms).await {
                        Ok(0) => {}
                        Ok(pruned) => {
                            tracing::info!(pruned, "moved old mirrored messages out of the control plane");
                        }
                        Err(error) => {
                            warn!(error = %error, "hot-tail pruning could not save; the messages stay in the control plane");
                        }
                    }
                }
```

- update the `HistoryWorker` doc comment to `/// Runs the outbox flush loop and, every ten minutes, hot-tail pruning.`

- [ ] **Step 5: Document the hot tail**

In `hosts/rust-daemon/src/routes/mod.rs` (hand-format):

- in the `list_agents_entry` attribute, change the 200 description to `"List agents (AgentSummariesEnvelope with view=summary). messages hold the hot tail: each session's newest 200 messages plus anything from the last 24 hours; page older ones with GET /api/agents/{agent_id}/sessions/{session_id}/messages"`;
- in the `get_agent_entry` attribute, change `description = "Agent snapshot"` to `description = "Agent snapshot. messages hold the hot tail: each session's newest 200 messages plus anything from the last 24 hours; page older ones with GET /api/agents/{agent_id}/sessions/{session_id}/messages"`.

In `hosts/rust-daemon/README.md`, add after the Sessions table:

```markdown
Every committed message is mirrored to the history store (`ANIMAOS_RS_HISTORY_SQLITE_FILE`, the Postgres history tables, or bounded in-memory tables in ephemeral mode). Every 10 minutes the daemon prunes mirrored messages that are neither among their session's newest 200 nor from the last 24 hours from the control plane (never in ephemeral mode, never while a run in the session is active or a Telegram delivery still needs the message). `GET /api/agents` and `GET /api/agents/{agent_id}` then carry only this hot tail; the session messages and export routes still return the full history.
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- sessions:: state:: connectors:: schedules:: history:: runs::ledger`
Expected: PASS — the 3 pruning tests, the validation test, and every existing connector, schedule, state, and history test (the new field defaults to `false` and is omitted from JSON).

- [ ] **Step 7: Commit**

```bash
git add hosts/rust-daemon/src/sessions/pruning.rs hosts/rust-daemon/src/sessions/mod.rs hosts/rust-daemon/src/runs/ledger.rs hosts/rust-daemon/src/connectors/mod.rs hosts/rust-daemon/src/state.rs hosts/rust-daemon/src/history/outbox.rs hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/connectors/runtime.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/README.md
git commit -m "feat(daemon): prune mirrored messages from the control plane's hot tail"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. The newest-200 hot-tail rank counts visible messages only (silent check-in pairs do not consume the window).
2. Remove the stale `agent_snapshots` duplication (M1 carry-forward): `list_agents`, `get_agent`, `team_roster`, `peer_ids`, and `resolve_peer` read the canonical runtimes; `agent_snapshots` keeps entries only for agents without a loaded runtime (verify every user; delete the map if none remain), so `retain_messages` (delete and prune) actually frees memory. Test: after pruning, `list_agents()` returns only the hot tail.

---

### Task 14: SDK sessions client

**Files:**

- Create: `packages/sdk/src/sessions.ts`, `packages/sdk/src/sessions.spec.ts`
- Modify: `packages/sdk/src/client.ts` (`requestText`, `sessions`), `packages/sdk/src/agents.ts` (`AgentSummary`, `listSummaries`), `packages/sdk/src/agents.spec.ts` (one test), `packages/sdk/src/index.ts` (exports)

**Interfaces:**

- Consumes: the Task 11–12 routes and JSON shapes (camelCase): `GET/POST /api/agents/{id}/sessions`, `GET/PATCH/DELETE /api/agents/{id}/sessions/{sid}`, `GET …/messages`, `GET …/export`, `GET /api/agents?view=summary`.
- Produces (exported from `@animaOS-SWARM/sdk`): `SessionsClient` with `list(agentId, SessionListOptions?) -> Promise<SessionPage>`, `get(agentId, sessionId, { signal? }?) -> Promise<Session>`, `messages(agentId, sessionId, SessionMessageOptions?) -> Promise<SessionMessagePage>`, `create(agentId, { title? }?) -> Promise<Session>`, `update(agentId, sessionId, SessionUpdateInput) -> Promise<Session>`, `remove(agentId, sessionId) -> Promise<void>`, `exportMarkdown(agentId, sessionId, { signal? }?) -> Promise<string>`; types `Session`, `SessionKind`, `SessionOrigin`, `SessionTitleSource`, `SessionCapabilities`, `SessionSummary`, `SessionContextTrimmed`, `SessionMatch`, `SessionPage`, `SessionListOptions`, `SessionMessage`, `SessionMessageAttachment`, `SessionMessagePage`, `SessionMessageOptions`, `SessionUpdateInput`; `DaemonClient.sessions`; `DaemonClient.requestText(path, init?) -> Promise<string>`; `AgentsClient.listSummaries() -> Promise<AgentSummary[]>` and type `AgentSummary`. Changes are additive.

- [ ] **Step 1: Write the failing tests**

Create `packages/sdk/src/sessions.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';

import { createDaemonClient, DaemonHttpError } from './index.js';

function transport(respond: (url: string) => Response) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return respond(String(url));
    },
  });
  return { sessions: client.sessions, requests };
}

const session = {
  id: 'chat:1',
  agentId: 'agent/a',
  roomId: 'chat:1',
  kind: 'chat',
  title: 'Plans',
};

describe('sessions client', () => {
  it('lists sessions with encoded filters', async () => {
    const page = { sessions: [session], nextCursor: 'next' };
    const { sessions, requests } = transport(() => Response.json(page));

    expect(
      await sessions.list('agent/a', {
        kind: 'chat',
        archived: false,
        q: 'budget plan',
        cursor: 'abc',
        limit: 20,
        includeHelpers: false,
      }),
    ).toEqual(page);
    await sessions.list('agent/a');

    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/agent%2Fa/sessions?kind=chat&archived=false&q=budget+plan&cursor=abc&limit=20&includeHelpers=false',
      '/api/agents/agent%2Fa/sessions',
    ]);
  });

  it('reads one session and a message page with an encoded session id', async () => {
    const { sessions, requests } = transport((url) =>
      url.includes('/messages')
        ? Response.json({ messages: [{ id: 'm1' }], nextBefore: 'm1' })
        : Response.json({ session }),
    );

    expect(await sessions.get('agent/a', 'chat:1')).toEqual(session);
    expect(
      await sessions.messages('agent/a', 'chat:1', {
        before: 'm2',
        limit: 10,
        includeHidden: true,
      }),
    ).toEqual({ messages: [{ id: 'm1' }], nextBefore: 'm1' });
    await sessions.messages('agent/a', 'chat:1');

    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/agent%2Fa/sessions/chat%3A1',
      '/api/agents/agent%2Fa/sessions/chat%3A1/messages?before=m2&limit=10&includeHidden=true',
      '/api/agents/agent%2Fa/sessions/chat%3A1/messages',
    ]);
  });

  it('creates, updates, and removes sessions', async () => {
    const { sessions, requests } = transport((url) =>
      url.endsWith('chat%3A1') && requests.at(-1)?.init?.method === 'DELETE'
        ? Response.json({ deleted: true })
        : Response.json({ session }),
    );

    expect(await sessions.create('agent/a', { title: 'Trip' })).toEqual(
      session,
    );
    expect(await sessions.create('agent/a')).toEqual(session);
    expect(
      await sessions.update('agent/a', 'chat:1', {
        archived: true,
        lastReadAtMs: 5,
      }),
    ).toEqual(session);
    await expect(sessions.remove('agent/a', 'chat:1')).resolves.toBeUndefined();

    expect(
      requests.map(({ url, init }) => [
        init?.method,
        url,
        init?.body === undefined ? undefined : JSON.parse(String(init.body)),
      ]),
    ).toEqual([
      ['POST', '/api/agents/agent%2Fa/sessions', { title: 'Trip' }],
      ['POST', '/api/agents/agent%2Fa/sessions', {}],
      [
        'PATCH',
        '/api/agents/agent%2Fa/sessions/chat%3A1',
        { archived: true, lastReadAtMs: 5 },
      ],
      ['DELETE', '/api/agents/agent%2Fa/sessions/chat%3A1', undefined],
    ]);
  });

  it('exports Markdown as text and surfaces daemon errors', async () => {
    const { sessions, requests } = transport((url) =>
      url.includes('missing')
        ? Response.json({ error: 'not found' }, { status: 404 })
        : new Response('# Plans\n', {
            headers: { 'content-type': 'text/markdown; charset=utf-8' },
          }),
    );

    expect(await sessions.exportMarkdown('agent/a', 'chat:1')).toBe(
      '# Plans\n',
    );
    expect(requests[0].url).toBe(
      '/api/agents/agent%2Fa/sessions/chat%3A1/export',
    );
    expect(
      (requests[0].init?.headers as Record<string, string>).accept,
    ).toContain('text/markdown');

    const failure = sessions.exportMarkdown('agent/a', 'chat:missing');
    await expect(failure).rejects.toBeInstanceOf(DaemonHttpError);
    await expect(failure).rejects.toMatchObject({
      status: 404,
      message: 'not found',
    });
  });
});
```

Append to the `describe('agent transport', …)` block in `packages/sdk/src/agents.spec.ts`:

```ts
it('lists agent summaries without transcripts', async () => {
  const summary = {
    state: { id: 'agent/a b' },
    messageCount: 3,
    eventCount: 1,
    lastTask: null,
  };
  const { agents, requests } = transport({ agents: [summary] });
  expect(await agents.listSummaries()).toEqual([summary]);
  expect(requests[0].url).toBe('/api/agents?view=summary');
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `bun x nx test @animaOS-SWARM/sdk`
Expected: FAIL — `client.sessions` is undefined (`Cannot read properties of undefined (reading 'list')`) and `agents.listSummaries is not a function`.

- [ ] **Step 3: Implement**

Create `packages/sdk/src/sessions.ts`:

```ts
import type { DaemonClient } from './client.js';

export type SessionKind = 'chat' | 'telegram' | 'checkin' | 'job' | 'helper';
export type SessionOrigin =
  | 'web'
  | 'api'
  | 'telegram'
  | 'schedule'
  | 'job'
  | 'delegation'
  | 'peer';
export type SessionTitleSource =
  | 'first_message'
  | 'generated'
  | 'owner'
  | 'system';

export interface SessionCapabilities {
  send: boolean;
  steer: boolean;
  stop: boolean;
  rename: boolean;
  archive: boolean;
  delete: boolean;
  compact: boolean;
  export: boolean;
}

export interface SessionSummary {
  text: string;
  throughMessageId: string;
  createdAtMs: number;
  sourceMessageCount: number;
}

export interface SessionContextTrimmed {
  droppedThroughMessageId: string;
  atMs: number;
}

/** Why a search result matched; `messageId` is null for a title match. */
export interface SessionMatch {
  messageId: string | null;
  snippet: string;
}

/** A session with the daemon's derived fields (spec §3.2). */
export interface Session {
  id: string;
  agentId: string;
  /** The transcript room; equals `id` except for mapped legacy rooms. */
  roomId: string;
  kind: SessionKind;
  origin: SessionOrigin;
  title: string;
  titleSource: SessionTitleSource;
  createdAtMs: number;
  lastActivityAtMs: number;
  lastReadAtMs: number | null;
  archived: boolean;
  parentSessionId: string | null;
  parentRunId: string | null;
  parentAgentId: string | null;
  summary: SessionSummary | null;
  contextTrimmed: SessionContextTrimmed | null;
  messageCount: number;
  preview: string | null;
  activeRuns: number;
  pendingApprovals: number;
  unread: boolean;
  capabilities: SessionCapabilities;
  match?: SessionMatch;
}

export interface SessionPage {
  sessions: Session[];
  nextCursor: string | null;
}

export interface SessionListOptions {
  kind?: SessionKind;
  /** true lists only archived sessions; the daemon default is false. */
  archived?: boolean;
  q?: string;
  cursor?: string;
  /** 1–200; the daemon default is 50. */
  limit?: number;
  /** The daemon default is true. */
  includeHelpers?: boolean;
  signal?: AbortSignal;
}

export interface SessionMessageAttachment {
  type: 'file' | 'image' | 'url';
  name: string;
}

export interface SessionMessage {
  id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  text: string;
  attachments: SessionMessageAttachment[];
  metadata: Record<string, unknown>;
  createdAtMs: number;
  /** Present on silent check-in messages when `includeHidden` is set. */
  hidden?: boolean;
}

export interface SessionMessagePage {
  /** Oldest to newest within the page. */
  messages: SessionMessage[];
  nextBefore: string | null;
}

export interface SessionMessageOptions {
  before?: string;
  limit?: number;
  includeHidden?: boolean;
  signal?: AbortSignal;
}

export interface SessionUpdateInput {
  title?: string;
  archived?: boolean;
  lastReadAtMs?: number;
}

export class SessionsClient {
  constructor(private readonly client: DaemonClient) {}

  async list(
    agentId: string,
    options: SessionListOptions = {},
  ): Promise<SessionPage> {
    const search = new URLSearchParams();
    if (options.kind) search.set('kind', options.kind);
    if (options.archived !== undefined)
      search.set('archived', String(options.archived));
    if (options.q) search.set('q', options.q);
    if (options.cursor) search.set('cursor', options.cursor);
    if (options.limit !== undefined) search.set('limit', String(options.limit));
    if (options.includeHelpers !== undefined)
      search.set('includeHelpers', String(options.includeHelpers));
    return this.client.requestJson<SessionPage>(
      withQuery(sessionsPath(agentId), search),
      { signal: options.signal },
    );
  }

  async get(
    agentId: string,
    sessionId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<Session> {
    const response = await this.client.requestJson<{ session: Session }>(
      sessionPath(agentId, sessionId),
      { signal: options.signal },
    );
    return response.session;
  }

  async messages(
    agentId: string,
    sessionId: string,
    options: SessionMessageOptions = {},
  ): Promise<SessionMessagePage> {
    const search = new URLSearchParams();
    if (options.before) search.set('before', options.before);
    if (options.limit !== undefined) search.set('limit', String(options.limit));
    if (options.includeHidden !== undefined)
      search.set('includeHidden', String(options.includeHidden));
    return this.client.requestJson<SessionMessagePage>(
      withQuery(`${sessionPath(agentId, sessionId)}/messages`, search),
      { signal: options.signal },
    );
  }

  async create(
    agentId: string,
    input: { title?: string } = {},
  ): Promise<Session> {
    const response = await this.client.requestJson<{ session: Session }>(
      sessionsPath(agentId),
      { method: 'POST', body: input },
    );
    return response.session;
  }

  async update(
    agentId: string,
    sessionId: string,
    patch: SessionUpdateInput,
  ): Promise<Session> {
    const response = await this.client.requestJson<{ session: Session }>(
      sessionPath(agentId, sessionId),
      { method: 'PATCH', body: patch },
    );
    return response.session;
  }

  async remove(agentId: string, sessionId: string): Promise<void> {
    await this.client.requestJson<{ deleted: boolean }>(
      sessionPath(agentId, sessionId),
      { method: 'DELETE' },
    );
  }

  /** The visible transcript as Markdown, including archived history. */
  async exportMarkdown(
    agentId: string,
    sessionId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<string> {
    return this.client.requestText(
      `${sessionPath(agentId, sessionId)}/export`,
      {
        signal: options.signal,
      },
    );
  }
}

function sessionsPath(agentId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}/sessions`;
}

function sessionPath(agentId: string, sessionId: string): string {
  return `${sessionsPath(agentId)}/${encodeURIComponent(sessionId)}`;
}

function withQuery(path: string, search: URLSearchParams): string {
  const query = search.toString();
  return query ? `${path}?${query}` : path;
}
```

In `packages/sdk/src/client.ts`:

- add `import { SessionsClient } from './sessions.js';` after `import { SwarmsClient } from './swarms.js';`;
- add `readonly sessions: SessionsClient;` after `readonly swarms: SwarmsClient;` in `DaemonClient`, and `this.sessions = new SessionsClient(this);` after `this.swarms = new SwarmsClient(this);` in the constructor;
- add after `requestJson`:

```ts
  /** A text response such as Markdown; failures throw like `requestJson`. */
  async requestText(path: string, init: RequestInit = {}): Promise<string> {
    const response = await this.fetchWithConnectionErrors(path, {
      ...init,
      headers: {
        accept: 'text/markdown, text/plain;q=0.9, */*;q=0.1',
        ...headersToObject(init.headers),
      },
    });
    if (!response.ok) {
      throw new DaemonHttpError(response.status, await readResponseBody(response));
    }
    return response.text();
  }
```

In `packages/sdk/src/agents.ts`, add after `AgentSnapshot`:

```ts
/** An agent without its transcript (`GET /api/agents?view=summary`). */
export interface AgentSummary {
  state: DaemonAgentState;
  messageCount: number;
  eventCount: number;
  lastTask: DaemonTaskResult | null;
}
```

and add to `AgentsClient` after `list()`:

```ts
  async listSummaries(): Promise<AgentSummary[]> {
    const response = await this.client.requestJson<{ agents: AgentSummary[] }>(
      '/api/agents?view=summary',
    );
    return response.agents;
  }
```

In `packages/sdk/src/index.ts`:

- add `AgentSummary,` to the `export type { … } from './agents.js';` list after `AgentSnapshot,`;
- add after the `ConnectorsClient` export line:

```ts
export { SessionsClient } from './sessions.js';
export type {
  Session,
  SessionCapabilities,
  SessionContextTrimmed,
  SessionKind,
  SessionListOptions,
  SessionMatch,
  SessionMessage,
  SessionMessageAttachment,
  SessionMessageOptions,
  SessionMessagePage,
  SessionOrigin,
  SessionPage,
  SessionSummary,
  SessionTitleSource,
  SessionUpdateInput,
} from './sessions.js';
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `bun x nx test @animaOS-SWARM/sdk`
Expected: PASS (4 new session tests, the new agents test, and every existing SDK test).

Run: `bun x nx run @animaOS-SWARM/sdk:typecheck`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add packages/sdk/src/sessions.ts packages/sdk/src/sessions.spec.ts packages/sdk/src/client.ts packages/sdk/src/agents.ts packages/sdk/src/agents.spec.ts packages/sdk/src/index.ts
git commit -m "feat(sdk): add the sessions client and agent summaries"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. End the task by running `bun x nx run @animaOS-SWARM/sdk:build` so later direct `vitest` runs in apps/web resolve the new sessions client from `packages/sdk/dist`.
2. The SDK maps a 404 from the sessions routes to a typed daemon-too-old error so the web can show "Update the daemon" (spec §13.4). Test.

---

### Task 15: Web hash routing and session data hooks

**Files:**

- Create: `apps/web/src/lib/hash-route.ts`, `apps/web/src/lib/hash-route.test.ts`, `apps/web/src/lib/session-groups.ts`, `apps/web/src/lib/session-groups.test.ts`, `apps/web/src/test/sessions.ts`, `apps/web/src/hooks/useCompanionSessions.ts`, `apps/web/src/hooks/useCompanionSessions.test.tsx`, `apps/web/src/hooks/useSessionMessages.ts`, `apps/web/src/hooks/useSessionMessages.test.tsx`
- Modify: `apps/web/src/lib/daemon-api.ts` (session methods, `toChatMessage`), `apps/web/src/lib/daemon-api.test.ts` (two tests)

**Interfaces:**

- Consumes: Task 14's `Session`, `SessionKind`, `SessionMessage`, `SessionListOptions`, `SessionMessageOptions`, `SessionUpdateInput` and `setupClient.sessions` (the SDK client already created in `daemon-api.ts`).
- Produces:
  - `lib/hash-route.ts`: `HASH_PAGES` (the 11 pages of §15.1), `HashPage`, `HashRoute = { kind: 'home' } | { kind: 'session'; sessionId } | { kind: 'page'; page }`, `parseHashRoute(hash)`, `formatHashRoute(route)`, `sameRoute(a, b)`, `Navigate = (route, { replace? }?) => void`, `useHashRoute(): [HashRoute, Navigate]` (listens to `hashchange` and `popstate`; navigation uses `pushState`/`replaceState`, so it never fires events itself);
  - `lib/session-groups.ts`: `SESSION_KIND_ORDER`, `SESSION_KIND_LABELS` (`Chat`, `Telegram`, `Check-in`, `Job`, `Helper`), `SESSION_KIND_FILTER_LABELS` (`Chats`, `Telegram`, `Check-ins`, `Jobs`, `Helpers`), `sessionKey(session)`, `presentKinds(sessions)`, `SessionGroupLabel`, `SessionNode { session, helpers }`, `SessionGroup { label, nodes }`, `groupSessions(sessions, now?, nestHelpers?)` (Today, Yesterday, Previous 7 days, Older; helpers nest under their listed parent session), `exportFileName(title)`;
  - `test/sessions.ts`: `sessionFixture(id, overrides?) -> Session` (a read-write chat of `agent-main`; test support, not a test file);
  - `hooks/useCompanionSessions.ts`: `SESSION_LIST_POLL_MS` (10 000), `SESSION_LIST_LIMIT` (200), `useCompanionSessions(agentId, { archived, query }) -> { sessions, loading, error, refresh, upsert(session), remove(session) }` (always `includeHelpers: true`);
  - `hooks/useSessionMessages.ts`: `SESSION_MESSAGE_PAGE` (50), `SESSION_MESSAGES_POLL_MS` (3 000), `mergeNewest(current, page)`, `useSessionMessages(agentId, sessionId, refreshKey?) -> { messages, hasOlder, loadingOlder, loadOlder, missing, error, refresh }` (a 404 sets `missing` and stops polling; a new `refreshKey` reloads at once);
  - `daemon` methods `listSessions(agentId, options?)`, `createSession(agentId, { title? }?)`, `updateSession(agentId, sessionId, patch)`, `deleteSession(agentId, sessionId)`, `sessionMessages(agentId, sessionId, options?)`, `exportSession(agentId, sessionId)`; `toChatMessage(SessionMessage) -> ChatMessage` (check-in prompts become a `System` line without the scheduler suffix).
- Timers: the bootstrap agent poll stays at 5 000 ms (tests capture it by that value), so the new polls use 10 000 and 3 000.

- [ ] **Step 1: Write the failing tests**

Create `apps/web/src/test/sessions.ts`:

```ts
import type { Session } from '@animaOS-SWARM/sdk';

/** A read-write chat of `agent-main` with every derived field. */
export function sessionFixture(
  id: string,
  overrides: Partial<Session> = {},
): Session {
  return {
    id,
    agentId: 'agent-main',
    roomId: id,
    kind: 'chat',
    origin: 'web',
    title: 'New chat',
    titleSource: 'first_message',
    createdAtMs: 1,
    lastActivityAtMs: 1,
    lastReadAtMs: null,
    archived: false,
    parentSessionId: null,
    parentRunId: null,
    parentAgentId: null,
    summary: null,
    contextTrimmed: null,
    messageCount: 0,
    preview: null,
    activeRuns: 0,
    pendingApprovals: 0,
    unread: false,
    capabilities: {
      send: true,
      steer: true,
      stop: true,
      rename: true,
      archive: true,
      delete: true,
      compact: true,
      export: true,
    },
    ...overrides,
  };
}
```

Create `apps/web/src/lib/hash-route.test.ts`:

```ts
import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import {
  formatHashRoute,
  parseHashRoute,
  useHashRoute,
  type HashRoute,
} from './hash-route';

afterEach(() => {
  window.history.replaceState(null, '', '/');
});

describe('hash routes', () => {
  it('parses sessions and pages and falls back to a new chat', () => {
    expect(parseHashRoute('')).toEqual({ kind: 'home' });
    expect(parseHashRoute('#/')).toEqual({ kind: 'home' });
    expect(parseHashRoute('#/s/chat%3Aabc')).toEqual({
      kind: 'session',
      sessionId: 'chat:abc',
    });
    expect(parseHashRoute('#/s/legacy-room:0f')).toEqual({
      kind: 'session',
      sessionId: 'legacy-room:0f',
    });
    expect(parseHashRoute('#/work')).toEqual({ kind: 'page', page: 'work' });
    expect(parseHashRoute('#/capabilities')).toEqual({
      kind: 'page',
      page: 'capabilities',
    });
    for (const hash of ['#/unknown', '#/s/', '#/s/bad%20id', '#/s/%E0%A4%A']) {
      expect(parseHashRoute(hash)).toEqual({ kind: 'home' });
    }
  });

  it('formats routes that parse back to themselves', () => {
    const routes: HashRoute[] = [
      { kind: 'home' },
      { kind: 'session', sessionId: 'chat:abc' },
      { kind: 'page', page: 'connectors' },
    ];
    for (const route of routes) {
      expect(parseHashRoute(formatHashRoute(route))).toEqual(route);
    }
    expect(formatHashRoute({ kind: 'session', sessionId: 'chat:abc' })).toBe(
      '#/s/chat%3Aabc',
    );
  });

  it('follows the location and navigates with or without a history entry', () => {
    window.history.replaceState(null, '', '/#/work');
    const { result } = renderHook(() => useHashRoute());
    expect(result.current[0]).toEqual({ kind: 'page', page: 'work' });

    const before = window.history.length;
    act(() => result.current[1]({ kind: 'session', sessionId: 'chat:1' }));
    expect(window.location.hash).toBe('#/s/chat%3A1');
    expect(window.history.length).toBe(before + 1);
    expect(result.current[0]).toEqual({ kind: 'session', sessionId: 'chat:1' });

    act(() => result.current[1]({ kind: 'home' }, { replace: true }));
    expect(window.location.hash).toBe('#/');
    expect(window.history.length).toBe(before + 1);

    act(() => {
      window.history.replaceState(null, '', '/#/files');
      window.dispatchEvent(new HashChangeEvent('hashchange'));
    });
    expect(result.current[0]).toEqual({ kind: 'page', page: 'files' });
  });
});
```

Create `apps/web/src/lib/session-groups.test.ts`:

```ts
import { describe, expect, it } from 'vitest';

import { sessionFixture } from '../test/sessions';
import { exportFileName, groupSessions, presentKinds } from './session-groups';

const NOW = new Date(2026, 8, 24, 12, 0, 0);
const HOUR = 60 * 60 * 1000;

describe('session groups', () => {
  it('groups by local day and nests helpers under their listed parent', () => {
    const today = sessionFixture('chat:today', {
      title: 'Today',
      lastActivityAtMs: NOW.getTime() - HOUR,
    });
    const helper = sessionFixture('room-9', {
      agentId: 'helper-1',
      kind: 'helper',
      parentAgentId: 'agent-main',
      parentSessionId: 'chat:today',
      lastActivityAtMs: NOW.getTime() - 2 * HOUR,
    });
    const orphan = sessionFixture('peer:beta:agent-main', {
      kind: 'helper',
      parentAgentId: 'beta',
      lastActivityAtMs: NOW.getTime() - 3 * HOUR,
    });
    const yesterday = sessionFixture('chat:yesterday', {
      lastActivityAtMs: NOW.getTime() - 24 * HOUR,
    });
    const week = sessionFixture('chat:week', {
      lastActivityAtMs: NOW.getTime() - 4 * 24 * HOUR,
    });
    const older = sessionFixture('chat:older', {
      lastActivityAtMs: NOW.getTime() - 30 * 24 * HOUR,
    });

    const groups = groupSessions(
      [today, helper, orphan, yesterday, week, older],
      NOW,
    );

    expect(
      groups.map((group) => [
        group.label,
        group.nodes.map((node) => [
          node.session.id,
          node.helpers.map((item) => item.id),
        ]),
      ]),
    ).toEqual([
      [
        'Today',
        [
          ['chat:today', ['room-9']],
          ['peer:beta:agent-main', []],
        ],
      ],
      ['Yesterday', [['chat:yesterday', []]]],
      ['Previous 7 days', [['chat:week', []]]],
      ['Older', [['chat:older', []]]],
    ]);
    expect(
      groupSessions([today, helper], NOW, false)[0].nodes.map(
        (node) => node.session.id,
      ),
    ).toEqual(['chat:today', 'room-9']);
  });

  it('lists only the kinds present, in sidebar order', () => {
    expect(
      presentKinds([
        sessionFixture('job:1', { kind: 'job' }),
        sessionFixture('chat:1'),
        sessionFixture('telegram:t', { kind: 'telegram' }),
      ]),
    ).toEqual(['chat', 'telegram', 'job']);
  });

  it('names export files like the daemon', () => {
    expect(exportFileName('Check-in · Check status')).toBe(
      'check-in-check-status.md',
    );
    expect(exportFileName('···')).toBe('session.md');
  });
});
```

Create `apps/web/src/hooks/useCompanionSessions.test.tsx`:

```tsx
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../lib/daemon-api';
import { sessionFixture } from '../test/sessions';
import {
  SESSION_LIST_POLL_MS,
  useCompanionSessions,
} from './useCompanionSessions';

const nativeSetTimeout = window.setTimeout.bind(window);

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useCompanionSessions', () => {
  it('loads helper sessions too and reloads for a new search', async () => {
    const list = vi.spyOn(daemon, 'listSessions').mockResolvedValue({
      sessions: [sessionFixture('chat:1')],
      nextCursor: null,
    });
    const { result, rerender } = renderHook(
      ({ query }) =>
        useCompanionSessions('agent-main', { archived: false, query }),
      { initialProps: { query: '' } },
    );

    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    expect(list).toHaveBeenLastCalledWith('agent-main', {
      includeHelpers: true,
      archived: false,
      limit: 200,
    });
    rerender({ query: ' budget ' });
    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith('agent-main', {
        includeHelpers: true,
        archived: false,
        limit: 200,
        q: 'budget',
      }),
    );
  });

  it('polls on its own interval and keeps the list when a poll fails', async () => {
    let poll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === SESSION_LIST_POLL_MS) {
        poll = handler as () => void;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);
    const list = vi
      .spyOn(daemon, 'listSessions')
      .mockResolvedValueOnce({
        sessions: [sessionFixture('chat:1')],
        nextCursor: null,
      })
      .mockRejectedValueOnce(new Error('daemon unavailable'));
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );

    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await waitFor(() => expect(poll).toBeDefined());
    await act(async () => {
      poll?.();
    });

    await waitFor(() =>
      expect(result.current.error).toBe('daemon unavailable'),
    );
    expect(result.current.sessions).toHaveLength(1);
    expect(list).toHaveBeenCalledTimes(2);
  });

  it('shows a created session at once and forgets a deleted one', async () => {
    vi.spyOn(daemon, 'listSessions').mockResolvedValue({
      sessions: [sessionFixture('chat:1')],
      nextCursor: null,
    });
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));

    act(() => result.current.upsert(sessionFixture('chat:2')));
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:2',
      'chat:1',
    ]);
    act(() => result.current.remove(sessionFixture('chat:1')));
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:2',
    ]);
  });
});
```

Create `apps/web/src/hooks/useSessionMessages.test.tsx`:

```tsx
import type { SessionMessage } from '@animaOS-SWARM/sdk';
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../lib/daemon-api';
import {
  SESSION_MESSAGES_POLL_MS,
  mergeNewest,
  useSessionMessages,
} from './useSessionMessages';

const nativeSetTimeout = window.setTimeout.bind(window);

function message(id: string, createdAtMs: number): SessionMessage {
  return {
    id,
    role: 'assistant',
    text: id,
    attachments: [],
    metadata: {},
    createdAtMs,
  };
}

function capturePolls() {
  const polls: (() => void)[] = [];
  vi.spyOn(window, 'setTimeout').mockImplementation(((
    handler: TimerHandler,
    timeout?: number,
  ) => {
    if (typeof handler === 'function' && timeout === SESSION_MESSAGES_POLL_MS) {
      polls.push(handler as () => void);
      return polls.length;
    }
    return nativeSetTimeout(handler, timeout);
  }) as typeof window.setTimeout);
  return polls;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSessionMessages', () => {
  it('loads the newest page, then older pages without duplicates', async () => {
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockImplementation(async (_agentId, _sessionId, options = {}) =>
        options.before
          ? { messages: [message('m1', 1), message('m2', 2)], nextBefore: null }
          : {
              messages: [message('m3', 3), message('m4', 4)],
              nextBefore: 'm3',
            },
      );
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:1'),
    );

    await waitFor(() =>
      expect(result.current.messages.map((item) => item.id)).toEqual([
        'm3',
        'm4',
      ]),
    );
    expect(result.current.hasOlder).toBe(true);
    expect(pages).toHaveBeenCalledWith('agent-main', 'chat:1', { limit: 50 });

    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.messages.map((item) => item.id)).toEqual([
      'm1',
      'm2',
      'm3',
      'm4',
    ]);
    expect(result.current.hasOlder).toBe(false);
    expect(pages).toHaveBeenLastCalledWith('agent-main', 'chat:1', {
      before: 'm3',
      limit: 50,
    });
  });

  it('merges the polled newest page and reloads for a new refresh key', async () => {
    const polls = capturePolls();
    let newest = [message('m1', 1)];
    vi.spyOn(daemon, 'sessionMessages').mockImplementation(async () => ({
      messages: newest,
      nextBefore: null,
    }));
    const { result, rerender } = renderHook(
      ({ refreshKey }) =>
        useSessionMessages('agent-main', 'chat:1', refreshKey),
      { initialProps: { refreshKey: 0 } },
    );
    const ids = () => result.current.messages.map((item) => item.id);

    await waitFor(() => expect(ids()).toEqual(['m1']));
    newest = [message('m1', 1), message('m2', 2)];
    await waitFor(() => expect(polls.length).toBeGreaterThan(0));
    await act(async () => {
      polls[polls.length - 1]();
    });
    await waitFor(() => expect(ids()).toEqual(['m1', 'm2']));

    newest = [message('m1', 1), message('m2', 2), message('m3', 3)];
    rerender({ refreshKey: 1 });
    await waitFor(() => expect(ids()).toEqual(['m1', 'm2', 'm3']));
  });

  it('reports a deleted session and stops polling it', async () => {
    const polls = capturePolls();
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockRejectedValue(
        Object.assign(new Error('not found'), { status: 404 }),
      );
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:gone'),
    );

    await waitFor(() => expect(result.current.missing).toBe(true));
    expect(polls).toHaveLength(0);
    expect(pages).toHaveBeenCalledTimes(1);
  });

  it('keeps loaded older messages when the newest page moves on', () => {
    expect(
      mergeNewest(
        [message('m1', 1), message('m2', 2), message('m3', 3)],
        [message('m2', 2), message('m3', 3), message('m4', 4)],
      ).map((item) => item.id),
    ).toEqual(['m1', 'm2', 'm3', 'm4']);
    expect(mergeNewest([message('m1', 1)], [])).toEqual([]);
  });
});
```

Append to `apps/web/src/lib/daemon-api.test.ts` (add `toChatMessage` to the import from `./daemon-api`):

```ts
describe('daemon session requests', () => {
  it('reads and changes sessions through the SDK routes', async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockImplementation(async (input) => {
        const url = String(input);
        if (url.endsWith('/export'))
          return new Response('# Plans\n', {
            status: 200,
            headers: { 'content-type': 'text/markdown' },
          });
        const body = url.includes('/messages')
          ? { messages: [], nextBefore: null }
          : {
              sessions: [],
              nextCursor: null,
              session: { id: 'chat:1' },
              deleted: true,
            };
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      });
    vi.stubGlobal('fetch', fetchMock);

    await daemon.listSessions('agent 1', {
      includeHelpers: true,
      archived: false,
      limit: 200,
    });
    await daemon.createSession('agent 1');
    await daemon.updateSession('agent 1', 'chat:1', { lastReadAtMs: 4 });
    await daemon.sessionMessages('agent 1', 'chat:1', {
      before: 'm1',
      limit: 50,
    });
    await daemon.deleteSession('agent 1', 'chat:1');
    expect(await daemon.exportSession('agent 1', 'chat:1')).toBe('# Plans\n');

    expect(
      fetchMock.mock.calls.map(
        ([url, init]) => `${init?.method ?? 'GET'} ${String(url)}`,
      ),
    ).toEqual([
      'GET /api/agents/agent%201/sessions?archived=false&limit=200&includeHelpers=true',
      'POST /api/agents/agent%201/sessions',
      'PATCH /api/agents/agent%201/sessions/chat%3A1',
      'GET /api/agents/agent%201/sessions/chat%3A1/messages?before=m1&limit=50',
      'DELETE /api/agents/agent%201/sessions/chat%3A1',
      'GET /api/agents/agent%201/sessions/chat%3A1/export',
    ]);
  });

  it('adapts session messages and folds check-in prompts into a system line', () => {
    expect(
      toChatMessage({
        id: 'c1',
        role: 'user',
        text: 'Check goals\n\n(This is a scheduled check-in. If you have nothing worth saying right now, reply with exactly CHECKIN_OK and nothing else.)',
        attachments: [],
        metadata: { kind: 'checkin', id: 's1' },
        createdAtMs: 5,
      }),
    ).toEqual({
      id: 'c1',
      role: 'System',
      content: { text: 'Check goals', metadata: { kind: 'checkin', id: 's1' } },
      created_at_ms: 5,
    });
    expect(
      toChatMessage({
        id: 't1',
        role: 'tool',
        text: '{}',
        attachments: [],
        metadata: {},
        createdAtMs: 6,
      }).role,
    ).toBe('Tool');
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run (from `apps/web`): `bun x vitest run src/lib/hash-route.test.ts src/lib/session-groups.test.ts src/hooks/useCompanionSessions.test.tsx src/hooks/useSessionMessages.test.tsx src/lib/daemon-api.test.ts`
Expected: FAIL — `Failed to resolve import "./hash-route"`, `"./session-groups"`, `"./useCompanionSessions"`, `"./useSessionMessages"`, and `daemon.listSessions is not a function`.

- [ ] **Step 3: Implement**

Create `apps/web/src/lib/hash-route.ts`:

```ts
import { useCallback, useEffect, useState } from 'react';

/** The pages of spec §15.1; later milestones build the ones this release hides. */
export const HASH_PAGES = [
  'approvals',
  'automations',
  'memory',
  'skills',
  'work',
  'files',
  'connectors',
  'usage',
  'logs',
  'health',
  'capabilities',
] as const;
export type HashPage = (typeof HASH_PAGES)[number];

export type HashRoute =
  | { kind: 'home' }
  | { kind: 'session'; sessionId: string }
  | { kind: 'page'; page: HashPage };

const SESSION_ID = /^[A-Za-z0-9._:-]{1,200}$/;

/** `#/s/<sessionId>` or `#/<page>`; anything else is a new chat. */
export function parseHashRoute(hash: string): HashRoute {
  const path = hash.startsWith('#') ? hash.slice(1) : hash;
  if (path.startsWith('/s/')) {
    try {
      const sessionId = decodeURIComponent(path.slice(3));
      if (SESSION_ID.test(sessionId)) return { kind: 'session', sessionId };
    } catch {
      // A malformed escape opens a new chat.
    }
    return { kind: 'home' };
  }
  const page = path.replace(/^\/+/, '');
  return (HASH_PAGES as readonly string[]).includes(page)
    ? { kind: 'page', page: page as HashPage }
    : { kind: 'home' };
}

export function formatHashRoute(route: HashRoute): string {
  switch (route.kind) {
    case 'home':
      return '#/';
    case 'session':
      return `#/s/${encodeURIComponent(route.sessionId)}`;
    case 'page':
      return `#/${route.page}`;
  }
}

export function sameRoute(left: HashRoute, right: HashRoute): boolean {
  return formatHashRoute(left) === formatHashRoute(right);
}

export type Navigate = (
  route: HashRoute,
  options?: { replace?: boolean },
) => void;

/** The current hash route; a reload restores it and Back/Forward follow it. */
export function useHashRoute(): [HashRoute, Navigate] {
  const [route, setRoute] = useState<HashRoute>(() =>
    parseHashRoute(window.location.hash),
  );
  useEffect(() => {
    const update = () => setRoute(parseHashRoute(window.location.hash));
    window.addEventListener('hashchange', update);
    window.addEventListener('popstate', update);
    return () => {
      window.removeEventListener('hashchange', update);
      window.removeEventListener('popstate', update);
    };
  }, []);
  const navigate = useCallback<Navigate>((next, options = {}) => {
    const hash = formatHashRoute(next);
    if (options.replace) {
      window.history.replaceState(window.history.state, '', hash);
    } else if (window.location.hash !== hash) {
      window.history.pushState(window.history.state, '', hash);
    }
    setRoute(parseHashRoute(hash));
  }, []);
  return [route, navigate];
}
```

Create `apps/web/src/lib/session-groups.ts`:

```ts
import type { Session, SessionKind } from '@animaOS-SWARM/sdk';

export const SESSION_KIND_ORDER: readonly SessionKind[] = [
  'chat',
  'telegram',
  'checkin',
  'job',
  'helper',
];

export const SESSION_KIND_LABELS: Record<SessionKind, string> = {
  chat: 'Chat',
  telegram: 'Telegram',
  checkin: 'Check-in',
  job: 'Job',
  helper: 'Helper',
};

export const SESSION_KIND_FILTER_LABELS: Record<SessionKind, string> = {
  chat: 'Chats',
  telegram: 'Telegram',
  checkin: 'Check-ins',
  job: 'Jobs',
  helper: 'Helpers',
};

export type SessionGroupLabel =
  | 'Today'
  | 'Yesterday'
  | 'Previous 7 days'
  | 'Older';

export interface SessionNode {
  session: Session;
  helpers: Session[];
}

export interface SessionGroup {
  label: SessionGroupLabel;
  nodes: SessionNode[];
}

const DAY_MS = 24 * 60 * 60 * 1000;
const GROUP_ORDER: readonly SessionGroupLabel[] = [
  'Today',
  'Yesterday',
  'Previous 7 days',
  'Older',
];

/** Sessions are keyed by agent and id; helper sessions belong to other agents. */
export function sessionKey(session: Pick<Session, 'agentId' | 'id'>): string {
  return `${session.agentId}\u0000${session.id}`;
}

/** The kinds present, in sidebar order (spec §15.1: only kinds that exist). */
export function presentKinds(sessions: readonly Session[]): SessionKind[] {
  const kinds = new Set(sessions.map((session) => session.kind));
  return SESSION_KIND_ORDER.filter((kind) => kinds.has(kind));
}

function groupLabel(activityMs: number, now: Date): SessionGroupLabel {
  const today = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
  ).getTime();
  if (activityMs >= today) return 'Today';
  if (activityMs >= today - DAY_MS) return 'Yesterday';
  if (activityMs >= today - 7 * DAY_MS) return 'Previous 7 days';
  return 'Older';
}

/**
 * Top-level sessions by last activity (local days). With `nestHelpers`, a
 * helper session sits under the listed session it came from; one whose parent
 * is not listed stays at the top level.
 */
export function groupSessions(
  sessions: readonly Session[],
  now: Date = new Date(),
  nestHelpers = true,
): SessionGroup[] {
  const listed = new Map(
    sessions.map((session) => [sessionKey(session), session]),
  );
  const children = new Map<string, Session[]>();
  const roots: Session[] = [];
  for (const session of sessions) {
    const parentKey =
      nestHelpers &&
      session.kind === 'helper' &&
      session.parentAgentId &&
      session.parentSessionId
        ? sessionKey({
            agentId: session.parentAgentId,
            id: session.parentSessionId,
          })
        : null;
    const parent = parentKey ? listed.get(parentKey) : undefined;
    if (parentKey && parent && parent.kind !== 'helper') {
      children.set(parentKey, [...(children.get(parentKey) ?? []), session]);
    } else {
      roots.push(session);
    }
  }
  const groups = new Map<SessionGroupLabel, SessionNode[]>();
  for (const session of roots) {
    const label = groupLabel(session.lastActivityAtMs, now);
    groups.set(label, [
      ...(groups.get(label) ?? []),
      { session, helpers: children.get(sessionKey(session)) ?? [] },
    ]);
  }
  return GROUP_ORDER.filter((label) => groups.has(label)).map((label) => ({
    label,
    nodes: groups.get(label) ?? [],
  }));
}

/** A download name from a title, matching the daemon's export file name. */
export function exportFileName(title: string): string {
  const stem = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+/, '')
    .slice(0, 60)
    .replace(/-+$/, '');
  return `${stem || 'session'}.md`;
}
```

Create `apps/web/src/hooks/useCompanionSessions.ts`:

```ts
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import type { Session } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { sessionKey } from '../lib/session-groups';

/** The sidebar re-reads its sessions this often (the agent poll uses 5 s). */
export const SESSION_LIST_POLL_MS = 10_000;
/** Sessions loaded at once; search reaches older ones. */
export const SESSION_LIST_LIMIT = 200;

export interface CompanionSessionFilters {
  archived: boolean;
  query: string;
}

/** The companion's sessions plus its helpers' (spec §3.3 `includeHelpers`). */
export function useCompanionSessions(
  agentId: string | null,
  filters: CompanionSessionFilters,
) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const query = filters.query.trim();
  const { archived } = filters;

  useLayoutEffect(() => {
    generation.current += 1;
    setSessions([]);
    setError(null);
  }, [agentId]);

  const refresh = useCallback(async () => {
    if (!agentId) return;
    const request = ++generation.current;
    setLoading(true);
    try {
      const page = await daemon.listSessions(agentId, {
        includeHelpers: true,
        archived,
        limit: SESSION_LIST_LIMIT,
        ...(query ? { q: query } : {}),
      });
      if (request !== generation.current) return;
      setSessions(page.sessions);
      setError(null);
    } catch (caught) {
      if (request !== generation.current) return;
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }, [agentId, archived, query]);

  useEffect(() => {
    if (!agentId) return;
    let active = true;
    let timer: number | undefined;
    const schedule = () => {
      if (!active) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        void refresh().finally(schedule);
      }, SESSION_LIST_POLL_MS);
    };
    void refresh().finally(schedule);
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentId, refresh]);

  const upsert = useCallback((session: Session) => {
    setSessions((current) => [
      session,
      ...current.filter((item) => sessionKey(item) !== sessionKey(session)),
    ]);
  }, []);

  const remove = useCallback((session: Pick<Session, 'agentId' | 'id'>) => {
    setSessions((current) =>
      current.filter((item) => sessionKey(item) !== sessionKey(session)),
    );
  }, []);

  return { sessions, loading, error, refresh, upsert, remove };
}
```

Create `apps/web/src/hooks/useSessionMessages.ts`:

```ts
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import type { SessionMessage } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';

/** Messages per page (the daemon default). */
export const SESSION_MESSAGE_PAGE = 50;
/** The open session re-reads its newest page this often until M3's stream. */
export const SESSION_MESSAGES_POLL_MS = 3_000;

/** The newest page replaces the tail; older loaded messages stay. */
export function mergeNewest(
  current: readonly SessionMessage[],
  page: readonly SessionMessage[],
): SessionMessage[] {
  if (page.length === 0) return [];
  const start = current.findIndex((message) => message.id === page[0].id);
  const older =
    start >= 0
      ? current.slice(0, start)
      : current.filter((message) => message.createdAtMs < page[0].createdAtMs);
  return [...older, ...page];
}

function httpStatus(error: unknown): unknown {
  return typeof error === 'object' && error !== null && 'status' in error
    ? error.status
    : undefined;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** One session's messages: the newest page, older pages on demand, and a poll. */
export function useSessionMessages(
  agentId: string | null,
  sessionId: string | null,
  refreshKey = 0,
) {
  const [messages, setMessages] = useState<SessionMessage[]>([]);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const loadedOlder = useRef(false);
  const loadingOlderRef = useRef(false);
  const missingRef = useRef(false);

  useLayoutEffect(() => {
    generation.current += 1;
    loadedOlder.current = false;
    loadingOlderRef.current = false;
    missingRef.current = false;
    setMessages([]);
    setNextBefore(null);
    setLoadingOlder(false);
    setMissing(false);
    setError(null);
  }, [agentId, sessionId]);

  const refresh = useCallback(async () => {
    if (!agentId || !sessionId) return;
    const request = generation.current;
    try {
      const page = await daemon.sessionMessages(agentId, sessionId, {
        limit: SESSION_MESSAGE_PAGE,
      });
      if (request !== generation.current) return;
      setMessages((current) => mergeNewest(current, page.messages));
      if (!loadedOlder.current) setNextBefore(page.nextBefore);
      missingRef.current = false;
      setMissing(false);
      setError(null);
    } catch (caught) {
      if (request !== generation.current) return;
      if (httpStatus(caught) === 404) {
        missingRef.current = true;
        setMissing(true);
      } else {
        setError(errorText(caught));
      }
    }
  }, [agentId, sessionId]);

  const loadOlder = useCallback(async () => {
    if (!agentId || !sessionId || !nextBefore || loadingOlderRef.current)
      return;
    const request = generation.current;
    loadingOlderRef.current = true;
    setLoadingOlder(true);
    try {
      const page = await daemon.sessionMessages(agentId, sessionId, {
        before: nextBefore,
        limit: SESSION_MESSAGE_PAGE,
      });
      if (request !== generation.current) return;
      loadedOlder.current = true;
      setMessages((current) => {
        const known = new Set(current.map((message) => message.id));
        return [
          ...page.messages.filter((message) => !known.has(message.id)),
          ...current,
        ];
      });
      setNextBefore(page.nextBefore);
    } catch (caught) {
      if (request === generation.current) setError(errorText(caught));
    } finally {
      if (request === generation.current) {
        loadingOlderRef.current = false;
        setLoadingOlder(false);
      }
    }
  }, [agentId, sessionId, nextBefore]);

  useEffect(() => {
    if (!agentId || !sessionId) return;
    let active = true;
    let timer: number | undefined;
    const schedule = () => {
      if (!active || missingRef.current) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        void refresh().finally(schedule);
      }, SESSION_MESSAGES_POLL_MS);
    };
    void refresh().finally(schedule);
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [agentId, sessionId, refresh, refreshKey]);

  return {
    messages,
    hasOlder: nextBefore !== null,
    loadingOlder,
    loadOlder,
    missing,
    error,
    refresh,
  };
}
```

In `apps/web/src/lib/daemon-api.ts`:

- add `type Session`, `type SessionListOptions`, `type SessionMessage`, `type SessionMessageOptions`, and `type SessionUpdateInput` to the `import { createDaemonClient, … } from '@animaOS-SWARM/sdk';` list;
- add to the `daemon` object, after `goalJobs`:

```ts
  listSessions: (agentId: string, options: SessionListOptions = {}) =>
    setupClient.sessions.list(agentId, options),
  createSession: (agentId: string, input: { title?: string } = {}): Promise<Session> =>
    setupClient.sessions.create(agentId, input),
  updateSession: (agentId: string, sessionId: string, patch: SessionUpdateInput) =>
    setupClient.sessions.update(agentId, sessionId, patch),
  deleteSession: (agentId: string, sessionId: string) =>
    setupClient.sessions.remove(agentId, sessionId),
  sessionMessages: (
    agentId: string,
    sessionId: string,
    options: SessionMessageOptions = {},
  ) => setupClient.sessions.messages(agentId, sessionId, options),
  exportSession: (agentId: string, sessionId: string) =>
    setupClient.sessions.exportMarkdown(agentId, sessionId),
```

- append after `toAgentDetail`:

```ts
const SESSION_ROLES = {
  user: 'User',
  assistant: 'Assistant',
  system: 'System',
  tool: 'Tool',
} as const satisfies Record<SessionMessage['role'], ChatMessage['role']>;
const CHECKIN_SUFFIX = /\n\n\(This is a scheduled check-in\.[\s\S]*\)\s*$/;

/** A session-route message as the chat components render it. A check-in
 *  prompt becomes a system line without the scheduler's instructions. */
export function toChatMessage(message: SessionMessage): ChatMessage {
  const checkin =
    message.role === 'user' && message.metadata.kind === 'checkin';
  return {
    id: message.id,
    role: checkin ? 'System' : SESSION_ROLES[message.role],
    content: {
      text: checkin ? message.text.replace(CHECKIN_SUFFIX, '') : message.text,
      metadata: message.metadata,
    },
    created_at_ms: message.createdAtMs,
  };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run (from `apps/web`): `bun x vitest run src/lib/hash-route.test.ts src/lib/session-groups.test.ts src/hooks/useCompanionSessions.test.tsx src/hooks/useSessionMessages.test.tsx src/lib/daemon-api.test.ts src/visual-tokens.test.ts`
Expected: PASS (the new files contain no color tokens, so the visual contract still holds).

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/lib/hash-route.ts apps/web/src/lib/hash-route.test.ts apps/web/src/lib/session-groups.ts apps/web/src/lib/session-groups.test.ts apps/web/src/test/sessions.ts apps/web/src/hooks/useCompanionSessions.ts apps/web/src/hooks/useCompanionSessions.test.tsx apps/web/src/hooks/useSessionMessages.ts apps/web/src/hooks/useSessionMessages.test.tsx apps/web/src/lib/daemon-api.ts apps/web/src/lib/daemon-api.test.ts
git commit -m "feat(web): add hash routes, session grouping, and session data hooks"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. Before running web tests directly, make sure the SDK dist is current (`bun x nx run @animaOS-SWARM/sdk:build`), or run them through `bun x nx test @animaOS-SWARM/web`.
2. `mergeNewest`: when the newest page's first message is not already in the list (a gap), reset `nextBefore` from the page or reload the session so no permanent gap remains. Test.

---

### Task 16: Sessions sidebar and session view components

**Files:**

- Create: `apps/web/src/components/sessions/SessionSidebar.tsx`, `apps/web/src/components/sessions/SessionSidebar.test.tsx`, `apps/web/src/components/sessions/SessionView.tsx`, `apps/web/src/components/sessions/SessionView.test.tsx`, `apps/web/src/sessions.css`
- Modify: `apps/web/src/components/ChatScreen.tsx` (optional `MessageList` and `Composer` props), `apps/web/src/styles.css` (import)

**Interfaces:**

- Consumes: Task 14's `Session`, `SessionKind`; Task 15's `SESSION_KIND_LABELS`, `SESSION_KIND_FILTER_LABELS`, `groupSessions`, `presentKinds`, `sessionKey`, `sessionFixture`.
- Produces:
  - `MessageList` gains optional `hasOlder`, `loadingOlder`, `onLoadOlder` (a "Load older messages" button, loading on scroll to the top, and a kept reading position when older messages are prepended) and `emptyState` (replaces the welcome screen); `Composer` gains optional `label` (the textarea's name and placeholder, default `Message <agent>`). Existing callers are unchanged.
  - `SessionSidebar` (props `SessionSidebarProps { sessions, activeKey, query, onQueryChange, showArchived, onShowArchivedChange, error?, now?, onOpen(session), onRename(session, title) -> Promise<boolean>, onArchive(session, archived) -> Promise<void>, onExport(session) -> Promise<void>, onDelete(session) -> Promise<void> }`), `SESSION_SEARCH_DEBOUNCE_MS` (250): a `nav` named "Sessions" with a "Search sessions" search box, a "Session kinds" chip group (All plus the kinds present, shown when more than one kind exists), day groups (`role="group"` named Today, Yesterday, Previous 7 days, Older), helper sessions nested in a list named `Helpers of <title>`, rows named `<title>[, working][, unread]` with `aria-current="page"` on the active row, ArrowUp/ArrowDown between rows, a row menu (`Actions for <title>`: Rename, Archive/Unarchive, Export Markdown, Delete with a confirmation that memories are kept) following the capabilities, and a "Show archived"/"Hide archived" toggle.
  - `SessionView` (props `SessionViewProps { agent, session, messages, hasOlder, loadingOlder, onLoadOlder, missing, telegramAvailable, scrollerRef, onSuggestion, composer: SessionComposerState, onNewChat, onOpenWork, onRename(title) -> Promise<boolean>, onToggleArchived, onExport, notice? }`), `SessionComposerState { draft, setDraft, sending, disabled, offline, onSend, error, onDismissError, recovery? }`, and `sessionFooter(session, telegramAvailable)`: the header (title, kind badge, Rename, Archive/Unarchive, Export), the message list, and either the composer ("Reply on Telegram" for Telegram sessions) or a read-only note (check-ins until M3's reply route, jobs with "Open Work", helpers). A deleted session shows "This session was deleted." with "Start a new chat". The composer keeps its element across a new chat becoming a session.
  - `sessions.css` (imported from `styles.css`) with the `session-*` classes, using only palette tokens.

- [ ] **Step 1: Write the failing tests**

Create `apps/web/src/components/sessions/SessionSidebar.test.tsx`:

```tsx
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { sessionFixture } from '../../test/sessions';
import { SessionSidebar, type SessionSidebarProps } from './SessionSidebar';

const NOW = new Date(2026, 8, 24, 12, 0, 0);
const HOUR = 60 * 60 * 1000;
const readOnly = {
  send: false,
  steer: false,
  stop: true,
  rename: false,
  archive: true,
  delete: false,
  compact: false,
  export: true,
};

function renderSidebar(overrides: Partial<SessionSidebarProps> = {}) {
  const props: SessionSidebarProps = {
    sessions: [],
    activeKey: null,
    query: '',
    onQueryChange: vi.fn(),
    showArchived: false,
    onShowArchivedChange: vi.fn(),
    now: NOW,
    onOpen: vi.fn(),
    onRename: vi.fn().mockResolvedValue(true),
    onArchive: vi.fn().mockResolvedValue(undefined),
    onExport: vi.fn().mockResolvedValue(undefined),
    onDelete: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
  render(<SessionSidebar {...props} />);
  return props;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SessionSidebar', () => {
  it('groups by day, nests helpers, and marks unread, working, and current rows', async () => {
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      unread: true,
      lastActivityAtMs: NOW.getTime() - HOUR,
    });
    const helper = sessionFixture('room-9', {
      agentId: 'helper-1',
      kind: 'helper',
      title: 'Draft a plan',
      parentAgentId: 'agent-main',
      parentSessionId: 'chat:plans',
      activeRuns: 1,
      capabilities: readOnly,
      lastActivityAtMs: NOW.getTime() - 2 * HOUR,
    });
    const old = sessionFixture('chat:old', {
      title: 'Old notes',
      lastActivityAtMs: NOW.getTime() - 40 * 24 * HOUR,
    });
    const props = renderSidebar({
      sessions: [plans, helper, old],
      activeKey: 'agent-main\u0000chat:plans',
    });

    const today = screen.getByRole('group', { name: 'Today' });
    expect(
      within(today).getByRole('button', { name: 'Plans, unread' }),
    ).toHaveAttribute('aria-current', 'page');
    expect(
      within(
        within(today).getByRole('list', { name: 'Helpers of Plans' }),
      ).getByRole('button', { name: 'Draft a plan, working' }),
    ).toBeVisible();
    expect(
      within(screen.getByRole('group', { name: 'Older' })).getByRole('button', {
        name: 'Old notes',
      }),
    ).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Old notes' }));
    expect(props.onOpen).toHaveBeenCalledWith(old);
  });

  it('filters by the kinds present and follows each kind’s capabilities', async () => {
    const user = userEvent.setup();
    renderSidebar({
      sessions: [
        sessionFixture('chat:1', {
          title: 'Chat one',
          lastActivityAtMs: NOW.getTime(),
        }),
        sessionFixture('job:1', {
          kind: 'job',
          title: 'Job · Report',
          capabilities: readOnly,
          lastActivityAtMs: NOW.getTime(),
        }),
      ],
    });

    const chips = screen.getByRole('group', { name: 'Session kinds' });
    expect(
      within(chips)
        .getAllByRole('button')
        .map((chip) => chip.textContent),
    ).toEqual(['All', 'Chats', 'Jobs']);
    await user.click(within(chips).getByRole('button', { name: 'Jobs' }));
    expect(
      screen.queryByRole('button', { name: 'Chat one' }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole('button', { name: 'Actions for Job · Report' }),
    );
    expect(
      screen.queryByRole('menuitem', { name: 'Rename' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('menuitem', { name: 'Delete' }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole('menuitem', { name: 'Export Markdown' }),
    ).toBeVisible();
  });

  it('waits for typing to settle before searching', async () => {
    const props = renderSidebar();
    fireEvent.change(
      screen.getByRole('searchbox', { name: 'Search sessions' }),
      {
        target: { value: 'budget' },
      },
    );
    expect(props.onQueryChange).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(props.onQueryChange).toHaveBeenCalledWith('budget'),
    );
  });

  it('renames, archives, exports, and deletes after confirming that memories are kept', async () => {
    const user = userEvent.setup();
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      lastActivityAtMs: NOW.getTime(),
    });
    const props = renderSidebar({ sessions: [plans] });
    const menu = () =>
      screen.getByRole('button', { name: 'Actions for Plans' });

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Rename' }));
    const input = screen.getByRole('textbox', { name: 'Rename Plans' });
    await user.clear(input);
    await user.type(input, 'Offsite{Enter}');
    expect(props.onRename).toHaveBeenCalledWith(plans, 'Offsite');

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Archive' }));
    expect(props.onArchive).toHaveBeenCalledWith(plans, true);

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Export Markdown' }));
    expect(props.onExport).toHaveBeenCalledWith(plans);

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
    expect(props.onDelete).not.toHaveBeenCalled();
    expect(screen.getByText(/memories are kept/i)).toBeVisible();
    await user.click(screen.getByRole('menuitem', { name: 'Delete session' }));
    expect(props.onDelete).toHaveBeenCalledWith(plans);
  });

  it('moves between rows with the arrow keys and asks for archived sessions', async () => {
    const props = renderSidebar({
      sessions: ['A', 'B', 'C'].map((title, index) =>
        sessionFixture(`chat:${title}`, {
          title,
          lastActivityAtMs: NOW.getTime() - index * 1_000,
        }),
      ),
    });

    screen.getByRole('button', { name: 'A' }).focus();
    fireEvent.keyDown(screen.getByRole('button', { name: 'A' }), {
      key: 'ArrowDown',
    });
    expect(screen.getByRole('button', { name: 'B' })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole('button', { name: 'B' }), {
      key: 'ArrowUp',
    });
    expect(screen.getByRole('button', { name: 'A' })).toHaveFocus();

    await userEvent.click(
      screen.getByRole('button', { name: 'Show archived' }),
    );
    expect(props.onShowArchivedChange).toHaveBeenCalledWith(true);
  });
});
```

Create `apps/web/src/components/sessions/SessionView.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef } from 'react';
import { describe, expect, it, vi } from 'vitest';

import type { AgentDetail } from '../../lib/types';
import { sessionFixture } from '../../test/sessions';
import { SessionView, type SessionViewProps } from './SessionView';

const agent: AgentDetail = {
  id: 'agent-main',
  name: 'Nova',
  provider: 'openai',
  model: 'gpt-5.4',
  toolNames: [],
  created_at_ms: 1,
  status: 'Idle',
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  messages: [],
};
const readOnly = {
  send: false,
  steer: false,
  stop: true,
  rename: false,
  archive: true,
  delete: false,
  compact: false,
  export: true,
};

function renderView(overrides: Partial<SessionViewProps> = {}) {
  const props: SessionViewProps = {
    agent,
    session: null,
    messages: [],
    hasOlder: false,
    loadingOlder: false,
    onLoadOlder: vi.fn(),
    missing: false,
    telegramAvailable: true,
    scrollerRef: createRef<HTMLDivElement>(),
    onSuggestion: vi.fn(),
    composer: {
      draft: '',
      setDraft: vi.fn(),
      sending: false,
      disabled: false,
      offline: false,
      onSend: vi.fn(),
      error: null,
      onDismissError: vi.fn(),
    },
    onNewChat: vi.fn(),
    onOpenWork: vi.fn(),
    onRename: vi.fn().mockResolvedValue(true),
    onToggleArchived: vi.fn(),
    onExport: vi.fn(),
    ...overrides,
  };
  render(<SessionView {...props} />);
  return props;
}

describe('SessionView', () => {
  it('opens a new chat on the welcome screen with the companion composer', () => {
    renderView();
    expect(
      screen.getByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    expect(screen.getByPlaceholderText('Message Nova…')).toBeVisible();
  });

  it('shows a chat with its header, older history, rename, archive, and export', async () => {
    const user = userEvent.setup();
    const props = renderView({
      session: sessionFixture('chat:plans', { title: 'Plans' }),
      messages: [
        {
          id: 'm1',
          role: 'Assistant',
          content: { text: 'Here is the plan' },
          created_at_ms: 1,
        },
      ],
      hasOlder: true,
    });

    expect(screen.getByRole('heading', { name: 'Plans' })).toBeVisible();
    expect(
      screen.getByText('Chat', { selector: '.session-kind-badge' }),
    ).toBeVisible();
    await user.click(
      screen.getByRole('button', { name: 'Load older messages' }),
    );
    expect(props.onLoadOlder).toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    const title = screen.getByRole('textbox', { name: 'Session title' });
    await user.clear(title);
    await user.type(title, 'Offsite{Enter}');
    expect(props.onRename).toHaveBeenCalledWith('Offsite');
    await user.click(screen.getByRole('button', { name: 'Archive' }));
    expect(props.onToggleArchived).toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Export' }));
    expect(props.onExport).toHaveBeenCalled();
  });

  it('replies in a Telegram session through the Telegram composer', () => {
    renderView({
      session: sessionFixture('telegram:tg-1', {
        kind: 'telegram',
        title: 'Telegram · @nova_bot',
      }),
    });
    expect(screen.getByPlaceholderText('Reply on Telegram…')).toBeVisible();
  });

  it('shows a read-only note instead of a composer for jobs', async () => {
    const props = renderView({
      session: sessionFixture('job:1', {
        kind: 'job',
        title: 'Job · Report',
        capabilities: readOnly,
      }),
    });

    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.getByRole('note')).toHaveTextContent(
      'Job sessions are read-only',
    );
    expect(screen.getByText('No messages yet.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Open Work' }));
    expect(props.onOpenWork).toHaveBeenCalled();
  });

  it('offers a new chat when the session was deleted elsewhere', async () => {
    const props = renderView({
      session: sessionFixture('chat:gone'),
      missing: true,
    });
    expect(screen.getByText('This session was deleted.')).toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: 'Start a new chat' }),
    );
    expect(props.onNewChat).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run (from `apps/web`): `bun x vitest run src/components/sessions`
Expected: FAIL — `Failed to resolve import "./SessionSidebar"` and `"./SessionView"`.

- [ ] **Step 3: Extend the chat components**

In `apps/web/src/components/ChatScreen.tsx`:

- change the React import to `import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';`;
- replace the whole `export const MessageList = memo(function MessageList({ … }) { … });` with:

```tsx
export const MessageList = memo(function MessageList({
  agent,
  sending,
  scrollerRef,
  onSuggestion,
  hasOlder = false,
  loadingOlder = false,
  onLoadOlder,
  emptyState,
}: {
  agent: AgentDetail;
  sending: boolean;
  scrollerRef: React.RefObject<HTMLDivElement | null>;
  onSuggestion: (text: string) => void;
  /** Older history exists: show a control and load it on scroll to the top. */
  hasOlder?: boolean;
  loadingOlder?: boolean;
  onLoadOlder?: () => void;
  /** Replaces the welcome screen for sessions that are not new chats. */
  emptyState?: ReactNode;
}) {
  const [awayFromBottom, setAwayFromBottom] = useState(false);
  const [highlight, setHighlight] = useState<string | null>(null);
  const atBottom = useRef(true);
  const messageElements = useRef(new Map<string, HTMLDivElement>());
  const firstMessageId = agent.messages[0]?.id;
  const anchor = useRef<{ firstId: string | undefined; height: number }>({
    firstId: firstMessageId,
    height: 0,
  });
  const jumpToMessage = useCallback((id: string) => {
    atBottom.current = false;
    setAwayFromBottom(true);
    messageElements.current.get(id)?.scrollIntoView?.({ block: 'center' });
  }, []);
  const jumpToLatest = () => {
    const element = scrollerRef.current;
    if (element) element.scrollTop = element.scrollHeight;
    atBottom.current = true;
    setAwayFromBottom(false);
  };
  useLayoutEffect(() => {
    if (atBottom.current) {
      const element = scrollerRef.current;
      if (element) element.scrollTop = element.scrollHeight;
    }
  }, [agent.messages, sending, scrollerRef]);
  // Keep the reading position when older messages are prepended.
  useLayoutEffect(() => {
    const element = scrollerRef.current;
    if (!element) return;
    const previous = anchor.current;
    if (
      previous.firstId !== undefined &&
      previous.firstId !== firstMessageId &&
      !atBottom.current
    ) {
      element.scrollTop += element.scrollHeight - previous.height;
    }
    anchor.current = { firstId: firstMessageId, height: element.scrollHeight };
  }, [firstMessageId, agent.messages, scrollerRef]);

  return (
    <>
      {agent.messages.length > 0 && (
        <ConversationTools
          agent={agent}
          onJump={jumpToMessage}
          onHighlight={setHighlight}
        />
      )}
      <div className="studio-conversation-body">
        <div
          ref={scrollerRef}
          className="studio-message-scroller relative z-[1] min-h-0 flex-1 overflow-y-auto"
          aria-label={`Conversation with ${agent.name}`}
          onScroll={(event) => {
            const element = event.currentTarget;
            atBottom.current =
              element.scrollHeight - element.scrollTop - element.clientHeight <
              80;
            setAwayFromBottom(!atBottom.current);
            if (element.scrollTop < 40 && hasOlder && !loadingOlder) {
              onLoadOlder?.();
            }
          }}
        >
          {agent.messages.length === 0 && !sending ? (
            (emptyState ?? (
              <EmptyState agentName={agent.name} onPick={onSuggestion} />
            ))
          ) : (
            <div className="studio-messages mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6 sm:px-6">
              {hasOlder && onLoadOlder && (
                <button
                  type="button"
                  className="studio-tool-button session-load-older"
                  onClick={onLoadOlder}
                  disabled={loadingOlder}
                >
                  {loadingOlder
                    ? 'Loading older messages…'
                    : 'Load older messages'}
                </button>
              )}
              {agent.messages.map((m) => (
                <div
                  key={m.id}
                  ref={(element) => {
                    if (element) messageElements.current.set(m.id, element);
                    else messageElements.current.delete(m.id);
                  }}
                  data-search-match={highlight === m.id || undefined}
                  className="studio-message-anchor"
                >
                  <Bubble message={m} />
                </div>
              ))}
              {sending && <ThinkingIndicator name={agent.name} />}
            </div>
          )}
        </div>
        {awayFromBottom && (
          <button
            type="button"
            className="studio-jump-latest"
            onClick={jumpToLatest}
          >
            ↓ Jump to latest
          </button>
        )}
      </div>
    </>
  );
});
```

- in `Composer`, add `label,` after `agentName,` in the destructuring, add to its props type after `agentName: string;`:

```ts
  /** The textarea's name and placeholder; defaults to "Message <agent>". */
  label?: string;
```

add `const inputLabel = label ?? \`Message ${agentName}\`;` after `const taRef = useRef<HTMLTextAreaElement>(null);`, and change `aria-label={\`Message ${agentName}\`}` to `aria-label={inputLabel}` and `placeholder={\`Message ${agentName}…\`}` to `placeholder={\`${inputLabel}…\`}`.

(`function Bubble` through `const SUGGESTIONS` is unchanged, so `visual-tokens.test.ts` still finds `bg-panel-2/90` and no accent there.)

- [ ] **Step 4: Implement the sidebar, the view, and their styles**

Create `apps/web/src/components/sessions/SessionSidebar.tsx`:

```tsx
import { useEffect, useState, type KeyboardEvent } from 'react';
import type { Session, SessionKind } from '@animaOS-SWARM/sdk';

import {
  SESSION_KIND_FILTER_LABELS,
  groupSessions,
  presentKinds,
  sessionKey,
} from '../../lib/session-groups';

/** Typing settles this long before the list is searched. */
export const SESSION_SEARCH_DEBOUNCE_MS = 250;

export interface SessionSidebarProps {
  sessions: readonly Session[];
  /** `sessionKey` of the open session. */
  activeKey: string | null;
  query: string;
  onQueryChange: (query: string) => void;
  showArchived: boolean;
  onShowArchivedChange: (show: boolean) => void;
  error?: string | null;
  now?: Date;
  onOpen: (session: Session) => void;
  onRename: (session: Session, title: string) => Promise<boolean>;
  onArchive: (session: Session, archived: boolean) => Promise<void>;
  onExport: (session: Session) => Promise<void>;
  onDelete: (session: Session) => Promise<void>;
}

type RowActions = Pick<
  SessionSidebarProps,
  'onOpen' | 'onRename' | 'onArchive' | 'onExport' | 'onDelete'
>;

function rowLabel(session: Session): string {
  return [
    session.title,
    session.activeRuns > 0 ? 'working' : null,
    session.unread ? 'unread' : null,
  ]
    .filter(Boolean)
    .join(', ');
}

function SessionRow({
  session,
  active,
  actions,
}: {
  session: Session;
  active: boolean;
  actions: RowActions;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState(session.title);
  const closeMenu = () => {
    setMenuOpen(false);
    setConfirmDelete(false);
  };
  const cancelRename = () => {
    setRenaming(false);
    setTitle(session.title);
  };

  return (
    <div className="session-row">
      {renaming ? (
        <form
          className="flex min-w-0 flex-1 items-center gap-1"
          onSubmit={(event) => {
            event.preventDefault();
            void actions.onRename(session, title).then((saved) => {
              if (saved) setRenaming(false);
            });
          }}
        >
          <input
            className="session-rename"
            aria-label={`Rename ${session.title}`}
            value={title}
            maxLength={120}
            autoFocus
            onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') cancelRename();
            }}
          />
          <button type="submit" className="studio-tool-button">
            Save
          </button>
        </form>
      ) : (
        <button
          type="button"
          data-session-row
          className="session-row-button"
          aria-label={rowLabel(session)}
          aria-current={active ? 'page' : undefined}
          title={session.preview ?? session.title}
          onClick={() => actions.onOpen(session)}
        >
          {session.activeRuns > 0 && (
            <span className="session-pulse" aria-hidden />
          )}
          <span className="session-title">{session.title}</span>
          {session.unread && (
            <span className="session-unread-dot" aria-hidden />
          )}
        </button>
      )}
      <button
        type="button"
        className="session-row-actions"
        aria-label={`Actions for ${session.title}`}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={() => {
          setConfirmDelete(false);
          setMenuOpen((open) => !open);
        }}
      >
        ⋯
      </button>
      {menuOpen && (
        <div
          className="session-menu"
          role="menu"
          aria-label={`${session.title} actions`}
        >
          {confirmDelete ? (
            <>
              <p className="px-2 py-1 text-xs text-ink-2">
                Delete “{session.title}”? Its messages are removed; memories are
                kept.
              </p>
              <button
                type="button"
                role="menuitem"
                className="is-danger"
                onClick={() => {
                  closeMenu();
                  void actions.onDelete(session);
                }}
              >
                Delete session
              </button>
              <button type="button" role="menuitem" onClick={closeMenu}>
                Cancel
              </button>
            </>
          ) : (
            <>
              {session.capabilities.rename && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    closeMenu();
                    setTitle(session.title);
                    setRenaming(true);
                  }}
                >
                  Rename
                </button>
              )}
              {session.capabilities.archive && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    closeMenu();
                    void actions.onArchive(session, !session.archived);
                  }}
                >
                  {session.archived ? 'Unarchive' : 'Archive'}
                </button>
              )}
              {session.capabilities.export && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    closeMenu();
                    void actions.onExport(session);
                  }}
                >
                  Export Markdown
                </button>
              )}
              {session.capabilities.delete && (
                <button
                  type="button"
                  role="menuitem"
                  className="is-danger"
                  onClick={() => setConfirmDelete(true)}
                >
                  Delete
                </button>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

function moveBetweenRows(event: KeyboardEvent<HTMLDivElement>) {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
  const rows = Array.from(
    event.currentTarget.querySelectorAll<HTMLButtonElement>(
      '[data-session-row]',
    ),
  );
  const index = rows.findIndex((row) => row === document.activeElement);
  if (index === -1) return;
  event.preventDefault();
  const next = index + (event.key === 'ArrowDown' ? 1 : -1);
  rows[Math.min(rows.length - 1, Math.max(0, next))]?.focus();
}

/** The sessions list of the sidebar and the mobile drawer (spec §15.1). */
export function SessionSidebar({
  sessions,
  activeKey,
  query,
  onQueryChange,
  showArchived,
  onShowArchivedChange,
  error = null,
  now,
  ...actions
}: SessionSidebarProps) {
  const [text, setText] = useState(query);
  const [kind, setKind] = useState<SessionKind | null>(null);
  useEffect(() => {
    setText(query);
  }, [query]);
  useEffect(() => {
    if (text === query) return;
    const timer = window.setTimeout(
      () => onQueryChange(text),
      SESSION_SEARCH_DEBOUNCE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [text, query, onQueryChange]);

  const kinds = presentKinds(sessions);
  const activeKind = kind && kinds.includes(kind) ? kind : null;
  const visible = activeKind
    ? sessions.filter((session) => session.kind === activeKind)
    : sessions;
  const groups = groupSessions(visible, now ?? new Date(), activeKind === null);

  return (
    <nav className="session-sidebar" aria-label="Sessions">
      <input
        type="search"
        className="session-search"
        aria-label="Search sessions"
        placeholder="Search chats…"
        value={text}
        onChange={(event) => setText(event.target.value)}
      />
      {kinds.length > 1 && (
        <div className="session-chips" role="group" aria-label="Session kinds">
          <button
            type="button"
            className="session-chip"
            aria-pressed={activeKind === null}
            onClick={() => setKind(null)}
          >
            All
          </button>
          {kinds.map((item) => (
            <button
              key={item}
              type="button"
              className="session-chip"
              aria-pressed={activeKind === item}
              onClick={() => setKind(item)}
            >
              {SESSION_KIND_FILTER_LABELS[item]}
            </button>
          ))}
        </div>
      )}
      <div className="session-list" onKeyDown={moveBetweenRows}>
        {groups.length === 0 ? (
          <p className="px-2 py-3 text-xs text-ink-3">
            {query.trim()
              ? 'No sessions match.'
              : showArchived
                ? 'No archived sessions.'
                : 'No chats yet.'}
          </p>
        ) : (
          groups.map((group) => (
            <div key={group.label} role="group" aria-label={group.label}>
              <p className="session-group-label" aria-hidden>
                {group.label}
              </p>
              <ul>
                {group.nodes.map(({ session, helpers }) => (
                  <li key={sessionKey(session)}>
                    <SessionRow
                      session={session}
                      active={activeKey === sessionKey(session)}
                      actions={actions}
                    />
                    {helpers.length > 0 && (
                      <ul
                        className="session-children"
                        aria-label={`Helpers of ${session.title}`}
                      >
                        {helpers.map((helper) => (
                          <li key={sessionKey(helper)}>
                            <SessionRow
                              session={helper}
                              active={activeKey === sessionKey(helper)}
                              actions={actions}
                            />
                          </li>
                        ))}
                      </ul>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          ))
        )}
      </div>
      <button
        type="button"
        className="session-archived-toggle"
        aria-pressed={showArchived}
        onClick={() => onShowArchivedChange(!showArchived)}
      >
        {showArchived ? 'Hide archived' : 'Show archived'}
      </button>
      {error && (
        <p role="alert" className="px-2 text-xs text-danger">
          {error}
        </p>
      )}
    </nav>
  );
}
```

Create `apps/web/src/components/sessions/SessionView.tsx`:

```tsx
import {
  useEffect,
  useMemo,
  useState,
  type ReactNode,
  type RefObject,
} from 'react';
import type { Session } from '@animaOS-SWARM/sdk';

import { SESSION_KIND_LABELS } from '../../lib/session-groups';
import type { AgentDetail, ChatMessage } from '../../lib/types';
import { Composer, MessageList } from '../ChatScreen';
import { ghostBtnCls } from '../ui-bits';

export interface SessionComposerState {
  draft: string;
  setDraft: (value: string) => void;
  sending: boolean;
  disabled: boolean;
  offline: boolean;
  onSend: () => void;
  error: string | null;
  onDismissError: () => void;
  recovery?: {
    count: number;
    text: string;
    restore: () => void;
    dismiss: () => void;
  };
}

export interface SessionViewProps {
  agent: AgentDetail;
  /** null for a new chat that has no session yet. */
  session: Session | null;
  messages: ChatMessage[];
  hasOlder: boolean;
  loadingOlder: boolean;
  onLoadOlder: () => void;
  /** The session was deleted elsewhere. */
  missing: boolean;
  telegramAvailable: boolean;
  scrollerRef: RefObject<HTMLDivElement | null>;
  onSuggestion: (text: string) => void;
  composer: SessionComposerState;
  onNewChat: () => void;
  onOpenWork: () => void;
  onRename: (title: string) => Promise<boolean>;
  onToggleArchived: () => void;
  onExport: () => void;
  notice?: ReactNode;
}

export type SessionFooter =
  | { kind: 'composer'; label?: string }
  | { kind: 'note'; text: string; action?: 'new-chat' | 'work' };

/** What replaces the composer for each kind (spec §3.2, §15.2). */
export function sessionFooter(
  session: Session | null,
  telegramAvailable: boolean,
): SessionFooter {
  if (!session) return { kind: 'composer' };
  switch (session.kind) {
    case 'chat':
      return { kind: 'composer' };
    case 'telegram':
      return telegramAvailable
        ? { kind: 'composer', label: 'Reply on Telegram' }
        : {
            kind: 'note',
            text: 'This Telegram connection is not available. Reconnect it in Connectors to reply.',
          };
    case 'checkin':
      return {
        kind: 'note',
        text: 'Replying to a check-in is not available yet. Start a new chat to follow up.',
        action: 'new-chat',
      };
    case 'job':
      return {
        kind: 'note',
        text: 'Job sessions are read-only. Follow the job in Work.',
        action: 'work',
      };
    case 'helper':
      return { kind: 'note', text: 'Helper sessions are read-only.' };
  }
}

function SessionHeader({
  session,
  onRename,
  onToggleArchived,
  onExport,
}: {
  session: Session;
  onRename: (title: string) => Promise<boolean>;
  onToggleArchived: () => void;
  onExport: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(session.title);
  useEffect(() => {
    if (!editing) setTitle(session.title);
  }, [editing, session.title]);
  const cancel = () => {
    setEditing(false);
    setTitle(session.title);
  };

  return (
    <header className="session-view-header">
      {editing ? (
        <form
          className="flex min-w-0 flex-1 items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void onRename(title).then((saved) => {
              if (saved) setEditing(false);
            });
          }}
        >
          <input
            className="session-rename"
            aria-label="Session title"
            value={title}
            maxLength={120}
            autoFocus
            onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') cancel();
            }}
          />
          <button type="submit" className={ghostBtnCls}>
            Save
          </button>
          <button type="button" className={ghostBtnCls} onClick={cancel}>
            Cancel
          </button>
        </form>
      ) : (
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold text-ink">
          {session.title}
        </h2>
      )}
      <span className="session-kind-badge">
        {SESSION_KIND_LABELS[session.kind]}
      </span>
      {!editing && session.capabilities.rename && (
        <button
          type="button"
          className={ghostBtnCls}
          onClick={() => setEditing(true)}
        >
          Rename
        </button>
      )}
      {session.capabilities.archive && (
        <button
          type="button"
          className={ghostBtnCls}
          onClick={onToggleArchived}
        >
          {session.archived ? 'Unarchive' : 'Archive'}
        </button>
      )}
      {session.capabilities.export && (
        <button type="button" className={ghostBtnCls} onClick={onExport}>
          Export
        </button>
      )}
    </header>
  );
}

/** One session (or a new chat) on today's blocking run route (spec §15.2). */
export function SessionView({
  agent,
  session,
  messages,
  hasOlder,
  loadingOlder,
  onLoadOlder,
  missing,
  telegramAvailable,
  scrollerRef,
  onSuggestion,
  composer,
  onNewChat,
  onOpenWork,
  onRename,
  onToggleArchived,
  onExport,
  notice = null,
}: SessionViewProps) {
  const conversation = useMemo(
    () => ({ ...agent, messages }),
    [agent, messages],
  );
  if (missing) {
    return (
      <section
        className="flex h-full min-h-0 flex-col items-center justify-center gap-3 p-6 text-center"
        aria-label="Session"
      >
        <p className="text-sm text-ink-2">This session was deleted.</p>
        <button type="button" className={ghostBtnCls} onClick={onNewChat}>
          Start a new chat
        </button>
      </section>
    );
  }
  const footer = sessionFooter(session, telegramAvailable);
  return (
    <section
      className="flex h-full min-h-0 flex-col"
      aria-label={session?.title ?? 'New chat'}
    >
      {session ? (
        <SessionHeader
          session={session}
          onRename={onRename}
          onToggleArchived={onToggleArchived}
          onExport={onExport}
        />
      ) : null}
      {notice}
      <MessageList
        agent={conversation}
        sending={composer.sending || (session?.activeRuns ?? 0) > 0}
        scrollerRef={scrollerRef}
        onSuggestion={onSuggestion}
        hasOlder={hasOlder}
        loadingOlder={loadingOlder}
        onLoadOlder={onLoadOlder}
        emptyState={
          session && session.kind !== 'chat' ? (
            <p className="session-footer-note">No messages yet.</p>
          ) : undefined
        }
      />
      {footer.kind === 'composer' ? (
        <Composer
          agentName={agent.name}
          label={footer.label}
          draft={composer.draft}
          setDraft={composer.setDraft}
          sending={composer.sending}
          disabled={composer.disabled}
          offline={composer.offline}
          onSend={composer.onSend}
          error={composer.error}
          onDismissError={composer.onDismissError}
          recovery={composer.recovery}
        />
      ) : (
        <div className="session-footer-note" role="note">
          <p>{footer.text}</p>
          {footer.action === 'new-chat' && (
            <button type="button" className={ghostBtnCls} onClick={onNewChat}>
              Start a new chat
            </button>
          )}
          {footer.action === 'work' && (
            <button type="button" className={ghostBtnCls} onClick={onOpenWork}>
              Open Work
            </button>
          )}
        </div>
      )}
    </section>
  );
}
```

(`useMemo` runs before the early return, so the hook order is stable.)

Create `apps/web/src/sessions.css`:

```css
/* Sessions sidebar, drawer, and session view (spec §15.1–§15.2). */
.session-sidebar {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  gap: 8px;
  border-top: 1px solid var(--color-line);
  padding: 10px 12px 12px;
}
.session-search {
  width: 100%;
  border: 1px solid var(--color-line);
  border-radius: 10px;
  background: rgb(255 255 255 / 0.02);
  padding: 7px 10px;
  color: var(--color-ink);
  font-size: 12px;
}
.session-search::placeholder {
  color: var(--color-ink-3);
}
.session-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.session-chip,
.session-archived-toggle {
  border: 1px solid var(--color-line);
  border-radius: 999px;
  padding: 2px 9px;
  color: var(--color-ink-2);
  font-size: 11px;
}
.session-chip[aria-pressed='true'],
.session-archived-toggle[aria-pressed='true'] {
  border-color: rgb(var(--color-accent-rgb) / 0.6);
  background: rgb(var(--color-accent-rgb) / 0.12);
  color: var(--color-ink);
}
.session-archived-toggle {
  align-self: flex-start;
}
.session-list {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
}
.session-group-label {
  margin: 10px 4px 4px;
  color: var(--color-ink-3);
  font-size: 10px;
  letter-spacing: 0.14em;
  text-transform: uppercase;
}
.session-row {
  position: relative;
  display: flex;
  align-items: center;
  gap: 4px;
}
.session-row-button {
  display: flex;
  min-width: 0;
  flex: 1;
  align-items: center;
  gap: 8px;
  border-radius: 10px;
  padding: 7px 8px;
  color: var(--color-ink-2);
  font-size: 13px;
  text-align: left;
}
.session-row-button:hover {
  background: rgb(255 255 255 / 0.04);
  color: var(--color-ink);
}
.session-row-button[aria-current='page'] {
  background: #281e27;
  color: var(--color-ink);
}
.session-row-actions {
  flex-shrink: 0;
  border-radius: 8px;
  padding: 2px 6px;
  color: var(--color-ink-3);
}
.session-row-actions:hover,
.session-row-actions[aria-expanded='true'] {
  background: rgb(255 255 255 / 0.05);
  color: var(--color-ink);
}
.session-title {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.session-unread-dot,
.session-pulse {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 999px;
}
.session-unread-dot {
  background: var(--color-accent);
}
.session-pulse {
  background: var(--color-mint);
  animation: session-pulse 1.4s ease-in-out infinite;
}
@keyframes session-pulse {
  0%,
  100% {
    opacity: 0.35;
  }
  50% {
    opacity: 1;
  }
}
@media (prefers-reduced-motion: reduce) {
  .session-pulse {
    animation: none;
  }
}
.session-children {
  margin-left: 14px;
  border-left: 1px solid var(--color-line);
  padding-left: 6px;
}
.session-menu {
  position: absolute;
  top: calc(100% - 2px);
  right: 4px;
  z-index: 30;
  display: flex;
  min-width: 180px;
  flex-direction: column;
  gap: 2px;
  border: 1px solid var(--color-line);
  border-radius: 10px;
  background: var(--color-panel);
  padding: 4px;
  box-shadow: 0 12px 32px rgb(0 0 0 / 0.4);
}
.session-menu button {
  border-radius: 8px;
  padding: 6px 9px;
  color: var(--color-ink-2);
  font-size: 12px;
  text-align: left;
}
.session-menu button:hover {
  background: rgb(255 255 255 / 0.05);
  color: var(--color-ink);
}
.session-menu .is-danger {
  color: var(--color-danger);
}
.session-rename {
  min-width: 0;
  flex: 1;
  border: 1px solid var(--color-line-strong);
  border-radius: 8px;
  background: var(--color-panel-2);
  padding: 5px 8px;
  color: var(--color-ink);
  font-size: 13px;
}
.session-drawer {
  position: absolute;
  inset: 0;
  z-index: 40;
  display: flex;
}
.session-drawer-panel {
  display: flex;
  width: min(320px, 86vw);
  flex-direction: column;
  border-right: 1px solid var(--color-line);
}
.session-drawer-backdrop {
  flex: 1;
  background: rgb(0 0 0 / 0.45);
}
.session-view-header {
  display: flex;
  align-items: center;
  gap: 10px;
  border-bottom: 1px solid var(--color-line);
  padding: 10px 16px;
}
.session-kind-badge {
  flex-shrink: 0;
  border: 1px solid var(--color-line);
  border-radius: 999px;
  padding: 2px 8px;
  color: var(--color-ink-3);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}
.session-footer-note {
  display: flex;
  width: 100%;
  max-width: 48rem;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  margin: 0 auto;
  padding: 14px 24px 18px;
  color: var(--color-ink-3);
  font-size: 12px;
  text-align: center;
}
.session-load-older {
  align-self: center;
}
```

In `apps/web/src/styles.css`, add `@import './sessions.css';` after `@import './companion.css';`.

- [ ] **Step 5: Run the tests to verify they pass**

Run (from `apps/web`): `bun x vitest run src/components/sessions src/components/ChatScreen.test.tsx src/components/ChatScreen.memo.test.tsx src/visual-tokens.test.ts`
Expected: PASS — the new component tests, the unchanged chat tests (the new props are optional), and the visual contract (sessions.css uses only palette tokens).

- [ ] **Step 6: Commit**

```bash
git add apps/web/src/components/sessions/SessionSidebar.tsx apps/web/src/components/sessions/SessionSidebar.test.tsx apps/web/src/components/sessions/SessionView.tsx apps/web/src/components/sessions/SessionView.test.tsx apps/web/src/sessions.css apps/web/src/styles.css apps/web/src/components/ChatScreen.tsx
git commit -m "feat(web): add the sessions sidebar and session view components"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. Row and header menus close on Escape and return focus to their trigger. Test.

---

### Task 17: Route-driven shell and session-based chat

**Files:**

- Modify: `apps/web/src/components/WorkspaceShell.tsx` (rewrite), `apps/web/src/ViewHarness.tsx` (rewrite), `apps/web/src/lib/daemon-api.ts` (`getSession`)
- Modify tests: `apps/web/src/components/WorkspaceShell.test.tsx` (rewrite), `apps/web/src/components/CompanionShell.test.tsx` (rewrite), `apps/web/src/ViewHarness.test.tsx` (helpers, 13 tests changed, 3 added)
- Delete: `apps/web/src/components/ActivityView.tsx`, `apps/web/src/components/CheckinsView.tsx`, `apps/web/src/components/CheckinsView.test.tsx`, `apps/web/src/components/TelegramThread.tsx`, `apps/web/src/components/TelegramThread.test.tsx`
- Modify e2e fixtures (not run in the M2 gate): `apps/web-e2e/src/companion.spec.ts`, `apps/web-e2e/src/independent-agents.spec.ts`, `apps/web-e2e/src/main-workspace-agent.spec.ts`

**Interfaces:**

- Consumes: Task 15's `useHashRoute`, `HashRoute`, `HashPage`, `Navigate`, `useCompanionSessions`, `useSessionMessages`, `sessionKey`, `exportFileName`, `sessionFixture`, the `daemon` session methods and `toChatMessage`; Task 16's `SessionSidebar`, `SessionView`; existing `createTelegramIdempotencyKey`, `safeIntegrationError`, `daemon.sendConnectorMessage`, `useAgentIntegrations`.
- Produces:
  - `WorkspaceShell` props `{ mainAgent, agents, connection, route, navigate, conversation, sidebar?, connectors?, workspaceState?, onOpenSettings, onChangeWorkspaceAvatar?, onPickPrompt?, onNewChat? }` (replacing `workspace`, `activity`, and `telegram`); `AVAILABLE_PAGES` (`work`, `files`, `connectors`, `capabilities`), `AvailablePage`, `availablePage(route)`. Desktop sidebar: identity, "New chat", navigation (Work, Files, Connectors, and a collapsible "System" group with Capabilities), the sessions list, status, Settings. Mobile: an "Open sessions" top-bar button opening a "Sessions" drawer, and a bottom dock with Chats, Work, Files, Connectors, Capabilities. Other hash pages (approvals, automations, memory, skills, usage, logs, health) show the conversation until their milestones. "Back to chat" (`Open companion chat`) returns to the last conversation.
  - `daemon.getSession(agentId, sessionId)`.
  - `ViewHarness`: home (`#/`) is a new chat; the first send creates a `chat:` session (`daemon.createSession`), moves any text typed meanwhile into it, replaces the route with `#/s/<id>`, and runs `daemon.runAgent(agent, text, { clientRequestId }, session.roomId)`; later sends run in the session's room; Telegram sessions send with `daemon.sendConnectorMessage(agent, connector, text, createTelegramIdempotencyKey())`; each finished send reloads the session's messages (refresh key) and the list. Drafts, sending state, and recoverable messages are kept per agent and conversation. A timed-out send is confirmed from either the agent snapshot or the session messages (a committed user message means its blocking run finished). Opening an unread session marks it read up to its newest message. A page (Work, Files, …) hides the conversation but keeps the last chat or session loaded behind it. A changed companion returns to `#/`. The Telegram destination, `ActivityView`, `CheckinsView`, `TelegramThread`, and the "Delegated work" drawer are gone; check-in editing stays in Work › Schedules.

- [ ] **Step 1: Rewrite the shell tests (failing first)**

Replace `apps/web/src/components/WorkspaceShell.test.tsx` with:

```tsx
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ComponentProps } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from '../lib/agent-access';
import { daemon } from '../lib/daemon-api';
import type { HashRoute } from '../lib/hash-route';
import type { AgentDetail } from '../lib/types';
import { WorkspaceShell } from './WorkspaceShell';

function agent(
  id: string,
  name: string,
  createdAt: number,
  overrides: Partial<AgentDetail> = {},
): AgentDetail {
  return {
    id,
    name,
    provider: 'openai',
    model: 'gpt-4.1',
    toolNames: toolNamesForProfile('collaborate'),
    created_at_ms: createdAt,
    status: 'Idle',
    token_usage: {
      prompt_tokens: 3,
      completion_tokens: 5,
      total_tokens: 8,
    },
    messages: [],
    ...overrides,
  };
}

type ShellProps = Partial<ComponentProps<typeof WorkspaceShell>> & {
  initialRoute?: HashRoute;
};

/** Holds the route the way `useHashRoute` does in the app. */
function Shell({ initialRoute = { kind: 'home' }, ...props }: ShellProps) {
  const [route, setRoute] = useState<HashRoute>(initialRoute);
  const main = props.mainAgent ?? agent('agent-main', 'Nova', 1);
  return (
    <WorkspaceShell
      {...props}
      mainAgent={main}
      agents={props.agents ?? [main]}
      connection={props.connection ?? 'online'}
      conversation={props.conversation ?? <div>Workspace canvas</div>}
      onOpenSettings={props.onOpenSettings ?? vi.fn()}
      route={route}
      navigate={(next) => setRoute(next)}
    />
  );
}

function mobile() {
  vi.stubGlobal(
    'matchMedia',
    vi.fn(() => ({
      matches: false,
      media: '(min-width: 768px)',
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  );
}

const configuredWorkspace = (hasAvatar: boolean) => ({
  configured: true,
  workspace: {
    rootPath: '/workspaces/northwind',
    companyName: 'Northwind Research',
    mission: 'Map supply chains',
    values: ['rigor'],
    hasAvatar,
  },
  defaultRoot: '/workspaces',
});

beforeEach(() => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: '1',
  });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('WorkspaceShell', () => {
  it('lands on the conversation with a New chat action and no work dispatched', async () => {
    const onNewChat = vi.fn();
    render(<Shell onNewChat={onNewChat} />);
    expect(screen.getByText('Workspace canvas')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Operations' }),
    ).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'New chat' }));
    expect(onNewChat).toHaveBeenCalledOnce();
  });

  it('shows the sessions list between the navigation and the status', () => {
    render(<Shell sidebar={<div>Sessions list</div>} />);
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Sessions list')).toBeVisible();
    expect(
      within(sidebar)
        .getByRole('navigation', { name: 'Workspace navigation' })
        .compareDocumentPosition(within(sidebar).getByText('Sessions list')) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0);
  });

  it('opens capabilities from the collapsible System group', async () => {
    vi.spyOn(daemon, 'capabilities').mockResolvedValue({
      schemaVersion: 1,
      tools: [],
      persistence: {
        controlPlane: 'file',
        memory: 'file',
        executionJournal: false,
      },
      extensions: [],
      limitations: [],
    });
    render(<Shell />);
    expect(
      screen.queryByRole('button', { name: 'Capabilities', exact: true }),
    ).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'System' }));
    await userEvent.click(
      screen.getByRole('button', { name: 'Capabilities', exact: true }),
    );
    expect(
      await screen.findByText('Tools follow your authority'),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Capabilities', exact: true }),
    ).toHaveAttribute('aria-current', 'page');
  });

  it('opens commands with Control K, filters actions and navigates with Enter', async () => {
    const user = userEvent.setup();
    render(<Shell connectors={<div>Manage connections</div>} />);
    await user.keyboard('{Control>}k{/Control}');
    expect(screen.getByRole('dialog', { name: 'Command menu' })).toBeVisible();
    await user.type(
      screen.getByRole('combobox', { name: 'Search commands' }),
      'connectors',
    );
    await user.keyboard('{Enter}');
    expect(screen.getByText('Manage connections')).toBeVisible();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('can enter and leave focus mode without losing the conversation', async () => {
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(screen.getByRole('button', { name: 'Enter focus mode' }));
    expect(screen.queryByRole('complementary')).not.toBeInTheDocument();
    expect(screen.getByText('Workspace canvas')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Exit focus mode' }));
    expect(screen.getByRole('complementary')).toBeVisible();
  });

  it('contains keyboard focus in commands and restores the opener on Escape', async () => {
    const user = userEvent.setup();
    render(<Shell />);
    const opener = screen.getByRole('button', { name: 'Open command menu' });
    await user.click(opener);
    const search = screen.getByRole('combobox', { name: 'Search commands' });
    expect(search).toHaveFocus();
    await user.tab();
    expect(
      screen.getByRole('button', { name: 'Close command menu' }),
    ).toHaveFocus();
    await user.tab();
    expect(search).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(opener).toHaveFocus();
  });

  it('inserts a prompt from commands without invoking a send', async () => {
    const user = userEvent.setup();
    const pick = vi.fn();
    render(<Shell onPickPrompt={pick} />);
    await user.click(screen.getByRole('button', { name: 'Open command menu' }));
    await user.type(
      screen.getByRole('combobox', { name: 'Search commands' }),
      'Plan my next hour',
    );
    await user.keyboard('{Enter}');
    expect(pick).toHaveBeenCalledWith(
      expect.stringContaining('Help me plan my next hour'),
    );
    expect(screen.getByText('Workspace canvas')).toBeVisible();
  });

  it('keeps helpers out of the top-level navigation', () => {
    const nova = agent('agent-main', 'Nova', 1);
    render(
      <Shell mainAgent={nova} agents={[nova, agent('scout', 'Scout', 2)]} />,
    );
    expect(
      screen.queryByRole('button', { name: 'Team' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Message Scout' }),
    ).not.toBeInTheDocument();
    expect(screen.getByText('Workspace canvas')).toBeVisible();
  });

  it('uses a left sidebar and returns from a page to the mounted conversation', async () => {
    render(<Shell />);
    const navigation = screen.getByRole('navigation', {
      name: 'Workspace navigation',
    });
    expect(navigation).toHaveAttribute('data-placement', 'sidebar');
    expect(navigation).toHaveAttribute('aria-orientation', 'vertical');
    expect(navigation.closest('aside')?.nextElementSibling?.tagName).toBe(
      'MAIN',
    );
    await userEvent.click(
      within(navigation).getByRole('button', { name: 'Work' }),
    );
    expect(
      within(navigation).getByRole('button', { name: 'Work' }),
    ).toHaveAttribute('aria-current', 'page');
    expect(screen.getByText('Workspace canvas')).not.toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: 'Open companion chat' }),
    );
    expect(screen.getByText('Workspace canvas')).toBeVisible();
  });

  it('shows the conversation for pages that arrive in later releases', () => {
    render(<Shell initialRoute={{ kind: 'page', page: 'approvals' }} />);
    expect(screen.getByText('Workspace canvas')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Open companion chat' }),
    ).not.toBeInTheDocument();
  });

  it('shows the main agent identity in the sidebar presence block', () => {
    render(<Shell connection="offline" />);
    const sidebar = screen.getByRole('complementary');
    expect(
      within(sidebar).getByRole('heading', { name: 'Nova' }),
    ).toBeVisible();
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(within(sidebar).getByText('Companion')).toBeVisible();
  });

  it('shows the persisted workspace avatar and uploads a replacement', async () => {
    const user = userEvent.setup();
    const onChangeWorkspaceAvatar = vi.fn().mockResolvedValue(undefined);
    Object.defineProperties(URL, {
      createObjectURL: {
        configurable: true,
        value: vi.fn(() => 'blob:workspace-avatar-preview'),
      },
      revokeObjectURL: { configurable: true, value: vi.fn() },
    });
    render(
      <Shell
        workspaceState={configuredWorkspace(true)}
        onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
      />,
    );
    expect(
      screen
        .getByRole('button', { name: 'Change workspace avatar' })
        .querySelector('img'),
    ).toHaveAttribute('src', '/api/workspace/avatar?v=0');
    const file = new File(['avatar'], 'avatar.png', { type: 'image/png' });
    await user.upload(
      screen.getByLabelText('Workspace avatar image file'),
      file,
    );
    await waitFor(() =>
      expect(onChangeWorkspaceAvatar).toHaveBeenCalledWith(file),
    );
  });

  it('shows a compact presence bar on mobile', () => {
    mobile();
    render(
      <Shell
        mainAgent={agent('agent-main', 'Nova', 1, { status: 'Running' })}
        connection="offline"
      />,
    );
    const bar = screen.getByRole('banner');
    expect(within(bar).getByRole('heading', { name: 'Nova' })).toBeVisible();
    expect(within(bar).getByRole('button', { name: 'Settings' })).toBeVisible();
  });

  it('opens the sessions drawer from the mobile top bar', async () => {
    mobile();
    const user = userEvent.setup();
    render(<Shell sidebar={<div>Sessions list</div>} />);
    expect(screen.queryByText('Sessions list')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Open sessions' }));
    const drawer = screen.getByRole('dialog', { name: 'Sessions' });
    expect(within(drawer).getByText('Sessions list')).toBeVisible();
    await user.click(
      within(drawer).getByRole('button', { name: 'Close sessions' }),
    );
    expect(
      screen.queryByRole('dialog', { name: 'Sessions' }),
    ).not.toBeInTheDocument();
  });

  it('shows working helpers as status without introducing another persona', () => {
    const main = agent('agent-main', 'Nova', 1);
    render(
      <Shell
        mainAgent={main}
        agents={[agent('helper', 'Research', 2, { status: 'Running' }), main]}
      />,
    );
    expect(screen.getByText('1 helper is working')).toBeVisible();
    expect(
      screen.queryByRole('article', { name: 'Research agent' }),
    ).not.toBeInTheDocument();
  });

  it('exposes settings as a contextual action for the main agent', async () => {
    const onOpenSettings = vi.fn();
    render(<Shell onOpenSettings={onOpenSettings} />);
    const settings = screen.getByRole('button', { name: 'Settings' });
    expect(settings).toHaveAttribute('title', 'Settings for Nova');
    await userEvent.click(settings);
    expect(onOpenSettings).toHaveBeenCalledOnce();
  });

  it('places mobile navigation after workspace content in DOM and tab order', () => {
    mobile();
    render(
      <Shell conversation={<button type="button">Workspace action</button>} />,
    );
    const content = screen.getByRole('main');
    const navigation = screen.getByRole('navigation', {
      name: 'Workspace navigation',
    });
    expect(navigation).toHaveAttribute('data-placement', 'bottom-dock');
    expect(content.parentElement?.nextElementSibling).toBe(navigation);
    expect(
      within(navigation).getByRole('button', { name: 'Chats' }),
    ).toHaveAttribute('aria-current', 'page');
    expect(
      screen
        .getByRole('button', { name: 'Workspace action' })
        .compareDocumentPosition(
          within(navigation).getByRole('button', { name: 'Chats' }),
        ) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0);
  });

  it('shows the configured workspace company name next to the shell brand', () => {
    render(<Shell workspaceState={configuredWorkspace(false)} />);
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(within(sidebar).getByText('Northwind Research')).toBeVisible();
  });

  it('renders the presence block exactly as today when no workspace is configured', () => {
    render(<Shell workspaceState={null} />);
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(
      within(sidebar).getByRole('heading', { name: 'Nova' }),
    ).toBeVisible();
    expect(
      within(sidebar).queryByText('Northwind Research'),
    ).not.toBeInTheDocument();
  });

  it('hides the company name when the workspace state is not configured', () => {
    render(
      <Shell
        workspaceState={{
          configured: false,
          workspace: null,
          defaultRoot: '/workspaces',
        }}
      />,
    );
    const sidebar = screen.getByRole('complementary');
    expect(within(sidebar).getByText('Welcome back')).toBeVisible();
    expect(
      within(sidebar).queryByText('Northwind Research'),
    ).not.toBeInTheDocument();
  });

  it('opens Connectors as a page', async () => {
    render(<Shell connectors={<div>Manage connections</div>} />);
    await userEvent.click(screen.getByRole('button', { name: 'Connectors' }));
    expect(screen.getByText('Manage connections')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Telegram' }),
    ).not.toBeInTheDocument();
  });
});
```

Replace `apps/web/src/components/CompanionShell.test.tsx` with:

```tsx
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../lib/daemon-api';
import type { HashRoute } from '../lib/hash-route';
import type { AgentDetail } from '../lib/types';
import { WorkspaceShell } from './WorkspaceShell';

const companion: AgentDetail = {
  id: 'main',
  name: 'Anima',
  provider: 'openai',
  model: 'test-model',
  status: 'Idle',
  created_at_ms: 1,
  toolNames: [],
  messages: [],
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
};

function Shell({
  agents = [companion],
  connection = 'online',
  conversation,
}: {
  agents?: AgentDetail[];
  connection?: 'online' | 'offline';
  conversation: ReactNode;
}) {
  const [route, setRoute] = useState<HashRoute>({ kind: 'home' });
  return (
    <WorkspaceShell
      mainAgent={companion}
      agents={agents}
      connection={connection}
      route={route}
      navigate={(next) => setRoute(next)}
      conversation={conversation}
      onOpenSettings={vi.fn()}
    />
  );
}

beforeEach(() => {
  vi.spyOn(daemon, 'agentJobs').mockResolvedValue([]);
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: '1',
  });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('single companion experience', () => {
  it('opens the conversation immediately without swarm management or agent switching', () => {
    render(
      <Shell
        agents={[
          companion,
          { ...companion, id: 'helper', name: 'Research helper' },
        ]}
        conversation={<div>My conversation</div>}
      />,
    );
    expect(screen.getByText('My conversation')).toBeVisible();
    expect(screen.getByRole('button', { name: 'New chat' })).toBeVisible();
    for (const name of ['Team', 'Operations', 'Overview']) {
      expect(
        screen.queryByRole('button', { name, exact: true }),
      ).not.toBeInTheDocument();
    }
    expect(
      screen.queryByRole('combobox', { name: 'Chat with agent' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('navigation', { name: 'Direct messages' }),
    ).not.toBeInTheDocument();
    expect(
      within(screen.getByRole('complementary')).getByRole('heading', {
        name: 'Anima',
      }),
    ).toBeVisible();
  });

  it('keeps the conversation mounted when checking another page', async () => {
    render(
      <Shell
        conversation={
          <input aria-label="Unsaved draft" defaultValue="Remember this" />
        }
      />,
    );
    const input = screen.getByLabelText('Unsaved draft');
    await userEvent.click(
      screen.getByRole('button', { name: 'Work', exact: true }),
    );
    expect(input).not.toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: 'Open companion chat' }),
    );
    expect(screen.getByLabelText('Unsaved draft')).toBe(input);
    expect(input).toHaveValue('Remember this');
  });

  it('describes disconnection without promising work is still running', () => {
    render(
      <Shell connection="offline" conversation={<div>My conversation</div>} />,
    );
    expect(screen.getByText('Offline')).toBeVisible();
    expect(screen.getByText('Cannot reach your companion')).toBeVisible();
  });
});
```

Run (from `apps/web`): `bun x vitest run src/components/WorkspaceShell.test.tsx src/components/CompanionShell.test.tsx`
Expected: FAIL — the shell still renders the old `workspace`/`activity` destinations (for example `Unable to find an accessible element with the role "button" and name "New chat"`).

- [ ] **Step 2: Rewrite the shell**

Replace `apps/web/src/components/WorkspaceShell.tsx` with:

```tsx
import { useEffect, useState, type ReactNode } from 'react';
import type { DaemonConnection } from '../hooks/useDaemonBootstrap';
import type { DaemonWorkspaceState } from '../lib/daemon-api';
import type { HashPage, HashRoute, Navigate } from '../lib/hash-route';
import type { AgentDetail } from '../lib/types';
import { AgentPresence } from './AgentPresence';
import { WorkspaceHub } from './WorkspaceHub';
import { WorkspaceFiles } from './WorkspaceFiles';
import { WorkspaceCapabilities } from './WorkspaceCapabilities';
import { CommandMenu, type StudioCommand } from './CommandMenu';
import { PROMPT_LIBRARY } from '../lib/prompt-library';
import { GearIcon, PulseIcon, SendIcon, SparkIcon } from './icons';
import { ghostBtnCls } from './ui-bits';

/** Pages this release renders; the other hash pages open the conversation
 *  until their milestones build them. */
export const AVAILABLE_PAGES = [
  'work',
  'files',
  'connectors',
  'capabilities',
] as const satisfies readonly HashPage[];
export type AvailablePage = (typeof AVAILABLE_PAGES)[number];

interface Destination {
  page: AvailablePage;
  label: string;
  icon: ReactNode;
}

const PRIMARY_DESTINATIONS: Destination[] = [
  { page: 'work', label: 'Work', icon: <SparkIcon size={16} /> },
  { page: 'files', label: 'Files', icon: <PulseIcon size={16} /> },
  { page: 'connectors', label: 'Connectors', icon: <GearIcon size={16} /> },
];
const SYSTEM_DESTINATIONS: Destination[] = [
  {
    page: 'capabilities',
    label: 'Capabilities',
    icon: <SparkIcon size={16} />,
  },
];
const DESTINATIONS = [...PRIMARY_DESTINATIONS, ...SYSTEM_DESTINATIONS];

export function availablePage(route: HashRoute): AvailablePage | null {
  return route.kind === 'page' &&
    (AVAILABLE_PAGES as readonly HashPage[]).includes(route.page)
    ? (route.page as AvailablePage)
    : null;
}

const ignoreWorkspaceAvatarChange = async () => undefined;
const DESKTOP_NAVIGATION_QUERY = '(min-width: 768px)';

function useDesktopNavigation() {
  const [desktop, setDesktop] = useState(
    () =>
      typeof window.matchMedia !== 'function' ||
      window.matchMedia(DESKTOP_NAVIGATION_QUERY).matches,
  );
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(DESKTOP_NAVIGATION_QUERY);
    const update = () => setDesktop(media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  return desktop;
}

function DestinationNavigation({
  page,
  navigate,
  placement,
  onOpenChats,
}: {
  page: AvailablePage | null;
  navigate: Navigate;
  placement: 'sidebar' | 'bottom-dock';
  onOpenChats: () => void;
}) {
  const sidebar = placement === 'sidebar';
  const [systemOpen, setSystemOpen] = useState(false);
  const systemExpanded =
    systemOpen || SYSTEM_DESTINATIONS.some((item) => item.page === page);
  const itemClass = `studio-nav-item inline-flex items-center gap-3 rounded-xl px-3 py-2.5 text-sm transition ${sidebar ? 'w-full justify-start text-left' : 'min-w-16 shrink-0 flex-col gap-1 text-[10px]'}`;
  const destination = (item: Destination) => (
    <button
      key={item.page}
      type="button"
      onClick={() => navigate({ kind: 'page', page: item.page })}
      aria-current={page === item.page ? 'page' : undefined}
      aria-label={item.label}
      className={itemClass}
    >
      {item.icon}
      <span>{item.label}</span>
    </button>
  );
  return (
    <nav
      aria-label="Workspace navigation"
      aria-orientation={sidebar ? 'vertical' : 'horizontal'}
      data-placement={placement}
      className={
        sidebar
          ? 'studio-navigation flex shrink-0 flex-col gap-1 p-3'
          : 'safe-bottom-dock glass-strong absolute inset-x-3 z-30 flex items-center gap-1 overflow-x-auto rounded-2xl p-1.5'
      }
    >
      {!sidebar && (
        <button
          type="button"
          onClick={onOpenChats}
          aria-current={page === null ? 'page' : undefined}
          aria-label="Chats"
          className={itemClass}
        >
          <SendIcon size={16} />
          <span>Chats</span>
        </button>
      )}
      {PRIMARY_DESTINATIONS.map((item) => destination(item))}
      {sidebar ? (
        <>
          <button
            type="button"
            className={itemClass}
            aria-expanded={systemExpanded}
            onClick={() => setSystemOpen((open) => !open)}
          >
            <GearIcon size={16} />
            <span>System</span>
          </button>
          {systemExpanded &&
            SYSTEM_DESTINATIONS.map((item) => destination(item))}
        </>
      ) : (
        SYSTEM_DESTINATIONS.map((item) => destination(item))
      )}
    </nav>
  );
}

export function WorkspaceShell({
  mainAgent,
  agents,
  connection,
  route,
  navigate,
  conversation,
  sidebar = null,
  connectors = null,
  workspaceState = null,
  onOpenSettings,
  onChangeWorkspaceAvatar = ignoreWorkspaceAvatarChange,
  onPickPrompt,
  onNewChat,
}: {
  mainAgent: AgentDetail;
  agents: readonly AgentDetail[];
  connection: Exclude<DaemonConnection, 'unknown'>;
  route: HashRoute;
  navigate: Navigate;
  /** The chat or session view; kept mounted so drafts and scroll survive page visits. */
  conversation: ReactNode;
  /** The sessions list for the desktop sidebar and the mobile drawer. */
  sidebar?: ReactNode | null;
  connectors?: ReactNode | null;
  workspaceState?: DaemonWorkspaceState | null;
  onOpenSettings: () => void;
  onChangeWorkspaceAvatar?: (file: File) => Promise<void>;
  onPickPrompt?: (prompt: string) => void;
  onNewChat?: () => void;
}) {
  const page = availablePage(route);
  const [lastConversation, setLastConversation] = useState<HashRoute>(
    route.kind === 'session' ? route : { kind: 'home' },
  );
  const [commandsOpen, setCommandsOpen] = useState(false);
  const [focusMode, setFocusMode] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const desktopNavigation = useDesktopNavigation();
  const companyName = workspaceState?.configured
    ? (workspaceState.workspace?.companyName ?? null)
    : null;
  const hasAvatar =
    workspaceState?.configured === true &&
    workspaceState.workspace?.hasAvatar === true;
  const workingHelpers = agents.filter(
    (agent) => agent.id !== mainAgent.id && agent.status === 'Running',
  ).length;
  const newChat = onNewChat ?? (() => navigate({ kind: 'home' }));
  const openConversation = () => navigate(lastConversation);

  useEffect(() => {
    if (route.kind === 'session' || route.kind === 'home')
      setLastConversation(route);
    setDrawerOpen(false);
  }, [route]);

  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      if (
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === 'k' &&
        !event.isComposing
      ) {
        if (document.querySelector('[aria-modal="true"]')) return;
        event.preventDefault();
        setCommandsOpen(true);
      }
    };
    window.addEventListener('keydown', shortcut);
    return () => window.removeEventListener('keydown', shortcut);
  }, []);

  const commands: StudioCommand[] = [
    {
      id: 'new-chat',
      title: 'New chat',
      description: 'Start a fresh conversation',
      group: 'Navigate',
      run: newChat,
    },
    ...DESTINATIONS.map((item) => ({
      id: item.page,
      title: `Go to ${item.label}`,
      description: `Open ${item.label.toLowerCase()}`,
      group: 'Navigate',
      run: () => navigate({ kind: 'page', page: item.page }),
    })),
    {
      id: 'settings',
      title: 'Companion settings',
      description: 'Identity, model, and access',
      group: 'Navigate',
      run: () => requestAnimationFrame(onOpenSettings),
    },
    ...(desktopNavigation
      ? [
          {
            id: 'focus',
            title: focusMode ? 'Exit focus mode' : 'Enter focus mode',
            description: 'More room for your conversation',
            group: 'View',
            run: () => setFocusMode((value) => !value),
          },
        ]
      : []),
    ...(onPickPrompt
      ? PROMPT_LIBRARY.map((prompt) => ({
          id: prompt.id,
          title: prompt.title,
          description: prompt.description,
          group: prompt.category,
          run: () => {
            if (page !== null) openConversation();
            onPickPrompt(prompt.prompt);
            requestAnimationFrame(() =>
              document
                .querySelector<HTMLTextAreaElement>('[data-workspace-composer]')
                ?.focus(),
            );
          },
        }))
      : []),
  ];

  return (
    <>
      <div
        className={`studio-shell companion-shell relative z-[1] flex min-h-0 flex-1 flex-col ${focusMode ? 'is-focused' : ''}`}
        inert={commandsOpen || undefined}
        aria-hidden={commandsOpen || undefined}
      >
        {!desktopNavigation && (
          <AgentPresence
            agent={mainAgent}
            connection={connection}
            companyName={companyName}
            placement="mobile-bar"
            hasAvatar={hasAvatar}
            onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
            onOpenSettings={onOpenSettings}
          />
        )}
        <div className="studio-frame relative flex min-h-0 flex-1">
          {desktopNavigation && !focusMode && (
            <aside className="studio-sidebar relative z-20 flex w-60 shrink-0 flex-col border-r border-line">
              <div className="studio-brand">
                <span className="studio-brand-mark" aria-hidden>
                  ✳
                </span>
                <span>Anima</span>
              </div>
              <AgentPresence
                agent={mainAgent}
                connection={connection}
                companyName={companyName}
                placement="sidebar"
                hasAvatar={hasAvatar}
                onChangeWorkspaceAvatar={onChangeWorkspaceAvatar}
              />
              <div className="shrink-0 px-3 pt-3">
                <button
                  type="button"
                  className={`${ghostBtnCls} w-full justify-center`}
                  onClick={newChat}
                >
                  New chat
                </button>
              </div>
              <DestinationNavigation
                page={page}
                navigate={navigate}
                placement="sidebar"
                onOpenChats={openConversation}
              />
              {sidebar}
              <div className="companion-status" role="status">
                <p>
                  {connection === 'offline'
                    ? 'Cannot reach your companion'
                    : mainAgent.status === 'Running'
                      ? 'Working on your request'
                      : 'Ready when you are'}
                </p>
                <span>
                  {connection === 'offline'
                    ? 'Check the server connection.'
                    : workingHelpers > 0
                      ? `${workingHelpers} ${workingHelpers === 1 ? 'helper is' : 'helpers are'} working`
                      : 'Your conversations stay with you.'}
                </span>
              </div>
              <div className="border-t border-line p-3">
                <button
                  type="button"
                  onClick={onOpenSettings}
                  className={`${ghostBtnCls} w-full justify-start`}
                  aria-label="Settings"
                  title={`Settings for ${mainAgent.name}`}
                >
                  <GearIcon size={15} />
                  <span>Settings</span>
                </button>
              </div>
            </aside>
          )}
          <main className="studio-main spatial-canvas workspace-mobile-safe relative min-h-0 min-w-0 flex-1">
            <div className="studio-topbar">
              {!desktopNavigation && sidebar !== null && (
                <button
                  type="button"
                  className="studio-tool-button"
                  aria-label="Open sessions"
                  aria-expanded={drawerOpen}
                  onClick={() => setDrawerOpen(true)}
                >
                  ☰
                </button>
              )}
              <div className="studio-breadcrumb">
                <strong>
                  {page
                    ? DESTINATIONS.find((item) => item.page === page)?.label
                    : mainAgent.name}
                </strong>
                {page === null && (
                  <span className="companion-model">{mainAgent.model}</span>
                )}
              </div>
              <span
                className={`studio-connection ${connection === 'online' ? 'is-online' : 'is-offline'}`}
              >
                <i aria-hidden />
                {connection === 'online' ? 'Connected' : 'Offline'}
              </span>
              <div className="studio-topbar-actions">
                {page !== null && (
                  <button
                    type="button"
                    className="studio-tool-button"
                    aria-label="Open companion chat"
                    onClick={openConversation}
                  >
                    Back to chat
                  </button>
                )}
                <button
                  type="button"
                  className="studio-command-trigger"
                  onClick={() => setCommandsOpen(true)}
                  aria-label="Open command menu"
                >
                  <span aria-hidden>⌕</span>
                  <span className="studio-command-trigger-label">Search</span>
                  <kbd>Ctrl K</kbd>
                </button>
                {desktopNavigation && (
                  <button
                    type="button"
                    className="studio-tool-button"
                    onClick={() => setFocusMode((value) => !value)}
                    aria-label={
                      focusMode ? 'Exit focus mode' : 'Enter focus mode'
                    }
                    aria-pressed={focusMode}
                  >
                    {focusMode ? '↙' : '⛶'}
                  </button>
                )}
              </div>
            </div>
            <div className="studio-view">
              {/* Keep the conversation mounted so drafts and scroll survive page visits. */}
              <div className="companion-chat-panel" hidden={page !== null}>
                {conversation}
              </div>
              {page === 'connectors' ? (
                connectors
              ) : page === 'files' ? (
                <WorkspaceFiles online={connection === 'online'} />
              ) : page === 'capabilities' ? (
                <WorkspaceCapabilities online={connection === 'online'} />
              ) : page === 'work' ? (
                <WorkspaceHub agents={[mainAgent]} initialSection="Tasks" />
              ) : null}
            </div>
            {drawerOpen && !desktopNavigation && sidebar !== null && (
              <div
                className="session-drawer"
                role="dialog"
                aria-modal="true"
                aria-label="Sessions"
              >
                <div className="session-drawer-panel studio-sidebar">
                  <div className="flex items-center justify-between gap-2 p-3">
                    <button
                      type="button"
                      className={ghostBtnCls}
                      onClick={() => {
                        setDrawerOpen(false);
                        newChat();
                      }}
                    >
                      New chat
                    </button>
                    <button
                      type="button"
                      className="studio-tool-button"
                      aria-label="Close sessions"
                      onClick={() => setDrawerOpen(false)}
                    >
                      ×
                    </button>
                  </div>
                  {sidebar}
                </div>
                <div
                  className="session-drawer-backdrop"
                  aria-hidden
                  onClick={() => setDrawerOpen(false)}
                />
              </div>
            )}
          </main>
        </div>
        {!desktopNavigation && (
          <DestinationNavigation
            page={page}
            navigate={navigate}
            placement="bottom-dock"
            onOpenChats={openConversation}
          />
        )}
      </div>
      {commandsOpen && (
        <CommandMenu commands={commands} close={() => setCommandsOpen(false)} />
      )}
    </>
  );
}
```

(`visual-tokens.test.ts` keeps finding `'safe-bottom-dock glass-strong absolute`, `data-placement={placement}`, `'sidebar'`, `'bottom-dock'`, and `studio-frame relative flex`.)

Run (from `apps/web`): `bun x vitest run src/components/WorkspaceShell.test.tsx src/components/CompanionShell.test.tsx src/visual-tokens.test.ts`
Expected: PASS.

- [ ] **Step 3: Update the ViewHarness tests (failing first)**

In `apps/web/src/ViewHarness.test.tsx`:

(a) Add to the imports: `import type { Session, SessionMessage } from '@animaOS-SWARM/sdk';` and `import { sessionFixture } from './test/sessions';`.

(b) Replace `async function openChat() { … }` with:

```ts
async function openChat() {
  fireEvent.click(await screen.findByRole('button', { name: 'New chat' }));
}

const readOnly = {
  send: false,
  steer: false,
  stop: true,
  rename: false,
  archive: true,
  delete: false,
  compact: false,
  export: true,
};

interface SessionRoutes {
  sessions: Session[];
}

let routes: SessionRoutes;

/** In-memory session routes: created chats are listed as `chat:new-<n>`. */
function mockSessionRoutes(): SessionRoutes {
  const state: SessionRoutes = { sessions: [] };
  let created = 0;
  vi.spyOn(daemon, 'listSessions').mockImplementation(async () => ({
    sessions: [...state.sessions],
    nextCursor: null,
  }));
  vi.spyOn(daemon, 'getSession').mockImplementation(
    async (_agentId, sessionId) => {
      const found = state.sessions.find((item) => item.id === sessionId);
      if (!found) throw Object.assign(new Error('not found'), { status: 404 });
      return found;
    },
  );
  vi.spyOn(daemon, 'createSession').mockImplementation(async (agentId) => {
    created += 1;
    const session = sessionFixture(`chat:new-${created}`, { agentId });
    state.sessions.unshift(session);
    return session;
  });
  vi.spyOn(daemon, 'updateSession').mockImplementation(
    async (_agentId, sessionId, patch) => {
      const index = state.sessions.findIndex((item) => item.id === sessionId);
      const updated: Session = {
        ...state.sessions[index],
        ...patch,
        unread:
          patch.lastReadAtMs !== undefined
            ? false
            : state.sessions[index].unread,
      };
      state.sessions[index] = updated;
      return updated;
    },
  );
  vi.spyOn(daemon, 'deleteSession').mockImplementation(
    async (_agentId, sessionId) => {
      state.sessions = state.sessions.filter((item) => item.id !== sessionId);
    },
  );
  vi.spyOn(daemon, 'sessionMessages').mockResolvedValue({
    messages: [],
    nextBefore: null,
  });
  return state;
}

/** Session messages read from an agent snapshot's room, like the daemon route. */
function messagesFromSnapshot(snapshotOf: () => DaemonSnapshot) {
  vi.spyOn(daemon, 'sessionMessages').mockImplementation(
    async (_agentId, sessionId) => ({
      messages: snapshotOf()
        .messages.filter((message) => message.roomId === sessionId)
        .map(
          (message): SessionMessage => ({
            id: message.id,
            role: message.role as SessionMessage['role'],
            text: message.content.text,
            attachments: [],
            metadata: message.content.metadata ?? {},
            createdAtMs: message.createdAtMs,
          }),
        ),
      nextBefore: null,
    }),
  );
}
```

(c) Replace `function withMessage(source: DaemonSnapshot, text: string): DaemonSnapshot { … }` with:

```ts
function withMessage(
  source: DaemonSnapshot,
  text: string,
  roomId = `room-${source.state.id}`,
): DaemonSnapshot {
  const updated = structuredClone(source);
  updated.messages = [
    {
      id: `message-${source.state.id}`,
      agentId: source.state.id,
      roomId,
      role: 'assistant',
      content: { text },
      createdAtMs: source.state.createdAtMs + 1,
    },
  ];
  updated.messageCount = 1;
  return updated;
}
```

(d) In the top-level `beforeEach`, add `routes = mockSessionRoutes();` as its last statement, and in the top-level `afterEach` add `window.history.replaceState(null, '', '/');` before `vi.restoreAllMocks();`.

(e) Replace these tests with the versions below (same position in the file):

```tsx
it('keeps the companion draft and failed send while opening Work', async () => {
  const user = userEvent.setup();
  const alpha = snapshot('alpha', 'Alpha', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [alpha] });
  mockProviders();
  const run = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
  vi.spyOn(daemon, 'runAgent').mockReturnValue(run.promise);
  render(<ViewHarness />);
  await user.type(
    await screen.findByPlaceholderText('Message Alpha…'),
    'Alpha request',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  await act(async () => {
    run.reject(new Error('Alpha disconnected'));
  });
  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  await user.click(
    await screen.findByRole('button', { name: 'Restore message' }),
  );
  expect(screen.getByPlaceholderText('Message Alpha…')).toHaveValue(
    'Alpha request',
  );
});

it('retains a completed reply after opening Work and keeps settings on the companion', async () => {
  const user = userEvent.setup();
  const alpha = snapshot('alpha', 'Alpha', 1);
  const beta = snapshot('beta', 'Beta', 2);
  let current = alpha;
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [alpha, beta] });
  mockProviders();
  messagesFromSnapshot(() => current);
  const run = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
  vi.spyOn(daemon, 'runAgent').mockReturnValue(run.promise);
  render(<ViewHarness />);
  await user.type(
    await screen.findByPlaceholderText('Message Alpha…'),
    'Alpha request',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  current = withMessage(alpha, 'Alpha finished', 'chat:new-1');
  await act(async () =>
    run.resolve({
      agent: current,
      result: {
        status: 'success',
        durationMs: 1,
        data: { text: 'Alpha finished' },
      },
    }),
  );
  expect(await screen.findByText('Alpha finished')).not.toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  expect(
    within(screen.getByRole('dialog')).getByDisplayValue('Alpha'),
  ).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Close settings' }));
  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  expect(screen.getByText('Alpha finished')).toBeVisible();
});

it('lists peer requests as read-only helper sessions apart from the owner chat', async () => {
  const user = userEvent.setup();
  const alpha = withMessage(
    snapshot('alpha', 'Alpha', 1),
    'Owner reply',
    'chat:owner',
  );
  alpha.messages.push({
    id: 'peer-message',
    agentId: 'alpha',
    roomId: 'peer:beta:alpha',
    role: 'user',
    content: {
      text: 'Private teammate request',
      metadata: {
        communication: {
          kind: 'peer',
          fromAgentId: 'beta',
          toAgentId: 'alpha',
        },
      },
    },
    createdAtMs: 3,
  });
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [alpha, snapshot('beta', 'Beta', 2)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:owner', {
      agentId: 'alpha',
      title: 'Owner chat',
      lastActivityAtMs: Date.now(),
    }),
    sessionFixture('peer:beta:alpha', {
      agentId: 'alpha',
      kind: 'helper',
      origin: 'peer',
      title: 'Messages from Beta',
      parentAgentId: 'beta',
      capabilities: readOnly,
      lastActivityAtMs: Date.now() - 1,
    }),
  );
  messagesFromSnapshot(() => alpha);
  window.history.replaceState(null, '', '/#/s/chat%3Aowner');
  render(<ViewHarness />);
  await screen.findByText('Owner reply');
  expect(
    within(screen.getByLabelText('Conversation with Alpha')).queryByText(
      'Private teammate request',
    ),
  ).not.toBeInTheDocument();
  await user.click(
    await screen.findByRole('button', { name: 'Messages from Beta' }),
  );
  expect(await screen.findByText('Private teammate request')).toBeVisible();
  expect(screen.getByRole('note')).toHaveTextContent(
    'Helper sessions are read-only.',
  );
  expect(
    screen.queryByPlaceholderText('Message Alpha…'),
  ).not.toBeInTheDocument();
});
```

Inside `describe('ViewHarness workspace controller', …)`, replace:

```tsx
it('selects the oldest agent by creation time then id for chat, settings, and Main', async () => {
  const user = userEvent.setup();
  const alpha = snapshot('agent-a', 'Alpha', 10);
  const beta = snapshot('agent-b', 'Beta', 10);
  const later = snapshot('agent-later', 'Later', 20);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [later, beta, alpha],
  });
  mockProviders();
  const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
    agent: alpha,
    result: { status: 'success', durationMs: 1, data: { text: 'done' } },
  });

  render(<ViewHarness />);
  await openChat();

  expect(
    await screen.findByRole('heading', { name: 'Say something to Alpha' }),
  ).toBeVisible();
  await user.type(screen.getByPlaceholderText('Message Alpha…'), 'Hello');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() =>
    expect(runAgent).toHaveBeenCalledWith(
      'agent-a',
      'Hello',
      expect.objectContaining({ clientRequestId: expect.any(String) }),
      'chat:new-1',
    ),
  );
  expect(daemon.createSession).toHaveBeenCalledWith('agent-a');

  expect(screen.getByText('Companion')).toBeVisible();
  expect(
    screen.queryByRole('button', { name: 'Message Beta' }),
  ).not.toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: 'Settings' }));
  expect(screen.getByRole('heading', { name: 'Agent settings' })).toBeVisible();
  expect(screen.getByDisplayValue('Alpha')).toBeVisible();
});
```

```tsx
it('promotes the next agent and keeps its controller usable when local cleanup fails after DELETE', async () => {
  const user = userEvent.setup();
  const first = snapshot('agent-first', 'First', 1);
  const next = snapshot('agent-next', 'Next', 2);
  let current = next;
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [next, first] });
  mockProviders();
  vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
  messagesFromSnapshot(() => current);
  const runAgent = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (_id, _text, _metadata, roomId) => {
      current = withMessage(next, 'Next is responsive', roomId);
      return {
        agent: current,
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Next is responsive' },
        },
      };
    });
  const removeItem = vi
    .spyOn(Storage.prototype, 'removeItem')
    .mockImplementation(() => {
      throw new DOMException('Storage access denied', 'SecurityError');
    });

  render(<ViewHarness />);
  await openChat();

  await screen.findByRole('heading', { name: 'Say something to First' });
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(screen.getByRole('button', { name: 'Reset' }));

  expect(
    await screen.findByRole('heading', { name: 'Say something to Next' }),
  ).toBeVisible();
  expect(removeItem).toHaveBeenCalledWith('animaos.checkins.agent-first');
  await user.type(screen.getByPlaceholderText('Message Next…'), 'Continue');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() =>
    expect(runAgent).toHaveBeenCalledWith(
      'agent-next',
      'Continue',
      expect.objectContaining({ clientRequestId: expect.any(String) }),
      'chat:new-1',
    ),
  );
  expect(await screen.findByText('Next is responsive')).toBeVisible();
});
```

```tsx
it('patches main identity, provider, model, system, and deliberate access while preserving its messages', async () => {
  const user = userEvent.setup();
  const nova = snapshot('agent-main', 'Nova', 1);
  nova.messages = [
    {
      id: 'message-1',
      agentId: 'agent-main',
      roomId: 'room-1',
      role: 'assistant',
      content: { text: 'Existing conversation' },
      createdAtMs: 2,
    },
  ];
  nova.messageCount = 1;
  const updated = structuredClone(nova);
  updated.state.name = 'Nova Prime';
  updated.state.config.name = 'Nova Prime';
  updated.state.config.provider = 'anthropic';
  updated.state.config.model = 'claude-sonnet-4-6';
  updated.state.config.system = 'Be concise';
  updated.state.config.tools = toolNamesForProfile('operate').map((tool) => ({
    name: tool,
    description: tool,
    parameters: {},
  }));
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-1', { title: 'Earlier chat', origin: 'api' }),
  );
  messagesFromSnapshot(() => nova);
  const updateAgent = vi
    .spyOn(daemon, 'updateAgent')
    .mockResolvedValue({ agent: updated });
  window.history.replaceState(null, '', '/#/s/room-1');

  render(<ViewHarness />);

  await screen.findByText('Existing conversation');
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  const name = screen.getByDisplayValue('Nova');
  await user.clear(name);
  await user.type(name, 'Nova Prime');
  const provider = screen.getByRole('combobox', { name: 'Provider' });
  const model = screen.getByRole('combobox', { name: 'Model' });
  await user.selectOptions(provider, 'anthropic');
  await user.selectOptions(model, 'claude-sonnet-4-6');
  const system = screen.getByPlaceholderText(
    'Leave empty for the daemon default.',
  );
  await user.clear(system);
  await user.type(system, 'Be concise');
  await user.click(screen.getByRole('radio', { name: /^Operate/ }));
  await user.click(screen.getByRole('button', { name: 'Save changes' }));

  expect(updateAgent).toHaveBeenCalledWith('agent-main', {
    name: 'Nova Prime',
    provider: 'anthropic',
    model: 'claude-sonnet-4-6',
    system: 'Be concise',
    tools: toolNamesForProfile('operate'),
  });
  expect(await screen.findByDisplayValue('Nova Prime')).toBeVisible();
  expect(screen.getByText('Existing conversation')).toBeVisible();
  expect(
    screen.getByRole('heading', {
      name: 'Nova Prime',
      exact: true,
      hidden: true,
    }),
  ).toBeVisible();
  expect(screen.getByRole('heading', { name: 'Agent settings' })).toBeVisible();
});
```

```tsx
it('keeps the full draft mounted through a deferred save failure, then allows close', async () => {
  const user = userEvent.setup();
  const nova = snapshot('agent-main', 'Nova', 1);
  nova.messages = [
    {
      id: 'message-1',
      agentId: 'agent-main',
      roomId: 'room-1',
      role: 'assistant',
      content: { text: 'Existing conversation' },
      createdAtMs: 2,
    },
  ];
  nova.messageCount = 1;
  const update = deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-1', { title: 'Earlier chat', origin: 'api' }),
  );
  messagesFromSnapshot(() => nova);
  const updateAgent = vi
    .spyOn(daemon, 'updateAgent')
    .mockReturnValue(update.promise);
  window.history.replaceState(null, '', '/#/s/room-1');

  render(<ViewHarness />);

  await screen.findByText('Existing conversation');
  expect(screen.getByText('Welcome back')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  const name = screen.getByDisplayValue('Nova');
  await user.clear(name);
  await user.type(name, 'Unsaved Nova');
  const provider = screen.getByRole('combobox', { name: 'Provider' });
  const model = screen.getByRole('combobox', { name: 'Model' });
  await user.selectOptions(provider, 'anthropic');
  await user.selectOptions(model, '__custom__');
  await user.type(
    screen.getByPlaceholderText('model id, e.g. llama3.1'),
    'anthropic/unsaved-model',
  );
  const system = screen.getByPlaceholderText(
    'Leave empty for the daemon default.',
  );
  await user.clear(system);
  await user.type(system, 'Unsaved system');
  await user.click(screen.getByRole('radio', { name: /^Operate/ }));
  await user.click(screen.getByRole('button', { name: 'Save changes' }));

  expect(updateAgent).toHaveBeenCalledWith('agent-main', {
    name: 'Unsaved Nova',
    provider: 'anthropic',
    model: 'anthropic/unsaved-model',
    system: 'Unsaved system',
    tools: toolNamesForProfile('operate'),
  });
  const close = screen.getByRole('button', { name: 'Close settings' });
  expect(close).toBeDisabled();
  expect(close).toHaveAccessibleDescription(/saving/i);
  await user.click(close);
  fireEvent.click(screen.getByTestId('settings-backdrop'));
  await user.keyboard('{Escape}');
  expect(screen.getByRole('heading', { name: 'Agent settings' })).toBeVisible();

  await act(async () => {
    update.reject(new Error('PATCH denied'));
    await update.promise.catch(() => undefined);
  });

  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent('PATCH denied');
  expect(alert).toHaveAttribute('aria-live', 'assertive');
  expect(alert).toHaveFocus();
  expect(screen.getByDisplayValue('Unsaved Nova')).toBeVisible();
  expect(screen.getByRole('combobox', { name: 'Provider' })).toHaveValue(
    'anthropic',
  );
  expect(screen.getByDisplayValue('anthropic/unsaved-model')).toBeVisible();
  expect(screen.getByDisplayValue('Unsaved system')).toBeVisible();
  expect(screen.getByRole('radio', { name: /^Operate/ })).toBeChecked();
  expect(screen.getByText('Welcome back')).toBeVisible();
  expect(screen.getByText('Existing conversation')).toBeVisible();
  expect(screen.getByRole('heading', { name: 'Agent settings' })).toBeVisible();
  expect(close).toBeEnabled();
  await user.click(close);
  expect(
    screen.queryByRole('heading', { name: 'Agent settings' }),
  ).not.toBeInTheDocument();
});
```

In `keeps the last-known shell after a late poll failure`, change `fireEvent.click(screen.getByRole('button', { name: 'Chat', exact: true }));` to `fireEvent.click(screen.getByRole('button', { name: 'New chat' }));`.

In `does not re-add the previous main when its pending run resolves after poll replacement`, replace the `expect(daemon.runAgent).toHaveBeenCalledWith(…, 'direct:agent-a');` statement with:

```ts
await waitFor(() =>
  expect(daemon.runAgent).toHaveBeenCalledWith(
    'agent-a',
    'Alpha work',
    expect.objectContaining({ clientRequestId: expect.any(String) }),
    'chat:new-1',
  ),
);
```

After the `describe` block, replace `reconciles a timed-out send with its saved request ID without offering a duplicate retry` and `keeps a timed-out running request locked until polling confirms its completion` with:

```tsx
it('reconciles a timed-out send with its saved request ID without offering a duplicate retry', async () => {
  const user = userEvent.setup();
  let current = snapshot('agent-main', 'Nova', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  messagesFromSnapshot(() => current);
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (id, text, metadata, roomId) => {
      current = withMessage(
        snapshot(id, 'Nova', 1),
        'Completed despite timeout',
        roomId,
      );
      current.state.status = 'completed';
      current.messages.unshift({
        id: 'request',
        agentId: id,
        roomId: roomId ?? '',
        role: 'user',
        content: { text, metadata },
        createdAtMs: 2,
      });
      throw Object.assign(new Error('daemon request failed (408)'), {
        status: 408,
      });
    });
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Commit',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await screen.findByText('Completed despite timeout');
  await waitFor(() =>
    expect(
      screen.queryByRole('button', { name: 'Restore message' }),
    ).not.toBeInTheDocument(),
  );
  expect(screen.queryByText(/response timed out/)).not.toBeInTheDocument();
  expect(run).toHaveBeenCalledTimes(1);
});
```

```tsx
it('keeps a timed-out running request locked until the daemon confirms its completion', async () => {
  const user = userEvent.setup();
  let current = snapshot('agent-main', 'Nova', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  messagesFromSnapshot(() => current);
  let requestMetadata: Record<string, unknown> | undefined;
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (_id, _text, metadata) => {
      requestMetadata = metadata;
      current = { ...current, state: { ...current.state, status: 'running' } };
      throw Object.assign(new Error('timeout'), { status: 408 });
    });
  render(<ViewHarness />);
  await openChat();
  const input = await screen.findByPlaceholderText('Message Nova…');
  await user.type(input, 'Long work');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await screen.findByText(/Checking the daemon for completion/);
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
  current = withMessage(
    snapshot('agent-main', 'Nova', 1),
    'Long work completed',
    'chat:new-1',
  );
  current.state.status = 'completed';
  current.messages.unshift({
    id: 'long-request',
    agentId: current.state.id,
    roomId: 'chat:new-1',
    role: 'user',
    content: { text: 'Long work', metadata: requestMetadata },
    createdAtMs: 2,
  });
  await screen.findByText('Long work completed', {}, { timeout: 5000 });
  await waitFor(() =>
    expect(
      screen.queryByText(/Checking the daemon for completion/),
    ).not.toBeInTheDocument(),
  );
  expect(run).toHaveBeenCalledTimes(1);
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
}, 10000);
```

(f) Append these new tests at the end of the file:

```tsx
it('opens an existing session from the sidebar, marks it read, and sends in its room', async () => {
  const user = userEvent.setup();
  let current = withMessage(
    snapshot('agent-main', 'Nova', 1),
    'Earlier answer',
    'room-7',
  );
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-7', {
      title: 'Weekend plans',
      origin: 'api',
      unread: true,
      lastActivityAtMs: Date.now(),
    }),
  );
  messagesFromSnapshot(() => current);
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (id, text, metadata, roomId) => {
      current = structuredClone(current);
      current.messages.push(
        {
          id: 'user-2',
          agentId: id,
          roomId: roomId ?? '',
          role: 'user',
          content: { text, metadata },
          createdAtMs: 3,
        },
        {
          id: 'reply-2',
          agentId: id,
          roomId: roomId ?? '',
          role: 'assistant',
          content: { text: 'Saturday works' },
          createdAtMs: 4,
        },
      );
      return {
        agent: current,
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Saturday works' },
        },
      };
    });
  render(<ViewHarness />);

  await user.click(
    await screen.findByRole('button', { name: 'Weekend plans, unread' }),
  );
  expect(await screen.findByText('Earlier answer')).toBeVisible();
  await waitFor(() =>
    expect(daemon.updateSession).toHaveBeenCalledWith('agent-main', 'room-7', {
      lastReadAtMs: 2,
    }),
  );
  await user.type(
    screen.getByPlaceholderText('Message Nova…'),
    'Does Saturday work?',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(run).toHaveBeenCalledWith(
    'agent-main',
    'Does Saturday work?',
    expect.objectContaining({ clientRequestId: expect.any(String) }),
    'room-7',
  );
  expect(daemon.createSession).not.toHaveBeenCalled();
  expect(await screen.findByText('Saturday works')).toBeVisible();
});

it('replies to a Telegram session through its connector', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  vi.spyOn(daemon, 'listConnectors').mockResolvedValue({
    connectors: [
      {
        id: 'tg-1',
        agentId: 'agent-main',
        roomId: 'telegram:tg-1',
        type: 'telegram',
        bot: { id: '1', username: 'nova_bot', displayName: 'Nova' },
        approvedChat: null,
        pendingPairing: null,
        status: 'ready',
        enabled: true,
        createdAtMs: 1,
        updatedAtMs: 1,
      },
    ],
  });
  routes.sessions.push(
    sessionFixture('telegram:tg-1', {
      kind: 'telegram',
      origin: 'telegram',
      title: 'Telegram · @nova_bot',
      lastActivityAtMs: Date.now(),
    }),
  );
  const runAgent = vi.spyOn(daemon, 'runAgent');
  const reply = vi.spyOn(daemon, 'sendConnectorMessage').mockResolvedValue({
    messages: [],
    result: { status: 'success', durationMs: 1 },
    deliveryQueued: true,
  });
  window.history.replaceState(null, '', '/#/s/telegram%3Atg-1');
  render(<ViewHarness />);

  await user.type(
    await screen.findByPlaceholderText('Reply on Telegram…'),
    'On my way',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(reply).toHaveBeenCalledWith(
    'agent-main',
    'tg-1',
    'On my way',
    expect.stringMatching(/^telegram-/),
  );
  expect(runAgent).not.toHaveBeenCalled();
});

it('returns to a new chat when the open session is deleted from the sidebar', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:old', {
      title: 'Old plan',
      lastActivityAtMs: Date.now(),
    }),
  );
  window.history.replaceState(null, '', '/#/s/chat%3Aold');
  render(<ViewHarness />);

  await user.click(
    await screen.findByRole('button', { name: 'Actions for Old plan' }),
  );
  await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
  await user.click(screen.getByRole('menuitem', { name: 'Delete session' }));

  await waitFor(() =>
    expect(daemon.deleteSession).toHaveBeenCalledWith('agent-main', 'chat:old'),
  );
  expect(
    await screen.findByRole('heading', { name: 'Say something to Nova' }),
  ).toBeVisible();
  expect(window.location.hash).toBe('#/');
});
```

Every other test in the file is unchanged: `opens the main companion without automatically executing a prepared assignment`, `uploads a workspace avatar and refreshes daemon-owned workspace state`, `imports legacy prompts into the daemon without starting a browser execution timer`, `makes the workspace inert while settings are open and restores trigger focus on close`, `renders neutral connecting copy for unknown connection state and never claims connected`, `renders a focused offline retry state with the rust host command and no onboarding or navigation`, `renders only onboarding when the online daemon has zero agents`, `promotes the next-oldest agent after deleting Main and reloads its workspace`, `locks the settings transaction and ignores Reset until a deferred PATCH is adopted`, `locks settings during reset and rejects a forced save until DELETE settles`, `recovers a failed message without overwriting a newer draft or sending automatically`, `keeps every failed message until explicitly restored or dismissed`, `recovers a send failure even when settings were saved during the request`, `does not surface a pre-existing workspace error as a settings failure`, `keeps the full draft mounted through a deferred reset failure, then allows close`, `returns to onboarding after deleting the final agent`, `returns to onboarding when local cleanup fails after the final DELETE`, `keeps reset authoritative when an older poll resolves after deletion`, `does not re-add the previous main when its pending PATCH resolves after poll replacement`, `does not mistake an older identical message for the timed-out request`, and `does not clear a newer recovery entry when an older identical send is confirmed`. (`openChat` now clicks New chat, and the first send of each creates `chat:new-1` through the mocked session routes.)

Run (from `apps/web`): `bun x vitest run src/ViewHarness.test.tsx`
Expected: FAIL — `daemon.getSession` does not exist (`vi.spyOn` throws), and the old harness still sends to `direct:<agentId>`.

- [ ] **Step 4: Rewrite the harness**

In `apps/web/src/lib/daemon-api.ts`, add after `listSessions` in the `daemon` object:

```ts
  getSession: (agentId: string, sessionId: string) =>
    setupClient.sessions.get(agentId, sessionId),
```

Replace `apps/web/src/ViewHarness.tsx` with:

```tsx
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { Session } from '@animaOS-SWARM/sdk';

import { AlertIcon } from './components/icons';
import { CompanionSetup } from './components/onboarding/CompanionSetup';
import { SettingsPanel } from './components/SettingsPanel';
import { ConnectorsView } from './components/ConnectorsView';
import { SessionSidebar } from './components/sessions/SessionSidebar';
import { SessionView } from './components/sessions/SessionView';
import { TelegramSettings } from './components/TelegramSettings';
import { WorkspaceShell } from './components/WorkspaceShell';
import { useAgentIntegrations } from './hooks/useAgentIntegrations';
import { useCompanionSessions } from './hooks/useCompanionSessions';
import { useDaemonBootstrap } from './hooks/useDaemonBootstrap';
import { useSessionMessages } from './hooks/useSessionMessages';
import { clearCheckins, importLegacyCheckins } from './lib/checkins';
import {
  daemon,
  toAgentDetail,
  toChatMessage,
  type AgentUpdateInput,
  type DaemonSnapshot,
} from './lib/daemon-api';
import { selectMainAgent } from './lib/agent-access';
import { useHashRoute, type HashRoute } from './lib/hash-route';
import { exportFileName, sessionKey } from './lib/session-groups';
import {
  createTelegramIdempotencyKey,
  safeIntegrationError,
} from './lib/telegram';

interface AgentOperation {
  generation: number;
  lifecycleGeneration: number;
  targetAgentId: string;
}

type ChatState = {
  draft: string;
  failedDrafts: { requestId: string; text: string }[];
  sending: boolean;
  error: string | null;
};

const EMPTY_CHAT: ChatState = {
  draft: '',
  failedDrafts: [],
  sending: false,
  error: null,
};
const HOME_CONVERSATION = 'home';

/** Chat state is kept per agent and conversation (`home` or `session:<id>`). */
function chatKey(agentId: string, conversation: string): string {
  return `${agentId}\u0000${conversation}`;
}

function sessionConversation(sessionId: string): string {
  return `session:${sessionId}`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function saveTextFile(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/markdown' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

function ConnectingState() {
  return (
    <main
      className="relative z-[1] flex min-h-0 flex-1 items-center justify-center"
      aria-live="polite"
      aria-busy="true"
    >
      <div className="flex flex-col items-center gap-3 text-center">
        <div className="flex items-center gap-1.5" aria-hidden>
          {[0, 1, 2].map((index) => (
            <span
              key={index}
              className="typing-dot h-2 w-2 rounded-full bg-ink-3"
              style={{ animationDelay: `${index * 150}ms` }}
            />
          ))}
        </div>
        <p className="font-display text-sm font-medium text-ink">
          Connecting to anima-daemon…
        </p>
        <p className="font-mono text-[11px] text-ink-3">
          Checking daemon availability
        </p>
      </div>
    </main>
  );
}

function OfflineRetry({ retry }: { retry: () => Promise<void> }) {
  return (
    <main className="relative z-[1] flex min-h-0 flex-1 items-center justify-center px-5">
      <section
        role="alert"
        className="glass-strong w-full max-w-lg rounded-3xl p-7 text-center sm:p-9"
      >
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-danger/10 text-danger">
          <AlertIcon size={20} />
        </div>
        <h1 className="mt-4 font-display text-2xl font-semibold tracking-tight text-ink">
          Offline
        </h1>
        <p className="mx-auto mt-2 max-w-sm text-sm leading-relaxed text-ink-2">
          The workspace cannot reach anima-daemon yet. Start the Rust host, then
          retry this connection.
        </p>
        <code className="mt-4 inline-block rounded-xl border border-line bg-abyss/60 px-3 py-2 font-mono text-xs text-mint">
          bun dev --host rust
        </code>
        <div className="mt-5">
          <button
            type="button"
            autoFocus
            onClick={() => void retry()}
            className="rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-accent-fg shadow-lg shadow-accent/20 transition hover:bg-accent/90"
          >
            Retry connection
          </button>
        </div>
      </section>
    </main>
  );
}

export function ViewHarness() {
  const {
    connection,
    loaded,
    agents: agentSnapshots,
    providers,
    providersError,
    workspace,
    refreshAgents,
    retryProviders,
    refreshWorkspace,
    acceptAgentSnapshot,
    removeAgentSnapshot,
  } = useDaemonBootstrap();
  const agents = useMemo(
    () => agentSnapshots.map((snapshot) => toAgentDetail(snapshot)),
    [agentSnapshots],
  );
  const mainAgent = selectMainAgent(agents);
  // Helpers are implementation details, never a second top-level persona.
  const agent = mainAgent;
  const agentId = agent?.id ?? null;
  const availableAgentIdsRef = useRef(new Set<string>());
  availableAgentIdsRef.current = new Set(agents.map((item) => item.id));
  const [route, navigate] = useHashRoute();
  // A page hides the conversation but keeps it: the last chat or session stays loaded.
  const lastConversationRef = useRef<HashRoute>({ kind: 'home' });
  if (route.kind !== 'page') lastConversationRef.current = route;
  const conversationRoute = lastConversationRef.current;

  const [sessionQuery, setSessionQuery] = useState('');
  const [showArchived, setShowArchived] = useState(false);
  const [sessionActionError, setSessionActionError] = useState<string | null>(
    null,
  );
  const sessions = useCompanionSessions(agentId, {
    archived: showArchived,
    query: sessionQuery,
  });
  const routeSessionId =
    conversationRoute.kind === 'session' ? conversationRoute.sessionId : null;
  const listedSession = routeSessionId
    ? (sessions.sessions.find((item) => item.id === routeSessionId) ?? null)
    : null;
  const sessionListed = listedSession !== null;
  // A session outside the loaded list (archived or older) is read on its own.
  const [fetchedSession, setFetchedSession] = useState<Session | null>(null);
  useEffect(() => {
    if (!routeSessionId || sessionListed || !agentId) return;
    let active = true;
    void daemon.getSession(agentId, routeSessionId).then(
      (session) => {
        if (active) setFetchedSession(session);
      },
      () => undefined,
    );
    return () => {
      active = false;
    };
  }, [agentId, routeSessionId, sessionListed]);
  const activeSession =
    listedSession ??
    (fetchedSession && fetchedSession.id === routeSessionId
      ? fetchedSession
      : null);
  const [messagesRefresh, setMessagesRefresh] = useState(0);
  const history = useSessionMessages(
    routeSessionId ? (activeSession?.agentId ?? agentId) : null,
    routeSessionId,
    messagesRefresh,
  );
  const chatMessages = useMemo(
    () => history.messages.map(toChatMessage),
    [history.messages],
  );

  const conversation = routeSessionId
    ? sessionConversation(routeSessionId)
    : HOME_CONVERSATION;
  const activeChatKey = agentId ? chatKey(agentId, conversation) : null;
  const [chats, setChats] = useState<Record<string, ChatState>>({});
  const chat = (activeChatKey ? chats[activeChatKey] : undefined) ?? EMPTY_CHAT;
  const { draft, failedDrafts, sending, error: workspaceError } = chat;
  const failedDraft = failedDrafts[0]?.text ?? null;
  const updateChat = useCallback(
    (
      key: string,
      patch: Partial<ChatState> | ((value: ChatState) => Partial<ChatState>),
    ) => {
      setChats((current) => {
        const value = current[key] ?? EMPTY_CHAT;
        return {
          ...current,
          [key]: {
            ...value,
            ...(typeof patch === 'function' ? patch(value) : patch),
          },
        };
      });
    },
    [],
  );
  const setDraft = useCallback(
    (value: string | ((current: string) => string)) => {
      if (activeChatKey)
        updateChat(activeChatKey, (current) => ({
          draft: typeof value === 'function' ? value(current.draft) : value,
        }));
    },
    [activeChatKey, updateChat],
  );
  const setFailedDrafts = (
    value: (current: ChatState['failedDrafts']) => ChatState['failedDrafts'],
  ) => {
    if (activeChatKey)
      updateChat(activeChatKey, (current) => ({
        failedDrafts: value(current.failedDrafts),
      }));
  };
  const setWorkspaceError = (error: string | null) => {
    if (activeChatKey) updateChat(activeChatKey, { error });
  };
  const pendingSendsRef = useRef(new Set<string>());
  const uncertainSendsRef = useRef(
    new Map<
      string,
      { agentId: string; key: string; text: string; waiting: boolean }
    >(),
  );
  useEffect(() => {
    for (const [requestId, pending] of uncertainSendsRef.current) {
      const snapshot = agentSnapshots.find(
        (item) => item.state.id === pending.agentId,
      );
      if (!snapshot) continue;
      // A committed user message means its blocking run finished (M2 runs
      // commit their messages together).
      const delivered =
        snapshot.messages.some(
          (message) =>
            message.role === 'user' &&
            message.content.metadata?.clientRequestId === requestId,
        ) ||
        history.messages.some(
          (message) =>
            message.role === 'user' &&
            message.metadata.clientRequestId === requestId,
        );
      const running = snapshot.state.status === 'running';
      if (!delivered && (!pending.waiting || running)) continue;
      if (delivered) uncertainSendsRef.current.delete(requestId);
      else
        uncertainSendsRef.current.set(requestId, {
          ...pending,
          waiting: false,
        });
      if (pending.waiting) pendingSendsRef.current.delete(pending.key);
      updateChat(pending.key, (current) => {
        const index = delivered
          ? current.failedDrafts.findIndex(
              (item) => item.requestId === requestId,
            )
          : -1;
        const remaining = current.failedDrafts.filter(
          (_, position) => position !== index,
        );
        return {
          failedDrafts: remaining,
          sending: pending.waiting ? false : current.sending,
          error:
            current.sending && !pending.waiting
              ? current.error
              : delivered
                ? snapshot.state.status === 'failed'
                  ? 'The agent run failed. Check the conversation for details.'
                  : remaining.length
                    ? current.error
                    : null
                : 'The daemon has not confirmed this message. Check the conversation before restoring it.',
        };
      });
      if (delivered) setMessagesRefresh((value) => value + 1);
    }
  }, [agentSnapshots, history.messages, updateChat]);
  const [settingsSaveError, setSettingsSaveError] = useState<string | null>(
    null,
  );
  const [resetError, setResetError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [savingSettings, setSavingSettings] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [legacyMigrationError, setLegacyMigrationError] = useState<
    string | null
  >(null);

  const savingSettingsRef = useRef(false);
  const agentOperationGenerationRef = useRef(0);
  const agentLifecycleGenerationRef = useRef(0);
  const settingsOperationGenerationRef = useRef<number | null>(null);
  const resetInFlightRef = useRef<AgentOperation | null>(null);
  const settingsTriggerRef = useRef<HTMLElement | null>(null);
  const currentAgentIdRef = useRef<string | null>(null);
  const previousSelectedMainIdRef = useRef<string | null>(null);

  const integrations = useAgentIntegrations(agentId);
  const telegramConnector = integrations.connectors[0] ?? null;
  const activeConnector =
    activeSession?.kind === 'telegram'
      ? (integrations.connectors.find(
          (item) => item.roomId === activeSession.roomId,
        ) ?? null)
      : null;
  useLayoutEffect(() => {
    if (previousSelectedMainIdRef.current === agentId) return;

    const previousAgentId = previousSelectedMainIdRef.current;
    previousSelectedMainIdRef.current = agentId;
    agentLifecycleGenerationRef.current += 1;
    agentOperationGenerationRef.current += 1;
    currentAgentIdRef.current = agentId;
    settingsOperationGenerationRef.current = null;
    resetInFlightRef.current = null;
    settingsTriggerRef.current = null;
    savingSettingsRef.current = false;

    setLegacyMigrationError(null);
    setSessionActionError(null);
    setSettingsSaveError(null);
    setResetError(null);
    setShowSettings(false);
    setSavingSettings(false);
    setResetting(false);
    // Another companion has other sessions: start from a new chat.
    if (previousAgentId !== null) navigate({ kind: 'home' }, { replace: true });
  }, [agentId, navigate]);

  const beginAgentOperation = useCallback(
    (targetAgentId: string): AgentOperation => ({
      generation: ++agentOperationGenerationRef.current,
      lifecycleGeneration: agentLifecycleGenerationRef.current,
      targetAgentId,
    }),
    [],
  );

  const isCurrentAgentOperation = useCallback(
    (operation: AgentOperation) =>
      operation.generation === agentOperationGenerationRef.current &&
      operation.lifecycleGeneration === agentLifecycleGenerationRef.current &&
      operation.targetAgentId === currentAgentIdRef.current,
    [],
  );

  const isCurrentResetOperation = useCallback(
    (operation: AgentOperation) =>
      resetInFlightRef.current === operation &&
      operation.lifecycleGeneration === agentLifecycleGenerationRef.current &&
      operation.targetAgentId === currentAgentIdRef.current,
    [],
  );

  const adoptAgentSnapshot = useCallback(
    (operation: AgentOperation, snapshot: DaemonSnapshot) => {
      if (
        !isCurrentAgentOperation(operation) ||
        snapshot.state.id !== operation.targetAgentId
      ) {
        return false;
      }

      acceptAgentSnapshot(snapshot);
      return true;
    },
    [acceptAgentSnapshot, isCurrentAgentOperation],
  );

  const scrollerRef = useRef<HTMLDivElement>(null);
  const scrollDown = () => {
    requestAnimationFrame(() => {
      const element = scrollerRef.current;
      if (element) element.scrollTop = element.scrollHeight;
    });
  };

  useEffect(() => {
    if (!agentId) return;
    let current = true;
    void importLegacyCheckins(agentId)
      .then((result) => {
        if (!current) return;
        if (result.malformed > 0) {
          setLegacyMigrationError(
            `${result.malformed} legacy check-in record could not be imported and was kept in this browser.`,
          );
        } else if (result.imported > 0) {
          setLegacyMigrationError(null);
          void integrations.refresh();
        }
      })
      .catch(() => {
        if (current)
          setLegacyMigrationError(
            'Legacy check-ins could not be imported. They remain in this browser for retry.',
          );
      });
    return () => {
      current = false;
    };
  }, [agentId]);

  // Opening an unread session marks it read up to its newest message.
  const markedReadRef = useRef(new Map<string, number>());
  const refreshSessions = sessions.refresh;
  useEffect(() => {
    if (!activeSession?.unread || history.messages.length === 0) return;
    const newest = history.messages[history.messages.length - 1].createdAtMs;
    const key = sessionKey(activeSession);
    if ((markedReadRef.current.get(key) ?? 0) >= newest) return;
    markedReadRef.current.set(key, newest);
    void daemon
      .updateSession(activeSession.agentId, activeSession.id, {
        lastReadAtMs: newest,
      })
      .then(
        () => refreshSessions(),
        () => markedReadRef.current.delete(key),
      );
  }, [activeSession, history.messages, refreshSessions]);

  const changeWorkspaceAvatar = useCallback(
    async (file: File) => {
      await daemon.uploadWorkspaceAvatar(file);
      await refreshWorkspace();
    },
    [refreshWorkspace],
  );

  const openSettings = () => {
    settingsTriggerRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setShowSettings(true);
  };
  const closeSettings = () => {
    if (savingSettingsRef.current || resetInFlightRef.current !== null) return;
    setShowSettings(false);
  };

  useEffect(() => {
    if (showSettings || !settingsTriggerRef.current) return;
    const trigger = settingsTriggerRef.current;
    settingsTriggerRef.current = null;
    trigger.focus();
  }, [showSettings]);

  const saveSettings = async (patch: AgentUpdateInput): Promise<boolean> => {
    if (
      !agent ||
      savingSettingsRef.current ||
      resetInFlightRef.current !== null
    ) {
      return false;
    }
    const operation = beginAgentOperation(agent.id);
    settingsOperationGenerationRef.current = operation.generation;
    savingSettingsRef.current = true;
    setSavingSettings(true);
    setSettingsSaveError(null);
    setResetError(null);
    try {
      const { agent: updatedAgent } = await daemon.updateAgent(agent.id, patch);
      const adopted = adoptAgentSnapshot(operation, updatedAgent);
      if (adopted) {
        setSettingsSaveError(null);
      }
      return adopted;
    } catch (caught) {
      if (isCurrentAgentOperation(operation)) {
        setSettingsSaveError(errorMessage(caught));
      }
      return false;
    } finally {
      if (settingsOperationGenerationRef.current === operation.generation) {
        settingsOperationGenerationRef.current = null;
        savingSettingsRef.current = false;
        setSavingSettings(false);
      }
    }
  };

  const resetAgent = async () => {
    if (
      !agent ||
      savingSettingsRef.current ||
      resetInFlightRef.current !== null
    ) {
      return;
    }
    const targetAgentId = agent.id;
    const operation = beginAgentOperation(targetAgentId);
    resetInFlightRef.current = operation;
    setResetting(true);
    setResetError(null);
    setSettingsSaveError(null);
    try {
      try {
        await daemon.deleteAgent(targetAgentId);
      } catch (caught) {
        if (isCurrentResetOperation(operation)) {
          setResetError(errorMessage(caught));
        }
        return;
      }

      const ownsSelectedMain = isCurrentResetOperation(operation);
      if (ownsSelectedMain) {
        agentLifecycleGenerationRef.current += 1;
        agentOperationGenerationRef.current += 1;
        currentAgentIdRef.current = null;
      }
      availableAgentIdsRef.current.delete(targetAgentId);
      removeAgentSnapshot(targetAgentId);
      try {
        clearCheckins(targetAgentId);
      } catch {
        // The daemon deletion is authoritative; local cleanup is best-effort.
      }
    } finally {
      if (resetInFlightRef.current === operation) {
        resetInFlightRef.current = null;
        setResetting(false);
      }
    }
  };

  const refreshConversation = () => {
    setMessagesRefresh((value) => value + 1);
    void sessions.refresh();
  };

  /** One blocking run in a session's room (spec §4.9). */
  const runInSession = async (
    targetId: string,
    roomId: string,
    text: string,
    key: string,
    preserveDraft = false,
  ) => {
    if (
      !availableAgentIdsRef.current.has(targetId) ||
      connection !== 'online' ||
      pendingSendsRef.current.has(key) ||
      resetInFlightRef.current !== null
    )
      return;
    const clientRequestId = crypto.randomUUID();
    pendingSendsRef.current.add(key);
    updateChat(key, {
      sending: true,
      error: null,
      ...(preserveDraft ? {} : { draft: '' }),
    });
    try {
      const { agent: updatedAgent, result } = await daemon.runAgent(
        targetId,
        text,
        { clientRequestId },
        roomId,
      );
      if (
        availableAgentIdsRef.current.has(targetId) &&
        updatedAgent.state.id === targetId
      ) {
        acceptAgentSnapshot(updatedAgent);
        if (result.status === 'error')
          updateChat(key, { error: result.error ?? 'run failed' });
      }
    } catch (caught) {
      if (availableAgentIdsRef.current.has(targetId)) {
        const timedOut =
          caught instanceof Error &&
          'status' in caught &&
          caught.status === 408;
        uncertainSendsRef.current.set(clientRequestId, {
          agentId: targetId,
          key,
          text,
          waiting: timedOut,
        });
        updateChat(key, (current) => ({
          failedDrafts: [
            ...current.failedDrafts,
            { requestId: clientRequestId, text },
          ],
          error: timedOut
            ? 'The response timed out. Checking the daemon for completion—do not resend yet.'
            : errorMessage(caught),
        }));
        if (timedOut) void refreshAgents();
      }
    } finally {
      if (!uncertainSendsRef.current.get(clientRequestId)?.waiting) {
        pendingSendsRef.current.delete(key);
        updateChat(key, { sending: false });
      }
      refreshConversation();
    }
  };

  /** A new chat becomes a session with its first message (spec §3.3). */
  const startChat = async (targetId: string, text: string) => {
    const homeKey = chatKey(targetId, HOME_CONVERSATION);
    if (pendingSendsRef.current.has(homeKey)) return;
    pendingSendsRef.current.add(homeKey);
    updateChat(homeKey, { sending: true, error: null, draft: '' });
    let session: Session;
    try {
      session = await daemon.createSession(targetId);
    } catch (caught) {
      pendingSendsRef.current.delete(homeKey);
      updateChat(homeKey, (current) => ({
        sending: false,
        failedDrafts: [
          ...current.failedDrafts,
          { requestId: crypto.randomUUID(), text },
        ],
        error: errorMessage(caught),
      }));
      return;
    }
    pendingSendsRef.current.delete(homeKey);
    if (currentAgentIdRef.current !== targetId) {
      updateChat(homeKey, { sending: false });
      return;
    }
    const target = chatKey(targetId, sessionConversation(session.id));
    // Text typed while the chat was created moves with it.
    setChats((current) => {
      const home = current[homeKey] ?? EMPTY_CHAT;
      return {
        ...current,
        [homeKey]: { ...home, draft: '', sending: false },
        [target]: { ...(current[target] ?? EMPTY_CHAT), draft: home.draft },
      };
    });
    sessions.upsert(session);
    navigate({ kind: 'session', sessionId: session.id }, { replace: true });
    await runInSession(targetId, session.roomId, text, target, true);
  };

  /** An owner turn in a Telegram session goes out through its connector. */
  const replyOnTelegram = async (
    targetId: string,
    connectorId: string,
    text: string,
    key: string,
  ) => {
    if (pendingSendsRef.current.has(key)) return;
    pendingSendsRef.current.add(key);
    updateChat(key, { sending: true, error: null, draft: '' });
    try {
      const response = await daemon.sendConnectorMessage(
        targetId,
        connectorId,
        text,
        createTelegramIdempotencyKey(),
      );
      if (response.result.status === 'error')
        updateChat(key, { error: response.result.error ?? 'run failed' });
    } catch (caught) {
      updateChat(key, (current) => ({
        failedDrafts: [
          ...current.failedDrafts,
          { requestId: crypto.randomUUID(), text },
        ],
        error: safeIntegrationError(caught),
      }));
    } finally {
      pendingSendsRef.current.delete(key);
      updateChat(key, { sending: false });
      refreshConversation();
    }
  };

  const send = () => {
    if (!agent || connection !== 'online' || resetInFlightRef.current !== null)
      return;
    const text = draft.trim();
    if (!text) return;
    if (!routeSessionId) {
      void startChat(agent.id, text);
      return;
    }
    const key = chatKey(agent.id, sessionConversation(routeSessionId));
    if (activeSession?.kind === 'telegram') {
      if (activeConnector)
        void replyOnTelegram(agent.id, activeConnector.id, text, key);
      return;
    }
    void runInSession(
      activeSession?.agentId ?? agent.id,
      activeSession?.roomId ?? routeSessionId,
      text,
      key,
    );
  };

  const newChat = () => navigate({ kind: 'home' });
  const openSession = (session: Session) =>
    navigate({ kind: 'session', sessionId: session.id });
  const renameSession = async (session: Session, title: string) => {
    try {
      await daemon.updateSession(session.agentId, session.id, { title });
      setSessionActionError(null);
      await sessions.refresh();
      return true;
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
      return false;
    }
  };
  const archiveSession = async (session: Session, archived: boolean) => {
    try {
      await daemon.updateSession(session.agentId, session.id, { archived });
      setSessionActionError(null);
      await sessions.refresh();
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
    }
  };
  const exportSession = async (session: Session) => {
    try {
      saveTextFile(
        exportFileName(session.title),
        await daemon.exportSession(session.agentId, session.id),
      );
      setSessionActionError(null);
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
    }
  };
  const deleteSession = async (session: Session) => {
    try {
      await daemon.deleteSession(session.agentId, session.id);
      setSessionActionError(null);
      sessions.remove(session);
      if (routeSessionId === session.id) {
        if (route.kind === 'page')
          lastConversationRef.current = { kind: 'home' };
        else navigate({ kind: 'home' }, { replace: true });
      }
    } catch (caught) {
      setSessionActionError(errorMessage(caught));
    }
  };

  if (connection === 'unknown' || (connection === 'online' && !loaded)) {
    return <ConnectingState />;
  }

  if (!agent && connection === 'offline') {
    return <OfflineRetry retry={refreshAgents} />;
  }

  const onboardingLifecycleGeneration = agentLifecycleGenerationRef.current;
  if (!agent) {
    return (
      <CompanionSetup
        providers={providers}
        providersError={providersError}
        retryProviders={retryProviders}
        onCreated={(snapshot) => {
          if (
            currentAgentIdRef.current !== null ||
            agentLifecycleGenerationRef.current !==
              onboardingLifecycleGeneration
          ) {
            return;
          }
          agentOperationGenerationRef.current += 1;
          agentLifecycleGenerationRef.current += 1;
          acceptAgentSnapshot(snapshot);
          void refreshWorkspace();
          scrollDown();
        }}
      />
    );
  }

  const settingsPanel = showSettings ? (
    <SettingsPanel
      refreshProviders={retryProviders}
      agent={agent}
      providers={providers}
      workspace={workspace}
      saving={savingSettings}
      resetting={resetting}
      saveError={settingsSaveError}
      resetError={resetError}
      saveSettings={saveSettings}
      resetAgent={resetAgent}
      close={closeSettings}
    />
  ) : null;

  const sidebar = (
    <SessionSidebar
      sessions={sessions.sessions}
      activeKey={activeSession ? sessionKey(activeSession) : null}
      query={sessionQuery}
      onQueryChange={setSessionQuery}
      showArchived={showArchived}
      onShowArchivedChange={setShowArchived}
      error={sessionActionError ?? sessions.error}
      onOpen={openSession}
      onRename={renameSession}
      onArchive={archiveSession}
      onExport={exportSession}
      onDelete={deleteSession}
    />
  );

  const sessionView = (
    <SessionView
      agent={agent}
      session={activeSession}
      messages={routeSessionId ? chatMessages : []}
      hasOlder={history.hasOlder}
      loadingOlder={history.loadingOlder}
      onLoadOlder={() => void history.loadOlder()}
      missing={routeSessionId !== null && history.missing}
      telegramAvailable={activeConnector !== null}
      scrollerRef={scrollerRef}
      onSuggestion={setDraft}
      composer={{
        draft,
        setDraft,
        sending,
        disabled: resetting || (activeSession?.activeRuns ?? 0) > 0,
        offline: connection === 'offline',
        onSend: send,
        error: workspaceError,
        onDismissError: () => setWorkspaceError(null),
        recovery:
          failedDraft && !sending
            ? {
                count: failedDrafts.length,
                text: failedDraft,
                restore: () => {
                  setDraft((current) =>
                    current.trim()
                      ? `${current}\n\n${failedDraft}`
                      : failedDraft,
                  );
                  setFailedDrafts((current) => current.slice(1));
                },
                dismiss: () => setFailedDrafts((current) => current.slice(1)),
              }
            : undefined,
      }}
      onNewChat={newChat}
      onOpenWork={() => navigate({ kind: 'page', page: 'work' })}
      onRename={(title) =>
        activeSession
          ? renameSession(activeSession, title)
          : Promise.resolve(false)
      }
      onToggleArchived={() => {
        if (activeSession)
          void archiveSession(activeSession, !activeSession.archived);
      }}
      onExport={() => {
        if (activeSession) void exportSession(activeSession);
      }}
      notice={
        legacyMigrationError ? (
          <p role="status" className="px-4 pt-3 text-xs text-ink-3">
            {legacyMigrationError}
          </p>
        ) : null
      }
    />
  );

  return (
    <>
      <div
        data-testid="workspace-background"
        className="contents"
        aria-hidden={showSettings || undefined}
        inert={showSettings || undefined}
      >
        <WorkspaceShell
          mainAgent={mainAgent ?? agent}
          agents={agents}
          connection={connection}
          route={route}
          navigate={navigate}
          onNewChat={newChat}
          onOpenSettings={openSettings}
          onChangeWorkspaceAvatar={changeWorkspaceAvatar}
          onPickPrompt={(prompt) =>
            setDraft((current) =>
              current.trim() ? `${current}\n\n${prompt}` : prompt,
            )
          }
          connectors={
            <ConnectorsView
              agentId={agent.id}
              telegram={
                <TelegramSettings
                  connector={telegramConnector}
                  busy={integrations.connectorBusy}
                  error={integrations.connectorError}
                  connect={integrations.connectTelegram}
                  replace={integrations.replaceTelegram}
                  approve={integrations.approvePairing}
                  restart={integrations.restartTelegram}
                  disconnect={integrations.disconnectTelegram}
                  refresh={integrations.refresh}
                />
              }
            />
          }
          workspaceState={workspace}
          sidebar={sidebar}
          conversation={sessionView}
        />
      </div>
      {settingsPanel}
    </>
  );
}
```

Delete the replaced views (this also stages the deletions for Step 7): `git rm apps/web/src/components/ActivityView.tsx apps/web/src/components/CheckinsView.tsx apps/web/src/components/CheckinsView.test.tsx apps/web/src/components/TelegramThread.tsx apps/web/src/components/TelegramThread.test.tsx`.

Run: `grep -rn "ActivityView\|CheckinsView\|TelegramThread\|direct:\${" apps/web/src`
Expected: no output.

- [ ] **Step 5: Update the e2e fixtures (they run with M10's gate, not this one)**

In `apps/web-e2e/src/companion.spec.ts`:

- add after `let sent = 0;`:

```ts
const chatSession = {
  id: 'chat:e2e',
  agentId: 'companion',
  roomId: 'chat:e2e',
  kind: 'chat',
  origin: 'web',
  title: 'Help me plan my day',
  titleSource: 'first_message',
  createdAtMs: 2,
  lastActivityAtMs: 3,
  lastReadAtMs: null,
  archived: false,
  parentSessionId: null,
  parentRunId: null,
  parentAgentId: null,
  summary: null,
  contextTrimmed: null,
  messageCount: 0,
  preview: null,
  activeRuns: 0,
  pendingApprovals: 0,
  unread: false,
  capabilities: {
    send: true,
    steer: true,
    stop: true,
    rename: true,
    archive: true,
    delete: true,
    compact: true,
    export: true,
  },
};
let created = false;
```

- insert before `else if (path.endsWith('/run')) {`:

```ts
    else if (path === '/api/agents/companion/sessions') {
      if (route.request().method() === 'POST') { created = true; body = { session: chatSession }; }
      else body = { sessions: created ? [chatSession] : [], nextCursor: null };
    } else if (/^\/api\/agents\/companion\/sessions\/[^/]+\/messages$/.test(path)) {
      body = { messages: (agent.messages as { id: string; role: string; content: { text: string }; createdAtMs: number }[]).map(m => ({ id: m.id, role: m.role, text: m.content.text, attachments: [], metadata: {}, createdAtMs: m.createdAtMs })), nextBefore: null };
    } else if (/^\/api\/agents\/companion\/sessions\/[^/]+$/.test(path)) body = { session: chatSession };
```

- in the `/run` branch, change both `roomId: 'direct:companion'` to `roomId: route.request().postDataJSON().roomId`;
- in the viewport test, replace `await page.getByRole('button', { name: 'Activity', exact: true }).click();` with `await page.getByRole('button', { name: 'Work', exact: true }).click();` and `await page.getByRole('button', { name: 'Chat', exact: true }).click();` with `await page.getByRole('button', { name: 'Open companion chat' }).click();`.

In `apps/web-e2e/src/independent-agents.spec.ts`:

- add before `await page.route('**/api/**', …)`:

```ts
const managerChat = {
  id: 'chat:e2e',
  agentId: 'manager',
  roomId: 'chat:e2e',
  kind: 'chat',
  origin: 'web',
  title: 'Companion draft stays here',
  titleSource: 'first_message',
  createdAtMs: 10,
  lastActivityAtMs: 12,
  lastReadAtMs: null,
  archived: false,
  parentSessionId: null,
  parentRunId: null,
  parentAgentId: null,
  summary: null,
  contextTrimmed: null,
  messageCount: 0,
  preview: null,
  activeRuns: 0,
  pendingApprovals: 0,
  unread: false,
  capabilities: {
    send: true,
    steer: true,
    stop: true,
    rename: true,
    archive: true,
    delete: true,
    compact: true,
    export: true,
  },
};
let managerChatCreated = false;
```

- insert directly after `if (path === '/agents') return json(route, { agents });`:

```ts
if (path === '/agents/manager/sessions') {
  if (request.method() === 'POST') {
    managerChatCreated = true;
    return json(route, { session: managerChat });
  }
  return json(route, {
    sessions: managerChatCreated ? [managerChat] : [],
    nextCursor: null,
  });
}
const sessionMessages = path.match(
  /^\/agents\/manager\/sessions\/([^/]+)\/messages$/,
);
if (sessionMessages)
  return json(route, {
    messages: manager.messages
      .filter(
        (message) => message.roomId === decodeURIComponent(sessionMessages[1]),
      )
      .map((message) => ({
        id: message.id,
        role: message.role,
        text: message.content.text,
        attachments: [],
        metadata: message.content.metadata ?? {},
        createdAtMs: message.createdAtMs,
      })),
    nextBefore: null,
  });
if (/^\/agents\/manager\/sessions\/[^/]+$/.test(path))
  return json(route, { session: managerChat });
```

- replace `await page.getByRole('button', { name: 'Activity', exact: true }).click();` with `await page.getByRole('button', { name: 'Work', exact: true }).click();` and `await page.getByRole('button', { name: 'Chat', exact: true }).click();` with `await page.getByRole('button', { name: 'Open companion chat' }).click();`;
- in the `expect(runs).toEqual([…])` block, change `roomId: 'direct:manager'` to `roomId: 'chat:e2e'`.

In `apps/web-e2e/src/main-workspace-agent.spec.ts`:

- insert in `installApiFixture` before `if (path.includes('/connectors')) {`:

```ts
const sessionsMatch = path.match(
  /^\/agents\/([^/]+)\/sessions(?:\/([^/]+)(\/messages)?)?$/,
);
if (sessionsMatch) {
  const owner = state.agents.find(
    (agent) => agent.state.id === sessionsMatch[1],
  );
  const rooms = [
    ...new Set((owner?.messages ?? []).map((message) => message.roomId)),
  ];
  const session = (roomId: string) => ({
    id: roomId,
    agentId: sessionsMatch[1],
    roomId,
    kind: 'chat',
    origin: 'web',
    title: 'Earlier chat',
    titleSource: 'first_message',
    createdAtMs: 1,
    lastActivityAtMs: 2,
    lastReadAtMs: 2,
    archived: false,
    parentSessionId: null,
    parentRunId: null,
    parentAgentId: null,
    summary: null,
    contextTrimmed: null,
    messageCount: 1,
    preview: null,
    activeRuns: 0,
    pendingApprovals: 0,
    unread: false,
    capabilities: {
      send: true,
      steer: true,
      stop: true,
      rename: true,
      archive: true,
      delete: true,
      compact: true,
      export: true,
    },
  });
  const sessionId = sessionsMatch[2]
    ? decodeURIComponent(sessionsMatch[2])
    : null;
  if (sessionId && sessionsMatch[3]) {
    await fulfillJson(route, {
      messages: (owner?.messages ?? [])
        .filter((message) => message.roomId === sessionId)
        .map((message) => ({
          id: message.id,
          role: message.role,
          text: message.content.text,
          attachments: [],
          metadata: message.content.metadata ?? {},
          createdAtMs: message.createdAtMs,
        })),
      nextBefore: null,
    });
  } else if (sessionId) {
    await fulfillJson(route, { session: session(sessionId) });
  } else {
    await fulfillJson(route, {
      sessions: rooms.map(session),
      nextCursor: null,
    });
  }
  return;
}
```

- in `failed settings save preserves draft, conversation and original identity`, change `await page.goto('/');` to `await page.goto('/#/s/direct%3Amain');`;
- in `mobile chat keeps a bounded navigation dock and reports disconnection`, change `navigation.getByRole('button', { name: 'Chat', exact: true })` to `navigation.getByRole('button', { name: 'Chats', exact: true })`.

- [ ] **Step 6: Run the web tests**

Run: `bun x nx test @animaOS-SWARM/web`
Expected: PASS — every web test, including the rewritten shell tests, the updated and new ViewHarness tests, and `visual-tokens.test.ts`.

Run: `bun x nx run @animaOS-SWARM/web:typecheck`
Expected: exit 0 (no references to the deleted components or the removed `workspace`/`activity`/`telegram` props remain).

- [ ] **Step 7: Commit**

```bash
git add apps/web/src/components/WorkspaceShell.tsx apps/web/src/components/WorkspaceShell.test.tsx apps/web/src/components/CompanionShell.test.tsx apps/web/src/ViewHarness.tsx apps/web/src/ViewHarness.test.tsx apps/web/src/lib/daemon-api.ts apps/web-e2e/src/companion.spec.ts apps/web-e2e/src/independent-agents.spec.ts apps/web-e2e/src/main-workspace-agent.spec.ts
git commit -m "feat(web): route the console by session with a sessions sidebar and session view"
```

---

#### Controller rulings from the pre-flight audit (binding)

1. Resolve uncertain or timed-out sends from the open session (its `activeRuns` or a re-fetched session plus the session's messages), not agent-wide status, and keep polling while the session has active runs. Tests: a check-in running in another session does not lock an unrelated timed-out send; a send still queued for its room is not declared unconfirmed.
2. Disable the composer while `routeSessionId && !activeSession` (the session record is still loading).
3. Keep Telegram-target check-in creation: add a Workspace/Telegram target selector (Telegram only when a chat is approved) to the Work › Schedules create form, mirroring the removed CheckinsView. Test.
4. Show `useSessionMessages().error` in the session view (for example a 503 when loading older pages).
5. Persist drafts per session in `sessionStorage` (wrapped in try/catch; failures fall back to memory), per spec §15.5.
6. The sessions drawer traps focus, sets initial focus, and closes on Escape.
7. `startChat` skips navigation when `route.kind === 'page'` and updates `lastConversationRef` instead.
8. When the sessions routes report the daemon is too old (Task 14's error), show "Update the daemon" instead of failing sends.
9. Correct this task's test-change count to its actual list (11 changed, 3 new).

---

### Task 18: M2 verification

**Files:**

- Modify: `docs/superpowers/plans/2026-09-23-companion-console.md` (status table)

- [ ] **Step 1: Check the removed paths and the new contracts**

Run: `grep -rn "ActivityView\|CheckinsView\|TelegramThread\|direct:\${" apps/web/src`
Expected: no output.

Run: `grep -n "CONTROL_PLANE_STORE_VERSION: u32 = 5" hosts/rust-daemon/src/control_plane_store.rs && grep -n "ANIMAOS_RS_HISTORY_SQLITE_FILE" hosts/rust-daemon/src/app/persistence.rs hosts/rust-daemon/README.md`
Expected: one version line and matches in both files.

Run: `grep -rn "TelegramOutboundRecord {" hosts/rust-daemon/src | wc -l && grep -rn "message_pruned: false" hosts/rust-daemon/src | wc -l`
Expected: 15 and 14 (the definition plus 14 literals, each with the flag).

Run: `grep -rn "AgentRunRequest {" hosts/rust-daemon/src | grep -v "pub(crate) struct\|let AgentRunRequest"`
Expected: every listed literal either contains `parent` (check each hit) or is a `..request(…)`/`..room_request(…)` update.

- [ ] **Step 2: Run the milestone gate**

Run: `df -h /System/Volumes/Data`

- With at least 12 GB available: run `bun x nx run rust-daemon:test --skipNxCache` (it also runs `core-rust:test`). Expected: PASS.
- Otherwise run the fallback in the shared `target/`: `CARGO_INCREMENTAL=0 cargo test -p anima-core --lib`, `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib`, `CARGO_INCREMENTAL=0 cargo test -p anima-core --tests`, then `CARGO_INCREMENTAL=0 cargo test -p anima-daemon --tests`. Expected: PASS. The fallback does not satisfy AGENTS.md's completion rule; record that the Nx gate is pending disk space.

Run: `bun x nx run-many -t test,typecheck,build -p @animaOS-SWARM/sdk @animaOS-SWARM/web --skipNxCache`
Expected: every target succeeds.

The Postgres conformance test stays `#[ignore]` without a database; when one is available, run `DATABASE_URL=postgres://… CARGO_INCREMENTAL=0 cargo test -p anima-daemon --lib -- --ignored history::postgres` and record the result.

- [ ] **Step 3: Update the master plan status**

In `docs/superpowers/plans/2026-09-23-companion-console.md`, change the M2 row to the following only if both gate commands passed:

```markdown
| M2 Sessions | `2026-09-23-companion-console-m2.md` | done |
```

If the Rust gate ran only through the fallback, use `implemented — Nx gate pending (disk)` instead of `done`.

```bash
git add docs/superpowers/plans/2026-09-23-companion-console.md
git commit -m "docs: mark the M2 sessions milestone complete"
```

#### Controller rulings from the pre-flight audit (binding)

1. Fix the grep expectation: `TelegramOutboundRecord {` also matches two function signatures; assert the 14 `message_pruned: false` literals instead of a 15-line count.
2. Run the three edited Playwright specs (web-e2e) locally if Playwright browsers and the simulated-API setup are available (CI runs `e2e-ci` on every push); otherwise record the deferral in the ledger instead of claiming them.

---

## Notes for the controller

**Names and shape (code over plan).**

- The master plan's `HistoryOutbox::enqueue(..)` is `HistoryService` (`enqueue_committed`, `enqueue_session_deletion`, the mirrored-id set, `flush_once`) plus a `HistoryWorker` for the loop. `SessionRecord`, `SessionKind`, `DaemonState::sessions`, and `derive_sessions_for_legacy_rooms(..)` keep their master-plan names.
- The master plan lists T2.7 (shell) before T2.8 (view). Here the components come first (Task 16) and the shell and harness switch together (Task 17), because the shell's props change and `ViewHarness` must switch in the same commit to keep the tree green.
- Postgres history tables are prefixed `history_` (SQLite uses the spec's names); the generated `tsvector` column needs Postgres 12+. All six tables exist, but M2 writes only messages and runs; usage (M8), approvals (M4), schedule runs (M6), and attachments (M9) arrive with their milestones.
- Unversioned JSON snapshots load as version 1, so they get the pre-upgrade backup too. The JSON backup is the file's exact bytes; Postgres uses the `control_plane.backup.<version>` row.

**Spec vs. code decisions.**

- Legacy room ids that fail the session-id pattern map to `legacy-room:<32 hex>` and keep `roomId` (F17). `RunChangeSet::session_id` still holds the room id because reply detection compares it with `Message::room_id`; only ledger records and history rows carry the mapped id.
- `TOOL_GRANTS` is empty in M2 (none of §13.3's tools exist yet); the helper and the once-only bookkeeping are in place, and the applied set is recorded on a fresh start too, so later agents never get retroactive grants. Agents with `tools: None` and helpers are skipped; grants do not rewrite `anima.yaml`.
- Session fields not stored in M2: `usage` totals (M8) and `sessionAllowances` (M4). `summary` and `contextTrimmed` exist but stay null until M3; `pendingApprovals` is always 0.
- Message metadata exposure is an allowlist: spec §3.3's keys plus `kind` (check-in prompts) and `source` (Telegram turns).
- Check-in sessions have `send: yes` in the capability table, but the blocking route rejects `schedule:` rooms, so the web shows a read-only note until M3's session runs route. Telegram sessions reply through the existing connector owner-send route.
- The delivery loop retries `Failed` Telegram records, so the code treats only `Delivered` as terminal: pruning skips messages referenced by `Pending` or `Failed` records, and validation accepts `messagePruned` only on `Delivered` ones (error text `outbound delivery '<id>' is marked messagePruned but was not delivered`).
- Session deletion also removes the session's terminal ledger runs and history run rows (the spec lists the record, hot and mirrored messages, and attachments). It reserves the room lock, so a run waiting for that room starts after the deletion and creates a fresh session record. History rows of deleted agents are orphaned; agent deletion does not queue a history deletion.
- `GET /api/agents?view=summary` keeps the route's existing authorization; an unknown `view` value now returns 400 (it was ignored before).
- Runs from the blocking route are `source: api`, so they count as the owner's own turns for read state (the route cannot tell the web from the CLI in M2). Every generated API/CLI room becomes a chat session with `origin: api`.

**Behavior changes to accept or schedule.**

- The bootstrap still polls full agents every 5 s; the switch to `view=summary` every 30 s belongs with M3's stream (§15.5). Tool and system messages keep today's pills until M3's tool cards.
- After pruning, `GET /api/agents` and `list_connector_messages` show only the hot tail, and `owner_send_replay` finds an owner-send retry only while its turn is hot (≥ 24 h).
- Removing `ActivityView` removes the in-chat check-in form; schedules stay manageable in Work › Schedules until M6's Automations page. The desktop sidebar has no "Chat" destination (New chat plus the sessions list replace it); the mobile dock says "Chats".
- The composer no longer waits for the agent's runs in other sessions (M1 allows different rooms at once); it is disabled while the open session has an active run.
- The sidebar loads the newest 200 sessions (no "load more" in M2); search reaches older ones. Unread is computed from the hot tail, and migrated legacy sessions start read.
- A run's model context is its room's hot messages, so after pruning a long chat no longer sends turns older than its newest 200 and the last 24 hours (M3's context budget, spec §5, replaces this). Stable `schedule:<id>` rooms give each check-in its earlier check-ins as context, where per-tick rooms had none; frequent check-ins therefore send more tokens until pruning or M3's budget bounds them.

**Risks.**

- A history store that cannot be opened fails startup; read failures degrade views to the hot tail (a `before` cursor only the store can resolve, and export, return 503).
- `messageCount` adds unmirrored hot messages to the store's count; right after a restart, before the first reconcile, it can double count (display only).
- Search matches word prefixes in SQLite and Postgres but substrings in the memory store and the hot tail.
- Hidden flags are fixed per commit in the store but recomputed over the hot tail; a check-in turn whose opening message was pruned hides only its bare `CHECKIN_OK` in hot views.
- A run dropped between its Phase A insert and its start save leaves its new, empty session record in memory until the next save.
- Postgres JSONB rejects `\u0000` in message text (the control plane has the same limitation).
- `app_with_state` starts the flush loop only when a Tokio runtime is present (tests without one flush explicitly).
- The e2e specs (`companion`, `independent-agents`, `main-workspace-agent`) are updated but run only in M10's gate.
- Tasks 6, 11, 12, and 17 are the largest. Task 17 rewrites `ViewHarness`; its ViewHarness test edits are listed per test so a reviewer can diff them against the originals.
