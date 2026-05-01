# Swarm Agent Architecture

## System Overview

```mermaid
graph TD
    Client["Client\n(SDK / CLI / UI)"]
    Daemon["Rust Daemon\n:8080"]
    SC["SwarmCoordinator"]
    MB["MessageBus\n(agent ↔ agent)"]
    EB["EventBus\n(system events → SSE)"]
    MA["ModelAdapter\n(OpenAI / Anthropic / …)"]
    MGR["Manager\nAgentRuntime"]
    W1["Worker 1\nAgentRuntime"]
    W2["Worker 2\nAgentRuntime"]
    WN["Worker N\nAgentRuntime"]

    Client -->|"POST /api/swarms"| Daemon
    Client -->|"POST /api/swarms/:id/run"| Daemon
    Client -->|"GET /api/swarms/:id/events (SSE)"| Daemon
    Daemon --> SC
    SC --> MB
    SC --> EB
    SC --> MGR
    SC --> W1
    SC --> W2
    SC --> WN
    MGR --> MA
    W1 --> MA
    W2 --> MA
    WN --> MA
    EB -->|"stream"| Client
```

## Swarm Lifecycle

```mermaid
sequenceDiagram
    participant C as Client
    participant SC as SwarmCoordinator
    participant W as Workers
    participant M as Manager

    Note over C,M: Persistent mode (multi-task)
    C->>SC: start()
    SC->>W: spawnAgent() × N (parallel)
    W-->>SC: handles in pool

    loop Each task
        C->>SC: dispatch(task)
        SC->>M: spawnAgent(manager + strategy tools)
        SC->>SC: strategy(ctx)
        M-->>SC: TaskResult
        SC->>M: terminate()
        SC-->>C: TaskResult
    end

    C->>SC: stop()
    SC->>W: terminate() × N

    Note over C,M: Single-shot mode (run)
    C->>SC: run(task)
    SC->>W: spawnAgent() × N
    SC->>M: spawnAgent()
    SC->>SC: strategy(ctx)
    M-->>SC: TaskResult
    SC->>W: terminateAll()
    SC-->>C: TaskResult
```

## Strategy: Supervisor

Manager breaks the task down and delegates subtasks to workers in parallel. Manager synthesises all results. Agents can also use swarm messaging tools to send direct handoffs or broadcast context through the shared message bus.

```mermaid
sequenceDiagram
    participant M as Manager
    participant W1 as Worker 1
    participant W2 as Worker 2
    participant WN as Worker N

    Note over M: Spawned with delegate_task tool
    M->>+W1: delegate_task("subtask A")
    M->>+W2: delegate_task("subtask B")
    M->>+WN: delegate_task("subtask N")
    W1-->>-M: result A
    W2-->>-M: result B
    WN-->>-M: result N
    Note over M: Synthesise → final answer
```

**Best for:** tasks that decompose cleanly into parallel subtasks (research + write + review, data pipeline stages).

---

## Strategy: Dynamic

Manager acts as an orchestrator with a `choose_speaker` tool. It picks which worker speaks next based on the evolving conversation, building shared context turn by turn.

```mermaid
sequenceDiagram
    participant M as Manager
    participant W1 as Worker 1
    participant W2 as Worker 2

    Note over M: Spawned with choose_speaker tool
    M->>W1: choose_speaker("researcher", instruction)
    W1-->>M: response → added to chat history
    M->>W2: choose_speaker("writer", instruction + history)
    W2-->>M: response → added to chat history
    M->>M: choose_speaker("DONE")
    Note over M: Synthesise history → final answer
```

**Best for:** tasks that need adaptive, back-and-forth reasoning where the manager decides who contributes next.

---

## Strategy: Round Robin

All agents (manager + workers) take turns in a fixed cycle. Each agent sees the full conversation history before responding. No tool use — pure sequential turns.

```mermaid
sequenceDiagram
    participant W1 as Agent 1 (manager)
    participant W2 as Agent 2
    participant WN as Agent N

    Note over W1,WN: Turn 0 → maxTurns
    W1->>W1: run(task)
    W1-->>W2: history updated
    W2->>W2: run(task + history)
    W2-->>WN: history updated
    WN->>WN: run(task + history)
    WN-->>W1: history updated
    Note over W1,WN: Final output = full history
```

**Best for:** creative or iterative tasks where each agent builds on what came before (story generation, code review cycles, debate).

---

## Agent Internals

```mermaid
graph LR
    Input["Input\n(task string)"]
    AR["AgentRuntime"]
    Tools["Action handlers\n(tools)"]
    MA["ModelAdapter"]
    MB["MessageBus\n(send / broadcast)"]
    EB["EventBus\n(agent:spawned, tool:before…)"]
    Output["TaskResult"]

    Input --> AR
    AR -->|"messages + system"| MA
    MA -->|"text / tool_call"| AR
    AR -->|"tool_call"| Tools
    Tools -->|"result"| AR
    AR -->|"events"| EB
    AR -->|"send(targetId, msg)"| MB
    MB -->|"message history"| AR
    AR --> Output
```

Swarm snapshots include the global message history as `messages`, so direct sends and broadcasts are inspectable through `POST /api/swarms/:id/run`, `GET /api/swarms/:id`, list responses, and SSE state payloads.

## Event Flow (SSE)

Events emitted by `EventBus` are streamed to connected clients via `GET /api/swarms/:id/events`.

| Event | When |
|---|---|
| `swarm:created` | Swarm starts |
| `agent:spawned` | An AgentRuntime is initialised |
| `tool:before` | Agent is about to call a tool |
| `tool:after` | Tool call completed |
| `agent:completed` | Agent finished a run |
| `swarm:completed` | Task done, result ready |
| `swarm:stopped` | Swarm shut down |
