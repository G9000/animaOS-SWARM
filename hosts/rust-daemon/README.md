# anima-daemon

`anima-daemon` is the runnable Rust host in `hosts/rust-daemon`. It is the
current Axum HTTP/SSE boundary for animaOS, wiring the reusable crates in
`packages/core-rust` to real infrastructure such as model providers, optional
Postgres persistence, and streaming clients.

---

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `ANTHROPIC_API_KEY` | Yes (for Anthropic models) | API key for the Anthropic provider. Aliases: `ANTHROPIC_KEY`, `ANTHROPIC_TOKEN`, `CLAUDE_API_KEY`. |
| `OPENAI_API_KEY` | Yes (for OpenAI models) | API key for OpenAI-compatible providers. Aliases: `OPENAI_KEY`, `OPENAI_TOKEN`. |
| `DATABASE_URL` | No | Postgres connection string. When set, the daemon connects, runs migrations, and injects `SqlxPostgresAdapter` into all agents for step-log persistence. Absent means in-memory only, with no step durability. |
| `ANIMAOS_RS_HOST` | No | Bind host (default `127.0.0.1`). |
| `ANIMAOS_RS_PORT` | No | Bind port (default `8080`). |
| `ANIMAOS_RS_MAX_REQUEST_BYTES` | No | Request body size limit in bytes (default `65536` / 64 KB). |
| `ANIMAOS_RS_REQUEST_TIMEOUT_SECS` | No | Per-request timeout for non-streaming HTTP routes (default `30`). |
| `ANIMAOS_RS_PERSISTENCE_MODE` | No | Persistence mode: `memory` (default) or `postgres`. `postgres` requires `DATABASE_URL` and fails startup if Postgres is unavailable or migrations fail. |

Other provider keys follow the same pattern: `GOOGLE_API_KEY`, `GROQ_API_KEY`,
`MOONSHOT_API_KEY`, `OLLAMA_API_KEY`, and so on. Moonshot/Kimi uses the
OpenAI-compatible endpoint at `https://api.moonshot.ai/v1`. The model to use is
specified per-agent in the request body (`model`), not via a single global env
var.

The daemon emits structured logs through `tracing` and adds `x-request-id` to
HTTP responses so request logs and client-visible responses can be correlated.

---

## HTTP API

This host serves both top-level operational/docs endpoints and `/api/*`
application endpoints. The summary below matches the live router in
`hosts/rust-daemon/src/routes/mod.rs`.

### Operational and docs routes

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Liveness check. Always returns `200 OK` with `{"status":"ok"}`. |
| `GET` | `/api/health` | Same health payload as `/health`. |
| `GET` | `/openapi.json` | OpenAPI document for the live daemon routes. |
| `GET` | `/docs/` | Swagger UI for exploring the daemon API in a browser. |

### Agents

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/agents` | List all registered agent snapshots. |
| `POST` | `/api/agents` | Create an agent. Body: `AgentConfig` JSON. Returns `201` with the created snapshot. |
| `GET` | `/api/agents/{agent_id}` | Get one agent snapshot. |
| `DELETE` | `/api/agents/{agent_id}` | Remove an agent runtime and return a deleted flag. |
| `POST` | `/api/agents/{agent_id}/run` | Run the agent with `{"text":"..."}`. Blocks until completion and returns the updated snapshot plus task result. |
| `GET` | `/api/agents/{agent_id}/memories/recent` | Get recent memories for the agent. Optional `?limit=N`. |

### Memories

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/memories` | Store a memory. Required fields: `agentId`, `agentName`, `type`, `content`, `importance` (0-1). |
| `GET` | `/api/memories/search` | Keyword search. Required `?q=`. Optional `?type=`, `?agentId=`, `?agentName=`, `?limit=`, `?minImportance=`. |
| `GET` | `/api/search` | Alias for `/api/memories/search` with the same query parameters and response shape. |
| `GET` | `/api/memories/recent` | Recent memories. Optional `?agentId=`, `?agentName=`, `?limit=`. |

### Swarms

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/swarms` | List all registered swarm snapshots. |
| `POST` | `/api/swarms` | Create a swarm. Body includes `strategy`, `manager`, `workers`, and optional runtime limits such as `maxTurns`. Returns `201` with the created swarm state. |
| `GET` | `/api/swarms/{swarm_id}` | Get one swarm snapshot. |
| `POST` | `/api/swarms/{swarm_id}/run` | Dispatch a task (`{"text":"..."}`) to the swarm. Blocks until completion and returns the updated swarm state plus task result. |
| `GET` | `/api/swarms/{swarm_id}/events` | Subscribe to the swarm's server-sent event stream. Event names include lifecycle updates plus task, tool, and token events. |

---

## Starting the daemon

```bash
# From the workspace root
ANTHROPIC_API_KEY=sk-ant-... cargo run -p anima-daemon

# With Postgres persistence
ANIMAOS_RS_PERSISTENCE_MODE=postgres \
DATABASE_URL=postgres://user:pass@localhost/anima \
  ANTHROPIC_API_KEY=sk-ant-... \
  cargo run -p anima-daemon
```

Verify it is up:

```bash
curl http://127.0.0.1:8080/health
# {"status":"ok"}
```

---

## Startup sequence

1. Read `ANIMAOS_RS_HOST`, `ANIMAOS_RS_PORT`, `ANIMAOS_RS_MAX_REQUEST_BYTES`, `ANIMAOS_RS_REQUEST_TIMEOUT_SECS`, and `ANIMAOS_RS_PERSISTENCE_MODE`, then bind the TCP listener.
2. Build `RuntimeModelAdapter::from_env()` to load provider API keys and base URLs.
3. Initialize `tracing` so every HTTP request is logged with method, URI, latency, and `x-request-id`.
4. If persistence mode is `memory`, start without Postgres and log that choice explicitly.
5. If persistence mode is `postgres`, require `DATABASE_URL`, connect, run embedded migrations from `./migrations`, and fail startup immediately if any step fails.
6. Inject `SqlxPostgresAdapter` into shared daemon state so all new agents get step persistence automatically.
7. Start Axum with request-id propagation, tracing middleware, request timeouts for non-streaming routes, and graceful shutdown on `Ctrl+C`.

---

## Architecture note

This host implements `ModelAdapter` (via `RuntimeModelAdapter`) and
`DatabaseAdapter` (via `SqlxPostgresAdapter`) from the reusable Rust core,
wiring those infrastructure-free crates to real provider APIs and Postgres at
the process boundary.
