# anima-daemon

`anima-daemon` is the runnable Rust host in `hosts/rust-daemon`. It is the
current Axum HTTP/SSE boundary for animaOS, wiring the reusable crates in
`packages/core-rust` to real infrastructure such as model providers, optional
Postgres persistence, and streaming clients.

For an implementation walkthrough, see [Rust Daemon Architecture](../../docs/rust-daemon-architecture.md).

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
| `ANIMAOS_RS_REQUEST_TIMEOUT_SECS` | No | Per-request timeout in seconds for standard routes and blocking `/run` endpoints (default `30`). |
| `ANIMAOS_RS_PERSISTENCE_MODE` | No | Persistence mode: `memory` (default) or `postgres`. `postgres` requires `DATABASE_URL` and fails startup if Postgres is unavailable or migrations fail. |
| `ANIMAOS_RS_MAX_CONCURRENT_RUNS` | No | Max number of concurrent `/api/agents/{id}/run` and `/api/swarms/{id}/run` requests before the daemon returns `503 Service Unavailable` (default `8`). |
| `ANIMAOS_RS_MAX_BACKGROUND_PROCESSES` | No | Max number of concurrently running `bg_start` processes allowed by the daemon tool surface (default `8`). |

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
| `GET` | `/ready` | Readiness check. Returns `200 OK` with `{"status":"ready",...}` when the daemon can serve traffic, otherwise `503` with issues. |
| `GET` | `/metrics` | Prometheus-style metrics for readiness, memory/runtime counts, persistence mode, configured limits, and background-process health. |
| `GET` | `/api/health` | Same health payload as `/health`. |
| `GET` | `/api/ready` | Same readiness payload as `/ready`. |
| `GET` | `/openapi.json` | OpenAPI document for the live daemon routes. |
| `GET` | `/docs` | Scalar API reference for exploring the daemon API in a browser. |
| `GET` | `/docs/` | Scalar API reference for exploring the daemon API in a browser. |

### Agents

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/agents` | List all registered agent snapshots. |
| `POST` | `/api/agents` | Create an agent. Body: `AgentConfig` JSON. Returns `201` with the created snapshot. |
| `GET` | `/api/agents/{agent_id}` | Get one agent snapshot. |
| `DELETE` | `/api/agents/{agent_id}` | Remove an agent runtime and return a deleted flag. |
| `POST` | `/api/agents/{agent_id}/run` | Run the agent with `{"text":"..."}`. Blocks until completion and returns the updated snapshot plus task result. |
| `GET` | `/api/agents/{agent_id}/memories/recent` | Get recent memories for the agent. Optional `?limit=N`. |

### Agencies

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/agencies/create` | Generate an agency and materialize a CLI-style workspace under the daemon workspace root. Body includes `name`, `description`, `teamSize`, optional `provider`, optional `model`, optional `modelPool`, optional `outputDir`, optional `seedMemories`, and optional `overwrite`. Writes `anima.yaml`, `org-chart.mmd`, `README.md`, and `agents/*/profile.md` plus workspace placeholders, and can also write `agents/*/memory/seed.json` files with LLM-generated starter memories. The response includes the created file list plus seed-memory counts. |
| `POST` | `/api/agencies/generate` | Generate an agency draft from a plain-language description. Body includes `name`, `description`, `teamSize`, optional `provider`, optional `model`, and optional `modelPool`. Returns a mission, values, and normalized agent definitions that can be spawned into a swarm without unsupported tool slugs. |

### Memories

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/memories` | Store a memory. Required fields: `agentId`, `agentName`, `type`, `content`, `importance` (0-1). Optional fields: `scope` (`shared`, `private`, `room`), `roomId`, `worldId`, `sessionId`, and `tags`. |
| `GET` | `/api/memories/search` | Keyword search. Required `?q=`. Optional `?type=`, `?agentId=`, `?agentName=`, `?scope=`, `?roomId=`, `?worldId=`, `?sessionId=`, `?limit=`, `?minImportance=`. |
| `GET` | `/api/search` | Alias for `/api/memories/search` with the same query parameters and response shape. |
| `GET` | `/api/memories/recent` | Recent memories. Optional `?agentId=`, `?agentName=`, `?scope=`, `?roomId=`, `?worldId=`, `?sessionId=`, `?limit=`. |
| `POST` | `/api/memories/entities` | Create or update a memory entity. Required fields: `kind` (`agent`, `user`, `system`, `external`), `id`, `name`. Optional fields: `aliases`, `summary`. |
| `GET` | `/api/memories/entities` | List memory entities. Optional `?entityId=`, `?kind=`, `?name=`, `?alias=`, `?limit=`. |
| `POST` | `/api/memories/evaluations` | Evaluate a candidate memory without storing it. Accepts the memory create fields plus optional `minContentChars` and `minImportance`; returns `store`, `merge`, or `ignore`. |
| `POST` | `/api/memories/evaluated` | Evaluate and conditionally store a memory. Returns the evaluation plus the stored memory when the decision is `store`; duplicate/low-value candidates do not append new memory records. |
| `GET` | `/api/memories/recall` | Hybrid recall. Required `?q=`. Optional memory filters plus `?entityId=`, `?recallAgentId=`, `?limit=`, `?lexicalLimit=`, `?recentLimit=`, `?relationshipLimit=`. Returns score breakdowns for lexical, vector, relationship, recency, and importance signals. |
| `POST` | `/api/memories/relationships` | Create or update a directed memory relationship edge. Required fields: `sourceAgentId`, `sourceAgentName`, `targetAgentId`, `targetAgentName`, `relationshipType`. Optional fields: `sourceKind`, `targetKind` (`agent`, `user`, `system`, `external`; default `agent`), `summary`, `strength`, `confidence`, `evidenceMemoryIds`, `tags`, `roomId`, `worldId`, `sessionId`. |
| `GET` | `/api/memories/relationships` | List relationship edges. Optional `?entityId=`, `?agentId=`, `?sourceKind=`, `?sourceAgentId=`, `?targetKind=`, `?targetAgentId=`, `?relationshipType=`, `?roomId=`, `?worldId=`, `?sessionId=`, `?minStrength=`, `?minConfidence=`, `?limit=`. |

Set `ANIMAOS_RS_MEMORY_FILE=/path/to/memories.json` to load daemon runtime memories, entities, and relationships from a JSON file on startup and autosave memory writes from HTTP routes, tools, and runtime evaluators. Runtime evaluation now uses evaluated writes for reflection evidence and links the responding agent to the user entity when request metadata includes `userId`/`userName`.

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

curl http://127.0.0.1:8080/ready
# {"status":"ready","controlPlaneDurability":"ephemeral",...}
```

Browse the Scalar UI at `http://127.0.0.1:8080/docs`.

---

## Startup sequence

1. Read `ANIMAOS_RS_HOST`, `ANIMAOS_RS_PORT`, `ANIMAOS_RS_MAX_REQUEST_BYTES`, `ANIMAOS_RS_REQUEST_TIMEOUT_SECS`, `ANIMAOS_RS_PERSISTENCE_MODE`, `ANIMAOS_RS_MAX_CONCURRENT_RUNS`, and `ANIMAOS_RS_MAX_BACKGROUND_PROCESSES`, then bind the TCP listener.
2. Build `RuntimeModelAdapter::from_env()` to load provider API keys and base URLs.
3. Initialize `tracing` so every HTTP request is logged with method, URI, latency, and `x-request-id`, and log the daemon as an `ephemeral` control plane at startup.
4. If persistence mode is `memory`, start without Postgres and log that choice explicitly.
5. If persistence mode is `postgres`, require `DATABASE_URL`, connect, run embedded migrations from `./migrations`, and fail startup immediately if any step fails.
6. Inject `SqlxPostgresAdapter` into shared daemon state so all new agents get step persistence automatically.
7. Start Axum with request-id propagation, tracing middleware, request timeouts for both standard and blocking `/run` routes, a semaphore-backed concurrent-run admission limit, readiness and metrics endpoints, and graceful shutdown on `Ctrl+C`.

---

## Operational notes

- The daemon currently acts as an explicit `ephemeral` control plane. Agent and swarm runtime state lives in memory.
- Postgres persistence adds step-log durability, but it does not make the daemon itself stateful across process restarts.
- Readiness reports `database: "missing"` and returns `503` when `ANIMAOS_RS_PERSISTENCE_MODE=postgres` is set without a working database adapter.
- Metrics include configured limits such as `anima_daemon_max_concurrent_runs` and `anima_daemon_max_background_processes`, plus current counts like `anima_daemon_agents`, `anima_daemon_swarms`, and `anima_daemon_background_processes`.

---

## Architecture note

This host implements `ModelAdapter` (via `RuntimeModelAdapter`) and
`DatabaseAdapter` (via `SqlxPostgresAdapter`) from the reusable Rust core,
wiring those infrastructure-free crates to real provider APIs and Postgres at
the process boundary.
