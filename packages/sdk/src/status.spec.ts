import { describe, expect, it } from 'vitest';

import {
  createDaemonClient,
  DaemonHttpError,
  STATUS_TOO_OLD,
  StatusTooOldError,
  type DaemonStatus,
} from './index.js';

const status: DaemonStatus = {
  version: '0.1.0',
  buildRevision: null,
  startedAtMs: 1_000,
  nowMs: 61_000,
  uptimeSeconds: 60,
  readiness: { status: 'not_ready', issues: ['no provider configured'] },
  storage: {
    persistenceMode: 'sqlite',
    controlPlane: 'json',
    controlPlaneDurability: 'durable',
    history: {
      store: 'sqlite',
      ephemeral: false,
      healthy: false,
      pendingFlush: 3,
      usageQueued: 1,
      lastError: 'disk full',
      failingSinceMs: 5_000,
      flushErrors: 2,
    },
  },
  providers: [{ id: 'openai', label: 'OpenAI', configured: true }],
  connectors: [
    {
      id: 'c1',
      agentId: 'agent/a',
      type: 'telegram',
      status: 'running',
      enabled: true,
    },
  ],
  automations: { total: 4, enabled: 3, failing: 1, failuresTotal: 7 },
  approvals: { pending: 2 },
  runs: { running: 1, queued: 1, byStatus: { running: 1, queued: 1 } },
  events: { subscribers: 2, laggedEvents: 0 },
  logs: { buffered: 100, newestSeq: 100 },
  limits: {
    maxRequestBytes: 1_048_576,
    maxConcurrentRuns: 8,
    maxRunsPerAgent: 4,
    queuedRunsPerAgent: 8,
    maxBackgroundProcesses: 8,
    logBufferLines: 2_000,
    eventBuffer: 1_024,
  },
};

function statusClient(respond: () => Response) {
  const requests: string[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url) => {
      requests.push(String(url));
      return respond();
    },
  });
  return { status: client.status, requests };
}

describe('status client', () => {
  it('get parses the status', async () => {
    const { status: client, requests } = statusClient(() =>
      Response.json(status),
    );

    expect(await client.get()).toEqual(status);
    expect(requests).toEqual(['/api/status']);
  });

  it('a 404 becomes StatusTooOldError with the update text', async () => {
    const { status: client } = statusClient(() =>
      Response.json({ error: 'not found' }, { status: 404 }),
    );

    const failure = await client.get().catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(StatusTooOldError);
    expect(failure).toMatchObject({
      code: 'daemon_too_old',
      message: STATUS_TOO_OLD,
    });
    expect(STATUS_TOO_OLD).toBe('Update the daemon to see its health.');
  });

  it('other failures stay DaemonHttpError', async () => {
    const { status: client } = statusClient(() =>
      Response.json(
        { error: 'local owner authorization required' },
        { status: 403 },
      ),
    );

    const failure = await client.get().catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(DaemonHttpError);
    expect(failure).toMatchObject({ status: 403 });
  });

  it('the status types accept the full daemon JSON', async () => {
    const { status: client } = statusClient(() =>
      Response.json(JSON.parse(JSON.stringify(status))),
    );

    const parsed: DaemonStatus = await client.get();

    expect(parsed.storage.history.lastError).toBe('disk full');
    expect(parsed.runs.byStatus.running).toBe(1);
    expect(parsed.connectors[0].type).toBe('telegram');
  });
});
