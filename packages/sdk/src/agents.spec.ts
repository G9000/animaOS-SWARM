import { describe, expect, it } from 'vitest';
import { createDaemonClient } from './index.js';

function transport(payload: unknown) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return Response.json(payload);
    },
  });
  return { agents: client.agents, requests };
}

describe('agent transport', () => {
  it('includes a stable room ID alongside content without changing legacy runs', async () => {
    const { agents, requests } = transport({});
    const input = { text: 'Hello', metadata: { source: 'direct' } };
    await agents.run('agent/a b', input, { roomId: 'conversation-1' });
    await agents.run('agent/a b', input);
    expect(requests.map(({ url }) => url)).toEqual([
      '/api/agents/agent%2Fa%20b/run',
      '/api/agents/agent%2Fa%20b/run',
    ]);
    expect(requests[0].init?.method).toBe('POST');
    expect(JSON.parse(requests[0].init?.body as string)).toEqual({
      ...input,
      roomId: 'conversation-1',
    });
    expect(JSON.parse(requests[1].init?.body as string)).toEqual(input);
  });

  it('sends one peer request and preserves recipient result nulls', async () => {
    const envelope = {
      agent: { state: { id: 'recipient' }, lastTask: null },
      result: {
        status: 'error',
        data: null,
        error: 'unavailable',
        durationMs: 4,
      },
    };
    const { agents, requests } = transport(envelope);
    expect(
      await agents.sendMessage('sender/a b', 'recipient/a b', 'Review this'),
    ).toEqual(envelope);
    expect(requests).toHaveLength(1);
    expect(requests[0].url).toBe('/api/agents/sender%2Fa%20b/messages');
    expect(requests[0].init?.method).toBe('POST');
    expect(JSON.parse(requests[0].init?.body as string)).toEqual({
      toAgentId: 'recipient/a b',
      message: 'Review this',
    });
  });

  it('patches supported config fields including clearing tools and defaults', async () => {
    const snapshot = { state: { id: 'agent/a b' }, lastTask: null };
    const { agents, requests } = transport({ agent: snapshot });
    const patch = {
      name: 'New name',
      model: 'local',
      provider: '',
      system: '',
      tools: [],
    };
    expect(await agents.update('agent/a b', patch)).toEqual(snapshot);
    expect(requests[0].url).toBe('/api/agents/agent%2Fa%20b');
    expect(requests[0].init?.method).toBe('PATCH');
    expect(JSON.parse(requests[0].init?.body as string)).toEqual(patch);
  });

  it('removes an encoded agent and consumes the delete envelope', async () => {
    const { agents, requests } = transport({ deleted: true });
    await expect(agents.remove('agent/a b')).resolves.toBeUndefined();
    expect(requests[0].url).toBe('/api/agents/agent%2Fa%20b');
    expect(requests[0].init?.method).toBe('DELETE');
  });
});

it('uploads binary avatars and encodes agent IDs for removal', async () => {
  const { agents, requests } = transport({});
  const image = new Blob(['image bytes'], { type: 'image/png' });
  await agents.setAvatar('agent/a b', image);
  await agents.removeAvatar('agent/a b');
  expect(requests[0].url).toBe('/api/agents/agent%2Fa%20b/avatar');
  expect(requests[0].init?.body).toBe(image);
  expect(requests[0].init?.headers).toMatchObject({
    'content-type': 'image/png',
  });
  expect(requests[1].init?.method).toBe('DELETE');
});

it('preserves task revisions and scopes proactive controls to the selected agent', async () => {
  const { agents, requests } = transport({
    tasks: [],
    revision: 'r1',
    schedules: [],
    schedule: { id: 'daily' },
  });
  const tasks = await agents.tasks('agent/a');
  await agents.updateTasks('agent/a', tasks);
  await agents.schedules('agent/b');
  await agents.createSchedule('agent/b', {
    prompt: 'Check tasks',
    trigger: { type: 'interval', intervalMs: 60000 },
    target: { type: 'workspace' },
    enabled: true,
  });
  await agents.updateSchedule('agent/b', 'schedule/1', { enabled: false });
  await agents.removeSchedule('agent/b', 'schedule/1');
  expect(requests.map((item) => item.url)).toEqual([
    '/api/agents/agent%2Fa/tasks',
    '/api/agents/agent%2Fa/tasks',
    '/api/agents/agent%2Fb/schedules',
    '/api/agents/agent%2Fb/schedules',
    '/api/agents/agent%2Fb/schedules/schedule%2F1',
    '/api/agents/agent%2Fb/schedules/schedule%2F1',
  ]);
  expect(JSON.parse(requests[1].init?.body as string).revision).toBe('r1');
  expect(JSON.parse(requests[4].init?.body as string)).toEqual({
    enabled: false,
  });
});
