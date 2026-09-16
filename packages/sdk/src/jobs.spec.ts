import { expect, it } from 'vitest';
import { createDaemonClient, DaemonHttpError, type AgentJob } from './index.js';

it('scopes jobs, preserves idempotency and revision inputs, and propagates conflicts', async () => {
  const requests: { url: string; init?: RequestInit }[] = [];
  let conflict = false;
  const job: AgentJob = {
    goalId: null,
    id: 'job/1',
    agentId: 'agent/a',
    title: 'Review',
    prompt: 'Work',
    requestKey: 'stable-key',
    status: 'needs_review',
    result: null,
    error: 'Interrupted',
    revision: 3,
    attempt: 1,
    maxAttempts: 2,
    requiresApproval: true,
    approvedAtMs: 1,
    createdAtMs: 1,
    updatedAtMs: 2,
    startedAtMs: 1,
    finishedAtMs: 2,
    attempts: [
      {
        attempt: 1,
        status: 'needs_review',
        startedAtMs: 1,
        finishedAtMs: 2,
        result: null,
        error: 'Interrupted',
        resultTruncated: false,
        review: null,
      },
    ],
  };
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
    maxAttempts: 2,
    requiresApproval: true,
  };
  expect(await client.agents.createJob('agent/a', input)).toEqual(job);
  await client.agents.cancelJob('agent/a', 'job/1', { revision: 2 });
  await client.agents.retryJob('agent/a', 'job/1', {
    revision: 3,
    acknowledgeUncertain: true,
  });
  await client.agents.approveJob('agent/a', 'job/1', { revision: 4 });
  await client.agents.reviewJob('agent/a', 'job/1', {
    revision: 5,
    decision: 'changes_requested',
    note: 'Add evidence',
  });
  expect(requests.map((r) => r.url)).toEqual([
    '/api/agents/agent%2Fa/jobs',
    '/api/agents/agent%2Fa/jobs',
    '/api/agents/agent%2Fa/jobs/job%2F1/cancel',
    '/api/agents/agent%2Fa/jobs/job%2F1/retry',
    '/api/agents/agent%2Fa/jobs/job%2F1/approve',
    '/api/agents/agent%2Fa/jobs/job%2F1/review',
  ]);
  expect(requests[0].init?.signal).toBe(signal);
  expect(JSON.parse(requests[1].init?.body as string)).toEqual(input);
  expect(JSON.parse(requests[2].init?.body as string)).toEqual({ revision: 2 });
  expect(JSON.parse(requests[3].init?.body as string)).toEqual({
    revision: 3,
    acknowledgeUncertain: true,
  });
  conflict = true;
  expect(JSON.parse(requests[4].init?.body as string)).toEqual({ revision: 4 });
  expect(JSON.parse(requests[5].init?.body as string)).toEqual({
    revision: 5,
    decision: 'changes_requested',
    note: 'Add evidence',
  });
  await expect(
    client.agents.cancelJob('agent/a', 'job/1', { revision: 2 }),
  ).rejects.toBeInstanceOf(DaemonHttpError);
});
