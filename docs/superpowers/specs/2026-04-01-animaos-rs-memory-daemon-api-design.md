# animaOS Rust Memory Daemon API Design

**Date:** 2026-04-01
**Status:** Draft

---

## Goal

Expose the Rust `anima-memory` port through `anima-daemon` so the daemon starts doing real engine work instead of only serving `/health`.

This slice validates the daemon-first architecture with the smallest useful API surface that exercises the Rust memory subsystem end to end.

---

## Scope

This design adds three daemon endpoints:

- `POST /api/memories`
- `GET /api/memories/search`
- `GET /api/memories/recent`

These routes are intentionally limited to the in-process `MemoryManager` already ported into Rust. They do not add agent runtime behavior, swarm execution, SSE, or cross-process persistence wiring.

---

## Why This Slice

This is the right next step because:

- the Rust memory port already exists and is verified
- the daemon currently has no meaningful engine API beyond health
- memory endpoints are easy to validate with deterministic integration tests
- this keeps the daemon-first boundary honest without dragging in model/runtime complexity too early

Alternatives considered:

1. Port the agent runtime next.
   This is higher leverage later, but it couples daemon work to model execution, event flow, and much more unstable behavior.

2. Add daemon CRUD without search.
   This is narrower, but it fails to exercise the BM25 and filtering behavior we just ported.

Recommended approach: add memory create, search, and recent endpoints first.

---

## Endpoint Design

### `POST /api/memories`

Creates one memory entry in the daemon-owned `MemoryManager`.

Request body:

```json
{
  "agentId": "agent-1",
  "agentName": "researcher",
  "type": "fact",
  "content": "Rust daemon memory endpoint created",
  "importance": 0.8,
  "tags": ["rust", "memory"]
}
```

Validation rules:

- `agentId` required, non-empty string
- `agentName` required, non-empty string
- `type` required, must be one of `fact`, `observation`, `task_result`, `reflection`
- `content` required, non-empty string
- `importance` required, numeric, expected in `0..=1`
- `tags` optional, if present must be an array of strings

Responses:

- `201` with the created memory on success
- `400` with `{ "error": "..." }` for invalid input

### `GET /api/memories/search`

Queries BM25-backed memory search.

Query parameters:

- `q` required
- `agentId` optional
- `agentName` optional
- `type` optional
- `limit` optional, default `10`
- `minImportance` optional, default `0`

Response:

```json
{
  "results": []
}
```

Validation rules:

- missing `q` returns `400`
- invalid `type`, `limit`, or `minImportance` returns `400`

### `GET /api/memories/recent`

Returns newest-first memories from the in-process manager.

Query parameters:

- `agentId` optional
- `agentName` optional
- `limit` optional, default `20`

Response:

```json
{
  "memories": []
}
```

Validation rules:

- invalid `limit` returns `400`

---

## State Ownership

The daemon should own exactly one in-process `MemoryManager` instance for now.

Implementation direction:

- keep `MemoryManager` as daemon state, not a request-local value
- use one shared owner in `Daemon`
- pass request handling through that owner

This means:

- memories created through `POST /api/memories` are immediately searchable through `GET /api/memories/search`
- recency and search behavior are consistent across requests in a single daemon process

No persistence file is required for this slice. The API can stay in-memory only unless a file path is already configured locally inside the daemon.

---

## Transport Shape

The Rust daemon should align with the existing TS server style where reasonable:

- JSON request/response bodies
- `/api/...` route prefix
- `404` for unknown routes
- `400` for input validation failures
- `201` for create success
- `200` for search/read success

The current `/health` route may remain available as-is for now. We do not need to rename it in this slice.

---

## Error Handling

All validation errors should be converted into stable JSON error responses:

```json
{ "error": "agentId is required" }
```

Design rules:

- do not panic on malformed input
- do not silently coerce invalid numeric values
- reject unknown memory types explicitly
- keep messages short and concrete

---

## Testing Strategy

This work should be implemented with daemon integration tests first.

Required tests:

1. `POST /api/memories` creates a memory and returns `201`
2. `POST /api/memories` rejects missing required fields with `400`
3. `GET /api/memories/search?q=...` returns created memory content
4. `GET /api/memories/search` rejects missing `q`
5. `GET /api/memories/recent` returns newest-first memories
6. search filters work through HTTP for at least one filter path

The integration tests should drive a live daemon over TCP in the same style as the existing health test.

---

## Non-Goals

- no agent runtime endpoints in this slice
- no swarm endpoints in this slice
- no SSE
- no auth
- no external storage backend
- no attempt to match TS search/task-history routes yet

---

## Success Criteria

This slice is complete when:

- the daemon owns a shared `MemoryManager`
- clients can create memories over HTTP
- clients can search and list recent memories over HTTP
- validation failures return stable `400` JSON errors
- the full Rust workspace test suite stays green
