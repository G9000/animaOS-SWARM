# Daemon-Owned Telegram Connector Design

**Date:** 2026-08-28
**Status:** Approved

---

## Decision Summary

Replace the standalone `hosts/telegram-gateway` process with a Telegram connector owned and supervised by `hosts/rust-daemon`. The web app configures the connector, approves a Telegram chat through a pairing flow, displays a dedicated Telegram conversation thread, and lets daemon-backed scheduled prompts target that thread.

The initial UI exposes one Telegram connector for the current main agent. Connector records are agent-scoped from the start so a later release can expose one Telegram channel for every agent without changing the persistence model or API shape.

The browser may submit a BotFather token once, but the token is stored only in the operating-system credential vault and is never returned, logged, or serialized into daemon snapshots.

## Goals

1. Let the owner connect a Telegram bot from the web without starting another process or setting Telegram environment variables.
2. Start and resume Telegram polling automatically with the normal Rust daemon workflow.
3. Pair exactly one Telegram chat in v1 through an explicit approval step in the web UI.
4. Give the connector a stable, dedicated agent room whose history is separate from the ordinary web conversation.
5. Run scheduled prompts in the daemon so they continue while the browser is closed and can target the Telegram thread.
6. Persist connector metadata, schedules, firing state, Telegram update progress, and pending outbound deliveries across daemon restarts.
7. Persist the bot token in the OS credential vault and keep it out of public and durable non-secret state.
8. Remove the legacy `telegram-gateway` host and its workspace registrations.

## Non-Goals

- Telegram groups, topics, media, files, reactions, commands, inline keyboards, or webhook delivery.
- More than one approved Telegram chat per connector in v1.
- Exposing connectors for secondary agents in the v1 web UI.
- A general connector/plugin marketplace or generic arbitrary-channel abstraction.
- Synchronizing Telegram credentials between machines or operating-system users.
- Exactly-once delivery across ambiguous upstream network failures; the daemon provides durable at-least-once retry without rerunning the agent.

## Architecture

### Stable agent rooms

Extend `anima-core` with a room-aware execution entry point. The caller supplies a stable room identifier and prior messages for that room. The runtime records the user, assistant, and tool messages produced by the turn under that same room identifier. Existing callers keep the current generated-room behavior.

The daemon selects history by `room_id` before a Telegram or connector-thread run. Ordinary web chat and Telegram therefore share the same agent configuration and long-term memory but do not share short-term conversation transcripts.

### Agent run coordination

Add a daemon-owned `AgentRunCoordinator` used by HTTP chat, Telegram polling, and scheduled jobs. It serializes turns per agent before taking an `AgentRuntime` out of daemon state. This prevents concurrent sources from observing the temporarily removed runtime as a missing agent while preserving the existing delete-during-run fencing semantics.

The coordinator accepts an execution target:

- generated room for the existing ordinary run API;
- stable room for connector and schedule execution.

It owns runtime restoration, control-plane persistence, task-result memory persistence, and safe result projection so every caller follows one execution path.

### Connector manager

Add a daemon-owned connector manager with a Telegram-specific transport. It owns:

- connector creation, replacement, restart, and deletion;
- bot-token verification with Telegram `getMe`;
- long polling with persisted update offsets;
- pending pairing capture and approval;
- incoming-message normalization and authorization;
- stable-room execution through `AgentRunCoordinator`;
- durable outbound delivery and retry;
- supervised task cancellation and restart;
- safe runtime status reporting.

The manager is host-specific and lives under `hosts/rust-daemon`. It does not make `anima-core` depend on Telegram, HTTP, Tokio, or host persistence.

### Scheduling service

Wire `anima-schedule` into a daemon scheduler service. Telegram knowledge stays outside the reusable scheduling crate: the host persists a scheduled-prompt record with an opaque delivery target and feeds its trigger into the deterministic scheduler.

Each scheduled prompt targets either:

- `workspace`, which runs the agent without external delivery; or
- `connector`, identified by connector ID, which runs inside that connector's stable room and queues the response for Telegram delivery.

Persist enough firing state to prevent every-interval jobs from firing immediately again after restart and daily jobs from firing twice on the same local day.

### Web application

The main agent settings sheet gains a Telegram section with:

- Connect Telegram using a password input for the BotFather token.
- Connected bot identity and safe status.
- Pending chat details and an explicit Approve action.
- Replace token, Restart, and Disconnect actions.
- Actionable sanitized errors and busy-state locking.

The workspace gains a dedicated Telegram thread surface backed by connector message APIs. Its composer submits prompts to the same stable room; resulting assistant messages are queued to the approved Telegram chat. Check-in creation moves from browser timers to daemon schedule APIs and adds a delivery-thread selector.

## Domain Model

### Persisted connector record

The non-secret control-plane snapshot stores an agent-scoped connector record with:

- `id`
- `kind` (`telegram`)
- `agent_id`
- `room_id`
- bot numeric ID, username, and display name
- optional approved chat ID and safe display metadata
- optional pending pairing candidate with last-seen time
- next Telegram update offset
- enabled flag
- created and updated timestamps

Runtime-only fields such as task handles, cancellation tokens, backoff counters, and current transient errors are never serialized. Safe status is derived as `pending_pairing`, `connected`, `degraded`, `stopped`, or `credential_required`.

### Credential record

The credential vault uses a stable service and account name derived from the connector ID. The stored versioned payload contains only the Telegram bot token. Tests use an in-memory credential-store implementation; production uses the platform credential vault through a narrow store trait.

Vault failure never falls back to plaintext storage. Connector creation is not published or started until token verification and vault persistence both succeed. Replacement writes the new credential before cancelling and restarting the poller; a failed replacement leaves the prior working connector untouched.

### Scheduled prompt record

The daemon control-plane snapshot stores:

- stable ID and optional import idempotency key
- owning agent ID
- prompt
- interval or daily trigger
- enabled state
- delivery target
- last-fired state and last safe outcome
- created and updated timestamps

Deleting a connector disables schedules targeting it and records a visible reason instead of silently retargeting them.

### Durable outbound delivery

An outbound record contains a stable ID, connector ID, room ID, assistant message ID, text, creation time, attempt count, and delivery state. The agent result is committed before Telegram delivery begins. Retries resend the stored response and never rerun the agent.

## HTTP API

### Connectors

- `GET /api/agents/{agent_id}/connectors`
- `POST /api/agents/{agent_id}/connectors/telegram`
  - Body: `{ "botToken": "..." }`
  - Verifies the bot, persists the credential, creates the stable room, and starts pairing-mode polling.
- `PUT /api/agents/{agent_id}/connectors/{connector_id}/credential`
  - Body: `{ "botToken": "..." }`
  - Atomically verifies and replaces the token.
- `POST /api/agents/{agent_id}/connectors/{connector_id}/pairings/{chat_id}/approve`
- `POST /api/agents/{agent_id}/connectors/{connector_id}/restart`
- `DELETE /api/agents/{agent_id}/connectors/{connector_id}`

Connector responses include safe metadata and status only. Credential mutation responses set `Cache-Control: no-store`.

### Connector thread

- `GET /api/agents/{agent_id}/connectors/{connector_id}/messages`
- `POST /api/agents/{agent_id}/connectors/{connector_id}/messages`
  - Body: `{ "text": "..." }`
  - Runs the agent in the connector room, returns the updated messages/result, and queues the assistant result for Telegram delivery when a chat is approved.

### Scheduled prompts

- `GET /api/agents/{agent_id}/schedules`
- `POST /api/agents/{agent_id}/schedules`
- `PATCH /api/agents/{agent_id}/schedules/{schedule_id}`
- `DELETE /api/agents/{agent_id}/schedules/{schedule_id}`

Create accepts an optional import idempotency key. Repeating an import returns the existing record rather than creating a duplicate.

## Telegram Data Flow

### Connection

1. The browser submits a token to the daemon.
2. The daemon calls the fixed Telegram `getMe` endpoint with strict timeout and body bounds.
3. After validation, the daemon stores the token in the OS vault.
4. It persists the non-secret connector record and starts the supervised polling task.
5. The API returns safe bot identity and `pending_pairing` status.

### Pairing

1. An unapproved chat sends a text message to the bot.
2. The daemon stores a bounded pending pairing candidate and replies that owner approval is required.
3. The web displays the candidate chat ID and safe user/chat labels.
4. The owner approves it; the daemon binds that chat to the connector.
5. Other chats are ignored after approval.

### Conversation

1. The poller persists progress past each consumed Telegram update so restart does not replay accepted messages.
2. An approved text message is recorded as connector-room input with Telegram metadata.
3. `AgentRunCoordinator` runs the assigned agent with that room's history.
4. The assistant result is persisted in the room and added to the durable outbound queue.
5. The delivery worker sends the stored text in UTF-8-safe Telegram-sized chunks and marks it delivered.

Non-text updates are ignored in v1. Messages that exceed the daemon input bound receive a safe rejection without invoking the agent.

### Scheduled delivery

1. The daemon scheduler claims a due prompt and persists its firing state.
2. A connector-targeted prompt runs in the connector room through the same coordinator.
3. A silent sentinel response produces no outbound item; any other assistant response is queued for Telegram.
4. Telegram delivery retries independently of schedule execution.

## Persistence and Startup

Connector, schedule, and outbound records extend the versioned control-plane snapshot with serde defaults for backward compatibility. JSON and Postgres snapshot modes use the same non-secret schema. Tokens never enter either backend.

At startup the daemon:

1. restores agents, connectors, schedules, and outbound items;
2. loads available connector credentials from the vault;
3. starts one supervised poller/delivery worker per enabled connector with a usable credential;
4. marks missing-vault connectors `credential_required` without preventing daemon startup;
5. starts the scheduler loop and resumes pending outbound delivery.

## Local-Owner and Secret Boundary

Connector creation, credential replacement, connection testing, pairing approval, restart, and deletion are local-owner administration operations. They fail closed unless the daemon is directly serving a loopback peer and the browser origin is in the daemon's explicit local UI origin allowlist. Originless clients require the configured local-admin bearer token. Forwarding headers are rejected rather than trusted.

Telegram transport rules:

- fixed `https://api.telegram.org` origin;
- redirects disabled;
- explicit connect, request, and long-poll timeouts;
- bounded upstream response bodies;
- never log a Telegram URL containing the token;
- sanitize upstream descriptions and never return arbitrary response bodies;
- redact the token from `Debug`, errors, tracing, and panic context.

The OS credential vault protects secrets from casual file access and repository leakage. It does not claim to protect against malware running as the same operating-system user or a compromised daemon process.

## Lifecycle and Error Behavior

- Pollers use capped exponential backoff for transient Telegram failures.
- Invalid/revoked credentials stop polling and expose `credential_required` with a safe replacement action.
- Restart cancels and joins the old task before starting a replacement.
- Pending pairing candidates expire and are capped to one latest candidate in v1.
- Connector-thread turns are serialized with every other turn for the same agent.
- Delivery failures retain the outbound item and expose a degraded connector status.
- Deleting a connector cancels its tasks, deletes its vault credential, removes active metadata, archives its room history, and disables connector-targeted schedules.
- Agent deletion removes its connectors and schedules and attempts credential cleanup before the agent deletion becomes durable; cleanup failure aborts the destructive operation.

Public connector errors use stable codes such as:

- `connector_not_found`
- `connector_already_exists`
- `connector_token_invalid`
- `connector_credential_store_unavailable`
- `connector_pairing_not_found`
- `connector_not_paired`
- `connector_upstream_unavailable`
- `connector_local_admin_required`
- `connector_origin_rejected`
- `schedule_not_found`
- `schedule_target_unavailable`

Messages give a corrective action without including tokens, credential identifiers, token-bearing URLs, or raw upstream bodies.

## Browser Migration

When the web loads daemon schedules for an agent, it checks the existing `animaos.checkins.{agentId}` local-storage record. Each valid legacy check-in is submitted with a deterministic import idempotency key. The browser removes the legacy record only after every item is confirmed by the daemon. Partial failure keeps the source record so retry is safe.

The token remains component-local in a password field, is cleared after every submission attempt and when settings closes, and is never written to local storage, session storage, URL state, global React state, analytics, or error reporting.

## Testing Strategy

### Core Rust

- Stable-room execution records every turn message in the supplied room.
- Room history is passed before the new input and history from other rooms is excluded by daemon selection.
- Existing generated-room APIs retain their behavior.

### Daemon unit and integration tests

- Credential store redaction, version validation, put/load/delete behavior, and restart simulation with an in-memory store.
- Connector create, token replacement rollback, restart, delete, and agent ownership checks.
- Pairing discovery, approval, rejection of other chats, update-offset persistence, and non-text filtering.
- Mock Telegram server tests for `getMe`, long polling, chunked send, timeouts, redirects, malformed/oversized bodies, and sanitized errors.
- Concurrent web/Telegram/schedule turns serialize per agent.
- Connector-room history is separate from ordinary web history.
- Control-plane round trips preserve non-secret connector, schedule, and outbound state and never contain a sentinel token.
- Scheduler firing state survives restart and avoids duplicate immediate/daily execution.
- Durable outbound retry does not rerun the agent.
- Local-owner mutation guards reject remote, forwarded, unapproved-origin, and unauthenticated originless requests before vault or network side effects.
- Agent/connector deletion cleanup and schedule disabling are atomic from the public API perspective.

### Web tests

- Connect, pending pairing, approve, connected, degraded, restart, replace-token, and disconnect states.
- Token input clearing and absence from browser storage and public error content.
- Dedicated Telegram thread filtering, send behavior, busy/error states, and accessible focus management.
- Daemon-backed schedule CRUD, delivery-target selection, and browser-closed semantics represented by API ownership.
- Idempotent legacy check-in import with source deletion only after full success.
- Existing main-agent settings and workspace operations remain serialized and mounted correctly.

## Removal and Documentation

Delete:

- `hosts/telegram-gateway/`
- its root Cargo workspace membership;
- its Nx project registration by deleting the project;
- gateway-specific environment-variable documentation.

Update the agent guide and daemon/web documentation to describe automatic embedded Telegram operation, pairing, OS-vault persistence, dedicated threads, schedule targeting, and troubleshooting.

## Verification

- `bun x nx run core-rust:test --skipNxCache`
- `bun x nx run rust-daemon:test --skipNxCache`
- `bun x nx run rust-daemon:lint --skipNxCache`
- `bun x nx run @animaOS-SWARM/web:test --skipNxCache`
- `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache`
- `bun x nx run @animaOS-SWARM/web:build --skipNxCache`
- `bun x nx test workspace-dev --runInBand --skipNxCache` if workspace launcher behavior or project selection changes beyond deleting the legacy host.

## Acceptance Criteria

- A user can paste a BotFather token in the main agent settings, message the bot, approve the pending chat, and converse without starting another process.
- Restarting the daemon resumes the paired connector, schedule loop, update progress, and pending outbound delivery without re-entering the token.
- Telegram and ordinary web conversations have distinct short-term histories.
- A daemon-backed scheduled prompt can target Telegram and fires while the web is closed.
- Token sentinels are absent from snapshots, API responses, logs/errors exercised by tests, and browser storage.
- Disconnect removes the stored credential, stops polling, archives history, and visibly disables dependent schedules.
- The legacy `telegram-gateway` host no longer exists or appears in Cargo/Nx project lists.
