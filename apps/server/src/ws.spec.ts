import type { Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocket } from 'ws';
import { createServer } from './server.js';

interface BroadcastEvent {
  type: string;
  agentId?: string;
  timestamp: number;
  data: Record<string, unknown>;
}

describe('server websocket bridge', () => {
  let server: Server | null = null;
  const sockets = new Set<WebSocket>();
  const originalOpenAiApiKey = process.env.OPENAI_API_KEY;

  beforeEach(() => {
    process.env.OPENAI_API_KEY = 'test-key';
  });

  afterEach(async () => {
    if (typeof originalOpenAiApiKey === 'undefined') {
      delete process.env.OPENAI_API_KEY;
    } else {
      process.env.OPENAI_API_KEY = originalOpenAiApiKey;
    }

    for (const socket of sockets) {
      socket.close();
    }
    sockets.clear();

    if (!server) {
      return;
    }

    await new Promise<void>((resolve, reject) => {
      server?.close((error) => {
        if (error) {
          reject(error);
          return;
        }

        resolve();
      });
    });
    server = null;
  });

  async function listen(): Promise<{ httpBaseUrl: string; wsUrl: string }> {
    server = createServer();

    await new Promise<void>((resolve, reject) => {
      server?.listen(0, '127.0.0.1', () => resolve());
      server?.once('error', reject);
    });

    const address = server.address();
    if (!address || typeof address === 'string') {
      throw new Error('failed to bind test server');
    }

    const port = String((address as AddressInfo).port);
    return {
      httpBaseUrl: `http://127.0.0.1:${port}`,
      wsUrl: `ws://127.0.0.1:${port}/ws`,
    };
  }

  async function connectWebSocket(url: string): Promise<WebSocket> {
    const socket = new WebSocket(url);
    sockets.add(socket);

    await new Promise<void>((resolve, reject) => {
      socket.once('open', () => resolve());
      socket.once('error', reject);
    });

    return socket;
  }

  async function waitForEvent(
    socket: WebSocket,
    eventType: string
  ): Promise<BroadcastEvent> {
    return new Promise<BroadcastEvent>((resolve, reject) => {
      const timeout = setTimeout(() => {
        cleanup();
        reject(new Error(`timed out waiting for websocket event ${eventType}`));
      }, 5000);

      const cleanup = () => {
        clearTimeout(timeout);
        socket.off('message', handleMessage);
      };

      const handleMessage = (rawPayload: Buffer) => {
        const parsed = JSON.parse(rawPayload.toString()) as BroadcastEvent;
        if (parsed.type !== eventType) {
          return;
        }

        cleanup();
        resolve(parsed);
      };

      socket.on('message', handleMessage);
    });
  }

  it('broadcasts agent spawn events to websocket clients', async () => {
    const { httpBaseUrl, wsUrl } = await listen();
    const socket = await connectWebSocket(wsUrl);
    const spawnEventPromise = waitForEvent(socket, 'agent:spawned');

    const response = await fetch(`${httpBaseUrl}/api/agents`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        name: 'observer',
        model: 'gpt-5.4',
      }),
    });

    expect(response.status).toBe(201);
    await expect(spawnEventPromise).resolves.toMatchObject({
      type: 'agent:spawned',
      data: {
        name: 'observer',
      },
      agentId: expect.any(String),
      timestamp: expect.any(Number),
    });
  });

  it('broadcasts swarm creation events to websocket clients', async () => {
    const { httpBaseUrl, wsUrl } = await listen();
    const socket = await connectWebSocket(wsUrl);
    const swarmEventPromise = waitForEvent(socket, 'swarm:created');

    const response = await fetch(`${httpBaseUrl}/api/swarms`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        strategy: 'supervisor',
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
      }),
    });

    expect(response.status).toBe(201);
    await expect(swarmEventPromise).resolves.toMatchObject({
      type: 'swarm:created',
      data: {
        strategy: 'supervisor',
        swarmId: expect.any(String),
      },
      timestamp: expect.any(Number),
    });
  });
});
