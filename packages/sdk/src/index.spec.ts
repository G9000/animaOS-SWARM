import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  agent,
  createDaemonClient,
  DaemonConnectionError,
  DaemonHttpError,
  swarm,
} from './index.js';

const originalFetchDescriptor = Object.getOwnPropertyDescriptor(
  globalThis,
  'fetch'
);

function jsonResponse(body: unknown, init?: ResponseInit): Response {
  return new Response(JSON.stringify(body), {
    status: init?.status ?? 200,
    headers: {
      'content-type': 'application/json',
      ...init?.headers,
    },
  });
}

function sseResponse(messages: string[]): Response {
  const encoder = new TextEncoder();

  return new Response(
    new ReadableStream({
      start(controller) {
        for (const message of messages) {
          controller.enqueue(encoder.encode(message));
        }
        controller.close();
      },
    }),
    {
      headers: {
        'content-type': 'text/event-stream',
      },
    }
  );
}

describe('@animaOS-SWARM/sdk daemon clients', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn();
    Object.defineProperty(globalThis, 'fetch', {
      value: fetchMock,
      configurable: true,
      writable: true,
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    if (originalFetchDescriptor) {
      Object.defineProperty(globalThis, 'fetch', originalFetchDescriptor);
    } else {
      Reflect.deleteProperty(globalThis, 'fetch');
    }
  });

  it('creates and runs an agent through the daemon', async () => {
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse(
          {
            agent: {
              state: {
                id: 'agent-1',
                name: 'researcher',
                status: 'idle',
              },
              messageCount: 0,
              eventCount: 1,
              lastTask: null,
            },
          },
          { status: 201 }
        )
      )
      .mockResolvedValueOnce(
        jsonResponse({
          agent: {
            state: {
              id: 'agent-1',
              name: 'researcher',
              status: 'completed',
            },
            messageCount: 2,
            eventCount: 8,
            lastTask: {
              status: 'success',
              data: {
                text: 'researched answer',
              },
              durationMs: 12,
            },
          },
          result: {
            status: 'success',
            data: {
              text: 'researched answer',
            },
            durationMs: 12,
          },
        })
      );

    const client = createDaemonClient({ baseUrl: 'http://daemon.test/' });
    const createdAgent = await client.agents.create(
      agent({
        name: 'researcher',
        model: 'gpt-5.4',
      })
    );
    const runResult = await client.agents.run('agent-1', {
      text: 'Find the answer',
    });

    expect(createdAgent).toMatchObject({
      state: {
        id: 'agent-1',
        name: 'researcher',
        status: 'idle',
      },
    });
    expect(runResult.agent).toMatchObject({
      state: {
        id: 'agent-1',
        status: 'completed',
      },
    });
    expect(runResult.result.data).toEqual({
      text: 'researched answer',
    });
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      'http://daemon.test/api/agents',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          name: 'researcher',
          model: 'gpt-5.4',
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      'http://daemon.test/api/agents/agent-1/run',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          text: 'Find the answer',
        }),
      })
    );
  });

  it('reads daemon health and manages memories through the daemon', async () => {
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse({
          status: 'ok',
        })
      )
      .mockResolvedValueOnce(
        jsonResponse(
          {
            id: 'memory-1',
            agentId: 'agent-1',
            agentName: 'researcher',
            type: 'fact',
            content: 'Daemon memory endpoint created',
            importance: 0.8,
            createdAt: 1712448000000,
            tags: ['daemon', 'memory'],
          },
          { status: 201 }
        )
      )
      .mockResolvedValueOnce(
        jsonResponse({
          results: [
            {
              id: 'memory-1',
              agentId: 'agent-1',
              agentName: 'researcher',
              type: 'fact',
              content: 'Daemon memory endpoint created',
              importance: 0.8,
              createdAt: 1712448000000,
              tags: ['daemon', 'memory'],
              score: 0.93,
            },
          ],
        })
      )
      .mockResolvedValueOnce(
        jsonResponse({
          memories: [
            {
              id: 'memory-2',
              agentId: 'agent-1',
              agentName: 'researcher',
              type: 'reflection',
              content: 'Most recent note',
              importance: 0.6,
              createdAt: 1712448001000,
              tags: ['recent'],
            },
          ],
        })
      );

    const client = createDaemonClient({ baseUrl: 'http://daemon.test/' });

    await expect(client.health()).resolves.toEqual({
      status: 'ok',
    });

    await expect(
      client.memories.create({
        agentId: 'agent-1',
        agentName: 'researcher',
        type: 'fact',
        content: 'Daemon memory endpoint created',
        importance: 0.8,
        tags: ['daemon', 'memory'],
      })
    ).resolves.toMatchObject({
      id: 'memory-1',
      agentId: 'agent-1',
      type: 'fact',
    });

    await expect(
      client.memories.search('daemon memory', {
        agentName: 'researcher',
        type: 'fact',
        limit: 5,
        minImportance: 0.5,
      })
    ).resolves.toEqual([
      expect.objectContaining({
        id: 'memory-1',
        score: 0.93,
      }),
    ]);

    await expect(
      client.memories.recent({
        agentId: 'agent-1',
        limit: 1,
      })
    ).resolves.toEqual([
      expect.objectContaining({
        id: 'memory-2',
        type: 'reflection',
      }),
    ]);

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      'http://daemon.test/health',
      expect.objectContaining({
        headers: expect.objectContaining({
          accept: 'application/json',
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      'http://daemon.test/api/memories',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          agentId: 'agent-1',
          agentName: 'researcher',
          type: 'fact',
          content: 'Daemon memory endpoint created',
          importance: 0.8,
          tags: ['daemon', 'memory'],
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      3,
      'http://daemon.test/api/memories/search?q=daemon+memory&agentName=researcher&type=fact&limit=5&minImportance=0.5',
      expect.objectContaining({
        headers: expect.objectContaining({
          accept: 'application/json',
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      4,
      'http://daemon.test/api/memories/recent?agentId=agent-1&limit=1',
      expect.objectContaining({
        headers: expect.objectContaining({
          accept: 'application/json',
        }),
      })
    );
  });

  it('manages memory entities, evaluations, and recall through the daemon', async () => {
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse(
          {
            kind: 'user',
            id: 'user-1',
            name: 'Leo',
            aliases: ['operator'],
            summary: 'Primary operator',
            createdAt: 1712448000000,
            updatedAt: 1712448000000,
          },
          { status: 201 }
        )
      )
      .mockResolvedValueOnce(
        jsonResponse({
          entities: [
            {
              kind: 'user',
              id: 'user-1',
              name: 'Leo',
              aliases: ['operator'],
              summary: 'Primary operator',
              createdAt: 1712448000000,
              updatedAt: 1712448000000,
            },
          ],
        })
      )
      .mockResolvedValueOnce(
        jsonResponse({
          decision: 'ignore',
          reason: 'memory is too short and below the importance threshold',
          score: 0.03,
          suggestedImportance: 0.05,
          duplicateMemoryId: null,
        })
      )
      .mockResolvedValueOnce(
        jsonResponse({
          evaluation: {
            decision: 'store',
            reason: 'memory contains distinct evidence',
            score: 0.74,
            suggestedImportance: 0.74,
            duplicateMemoryId: null,
          },
          memory: {
            id: 'memory-9',
            agentId: 'agent-1',
            agentName: 'researcher',
            type: 'fact',
            content: 'User prefers concise release notes',
            importance: 0.74,
            createdAt: 1712448002000,
            tags: ['preference'],
            scope: 'private',
          },
        })
      )
      .mockResolvedValueOnce(
        jsonResponse({
          results: [
            {
              memory: {
                id: 'memory-9',
                agentId: 'agent-1',
                agentName: 'researcher',
                type: 'fact',
                content: 'User prefers concise release notes',
                importance: 0.74,
                createdAt: 1712448002000,
                tags: ['preference'],
                scope: 'private',
              },
              score: 0.61,
              lexicalScore: 0,
              vectorScore: 0,
              relationshipScore: 0.9,
              recencyScore: 0,
              importanceScore: 0.74,
            },
          ],
        })
      )
      .mockResolvedValueOnce(
        jsonResponse({
          memory: {
            id: 'memory-9',
            agentId: 'agent-1',
            agentName: 'researcher',
            type: 'fact',
            content: 'User prefers concise release notes',
            importance: 0.74,
            createdAt: 1712448002000,
            tags: ['preference'],
            scope: 'private',
          },
          relationships: [
            {
              id: 'relationship-1',
              sourceKind: 'agent',
              sourceAgentId: 'agent-1',
              sourceAgentName: 'researcher',
              targetKind: 'user',
              targetAgentId: 'user-1',
              targetAgentName: 'Leo',
              relationshipType: 'responds_to',
              summary: 'researcher responded to Leo',
              strength: 0.8,
              confidence: 0.7,
              evidenceMemoryIds: ['memory-9'],
              tags: ['preference'],
              createdAt: 1712448003000,
              updatedAt: 1712448003000,
            },
          ],
          entities: [
            {
              kind: 'user',
              id: 'user-1',
              name: 'Leo',
              aliases: ['operator'],
              summary: 'Primary operator',
              createdAt: 1712448000000,
              updatedAt: 1712448000000,
            },
          ],
        })
      )
      .mockResolvedValueOnce(
        jsonResponse({
          decayedMemories: [
            {
              memoryId: 'memory-9',
              previousImportance: 0.74,
              newImportance: 0.37,
            },
          ],
          removedMemoryIds: ['memory-old'],
          removedRelationshipIds: ['relationship-old'],
        })
      )
      .mockResolvedValueOnce(
        jsonResponse({
          passed: true,
          embeddings: {
            enabled: true,
            provider: 'local',
            model: 'local-semantic-v1',
            dimension: 96,
            vectorCount: 3,
            persisted: true,
            storageFile: 'memory.sqlite',
          },
          evaluation: {
            passed: true,
            totalChecks: 14,
            passedChecks: 14,
            failureMessages: [],
            cases: [
              {
                name: 'relationship recall',
                checks: [
                  {
                    name: 'recall top 3',
                    passed: true,
                    detail: 'matched expected memory',
                  },
                ],
              },
            ],
          },
        })
      );

    const client = createDaemonClient({ baseUrl: 'http://daemon.test' });

    await expect(
      client.memories.createEntity({
        kind: 'user',
        id: 'user-1',
        name: 'Leo',
        aliases: ['operator'],
        summary: 'Primary operator',
      })
    ).resolves.toMatchObject({
      kind: 'user',
      id: 'user-1',
    });

    await expect(
      client.memories.entities({ kind: 'user', alias: 'operator', limit: 5 })
    ).resolves.toEqual([
      expect.objectContaining({
        id: 'user-1',
        aliases: ['operator'],
      }),
    ]);

    await expect(
      client.memories.evaluate({
        agentId: 'agent-1',
        agentName: 'researcher',
        type: 'fact',
        content: 'ok',
        importance: 0.05,
      })
    ).resolves.toMatchObject({
      decision: 'ignore',
    });

    await expect(
      client.memories.addEvaluated({
        agentId: 'agent-1',
        agentName: 'researcher',
        type: 'fact',
        content: 'User prefers concise release notes',
        importance: 0.4,
        tags: ['preference'],
        minContentChars: 8,
      })
    ).resolves.toMatchObject({
      evaluation: {
        decision: 'store',
      },
      memory: {
        id: 'memory-9',
      },
    });

    await expect(
      client.memories.recall('evidence probe', {
        entityId: 'user-1',
        agentId: 'agent-1',
        recentLimit: 0,
        limit: 3,
      })
    ).resolves.toEqual([
      expect.objectContaining({
        memory: expect.objectContaining({
          id: 'memory-9',
        }),
        relationshipScore: 0.9,
      }),
    ]);

    await expect(client.memories.trace('memory-9')).resolves.toMatchObject({
      memory: { id: 'memory-9' },
      relationships: [expect.objectContaining({ id: 'relationship-1' })],
      entities: [expect.objectContaining({ id: 'user-1' })],
    });

    await expect(
      client.memories.applyRetention({
        minImportance: 0.2,
        maxMemories: 100,
        decayHalfLifeMillis: 86_400_000,
      })
    ).resolves.toMatchObject({
      decayedMemories: [expect.objectContaining({ memoryId: 'memory-9' })],
      removedMemoryIds: ['memory-old'],
      removedRelationshipIds: ['relationship-old'],
    });

    await expect(client.memories.readiness()).resolves.toMatchObject({
      passed: true,
      embeddings: {
        enabled: true,
        provider: 'local',
        vectorCount: 3,
      },
      evaluation: {
        totalChecks: 14,
        passedChecks: 14,
      },
    });

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      'http://daemon.test/api/memories/entities',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          kind: 'user',
          id: 'user-1',
          name: 'Leo',
          aliases: ['operator'],
          summary: 'Primary operator',
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      'http://daemon.test/api/memories/entities?kind=user&alias=operator&limit=5',
      expect.any(Object)
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      3,
      'http://daemon.test/api/memories/evaluations',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          agentId: 'agent-1',
          agentName: 'researcher',
          type: 'fact',
          content: 'ok',
          importance: 0.05,
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      4,
      'http://daemon.test/api/memories/evaluated',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          agentId: 'agent-1',
          agentName: 'researcher',
          type: 'fact',
          content: 'User prefers concise release notes',
          importance: 0.4,
          tags: ['preference'],
          minContentChars: 8,
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      5,
      'http://daemon.test/api/memories/recall?q=evidence+probe&agentId=agent-1&limit=3&entityId=user-1&recentLimit=0',
      expect.any(Object)
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      6,
      'http://daemon.test/api/memories/memory-9/trace',
      expect.any(Object)
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      7,
      'http://daemon.test/api/memories/retention',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          minImportance: 0.2,
          maxMemories: 100,
          decayHalfLifeMillis: 86_400_000,
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      8,
      'http://daemon.test/api/memories/readiness',
      expect.any(Object)
    );
  });

  it('creates and runs a swarm through the daemon', async () => {
    fetchMock
      .mockResolvedValueOnce(
        jsonResponse(
          {
            swarm: {
              id: 'swarm-1',
              status: 'idle',
              agentIds: ['manager', 'worker-a'],
            },
          },
          { status: 201 }
        )
      )
      .mockResolvedValueOnce(
        jsonResponse({
          swarm: {
            id: 'swarm-1',
            status: 'idle',
            agentIds: ['manager', 'worker-a'],
          },
          result: {
            status: 'success',
            data: {
              text: '[manager]: coordinated',
            },
          },
        })
      );

    const client = createDaemonClient({ baseUrl: 'http://daemon.test' });
    const createdSwarm = await client.swarms.create(
      swarm({
        strategy: 'round-robin',
        manager: {
          name: 'manager',
          model: 'gpt-5.4',
        },
        workers: [
          {
            name: 'worker-a',
            model: 'gpt-5.4',
          },
        ],
        maxTurns: 2,
      })
    );
    const runResult = await client.swarms.run('swarm-1', {
      text: 'Coordinate the patch',
    });

    expect(createdSwarm).toMatchObject({
      id: 'swarm-1',
      status: 'idle',
      agentIds: ['manager', 'worker-a'],
    });
    expect(runResult.swarm).toMatchObject({
      id: 'swarm-1',
      status: 'idle',
    });
    expect(runResult.result.status).toBe('success');
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      'http://daemon.test/api/swarms',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          strategy: 'round-robin',
          manager: {
            name: 'manager',
            model: 'gpt-5.4',
          },
          workers: [
            {
              name: 'worker-a',
              model: 'gpt-5.4',
            },
          ],
          maxTurns: 2,
        }),
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      'http://daemon.test/api/swarms/swarm-1/run',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          text: 'Coordinate the patch',
        }),
      })
    );
  });

  it('wraps connection failures in a daemon-specific error', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('fetch failed'));

    const client = createDaemonClient({ baseUrl: 'http://daemon.test' });
    const healthRequest = client.health();

    await expect(healthRequest).rejects.toMatchObject({
      name: 'DaemonConnectionError',
      message: 'Failed to reach daemon at http://daemon.test/health',
      cause: expect.any(TypeError),
    });
    await expect(healthRequest).rejects.toBeInstanceOf(DaemonConnectionError);
  });

  it('surfaces daemon http errors with parsed response bodies', async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(
        {
          error: 'q query parameter is required',
        },
        { status: 400 }
      )
    );

    const client = createDaemonClient({ baseUrl: 'http://daemon.test' });
    const searchRequest = client.memories.search('');

    await expect(searchRequest).rejects.toMatchObject({
      name: 'DaemonHttpError',
      status: 400,
      body: {
        error: 'q query parameter is required',
      },
      message: 'q query parameter is required',
    });
    await expect(searchRequest).rejects.toBeInstanceOf(DaemonHttpError);
  });

  it('subscribes to swarm events over SSE', async () => {
    fetchMock.mockResolvedValueOnce(
      sseResponse([
        'event: swarm:running\ndata: {"swarmId":"swarm-1","state":{"status":"running"},"result":null}\n\n',
        'event: tool:after\ndata: {"agentId":"agent-1","agentName":"manager","toolName":"memory_search","status":"success","durationMs":12,"result":"Found prior note"}\n\n',
        'event: swarm:completed\ndata: {"swarmId":"swarm-1","state":{"status":"idle"},"result":{"status":"success"}}\n\n',
      ])
    );

    const client = createDaemonClient({ baseUrl: 'http://daemon.test' });
    const received = [];

    for await (const event of client.swarms.subscribe('swarm-1')) {
      received.push(event);
      if (received.length === 3) {
        break;
      }
    }

    expect(fetchMock).toHaveBeenCalledWith(
      'http://daemon.test/api/swarms/swarm-1/events',
      expect.objectContaining({
        method: 'GET',
        headers: expect.objectContaining({
          accept: 'text/event-stream',
        }),
      })
    );
    expect(received).toEqual([
      {
        event: 'swarm:running',
        data: {
          swarmId: 'swarm-1',
          state: {
            status: 'running',
          },
          result: null,
        },
      },
      {
        event: 'tool:after',
        data: {
          agentId: 'agent-1',
          agentName: 'manager',
          toolName: 'memory_search',
          status: 'success',
          durationMs: 12,
          result: 'Found prior note',
        },
      },
      {
        event: 'swarm:completed',
        data: {
          swarmId: 'swarm-1',
          state: {
            status: 'idle',
          },
          result: {
            status: 'success',
          },
        },
      },
    ]);
  });

  it('tears down swarm SSE subscriptions when iteration stops early', async () => {
    const cancelSpy = vi.fn();
    let requestSignal: AbortSignal | undefined;

    fetchMock.mockImplementationOnce((_url: string, init?: RequestInit) => {
      requestSignal = init?.signal as AbortSignal | undefined;

      return Promise.resolve(
        new Response(
          new ReadableStream({
            start(controller) {
              controller.enqueue(
                new TextEncoder().encode(
                  'event: swarm:running\ndata: {"swarmId":"swarm-1"}\n\n'
                )
              );
            },
            cancel(reason) {
              cancelSpy(reason);
            },
          }),
          {
            headers: {
              'content-type': 'text/event-stream',
            },
          }
        )
      );
    });

    const client = createDaemonClient({ baseUrl: 'http://daemon.test' });

    for await (const event of client.swarms.subscribe('swarm-1')) {
      expect(event).toEqual({
        event: 'swarm:running',
        data: {
          swarmId: 'swarm-1',
        },
      });
      break;
    }

    expect(cancelSpy).toHaveBeenCalledOnce();
    expect(requestSignal?.aborted).toBe(true);
  });
});
