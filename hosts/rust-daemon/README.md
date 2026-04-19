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
| `DATABASE_URL` | No | Postgres connection string. When set, the daemon connects, runs migrations, and injects `SqlxPostgresAdapter` into all agents for step-log persistence. Absent → in-memory only, no step durability. |
| `ANIMAOS_RS_HOST` | No | Bind host (default `127.0.0.1`). |
| `ANIMAOS_RS_PORT` | No | Bind port (default `8080`). |
| `ANIMAOS_RS_MAX_REQUEST_BYTES` | No | Request body size limit in bytes (default `65536` / 64 KB). |

Other provider keys follow the same pattern: `GOOGLE_API_KEY`, `GROQ_API_KEY`,
`OLLAMA_API_KEY`, etc. The model to use is specified per-agent in the request
body (`"model"` field), not via a global env var.

---

## HTTP API

All routes are prefixed with `/api`. The bare `/health` path is also accepted
for load-balancer probes.

### Health

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Liveness check. Always returns `200 OK` with `{"status":"ok"}`. |
| `GET` | `/api/health` | Same as above. |
| `GET` | `/openapi.json` | OpenAPI spec for the daemon routes. |
| `GET` | `/docs` | Swagger UI for exploring the daemon API in a browser. |

### Agents

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/agents` | Create an agent. Body: `AgentConfig` JSON. Returns `201` with agent snapshot. |
| `GET` | `/api/agents` | List all agents. Returns snapshot array sorted by creation time. |
| `GET` | `/api/agents/:id` | Get a single agent snapshot. |
| `POST` | `/api/agents/:id/run` | Run the agent with `{"text":"…"}`. Blocks until completion; returns agent snapshot + task result. |
| `GET` | `/api/agents/:id/memories/recent` | Recent memories for this agent. Optional `?limit=N`. |

### Memories

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/memories` | Store a memory. Required fields: `agentId`, `agentName`, `type`, `content`, `importance` (0–1). |
| `GET` | `/api/memories/search` | Keyword search. Required `?q=`. Optional `?type=`, `?agentId=`, `?agentName=`, `?limit=`, `?minImportance=`. |
| `GET` | `/api/memories/recent` | Recent memories. Optional `?agentId=`, `?agentName=`, `?limit=`. |

### Swarms

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/swarms` | Create and start a swarm. Body includes `strategy`, `manager` config, and `workers` array. Returns `201` with swarm state. |
| `GET` | `/api/swarms/:id` | Get swarm state snapshot. |
| `POST` | `/api/swarms/:id/run` | Dispatch a task (`{"text":"…"}`) to the swarm. Blocks until complete; returns swarm state + result. |
| `GET` | `/api/swarms/:id/events` | Subscribe to swarm events as server-sent events (SSE). Events: `swarm:created`, `swarm:running`, `swarm:completed`. |

---

## Starting the daemon

```bash
# From the workspace root
ANTHROPIC_API_KEY=sk-ant-... cargo run -p anima-daemon

# With Postgres persistence
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

1. Read `ANIMAOS_RS_HOST` / `ANIMAOS_RS_PORT` / `ANIMAOS_RS_MAX_REQUEST_BYTES` and bind the TCP listener.
2. Build `RuntimeModelAdapter::from_env()` — reads all provider API keys and base URLs from the environment once.
3. Attempt to connect Postgres using `DATABASE_URL`; skip silently if the variable is absent.
4. If connected, run embedded migrations (`./migrations`) to create the `step_log` table.
5. If migrations succeed, wrap the pool in `SqlxPostgresAdapter` and inject it into shared daemon state so all new agents get step persistence automatically.
6. Start Axum with the configured router and begin serving requests.

---

## Architecture note

This host implements `ModelAdapter` (via `RuntimeModelAdapter`) and
`DatabaseAdapter` (via `SqlxPostgresAdapter`) from the reusable Rust core,
wiring those infrastructure-free crates to real provider APIs and Postgres at
the process boundary.
