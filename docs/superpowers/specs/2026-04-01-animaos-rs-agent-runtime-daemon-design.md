# animaOS Rust Agent Runtime Daemon Design

**Date:** 2026-04-01
**Status:** Draft

---

## Goal

Add real in-process `AgentRuntime` instances to the Rust engine and expose them through `anima-daemon`.

This slice should make the daemon own runtime objects, not plain metadata, while still stopping short of the full TS execution loop.

---

## Scope

This design adds:

- a Rust `AgentRuntime` type in `anima-core`
- daemon-owned runtime instances stored in shared process state
- `POST /api/agents`
- `GET /api/agents`
- `GET /api/agents/:id`
- `GET /api/agents/:id/memories/recent`

This slice also gives each runtime a small amount of real execution context:

- lifecycle state
- in-memory message history
- last task result
- event log

---

## Why This Slice

This is the right next step because:

- the daemon already owns real Rust memory state
- the next architectural boundary is runtime ownership, not more daemon-only CRUD
- storing actual runtime objects now keeps the daemon-first architecture honest
- message/task/event context makes the runtime object useful without forcing model/tool parity yet

Alternatives considered:

1. Store only `AgentState` records in the daemon.
   This is too weak. It recreates a registry, not a runtime boundary.

2. Port the full TS `run()` loop immediately.
   This is too broad for one slice and would sprawl into model adapters, tools, providers, and evaluator behavior before the basic runtime boundary is stable.

Recommended approach: add a real runtime shell with message/task/event context first.

---

## Runtime Design

`anima-core` should gain a new `AgentRuntime` type that owns:

- `AgentConfig`
- `AgentState`
- `Vec<Message>`
- `Option<TaskResult<Content>>`
- `Vec<EngineEvent>`

Required runtime behavior:

- `new(config)` creates a runtime with generated `agentId`
- `init()` records the spawned event and leaves the runtime idle
- `record_message(role, content)` appends one message
- `mark_running()` updates status to `running` and records start events
- `mark_completed(content, duration_ms)` updates status, stores last task result, records completion events, and may append an assistant message
- `mark_failed(error, duration_ms)` updates status, stores last task result, records failure events
- `stop()` updates status to `terminated` and records termination
- `snapshot()` returns a serializable view combining state and lightweight runtime context

The runtime in this slice is still lifecycle-oriented. It does not execute tools or call models yet.

---

## Runtime Snapshot Shape

The daemon should respond with runtime snapshots, not raw internal objects.

Snapshot fields:

- `state`
- `messageCount`
- `eventCount`
- `lastTask`

This keeps API responses stable and small while still exposing enough context to prove the daemon owns real runtime instances.

---

## Endpoint Design

### `POST /api/agents`

Creates and initializes one runtime instance.

Request body:

```json
{
  "name": "researcher",
  "model": "gpt-5.4",
  "bio": "Finds answers quickly",
  "provider": "openai",
  "knowledge": ["Rust", "TypeScript"],
  "settings": {
    "temperature": 0.2,
    "maxTokens": 2048
  }
}
```

Validation rules:

- `name` required, non-empty string
- `model` required, non-empty string
- other config fields optional
- string arrays must actually contain strings
- `settings` if present must be an object

Response:

- `201` with `{ "agent": { ...snapshot... } }`

### `GET /api/agents`

Lists all runtime snapshots:

```json
{
  "agents": []
}
```

### `GET /api/agents/:id`

Returns one runtime snapshot:

```json
{
  "agent": { ...snapshot... }
}
```

Response codes:

- `200` if found
- `404` if unknown

### `GET /api/agents/:id/memories/recent`

Returns recent memories filtered by the runtime's `agentId`.

Query parameters:

- `limit` optional, default `20`

Response:

```json
{
  "memories": []
}
```

Response codes:

- `200` if agent exists
- `404` if unknown
- `400` if `limit` is invalid

---

## State Ownership

`DaemonState` should own:

- one `MemoryManager`
- one `HashMap<AgentId, AgentRuntime>`
- the placeholder swarm coordinator until swarm work starts

The daemon remains the sole owner of runtime instances for now. No persistence is required in this slice.

---

## Testing Strategy

This work should be driven by daemon integration tests first.

Required tests:

1. `POST /api/agents` creates a runtime and returns `201`
2. `GET /api/agents` includes a created runtime
3. `GET /api/agents/:id` returns runtime snapshot details
4. `GET /api/agents/:id` returns `404` for unknown ids
5. `GET /api/agents/:id/memories/recent` filters memories by runtime agent id
6. invalid create payload returns `400`

`anima-core` should also get unit tests for runtime lifecycle behavior and snapshot bookkeeping.

---

## Non-Goals

- no `run()` loop parity with TS
- no model adapter integration
- no tool execution
- no provider/evaluator/plugin behavior beyond config storage
- no persistent runtime storage
- no swarm runtime orchestration yet

---

## Success Criteria

This slice is complete when:

- `anima-core` owns a real `AgentRuntime` type
- `anima-daemon` stores runtime instances, not plain metadata
- clients can create, list, and fetch runtime snapshots over HTTP
- clients can fetch recent memories for a runtime by agent id
- Rust unit and integration tests prove runtime ownership and daemon behavior
