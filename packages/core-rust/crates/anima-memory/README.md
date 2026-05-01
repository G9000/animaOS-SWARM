# anima-memory

`anima-memory` is the host-agnostic memory engine used by AnimaOS agents. It provides a `MemoryManager` that stores agent memories in an in-memory hash map, indexed with a BM25 full-text search engine for keyword-based retrieval. It also stores durable memory entities, first-class relationship edges, evaluated write decisions, temporal facts/relationships, and hybrid recall scores so identity, collaboration history, and evidence-backed retrieval can be modeled separately from free-text memories. The crate has no file, database, external service, or async runtime dependencies; hosts own persistence.

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

The string keys are used internally in the search index and by host adapters that serialize domain records.

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

## Temporal facts and relationships

Temporal records model durable extracted truth separately from raw episodic memories. Raw `Memory` rows remain immutable evidence; temporal facts and temporal relationships point back to evidence memory IDs and carry validity windows so callers can ask what is true now versus what was true at an earlier instant.

Use `add_temporal_fact(NewTemporalFact)` for statements about an entity, such as a user preference or profile attribute. A temporal fact includes a subject endpoint, predicate, optional object endpoint, optional scalar value, `observed_at`, optional `valid_from` / `valid_to`, confidence, evidence memory IDs, optional context IDs, tags, and status.

Active temporal facts also participate in `recall(...)`. When a query is relevant to a currently valid fact, the fact boosts its evidence memories first and only falls back to subject/value text matches for facts that do not carry explicit evidence memory IDs. `MemoryRecallOptions.temporal_limit` controls how many active facts are considered; use `Some(0)` to disable this lane for a call. `MemoryRecallOptions.temporal_intent_terms` lets hosts add domain- or language-specific intent words beyond the built-in English defaults, and `MemoryRecallOptions.weights` can tune recall signal weights for benchmarks or host-specific ranking profiles.

Use `add_temporal_relationship(NewTemporalRelationship)` for time-varying directed graph edges, such as an agent's current trust in another agent, a user-agent collaboration link, or a relationship that supersedes an older belief. Relationship records include source/target endpoints, relationship type, optional summary, strength, confidence, observation and validity times, evidence memory IDs, context IDs, tags, and status.

Both temporal APIs support supersession. When a new record lists `supersedes_fact_ids` or `supersedes_relationship_ids`, matching older records are marked `TemporalRecordStatus::Superseded`; if they did not already have `valid_to`, it is set to the new record's `valid_from` or `observed_at`. Superseded and retracted records remain stored for audit and historical recall, but list queries hide inactive records by default.

Temporal query options:

| API | Filters |
|---|---|
| `list_temporal_facts(TemporalFactOptions)` | Subject kind/ID, predicate, object kind/ID, status, `valid_at`, context IDs, minimum confidence, inactive inclusion, limit |
| `list_temporal_relationships(TemporalRelationshipOptions)` | Source/target kind/ID, relationship type, status, `valid_at`, context IDs, minimum strength/confidence, inactive inclusion, limit |

`get_temporal_fact(id)`, `get_temporal_relationship(id)`, `forget_temporal_fact(id)`, and `forget_temporal_relationship(id)` provide direct record access. The core crate only stores and filters these temporal records; extraction, conflict policy, and host route shape remain host responsibilities.

## Host persistence boundary

`MemoryManager` is in-memory only. Core owns memory behavior and the domain model; hosts own reality such as files, databases, migrations, credentials, and startup/shutdown policy.

```rust
let manager = MemoryManager::new();
```

For host adapters, `snapshot()` exports a pure in-memory `MemoryManagerSnapshot`, and `replace_snapshot(snapshot)` hydrates a manager while rebuilding BM25 indexes and relationship/entity lookup state. JSON, SQLite, Postgres, Chroma, or any other durable adapter should live in the host or an adapter package and translate to/from that snapshot or public domain APIs.

## Search

`search(query, opts)` runs BM25 ranking over the full-text index. The index is built from each memory's `content`, `memory_type`, `scope`, `agent_name`, optional room/world/session IDs, and any `tags`. BM25 search does not require embeddings or external models.

Default BM25 query processing uses `TextAnalyzer::multilingual()`: Unicode-aware tokenization, no stop-word removal, no language-specific stemming, and CJK character/bigram tokens for text without whitespace. This keeps the production default language-neutral. The analyzer is strongest for whitespace-delimited Unicode scripts and CJK/Hangul/Kana text; Arabic scriptio continua, Thai dictionary segmentation, and Indic grapheme/word-boundary handling need a future segmenter before they should be treated as fully supported. `TextAnalyzer::unicode()` is retained as an alias for the multilingual analyzer.

Domain-specific query expansion is opt-in through `QueryExpander`; use `MemoryManager::with_query_expander(...)`, `MemoryManager::with_text_analyzer_and_query_expander(...)`, `BM25::with_expander(...)`, or `BM25::with_expander_and_analyzer(...)` when a host or benchmark has explicit expansion rules. The LOCOMO benchmark helpers, including `locomo_query_expander()`, are behind the `locomo-eval` Cargo feature and are not compiled or exported by default, so production memory recall does not inherit benchmark-specific synonyms by default.

`recall(query, opts)` performs hybrid recall over BM25 results, recent memories, relationship evidence, active temporal fact evidence, and importance. `recall_with_vector_index(query, opts, Some(index))` also accepts an optional `MemoryVectorIndex` implementation, letting hosts plug in embeddings or a vector database later without making the core crate depend on one.

Each `MemoryRecallResult` includes the memory plus score breakdowns: `lexical_score`, `vector_score`, `relationship_score`, `temporal_score`, `recency_score`, and `importance_score`.

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

`run_memory_eval_cases(&baseline_memory_eval_cases())` runs deterministic quality checks over evaluated writes, relationship-backed recall, agent-agent handoff memory, room/world isolation, vector recall false-positive suppression, trace evidence, retention behavior, and decay behavior. The returned `MemoryEvalReport` exposes `passed()`, `total_checks()`, `passed_checks()`, and `failure_messages()` so embedding/vector adapters can prove they improve recall without breaking existing memory guarantees.

With the `locomo-eval` feature enabled, `run_locomo_eval_cases(&locomo_smoke_eval_cases())` runs a LOCOMO-style long-memory smoke benchmark over single-hop profile recall, temporal preference updates, agent-agent handoff, speaker attribution, abstention on unknown answers, and semantic vector recall. It reports pass/fail plus `recall_at_k()`, `answer_coverage()`, and `false_positive_rate()`. The LOCOMO harness opts into `locomo_query_expander()` explicitly; the default `MemoryManager::new()` path remains domain-neutral. The public LOCOMO CSV and labeled benchmark JSON can be fetched into the local git-ignored cache with `bun x nx run core-rust:memory-locomo-fetch`. The production dataset target `bun x nx run core-rust:memory-locomo-dataset` reports pure core, LOCOMO-tuned, temporal-seeded, and eval-only temporal-rerank profiles over all cached benchmark conversation turns, scores category 1-4 questions against official evidence turn IDs, supports `LOCOMO_TEMPORAL_WEIGHT_SWEEP` and `LOCOMO_TEMPORAL_RERANK_WEIGHT_SWEEP`, and can print bounded miss diagnostics with question/evidence relation labels via `LOCOMO_MISS_REPORT_CATEGORY=3` or `LOCOMO_CATEGORY3_MISS_REPORT=1`.

In the Nx workspace, run the focused harness with `bun x nx run core-rust:memory-eval`, fetch LOCOMO data with `bun x nx run core-rust:memory-locomo-fetch`, run the LOCOMO-style smoke benchmark with `bun x nx run core-rust:memory-locomo`, run the labeled dataset benchmark with `bun x nx run core-rust:memory-locomo-dataset`, and use `bun x nx run core-rust:test` for full core validation.

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
- `recall(query, MemoryRecallOptions)` — hybrid recall over lexical, relationship, temporal fact, recent, and importance signals.
- `recall_with_vector_index(query, MemoryRecallOptions, Option<&dyn MemoryVectorIndex>)` — hybrid recall with an optional vector search adapter.
- `InMemoryVectorIndex` — in-process cosine-similarity vector index that implements `MemoryVectorIndex`.
- `trace_memory(id)` — returns the memory, citing relationships, and involved entities for evidence inspection.
- `evaluate_new_memory(NewMemory, MemoryEvaluationOptions)` — evaluates whether a candidate memory should be stored, merged, or ignored.
- `add_evaluated(NewMemory, MemoryEvaluationOptions)` — evaluates and conditionally stores a memory.
- `apply_retention(MemoryRetentionPolicy)` — applies decay/removal rules and returns a retention report.
- `snapshot()` / `replace_snapshot(MemoryManagerSnapshot)` — export or hydrate pure in-memory state for host-owned persistence adapters.
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
- `add_temporal_fact(NewTemporalFact)` — stores an evidence-backed temporal fact with validity and supersession metadata.
- `list_temporal_facts(TemporalFactOptions)` — lists temporal facts by subject, predicate, object, status, valid time, context, and confidence filters.
- `get_temporal_fact(id)` — returns one temporal fact by ID.
- `forget_temporal_fact(id)` — removes one temporal fact and prunes supersession references to it.
- `temporal_fact_count()` — returns the total number of stored temporal facts.
- `add_temporal_relationship(NewTemporalRelationship)` — stores an evidence-backed time-varying relationship edge.
- `list_temporal_relationships(TemporalRelationshipOptions)` — lists temporal relationships by endpoint, relationship type, status, valid time, context, strength, and confidence filters.
- `get_temporal_relationship(id)` — returns one temporal relationship by ID.
- `forget_temporal_relationship(id)` — removes one temporal relationship and prunes supersession references to it.
- `temporal_relationship_count()` — returns the total number of stored temporal relationships.

## Importance

`importance` must be a finite `f64` in the range `[0.0, 1.0]`. `add()` returns `Err(MemoryError::InvalidImportance)` if this constraint is violated.
