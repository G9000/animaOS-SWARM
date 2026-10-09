import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  daemon,
  toAgentDetail,
  toChatMessage,
  PROFILE_GENERATION_UNAVAILABLE,
  workspaceAvatarUrl,
  type DaemonSnapshot,
} from './daemon-api';

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
  it('maps the durable agency lead role', () => {
    const lead = snapshot();
    lead.state.config.settings = { additional: { workspaceRole: 'lead' } };
    expect(toAgentDetail(lead).workspaceRole).toBe('lead');
  });
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

describe('daemon workspace requests', () => {
  const workspaceState = {
    configured: true,
    workspace: {
      rootPath: '/srv/company',
      companyName: 'Acme',
      mission: 'Ship it',
      values: ['rigor', 'care'],
      hasAvatar: false,
    },
    defaultRoot: '/srv',
  };

  it('getWorkspace fetches /workspace and parses configured/defaultRoot', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify(workspaceState), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const state = await daemon.getWorkspace();

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/workspace',
      expect.any(Object),
    );
    expect(state.configured).toBe(true);
    expect(state.defaultRoot).toBe('/srv');
    expect(state.workspace?.companyName).toBe('Acme');
  });

  it('putWorkspace PUTs to /workspace with the exact body', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify(workspaceState), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const input = {
      rootPath: '/srv/company',
      companyName: 'Acme',
      mission: 'Ship it',
      values: ['rigor', 'care'],
    };
    await daemon.putWorkspace(input);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/workspace',
      expect.objectContaining({
        method: 'PUT',
        body: JSON.stringify(input),
      }),
    );
  });

  it('uploads the workspace avatar as raw bytes and accepts 204', async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchMock);
    const file = new File(['avatar-bytes'], 'avatar.png', {
      type: 'image/png',
    });

    await daemon.uploadWorkspaceAvatar(file);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/workspace/avatar',
      expect.objectContaining({
        method: 'PUT',
        body: file,
        headers: expect.objectContaining({ 'content-type': 'image/png' }),
      }),
    );
  });

  it('builds a cache-busted workspace avatar URL', () => {
    expect(workspaceAvatarUrl(3)).toBe('/api/workspace/avatar?v=3');
  });

  it('validateWorkspace PUTs with validateOnly', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(
        JSON.stringify({ ...workspaceState, rootPathExists: false }),
        {
          status: 200,
          headers: { 'content-type': 'application/json' },
        },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const input = {
      rootPath: '/srv/company',
      companyName: 'Acme',
      mission: 'Ship it',
      values: ['rigor', 'care'],
    };
    const state = await daemon.validateWorkspace(input);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/workspace',
      expect.objectContaining({
        method: 'PUT',
        body: JSON.stringify({ ...input, validateOnly: true }),
      }),
    );
    expect(state.rootPathExists).toBe(false);
  });

  it('generateProfile POSTs preset, intent, model, and workspace identity', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(
        JSON.stringify({
          profile: {
            bio: 'A precise operator.',
            adjectives: ['precise', 'calm'],
            style: 'Concise',
            system: 'You are precise.',
          },
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const input = {
      presetId: 'operator',
      intent: 'Runs the back office',
      provider: 'anthropic',
      model: 'claude-sonnet-4',
      workspace: {
        companyName: 'Acme',
        mission: 'Ship it',
        values: ['rigor', 'care'],
      },
    };
    const result = await daemon.generateProfile(input);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/agents/generate-profile',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify(input),
      }),
    );
    expect(result.profile.adjectives).toEqual(['precise', 'calm']);
  });

  it('generateProfile surfaces the PROFILE_GENERATION_UNAVAILABLE error prefix', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(
        JSON.stringify({
          error:
            'PROFILE_GENERATION_UNAVAILABLE: no generative provider configured',
        }),
        { status: 400, headers: { 'content-type': 'application/json' } },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const error = await daemon
      .generateProfile({
        presetId: 'operator',
        intent: 'Runs the back office',
        provider: 'anthropic',
        model: 'claude-sonnet-4',
        workspace: {
          companyName: 'Acme',
          mission: 'Ship it',
          values: ['rigor', 'care'],
        },
      })
      .catch((err: unknown) => err);

    expect(error).toBeInstanceOf(Error);
    expect(
      (error as Error).message.startsWith(PROFILE_GENERATION_UNAVAILABLE),
    ).toBe(true);
  });

  it('bootstrapWorkspace POSTs workspace and agent payloads', async () => {
    const created = snapshot();
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(
        JSON.stringify({
          workspace: workspaceState.workspace,
          agent: created,
        }),
        { status: 201, headers: { 'content-type': 'application/json' } },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const input = {
      workspace: {
        rootPath: '/srv/company',
        companyName: 'Acme',
        mission: 'Ship it',
        values: ['rigor', 'care'],
      },
      agent: {
        name: 'Anima',
        presetId: 'operator',
        bio: 'A precise operator.',
        system: 'You are precise.',
        model: 'claude-sonnet-4',
        tools: ['read_file'],
      },
    };
    const result = await daemon.bootstrapWorkspace(input);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/workspace/bootstrap',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify(input),
      }),
    );
    expect(result.agent.state.id).toBe('agent-1');
  });

  it('inspectWorkspace issues GET with encoded rootPath', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ found: false }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const result = await daemon.inspectWorkspace('C:\\anima');

    expect(result).toEqual({ found: false });
    expect(fetchMock).toHaveBeenCalledWith(
      `/api/workspace/inspect?rootPath=${encodeURIComponent('C:\\anima')}`,
      expect.any(Object),
    );
  });

  it('inspectWorkspace parses the found preview envelope', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(
        JSON.stringify({
          found: true,
          companyName: 'Acme',
          mission: 'Ship it',
          values: ['rigor', 'care'],
          orchestrator: {
            name: 'Anima',
            bio: 'A precise operator.',
            provider: 'anthropic',
            model: 'claude-sonnet-4',
          },
          workers: [
            { name: 'Scout', provider: 'anthropic', model: 'claude-sonnet-4' },
          ],
          providerAvailable: true,
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const result = await daemon.inspectWorkspace('/srv/company');

    expect(result.found).toBe(true);
    if (result.found) {
      expect(result.companyName).toBe('Acme');
      expect(result.orchestrator.name).toBe('Anima');
      expect(result.workers).toHaveLength(1);
      expect(result.providerAvailable).toBe(true);
    }
  });

  it('inspectWorkspace parses a found preview without mission/values', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(
        JSON.stringify({
          found: true,
          companyName: 'Acme',
          orchestrator: {
            name: 'Anima',
            provider: 'anthropic',
            model: 'claude-sonnet-4',
          },
          workers: [],
          providerAvailable: false,
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const result = await daemon.inspectWorkspace('/srv/company');

    expect(result.found).toBe(true);
    if (result.found) {
      expect(result.companyName).toBe('Acme');
      expect(result.mission).toBeUndefined();
      expect(result.values).toBeUndefined();
      expect(result.workers).toEqual([]);
    }
  });

  it('resumeWorkspace posts rootPath and returns the envelope', async () => {
    const orchestrator = snapshot();
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(
        JSON.stringify({
          workspace: workspaceState.workspace,
          orchestrator,
          workers: [],
          skipped: ['Scout'],
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const result = await daemon.resumeWorkspace('C:\\anima');

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/workspace/resume',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ rootPath: 'C:\\anima' }),
      }),
    );
    expect(result.workspace.companyName).toBe('Acme');
    expect(result.orchestrator.state.id).toBe('agent-1');
    expect(result.workers).toEqual([]);
    expect(result.skipped).toEqual(['Scout']);
  });
});

describe('daemon integration requests', () => {
  it('reads a connector’s messages with its page query', async () => {
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

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/agents/agent%201/connectors/connector%2F1/messages?before=message+1&limit=25',
      expect.any(Object),
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

describe('daemon session requests', () => {
  it('reads and changes sessions through the SDK routes', async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockImplementation(async (input) => {
        const url = String(input);
        if (url.endsWith('/export'))
          return new Response('# Plans\n', {
            status: 200,
            headers: { 'content-type': 'text/markdown' },
          });
        const body = url.includes('/messages')
          ? { messages: [], nextBefore: null }
          : {
              sessions: [],
              nextCursor: null,
              session: { id: 'chat:1' },
              deleted: true,
            };
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      });
    vi.stubGlobal('fetch', fetchMock);

    await daemon.listSessions('agent 1', {
      includeHelpers: true,
      archived: false,
      limit: 200,
    });
    await daemon.createSession('agent 1');
    await daemon.updateSession('agent 1', 'chat:1', { lastReadAtMs: 4 });
    await daemon.sessionMessages('agent 1', 'chat:1', {
      before: 'm1',
      limit: 50,
    });
    await daemon.deleteSession('agent 1', 'chat:1');
    expect(await daemon.exportSession('agent 1', 'chat:1')).toBe('# Plans\n');

    expect(
      fetchMock.mock.calls.map(
        ([url, init]) => `${init?.method ?? 'GET'} ${String(url)}`,
      ),
    ).toEqual([
      'GET /api/agents/agent%201/sessions?archived=false&limit=200&includeHelpers=true',
      'POST /api/agents/agent%201/sessions',
      'PATCH /api/agents/agent%201/sessions/chat%3A1',
      'GET /api/agents/agent%201/sessions/chat%3A1/messages?before=m1&limit=50',
      'DELETE /api/agents/agent%201/sessions/chat%3A1',
      'GET /api/agents/agent%201/sessions/chat%3A1/export',
    ]);
  });

  it('adapts session messages and folds check-in prompts into a system line', () => {
    expect(
      toChatMessage({
        id: 'c1',
        role: 'user',
        text: 'Check goals\n\n(This is a scheduled check-in. If you have nothing worth saying right now, reply with exactly CHECKIN_OK and nothing else.)',
        attachments: [],
        metadata: { kind: 'checkin', id: 's1' },
        createdAtMs: 5,
      }),
    ).toEqual({
      id: 'c1',
      role: 'System',
      content: { text: 'Check goals', metadata: { kind: 'checkin', id: 's1' } },
      created_at_ms: 5,
    });
    expect(
      toChatMessage({
        id: 't1',
        role: 'tool',
        text: '{}',
        attachments: [],
        metadata: {},
        createdAtMs: 6,
      }).role,
    ).toBe('Tool');
  });
});

describe('daemon memory requests', () => {
  it('reads and changes memories, facts, and entities through the SDK routes', async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockImplementation(async (input) => {
        const url = String(input);
        const body = url.includes('/search')
          ? { results: [] }
          : url.includes('/facts') && !url.includes('/facts/')
            ? { facts: [] }
            : url.includes('/entities?kind')
              ? { kind: 'user', id: 'e 1' }
              : url.includes('/entities')
                ? { entities: [] }
                : url.includes('/relationships')
                  ? { relationships: [] }
                  : url.includes('/recent')
                    ? { memories: [] }
                    : {};
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      });
    vi.stubGlobal('fetch', fetchMock);

    await daemon.recentMemories('agent 1', 200);
    await daemon.searchMemories('tea', 'agent 1', 200);
    await daemon.traceMemory('m 1');
    await daemon.updateMemory('m 1', { content: 'New' });
    await daemon.deleteMemory('m 1');
    await daemon.saveMemory({
      agentId: 'agent 1',
      agentName: 'Anima',
      type: 'fact',
      content: 'Saved',
      importance: 0.5,
    });
    await daemon.listFacts({
      agentId: 'agent 1',
      includeInactive: true,
      limit: 500,
    });
    await daemon.replaceFact('f 1', 'Value');
    await daemon.deleteFact('f 1');
    await daemon.listMemoryEntities(200);
    await daemon.listMemoryRelationships('agent 1', 200);
    await daemon.deleteMemoryEntity('user', 'e 1');

    expect(
      fetchMock.mock.calls.map(
        ([url, init]) => `${init?.method ?? 'GET'} ${String(url)}`,
      ),
    ).toEqual([
      'GET /api/memories/recent?agentId=agent+1&limit=200',
      'GET /api/memories/search?q=tea&agentId=agent+1&limit=200',
      'GET /api/memories/m%201/trace',
      'PATCH /api/memories/m%201',
      'DELETE /api/memories/m%201',
      'POST /api/memories',
      'GET /api/memories/facts?agentId=agent+1&includeInactive=true&limit=500',
      'PATCH /api/memories/facts/f%201',
      'DELETE /api/memories/facts/f%201',
      'GET /api/memories/entities?limit=200',
      'GET /api/memories/relationships?agentId=agent+1&limit=200',
      'DELETE /api/memories/entities/e%201?kind=user',
    ]);
  });
});

describe('daemon usage requests', () => {
  it('reads usage, exports the csv, and reads and sets the pricing through the SDK routes', async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockImplementation(async (input) => {
        const url = String(input);
        if (url.includes('export.csv'))
          return new Response('id,agent\n', { status: 200 });
        const body = url.includes('/pricing')
          ? { overrides: [], tableDate: '2026-09-01' }
          : url.includes('/records')
            ? { records: [], nextCursor: null }
            : {};
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      });
    vi.stubGlobal('fetch', fetchMock);

    await daemon.usageSummary({
      from: 1,
      to: 2,
      agentId: 'agent 1',
      groupBy: 'day',
      tzOffsetMinutes: 60,
    });
    await daemon.usageRecords({ agentId: 'agent 1', limit: 50 });
    expect(await daemon.exportUsageCsv({ from: 1, to: 2 })).toBe('id,agent\n');
    await daemon.usagePricing();
    await daemon.setUsagePricing([]);

    expect(
      fetchMock.mock.calls.map(
        ([url, init]) => `${init?.method ?? 'GET'} ${String(url)}`,
      ),
    ).toEqual([
      'GET /api/usage/summary?from=1&to=2&agentId=agent+1&groupBy=day&tzOffsetMinutes=60',
      'GET /api/usage/records?agentId=agent+1&limit=50',
      'GET /api/usage/export.csv?from=1&to=2',
      'GET /api/usage/pricing',
      'PUT /api/usage/pricing',
    ]);
  });
});

describe('daemon logs and status requests', () => {
  it('lists logs, opens the stream, and reads the status through the SDK routes', async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockImplementation(async (input) => {
        const url = String(input);
        if (url.startsWith('/api/logs/stream'))
          return new Response(
            'event: log\ndata: {"seq":7,"at":1,"level":"info","target":"t","message":"m"}\n\nevent: resync\ndata: {"newestSeq":9}\n\n',
            {
              status: 200,
              headers: { 'content-type': 'text/event-stream' },
            },
          );
        const body = url.startsWith('/api/logs')
          ? { lines: [], newestSeq: 0 }
          : { version: '1' };
        return new Response(JSON.stringify(body), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      });
    vi.stubGlobal('fetch', fetchMock);

    await daemon.logs({ level: 'warn', q: 'a b', after: 3, limit: 500 });
    const events = [];
    for await (const event of daemon.logStream({ level: 'info', after: 5 }))
      events.push(event);
    await daemon.status();

    expect(
      fetchMock.mock.calls.map(
        ([url, init]) => `${init?.method ?? 'GET'} ${String(url)}`,
      ),
    ).toEqual([
      'GET /api/logs?level=warn&q=a+b&after=3&limit=500',
      'GET /api/logs/stream?level=info&after=5',
      'GET /api/status',
    ]);
    expect(events.map((event) => event.kind)).toEqual(['line', 'resync']);
  });
});
