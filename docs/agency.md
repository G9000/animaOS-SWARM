# Agencies

An **agency** is a saved, portable definition of a multi-agent team. Where a "swarm" is the runtime concept (a coordinator with strategy), an agency is the *authored artifact*: a cast of characters with org-chart structure, personality, expertise, and skills, written to `anima.yaml`.

## Mental model

```
agency  =  org chart  +  personalities  +  declared skills
swarm   =  the runtime that brings them to life
```

You don't configure microservices — you describe a team. An LLM materialises that description into a structured `anima.yaml` you can read, edit, version-control, and re-launch.

---

## Creating an agency

```bash
animaos create
```

Three interactive prompts:

| Prompt | What it asks |
|---|---|
| **Agency name** | Used as the directory name and identifier |
| **What does this agency do?** | Plain-English mandate the LLM uses to design the team |
| **Team size (2-10)** | Total agents including the orchestrator — defaults to 4 |

Or non-interactive:

```bash
animaos create content-team \
  --description "Research topics and write articles" \
  --provider ollama \
  --model gemma4:31b \
  --size 6 \
  --yes
```

### Provider options

Any provider supported by the runtime works for *generating* the team — `openai`, `anthropic`, `ollama`, `groq`, `xai`, `openrouter`, `mistral`, `together`, `deepseek`, `fireworks`, `perplexity`, `moonshot`, `google`/`gemini`. Local Ollama needs no API key, defaults to `http://127.0.0.1:11434/v1`.

### Team size and overlap

Size is bounded **2–10** (1 orchestrator + 1–9 workers).

**Overlap is allowed when it adds value.** The generator may produce 2–3 agents in similar roles with different angles — `researcher_quantitative` + `researcher_qualitative`, or `writer_long_form` + `writer_punchy` — so the orchestrator can compare outputs or run parallel exploration. It will not produce verbatim duplicates.

---

## The `anima.yaml` schema

```yaml
name: AnimaOS Martech
description: A martech taskforce promoting AnimaOS
model: gemma4:31b
provider: ollama
strategy: supervisor

orchestrator:
  name: anima_director              # snake_case identifier
  position: Chief Marketing Officer # NEW — job title
  bio: ...                          # 1-2 sentences, personality + expertise
  lore: ...                         # 1-2 sentences of backstory
  adjectives: [visionary, decisive, organized]
  topics: [campaign orchestration, KPI definition]
  knowledge: [Go-to-market frameworks, ICP definition]
  style: Direct, authoritative.
  system: Coordinates the workflow...
  tools:                            # NEW — declared skill slugs
    - delegate_task
    - kpi_review
    - strategy_synthesis

agents:
  - name: growth_engine
    position: Head of Growth
    bio: ...
    tools: [web_search, ab_test_design, cac_analysis]
    # ... same shape as orchestrator
```

### Field reference

| Field | Required | Purpose |
|---|---|---|
| `name` | yes | Snake_case slug, used internally and in delegation |
| `position` | no | Real-world job title — shapes how the agent reasons about its scope |
| `bio` | yes | Personality + expertise, fed into the system prompt |
| `lore` | no | Backstory — adds character without bloating the prompt |
| `adjectives` | no | 3–5 trait words; reinforces tone |
| `topics` | no | 3–6 expertise tags — agent's "territory" |
| `knowledge` | no | 2–4 specific things the agent knows deeply |
| `style` | no | How they communicate (formal, terse, playful…) |
| `system` | yes | Core instruction — what they do, decide, and own |
| `tools` | no | Skill slugs the agent can invoke (see Skills below) |

### Strategy

`anima.yaml` always saves with `strategy: supervisor` — the orchestrator delegates to workers in parallel and synthesises. To change to `dynamic` or `round-robin`, edit the YAML directly. See [`swarm-architecture.md`](./swarm-architecture.md) for what each strategy does.

---

## Skills (tools)

Each agent declares a `tools` array of snake_case slugs — capabilities the LLM should reach for. The generator suggests skills that fit each role:

```yaml
- name: market_oracle
  position: Director of Market Intelligence
  tools:
    - web_search
    - competitor_scrape
    - trend_forecast
```

> **Current state:** skills are *declarative* unless the slug is registered in the daemon tool registry. Launch binds registered tool slugs into executable handlers and ignores unregistered slugs with a warning, so generated agencies can still run while custom skills remain metadata.

When skills are wired, the runtime will:
1. Look up each slug in the registered tool set.
2. Inject matching `Action`s into the agent's `tools` field at spawn time.
3. The agent uses them as native tool calls, with `tool:before` / `tool:after` events emitted to the SSE stream.

---

## Seeding agent memory

Each agent has an `agents/<slug>/memory/` folder. Drop a `seed.json` there to pre-load that agent's memory store on every launch:

```json
[
  { "type": "fact", "content": "Our ICP is mid-market SaaS, 50–500 employees", "importance": 0.9, "tags": ["icp"] },
  { "type": "observation", "content": "Q1 demos converted 22% — well above the 14% baseline", "importance": 0.7 }
]
```

| Field | Required | Notes |
|---|---|---|
| `type` | yes | `fact`, `observation`, `task_result`, or `reflection` |
| `content` | yes | The memory itself |
| `importance` | no | 0–1; defaults to 0.5 |
| `tags` | no | Free-form labels |

The launch flow loads every `seed.json` it finds, resolves agent name → daemon agent ID once the swarm is created, and POSTs each entry to `/api/memories` *before* the first run. Failures are logged but never block a launch — a typo in one seed file won't take down the team.

A single object instead of an array is also accepted, for one-off memories. Bad JSON or invalid types throw at launch start with the offending agent name in the message.

---

## Launching

```bash
cd content-team
animaos launch "Write an article about multi-agent AI systems"
```

The launch flow:
1. Loads `anima.yaml`
2. Translates it into a `SwarmConfig` (manager + workers)
3. Sends `POST /api/swarms` to the daemon
4. Seeds memories from `agents/<slug>/memory/seed.json` (if present)
5. Sends `POST /api/swarms/:id/run`
6. Streams events from `GET /api/swarms/:id/events` (SSE)
7. Prints the orchestrator's synthesised result

---

## Editing an agency

Everything in `anima.yaml` is meant to be hand-edited. Common adjustments:

| Want to… | Edit |
|---|---|
| Switch model or provider | top-level `model` / `provider` |
| Change coordination | top-level `strategy` |
| Tighten an agent's voice | `style`, `adjectives` |
| Expand expertise | `topics`, `knowledge` |
| Rewire delegation | `system` of the orchestrator |
| Add a skill | append to that agent's `tools` array |

---

## Design philosophy

**Three principles that shape this:**

1. **Org-chart, not microservice config.** Agents have job titles, personalities, and territory. Reading an agency YAML should feel like reading a team page, not a Helm chart.

2. **Author once, run anywhere.** The same YAML works against any provider — Ollama for local iteration, Anthropic / OpenAI for production — by changing a single field.

3. **Strategic overlap > false uniqueness.** Two researchers from different angles often beat one researcher trying to do both. The size+overlap design lets you scale that intentionally.
