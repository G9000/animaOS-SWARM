import { expect, it } from 'vitest';
import { createDaemonClient, DaemonHttpError } from './index.js';

it('scopes jobs, preserves idempotency and revision inputs, and propagates conflicts', async () => {
  const requests: { url: string; init?: RequestInit }[] = [];
  let conflict = false;
  const job = { id: 'job/1', status: 'needs_review', result: null };
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return conflict
        ? Response.json({ error: 'Changed' }, { status: 409 })
        : Response.json(init?.method ? job : { jobs: [job] });
    },
  });
  const signal = new AbortController().signal;
  expect(await client.agents.jobs('agent/a', { signal })).toEqual([job]);
  const input = {
    title: 'Review',
    prompt: 'Review work',
    requestKey: 'stable-key',
  };
  expect(await client.agents.createJob('agent/a', input)).toEqual(job);
  await client.agents.cancelJob('agent/a', 'job/1', { revision: 2 });
  await client.agents.retryJob('agent/a', 'job/1', {
    revision: 3,
    acknowledgeUncertain: true,
  });
  expect(requests.map((r) => r.url)).toEqual([
    '/api/agents/agent%2Fa/jobs',
    '/api/agents/agent%2Fa/jobs',
    '/api/agents/agent%2Fa/jobs/job%2F1/cancel',
    '/api/agents/agent%2Fa/jobs/job%2F1/retry',
  ]);
  expect(requests[0].init?.signal).toBe(signal);
  expect(JSON.parse(requests[1].init?.body as string)).toEqual(input);
  expect(JSON.parse(requests[2].init?.body as string)).toEqual({ revision: 2 });
  expect(JSON.parse(requests[3].init?.body as string)).toEqual({
    revision: 3,
    acknowledgeUncertain: true,
  });
  conflict = true;
  await expect(
    client.agents.cancelJob('agent/a', 'job/1', { revision: 2 }),
  ).rejects.toBeInstanceOf(DaemonHttpError);
});
