# AnimaOS Web UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a real-time cyberpunk gold-on-black dashboard for the AnimaOS server with WebSocket-driven live agent/swarm monitoring.

**Architecture:** Add a `ws`-backed WebSocket server to `apps/server` that bridges `EventBus` events to connected clients. The React UI in `apps/ui` uses a Zustand store and a `useWebSocket` hook to receive live updates, rendered in a sidebar + split-pane layout.

**Tech Stack:** React 19, Zustand, Vite (dev proxy), `ws` (WS server), Vitest + jsdom + @testing-library/react, CSS custom properties (no CSS-in-JS, no Tailwind).

**Worktree:** `.worktrees/feat/web-ui` — all commits go on branch `feat/web-ui`.

---

## File Map

### New — `apps/server`
| File | Purpose |
|---|---|
| `apps/server/src/ws.ts` | Attach `WebSocketServer` to HTTP server, bridge `EventBus` |
| `apps/server/src/ws.test.ts` | Integration test for WS broadcasting |

### Modified — `apps/server`
| File | Change |
|---|---|
| `apps/server/src/server.ts` | Call `attachWebSocketServer` after creating HTTP server |

### New — `apps/ui`
| File | Purpose |
|---|---|
| `apps/ui/src/test-setup.ts` | Import `@testing-library/jest-dom` matchers |
| `apps/ui/src/styles/tokens.css` | CSS custom properties (colors, font) |
| `apps/ui/src/styles/global.css` | Reset, scanlines, base element styles |
| `apps/ui/src/App.module.css` | Shell layout grid |
| `apps/ui/src/store/index.ts` | Zustand store (agents + swarms + ws slices) |
| `apps/ui/src/store/index.test.ts` | Unit tests for store actions |
| `apps/ui/src/hooks/useHealth.ts` | Poll `/api/health` every 30 s |
| `apps/ui/src/hooks/useHealth.test.ts` | Unit tests with fetch mock |
| `apps/ui/src/hooks/useWebSocket.ts` | Connect to WS, dispatch into store |
| `apps/ui/src/hooks/useWebSocket.test.ts` | Unit tests with WebSocket mock |
| `apps/ui/src/components/Sidebar/Sidebar.tsx` | Nav + status footer |
| `apps/ui/src/components/Sidebar/Sidebar.css` | Sidebar styles |
| `apps/ui/src/components/Sidebar/Sidebar.test.tsx` | Render + nav tests |
| `apps/ui/src/components/AgentList/AgentList.tsx` | List agents, create, delete |
| `apps/ui/src/components/AgentList/AgentList.css` | AgentList styles |
| `apps/ui/src/components/AgentList/AgentList.test.tsx` | Render + interaction tests |
| `apps/ui/src/components/OutputPanel/OutputPanel.tsx` | Task input + WS event stream |
| `apps/ui/src/components/OutputPanel/OutputPanel.css` | OutputPanel styles |
| `apps/ui/src/components/OutputPanel/OutputPanel.test.tsx` | Render + event filtering tests |
| `apps/ui/src/components/SwarmList/SwarmList.tsx` | List swarms, create |
| `apps/ui/src/components/SwarmList/SwarmList.test.tsx` | Render tests |
| `apps/ui/src/components/SearchPanel/SearchPanel.tsx` | Search task history + docs |
| `apps/ui/src/components/SearchPanel/SearchPanel.test.tsx` | Render + search tests |

### Modified — `apps/ui`
| File | Change |
|---|---|
| `apps/ui/vite.config.mts` | Add `test` (vitest) + `server.proxy` |
| `apps/ui/src/App.tsx` | Replace placeholder with layout shell |
| `apps/ui/src/main.tsx` | Import CSS tokens + global |
| `apps/ui/src/app/app.tsx` | Remove; `main.tsx` imports `App.tsx` directly |

---

## Task 1: Install dependencies

**Files:** root `package.json` (pnpm workspace auto-updates)

- [ ] **Install server WS packages**

```bash
cd /path/to/animaos-kit
pnpm add ws --filter @animaOS-SWARM/server
pnpm add -D @types/ws --filter @animaOS-SWARM/server
```

- [ ] **Install UI packages**

```bash
pnpm add zustand --filter @animaOS-SWARM/ui
pnpm add -D @testing-library/react @testing-library/jest-dom --filter @animaOS-SWARM/ui
```

- [ ] **Verify installs**

```bash
pnpm list ws --filter @animaOS-SWARM/server
pnpm list zustand --filter @animaOS-SWARM/ui
```

Expected: both packages listed with versions.

- [ ] **Commit**

```bash
git add pnpm-lock.yaml
git commit -m "chore: add ws, zustand, and testing-library deps"
```

---

## Task 2: Vitest config for UI

**Files:**
- Modify: `apps/ui/vite.config.mts`
- Create: `apps/ui/src/test-setup.ts`

- [ ] **Create test setup file**

```typescript
// apps/ui/src/test-setup.ts
import '@testing-library/jest-dom'
```

- [ ] **Add vitest + proxy config to vite.config.mts**

Replace the full file content:

```typescript
/// <reference types='vitest' />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(() => ({
  root: import.meta.dirname,
  cacheDir: '../../node_modules/.vite/apps/ui',
  server: {
    port: 4200,
    host: 'localhost',
    proxy: {
      '/api': 'http://localhost:3000',
      '/ws': { target: 'ws://localhost:3000', ws: true },
    },
  },
  preview: {
    port: 4200,
    host: 'localhost',
  },
  plugins: [react()],
  build: {
    outDir: './dist',
    emptyOutDir: true,
    reportCompressedSize: true,
    commonjsOptions: {
      transformMixedEsModules: true,
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
  },
}));
```

- [ ] **Run tests to verify config works** (no tests yet, just check it doesn't crash)

```bash
pnpm nx test @animaOS-SWARM/ui --passWithNoTests
```

Expected: `No test files found` or exit 0.

- [ ] **Commit**

```bash
git add apps/ui/vite.config.mts apps/ui/src/test-setup.ts
git commit -m "chore: configure vitest + dev proxy for ui"
```

---

## Task 3: CSS design tokens + global styles

**Files:**
- Create: `apps/ui/src/styles/tokens.css`
- Create: `apps/ui/src/styles/global.css`

- [ ] **Create tokens.css**

```css
/* apps/ui/src/styles/tokens.css */
:root {
  --bg:           #000;
  --surface:      #020202;
  --surface-2:    #030303;
  --surface-3:    #050505;
  --border:       #111;
  --border-dim:   #1a1a1a;
  --accent:       #c9a227;
  --accent-glow:  rgba(201, 162, 39, 0.08);
  --accent-rim:   rgba(201, 162, 39, 0.3);
  --text-gold:    #c9a227;
  --text-bright:  #888;
  --text-mid:     #555;
  --text-dim:     #333;
  --text-ghost:   #2a2a2a;
  --status-idle:  #c9a227;
  --status-busy:  #888;
  --status-ok:    #c9a227;
  --status-err:   #cc3333;
  --font:         'Courier New', Courier, monospace;
  --font-size:    11px;
}
```

- [ ] **Create global.css**

```css
/* apps/ui/src/styles/global.css */
*, *::before, *::after {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
}

html, body, #root {
  height: 100%;
  background: var(--bg);
  color: var(--text-gold);
  font-family: var(--font);
  font-size: var(--font-size);
  overflow: hidden;
}

/* scanline overlay */
body::after {
  content: '';
  position: fixed;
  inset: 0;
  background: repeating-linear-gradient(
    0deg,
    transparent,
    transparent 2px,
    rgba(0, 0, 0, 0.12) 2px,
    rgba(0, 0, 0, 0.12) 4px
  );
  pointer-events: none;
  z-index: 9999;
}

input, button {
  font-family: var(--font);
  font-size: var(--font-size);
  border-radius: 0;
  outline: none;
}

input {
  background: var(--bg);
  border: 1px solid var(--border-dim);
  color: var(--text-gold);
  padding: 3px 8px;
  letter-spacing: 0.04em;
}

input::placeholder { color: var(--text-ghost); }

button {
  cursor: pointer;
  border: 1px solid var(--accent);
  background: var(--accent-glow);
  color: var(--text-gold);
  padding: 3px 10px;
  letter-spacing: 0.08em;
}

button:hover { background: rgba(201, 162, 39, 0.15); }

button:disabled {
  border-color: var(--border-dim);
  color: var(--text-ghost);
  background: transparent;
  cursor: default;
}

scrollbar-width: thin;
scrollbar-color: var(--border) var(--bg);
```

- [ ] **Commit**

```bash
git add apps/ui/src/styles/
git commit -m "feat(ui): add cyberpunk gold design tokens and global styles"
```

---

## Task 4: Zustand store

**Files:**
- Create: `apps/ui/src/store/index.ts`
- Create: `apps/ui/src/store/index.test.ts`

- [ ] **Write failing tests**

```typescript
// apps/ui/src/store/index.test.ts
import { describe, it, expect, beforeEach } from 'vitest'
import { useStore } from './index'

beforeEach(() => {
  useStore.setState({
    agents: {},
    selectedId: null,
    swarms: {},
    wsStatus: 'connecting',
    events: [],
  })
})

describe('agents slice', () => {
  it('upserts an agent', () => {
    useStore.getState().upsertAgent({ id: 'a1', name: 'alpha', status: 'idle' })
    expect(useStore.getState().agents['a1']).toMatchObject({ id: 'a1', name: 'alpha' })
  })

  it('removes an agent and clears selectedId if it was selected', () => {
    useStore.getState().upsertAgent({ id: 'a1', name: 'alpha', status: 'idle' })
    useStore.getState().setSelected('a1')
    useStore.getState().removeAgent('a1')
    expect(useStore.getState().agents['a1']).toBeUndefined()
    expect(useStore.getState().selectedId).toBeNull()
  })

  it('preserves selectedId when removing a different agent', () => {
    useStore.getState().upsertAgent({ id: 'a1', name: 'alpha', status: 'idle' })
    useStore.getState().upsertAgent({ id: 'a2', name: 'beta', status: 'idle' })
    useStore.getState().setSelected('a1')
    useStore.getState().removeAgent('a2')
    expect(useStore.getState().selectedId).toBe('a1')
  })
})

describe('swarms slice', () => {
  it('upserts a swarm', () => {
    useStore.getState().upsertSwarm({ id: 's1', strategy: 'parallel' })
    expect(useStore.getState().swarms['s1']).toMatchObject({ id: 's1' })
  })

  it('removes a swarm', () => {
    useStore.getState().upsertSwarm({ id: 's1', strategy: 'parallel' })
    useStore.getState().removeSwarm('s1')
    expect(useStore.getState().swarms['s1']).toBeUndefined()
  })
})

describe('ws slice', () => {
  it('sets ws status', () => {
    useStore.getState().setWsStatus('open')
    expect(useStore.getState().wsStatus).toBe('open')
  })

  it('keeps only last 200 events', () => {
    for (let i = 0; i < 205; i++) {
      useStore.getState().pushEvent({ type: 'agent:message', timestamp: i, data: i })
    }
    expect(useStore.getState().events).toHaveLength(200)
    expect(useStore.getState().events[199].data).toBe(204)
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/store/index.test.ts
```

Expected: FAIL — `Cannot find module './index'`

- [ ] **Create the store**

```typescript
// apps/ui/src/store/index.ts
import { create } from 'zustand'

export type AgentStatus = 'idle' | 'running' | 'error' | 'terminated'

export interface Agent {
  id: string
  name: string
  status: AgentStatus
  model?: string
  tokenUsage?: { totalTokens: number }
}

export interface Swarm {
  id: string
  strategy: string
  status?: string
}

export type WsStatus = 'connecting' | 'open' | 'closed' | 'error'

export interface WsEvent {
  type: string
  agentId?: string
  timestamp: number
  data: unknown
}

interface Store {
  // agents
  agents: Record<string, Agent>
  selectedId: string | null
  setSelected: (id: string | null) => void
  upsertAgent: (agent: Agent) => void
  removeAgent: (id: string) => void
  // swarms
  swarms: Record<string, Swarm>
  upsertSwarm: (swarm: Swarm) => void
  removeSwarm: (id: string) => void
  // ws
  wsStatus: WsStatus
  events: WsEvent[]
  setWsStatus: (status: WsStatus) => void
  pushEvent: (event: WsEvent) => void
}

export const useStore = create<Store>((set) => ({
  agents: {},
  selectedId: null,
  setSelected: (id) => set({ selectedId: id }),
  upsertAgent: (agent) =>
    set((s) => ({ agents: { ...s.agents, [agent.id]: agent } })),
  removeAgent: (id) =>
    set((s) => {
      const agents = { ...s.agents }
      delete agents[id]
      return { agents, selectedId: s.selectedId === id ? null : s.selectedId }
    }),

  swarms: {},
  upsertSwarm: (swarm) =>
    set((s) => ({ swarms: { ...s.swarms, [swarm.id]: swarm } })),
  removeSwarm: (id) =>
    set((s) => {
      const swarms = { ...s.swarms }
      delete swarms[id]
      return { swarms }
    }),

  wsStatus: 'connecting',
  events: [],
  setWsStatus: (wsStatus) => set({ wsStatus }),
  pushEvent: (event) =>
    set((s) => ({ events: [...s.events.slice(-199), event] })),
}))
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/store/index.test.ts
```

Expected: 7 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/store/
git commit -m "feat(ui): add Zustand store with agents/swarms/ws slices"
```

---

## Task 5: WebSocket server

**Files:**
- Create: `apps/server/vitest.config.ts`
- Modify: `apps/server/package.json` (add `test` nx target)
- Create: `apps/server/src/ws.ts`
- Create: `apps/server/src/ws.test.ts`

- [ ] **Create vitest config for server**

```typescript
// apps/server/vitest.config.ts
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
})
```

- [ ] **Add test target to apps/server/package.json nx.targets**

In `apps/server/package.json`, inside the `"nx"` → `"targets"` object, add:

```json
"test": {
  "executor": "@nx/vitest:vitest",
  "outputs": ["{workspaceRoot}/coverage/apps/server"],
  "options": {
    "configFile": "apps/server/vitest.config.ts"
  }
}
```

- [ ] **Write failing test**

```typescript
// apps/server/src/ws.test.ts
import { describe, it, expect, afterEach } from 'vitest'
import { createServer as createHttpServer } from 'node:http'
import { WebSocket } from 'ws'
import { EventBus } from '@animaOS-SWARM/core'
import { attachWebSocketServer } from './ws.js'

let server: ReturnType<typeof createHttpServer>
let port: number

afterEach(() => new Promise<void>((res) => server?.close(() => res())))

function startServer(bus: EventBus): Promise<number> {
  return new Promise((resolve) => {
    server = createHttpServer()
    attachWebSocketServer(server, bus)
    server.listen(0, '127.0.0.1', () => {
      port = (server.address() as { port: number }).port
      resolve(port)
    })
  })
}

function wsConnect(p: number): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${p}/ws`)
    ws.once('open', () => resolve(ws))
    ws.once('error', reject)
  })
}

describe('attachWebSocketServer', () => {
  it('broadcasts EventBus events to connected clients', async () => {
    const bus = new EventBus()
    await startServer(bus)
    const ws = await wsConnect(port)

    const received = await new Promise<object>((resolve) => {
      ws.once('message', (raw) => resolve(JSON.parse(raw.toString())))
      bus.emit('agent:started', { foo: 'bar' }, 'agent-1')
    })

    expect(received).toMatchObject({ type: 'agent:started', agentId: 'agent-1' })
    ws.close()
  })

  it('stops broadcasting after client disconnects', async () => {
    const bus = new EventBus()
    await startServer(bus)
    const ws = await wsConnect(port)

    await new Promise<void>((res) => { ws.close(); ws.once('close', res) })

    // emitting after close should not throw
    await expect(bus.emit('agent:started', {}, 'a')).resolves.toBeUndefined()
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/server --testFile=src/ws.test.ts
```

Expected: FAIL — `Cannot find module './ws.js'`

- [ ] **Create ws.ts**

```typescript
// apps/server/src/ws.ts
import { WebSocketServer, WebSocket } from 'ws'
import type { Server } from 'node:http'
import type { EventBus, EventType } from '@animaOS-SWARM/core'

const ALL_EVENTS: EventType[] = [
  'agent:spawned', 'agent:started', 'agent:completed', 'agent:failed',
  'agent:terminated', 'agent:message', 'task:started', 'task:completed',
  'task:failed', 'tool:before', 'tool:after', 'agent:tokens',
  'swarm:created', 'swarm:completed', 'swarm:stopped',
]

export function attachWebSocketServer(server: Server, eventBus: EventBus): WebSocketServer {
  const wss = new WebSocketServer({ server, path: '/ws' })

  wss.on('connection', (socket: WebSocket) => {
    const unsubscribers = ALL_EVENTS.map((type) =>
      eventBus.on(type, (event) => {
        if (socket.readyState === WebSocket.OPEN) {
          socket.send(JSON.stringify(event))
        }
      })
    )

    socket.on('close', () => unsubscribers.forEach((fn) => fn()))
  })

  return wss
}
```

- [ ] **Check EventType export exists**

```bash
grep -r "export.*EventType" /path/to/animaos-kit/packages/core/dist/types/events.d.ts
```

Expected: `export type EventType = ...`

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/server --testFile=src/ws.test.ts
```

Expected: 2 tests passing.

- [ ] **Commit**

```bash
git add apps/server/src/ws.ts apps/server/src/ws.test.ts
git commit -m "feat(server): add WebSocket server bridging EventBus to clients"
```

---

## Task 6: Attach WebSocket server to HTTP server

**Files:**
- Modify: `apps/server/src/server.ts`

- [ ] **Add WS attachment to createServer()**

In `apps/server/src/server.ts`, add the import at the top:

```typescript
import { attachWebSocketServer } from './ws.js'
```

Then at the end of `createServer()`, before the `return` statement, add:

```typescript
export function createServer() {
  const state = new AppState()
  const routes: Route[] = [
    ...healthRoutes,
    ...agentRoutes,
    ...swarmRoutes,
    ...searchRoutes,
  ]

  const httpServer = createHttpServer(async (req, res) => {
    cors(res)

    if (req.method === 'OPTIONS') {
      res.writeHead(204)
      res.end()
      return
    }

    const url = (req.url ?? '/').split('?')[0]

    const matched = matchRoute(routes, req.method ?? 'GET', url)
    if (!matched) {
      json(res, 404, { error: 'Not found' })
      return
    }

    try {
      const body = req.method === 'POST' || req.method === 'PUT' ? await parseBody(req) : {}
      await matched.route.handler(req, res, state, body, matched.params)
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      json(res, 500, { error: message })
    }
  })

  attachWebSocketServer(httpServer, state.eventBus)

  return httpServer
}
```

- [ ] **Build server to verify no type errors**

```bash
pnpm nx build @animaOS-SWARM/server
```

Expected: build succeeds with no errors.

- [ ] **Commit**

```bash
git add apps/server/src/server.ts
git commit -m "feat(server): attach WebSocket server to HTTP server"
```

---

## Task 7: useHealth hook

**Files:**
- Create: `apps/ui/src/hooks/useHealth.ts`
- Create: `apps/ui/src/hooks/useHealth.test.ts`

- [ ] **Write failing tests**

```typescript
// apps/ui/src/hooks/useHealth.test.ts
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook, act } from '@testing-library/react'
import { useHealth } from './useHealth'

const mockFetch = vi.fn()
vi.stubGlobal('fetch', mockFetch)

beforeEach(() => {
  vi.useFakeTimers()
  mockFetch.mockResolvedValue({
    json: async () => ({ status: 'ok', agents: 2, swarms: 1, uptime: 42 }),
  })
})

afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('useHealth', () => {
  it('fetches health on mount', async () => {
    const { result } = renderHook(() => useHealth(5000))
    await act(async () => { await Promise.resolve() })
    expect(result.current).toMatchObject({ status: 'ok', agents: 2 })
  })

  it('re-fetches after interval', async () => {
    const { result } = renderHook(() => useHealth(5000))
    await act(async () => { await Promise.resolve() })
    mockFetch.mockResolvedValue({
      json: async () => ({ status: 'ok', agents: 5, swarms: 2, uptime: 100 }),
    })
    await act(async () => { vi.advanceTimersByTime(5000); await Promise.resolve() })
    expect(result.current?.agents).toBe(5)
  })

  it('returns null on fetch error', async () => {
    mockFetch.mockRejectedValue(new Error('network'))
    const { result } = renderHook(() => useHealth(5000))
    await act(async () => { await Promise.resolve() })
    expect(result.current).toBeNull()
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/hooks/useHealth.test.ts
```

Expected: FAIL — `Cannot find module './useHealth'`

- [ ] **Create useHealth.ts**

```typescript
// apps/ui/src/hooks/useHealth.ts
import { useState, useEffect } from 'react'

export interface HealthData {
  status: 'ok' | 'error'
  agents: number
  swarms: number
  uptime: number
}

export function useHealth(intervalMs = 30_000): HealthData | null {
  const [health, setHealth] = useState<HealthData | null>(null)

  useEffect(() => {
    let cancelled = false

    async function poll() {
      try {
        const res = await fetch('/api/health')
        const data = await res.json()
        if (!cancelled) setHealth(data as HealthData)
      } catch {
        if (!cancelled) setHealth(null)
      }
    }

    poll()
    const id = setInterval(poll, intervalMs)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [intervalMs])

  return health
}
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/hooks/useHealth.test.ts
```

Expected: 3 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/hooks/useHealth.ts apps/ui/src/hooks/useHealth.test.ts
git commit -m "feat(ui): add useHealth hook with 30s polling"
```

---

## Task 8: useWebSocket hook

**Files:**
- Create: `apps/ui/src/hooks/useWebSocket.ts`
- Create: `apps/ui/src/hooks/useWebSocket.test.ts`

- [ ] **Write failing tests**

```typescript
// apps/ui/src/hooks/useWebSocket.test.ts
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook, act } from '@testing-library/react'
import { useStore } from '../store/index'
import { useWebSocket } from './useWebSocket'

// Minimal WebSocket mock
class MockWS {
  static OPEN = 1
  readyState = MockWS.OPEN
  onopen: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  close = vi.fn(() => { this.onclose?.() })
  send = vi.fn()
}

let mockWs: MockWS
vi.stubGlobal('WebSocket', vi.fn(() => { mockWs = new MockWS(); return mockWs }))

beforeEach(() => {
  useStore.setState({ wsStatus: 'connecting', events: [], agents: {}, swarms: {}, selectedId: null })
})

afterEach(() => vi.clearAllMocks())

describe('useWebSocket', () => {
  it('sets status to open on connect', () => {
    renderHook(() => useWebSocket())
    act(() => { mockWs.onopen?.() })
    expect(useStore.getState().wsStatus).toBe('open')
  })

  it('pushes parsed events into the store', () => {
    renderHook(() => useWebSocket())
    act(() => { mockWs.onopen?.() })
    act(() => {
      mockWs.onmessage?.({
        data: JSON.stringify({ type: 'agent:message', agentId: 'a1', timestamp: 1, data: 'hello' }),
      })
    })
    expect(useStore.getState().events).toHaveLength(1)
    expect(useStore.getState().events[0].type).toBe('agent:message')
  })

  it('sets status to closed on disconnect', () => {
    renderHook(() => useWebSocket())
    act(() => { mockWs.onopen?.() })
    act(() => { mockWs.onclose?.() })
    expect(useStore.getState().wsStatus).toBe('closed')
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/hooks/useWebSocket.test.ts
```

Expected: FAIL — `Cannot find module './useWebSocket'`

- [ ] **Create useWebSocket.ts**

```typescript
// apps/ui/src/hooks/useWebSocket.ts
import { useEffect, useRef } from 'react'
import { useStore, type Agent, type Swarm } from '../store/index'

const WS_URL = typeof window !== 'undefined'
  ? `ws://${window.location.host}/ws`
  : 'ws://localhost:3000/ws'

const MAX_BACKOFF_MS = 30_000
const AGENT_EVENTS = new Set(['agent:spawned', 'agent:started', 'agent:completed', 'agent:failed', 'agent:terminated'])
const SWARM_EVENTS = new Set(['swarm:created', 'swarm:completed', 'swarm:stopped'])

export function useWebSocket(): void {
  const setWsStatus = useStore((s) => s.setWsStatus)
  const pushEvent = useStore((s) => s.pushEvent)
  const upsertAgent = useStore((s) => s.upsertAgent)
  const upsertSwarm = useStore((s) => s.upsertSwarm)
  const backoff = useRef(1_000)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const ws = useRef<WebSocket | null>(null)

  useEffect(() => {
    function connect() {
      setWsStatus('connecting')
      const socket = new WebSocket(WS_URL)
      ws.current = socket

      socket.onopen = () => {
        setWsStatus('open')
        backoff.current = 1_000
      }

      socket.onmessage = (e) => {
        try {
          const event = JSON.parse(e.data as string) as {
            type: string; agentId?: string; timestamp: number; data: unknown
          }
          pushEvent(event)
          if (AGENT_EVENTS.has(event.type) && event.agentId && event.data) {
            upsertAgent({ id: event.agentId, ...(event.data as object) } as Agent)
          }
          if (SWARM_EVENTS.has(event.type) && event.data) {
            const swarm = event.data as Swarm
            if (swarm.id) upsertSwarm(swarm)
          }
        } catch { /* ignore malformed */ }
      }

      socket.onclose = () => {
        setWsStatus('closed')
        timer.current = setTimeout(() => {
          backoff.current = Math.min(backoff.current * 2, MAX_BACKOFF_MS)
          connect()
        }, backoff.current)
      }

      socket.onerror = () => {
        setWsStatus('error')
        socket.close()
      }
    }

    connect()
    return () => {
      if (timer.current) clearTimeout(timer.current)
      ws.current?.close()
    }
  }, []) // eslint-disable-line react-hooks/exhaustive-deps
}
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/hooks/useWebSocket.test.ts
```

Expected: 3 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/hooks/useWebSocket.ts apps/ui/src/hooks/useWebSocket.test.ts
git commit -m "feat(ui): add useWebSocket hook with exponential backoff reconnect"
```

---

## Task 9: App layout shell

**Files:**
- Modify: `apps/ui/src/App.tsx`
- Create: `apps/ui/src/App.module.css`
- Modify: `apps/ui/src/main.tsx`
- Modify: `apps/ui/src/app/app.tsx` (gutted — not deleted, Nx may reference it)

- [ ] **Update main.tsx to import CSS and use App directly**

```tsx
// apps/ui/src/main.tsx
import { StrictMode } from 'react';
import * as ReactDOM from 'react-dom/client';
import './styles/tokens.css';
import './styles/global.css';
import App from './App';

const root = ReactDOM.createRoot(
  document.getElementById('root') as HTMLElement
);

root.render(
  <StrictMode>
    <App />
  </StrictMode>
);
```

- [ ] **Create App.module.css**

```css
/* apps/ui/src/App.module.css */
.shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

.main {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--surface);
}

.top {
  flex: 0 0 44%;
  overflow: hidden;
  border-bottom: 1px solid var(--border);
}

.bottom {
  flex: 1;
  overflow: hidden;
}

/* full-height panel used when split pane is not shown (search, health) */
.full {
  flex: 1;
  overflow: hidden;
}
```

- [ ] **Update App.tsx**

```tsx
// apps/ui/src/App.tsx
import { useState } from 'react'
import { Sidebar } from './components/Sidebar/Sidebar'
import { AgentList } from './components/AgentList/AgentList'
import { OutputPanel } from './components/OutputPanel/OutputPanel'
import { SwarmList } from './components/SwarmList/SwarmList'
import { SearchPanel } from './components/SearchPanel/SearchPanel'
import { useWebSocket } from './hooks/useWebSocket'
import { useHealth, type HealthData } from './hooks/useHealth'
import styles from './App.module.css'

export type NavSection = 'agents' | 'swarms' | 'search' | 'health'

export default function App() {
  const [section, setSection] = useState<NavSection>('agents')
  const health = useHealth()
  useWebSocket()

  const showSplit = section === 'agents' || section === 'swarms'

  return (
    <div className={styles.shell}>
      <Sidebar section={section} onNav={setSection} health={health} />
      <div className={styles.main}>
        <div className={showSplit ? styles.top : styles.full}>
          {section === 'agents' && <AgentList />}
          {section === 'swarms' && <SwarmList />}
          {section === 'search' && <SearchPanel />}
          {section === 'health' && <HealthPanel health={health} />}
        </div>
        {showSplit && (
          <div className={styles.bottom}>
            <OutputPanel />
          </div>
        )}
      </div>
    </div>
  )
}

function HealthPanel({ health }: { health: HealthData | null }) {
  if (!health) return <div style={{ padding: '12px 14px', color: 'var(--text-dim)' }}>connecting...</div>
  const hh = String(Math.floor(health.uptime / 3600)).padStart(2, '0')
  const mm = String(Math.floor((health.uptime % 3600) / 60)).padStart(2, '0')
  const ss = String(Math.floor(health.uptime % 60)).padStart(2, '0')
  return (
    <div style={{ padding: '12px 14px', display: 'flex', flexDirection: 'column', gap: '6px' }}>
      <div>SYS_STATUS: <span style={{ color: health.status === 'ok' ? 'var(--status-ok)' : 'var(--status-err)' }}>{health.status.toUpperCase()}</span></div>
      <div style={{ color: 'var(--text-mid)' }}>AGENTS: {health.agents}</div>
      <div style={{ color: 'var(--text-mid)' }}>SWARMS: {health.swarms}</div>
      <div style={{ color: 'var(--text-dim)' }}>UPTIME: {hh}:{mm}:{ss}</div>
    </div>
  )
}
```

- [ ] **Gut app.tsx so Nx doesn't break** (do not delete the file)

```tsx
// apps/ui/src/app/app.tsx
export { default } from '../App'
export function App() { return null }
```

- [ ] **Build UI to verify no type errors**

```bash
pnpm nx build @animaOS-SWARM/ui
```

Expected: build completes (Sidebar etc. will be missing — expected at this stage; use `// @ts-nocheck` temporarily at top of App.tsx if needed until components exist)

- [ ] **Commit**

```bash
git add apps/ui/src/App.tsx apps/ui/src/App.module.css apps/ui/src/main.tsx apps/ui/src/app/app.tsx
git commit -m "feat(ui): add app layout shell (sidebar + split pane)"
```

---

## Task 10: Sidebar component

**Files:**
- Create: `apps/ui/src/components/Sidebar/Sidebar.tsx`
- Create: `apps/ui/src/components/Sidebar/Sidebar.css`
- Create: `apps/ui/src/components/Sidebar/Sidebar.test.tsx`

- [ ] **Write failing tests**

```tsx
// apps/ui/src/components/Sidebar/Sidebar.test.tsx
import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Sidebar } from './Sidebar'

const health = { status: 'ok' as const, agents: 3, swarms: 1, uptime: 2537 }

describe('Sidebar', () => {
  it('renders all nav items', () => {
    render(<Sidebar section="agents" onNav={vi.fn()} health={health} />)
    expect(screen.getByText('AGENTS')).toBeTruthy()
    expect(screen.getByText('SWARMS')).toBeTruthy()
    expect(screen.getByText('SEARCH')).toBeTruthy()
    expect(screen.getByText('HEALTH')).toBeTruthy()
  })

  it('calls onNav with correct section when clicked', () => {
    const onNav = vi.fn()
    render(<Sidebar section="agents" onNav={onNav} health={health} />)
    fireEvent.click(screen.getByText('SWARMS'))
    expect(onNav).toHaveBeenCalledWith('swarms')
  })

  it('shows SYS_OK when health status is ok', () => {
    render(<Sidebar section="agents" onNav={vi.fn()} health={health} />)
    expect(screen.getByText('SYS_OK')).toBeTruthy()
  })

  it('shows SYS_ERR when health is null', () => {
    render(<Sidebar section="agents" onNav={vi.fn()} health={null} />)
    expect(screen.getByText('SYS_ERR')).toBeTruthy()
  })

  it('formats uptime as HH:MM:SS', () => {
    render(<Sidebar section="agents" onNav={vi.fn()} health={health} />)
    expect(screen.getByText('00:42:17')).toBeTruthy()
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/Sidebar/Sidebar.test.tsx
```

Expected: FAIL

- [ ] **Create Sidebar.css**

```css
/* apps/ui/src/components/Sidebar/Sidebar.css */
.sidebar {
  width: 150px;
  flex-shrink: 0;
  background: var(--surface-3);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  position: relative;
}

.logo {
  padding: 12px 14px 14px;
  border-bottom: 1px solid var(--border);
  position: relative;
}

.logo::after {
  content: '';
  position: absolute;
  bottom: 0;
  left: 14px;
  right: 0;
  height: 1px;
  background: linear-gradient(90deg, var(--accent) 0%, rgba(201,162,39,0.15) 70%, transparent 100%);
}

.logoText {
  color: var(--accent);
  font-size: 13px;
  font-weight: bold;
  letter-spacing: 0.18em;
}

.logoSub {
  color: var(--text-ghost);
  font-size: 9px;
  letter-spacing: 0.12em;
  margin-top: 3px;
}

.nav {
  flex: 1;
  padding: 6px 0;
}

.navItem {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 14px;
  border-left: 2px solid transparent;
  cursor: pointer;
  color: var(--text-ghost);
  letter-spacing: 0.1em;
  font-size: 10px;
  user-select: none;
}

.navItem:hover { color: var(--text-mid); }

.navItem.active {
  border-left-color: var(--accent);
  background: var(--accent-glow);
  color: var(--text-gold);
}

.navArrow { font-size: 10px; }

.footer {
  padding: 10px 14px;
  border-top: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.sysStatus { font-size: 9px; letter-spacing: 0.08em; }
.sysOk { color: var(--status-ok); }
.sysErr { color: var(--status-err); }
.footerStat { color: var(--text-ghost); font-size: 9px; }
```

- [ ] **Create Sidebar.tsx**

```tsx
// apps/ui/src/components/Sidebar/Sidebar.tsx
import './Sidebar.css'
import type { NavSection } from '../../App'
import type { HealthData } from '../../hooks/useHealth'

const NAV_ITEMS: { key: NavSection; label: string }[] = [
  { key: 'agents', label: 'AGENTS' },
  { key: 'swarms', label: 'SWARMS' },
  { key: 'search', label: 'SEARCH' },
  { key: 'health', label: 'HEALTH' },
]

interface SidebarProps {
  section: NavSection
  onNav: (s: NavSection) => void
  health: HealthData | null
}

function formatUptime(s: number): string {
  const hh = String(Math.floor(s / 3600)).padStart(2, '0')
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0')
  const ss = String(Math.floor(s % 60)).padStart(2, '0')
  return `${hh}:${mm}:${ss}`
}

export function Sidebar({ section, onNav, health }: SidebarProps) {
  const sysOk = health?.status === 'ok'

  return (
    <aside className="sidebar">
      <div className="logo">
        <div className="logoText">ANIMA<span style={{ color: '#888' }}>OS</span></div>
        <div className="logoSub">KIT_v0.0.1</div>
      </div>

      <nav className="nav">
        {NAV_ITEMS.map(({ key, label }) => (
          <div
            key={key}
            className={`navItem${section === key ? ' active' : ''}`}
            onClick={() => onNav(key)}
          >
            <span className="navArrow">{section === key ? '▶' : '▷'}</span>
            <span>{label}</span>
          </div>
        ))}
      </nav>

      <div className="footer">
        <div className={`sysStatus ${sysOk ? 'sysOk' : 'sysErr'}`}>
          {sysOk ? 'SYS_OK' : 'SYS_ERR'}
        </div>
        {health && (
          <>
            <div className="footerStat">{health.agents} AGT · {health.swarms} SWM</div>
            <div className="footerStat">UP: {formatUptime(health.uptime)}</div>
          </>
        )}
      </div>
    </aside>
  )
}
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/Sidebar/Sidebar.test.tsx
```

Expected: 5 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/components/Sidebar/
git commit -m "feat(ui): add Sidebar component with nav and health footer"
```

---

## Task 11: AgentList component

**Files:**
- Create: `apps/ui/src/components/AgentList/AgentList.tsx`
- Create: `apps/ui/src/components/AgentList/AgentList.css`
- Create: `apps/ui/src/components/AgentList/AgentList.test.tsx`

- [ ] **Write failing tests**

```tsx
// apps/ui/src/components/AgentList/AgentList.test.tsx
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { useStore } from '../../store/index'
import { AgentList } from './AgentList'

const mockFetch = vi.fn()
vi.stubGlobal('fetch', mockFetch)

beforeEach(() => {
  useStore.setState({ agents: {}, selectedId: null, swarms: {}, wsStatus: 'open', events: [] })
  mockFetch.mockResolvedValue({ ok: true, json: async () => ({ agents: [] }) })
})

describe('AgentList', () => {
  it('renders agents from store', async () => {
    useStore.setState({
      agents: {
        a1: { id: 'a1', name: 'alpha', status: 'idle' },
        a2: { id: 'a2', name: 'beta', status: 'running' },
      },
    })
    render(<AgentList />)
    expect(screen.getByText('alpha')).toBeTruthy()
    expect(screen.getByText('beta')).toBeTruthy()
  })

  it('selects an agent on click', async () => {
    useStore.setState({ agents: { a1: { id: 'a1', name: 'alpha', status: 'idle' } } })
    render(<AgentList />)
    fireEvent.click(screen.getByText('alpha'))
    expect(useStore.getState().selectedId).toBe('a1')
  })

  it('shows IDLE/BUSY status labels', () => {
    useStore.setState({
      agents: {
        a1: { id: 'a1', name: 'alpha', status: 'idle' },
        a2: { id: 'a2', name: 'beta', status: 'running' },
      },
    })
    render(<AgentList />)
    expect(screen.getByText('IDLE')).toBeTruthy()
    expect(screen.getByText('BUSY')).toBeTruthy()
  })

  it('calls DELETE and removes agent on delete button click', async () => {
    useStore.setState({ agents: { a1: { id: 'a1', name: 'alpha', status: 'idle' } }, selectedId: 'a1' })
    mockFetch.mockResolvedValue({ ok: true, json: async () => ({ deleted: true }) })
    render(<AgentList />)
    fireEvent.click(screen.getByTitle('delete agent'))
    await waitFor(() => expect(useStore.getState().agents['a1']).toBeUndefined())
    expect(mockFetch).toHaveBeenCalledWith('/api/agents/a1', expect.objectContaining({ method: 'DELETE' }))
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/AgentList/AgentList.test.tsx
```

Expected: FAIL

- [ ] **Create AgentList.css**

```css
/* apps/ui/src/components/AgentList/AgentList.css */
.panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--surface-2);
}

.header {
  padding: 8px 14px;
  border-bottom: 1px solid var(--border);
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.title {
  color: var(--accent);
  letter-spacing: 0.14em;
  font-size: 11px;
}

.list {
  flex: 1;
  overflow-y: auto;
  padding: 8px 12px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.row {
  border: 1px solid var(--border-dim);
  padding: 8px 10px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  cursor: pointer;
  position: relative;
}

.row:hover { border-color: var(--accent-rim); }

.row.selected {
  border-color: var(--accent);
  background: var(--accent-glow);
}

.row.selected::before {
  content: '';
  position: absolute;
  top: -1px; left: -1px;
  width: 7px; height: 7px;
  border-top: 1px solid var(--accent);
  border-left: 1px solid var(--accent);
}

.row.selected::after {
  content: '';
  position: absolute;
  bottom: -1px; right: -1px;
  width: 7px; height: 7px;
  border-bottom: 1px solid var(--accent);
  border-right: 1px solid var(--accent);
}

.agentName { color: var(--text-gold); font-size: 11px; letter-spacing: 0.06em; }
.agentMeta { color: var(--text-ghost); font-size: 9px; margin-top: 2px; }
.rowRight { display: flex; align-items: center; gap: 6px; }

.statusBadge {
  border: 1px solid var(--border-dim);
  padding: 1px 7px;
  font-size: 9px;
  letter-spacing: 0.06em;
  color: var(--text-mid);
}
.statusBadge.idle { border-color: var(--accent); color: var(--accent); }
.statusBadge.busy { border-color: var(--border); color: var(--text-bright); }

.deleteBtn {
  background: transparent;
  border: none;
  color: var(--text-ghost);
  padding: 0 3px;
  font-size: 12px;
  cursor: pointer;
  line-height: 1;
}
.deleteBtn:hover { color: var(--status-err); }

.createForm {
  padding: 8px 12px;
  border-top: 1px solid var(--border);
  display: flex;
  gap: 6px;
}

.createForm input { flex: 1; }
```

- [ ] **Create AgentList.tsx**

```tsx
// apps/ui/src/components/AgentList/AgentList.tsx
import { useEffect, useState } from 'react'
import { useStore, type Agent } from '../../store/index'
import './AgentList.css'

const MODELS = ['claude-opus-4-6', 'claude-sonnet-4-6', 'claude-haiku-4-5']

export function AgentList() {
  const agents = useStore((s) => Object.values(s.agents))
  const selectedId = useStore((s) => s.selectedId)
  const setSelected = useStore((s) => s.setSelected)
  const upsertAgent = useStore((s) => s.upsertAgent)
  const removeAgent = useStore((s) => s.removeAgent)

  const [newName, setNewName] = useState('')
  const [newModel, setNewModel] = useState(MODELS[0])
  const [creating, setCreating] = useState(false)
  const [showForm, setShowForm] = useState(false)

  useEffect(() => {
    fetch('/api/agents')
      .then((r) => r.json())
      .then(({ agents: list }: { agents: Agent[] }) => {
        list.forEach(upsertAgent)
      })
      .catch(() => {})
  }, [])

  async function createAgent() {
    if (!newName.trim()) return
    setCreating(true)
    try {
      const res = await fetch('/api/agents', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: newName.trim(), model: newModel }),
      })
      const data = await res.json() as { id: string; name: string; status: string }
      upsertAgent({ id: data.id, name: data.name, status: 'idle' })
      setNewName('')
      setShowForm(false)
    } finally {
      setCreating(false)
    }
  }

  async function deleteAgent(id: string, e: React.MouseEvent) {
    e.stopPropagation()
    await fetch(`/api/agents/${id}`, { method: 'DELETE' })
    removeAgent(id)
  }

  return (
    <div className="panel">
      <div className="header">
        <span className="title">// AGENTS</span>
        <button onClick={() => setShowForm((v) => !v)}>+ NEW_AGENT</button>
      </div>

      {showForm && (
        <div className="createForm">
          <input
            placeholder="name"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && createAgent()}
          />
          <select
            value={newModel}
            onChange={(e) => setNewModel(e.target.value)}
            style={{ background: 'var(--bg)', color: 'var(--text-gold)', border: '1px solid var(--border-dim)', padding: '3px 4px', fontFamily: 'var(--font)', fontSize: 'var(--font-size)' }}
          >
            {MODELS.map((m) => <option key={m} value={m}>{m}</option>)}
          </select>
          <button onClick={createAgent} disabled={creating || !newName.trim()}>
            {creating ? '...' : 'CREATE'}
          </button>
        </div>
      )}

      <div className="list">
        {agents.map((agent) => (
          <div
            key={agent.id}
            className={`row${selectedId === agent.id ? ' selected' : ''}`}
            onClick={() => setSelected(agent.id)}
          >
            <div>
              <div className="agentName">
                {selectedId === agent.id ? '▶ ' : ''}{agent.name}
              </div>
              <div className="agentMeta">
                MDL:{agent.model ?? '—'} · TOK:{agent.tokenUsage?.totalTokens ?? 0}
              </div>
            </div>
            <div className="rowRight">
              <span className={`statusBadge ${agent.status === 'idle' ? 'idle' : 'busy'}`}>
                {agent.status === 'idle' ? 'IDLE' : 'BUSY'}
              </span>
              {selectedId === agent.id && (
                <button
                  className="deleteBtn"
                  title="delete agent"
                  onClick={(e) => deleteAgent(agent.id, e)}
                >×</button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/AgentList/AgentList.test.tsx
```

Expected: 4 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/components/AgentList/
git commit -m "feat(ui): add AgentList component with create/delete and live store updates"
```

---

## Task 12: OutputPanel component

**Files:**
- Create: `apps/ui/src/components/OutputPanel/OutputPanel.tsx`
- Create: `apps/ui/src/components/OutputPanel/OutputPanel.css`
- Create: `apps/ui/src/components/OutputPanel/OutputPanel.test.tsx`

- [ ] **Write failing tests**

```tsx
// apps/ui/src/components/OutputPanel/OutputPanel.test.tsx
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { useStore } from '../../store/index'
import { OutputPanel } from './OutputPanel'

const mockFetch = vi.fn()
vi.stubGlobal('fetch', mockFetch)

beforeEach(() => {
  useStore.setState({ agents: {}, selectedId: null, swarms: {}, wsStatus: 'open', events: [] })
  mockFetch.mockResolvedValue({ ok: true, json: async () => ({}) })
})

describe('OutputPanel', () => {
  it('shows prompt when no agent selected', () => {
    render(<OutputPanel />)
    expect(screen.getByText(/select an agent/i)).toBeTruthy()
  })

  it('shows agent name in header when selected', () => {
    useStore.setState({
      agents: { a1: { id: 'a1', name: 'alpha', status: 'idle' } },
      selectedId: 'a1',
    })
    render(<OutputPanel />)
    expect(screen.getByText(/alpha/)).toBeTruthy()
  })

  it('renders only events for the selected agent', () => {
    useStore.setState({
      agents: {
        a1: { id: 'a1', name: 'alpha', status: 'idle' },
        a2: { id: 'a2', name: 'beta', status: 'idle' },
      },
      selectedId: 'a1',
      events: [
        { type: 'agent:message', agentId: 'a1', timestamp: 1, data: 'hello from alpha' },
        { type: 'agent:message', agentId: 'a2', timestamp: 2, data: 'hello from beta' },
      ],
    })
    render(<OutputPanel />)
    expect(screen.getByText('hello from alpha')).toBeTruthy()
    expect(screen.queryByText('hello from beta')).toBeNull()
  })

  it('calls POST /api/agents/:id/run when RUN is clicked', async () => {
    useStore.setState({
      agents: { a1: { id: 'a1', name: 'alpha', status: 'idle' } },
      selectedId: 'a1',
    })
    mockFetch.mockResolvedValue({ ok: true, json: async () => ({ status: 'success' }) })
    render(<OutputPanel />)
    fireEvent.change(screen.getByPlaceholderText(/_enter_task_/), { target: { value: 'do stuff' } })
    fireEvent.click(screen.getByText(/RUN/))
    await waitFor(() =>
      expect(mockFetch).toHaveBeenCalledWith('/api/agents/a1/run', expect.objectContaining({ method: 'POST' }))
    )
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/OutputPanel/OutputPanel.test.tsx
```

Expected: FAIL

- [ ] **Create OutputPanel.css**

```css
/* apps/ui/src/components/OutputPanel/OutputPanel.css */
.panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg);
}

.header {
  padding: 6px 14px;
  border-bottom: 1px solid var(--border);
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-shrink: 0;
}

.headerLabel {
  color: var(--text-ghost);
  font-size: 10px;
  letter-spacing: 0.06em;
}

.controls {
  display: flex;
  gap: 6px;
  align-items: center;
}

.taskInput { width: 200px; }

.stream {
  flex: 1;
  overflow-y: auto;
  padding: 10px 14px;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.empty {
  color: var(--text-ghost);
  padding: 12px 14px;
  letter-spacing: 0.06em;
}

.line { line-height: 1.6; }
.line.cmd { color: var(--accent); }
.line.ok { color: var(--text-mid); }
.line.info { color: var(--text-bright); }
.line.dim { color: var(--text-ghost); }

.cursor {
  color: var(--accent);
  animation: blink 1s step-end infinite;
}

@keyframes blink { 50% { opacity: 0; } }
```

- [ ] **Create OutputPanel.tsx**

```tsx
// apps/ui/src/components/OutputPanel/OutputPanel.tsx
import { useState, useRef, useEffect } from 'react'
import { useStore, type WsEvent } from '../../store/index'
import './OutputPanel.css'

function eventToLine(event: WsEvent): { text: string; cls: string } {
  const { type, data } = event
  if (type === 'agent:message') return { text: `  ${String(data)}`, cls: 'info' }
  if (type === 'task:started') return { text: `$ ${String(data)}`, cls: 'cmd' }
  if (type === 'task:completed') return { text: `  ✓ ${JSON.stringify(data)}`, cls: 'ok' }
  if (type === 'task:failed') return { text: `  ✗ ${String(data)}`, cls: 'info' }
  if (type === 'tool:before') return { text: `  → ${String(data)}`, cls: 'dim' }
  if (type === 'tool:after') return { text: `  ← ${String(data)}`, cls: 'dim' }
  if (type === 'agent:tokens') return { text: `  TOK: ${JSON.stringify(data)}`, cls: 'dim' }
  return { text: `  [${type}] ${JSON.stringify(data)}`, cls: 'dim' }
}

export function OutputPanel() {
  const selectedId = useStore((s) => s.selectedId)
  const agent = useStore((s) => (s.selectedId ? s.agents[s.selectedId] : null))
  const events = useStore((s) =>
    s.events.filter((e) => e.agentId === s.selectedId)
  )
  const upsertAgent = useStore((s) => s.upsertAgent)

  const [task, setTask] = useState('')
  const [running, setRunning] = useState(false)
  const streamRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (streamRef.current) {
      streamRef.current.scrollTop = streamRef.current.scrollHeight
    }
  }, [events.length])

  async function runTask() {
    if (!selectedId || !task.trim() || running) return
    setRunning(true)
    try {
      await fetch(`/api/agents/${selectedId}/run`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ task: task.trim() }),
      })
      if (agent) upsertAgent({ ...agent, status: 'idle' })
    } finally {
      setRunning(false)
    }
  }

  if (!selectedId || !agent) {
    return <div className="empty">select an agent to view output</div>
  }

  return (
    <div className="panel">
      <div className="header">
        <span className="headerLabel">OUTPUT // {agent.name}</span>
        <div className="controls">
          <input
            className="taskInput"
            placeholder="_enter_task_"
            value={task}
            onChange={(e) => setTask(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && runTask()}
          />
          <button onClick={runTask} disabled={running || !task.trim()}>
            {running ? '...' : 'RUN ▶'}
          </button>
        </div>
      </div>

      <div className="stream" ref={streamRef}>
        {events.length === 0 && (
          <div className="line dim">// no output yet</div>
        )}
        {events.map((event, i) => {
          const { text, cls } = eventToLine(event)
          return <div key={i} className={`line ${cls}`}>{text}</div>
        })}
        {agent.status === 'running' && <div className="cursor">▌</div>}
      </div>
    </div>
  )
}
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/OutputPanel/OutputPanel.test.tsx
```

Expected: 4 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/components/OutputPanel/
git commit -m "feat(ui): add OutputPanel component with live WS event stream"
```

---

## Task 13: SwarmList component

**Files:**
- Create: `apps/ui/src/components/SwarmList/SwarmList.tsx`
- Create: `apps/ui/src/components/SwarmList/SwarmList.test.tsx`

- [ ] **Write failing tests**

```tsx
// apps/ui/src/components/SwarmList/SwarmList.test.tsx
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { useStore } from '../../store/index'
import { SwarmList } from './SwarmList'

const mockFetch = vi.fn()
vi.stubGlobal('fetch', mockFetch)

beforeEach(() => {
  useStore.setState({ swarms: {}, selectedId: null, agents: {}, wsStatus: 'open', events: [] })
  mockFetch.mockResolvedValue({ ok: true, json: async () => ({ swarms: [] }) })
})

describe('SwarmList', () => {
  it('renders swarms from store', () => {
    useStore.setState({ swarms: { s1: { id: 's1', strategy: 'parallel' } } })
    render(<SwarmList />)
    expect(screen.getByText('s1')).toBeTruthy()
    expect(screen.getByText('parallel')).toBeTruthy()
  })

  it('shows SWARMS header', () => {
    render(<SwarmList />)
    expect(screen.getByText('// SWARMS')).toBeTruthy()
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/SwarmList/SwarmList.test.tsx
```

Expected: FAIL

- [ ] **Create SwarmList.tsx**

```tsx
// apps/ui/src/components/SwarmList/SwarmList.tsx
import { useEffect } from 'react'
import { useStore, type Swarm } from '../../store/index'

export function SwarmList() {
  const swarms = useStore((s) => Object.values(s.swarms))
  const upsertSwarm = useStore((s) => s.upsertSwarm)

  useEffect(() => {
    fetch('/api/swarms')
      .then((r) => r.json())
      .then(({ swarms: list }: { swarms: Swarm[] }) => { list.forEach(upsertSwarm) })
      .catch(() => {})
  }, [])

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', background: 'var(--surface-2)' }}>
      <div style={{ padding: '8px 14px', borderBottom: '1px solid var(--border)', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <span style={{ color: 'var(--accent)', letterSpacing: '0.14em', fontSize: '11px' }}>// SWARMS</span>
      </div>
      <div style={{ flex: 1, overflowY: 'auto', padding: '8px 12px', display: 'flex', flexDirection: 'column', gap: '4px' }}>
        {swarms.length === 0 && (
          <div style={{ color: 'var(--text-ghost)', padding: '4px 0' }}>no swarms</div>
        )}
        {swarms.map((swarm) => (
          <div key={swarm.id} style={{ border: '1px solid var(--border-dim)', padding: '8px 10px', display: 'flex', justifyContent: 'space-between' }}>
            <div>
              <div style={{ color: 'var(--text-gold)', fontSize: '11px' }}>{swarm.id}</div>
              <div style={{ color: 'var(--text-ghost)', fontSize: '9px', marginTop: '2px' }}>STRATEGY:{swarm.strategy}</div>
            </div>
            <span style={{ border: '1px solid var(--border-dim)', color: 'var(--text-mid)', padding: '1px 7px', fontSize: '9px', alignSelf: 'center' }}>
              {swarm.status?.toUpperCase() ?? 'IDLE'}
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/SwarmList/SwarmList.test.tsx
```

Expected: 2 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/components/SwarmList/
git commit -m "feat(ui): add SwarmList component"
```

---

## Task 14: SearchPanel component

**Files:**
- Create: `apps/ui/src/components/SearchPanel/SearchPanel.tsx`
- Create: `apps/ui/src/components/SearchPanel/SearchPanel.test.tsx`

- [ ] **Write failing tests**

```tsx
// apps/ui/src/components/SearchPanel/SearchPanel.test.tsx
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { SearchPanel } from './SearchPanel'

const mockFetch = vi.fn()
vi.stubGlobal('fetch', mockFetch)

beforeEach(() => {
  mockFetch.mockResolvedValue({ ok: true, json: async () => ({ results: [] }) })
})

describe('SearchPanel', () => {
  it('renders search header', () => {
    render(<SearchPanel />)
    expect(screen.getByText('// SEARCH')).toBeTruthy()
  })

  it('calls /api/search when search button clicked', async () => {
    render(<SearchPanel />)
    fireEvent.change(screen.getByPlaceholderText(/_query_/), { target: { value: 'analyze' } })
    fireEvent.click(screen.getByText('SEARCH'))
    await waitFor(() =>
      expect(mockFetch).toHaveBeenCalledWith(expect.stringContaining('/api/search?q=analyze'))
    )
  })

  it('renders results', async () => {
    mockFetch.mockResolvedValue({
      ok: true,
      json: async () => ({
        results: [{ id: 'r1', task: 'analyze logs', status: 'success', agentId: 'a1', timestamp: 0, result: '', durationMs: 100, tokensUsed: 50 }],
      }),
    })
    render(<SearchPanel />)
    fireEvent.change(screen.getByPlaceholderText(/_query_/), { target: { value: 'analyze' } })
    fireEvent.click(screen.getByText('SEARCH'))
    await waitFor(() => expect(screen.getByText('analyze logs')).toBeTruthy())
  })
})
```

- [ ] **Run tests to verify they fail**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/SearchPanel/SearchPanel.test.tsx
```

Expected: FAIL

- [ ] **Create SearchPanel.tsx**

```tsx
// apps/ui/src/components/SearchPanel/SearchPanel.tsx
import { useState } from 'react'

interface TaskRecord {
  id: string
  agentId: string
  task: string
  result: string
  status: string
  timestamp: number
  durationMs: number
  tokensUsed: number
}

export function SearchPanel() {
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<TaskRecord[]>([])
  const [searching, setSearching] = useState(false)

  async function search() {
    if (!query.trim()) return
    setSearching(true)
    try {
      const res = await fetch(`/api/search?q=${encodeURIComponent(query.trim())}`)
      const data = await res.json() as { results: TaskRecord[] }
      setResults(data.results)
    } finally {
      setSearching(false)
    }
  }

  const base: React.CSSProperties = { fontFamily: 'var(--font)', fontSize: 'var(--font-size)' }

  return (
    <div style={{ ...base, display: 'flex', flexDirection: 'column', height: '100%', background: 'var(--surface-2)' }}>
      <div style={{ padding: '8px 14px', borderBottom: '1px solid var(--border)', display: 'flex', gap: '8px', alignItems: 'center' }}>
        <span style={{ color: 'var(--accent)', letterSpacing: '0.14em', fontSize: '11px', marginRight: '4px' }}>// SEARCH</span>
        <input
          placeholder="_query_"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && search()}
          style={{ flex: 1 }}
        />
        <button onClick={search} disabled={searching || !query.trim()}>
          {searching ? '...' : 'SEARCH'}
        </button>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', padding: '8px 12px', display: 'flex', flexDirection: 'column', gap: '4px' }}>
        {results.length === 0 && (
          <div style={{ color: 'var(--text-ghost)' }}>// no results</div>
        )}
        {results.map((r) => (
          <div key={r.id} style={{ border: '1px solid var(--border-dim)', padding: '8px 10px' }}>
            <div style={{ color: 'var(--text-gold)', fontSize: '11px' }}>{r.task}</div>
            <div style={{ color: 'var(--text-ghost)', fontSize: '9px', marginTop: '2px' }}>
              AGT:{r.agentId} · {r.status.toUpperCase()} · {r.durationMs}ms · TOK:{r.tokensUsed}
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
```

- [ ] **Run tests to verify they pass**

```bash
pnpm nx test @animaOS-SWARM/ui --testFile=src/components/SearchPanel/SearchPanel.test.tsx
```

Expected: 3 tests passing.

- [ ] **Commit**

```bash
git add apps/ui/src/components/SearchPanel/
git commit -m "feat(ui): add SearchPanel component"
```

---

## Task 15: Full run + remove @ts-nocheck

**Files:**
- Modify: `apps/ui/src/App.tsx` (remove any `@ts-nocheck` added in Task 9)

- [ ] **Run all UI tests**

```bash
pnpm nx test @animaOS-SWARM/ui
```

Expected: all tests passing.

- [ ] **Run all server tests**

```bash
pnpm nx test @animaOS-SWARM/server
```

Expected: all tests passing.

- [ ] **Build both apps**

```bash
pnpm nx run-many -t build --projects=@animaOS-SWARM/server,@animaOS-SWARM/ui
```

Expected: both builds succeed.

- [ ] **Smoke test manually: start server**

```bash
pnpm nx serve @animaOS-SWARM/server
```

Expected: `AnimaOS Kit server running on http://localhost:3000`

- [ ] **Smoke test manually: start UI (separate terminal)**

```bash
pnpm nx serve @animaOS-SWARM/ui
```

Expected: dev server at `http://localhost:4200`, dashboard visible with gold-on-black cyberpunk style.

- [ ] **Final commit**

```bash
git add apps/ui/src/App.tsx
git commit -m "feat(ui): wire all components — AnimaOS dashboard complete"
```
