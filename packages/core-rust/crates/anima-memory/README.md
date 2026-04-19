# anima-memory

`anima-memory` is the memory service used by AnimaOS agents. It provides a `MemoryManager` that stores agent memories in an in-memory hash map, indexed with a BM25 full-text search engine for keyword-based retrieval. Persistence to a JSON file is optional. There are no external database dependencies and no async runtime requirements.

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

When a storage file is configured, call `load()` once at startup to restore memories from disk. Call `save()` to flush the current state to disk — this is not done automatically on `add()`, so callers are responsible for deciding when to persist.

## Search

`search(query, opts)` runs BM25 ranking over the full-text index. The index is built from each memory's `content`, `memory_type`, `agent_name`, and any `tags`. No embeddings or external models are involved.

`MemorySearchOptions` fields:

| Field | Type | Description |
|---|---|---|
| `agent_id` | `Option<String>` | Restrict results to a specific agent ID |
| `agent_name` | `Option<String>` | Restrict results to a specific agent name |
| `memory_type` | `Option<MemoryType>` | Restrict results to one memory type |
| `limit` | `Option<usize>` | Maximum number of results (default: 10) |
| `min_importance` | `Option<f64>` | Exclude memories below this importance threshold |

Results are returned as `Vec<MemorySearchResult>`, which extends `Memory` with a `score: f64` field indicating BM25 relevance rank.

## Other API

- `get_recent(RecentMemoryOptions)` — returns the most recently added memories, sorted by creation time. Accepts `agent_id`, `agent_name`, and `limit` filters.
- `forget(id)` — removes a single memory by ID from both the store and the index.
- `clear(agent_id)` — removes all memories, or all memories for a specific agent if an ID is provided.
- `size()` — returns the total number of stored memories.

## Importance

`importance` must be a finite `f64` in the range `[0.0, 1.0]`. `add()` returns `Err(MemoryError::InvalidImportance)` if this constraint is violated.
