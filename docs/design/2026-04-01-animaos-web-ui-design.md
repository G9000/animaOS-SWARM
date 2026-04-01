# AnimaOS Kit — Web UI Design Spec

**Date:** 2026-04-01  
**Branch:** feat/web-ui  
**Worktree:** `.worktrees/feat/web-ui`

---

## Overview

A real-time dashboard web UI for the AnimaOS server (`apps/server`), built inside the existing `apps/ui` React + Vite app. Connects to the server's REST API and WebSocket endpoint to display and control agents, swarms, search, and system health.

---

## Visual Design

**Style:** Cyberpunk monochrome with luxury dark gold accent.

| Token | Value |
|---|---|
| Background | `#000` / `#020202` |
| Surface | `#030303` / `#050505` |
| Border (inactive) | `#111` / `#1a1a1a` |
| Border (active) | `#c9a227` |
| Accent | `#c9a227` (dark gold) |
| Text (primary) | `#c9a227` |
| Text (secondary) | `#555` |
| Text (dim) | `#2a2a2a` |
| Font | `'Courier New', monospace` |
| Corner brackets | `1px solid #c9a227`, `7–18px` |
| Scanlines | `repeating-linear-gradient`, 4px pitch, 12% opacity |
| Border radius | **0** (sharp everywhere) |

---

## Layout — Sidebar + Split Pane (Layout C)

```
┌──────────┬──────────────────────────────────┐
│          │ // AGENTS               + NEW     │
│ SIDEBAR  ├──────────────────────────────────┤
│          │ agent list (top half)             │
│ ANIMA    │  ▶ agent-alpha  [IDLE]            │
│ OS       │    agent-beta   [BUSY]            │
│          ├──────────────────────────────────┤
│ ▶ AGENTS │ OUTPUT // agent-alpha             │
│   SWARMS │  $ task input          [RUN ▶]   │
│   SEARCH │  → streaming output...            │
│   HEALTH │  ✓ done                          │
│          │                                   │
│ SYS_OK   │                                   │
└──────────┴──────────────────────────────────┘
```

- Fixed left sidebar (`150px`) — logo, nav, status footer
- Right panel splits vertically: top 44% list, bottom 56% output/detail
- Active nav item: `border-left: 1px solid #c9a227` + gold text
- Selected list item: gold border + corner bracket decorations

---

## File Structure

```
apps/ui/src/
  store/
    index.ts          # Zustand store root
    agents.ts         # agents slice
    swarms.ts         # swarms slice
    ws.ts             # WebSocket connection state + event log
  hooks/
    useWebSocket.ts   # connects to ws://localhost:3000/ws, dispatches to store
    useHealth.ts      # polls /api/health every 30s
  components/
    Sidebar/
      Sidebar.tsx
      Sidebar.css
    AgentList/
      AgentList.tsx
      AgentList.css
    OutputPanel/
      OutputPanel.tsx
      OutputPanel.css
    SwarmList/
      SwarmList.tsx
    SearchPanel/
      SearchPanel.tsx
  App.tsx             # layout shell — sidebar + split pane
  styles/
    tokens.css        # CSS custom properties (colors, font)
    global.css        # resets, scanlines, base styles
```

---

## State Management — Zustand

Three slices composed into one store:

**`agents` slice**
```ts
{
  agents: Record<string, Agent>   // keyed by id
  selectedId: string | null
  setSelected: (id: string) => void
  upsertAgent: (agent: Agent) => void
  removeAgent: (id: string) => void
}
```

**`swarms` slice**
```ts
{
  swarms: Record<string, Swarm>
  upsertSwarm: (swarm: Swarm) => void
}
```

**`ws` slice**
```ts
{
  status: 'connecting' | 'open' | 'closed' | 'error'
  events: WsEvent[]               // ring buffer, last 200
  setStatus: (s: WsStatus) => void
  pushEvent: (e: WsEvent) => void
}
```

---

## WebSocket Server (new — must be built)

The `/ws` endpoint is currently **logged but not implemented** in `server.ts`. As part of this feature, a WebSocket server must be added to `apps/server`.

**Implementation plan for server side:**

- Add `ws` npm package to `apps/server`
- Upgrade `createServer()` in `server.ts` to attach a `WebSocketServer` to the same HTTP server
- On each WS connection, subscribe to all `EventBus` event types and broadcast JSON to that client
- On client disconnect, unsubscribe

**Real EventBus event types** (from `@animaOS-SWARM/core`):

```ts
"agent:spawned" | "agent:started" | "agent:completed" | "agent:failed" |
"agent:terminated" | "agent:message" | "task:started" | "task:completed" |
"task:failed" | "tool:before" | "tool:after" | "agent:tokens" |
"swarm:created" | "swarm:completed" | "swarm:stopped"
```

Each broadcast message shape:
```ts
{ type: EventType, agentId?: string, timestamp: number, data: unknown }
```

---

## WebSocket Client

`useWebSocket` hook connects on mount to `ws://localhost:3000/ws`.

- On open: sets `ws.status = 'open'`
- On message: parses JSON, routes by event `type`:
  - `agent:spawned` / `agent:started` / `agent:completed` / `agent:failed` → `upsertAgent` + `pushEvent`
  - `agent:message` / `task:started` / `task:completed` / `task:failed` / `tool:before` / `tool:after` → `pushEvent` (shown in OutputPanel for matching `agentId`)
  - `agent:tokens` → update token count in agent slice
  - `swarm:created` / `swarm:completed` / `swarm:stopped` → `upsertSwarm`
- On close/error: sets status, attempts reconnect with exponential backoff (1s → 2s → 4s → max 30s)

---

## Components

### Sidebar
- Logo (`ANIMAOS`), version, branch
- Nav items: AGENTS / SWARMS / SEARCH / HEALTH
- Active item tracked by local `useState` (no router needed)
- Footer: `SYS_OK` status, agent count, swarm count, uptime (from `useHealth`)

### AgentList
- Fetches `GET /api/agents` on mount (plain `fetch`, no library)
- WS `agent.updated` events keep list live
- Click → `store.setSelected(id)`
- "+ NEW_AGENT" button → inline form (name + model fields) → `POST /api/agents`
- Delete on selected agent → `DELETE /api/agents/:id`

### OutputPanel
- Reads `store.selectedId`, shows that agent's data
- Task input + `RUN ▶` → `POST /api/agents/:id/run`
- Output area renders `ws.events` filtered to selected agent
- Shows cursor `▌` while agent is `running`
- Falls back to "select an agent" prompt when nothing selected

### SwarmList
- Same pattern as AgentList — `GET /api/swarms`, WS live updates
- "+ NEW_SWARM" → inline form (strategy + manager + workers)
- Run task on selected swarm

### SearchPanel
- Text input → `GET /api/search?q=...` (task history)
- Secondary input for document ingestion → `POST /api/documents`
- Results rendered as monochrome list rows

### HealthBar (sidebar footer)
- `useHealth` polls `GET /api/health` every 30s
- Shows: `SYS_OK` / `SYS_ERR`, agent count, swarm count, uptime formatted `HH:MM:SS`

---

## Data Fetching

- Initial data: plain `fetch` calls in `useEffect` on component mount
- Mutations: plain `fetch` POST/DELETE, then re-fetch or apply optimistic update
- Live updates: WebSocket (no polling except health)
- No TanStack Query, no axios — keep deps minimal

---

## Vite Config

Add a dev proxy so UI at `:4200` can reach server at `:3000` without CORS issues in dev:

```ts
server: {
  proxy: {
    '/api': 'http://localhost:3000',
    '/ws': { target: 'ws://localhost:3000', ws: true }
  }
}
```

---

## Dependencies to Add

- `zustand` — UI state management (`apps/ui`)
- `ws` — WebSocket server (`apps/server`)
- `@types/ws` — types for `ws` (`apps/server`, dev)

---

## Out of Scope

- Authentication / auth gates
- Dark/light theme toggle
- Mobile responsiveness
- Persistent storage (localStorage, etc.)
- WebSocket server implementation changes
