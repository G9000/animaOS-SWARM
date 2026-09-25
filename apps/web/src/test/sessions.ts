import type { Session } from '@animaOS-SWARM/sdk';

/** A read-write chat of `agent-main` with every derived field. */
export function sessionFixture(
  id: string,
  overrides: Partial<Session> = {},
): Session {
  return {
    id,
    agentId: 'agent-main',
    roomId: id,
    kind: 'chat',
    origin: 'web',
    title: 'New chat',
    titleSource: 'first_message',
    createdAtMs: 1,
    lastActivityAtMs: 1,
    lastReadAtMs: null,
    archived: false,
    parentSessionId: null,
    parentRunId: null,
    parentAgentId: null,
    summary: null,
    contextTrimmed: null,
    messageCount: 0,
    preview: null,
    activeRuns: 0,
    pendingApprovals: 0,
    unread: false,
    capabilities: {
      send: true,
      steer: true,
      stop: true,
      rename: true,
      archive: true,
      delete: true,
      compact: true,
      export: true,
    },
    ...overrides,
  };
}
