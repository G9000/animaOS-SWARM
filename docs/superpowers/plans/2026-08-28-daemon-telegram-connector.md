# Daemon Telegram Connector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed an automatically supervised, securely persisted Telegram connector in the Rust daemon and expose pairing, a dedicated Telegram thread, and daemon-backed scheduled delivery in the web app.

**Architecture:** `anima-core` gains a host-agnostic stable-room execution primitive. `hosts/rust-daemon` owns agent run coordination, connector/schedule persistence, OS-vault credentials, Telegram transport, polling, pairing, and durable delivery. The React web app remains a secret-submission and configuration surface and consumes agent-scoped daemon APIs.

**Tech Stack:** Rust 2021, Tokio, Axum, Reqwest/rustls, keyring, Serde, Utoipa, React 19, TypeScript, Vite/Vitest, Nx, Bun.

---

## File Map

- `packages/core-rust/crates/anima-core/src/runtime.rs`: stable-room execution API with existing generated-room wrappers.
- `packages/core-rust/crates/anima-core/src/runtime/tests.rs`: room isolation and history-order tests.
- `hosts/rust-daemon/src/agent_runs.rs`: one per-agent serialized execution path shared by HTTP, Telegram, and schedules.
- `hosts/rust-daemon/src/connectors/mod.rs`: persisted connector/inbound/outbound records, runtime status, and manager surface.
- `hosts/rust-daemon/src/connectors/credentials.rs`: redacting credential-store contract, in-memory test store, and OS-vault implementation.
- `hosts/rust-daemon/src/connectors/telegram.rs`: bounded Telegram Bot API client and update/message normalization.
- `hosts/rust-daemon/src/connectors/runtime.rs`: supervised polling, durable inbox processing, pairing, and outbound delivery loops.
- `hosts/rust-daemon/src/schedules.rs`: persisted scheduled prompts, due-time rules, daemon scheduler loop, and check-in semantics.
- `hosts/rust-daemon/src/routes/connectors.rs`: connector/thread route handlers.
- `hosts/rust-daemon/src/routes/schedules.rs`: schedule CRUD route handlers.
- `hosts/rust-daemon/src/routes/contracts/connectors.rs`: public connector request/response contracts.
- `hosts/rust-daemon/src/routes/contracts/schedules.rs`: public schedule request/response contracts.
- `hosts/rust-daemon/src/control_plane_store.rs`: versioned connector, inbound, outbound, and schedule snapshot fields.
- `hosts/rust-daemon/src/state.rs`: restored/persisted non-secret connector and schedule state.
- `hosts/rust-daemon/src/app.rs`: service construction, restoration, and background-task startup.
- `hosts/rust-daemon/src/routes/mod.rs`: service-bearing router state, route mounting, OpenAPI registration, and local-owner guards.
- `apps/web/src/lib/daemon-api.ts`: typed connector/thread/schedule client.
- `apps/web/src/lib/telegram.ts`: connector view-model helpers and safe status labels.
- `apps/web/src/hooks/useAgentIntegrations.ts`: connector and schedule loading/mutation with stale-request fencing.
- `apps/web/src/components/TelegramSettings.tsx`: connect, pair, replace, restart, and disconnect UI.
- `apps/web/src/components/TelegramThread.tsx`: dedicated paginated thread and composer.
- `apps/web/src/components/SettingsPanel.tsx`: mount agent Telegram settings without storing token globally.
- `apps/web/src/components/WorkspaceShell.tsx`: add the Telegram workspace destination.
- `apps/web/src/components/CheckinsView.tsx`: daemon schedule CRUD and delivery-target selector.
- `apps/web/src/ViewHarness.tsx`: integration ownership and one-time local check-in import.
- `apps/web/src/lib/checkins.ts`: legacy parsing/import helpers only; remove browser timer behavior.
- `Cargo.toml`, `Cargo.lock`, `AGENTS.md`: remove the old gateway and add daemon dependencies.
- `hosts/telegram-gateway/`: delete after embedded behavior is covered.

### Task 1: Add Stable-Room Agent Execution

**Files:**
- Modify: `packages/core-rust/crates/anima-core/src/runtime.rs`
- Modify: `packages/core-rust/crates/anima-core/src/runtime/tests.rs`

- [ ] **Step 1: Write failing stable-room tests**

Add tests that call a wished-for room-aware method twice and assert every new user/assistant/tool message uses the supplied room while supplied room history precedes the new input:

```rust
#[test]
fn run_in_room_records_the_complete_turn_in_the_supplied_room() {
    let mut runtime = runtime();
    runtime.init();
    let result = block_on(runtime.run_in_room_with_context(
        "telegram-room".into(),
        Vec::new(),
        Content { text: "hello".into(), ..Content::default() },
    ));
    assert_eq!(result.status, TaskStatus::Success);
    assert!(runtime.messages().iter().all(|message| message.room_id == "telegram-room"));
}
```

Also retain a generated-room compatibility assertion for `run()`.

- [ ] **Step 2: Run the focused test and confirm RED**

Run: `cargo test -p anima-core runtime::tests::run_in_room -- --nocapture`

Expected: compilation failure because the room-aware method does not exist.

- [ ] **Step 3: Implement the minimal room-aware API**

Add public wrappers ending in one private implementation that accepts `room_id` rather than generating it:

```rust
pub async fn run_in_room_with_context(
    &mut self,
    room_id: String,
    history: Vec<Message>,
    input: Content,
) -> TaskResult<Content> {
    self.run_in_room_with_context_and_tools(room_id, history, input, |_, _, call| {
        let name = call.name.clone();
        async move { TaskResult::error(format!("Unknown tool: {name}"), 0) }
    }).await
}
```

Keep `run`, `run_with_context`, and `run_with_context_and_tools` as compatibility wrappers that generate a fresh room once and delegate.

- [ ] **Step 4: Run focused and crate tests**

Run: `cargo test -p anima-core runtime::tests`

Expected: all runtime tests pass.

- [ ] **Step 5: Commit**

```text
git add packages/core-rust/crates/anima-core/src/runtime.rs packages/core-rust/crates/anima-core/src/runtime/tests.rs
git commit --no-gpg-sign -m "feat(core): support stable agent rooms"
```

### Task 2: Define and Persist Connector/Schedule State

**Files:**
- Create: `hosts/rust-daemon/src/connectors/mod.rs`
- Create: `hosts/rust-daemon/src/schedules.rs`
- Modify: `hosts/rust-daemon/src/lib.rs`
- Modify: `hosts/rust-daemon/src/control_plane_store.rs`
- Modify: `hosts/rust-daemon/src/state.rs`

- [ ] **Step 1: Write failing snapshot round-trip tests**

Add tests with a sentinel token held outside the records. Round-trip a `ControlPlaneSnapshot` containing a connector, pending inbound record, pending outbound record, and schedule. Assert all non-secret fields survive and serialized JSON does not contain the sentinel.

The persisted connector shape must be:

```rust
pub(crate) struct TelegramConnectorRecord {
    pub id: String,
    pub agent_id: String,
    pub room_id: String,
    pub bot: TelegramBotIdentity,
    pub approved_chat: Option<TelegramChatIdentity>,
    pub pending_pairing: Option<TelegramPairingCandidate>,
    pub next_update_id: i64,
    pub enabled: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
```

`ScheduledPromptRecord` includes `next_due_at_ms`, last outcome, and `ScheduleTarget::{Workspace, Connector { connector_id }}`. Inbound records are keyed by connector/update ID; outbound records reference the persisted assistant message ID.

- [ ] **Step 2: Run focused tests and confirm RED**

Run: `cargo test -p anima-daemon control_plane_store::tests::connector`

Expected: compilation failure because the record types and snapshot fields do not exist.

- [ ] **Step 3: Implement serde-safe domain records and state collections**

Use bounded strings at mutation boundaries, stable IDs, `#[serde(default)]` snapshot fields, and explicit state enums. Bump `CONTROL_PLANE_STORE_VERSION` to `2`; older version-1 snapshots load empty connector/schedule/delivery collections.

Extend `DaemonState::control_plane_snapshot` and `restore_control_plane_snapshot`. Restoration validates ownership references and rejects duplicate IDs or connector references to missing agents rather than partially restoring them.

- [ ] **Step 4: Verify round trips and existing persistence tests**

Run: `cargo test -p anima-daemon control_plane_store`

Run: `cargo test -p anima-daemon state::tests::stale_delayed_persist_request`

Expected: connector tests and existing persistence ordering tests pass.

- [ ] **Step 5: Commit**

```text
git add hosts/rust-daemon/src/connectors/mod.rs hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/lib.rs hosts/rust-daemon/src/control_plane_store.rs hosts/rust-daemon/src/state.rs
git commit --no-gpg-sign -m "feat(daemon): persist connector and schedule state"
```

### Task 3: Centralize Serialized Agent Runs

**Files:**
- Create: `hosts/rust-daemon/src/agent_runs.rs`
- Modify: `hosts/rust-daemon/src/lib.rs`
- Modify: `hosts/rust-daemon/src/routes/agents.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Modify: `hosts/rust-daemon/tests/agent_api.rs`

- [ ] **Step 1: Write failing concurrency and room-isolation tests**

Add a test that launches two runs for the same agent through a shared coordinator, holds the first model call, and proves the second waits instead of returning not-found. Add a stable-room test that seeds ordinary messages and Telegram messages, then asserts only Telegram-room history is passed to the model.

- [ ] **Step 2: Run focused tests and confirm RED**

Run: `cargo test -p anima-daemon agent_runs -- --nocapture`

Expected: compilation failure because `AgentRunCoordinator` does not exist.

- [ ] **Step 3: Extract the existing route execution path**

Implement:

```rust
pub(crate) enum RunRoom {
    Generated,
    Stable(String),
}

pub(crate) struct AgentRunRequest {
    pub agent_id: String,
    pub content: Content,
    pub room: RunRoom,
    pub idempotency_key: Option<String>,
}
```

Use a guarded map of `Arc<tokio::sync::Mutex<()>>` keyed by agent ID. After acquiring the per-agent lock, take the runtime, select stable-room history when requested, execute, restore, persist, and project the response. Move task-result memory persistence into this shared path. Preserve delete-during-run fencing and the global run semaphore.

The low-level coordinator entry point also accepts a synchronous commit hook:

```rust
pub(crate) async fn run_with_commit<F>(
    &self,
    request: AgentRunRequest,
    commit: F,
) -> Result<AgentRunEnvelope, ApiError>
where
    F: FnOnce(
        &mut DaemonState,
        &AgentRuntimeSnapshot,
        &TaskResult<Content>,
    ) -> Result<(), ApiError> + Send,
```

After model/tool execution, reacquire the daemon write lock, restore the runtime, invoke `commit` while still holding the lock, and only then create one `control_plane_persist_request`. The ordinary `run` method uses a no-op hook. Connector processing later uses this hook to mark inbound completion and create outbound delivery in the same snapshot as the restored agent turn.

- [ ] **Step 4: Route ordinary HTTP runs through the coordinator**

`POST /api/agents/{id}/run` parses the request as before and delegates with `RunRoom::Generated`. Keep public response and error behavior unchanged.

- [ ] **Step 5: Run daemon agent tests**

Run: `cargo test -p anima-daemon agent_runs`

Run: `cargo test -p anima-daemon routes::agents`

Run: `cargo test -p anima-daemon --test agent_api`

Expected: new coordination tests and existing agent API tests pass.

- [ ] **Step 6: Commit**

```text
git add hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/lib.rs hosts/rust-daemon/src/routes/agents.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/tests/agent_api.rs
git commit --no-gpg-sign -m "refactor(daemon): coordinate agent runs"
```

### Task 4: Add Secure Credentials and Telegram Transport

**Files:**
- Create: `hosts/rust-daemon/src/connectors/credentials.rs`
- Create: `hosts/rust-daemon/src/connectors/telegram.rs`
- Modify: `hosts/rust-daemon/src/connectors/mod.rs`
- Modify: `hosts/rust-daemon/Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing credential redaction tests**

Test in-memory put/load/delete, version rejection, failed replacement rollback, and `Debug` redaction. Serialize every public/store error with sentinel token `telegram-secret-sentinel` and assert it never appears.

- [ ] **Step 2: Write failing Telegram client tests**

Against a local mock server, test `getMe`, `getUpdates`, `sendMessage`, UTF-8-safe chunking, redirect rejection, request timeout, malformed bodies, oversized bodies, and upstream errors that contain a sentinel token. The injectable mock base URL is `#[cfg(test)]`/constructor-only; production always uses `https://api.telegram.org`.

- [ ] **Step 3: Run focused tests and confirm RED**

Run: `cargo test -p anima-daemon connectors::credentials`

Run: `cargo test -p anima-daemon connectors::telegram`

Expected: compilation failure because credential and transport modules do not exist.

- [ ] **Step 4: Implement the credential boundary**

Define an async `ConnectorCredentialStore` trait with `load`, `put`, and `delete`. Use a redacting token newtype. Implement in-memory tests and a production keyring store with service `animaos.connector.telegram` and account `connector:{id}`. Never fall back to files.

- [ ] **Step 5: Implement the bounded Telegram client**

Build one `reqwest::Client` with redirects disabled, explicit timeouts, rustls, and body bounds. Never include token-bearing URLs in displayable errors or trace fields. Normalize only text updates and expose safe bot/chat/user identities.

- [ ] **Step 6: Run focused tests**

Run: `cargo test -p anima-daemon connectors::credentials`

Run: `cargo test -p anima-daemon connectors::telegram`

Expected: all credential and transport tests pass.

- [ ] **Step 7: Commit**

```text
git add hosts/rust-daemon/src/connectors/credentials.rs hosts/rust-daemon/src/connectors/telegram.rs hosts/rust-daemon/src/connectors/mod.rs hosts/rust-daemon/Cargo.toml Cargo.lock
git commit --no-gpg-sign -m "feat(daemon): add secure telegram transport"
```

### Task 5: Implement Connector Management, Pairing, and Durable Delivery

**Files:**
- Create: `hosts/rust-daemon/src/connectors/runtime.rs`
- Modify: `hosts/rust-daemon/src/connectors/mod.rs`
- Modify: `hosts/rust-daemon/src/agent_runs.rs`
- Modify: `hosts/rust-daemon/src/app.rs`
- Modify: `hosts/rust-daemon/src/app/persistence.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`

- [ ] **Step 1: Write failing manager lifecycle tests**

Use the in-memory credential store and fake Telegram transport to test:

- create verifies before vault write/publish;
- one connector per agent;
- pending pairing records only the latest bounded candidate;
- approval binds one chat and ignores others;
- token replacement rollback;
- restart cancels the old worker;
- delete removes credentials and archives room records;
- agent deletion aborts when credential cleanup fails;
- missing restored credential reports `credential_required` without blocking daemon startup.

- [ ] **Step 2: Write failing durable inbox/outbox tests**

Test atomic snapshot acceptance of update+offset, duplicate update reuse, crash restoration of pending inbound, stable run idempotency, result+completed-inbound+outbound persistence through the coordinator commit hook, send retry without a second model call, and deletion archiving/purging rules. Connector deletion archives completed records and purges never-processed inbound plus undeliverable pending outbound after dependent schedules are disabled.

- [ ] **Step 3: Run focused tests and confirm RED**

Run: `cargo test -p anima-daemon connectors::runtime`

Expected: tests fail because manager behavior is absent.

- [ ] **Step 4: Implement manager mutations**

Create connectors with stable IDs/rooms, validate agent ownership, persist state after every mutation, and expose safe summaries. Keep task handles outside `DaemonState`. Implement create/replace/restart/delete as transactional sequences with compensating vault cleanup where needed.

- [ ] **Step 5: Implement supervised workers**

Long polling atomically accepts authorized updates into the durable inbox and advances offsets. Pairing-mode updates persist a candidate and send the approval notice. Inbound processing calls `AgentRunCoordinator::run_with_commit` with the connector room and stable idempotency key. Its commit hook marks the inbound item complete and creates the outbound item before the coordinator takes its single post-run snapshot. Outbound delivery sends stored chunks, tracks attempts, and compacts delivered records after seven days/1,000 records.

- [ ] **Step 6: Restore workers on daemon startup**

Construct services once, restore snapshot state, load credentials, mark missing credentials safely, then start connector and scheduler workers. Ensure graceful shutdown cancels and joins workers.

- [ ] **Step 7: Run manager and persistence tests**

Run: `cargo test -p anima-daemon connectors`

Run: `cargo test -p anima-daemon app::persistence`

Run: `cargo test -p anima-daemon state::tests`

Expected: lifecycle, crash recovery, and existing persistence tests pass.

- [ ] **Step 8: Commit**

```text
git add hosts/rust-daemon/src/connectors hosts/rust-daemon/src/agent_runs.rs hosts/rust-daemon/src/app.rs hosts/rust-daemon/src/app/persistence.rs hosts/rust-daemon/src/routes/mod.rs
git commit --no-gpg-sign -m "feat(daemon): supervise telegram connectors"
```

### Task 6: Expose Connector and Thread APIs with Local-Owner Guards

**Files:**
- Create: `hosts/rust-daemon/src/routes/connectors.rs`
- Create: `hosts/rust-daemon/src/routes/contracts/connectors.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/mod.rs`
- Modify: `hosts/rust-daemon/src/routes/http.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Create: `hosts/rust-daemon/tests/connector_api.rs`

- [ ] **Step 1: Write failing API contract tests**

Cover every connector route, ownership/not-found behavior, `Cache-Control: no-store`, safe summaries, pagination, pairing approval, thread send, and secret sentinel absence.

Add guard tests proving forwarded headers, unapproved origins, disabled remote binding, and originless requests without a valid `ANIMA_LOCAL_ADMIN_TOKEN` fail before fake vault/network side effects. Read-only summaries/messages remain readable and secret-free.

- [ ] **Step 2: Run the integration test and confirm RED**

Run: `cargo test -p anima-daemon --test connector_api`

Expected: connector endpoints return 404.

- [ ] **Step 3: Implement contracts and route handlers**

Use camelCase Serde/Utoipa contracts. Bound token/text/limit inputs before mutation. `POST .../messages` delegates to the manager, returns the room messages/result, and queues Telegram delivery only after pairing.

- [ ] **Step 4: Implement the local-owner guard**

Compute whether the daemon bind address is loopback at startup, reject forwarding headers, compare browser `Origin` against `ANIMA_ALLOWED_UI_ORIGINS` plus documented local defaults, and require constant-time bearer comparison for originless clients. Apply to all connector mutations/thread sends and schedule mutations.

- [ ] **Step 5: Mount routes and OpenAPI entries**

Add connector tags, paths, and schemas without changing existing routes.

- [ ] **Step 6: Run connector API and route tests**

Run: `cargo test -p anima-daemon --test connector_api`

Run: `cargo test -p anima-daemon routes::tests`

Expected: all tests pass.

- [ ] **Step 7: Commit**

```text
git add hosts/rust-daemon/src/routes/connectors.rs hosts/rust-daemon/src/routes/contracts/connectors.rs hosts/rust-daemon/src/routes/contracts/mod.rs hosts/rust-daemon/src/routes/http.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/tests/connector_api.rs
git commit --no-gpg-sign -m "feat(daemon): expose telegram connector api"
```

### Task 7: Run Scheduled Prompts in the Daemon

**Files:**
- Modify: `hosts/rust-daemon/src/schedules.rs`
- Create: `hosts/rust-daemon/src/routes/schedules.rs`
- Create: `hosts/rust-daemon/src/routes/contracts/schedules.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/mod.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Modify: `hosts/rust-daemon/src/app.rs`
- Modify: `hosts/rust-daemon/Cargo.toml`
- Create: `hosts/rust-daemon/tests/schedule_api.rs`

- [ ] **Step 1: Write failing due-time and migration tests**

Test first-run-after-full-interval, daily next occurrence, restart preservation, trigger PATCH, disable/re-enable, prompt-only PATCH, and import derivation:

```rust
assert_eq!(
    imported.next_due_at_ms,
    imported.last_run_at_ms.unwrap_or(imported.created_at_ms)
        .checked_add(imported.interval_secs * 1_000)
        .unwrap()
);
```

Test idempotent import, malformed/overflow rejection, and exact `CHECKIN_OK` handling with `kind: checkin` metadata.

- [ ] **Step 2: Write failing schedule API tests**

Cover list/create/update/delete, agent ownership, target validation, local-owner guard, unavailable connector behavior, connector delivery, workspace generated-room behavior, and disabling schedules when a connector is deleted.

- [ ] **Step 3: Run focused tests and confirm RED**

Run: `cargo test -p anima-daemon --test schedule_api`

Run: `cargo test -p anima-daemon schedules`

Expected: missing service/routes cause failures.

- [ ] **Step 4: Implement the daemon scheduler service**

Use `anima-schedule` for trigger validation and host-owned `next_due_at_ms`/outcome persistence. Claim and advance due state before execution. Wrap prompts with the exact existing silence convention. Require a ready paired connector before connector-targeted execution; unavailable targets record an error and wait until the next occurrence.

- [ ] **Step 5: Implement CRUD/import routes and start the loop**

Use idempotency keys for migration. Start one scheduler worker during daemon startup and cancel it on shutdown.

- [ ] **Step 6: Run schedule and connector tests**

Run: `cargo test -p anima-daemon --test schedule_api --test connector_api`

Expected: schedule tests and connector deletion interaction pass.

- [ ] **Step 7: Commit**

```text
git add hosts/rust-daemon/src/schedules.rs hosts/rust-daemon/src/routes/schedules.rs hosts/rust-daemon/src/routes/contracts/schedules.rs hosts/rust-daemon/src/routes/contracts/mod.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/app.rs hosts/rust-daemon/Cargo.toml hosts/rust-daemon/tests/schedule_api.rs Cargo.lock
git commit --no-gpg-sign -m "feat(daemon): run durable scheduled prompts"
```

### Task 8: Add Typed Web Connector and Schedule State

**Files:**
- Modify: `apps/web/src/lib/daemon-api.ts`
- Modify: `apps/web/src/lib/daemon-api.test.ts`
- Create: `apps/web/src/lib/telegram.ts`
- Create: `apps/web/src/lib/telegram.test.ts`
- Create: `apps/web/src/hooks/useAgentIntegrations.ts`
- Create: `apps/web/src/hooks/useAgentIntegrations.test.tsx`

- [ ] **Step 1: Write failing client and hook tests**

Test exact routes/payloads, connector status mapping, message pagination, schedule CRUD/import, stale retry fencing, agent-switch cleanup, and independent connector/schedule errors.

- [ ] **Step 2: Run focused web tests and confirm RED**

Run: `bun x nx run @animaOS-SWARM/web:test -- --run src/lib/daemon-api.test.ts src/hooks/useAgentIntegrations.test.tsx`

Expected: missing methods/modules fail compilation.

- [ ] **Step 3: Implement typed client methods**

Add public types with camelCase wire fields and methods for connector list/create/replace/approve/restart/delete, connector messages/send, and schedule CRUD/import. Reuse the existing `request` error handling without ever incorporating request bodies into errors.

- [ ] **Step 4: Implement the integration hook**

Load state per agent, fence overlapping mutations with generations, expose transaction-specific busy/errors, and clear connector/schedule state immediately on agent change.

- [ ] **Step 5: Run focused tests**

Run: `bun x nx run @animaOS-SWARM/web:test -- --run src/lib/daemon-api.test.ts src/lib/telegram.test.ts src/hooks/useAgentIntegrations.test.tsx`

Expected: all focused tests pass.

- [ ] **Step 6: Commit**

```text
git add apps/web/src/lib/daemon-api.ts apps/web/src/lib/daemon-api.test.ts apps/web/src/lib/telegram.ts apps/web/src/lib/telegram.test.ts apps/web/src/hooks/useAgentIntegrations.ts apps/web/src/hooks/useAgentIntegrations.test.tsx
git commit --no-gpg-sign -m "feat(web): add telegram connector client state"
```

### Task 9: Build Telegram Settings and Dedicated Thread UI

**Files:**
- Create: `apps/web/src/components/TelegramSettings.tsx`
- Create: `apps/web/src/components/TelegramSettings.test.tsx`
- Create: `apps/web/src/components/TelegramThread.tsx`
- Create: `apps/web/src/components/TelegramThread.test.tsx`
- Modify: `apps/web/src/components/SettingsPanel.tsx`
- Modify: `apps/web/src/components/SettingsPanel.test.tsx`
- Modify: `apps/web/src/components/WorkspaceShell.tsx`
- Modify: `apps/web/src/components/WorkspaceShell.test.tsx`
- Modify: `apps/web/src/ViewHarness.tsx`
- Modify: `apps/web/src/ViewHarness.test.tsx`

- [ ] **Step 1: Write failing Telegram settings tests**

Cover connect, pending pairing, approve, connected, degraded, replace token, restart, disconnect confirmation, disabled states, focus behavior, and sanitized errors. Assert the password value clears after success/failure/close and never reaches storage mocks.

- [ ] **Step 2: Write failing thread/navigation tests**

Cover a Telegram workspace destination appearing only when a connector exists, paginated dedicated messages, connector-room send, delivery status, retry-safe busy state, agent switch/reset cleanup, and mobile/keyboard accessibility.

- [ ] **Step 3: Run focused tests and confirm RED**

Run: `bun x nx run @animaOS-SWARM/web:test -- --run src/components/TelegramSettings.test.tsx src/components/TelegramThread.test.tsx src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx`

Expected: missing components and navigation fail.

- [ ] **Step 4: Implement secret-local settings UI**

Keep the token only in `TelegramSettings` component state. Submit directly to the mutation callback, clear it in `finally`, and unmount/clear on settings close or agent change. Reuse current panel primitives, focus trap, error banners, and transaction locking.

- [ ] **Step 5: Implement the Telegram workspace thread**

Add a dedicated workspace destination and render only connector-room messages returned by the connector API. The composer sends into that room and shows safe delivery feedback.

- [ ] **Step 6: Integrate without overwriting the user's main-checkout work**

Make changes only in this isolated worktree. Preserve current Neon Rose tokens, safe-area classes, and the main branch's existing `WorkspaceShell` semantics.

- [ ] **Step 7: Run focused UI tests**

Run: `bun x nx run @animaOS-SWARM/web:test -- --run src/components/TelegramSettings.test.tsx src/components/TelegramThread.test.tsx src/components/SettingsPanel.test.tsx src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx`

Expected: all focused tests pass.

- [ ] **Step 8: Commit**

```text
git add apps/web/src/components/TelegramSettings.tsx apps/web/src/components/TelegramSettings.test.tsx apps/web/src/components/TelegramThread.tsx apps/web/src/components/TelegramThread.test.tsx apps/web/src/components/SettingsPanel.tsx apps/web/src/components/SettingsPanel.test.tsx apps/web/src/components/WorkspaceShell.tsx apps/web/src/components/WorkspaceShell.test.tsx apps/web/src/ViewHarness.tsx apps/web/src/ViewHarness.test.tsx
git commit --no-gpg-sign -m "feat(web): manage telegram connector and thread"
```

### Task 10: Migrate Check-Ins to Daemon Schedules

**Files:**
- Modify: `apps/web/src/lib/checkins.ts`
- Create: `apps/web/src/lib/checkins.test.ts`
- Modify: `apps/web/src/components/CheckinsView.tsx`
- Create: `apps/web/src/components/CheckinsView.test.tsx`
- Modify: `apps/web/src/ViewHarness.tsx`
- Modify: `apps/web/src/ViewHarness.test.tsx`

- [ ] **Step 1: Write failing legacy migration tests**

Test deterministic idempotency keys, valid record projection of `createdAtMs`/`lastRunAtMs`, partial failure retention, full-success local deletion, retry without duplicates, and malformed-record preservation/reporting.

- [ ] **Step 2: Write failing schedule UI tests**

Test daemon-loaded schedules, create/delete, workspace versus Telegram target selection, disabled-target explanation, last outcomes, and copy stating that schedules run while the daemon is active rather than while the tab is open.

- [ ] **Step 3: Run focused tests and confirm RED**

Run: `bun x nx run @animaOS-SWARM/web:test -- --run src/lib/checkins.test.ts src/ViewHarness.test.tsx`

Expected: current browser timer/storage behavior fails the new assertions.

- [ ] **Step 4: Replace browser timers with daemon schedule calls**

Remove `setInterval`, due filtering, and local execution from `ViewHarness`. Render/mutate schedules through `useAgentIntegrations`. Keep only strict legacy parsing/import helpers in `checkins.ts`.

- [ ] **Step 5: Implement safe one-time migration**

Import every valid legacy record with a deterministic key. Remove `animaos.checkins.{agentId}` only after all records are confirmed. Never clear on partial failure.

- [ ] **Step 6: Run check-in and harness tests**

Run: `bun x nx run @animaOS-SWARM/web:test -- --run src/lib/checkins.test.ts src/ViewHarness.test.tsx src/components/CheckinsView.test.tsx`

Expected: schedule/migration tests pass.

- [ ] **Step 7: Commit**

```text
git add apps/web/src/lib/checkins.ts apps/web/src/lib/checkins.test.ts apps/web/src/components/CheckinsView.tsx apps/web/src/components/CheckinsView.test.tsx apps/web/src/ViewHarness.tsx apps/web/src/ViewHarness.test.tsx
git commit --no-gpg-sign -m "feat(web): move proactive prompts to daemon"
```

### Task 11: Remove the Legacy Gateway and Update Documentation

**Files:**
- Delete: `hosts/telegram-gateway/`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `AGENTS.md`
- Modify: `docs/rust-daemon-architecture.md`
- Modify: `README.md` if it references the old gateway

- [ ] **Step 1: Add a failing workspace inventory assertion**

Update an appropriate workspace/config test or add a narrow script assertion that `bun x nx show projects --json` and Cargo metadata contain no `telegram-gateway`/`anima-telegram-gateway` entry after removal.

- [ ] **Step 2: Delete the exact legacy host**

Remove only `hosts/telegram-gateway`, then delete its Cargo member. Regenerate `Cargo.lock` through the package manager/Cargo resolution; do not hand-edit lock entries.

- [ ] **Step 3: Update architecture and operating docs**

Document embedded startup, pairing, vault behavior, dedicated rooms, daemon-backed schedules, replacement/disconnect, and safe troubleshooting. Remove environment-variable instructions for the old sidecar.

- [ ] **Step 4: Verify inventory**

Run: `bun x nx show projects --json`

Expected: no `telegram-gateway` project.

Run: `cargo metadata --no-deps --format-version 1`

Expected: no `anima-telegram-gateway` package.

- [ ] **Step 5: Commit**

```text
git add -A -- hosts/telegram-gateway Cargo.toml Cargo.lock AGENTS.md docs/rust-daemon-architecture.md README.md
git commit --no-gpg-sign -m "chore: remove standalone telegram gateway"
```

### Task 12: Full Verification and Review

**Files:**
- Modify only files required by failures found during verification.

- [ ] **Step 1: Format and lint Rust**

Run: `bun x nx run rust-daemon:lint --skipNxCache`

Expected: exit 0.

- [ ] **Step 2: Run core and daemon tests**

Run: `bun x nx run core-rust:test --skipNxCache`

Expected: exit 0.

Run: `bun x nx run rust-daemon:test --skipNxCache`

Expected: exit 0. On a Windows daemon lock, use the AGENTS.md isolated validation target directory.

- [ ] **Step 3: Run web verification**

Run: `bun x nx run @animaOS-SWARM/web:test --skipNxCache`

Run: `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache`

Run: `bun x nx run @animaOS-SWARM/web:build --skipNxCache`

Expected: all exit 0.

- [ ] **Step 4: Run launcher and inventory verification**

Run: `bun x nx test workspace-dev --runInBand --skipNxCache`

Run: `bun x nx show projects --json`

Expected: launcher tests pass and the legacy gateway is absent.

- [ ] **Step 5: Audit secret leakage and diff scope**

Search production files, fixtures, snapshots, and test output for sentinel tokens. Run `git diff --check`, `git status --short`, and review `git diff origin/main...HEAD` to confirm only planned changes are present.

- [ ] **Step 6: Request code review and address findings**

Use `superpowers:requesting-code-review`; fix substantive issues with TDD and rerun affected plus full verification.

- [ ] **Step 7: Finish the branch**

Use `superpowers:finishing-a-development-branch` and present the verified integration options without touching the user's dirty main checkout.
