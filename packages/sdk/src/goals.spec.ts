import { expect, it } from 'vitest';
import { createDaemonClient } from './client.js';
it('roundtrips goal creation, revision status and scoped jobs', async () => {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return Response.json(
        String(url).endsWith('/jobs')
          ? { jobs: [] }
          : init?.method
            ? { id: 'g' }
            : { goals: [] },
      );
    },
  });
  expect(await client.goals.list()).toEqual([]);
  const input = {
    title: 'Launch',
    objective: 'Deliver',
    requestKey: 'stable',
    maxAttempts: 4,
  };
  await client.goals.create(input);
  await client.goals.setStatus('g/a', { revision: 2, status: 'paused' });
  expect(await client.goals.jobs('g/a')).toEqual([]);
  expect(requests.map((r) => r.url)).toEqual([
    '/api/goals',
    '/api/goals',
    '/api/goals/g%2Fa/status',
    '/api/goals/g%2Fa/jobs',
  ]);
  expect(JSON.parse(requests[1].init?.body as string)).toEqual(input);
  expect(JSON.parse(requests[2].init?.body as string)).toEqual({
    revision: 2,
    status: 'paused',
  });
});
