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
| `DATABASE_URL` | No | Postgres connection string. Required when `ANIMAOS_RS_PERSISTENCE_MODE=postgres`. Postgres mode runs migrations, stores host snapshots in `host_snapshots`, and injects `SqlxPostgresAdapter` for step-log persistence plus completed-tool reuse for retried requests that repeat the same metadata retry key. |
| `ANIMAOS_RS_HOST` | No | Bind host (default `127.0.0.1`). |
| `ANIMAOS_RS_PORT` | No | Bind port (default `8080`). |
| `ANIMAOS_RS_MAX_REQUEST_BYTES` | No | Request body size limit in bytes (default `65536` / 64 KB). |
| `ANIMAOS_RS_REQUEST_TIMEOUT_SECS` | No | Per-request timeout in seconds for standard routes and blocking `/run` endpoints (default `30`). |
| `ANIMAOS_RS_PERSISTENCE_MODE` | No | Persistence mode: `memory` (default) or `postgres`. `postgres` requires `DATABASE_URL` and fails startup if Postgres is unavailable or migrations fail. |
| `ANIMAOS_RS_CONTROL_PLANE_FILE` | No | Host-owned JSON snapshot file for registered agents, swarms, and their latest runtime snapshots. When omitted in Postgres mode, the daemon stores the control-plane snapshot in Postgres `host_snapshots`. |
| `ANIMAOS_RS_MEMORY_FILE` | No | Host-owned JSON snapshot file for runtime memories, entities, relationships, and temporal records. Mutating memory routes/tools/evaluators autosave the snapshot. Set only one of this and `ANIMAOS_RS_MEMORY_SQLITE_FILE`; when both are omitted in Postgres mode, memory snapshots use Postgres `host_snapshots`. |
| `ANIMAOS_RS_MEMORY_SQLITE_FILE` | No | Host-owned SQLite snapshot file for runtime memories, entities, relationships, temporal records, and, by default, embedding vectors. Memory snapshots use the daemon `memory_store_snapshots` table; embeddings use separate `memory_embeddings` tables. |
| `ANIMAOS_RS_MAX_CONCURRENT_RUNS` | No | Max number of concurrent `/api/agents/{id}/run` and `/api/swarms/{id}/run` requests before the daemon returns `503 Service Unavailable` (default `8`). |
| `ANIMAOS_RS_MAX_BACKGROUND_PROCESSES` | No | Max number of concurrently running `bg_start` processes allowed by the daemon tool surface (default `8`). |
| `ANIMAOS_RS_MEMORY_EMBEDDINGS` | No | Runtime memory embedding mode: `local` (default), `fastembed`, `ollama`, `openai`, `openai-compatible`, or `disabled`. `local` is deterministic and cheap for tests. `fastembed` runs a real local ONNX embedding model in process. Provider modes call an OpenAI-compatible `/embeddings` endpoint. |
| `ANIMAOS_RS_MEMORY_TEXT_ANALYZER` | No | BM25 text analyzer profile: `multilingual`/`unicode` (default, language-neutral Unicode tokenization with CJK character/bigram terms). Core search does not use language-specific stop-word removal or stemming; Arabic scriptio continua, Thai dictionary segmentation, and Indic grapheme/word-boundary handling need future segmenter work. |
| `ANIMAOS_RS_MEMORY_EMBEDDING_MODEL` | No | Embedding model. `fastembed` defaults to `intfloat/multilingual-e5-small` and also accepts aliases such as `multilingual-e5-base`, `multilingual-e5-large`, `bge-m3`, and `paraphrase-multilingual-minilm-l12-v2`. Provider modes default to `text-embedding-3-small` for `openai`/`openai-compatible` and `nomic-embed-text` for `ollama`. |
| `ANIMAOS_RS_MEMORY_EMBEDDING_DIMENSIONS` | No | Expected embedding vector size. `local` defaults to `96` with a minimum of `24`; `fastembed` uses the model's fixed dimension and rejects mismatches; `openai` defaults to `1536`; `ollama` defaults to `768`. |
| `ANIMAOS_RS_MEMORY_EMBEDDINGS_BASE_URL` | No | OpenAI-compatible embedding base URL. Defaults to `https://api.openai.com/v1` for OpenAI-compatible modes and `http://127.0.0.1:11434/v1` for `ollama`. Provider-specific `OPENAI_BASE_URL` and `OLLAMA_BASE_URL` are also recognized. |
| `ANIMAOS_RS_MEMORY_EMBEDDINGS_API_KEY` | No | API key for provider-backed memory embeddings. `openai` requires this or `OPENAI_API_KEY`; `ollama` and `openai-compatible` treat it as optional for local/private endpoints. |
| `ANIMAOS_RS_MEMORY_EMBEDDINGS_TIMEOUT_MS` | No | HTTP timeout for provider-backed embedding calls (default `15000`). |
| `ANIMAOS_RS_MEMORY_EMBEDDINGS_CACHE_DIR` | No | Cache directory for `fastembed` model files. Defaults to fastembed's standard cache directory. |
| `ANIMAOS_RS_MEMORY_EMBEDDINGS_SHOW_DOWNLOAD_PROGRESS` | No | Whether `fastembed` shows model download progress on first use (default `true`). |
| `ANIMAOS_RS_MEMORY_EMBEDDINGS_SQLITE_FILE` | No | SQLite file for embedding vectors. When omitted and `ANIMAOS_RS_MEMORY_SQLITE_FILE` is set, vectors use the same SQLite file in separate `memory_embeddings` tables. |

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
| `GET` | `/api/memories/readiness` | Memory readiness report. Runs the baseline memory eval harness and returns embedding provider, model, vector count, persistence status, total checks, passed checks, and failure messages. |
| `GET` | `/api/memories/{memory_id}/trace` | Return an evidence trace for one memory, including the memory, relationships that cite it, and involved entities. |
| `POST` | `/api/memories/retention` | Apply an explicit retention/decay policy. Optional fields: `maxAgeMillis`, `minImportance`, `maxMemories`, and `decayHalfLifeMillis`. Returns decayed memory adjustments plus removed memory and relationship IDs. |
| `POST` | `/api/memories/relationships` | Create or update a directed memory relationship edge. Required fields: `sourceAgentId`, `sourceAgentName`, `targetAgentId`, `targetAgentName`, `relationshipType`. Optional fields: `sourceKind`, `targetKind` (`agent`, `user`, `system`, `external`; default `agent`), `summary`, `strength`, `confidence`, `evidenceMemoryIds`, `tags`, `roomId`, `worldId`, `sessionId`. |
| `GET` | `/api/memories/relationships` | List relationship edges. Optional `?entityId=`, `?agentId=`, `?sourceKind=`, `?sourceAgentId=`, `?targetKind=`, `?targetAgentId=`, `?relationshipType=`, `?roomId=`, `?worldId=`, `?sessionId=`, `?minStrength=`, `?minConfidence=`, `?limit=`. |

Set `ANIMAOS_RS_MEMORY_SQLITE_FILE=/path/to/memories.sqlite` to load the daemon-owned memory snapshot on startup and autosave runtime memory writes from HTTP routes, tools, runtime evaluators, and retention policy runs. For lightweight JSON memory persistence, set `ANIMAOS_RS_MEMORY_FILE=/path/to/memories.json` instead; embeddings remain in process unless `ANIMAOS_RS_MEMORY_EMBEDDINGS_SQLITE_FILE` is also set. Set only one memory store variable. BM25 memory search uses the multilingual analyzer and does not remove stop words or stem terms by language. Runtime evaluation uses evaluated writes for reflection evidence, extracts explicit user-stated preference/remember facts, indexes stored memories for semantic recall, and links the responding agent to the user entity when request metadata includes `userId`/`userName`.

### Swarms

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/swarms` | List all registered swarm snapshots. |
| `POST` | `/api/swarms` | Create a swarm. Body includes `strategy`, `manager`, `workers`, and optional runtime limits such as `maxTurns`. Returns `201` with the created swarm state. |
| `GET` | `/api/swarms/{swarm_id}` | Get one swarm snapshot. |
| `POST` | `/api/swarms/{swarm_id}/run` | Dispatch a task (`{"text":"..."}`) to the swarm. Blocks until completion and returns the updated swarm state plus task result. |
| `GET` | `/api/swarms/{swarm_id}/events` | Subscribe to the swarm's server-sent event stream. Event names include lifecycle updates plus task, tool, and token events. |

---

## Local development

For the normal workspace loop, run:

```bash
bun dev --host rust
```

To run only the host process through Nx, use:

```bash
bun x nx run rust-daemon:dev
```

If you want the raw daemon process without the Nx wrapper, use the `cargo run -p anima-daemon` profiles below.

For local memory development, the simplest durable profile is:

```bash
ANIMAOS_RS_MEMORY_SQLITE_FILE=./data/runtime-memories.sqlite \
ANIMAOS_RS_MEMORY_EMBEDDINGS=local \
ANTHROPIC_API_KEY=sk-ant-... \
cargo run -p anima-daemon
```

On Windows, if a running daemon keeps `target/debug/anima-daemon.exe` locked during validation, rerun tests with an isolated cargo target dir:

```bash
CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon bun x nx run rust-daemon:test --skipNxCache
```

Contributor-focused local workflow docs live in `apps/contributor-docs/src/content/docs/hosts/rust-daemon/local.mdx`.

---

## Starting the daemon

```bash
# From the workspace root
ANTHROPIC_API_KEY=sk-ant-... cargo run -p anima-daemon

# With Postgres persistence for step logs, control plane, and memory snapshots
ANIMAOS_RS_PERSISTENCE_MODE=postgres \
DATABASE_URL=postgres://user:pass@localhost/anima \
  ANTHROPIC_API_KEY=sk-ant-... \
  cargo run -p anima-daemon

# With durable runtime memory in SQLite
ANIMAOS_RS_MEMORY_SQLITE_FILE=./data/runtime-memories.sqlite \
  ANTHROPIC_API_KEY=sk-ant-... \
  cargo run -p anima-daemon

# With restart recovery for the agent/swarm control plane
ANIMAOS_RS_CONTROL_PLANE_FILE=./data/control-plane.json \
  ANTHROPIC_API_KEY=sk-ant-... \
  cargo run -p anima-daemon

# With real local multilingual semantic memory embeddings
ANIMAOS_RS_MEMORY_SQLITE_FILE=./data/runtime-memories.sqlite \
ANIMAOS_RS_MEMORY_EMBEDDINGS=fastembed \
ANIMAOS_RS_MEMORY_EMBEDDING_MODEL=intfloat/multilingual-e5-small \
  ANTHROPIC_API_KEY=sk-ant-... \
  cargo run -p anima-daemon

# With Ollama/OpenAI-compatible semantic memory embeddings
ANIMAOS_RS_MEMORY_SQLITE_FILE=./data/runtime-memories.sqlite \
ANIMAOS_RS_MEMORY_EMBEDDINGS=ollama \
ANIMAOS_RS_MEMORY_EMBEDDING_MODEL=nomic-embed-text \
  ANTHROPIC_API_KEY=sk-ant-... \
  cargo run -p anima-daemon
```

Verify it is up:

```bash
curl http://127.0.0.1:8080/health
# {"status":"ok"}

curl http://127.0.0.1:8080/ready
# {"status":"ready","controlPlaneDurability":"postgres",...}
```

Browse the Scalar UI at `http://127.0.0.1:8080/docs`.

---

## Online staging profile

Use the Dockerfile from the repository root:

```bash
docker build -f hosts/rust-daemon/Dockerfile -t anima-daemon:staging .
```

For staging, set `ANIMAOS_RS_PERSISTENCE_MODE=postgres` and `DATABASE_URL`, leave `ANIMAOS_RS_CONTROL_PLANE_FILE`, `ANIMAOS_RS_MEMORY_FILE`, and `ANIMAOS_RS_MEMORY_SQLITE_FILE` unset, and run one daemon replica. In that profile, Postgres stores step logs plus control-plane and memory snapshots in `host_snapshots`.

Vectors remain process-local for v1 staging. Start with `ANIMAOS_RS_MEMORY_EMBEDDINGS=local`; if cold-start vector rebuilds become expensive and your platform has a persistent volume, set `ANIMAOS_RS_MEMORY_EMBEDDINGS_SQLITE_FILE=/var/lib/anima/memory-embeddings.sqlite`. See `.env.staging.example` for the full recommended env profile.

---

## Startup sequence

1. Read `ANIMAOS_RS_HOST`, `ANIMAOS_RS_PORT`, `ANIMAOS_RS_MAX_REQUEST_BYTES`, `ANIMAOS_RS_REQUEST_TIMEOUT_SECS`, `ANIMAOS_RS_PERSISTENCE_MODE`, `ANIMAOS_RS_CONTROL_PLANE_FILE`, `ANIMAOS_RS_MAX_CONCURRENT_RUNS`, and `ANIMAOS_RS_MAX_BACKGROUND_PROCESSES`, then bind the TCP listener.
2. Build `RuntimeModelAdapter::from_env()` to load provider API keys and base URLs.
3. Initialize `tracing` so every HTTP request is logged with method, URI, latency, and `x-request-id`, and log the daemon as an `ephemeral` control plane at startup.
4. If persistence mode is `postgres`, require `DATABASE_URL`, connect, run embedded migrations from `./migrations`, and fail startup immediately if any step fails.
5. Inject `SqlxPostgresAdapter` into shared daemon state so all new agents get step persistence automatically, including reuse of completed tool results when a caller retries with the same metadata retry key.
6. Configure memory persistence: explicit JSON/SQLite env vars win, otherwise Postgres mode stores the memory snapshot in `host_snapshots`.
7. Configure control-plane persistence: explicit `ANIMAOS_RS_CONTROL_PLANE_FILE` wins, otherwise Postgres mode stores registered agents and swarms in `host_snapshots`.
8. Start Axum with request-id propagation, tracing middleware, request timeouts for both standard and blocking `/run` routes, a semaphore-backed concurrent-run admission limit, readiness and metrics endpoints, and graceful shutdown on `Ctrl+C`.

---

## Operational notes

- Without `ANIMAOS_RS_CONTROL_PLANE_FILE` or Postgres mode, the daemon acts as an `ephemeral` control plane and agent/swarm registrations live only in memory.
- With `ANIMAOS_RS_CONTROL_PLANE_FILE` or Postgres mode, the daemon restores registered agents, registered swarms, latest snapshots, and swarm message history after process restart. Work that was marked running before the restart is restored as failed/interrupted; the daemon does not resume a model turn from the middle.
- Postgres persistence stores step logs, control-plane snapshots, and memory snapshots in the host database. Step logs also let retried runs reuse completed tool results when the request repeats the same metadata retry key.
- Readiness reports `database: "missing"` and returns `503` when `ANIMAOS_RS_PERSISTENCE_MODE=postgres` is set without a working database adapter.
- Metrics include configured limits such as `anima_daemon_max_concurrent_runs` and `anima_daemon_max_background_processes`, plus current counts like `anima_daemon_agents`, `anima_daemon_swarms`, and `anima_daemon_background_processes`.

---

## Architecture note

This host implements `ModelAdapter` (via `RuntimeModelAdapter`) and
`DatabaseAdapter` (via `SqlxPostgresAdapter`) from the reusable Rust core,
wiring those infrastructure-free crates to real provider APIs and Postgres at
the process boundary.
