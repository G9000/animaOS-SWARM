import type { DaemonStatus, LogLine } from '@animaOS-SWARM/sdk';

export function logLineFixture(
  seq: number,
  overrides: Partial<LogLine> = {},
): LogLine {
  return {
    seq,
    at: new Date(2026, 8, 23, 10, 30, 15, 42).getTime() + seq,
    level: 'info',
    target: 'anima_daemon::runs',
    message: `Line ${seq}`,
    ...overrides,
  };
}

export function statusFixture(
  overrides: Partial<DaemonStatus> = {},
): DaemonStatus {
  return {
    version: '0.9.1',
    buildRevision: 'abc1234',
    startedAtMs: 0,
    nowMs: 11_100_000,
    uptimeSeconds: 11_100,
    readiness: { status: 'ready', issues: [] },
    storage: {
      persistenceMode: 'sqlite',
      controlPlane: 'json',
      controlPlaneDurability: 'durable',
      history: {
        store: 'sqlite',
        ephemeral: false,
        healthy: true,
        pendingFlush: 3,
        usageQueued: 0,
        lastError: null,
        failingSinceMs: null,
        flushErrors: 0,
      },
    },
    providers: [
      { id: 'openai', label: 'OpenAI', configured: true },
      { id: 'anthropic', label: 'Anthropic', configured: false },
    ],
    connectors: [
      {
        id: 'telegram-1',
        agentId: 'agent-main',
        type: 'telegram',
        status: 'ready',
        enabled: true,
      },
    ],
    automations: { total: 4, enabled: 3, failing: 0, failuresTotal: 0 },
    approvals: { pending: 0 },
    runs: { running: 1, queued: 2, byStatus: { completed: 5, running: 1 } },
    events: { subscribers: 2, laggedEvents: 0 },
    logs: { buffered: 100, newestSeq: 100 },
    limits: {
      maxRequestBytes: 1_000_000,
      maxConcurrentRuns: 4,
      maxRunsPerAgent: 2,
      queuedRunsPerAgent: 8,
      maxBackgroundProcesses: 4,
      logBufferLines: 2_000,
      eventBuffer: 1_000,
    },
    ...overrides,
  };
}
