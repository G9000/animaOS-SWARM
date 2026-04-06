import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { agent, createDaemonClient, swarm } from './index.js';

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
