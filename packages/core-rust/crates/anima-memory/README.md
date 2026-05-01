# anima-memory

`anima-memory` is the memory service used by AnimaOS agents. It provides a `MemoryManager` that stores agent memories in an in-memory hash map, indexed with a BM25 full-text search engine for keyword-based retrieval. It also stores durable memory entities, first-class relationship edges, evaluated write decisions, and hybrid recall scores so identity, collaboration history, and evidence-backed retrieval can be modeled separately from free-text memories. Persistence to a JSON file or SQLite snapshot is optional. There are no external service dependencies and no async runtime requirements.

## Quick usage

```rust
use anima_memory::{MemoryManager, MemoryType, NewMemory, MemorySearchOptions};

// Create a manager (in-memory only)
let mut manager = MemoryManager::new();

// Add a memory
let memory = manager.add(NewMemory {
    agent_id: "agent-42".to_string(),
    agent_name: "planner".to_string(),
    memory_type: MemoryType::Observation,
    content: "The user prefers concise responses.".to_string(),
    importance: 0.8,
    tags: Some(vec!["preference".to_string()]),
    scope: None,
    room_id: None,
    world_id: None,
    session_id: None,
})?;

// Search for memories
let results = manager.search(
    "concise responses",
    MemorySearchOptions {
        agent_id: Some("agent-42".to_string()),
        min_importance: Some(0.5),
        limit: Some(5),
        ..Default::default()
    },
);

for result in results {
    println!("[{:.2}] {}", result.score, result.content);
}
```

## Memory types

`MemoryType` classifies what kind of information a memory holds:

| Variant | String key | Intended use |
|---|---|---|
| `Fact` | `"fact"` | Persistent, objective facts about the world or the user |
| `Observation` | `"observation"` | Real-time observations made during a task or conversation |
| `TaskResult` | `"task_result"` | Output or outcome recorded after completing a task |
| `Reflection` | `"reflection"` | Higher-order inferences drawn from other memories |

The string keys are used internally in the search index and in JSON serialization.

## Memory scope

Every stored memory has a `MemoryScope`: `shared`, `private`, or `room`. New memories default to `room` when `room_id` is supplied and `private` otherwise. Optional `room_id`, `world_id`, and `session_id` fields let callers keep short-term room/session recall separate from durable per-agent facts while still using one indexed store.

## Memory entities

Memory entities are durable identity records for agents, users, systems, and external actors. They use the same endpoint kind values as relationships: `agent`, `user`, `system`, and `external`.

Use `upsert_entity(NewMemoryEntity)` to create or update an entity. Adding an agent memory auto-registers that agent entity when the memory has a non-empty agent ID/name, and relationship upserts auto-register both endpoints. Entity records include a stable ID, display name, aliases, optional summary, and timestamps.

Use `list_entities(MemoryEntityOptions)` to filter by ID, kind, name, alias, and limit. Use `get_entity(kind, id)` for direct lookup.

## Agent relationships

Agent relationships are directed graph edges between memory entities. They default to agent-to-agent edges, but each endpoint has a `source_kind` / `target_kind` of `agent`, `user`, `system`, or `external`, so the same store can represent agent-agent and agent-user links without pretending users are agents. They are not search documents; they are structured relationship records that can cite memory IDs as evidence.

Use `upsert_agent_relationship(NewAgentRelationship)` to create or update one edge. Edges are deduplicated by `source_agent_id`, `target_agent_id`, `relationship_type`, and `world_id`. Updating an existing edge refreshes strength/confidence and merges evidence IDs and tags.

Relationship fields include:

| Field | Description |
|---|---|
| `source_kind` / `target_kind` | Endpoint kind: `agent`, `user`, `system`, or `external` |
| `source_agent_id` / `target_agent_id` | Directed edge endpoints |
| `relationship_type` | Extensible string such as `collaborates_with`, `trusts`, `delegates_to`, or `blocks` |
| `summary` | Optional human/model-readable description of the relationship |
| `strength` | `0.0..=1.0` edge strength |
| `confidence` | `0.0..=1.0` confidence in the relationship |
| `evidence_memory_ids` | Memory IDs that support the relationship |
| `room_id`, `world_id`, `session_id` | Optional context coordinates |

Use `list_agent_relationships(AgentRelationshipOptions)` to filter by any entity ID, agent ID, directed endpoint, endpoint kind, relationship type, context IDs, minimum strength/confidence, and limit.

## Storage

### In-memory only

```rust
let manager = MemoryManager::new();
```

Memories live only in process memory. Nothing is written to disk.

### JSON file persistence

```rust
let mut manager = MemoryManager::with_storage_file("/var/data/memories.json");
manager.load()?; // load existing memories from disk on startup
```

When a storage file is configured, call `load()` once at startup to restore memories and agent relationships from disk. Call `save()` to flush the current state to disk — this is not done automatically on `add()` or relationship upserts, so callers are responsible for deciding when to persist. New saves use an object format with `memories` and `agentRelationships`; older array-only memory files still load.

The current storage object contains `memories`, `entities`, and `agentRelationships`. Older object files without `entities` still load, and entities are backfilled from loaded memories and relationships when possible.

### SQLite persistence

```rust
let mut manager = MemoryManager::with_sqlite_file("/var/data/memories.sqlite");
manager.load()?;
// ... mutate memories, entities, relationships, retention state ...
manager.save()?;
```

SQLite persistence stores the same snapshot as JSON in normalized tables for memories, tags, entities, aliases, relationship edges, relationship evidence, and relationship tags. `save()` replaces the previous snapshot in a transaction, so removed memories and pruned relationships do not linger as stale rows. The `sqlite` crate feature is enabled by default for this package; disable default features if an embedding host wants memory without the SQLite adapter.

## Search

`search(query, opts)` runs BM25 ranking over the full-text index. The index is built from each memory's `content`, `memory_type`, `scope`, `agent_name`, optional room/world/session IDs, and any `tags`. BM25 search does not require embeddings or external models.

`recall(query, opts)` performs hybrid recall over BM25 results, recent memories, relationship evidence, and importance. `recall_with_vector_index(query, opts, Some(index))` also accepts an optional `MemoryVectorIndex` implementation, letting hosts plug in embeddings or a vector database later without making the core crate depend on one.

Each `MemoryRecallResult` includes the memory plus score breakdowns: `lexical_score`, `vector_score`, `relationship_score`, `recency_score`, and `importance_score`.

Use `trace_memory(memory_id)` to inspect why a memory participates in the graph. It returns the memory, relationships that cite it as evidence, and the involved durable entities. This is the core primitive behind evidence trace UI surfaces.

### Vector adapter

The core crate includes `InMemoryVectorIndex`, a host-agnostic cosine-similarity adapter for memory embeddings. Hosts provide a `MemoryTextEmbedder` implementation, upsert memory embeddings with `upsert_text()` or `upsert_embedding()`, and pass the index into `recall_with_vector_index()`. The adapter validates IDs, dimensions, finite values, and non-zero vectors, but it does not call model providers itself.

```rust
use anima_memory::{InMemoryVectorIndex, MemoryTextEmbedder, MemoryVectorError};

struct HostEmbedder;

impl MemoryTextEmbedder for HostEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryVectorError> {
        host_embedding_for(text).map_err(|_| MemoryVectorError::EmbeddingUnavailable)
    }
}

let mut index = InMemoryVectorIndex::new(HostEmbedder);
index.upsert_embedding(memory.id.clone(), vec![0.12, 0.98, 0.03])?;
let recall = manager.recall_with_vector_index("release briefing style", opts, Some(&index));
```

## Memory evaluation

`evaluate_new_memory(memory, options)` scores a candidate write before storage. It can return `Store`, `Merge`, or `Ignore`, with a reason, suggested importance, and duplicate memory ID when applicable. `add_evaluated(memory, options)` uses that decision directly: distinct memories are stored with the suggested importance, duplicates and low-value short memories are not appended.

## Retention and decay

`apply_retention(MemoryRetentionPolicy)` applies an explicit maintenance pass over the in-memory store. A policy can remove memories older than `max_age_millis`, remove memories below `min_importance`, keep only the strongest `max_memories`, and decay memory importance by `decay_half_life_millis`. Removed evidence IDs are pruned from relationships, and relationships with no remaining evidence are removed. The returned `MemoryRetentionReport` lists decayed memories, removed memory IDs, and removed relationship IDs.

## Memory eval harness

`run_memory_eval_cases(&baseline_memory_eval_cases())` runs deterministic quality checks over evaluated writes, relationship-backed recall, agent-agent handoff memory, room/world isolation, vector recall false-positive suppression, trace evidence, retention behavior, decay behavior, and SQLite reload coverage. The returned `MemoryEvalReport` exposes `passed()`, `total_checks()`, `passed_checks()`, and `failure_messages()` so embedding/vector adapters can prove they improve recall without breaking existing memory guarantees.

In the Nx workspace, run the focused harness with `bun x nx run core-rust:memory-eval`; full core validation remains `bun x nx run core-rust:test`.

`MemorySearchOptions` fields:

| Field | Type | Description |
|---|---|---|
| `agent_id` | `Option<String>` | Restrict results to a specific agent ID |
| `agent_name` | `Option<String>` | Restrict results to a specific agent name |
| `memory_type` | `Option<MemoryType>` | Restrict results to one memory type |
| `scope` | `Option<MemoryScope>` | Restrict results to `shared`, `private`, or `room` memories |
| `room_id` | `Option<String>` | Restrict results to a room |
| `world_id` | `Option<String>` | Restrict results to a world |
| `session_id` | `Option<String>` | Restrict results to a session |
| `limit` | `Option<usize>` | Maximum number of results (default: 10) |
| `min_importance` | `Option<f64>` | Exclude memories below this importance threshold |

Results are returned as `Vec<MemorySearchResult>`, which extends `Memory` with a `score: f64` field indicating BM25 relevance rank.

## Other API

- `get_recent(RecentMemoryOptions)` — returns the most recently added memories, sorted by creation time. Accepts `agent_id`, `agent_name`, `scope`, `room_id`, `world_id`, `session_id`, and `limit` filters.
- `recall(query, MemoryRecallOptions)` — hybrid recall over lexical, relationship, recent, and importance signals.
- `recall_with_vector_index(query, MemoryRecallOptions, Option<&dyn MemoryVectorIndex>)` — hybrid recall with an optional vector search adapter.
- `InMemoryVectorIndex` — in-process cosine-similarity vector index that implements `MemoryVectorIndex`.
- `trace_memory(id)` — returns the memory, citing relationships, and involved entities for evidence inspection.
- `evaluate_new_memory(NewMemory, MemoryEvaluationOptions)` — evaluates whether a candidate memory should be stored, merged, or ignored.
- `add_evaluated(NewMemory, MemoryEvaluationOptions)` — evaluates and conditionally stores a memory.
- `apply_retention(MemoryRetentionPolicy)` — applies decay/removal rules and returns a retention report.
- `run_memory_eval_cases(&[MemoryEvalCase])` — runs reusable memory quality scenarios and returns a structured report.
- `upsert_entity(NewMemoryEntity)` — creates or updates a durable memory entity.
- `list_entities(MemoryEntityOptions)` — lists identity records by entity filters.
- `get_entity(kind, id)` — returns one entity by endpoint kind and ID.
- `forget(id)` — removes a single memory by ID from both the store and the index.
- `clear(agent_id)` — removes all memories, or all memories for a specific agent if an ID is provided.
- `size()` — returns the total number of stored memories.
- `entity_count()` — returns the total number of stored memory entities.
- `upsert_agent_relationship(NewAgentRelationship)` — creates or updates a directed relationship between agents.
- `list_agent_relationships(AgentRelationshipOptions)` — lists relationship edges with endpoint/context filters.
- `forget_agent_relationship(id)` — removes one relationship edge.
- `relationship_count()` — returns the total number of stored relationship edges.

## Importance

`importance` must be a finite `f64` in the range `[0.0, 1.0]`. `add()` returns `Err(MemoryError::InvalidImportance)` if this constraint is violated.
