# animaOS-SWARM Roadmap

## What It Is

A swarm framework — agents with personality, built to do work.

---

## Current State

| Area                                                | Status  |
| --------------------------------------------------- | ------- |
| Core runtime (Provider/Evaluator/Action)            | ✅ Done |
| Three strategies (supervisor, round-robin, dynamic) | ✅ Done |
| Persistent agent pool (start/dispatch/stop)         | ✅ Done |
| Parallel agent spawn                                | ✅ Done |
| BM25 memory + JSON persistence                      | ✅ Done |
| Event bus + TUI + full result view                  | ✅ Done |
| Test suite (157 tests)                              | ✅ Done |

---

## Phases

---

### Phase 1 — Quality & Observability

> Make what exists solid.

**1.1 Structured observability**

- Per-agent token cost breakdown (not just swarm total)
- Decision trace: which agent ran, what tool, what it returned, why
- Structured JSONL output that can be piped to external systems

**1.2 Confidence scoring as output contract**

- Agents return `{ text, confidence: 0–100 }` not just text
- Orchestrator exposes confidence in `TaskResult`
- Supervisor strategy drops results below configurable threshold

**1.3 Model as resource allocation**

- Agent config accepts `model: "inherit" | "haiku" | "sonnet" | "opus"`
- Orchestrator resolves `inherit` from calling context budget
- Gate agents use haiku by default (cheap pre-filter before expensive work)

**1.4 Memory confidence gating + categories**

- ObservationEvaluator asks LLM to rate confidence 0–1, drop below 0.7
- Add `category: "episodic" | "semantic" | "procedural"` to Memory type
- Feed existing memories into extraction prompt to prevent duplicates

**1.5 Rich agent personality**

Full character definition in `anima.yaml` — not coding-centric, works for any domain (legal, medical, creative, financial, research, anything):

```yaml
bio: string[]           # who they are — multiple facets, not a single line
lore: string[]          # backstory and context that colors their behavior
topics: string[]        # areas of expertise and genuine interest
adjectives: string[]    # personality descriptors compiled into prompt
style:
  all: string[]         # how they communicate in every context
  task: string[]        # how they approach work specifically
messageExamples:        # few-shot examples — the biggest quality lever
  - input: "..."
    output: "..."
knowledge: string[]     # domain knowledge chunks injected into context
expertise:              # any domain, not language/tool specific
  oncology: expert
  negotiation: intermediate
collaborationStyle:     # how they behave alongside other agents in a swarm
  - "challenges assumptions before accepting"
  - "defers to domain specialists"
confidenceBands:        # how they signal certainty in their output
  high: "I'm confident that..."
  low: "This needs verification but..."
```

- All fields optional — agents without them work exactly as today
- Compiled into system prompt automatically by AgentRuntime
- Designed to expand — new personality axes can be added without breaking changes

---

### Phase 2 — Parallel Execution & Smarter Routing

> Make it fast and make routing intelligent.

**2.1 Parallel task dispatch**

- Add `dispatchParallel(tasks[])` for independent tasks
- Results via `Promise.all`, returned as `TaskResult[]`
- Serial `dispatch()` remains the default safe path

**2.2 Generate → validate strategy**

- New strategy: fan out to N generator agents, launch one validator per result
- Validators are haiku (cheap), generators can be heavier
- Results below confidence threshold dropped before returning to caller

**2.3 Semantic agent routing**

- Agent configs gain `triggers: string[]` and `examples: string[]`
- Dynamic strategy routes tasks by keyword match first, LLM match fallback
- Removes hardcoded routing from coordinator

**2.4 Real-time shared memory**

- Agents write to shared `MemoryManager` mid-run via `memory_write` tool
- Other agents pick up writes on next turn via MemoryProvider

**2.5 Knowledge graph**

- Entities agents discover during a run (files, services, APIs, concepts, people)
- Relationships between them (`ServiceA → depends_on → DatabaseB`)
- `memory_link(entityA, relation, entityB)` tool agents call to record discoveries
- Agents query the graph before starting work — don't re-discover known facts
- Shared across all agents in a swarm, persisted across runs

---

### Phase 3 — Self-Healing & Adaptive Coordination

> Agents that fix themselves and improve over time.

**3.1 Stop hook loops (self-healing)**

- Task spec accepts `completionCondition` (XML tag pattern)
- If agent's final message doesn't match, task re-runs with prompt re-injected
- Max retries enforced as escape hatch, state in task metadata

**3.2 Phased orchestration with human gates**

- Workflow spec defines phases with `requireApproval: true`
- Coordinator pauses, emits `swarm:approval-required` event
- TUI prompts user, resumes on confirmation

**3.3 Sub-swarm routing (nested coordinators)**

- An agent can spawn its own `SwarmCoordinator` as a subtask
- Parent dispatches, child runs, result flows back
- Dynamic topology: agents spawning specialist sub-teams on demand

**3.4 Meta-agent policy monitor**

- Background agent watches swarm event logs
- Detects repeated failures, cost overruns, low-confidence patterns
- Proposes new guardrail rules as config files

---

### Phase 4 — Pluggability

> Safe, auditable, swappable.

**4.1 Storage provider abstraction**

- `IMemoryStorage` interface — default JSON, pluggable SQLite/Postgres/external API

**4.2 Managed permission tiers**

- `settings.json` gets `allowManagedHooksOnly: true`
- All guardrails must come from central policy registry when enabled

**4.3 Progressive agent knowledge loading**

- Agent configs gain `knowledge_modules: string[]`
- Orchestrator injects only matching chunks when task type matches
- Keeps context windows focused for specialist agents

---

## What Stays Out

| Feature                     | Why                                               |
| --------------------------- | ------------------------------------------------- |
| pgvector / PostgreSQL       | Too heavy, belongs in a dedicated cognitive layer |
| Heat scoring                | Same                                              |
| Knowledge graph             | Same                                              |
| Relationship / social graph | Not relevant to task swarms                       |
| Emergent role negotiation   | Too fragile for production use                    |
| Auto-compaction             | Too coupled to runtime internals                  |

---

## Priority Order

| #   | Item                                 | Why                                 |
| --- | ------------------------------------ | ----------------------------------- |
| 1   | Observability (1.1)                  | Can't trust what you can't see      |
| 2   | Confidence scoring (1.2)             | Quality gate on every result        |
| 3   | Parallel dispatch (2.1)              | Immediate performance win           |
| 4   | Model allocation (1.3)               | Cost control at scale               |
| 5   | Memory confidence + categories (1.4) | Better memory, small effort         |
| 6   | Rich agent personality (1.5)         | Agents that feel like someone       |
| 7   | Generate → validate (2.2)            | Biggest quality differentiator      |
| 8   | Real-time shared memory (2.4)        | Agents that actually coordinate     |
| 9   | Knowledge graph (2.5)                | Shared discovery across agents      |
| 10  | Stop hook loops (3.1)                | Self-healing without infrastructure |
| 11  | Sub-swarm routing (3.3)              | The moat                            |
| 12  | Pluggability (4.x)                   | Swappable backends                  |
