# Rust Core Architecture

The Rust core is the heart of animaOS. It lives in `packages/core-rust/crates/` and follows a single rule: **core defines behavior, hosts provide reality**.

This means the core has zero I/O dependencies — no HTTP clients, no database drivers, no filesystem access. It operates on pure data structures and trait interfaces. A host (like the Rust Daemon) wraps the core and provides the "real world": network calls to LLM providers, persistence to disk, and HTTP APIs.

## Architecture

**Host Layer** — Axum HTTP server, provider adapters, snapshot I/O

**Rust Core** — Three crates with zero I/O dependencies:

| Crate | Key Types |
|-------|-----------|
| `anima-core` | `AgentRuntime`, `ModelAdapter` (trait), `Provider` (trait), `Evaluator` (trait), `EventBus` |
| `anima-memory` | `MemoryManager`, `BM25`, vector search, temporal KG, relationship graph |
| `anima-swarm` | `SwarmCoordinator`, `MessageBus`, strategies (supervisor, round-robin, dynamic) |

## Crate: `anima-core`

Agent runtime, event system, and the task loop.

### `AgentRuntime`

The main struct. Created from an `AgentConfig` + a `ModelAdapter`, then driven by calling `run(input)`.

```rust
pub struct AgentRuntime {
    state: AgentState,
    messages: Vec<Message>,
    providers: Vec<Arc<dyn Provider>>,
    evaluators: Vec<Arc<dyn Evaluator>>,
    model_adapter: Arc<dyn ModelAdapter>,
    db: Option<Arc<dyn DatabaseAdapter>>,
}
```

**Task loop:**
1. Inject provider context into the prompt
2. Call `ModelAdapter::generate()`
3. If the model returns a tool call, execute it and feed the result back (up to 8 iterations)
4. Run evaluators on the final response
5. Return the `TaskResult`

### Key traits

| Trait | Role |
|-------|------|
| `ModelAdapter` | Abstracts LLM calls. Hosts implement this for OpenAI, Anthropic, Ollama, etc. |
| `Provider` | Injects dynamic context (current time, user info, search results) before each generation |
| `Evaluator` | Post-processing hook after each response. Used for scoring, reflection, follow-up actions |
| `DatabaseAdapter` | Persistence boundary. The host provides SQLite, JSON, or any storage backend |

### Events

`AgentRuntime` emits `EngineEvent`s for every significant state change:

| Event Type | Fired When |
|------------|-----------|
| `AgentSpawned` | Runtime created |
| `TaskStarted` | `run()` called |
| `ToolInvoked` | Model requested a tool call |
| `ToolCompleted` | Tool handler finished |
| `MessageGenerated` | Model produced a response |
| `TaskCompleted` | Task loop finished |

Hosts can attach an event listener to stream these over SSE or log them.

## Crate: `anima-memory`

Hybrid memory system with lexical search, vector similarity, temporal reasoning, and relationship graphs.

### `MemoryManager`

The central struct. Stores memories in-memory with no external database required.

```rust
pub struct MemoryManager {
    memories: HashMap<String, Memory>,
    memory_entities: HashMap<String, MemoryEntity>,
    agent_relationships: HashMap<String, AgentRelationship>,
    temporal_facts: HashMap<String, TemporalFact>,
    temporal_relationships: HashMap<String, TemporalRelationship>,
    index: BM25,
}
```

### Search pipeline

| Stage | Algorithm | Purpose |
|-------|-----------|---------|
| Lexical | BM25 | Keyword relevance |
| Vector | Cosine similarity | Semantic meaning |
| Temporal | Time-decay scoring | Recent events |
| Relationship | Graph proximity | Connected agents/entities |

### Recall fusion

`MemoryManager::recall()` fuses all four signals into a single ranked list:

```rust
pub fn recall(
    &self,
    query: &str,
    options: &MemoryRecallOptions,
) -> Result<Vec<MemoryRecallResult>, MemoryError>
```

Each result contains individual scores:
- `lexical_score` — BM25 contribution
- `vector_score` — embedding similarity
- `relationship_score` — graph proximity
- `temporal_score` — time relevance
- `recency_score` — raw recency
- `importance_score` — stored importance

### Evaluation

Before storing a memory, `evaluate()` returns a decision:

| Decision | Meaning |
|----------|---------|
| `Store` | Save as a new memory |
| `Merge` | Combine with an existing duplicate |
| `Ignore` | Too low quality or redundant |

### Snapshot boundary

`MemoryManager::snapshot()` serializes the entire state. The host calls this on shutdown and `replace_snapshot()` on startup.

> **Note:** Because the core has no filesystem access, the host is responsible for reading/writing the snapshot bytes. This makes the core trivial to test and embed in any environment.

## Crate: `anima-swarm`

Multi-agent coordination via strategies and a message bus.

### `SwarmCoordinator`

Owns the swarm lifecycle: create, run, and teardown.

```rust
pub struct SwarmCoordinator {
    state: SwarmState,
    message_bus: MessageBus,
    strategy_fn: Arc<CoordinatorStrategyFn>,
}
```

### Strategies

Strategies are pluggable functions that decide how work flows between agents.

| Strategy | Behavior |
|----------|----------|
| `Supervisor` | Manager delegates tasks, reviews results, and synthesizes the final answer |
| `RoundRobin` | Each worker takes a turn in fixed order |
| `Dynamic` | Manager dynamically assigns tasks based on context and worker capabilities |

### Message bus

`MessageBus` is an in-memory pub/sub channel between swarm agents. Agents `send()`, `broadcast()`, and read from their `inbox()`. The coordinator wires these together.

## Design Rules

1. **Zero I/O** — Core crates never open sockets, files, or database connections
2. **Trait boundaries** — All external concerns are abstracted behind traits (`ModelAdapter`, `DatabaseAdapter`)
3. **Snapshot persistence** — State is fully serializable via `snapshot()` / `replace_snapshot()`
4. **Pure algorithms** — BM25, cosine similarity, and temporal scoring are implemented from scratch with no external ML dependencies

## Building & Testing

```bash
# Run all Rust core tests
cargo test --workspace

# Run with nextest (if installed)
cargo nextest run --workspace

# Memory benchmark (LoCoMo eval)
bun x nx run core-rust:memory-locomo --skipNxCache

# Build release
cargo build --release
```

## Adding a New Provider

Implement `ModelAdapter` for your provider:

```rust
use anima_core::{ModelAdapter, ModelGenerateRequest, ModelGenerateResponse};

pub struct MyProviderAdapter;

#[async_trait]
impl ModelAdapter for MyProviderAdapter {
    async fn generate(
        &self,
        request: ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        // Make your HTTP call here (in the host, not the core)
        // Parse response and return
    }
}
```

The adapter lives in the **host**, not the core. The core only sees the trait.
