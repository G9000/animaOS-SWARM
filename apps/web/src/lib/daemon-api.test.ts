import { afterEach, describe, expect, it, vi } from 'vitest';

import { daemon, toAgentDetail, type DaemonSnapshot } from './daemon-api';

function snapshot(): DaemonSnapshot {
  return {
    state: {
      id: 'agent-1',
      name: 'Anima',
      status: 'idle',
      config: {
        name: 'Anima',
        model: 'deterministic',
        provider: 'deterministic',
        system: 'Stay exact',
        tools: [
          {
            name: 'read_file',
            description: 'Read a workspace file',
            parameters: {
              type: 'object',
              properties: { file_path: { type: 'string' } },
              required: ['file_path'],
            },
            examples: null,
          },
          {
            name: 'grep',
            description: 'Search workspace files',
            parameters: {
              type: 'object',
              properties: { pattern: { type: 'string' } },
              required: ['pattern'],
            },
            examples: [
              {
                input: 'Find TODOs',
                args: { pattern: 'TODO' },
                output: 'src/main.ts:12: TODO',
              },
            ],
          },
        ],
      },
      createdAtMs: 10,
      tokenUsage: {
        promptTokens: 1,
        completionTokens: 2,
        totalTokens: 3,
      },
    },
    messageCount: 4,
    messages: [
      {
        id: 'visible-user',
        agentId: 'agent-1',
        roomId: 'room-1',
        role: 'user',
        content: { text: 'Hello' },
        createdAtMs: 11,
      },
      {
        id: 'hidden-checkin',
        agentId: 'agent-1',
        roomId: 'room-1',
        role: 'user',
        content: { text: 'Status?', metadata: { kind: 'checkin' } },
        createdAtMs: 12,
      },
      {
        id: 'hidden-ok',
        agentId: 'agent-1',
        roomId: 'room-1',
        role: 'assistant',
        content: { text: '  CHECKIN_OK  ' },
        createdAtMs: 13,
      },
      {
        id: 'visible-assistant',
        agentId: 'agent-1',
        roomId: 'room-1',
        role: 'assistant',
        content: { text: 'Hi' },
        createdAtMs: 14,
      },
    ],
    eventCount: 0,
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('toAgentDetail', () => {
  it('maps canonical tool descriptors to ordered names and keeps message filtering', () => {
    const detail = toAgentDetail(snapshot());

    expect(detail.toolNames).toEqual(['read_file', 'grep']);
    expect(detail.messages.map((message) => message.id)).toEqual([
      'visible-user',
      'visible-assistant',
    ]);
  });

  it('uses an empty tool-name list when the daemon omits tools', () => {
    const withoutTools = snapshot();
    delete withoutTools.state.config.tools;

    expect(toAgentDetail(withoutTools).toolNames).toEqual([]);
  });
});

describe('daemon agent requests', () => {
  it('sends required tool names when creating an agent', async () => {
    const created = snapshot();
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ agent: created }), {
        status: 201,
        headers: { 'content-type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    await daemon.createAgent({
      name: 'Anima',
      model: 'deterministic',
      provider: 'deterministic',
      system: 'Stay exact',
      tools: ['read_file', 'grep'],
    });

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/agents',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          name: 'Anima',
          model: 'deterministic',
          provider: 'deterministic',
          system: 'Stay exact',
          tools: ['read_file', 'grep'],
        }),
      }),
    );
  });

  it('sends tool names together with the rest of an update patch', async () => {
    const updated = snapshot();
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ agent: updated }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    await daemon.updateAgent('agent-1', {
      name: 'Renamed',
      provider: '',
      system: '',
      tools: ['bash'],
    });

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/agents/agent-1',
      expect.objectContaining({
        method: 'PATCH',
        body: JSON.stringify({
          name: 'Renamed',
          provider: '',
          system: '',
          tools: ['bash'],
        }),
      }),
    );
  });
});

describe('daemon integration requests', () => {
  it('uses connector routes and requires a caller supplied idempotency key for sends', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockImplementation(
      async () =>
        new Response(JSON.stringify({ messages: [], nextBefore: null }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
    );
    vi.stubGlobal('fetch', fetchMock);

    await daemon.listConnectorMessages('agent 1', 'connector/1', {
      before: 'message 1',
      limit: 25,
    });
    await daemon.sendConnectorMessage(
      'agent 1',
      'connector/1',
      'hello',
      'telegram-send-1',
    );

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      '/api/agents/agent%201/connectors/connector%2F1/messages?before=message+1&limit=25',
      expect.any(Object),
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      '/api/agents/agent%201/connectors/connector%2F1/messages',
      expect.objectContaining({
        method: 'POST',
        headers: expect.objectContaining({
          'content-type': 'application/json',
          'Idempotency-Key': 'telegram-send-1',
        }),
        body: JSON.stringify({ text: 'hello' }),
      }),
    );
  });

  it('uses schedule CRUD and import wire payloads unchanged', async () => {
    const schedule = {
      id: 'schedule-1',
      importIdempotencyKey: null,
      agentId: 'agent-1',
      prompt: 'Check goals',
      trigger: { type: 'interval' as const, intervalMs: 60_000 },
      enabled: true,
      target: { type: 'workspace' as const },
      nextDueAtMs: 61_000,
      lastFiredAtMs: null,
      lastOutcome: null,
      createdAtMs: 1_000,
      updatedAtMs: 1_000,
    };
    const fetchMock = vi.fn<typeof fetch>().mockImplementation(
      async () =>
        new Response(JSON.stringify({ schedule, schedules: [schedule] }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
    );
    vi.stubGlobal('fetch', fetchMock);

    await daemon.createSchedule('agent-1', {
      prompt: 'Check goals',
      trigger: { type: 'interval', intervalMs: 60_000 },
      target: { type: 'workspace' },
    });
    await daemon.updateSchedule('agent-1', 'schedule-1', { enabled: false });
    await daemon.deleteSchedule('agent-1', 'schedule-1');
    await daemon.importLegacySchedules('agent-1', {
      schedules: [
        {
          id: 'legacy-1',
          prompt: 'Check goals',
          intervalSecs: 60,
          createdAtMs: 1_000,
          lastRunAtMs: 2_000,
        },
      ],
    });

    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      '/api/agents/agent-1/schedules',
      '/api/agents/agent-1/schedules/schedule-1',
      '/api/agents/agent-1/schedules/schedule-1',
      '/api/agents/agent-1/schedules/import',
    ]);
    expect(fetchMock.mock.calls[3]?.[1]).toEqual(
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          schedules: [
            {
              id: 'legacy-1',
              prompt: 'Check goals',
              intervalSecs: 60,
              createdAtMs: 1_000,
              lastRunAtMs: 2_000,
            },
          ],
        }),
      }),
    );
  });
});
