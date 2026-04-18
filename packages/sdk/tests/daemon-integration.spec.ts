import { spawn, type ChildProcess } from 'node:child_process';
import {
  createServer as createHttpServer,
  type IncomingMessage,
} from 'node:http';
import { createServer } from 'node:net';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { agent, createDaemonClient, swarm } from '../src/index.js';

const workspaceRoot = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '../../..'
);
const daemonManifestPath = resolve(
  workspaceRoot,
  'Cargo.toml'
);

describe.sequential('@animaOS-SWARM/sdk real daemon integration', () => {
  let daemonProcess: ChildProcess | null = null;
  let daemonOutput = '';
  let client = createDaemonClient();

  beforeAll(async () => {
    const port = await reservePort();
    const baseUrl = `http://127.0.0.1:${String(port)}`;

    const spawnedProcess = spawn(
      'cargo',
      ['run', '--manifest-path', daemonManifestPath, '-p', 'anima-daemon'],
      {
        cwd: workspaceRoot,
        env: {
          ...process.env,
          ANIMAOS_RS_HOST: '127.0.0.1',
          ANIMAOS_RS_PORT: String(port),
        },
        stdio: ['ignore', 'pipe', 'pipe'],
      }
    );
    daemonProcess = spawnedProcess;

    spawnedProcess.stdout?.on('data', (chunk: Buffer | string) => {
      daemonOutput += chunk.toString();
    });
    spawnedProcess.stderr?.on('data', (chunk: Buffer | string) => {
      daemonOutput += chunk.toString();
    });

    client = createDaemonClient({ baseUrl });
    await waitForDaemonHealthy(client, spawnedProcess, () => daemonOutput);
  }, 90000);

  afterAll(async () => {
    await stopDaemonProcess(daemonProcess);
    daemonProcess = null;
  });

  it('exercises health, memories, and agent reads against the Rust daemon', async () => {
    await expect(client.health()).resolves.toEqual({
      status: 'ok',
    });

    const agentName = `sdk-e2e-agent-${Date.now().toString(36)}`;
    const createdAgent = await client.agents.create(
      agent({
        name: agentName,
        model: 'gpt-5.4',
      })
    );

    expect(createdAgent.state.name).toBe(agentName);

    const agents = await client.agents.list();
    expect(
      agents.some((entry) => entry.state.id === createdAgent.state.id)
    ).toBe(true);

    await expect(
      client.agents.get(createdAgent.state.id)
    ).resolves.toMatchObject({
      state: {
        id: createdAgent.state.id,
        name: agentName,
      },
    });

    const memoryToken = `sdk-e2e-${Date.now().toString(36)}`;
    const createdMemory = await client.memories.create({
      agentId: createdAgent.state.id,
      agentName,
      type: 'fact',
      content: `daemon integration memory ${memoryToken}`,
      importance: 0.8,
      tags: ['sdk', 'integration'],
    });

    expect(createdMemory.agentId).toBe(createdAgent.state.id);
    expect(createdMemory.tags).toEqual(['sdk', 'integration']);

    await expect(
      client.memories.search(memoryToken, {
        agentId: createdAgent.state.id,
        limit: 5,
      })
    ).resolves.toEqual([
      expect.objectContaining({
        id: createdMemory.id,
        agentName,
        type: 'fact',
      }),
    ]);

    await expect(
      client.memories.recent({
        agentId: createdAgent.state.id,
        limit: 1,
      })
    ).resolves.toEqual([
      expect.objectContaining({
        id: createdMemory.id,
        content: `daemon integration memory ${memoryToken}`,
      }),
    ]);

    await expect(
      client.agents.recentMemories(createdAgent.state.id, {
        limit: 5,
      })
    ).resolves.toEqual([
      expect.objectContaining({
        id: createdMemory.id,
        agentId: createdAgent.state.id,
        agentName,
      }),
    ]);
  }, 90000);

  it('runs a swarm and streams lifecycle events through the live daemon', async () => {
    const modelStub = await startOpenAiCompatibleStub();

    try {
      const providerSettings = {
        apiKey: 'sdk-integration-key',
        baseUrl: `${modelStub.baseUrl}/v1`,
      };

      const createdSwarm = await client.swarms.create(
        swarm({
          strategy: 'round-robin',
          manager: {
            name: `sdk-e2e-manager-${Date.now().toString(36)}`,
            model: 'gpt-5.4',
            provider: 'openai',
            settings: providerSettings,
          },
          workers: [
            {
              name: `sdk-e2e-worker-${Date.now().toString(36)}`,
              model: 'gpt-5.4',
              provider: 'openai',
              settings: providerSettings,
            },
          ],
          maxTurns: 2,
        })
      );

      expect(createdSwarm.status).toBe('idle');

      const eventsPromise = collectSwarmEvents(
        client.swarms.subscribe(createdSwarm.id),
        'swarm:completed'
      );

      const runResult = await client.swarms.run(createdSwarm.id, {
        text: 'Coordinate a deterministic integration test',
      });
      const events = await eventsPromise;

      expect(runResult.swarm.id).toBe(createdSwarm.id);
      expect(runResult.result.status).toBe('success');
      expect(runResult.result.data).toMatchObject({
        text: expect.stringContaining('stubbed'),
      });
      expect(modelStub.requests.length).toBeGreaterThanOrEqual(2);

      const eventNames = events.map((event) => event.event);
      expect(eventNames).toContain('swarm:running');
      expect(eventNames).toContain('task:started');
      expect(eventNames).toContain('agent:tokens');
      expect(eventNames).toContain('swarm:completed');

      expect(
        events.some(
          (event) =>
            event.event === 'swarm:running' &&
            typeof event.data === 'object' &&
            event.data !== null &&
            'swarmId' in event.data &&
            event.data.swarmId === createdSwarm.id
        )
      ).toBe(true);

      expect(
        events.some(
          (event) =>
            event.event === 'agent:tokens' &&
            typeof event.data === 'object' &&
            event.data !== null &&
            'agentName' in event.data &&
            typeof event.data.agentName === 'string' &&
            event.data.agentName.includes('sdk-e2e-')
        )
      ).toBe(true);
    } finally {
      await modelStub.close();
    }
  }, 90000);

  it('returns task-level execution errors when runtime provider credentials are missing', async () => {
    const createdAgent = await client.agents.create(
      agent({
        name: `sdk-e2e-missing-key-${Date.now().toString(36)}`,
        model: 'gpt-5.4',
        provider: 'openai',
      })
    );

    const runResult = await client.agents.run(createdAgent.state.id, {
      text: 'Fail because there is no OpenAI key configured',
    });

    expect(runResult.result.status).toBe('error');
    expect(runResult.result.error).toContain('OPENAI_API_KEY');
    expect(runResult.agent.lastTask).toMatchObject({
      status: 'error',
      error: expect.stringContaining('OPENAI_API_KEY'),
    });

    await expect(
      client.agents.get(createdAgent.state.id)
    ).resolves.toMatchObject({
      state: {
        id: createdAgent.state.id,
        name: createdAgent.state.name,
      },
      lastTask: {
        status: 'error',
        error: expect.stringContaining('OPENAI_API_KEY'),
      },
    });
  }, 90000);
});

async function reservePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();

    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') {
        server.close(() => reject(new Error('failed to reserve test port')));
        return;
      }

      const { port } = address;
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }

        resolvePort(port);
      });
    });
  });
}

async function waitForDaemonHealthy(
  client: ReturnType<typeof createDaemonClient>,
  daemonProcess: ChildProcess,
  getOutput: () => string
): Promise<void> {
  const startedAt = Date.now();

  while (Date.now() - startedAt < 45000) {
    if (daemonProcess.exitCode !== null) {
      throw new Error(
        `daemon exited before becoming healthy (code ${String(
          daemonProcess.exitCode
        )}).\n${getOutput()}`
      );
    }

    try {
      const health = await client.health();
      if (health.status === 'ok') {
        return;
      }
    } catch {
      // The server may still be compiling or starting up.
    }

    await delay(250);
  }

  throw new Error(`timed out waiting for daemon health.\n${getOutput()}`);
}

async function stopDaemonProcess(
  daemonProcess: ChildProcess | null
): Promise<void> {
  if (!daemonProcess || daemonProcess.exitCode !== null) {
    return;
  }

  daemonProcess.kill();

  await new Promise<void>((resolveStop) => {
    const timeout = setTimeout(() => {
      daemonProcess.kill('SIGKILL');
      resolveStop();
    }, 5000);

    daemonProcess.once('exit', () => {
      clearTimeout(timeout);
      resolveStop();
    });
  });
}

function delay(ms: number): Promise<void> {
  return new Promise((resolveDelay) => {
    setTimeout(resolveDelay, ms);
  });
}

async function collectSwarmEvents(
  stream: AsyncGenerator<{ event: string; data: unknown }>,
  stopEventName: string
): Promise<Array<{ event: string; data: unknown }>> {
  const events: Array<{ event: string; data: unknown }> = [];

  for await (const event of stream) {
    events.push(event);
    if (event.event === stopEventName) {
      break;
    }
  }

  return events;
}

async function startOpenAiCompatibleStub(): Promise<{
  baseUrl: string;
  requests: Array<Record<string, unknown>>;
  close: () => Promise<void>;
}> {
  const requests: Array<Record<string, unknown>> = [];
  const server = createHttpServer(async (request, response) => {
    if (request.method !== 'POST' || request.url !== '/v1/chat/completions') {
      response.statusCode = 404;
      response.end('not found');
      return;
    }

    const rawBody = await readIncomingMessage(request);
    const body = JSON.parse(rawBody) as Record<string, unknown>;
    requests.push(body);

    const messages = Array.isArray(body.messages)
      ? (body.messages as Array<Record<string, unknown>>)
      : [];
    const lastUserMessage = [...messages]
      .reverse()
      .find((message) => message.role === 'user');
    const userContent =
      typeof lastUserMessage?.content === 'string'
        ? lastUserMessage.content
        : 'no-user-content';

    response.setHeader('content-type', 'application/json');
    response.end(
      JSON.stringify({
        choices: [
          {
            message: {
              content: `stubbed: ${userContent}`,
            },
            finish_reason: 'stop',
          },
        ],
        usage: {
          prompt_tokens: 10,
          completion_tokens: 4,
          total_tokens: 14,
        },
      })
    );
  });

  const port = await new Promise<number>((resolvePort, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') {
        reject(new Error('failed to bind model stub server'));
        return;
      }

      resolvePort(address.port);
    });
  });

  return {
    baseUrl: `http://127.0.0.1:${String(port)}`,
    requests,
    close: () =>
      new Promise<void>((resolveClose, reject) => {
        server.close((error) => {
          if (error) {
            reject(error);
            return;
          }

          resolveClose();
        });
      }),
  };
}

async function readIncomingMessage(request: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];

  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }

  return Buffer.concat(chunks).toString('utf8');
}
