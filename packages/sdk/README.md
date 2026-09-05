# @animaOS-SWARM/sdk

Public TypeScript client for the animaOS Rust daemon.

This package exports `createDaemonClient`, `AgentsClient`, `ConnectorsClient`, `MemoriesClient`, `SwarmsClient`, and the `agent()`, `action()`, `plugin()`, and `swarm()` helpers. It talks to the daemon over HTTP and SSE and does not embed the execution runtime.

Current SDK coverage includes:

- daemon health checks via `client.health()`
- agent create, list, get, update, remove, run, peer messages, and recent-memory reads
- memory create, search, recent-memory reads, entities, evaluated writes, hybrid recall, evidence trace, retention, readiness/eval reporting, and relationships
- swarm create, get, run, and live SSE event subscriptions
- Google Calendar connection management and write approvals; Gmail and Outlook connection management, inbox reads, local drafts, and owner-approved sending
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
const recalled = await client.memories.recall('rollback rehearsal', {
  entityId: 'user-1',
  recentLimit: 0,
});
const trace = recalled[0]
  ? await client.memories.trace(recalled[0].memory.id)
  : null;
const memoryReadiness = await client.memories.readiness();

console.log({
  daemon: health.status,
  agents: agents.length,
  matchingMemories: memories.length,
  tracedRelationships: trace?.relationships.length ?? 0,
  memoryReady: memoryReadiness.passed,
});
```

## Agent conversations

Pass a stable `roomId` to keep subsequent direct turns in the same conversation:

```ts
await client.agents.run(agentId, { text: 'Plan the release' }, { roomId: 'release-planning' });
await client.agents.run(agentId, { text: 'Expand the testing steps' }, { roomId: 'release-planning' });
const peer = await client.agents.sendMessage(agentId, reviewerId, 'Review the release plan');
console.log(peer.agent.state.id, peer.result.data?.text);
```

The two-argument `run(agentId, content)` call remains supported. `sendMessage(senderId, toAgentId, message)` requests one bounded recipient run and returns the recipient's `{ agent, result }` envelope, including nullable daemon result fields. It does not start an automatic message loop.

`update(agentId, patch)` supports `name`, `model`, `provider`, `system`, and JSON tool descriptors. Empty `provider` or `system` strings restore defaults; an empty `tools` array clears tools. It returns the updated snapshot. `remove(agentId)` deletes the agent and resolves without a value.

## Connectors

In a browser with same-origin daemon routing, use `createDaemonClient({ baseUrl: '' })`. A relative prefix such as `/daemon` is also supported. Node's default fetch requires an absolute base URL. Authentication can be supplied through the existing custom `fetch` option.

Configure Google and Microsoft OAuth apps through the daemon's owner-facing UI when possible. The SDK also exposes the same settings for trusted local administration code. Keep client secrets out of logs, errors, analytics, and agent-visible messages.

```ts
const status = await client.connectors.oauthAppStatus('google');

if (!status.configured) {
  await client.connectors.configureOauthApp('google', {
    clientId: process.env.GOOGLE_CLIENT_ID!,
    clientSecret: process.env.GOOGLE_CLIENT_SECRET!,
  });
}

// Remove credentials from the daemon vault when the OAuth app is retired.
await client.connectors.removeOauthApp('google');
```

Microsoft configuration also accepts an optional `tenant`. OAuth app status returns only configuration metadata and a masked client ID hint; it never returns the stored secret.

```ts
const client = createDaemonClient({ baseUrl: '' });
const status = await client.connectors.mailStatus(agentId, 'gmail');
if (status.connector?.status === 'active') {
  const messages = await client.connectors.mailMessages(agentId, 'gmail', status.connector.id);
  const drafts = await client.connectors.mailDrafts(agentId, 'gmail', status.connector.id);
}
```

`connectMail` and `connectCalendar` return `{ connector, consentUrl }` for the owner to authorize. Status methods return `{ configured, connector }`; list methods return arrays; draft/write actions return the updated record. Disconnect methods resolve without a value. Mail arguments are ordered `agentId, provider, connectorId, draftId` as applicable; Calendar omits `provider`.

`createMailDraft` stores `{ to, subject, body }` locally without sending. Call `approveMailDraft` only after explicit owner review; an ambiguous send must not be retried automatically. Calendar writes similarly require `approveCalendarWrite` or `rejectCalendarWrite` after review. These owner-facing approval methods must not be exposed as agent-callable tools.

## Build

Run `bun run build:cli-sdk` to build the SDK and CLI together, or `bun x nx build @animaOS-SWARM/sdk` to build only this package.

## Test

Run `bun x nx test @animaOS-SWARM/sdk`.

Integration tests build the Rust daemon with a separate compilation deadline, then run its binary on a temporary local port and workspace. They use an isolated environment and a local model stub; no developer credentials or persisted workspace data are inherited. `CARGO_TARGET_DIR` can select a build directory (otherwise `target/sdk-daemon-integration`).

## Wire contracts

Agent snapshots use the exported `DaemonAgentState` / `DaemonAgentConfig` types, not executable TypeScript runtime objects. Returned tools have JSON `parameters` and no callable `handler`; returned custom settings are under `settings.additional`. Optional response values are explicitly nullable. Create requests still use `AgentConfig` with flat custom settings: do not pass a returned snapshot config back as a create request unchanged.

Provider credentials and base URLs are configured on the daemon host, not in agent settings. For authenticated deployments, supply a custom `fetch` wrapper that adds `X-Api-Key` while forwarding the request options (including the SSE abort signal).

### Agent profiles and avatars

Use `client.agents.update(id, { name, system })` to edit an agent without replacing its conversation. The system instructions apply on its next run. Use `client.agents.recentMemories(id, { limit: 100 })` to inspect its recorded memory.

`client.agents.setAvatar(id, imageBlob)` stores a PNG, JPEG, or WebP image up to 5 MiB in the configured workspace. `client.agents.removeAvatar(id)` restores the UI fallback. Retrieve image bytes from `GET /api/agents/{agent_id}/avatar` on the daemon origin. Images are served with `Cache-Control: no-store`; they are separate from the workspace avatar and agent configuration JSON.

### Per-agent tasks and proactive schedules

`client.agents.tasks(id)` reads an agent's task list and revision. Pass both to `updateTasks(id, { tasks, revision })`; stale edits return HTTP 409, and UI task saves are rejected while the agent is running. The daemon's `todo_read` and `todo_write` tools use the same per-agent list. Existing shared `todos.json` files remain untouched; they are not copied to every agent.

Use `schedules(id)`, `createSchedule(id, input)`, `updateSchedule(id, scheduleId, { enabled: false })`, and `removeSchedule(id, scheduleId)` to control that agent's proactive work. Schedules persist in the daemon and require it to remain running. Adding a task does not enable a schedule.
