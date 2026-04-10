# @animaOS-SWARM/sdk

Public TypeScript client for the animaOS Rust daemon.

This package exports `createDaemonClient`, `AgentsClient`, `MemoriesClient`, `SwarmsClient`, and the `agent()`, `action()`, `plugin()`, and `swarm()` helpers. It talks to the daemon over HTTP and SSE and does not embed the execution runtime.

Current SDK coverage includes:

- daemon health checks via `client.health()`
- agent create, list, get, run, and recent-memory reads
- memory create, search, and recent-memory reads
- swarm create, get, run, and live SSE event subscriptions
- daemon-specific error surfaces for HTTP failures and connection failures

## Quick Example

```ts
import { createDaemonClient } from '@animaOS-SWARM/sdk';

const client = createDaemonClient({
  baseUrl: process.env.ANIMA_DAEMON_URL ?? 'http://127.0.0.1:8080',
});

const health = await client.health();
const agents = await client.agents.list();
const memories = await client.memories.search('launch warning', { limit: 5 });

console.log({
  daemon: health.status,
  agents: agents.length,
  matchingMemories: memories.length,
});
```

## Build

Run `bun run build:cli-sdk` to build the SDK and CLI together, or `bun x nx build @animaOS-SWARM/sdk` to build only this package.

## Test

Run `bun x nx test @animaOS-SWARM/sdk`.
