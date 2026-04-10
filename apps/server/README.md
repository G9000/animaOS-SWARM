# @animaOS-SWARM/server

Local HTTP app surface for animaOS-SWARM.

This app exposes the lightweight REST and WebSocket endpoints used during local development for health, agents, swarms, search, and live runtime events. It is a thin Node boundary for the workspace, not the canonical Rust daemon.

Current server coverage includes:

- the `createServer()` boundary that assembles routes and applies CORS headers
- `/api/health` smoke coverage for the default app state
- JSON 404 behavior for unknown routes
- focused `/ws` broadcast coverage for agent and swarm lifecycle events
- a deterministic `ANIMA_MODEL_ADAPTER=mock` path for local task-run validation without provider calls

## Quick Example

```bash
bun x nx serve @animaOS-SWARM/server
curl http://127.0.0.1:3000/api/health
node -e "const socket = new WebSocket('ws://127.0.0.1:3000/ws'); socket.onmessage = (event) => console.log(event.data)"
ANIMA_MODEL_ADAPTER=mock curl -X POST http://127.0.0.1:3000/api/agents -H "content-type: application/json" -d '{"name":"observer","model":"gpt-5.4"}'
```

## Build

Run `bun x nx build @animaOS-SWARM/server`.

## Serve

Run `bun x nx serve @animaOS-SWARM/server`.

## Test

Run `bun x nx test @animaOS-SWARM/server`.
