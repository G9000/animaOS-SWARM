import { describe, expect, it } from 'vitest';

import {
  AUTOMATION_PREVIEW_RUNS,
  MAX_AUTOMATION_HISTORY,
  MAX_AUTOMATION_NAME_CHARS,
  MAX_AUTOMATIONS_PER_AGENT,
  createDaemonClient,
  type Automation,
  type AutomationRun,
} from './index.js';

function transport(respond: (url: string, init?: RequestInit) => Response) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return respond(String(url), init);
    },
  });
  return { automations: client.automations, requests };
}

const automation: Automation = {
  id: 'schedule/1',
  agentId: 'agent/a',
  name: 'Stretch',
  prompt: 'Remind me to stretch',
  trigger: { type: 'interval', intervalMs: 1_800_000 },
  activeHours: null,
  enabled: true,
  target: { type: 'workspace' },
  nextDueAtMs: 10,
  lastFiredAtMs: null,
  lastOutcome: null,
  running: false,
  createdBy: {
    kind: 'agent',
    agentId: 'agent/a',
    sessionId: 'chat:1',
    runId: 'run_1',
    toolCallId: 'call-1',
  },
  preset: null,
  counters: { runs: 0, failures: 0, consecutiveFailures: 0 },
  importIdempotencyKey: null,
  createdAtMs: 1,
  updatedAtMs: 1,
};

const run: AutomationRun = {
  id: 'schedule:schedule/1:5',
  scheduleId: 'schedule/1',
  agentId: 'agent/a',
  firedAtMs: 5,
  finishedAtMs: 9,
  outcome: 'failed',
  runId: 'run_2',
  sessionId: 'schedule:schedule/1',
  errorCode: 'schedule_run_failed',
  manual: true,
};

describe('automations client', () => {
  it('lists, creates, edits, runs, and deletes with encoded ids', async () => {
    const { automations, requests } = transport((url, init) => {
      if (init?.method === 'DELETE') return Response.json({ deleted: true });
      if (!init?.method) return Response.json({ schedules: [automation] });
      if (url.endsWith('/run'))
        return Response.json({ schedule: automation }, { status: 202 });
      return Response.json({ schedule: automation });
    });

    expect(await automations.list('agent/a')).toEqual([automation]);
    await automations.create('agent/a', {
      prompt: 'Remind me to stretch',
      trigger: { type: 'cron', expression: '0 9 * * 1-5', timeZone: 'UTC' },
      name: 'Stretch',
      activeHours: {
        start: '08:00',
        end: '22:00',
        days: [1, 2],
        timeZone: 'UTC',
      },
    });
    await automations.createHeartbeat('agent/a', { timeZone: 'Europe/London' });
    await automations.update('agent/a', 'schedule/1', {
      enabled: false,
      activeHours: null,
    });
    expect(await automations.runNow('agent/a', 'schedule/1')).toEqual(
      automation,
    );
    await automations.remove('agent/a', 'schedule/1');

    expect(
      requests.map(({ url, init }) => [init?.method ?? 'GET', url, init?.body]),
    ).toEqual([
      ['GET', '/api/agents/agent%2Fa/schedules', undefined],
      [
        'POST',
        '/api/agents/agent%2Fa/schedules',
        JSON.stringify({
          prompt: 'Remind me to stretch',
          trigger: { type: 'cron', expression: '0 9 * * 1-5', timeZone: 'UTC' },
          name: 'Stretch',
          activeHours: {
            start: '08:00',
            end: '22:00',
            days: [1, 2],
            timeZone: 'UTC',
          },
        }),
      ],
      [
        'POST',
        '/api/agents/agent%2Fa/schedules',
        JSON.stringify({ preset: 'heartbeat', timeZone: 'Europe/London' }),
      ],
      [
        'PATCH',
        '/api/agents/agent%2Fa/schedules/schedule%2F1',
        JSON.stringify({ enabled: false, activeHours: null }),
      ],
      ['POST', '/api/agents/agent%2Fa/schedules/schedule%2F1/run', undefined],
      ['DELETE', '/api/agents/agent%2Fa/schedules/schedule%2F1', undefined],
    ]);
  });

  it('reads the history and previews fire times', async () => {
    const { automations, requests } = transport((url) =>
      url.includes('/history')
        ? Response.json({ runs: [run] })
        : Response.json({ nextRuns: [1, 2, 3] }),
    );

    expect(
      await automations.history('agent/a', 'schedule/1', { limit: 10 }),
    ).toEqual([run]);
    expect(await automations.history('agent/a', 'schedule/1')).toEqual([run]);
    expect(
      await automations.preview({
        trigger: { type: 'interval', intervalMs: 60_000 },
      }),
    ).toEqual([1, 2, 3]);

    expect(requests.map(({ url, init }) => [url, init?.body])).toEqual([
      [
        '/api/agents/agent%2Fa/schedules/schedule%2F1/history?limit=10',
        undefined,
      ],
      ['/api/agents/agent%2Fa/schedules/schedule%2F1/history', undefined],
      [
        '/api/schedules/preview',
        JSON.stringify({ trigger: { type: 'interval', intervalMs: 60_000 } }),
      ],
    ]);
  });

  it('exports the daemon limits', () => {
    expect(MAX_AUTOMATIONS_PER_AGENT).toBe(20);
    expect(MAX_AUTOMATION_HISTORY).toBe(50);
    expect(MAX_AUTOMATION_NAME_CHARS).toBe(80);
    expect(AUTOMATION_PREVIEW_RUNS).toBe(3);
  });
});
