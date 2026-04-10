import type { Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterEach, describe, expect, it } from 'vitest';
import { createServer } from './server.js';

describe('server app boundary', () => {
  let server: Server | null = null;
  const originalModelAdapter = process.env.ANIMA_MODEL_ADAPTER;

  afterEach(async () => {
    if (typeof originalModelAdapter === 'undefined') {
      delete process.env.ANIMA_MODEL_ADAPTER;
    } else {
      process.env.ANIMA_MODEL_ADAPTER = originalModelAdapter;
    }

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

  async function listen(): Promise<string> {
    server = createServer();

    await new Promise<void>((resolve, reject) => {
      server?.listen(0, '127.0.0.1', () => resolve());
      server?.once('error', reject);
    });

    const address = server.address();
    if (!address || typeof address === 'string') {
      throw new Error('failed to bind test server');
    }

    return `http://127.0.0.1:${String((address as AddressInfo).port)}`;
  }

  it('serves health from the app root boundary', async () => {
    const baseUrl = await listen();
    const response = await fetch(`${baseUrl}/api/health`);

    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toContain('application/json');
    expect(response.headers.get('access-control-allow-origin')).toBe('*');
    await expect(response.json()).resolves.toMatchObject({
      status: 'ok',
      agents: 0,
      swarms: 0,
      uptime: expect.any(Number),
    });
  });

  it('returns a JSON 404 for unknown routes', async () => {
    const baseUrl = await listen();
    const response = await fetch(`${baseUrl}/api/missing`);

    expect(response.status).toBe(404);
    await expect(response.json()).resolves.toEqual({ error: 'Not found' });
  });

  it('rejects invalid agent create payloads at the HTTP boundary', async () => {
    const baseUrl = await listen();
    const response = await fetch(`${baseUrl}/api/agents`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({ name: 'manager' }),
    });

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({
      error: 'name and model are required',
    });
  });

  it('rejects invalid swarm create payloads at the HTTP boundary', async () => {
    const baseUrl = await listen();
    const response = await fetch(`${baseUrl}/api/swarms`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({ strategy: 'supervisor' }),
    });

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({
      error: 'strategy, manager, and workers are required',
    });
  });

  it('runs agent tasks with the mock model adapter when configured', async () => {
    process.env.ANIMA_MODEL_ADAPTER = 'mock';
    const baseUrl = await listen();

    const createResponse = await fetch(`${baseUrl}/api/agents`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        name: 'observer',
        model: 'gpt-5.4',
      }),
    });

    expect(createResponse.status).toBe(201);
    const created = (await createResponse.json()) as { id: string };

    const runResponse = await fetch(`${baseUrl}/api/agents/${created.id}/run`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        task: 'Summarize the launch posture.',
      }),
    });

    expect(runResponse.status).toBe(200);
    await expect(runResponse.json()).resolves.toEqual({
      status: 'success',
      data: {
        text: 'Mock completion for: Summarize the launch posture.',
      },
      durationMs: expect.any(Number),
    });
  });

  it('runs swarm tasks with the mock model adapter when configured', async () => {
    process.env.ANIMA_MODEL_ADAPTER = 'mock';
    const baseUrl = await listen();

    const createResponse = await fetch(`${baseUrl}/api/swarms`, {
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

    expect(createResponse.status).toBe(201);
    const created = (await createResponse.json()) as { id: string };

    const runResponse = await fetch(`${baseUrl}/api/swarms/${created.id}/run`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        task: 'Summarize the swarm launch posture.',
      }),
    });

    expect(runResponse.status).toBe(200);
    await expect(runResponse.json()).resolves.toEqual({
      status: 'success',
      data: {
        text: 'Mock completion for: Summarize the swarm launch posture.',
      },
      durationMs: expect.any(Number),
    });
  });
});
