import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { executeLaunchCommand } from './launch.ts';

const DEFAULT_AGENCY_YAML = [
  'name: Launch Test Agency',
  'description: daemon launch fixture',
  'model: gpt-5.4',
  'provider: openai',
  'strategy: round-robin',
  'orchestrator:',
  '  name: manager',
  '  bio: Coordinate work',
  '  system: Orchestrate the workers',
  'agents:',
  '  - name: worker-a',
  '    bio: Execute tasks',
  '    system: Complete the assigned work',
].join('\n');

async function* emptySubscription() {}

function createDaemonTuiHarness() {
  let element:
    | {
        component: unknown;
        props: Record<string, unknown>;
      }
    | undefined;

  const render = vi.fn(() => ({
    unmount: vi.fn(),
  }));

  return {
    deps: {
      createDaemonTuiRuntime: async () => ({
        eventBus: {
          on() {
            return () => undefined;
          },
          async emit() {},
          clear() {},
        },
        render,
        createElement(component: unknown, props: Record<string, unknown>) {
          element = { component, props };
          return element;
        },
        App: Symbol('LaunchApp'),
      }),
    },
    getElement() {
      return element;
    },
    render,
  };
}

function createAgencyDir(agencyYaml = DEFAULT_AGENCY_YAML): string {
  const dir = mkdtempSync(join(tmpdir(), 'animaos-launch-'));
  writeFileSync(join(dir, 'anima.yaml'), agencyYaml);
  return dir;
}

describe('launch command daemon plain-text mode', () => {
  let agencyDir: string;

  beforeEach(() => {
    agencyDir = createAgencyDir();
    process.exitCode = undefined;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    rmSync(agencyDir, { recursive: true, force: true });
    process.exitCode = undefined;
  });

  it('creates and runs a daemon swarm for single-shot plain-text launch', async () => {
    const client = {
      swarms: {
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'daemon launch result' },
            durationMs: 12,
          },
        }),
      },
    };

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await executeLaunchCommand(
      'Ship the patch',
      {
        dir: agencyDir,
        tui: false,
      },
      { client }
    );

    expect(client.swarms.create).toHaveBeenCalledWith(
      expect.objectContaining({
        strategy: 'round-robin',
        manager: expect.objectContaining({
          name: 'manager',
          model: 'gpt-5.4',
          plugins: [expect.objectContaining({ name: 'memory' })],
          provider: 'openai',
          system: 'Orchestrate the workers',
          tools: expect.arrayContaining([
            expect.objectContaining({ name: 'memory_search' }),
            expect.objectContaining({ name: 'recent_memories' }),
          ]),
        }),
        workers: expect.arrayContaining([
          expect.objectContaining({
            name: 'worker-a',
            model: 'gpt-5.4',
            provider: 'openai',
            system: 'Complete the assigned work',
          }),
        ]),
      })
    );
    expect(client.swarms.run).toHaveBeenCalledWith('swarm-1', {
      text: 'Ship the patch',
    });
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('maps supported launch tools into daemon tool descriptors', async () => {
    rmSync(agencyDir, { recursive: true, force: true });
    agencyDir = createAgencyDir(
      [
        'name: Launch Test Agency',
        'description: daemon launch fixture',
        'model: gpt-5.4',
        'provider: openai',
        'strategy: round-robin',
        'orchestrator:',
        '  name: manager',
        '  bio: Coordinate work',
        '  system: Orchestrate the workers',
        '  tools:',
        '    - memory_recent',
        '    - memory_add',
        'agents:',
        '  - name: worker-a',
        '    bio: Execute tasks',
        '    system: Complete the assigned work',
      ].join('\n')
    );

    const client = {
      swarms: {
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'daemon launch result' },
            durationMs: 12,
          },
        }),
      },
    };

    await executeLaunchCommand(
      'Ship the patch',
      {
        dir: agencyDir,
        tui: false,
      },
      { client }
    );

    expect(client.swarms.create).toHaveBeenCalledWith(
      expect.objectContaining({
        manager: expect.objectContaining({
          tools: expect.arrayContaining([
            expect.objectContaining({ name: 'memory_search' }),
            expect.objectContaining({ name: 'recent_memories' }),
            expect.objectContaining({ name: 'memory_add' }),
          ]),
        }),
      })
    );
  });

  it('fails fast when launch requests unsupported daemon tools', async () => {
    rmSync(agencyDir, { recursive: true, force: true });
    agencyDir = createAgencyDir(
      [
        'name: Launch Test Agency',
        'description: daemon launch fixture',
        'model: gpt-5.4',
        'provider: openai',
        'strategy: round-robin',
        'orchestrator:',
        '  name: manager',
        '  bio: Coordinate work',
        '  system: Orchestrate the workers',
        '  tools:',
        '    - bash',
        'agents:',
        '  - name: worker-a',
        '    bio: Execute tasks',
        '    system: Complete the assigned work',
      ].join('\n')
    );

    const client = {
      swarms: {
        create: vi.fn(),
        run: vi.fn(),
      },
    };

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await executeLaunchCommand(
      'Ship the patch',
      {
        dir: agencyDir,
        tui: false,
      },
      { client }
    );

    expect(client.swarms.create).not.toHaveBeenCalled();
    expect(errorSpy).toHaveBeenCalledWith(
      'Error:',
      'daemon-backed launch does not support tool(s) for agent "manager": bash. Launch now runs only through the Rust daemon; remove those tools from anima.yaml or implement them in the daemon tool registry.'
    );
    expect(process.exitCode).toBe(1);
  });

  it('reuses one daemon swarm across plain-text interactive tasks', async () => {
    const client = {
      swarms: {
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'interactive daemon result' },
            durationMs: 8,
          },
        }),
      },
    };
    const inputs = ['First task', 'exit'];
    const readline = {
      question: vi.fn((_: string, callback: (input: string) => void) => {
        callback(inputs.shift() ?? 'exit');
      }),
      close: vi.fn(),
    };

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: false,
      },
      {
        client,
        createReadline: () => readline,
      }
    );

    expect(client.swarms.create).toHaveBeenCalledOnce();
    expect(client.swarms.run).toHaveBeenCalledOnce();
    expect(client.swarms.run).toHaveBeenCalledWith('swarm-1', {
      text: 'First task',
    });
    expect(readline.question).toHaveBeenCalledTimes(2);
    expect(readline.close).toHaveBeenCalledOnce();
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('rejects api-key overrides for daemon-backed plain-text launch', async () => {
    const client = {
      swarms: {
        create: vi.fn(),
        run: vi.fn(),
      },
    };

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await executeLaunchCommand(
      'Refuse override',
      {
        dir: agencyDir,
        tui: false,
        apiKey: 'secret',
      },
      { client }
    );

    expect(client.swarms.create).not.toHaveBeenCalled();
    expect(client.swarms.run).not.toHaveBeenCalled();
    expect(errorSpy).toHaveBeenCalledWith(
      'Error:',
      '--api-key is not supported by daemon-backed launch in plain-text mode. Configure credentials in the daemon environment.'
    );
    expect(process.exitCode).toBe(1);
  });

  it('reuses one daemon swarm across interactive TUI tasks', async () => {
    const harness = createDaemonTuiHarness();
    const client = {
      swarms: {
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          swarm: {
            id: 'swarm-1',
            tokenUsage: {
              promptTokens: 0,
              completionTokens: 0,
              totalTokens: 0,
            },
          },
          result: {
            status: 'success',
            data: { text: 'interactive daemon result' },
            durationMs: 8,
          },
        }),
        subscribe: vi.fn().mockImplementation(emptySubscription),
      },
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as {
      props: { onTask: (input: string) => Promise<{ status: string }> };
    };

    await expect(element.props.onTask('First task')).resolves.toEqual(
      expect.objectContaining({ status: 'success' })
    );
    await expect(element.props.onTask('Second task')).resolves.toEqual(
      expect.objectContaining({ status: 'success' })
    );

    expect(client.swarms.create).toHaveBeenCalledOnce();
    expect(client.swarms.run).toHaveBeenNthCalledWith(1, 'swarm-1', {
      text: 'First task',
    });
    expect(client.swarms.run).toHaveBeenNthCalledWith(2, 'swarm-1', {
      text: 'Second task',
    });
    expect(harness.render).toHaveBeenCalledOnce();
  });

  it('returns a task error instead of throwing when daemon TUI swarm creation fails', async () => {
    const harness = createDaemonTuiHarness();
    const client = {
      swarms: {
        create: vi.fn().mockRejectedValue(new Error('daemon unavailable')),
        run: vi.fn(),
        subscribe: vi.fn(),
      },
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as {
      props: {
        onTask: (input: string) => Promise<{ status: string; error?: string }>;
      };
    };

    await expect(element.props.onTask('First task')).resolves.toEqual({
      status: 'error',
      error: 'daemon unavailable',
      durationMs: 0,
    });
    expect(client.swarms.run).not.toHaveBeenCalled();
  });
});
