import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { executeLaunchCommand } from './launch.js';
import { relayLaunchSwarmEvent } from './launch-events.js';
import { LAUNCH_HISTORY_FILENAME } from './launch-history.js';

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

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function mockSwarms(overrides: Record<string, any> = {}): any {
  return {
    create: vi.fn(),
    run: vi.fn(),
    get: vi.fn(),
    subscribe: vi.fn(),
    ...overrides,
  };
}

function createDaemonTuiHarness() {
  let element:
    | {
        component: unknown;
        props: Record<string, unknown>;
      }
    | undefined;
  const events: Array<{ type: string; data: unknown; agentId?: string }> = [];

  const unmount = vi.fn();
  const waitUntilExit = vi.fn().mockResolvedValue(undefined);

  const render = vi.fn(() => ({
    unmount,
    waitUntilExit,
  }));

  return {
    deps: {
      createDaemonTuiRuntime: async () => ({
        eventBus: {
          on() {
            return () => undefined;
          },
          async emit(type: string, data: unknown, agentId?: string) {
            events.push({ type, data, agentId });
          },
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
    events,
    render,
    unmount,
    waitUntilExit,
  };
}

function createAgencyDir(agencyYaml = DEFAULT_AGENCY_YAML): string {
  const dir = mkdtempSync(join(tmpdir(), 'animaos-launch-'));
  writeFileSync(join(dir, 'anima.yaml'), agencyYaml);
  return dir;
}

function readHistory(dir: string) {
  return JSON.parse(
    readFileSync(join(dir, LAUNCH_HISTORY_FILENAME), 'utf-8')
  ) as Array<{
    task: string;
    result: string;
    isError: boolean;
  }>;
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
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'daemon launch result' },
            durationMs: 12,
          },
        }),
      }),
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
    expect(readHistory(agencyDir)).toEqual([
      expect.objectContaining({
        task: 'Ship the patch',
        result: 'daemon launch result',
        isError: false,
      }),
    ]);
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('prints daemon warning and recovery messages for single-shot plain-text launch', async () => {
    const client = {
      health: vi.fn().mockRejectedValue(new Error('daemon unavailable')),
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'daemon launch result' },
            durationMs: 12,
          },
        }),
      }),
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

    expect(errorSpy).toHaveBeenCalledWith(
      'Warning:',
      'daemon unavailable. Launch tasks will fail until the daemon is reachable.'
    );
    expect(logSpy).toHaveBeenCalledWith(
      'Daemon reachable again. Launch tasks can run.'
    );
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
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'daemon launch result' },
            durationMs: 12,
          },
        }),
      }),
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

    const createdSwarm = client.swarms.create.mock.calls[0]?.[0];
    const managerTools = createdSwarm?.manager?.tools ?? [];
    const memorySearch = managerTools.find(
      (tool: { name: string }) => tool.name === 'memory_search'
    );
    const memoryAdd = managerTools.find(
      (tool: { name: string }) => tool.name === 'memory_add'
    );

    expect(memorySearch?.parameters).toEqual({
      type: 'object',
      properties: {
        query: { type: 'string' },
        limit: { type: 'number' },
      },
      required: ['query'],
    });
    expect(memoryAdd?.parameters).toEqual({
      type: 'object',
      properties: {
        content: { type: 'string' },
        type: { type: 'string' },
        importance: { type: 'number' },
      },
      required: ['content'],
    });
  });

  it('passes maxParallelDelegations through to the daemon swarm config', async () => {
    rmSync(agencyDir, { recursive: true, force: true });
    agencyDir = createAgencyDir(
      [
        'name: Launch Test Agency',
        'description: daemon launch fixture',
        'model: gpt-5.4',
        'provider: openai',
        'strategy: supervisor',
        'maxParallelDelegations: 2',
        'orchestrator:',
        '  name: manager',
        '  bio: Coordinate work',
        '  system: Orchestrate the workers',
        'agents:',
        '  - name: worker-a',
        '    bio: Execute tasks',
        '    system: Complete the assigned work',
      ].join('\n')
    );

    const client = {
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'daemon launch result' },
            durationMs: 12,
          },
        }),
      }),
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
        strategy: 'supervisor',
        maxParallelDelegations: 2,
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
      swarms: mockSwarms(),
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
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'interactive daemon result' },
            durationMs: 8,
          },
        }),
      }),
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
        createReadline: () => readline as any,
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

  it('prints daemon warning and recovery messages across interactive plain-text tasks', async () => {
    const client = {
      health: vi.fn().mockRejectedValue(new Error('daemon unavailable')),
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'interactive daemon result' },
            durationMs: 8,
          },
        }),
      }),
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
        createReadline: () => readline as any,
      }
    );

    expect(errorSpy).toHaveBeenCalledWith(
      'Warning:',
      'daemon unavailable. Launch tasks will fail until the daemon is reachable.'
    );
    expect(errorSpy.mock.calls).not.toContainEqual([
      'Error:',
      expect.any(String),
    ]);
    expect(logSpy).toHaveBeenCalledWith(
      'Daemon reachable again. Launch tasks can run.'
    );
  });

  it('reports daemon recovery on demand with /health in interactive plain-text launch', async () => {
    const client = {
      health: vi
        .fn()
        .mockRejectedValueOnce(new Error('daemon unavailable'))
        .mockResolvedValueOnce(undefined),
      swarms: mockSwarms(),
    };
    const inputs = ['/health', 'exit'];
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
        createReadline: () => readline as any,
      }
    );

    expect(errorSpy).toHaveBeenCalledWith(
      'Warning:',
      'daemon unavailable. Launch tasks will fail until the daemon is reachable.'
    );
    expect(logSpy).toHaveBeenCalledWith(
      'Type "exit" to quit. Type "/health" to recheck daemon connectivity.\n'
    );
    expect(logSpy).toHaveBeenCalledWith(
      'Daemon reachable again. Launch tasks can run.'
    );
    expect(client.swarms.create).not.toHaveBeenCalled();
    expect(client.swarms.run).not.toHaveBeenCalled();
  });

  it('reports interactive plain-text commands on demand with /help when health is wired', async () => {
    const client = {
      health: vi.fn().mockResolvedValue(undefined),
      swarms: mockSwarms(),
    };
    const inputs = ['/help', 'exit'];
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
        createReadline: () => readline as any,
      }
    );

    expect(errorSpy).not.toHaveBeenCalled();
    expect(logSpy).toHaveBeenCalledWith(
      'Commands: /help show available commands · /health recheck daemon connectivity · exit quit'
    );
    expect(client.swarms.create).not.toHaveBeenCalled();
    expect(client.swarms.run).not.toHaveBeenCalled();
  });

  it('reports healthy daemon state on demand with /health in interactive plain-text launch', async () => {
    const client = {
      health: vi.fn().mockResolvedValue(undefined),
      swarms: mockSwarms(),
    };
    const inputs = ['/health', 'exit'];
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
        createReadline: () => readline as any,
      }
    );

    expect(errorSpy).not.toHaveBeenCalled();
    expect(logSpy).toHaveBeenCalledWith(
      'Type "exit" to quit. Type "/health" to recheck daemon connectivity.\n'
    );
    expect(logSpy).toHaveBeenCalledWith(
      'Daemon reachable. Launch tasks can run.'
    );
    expect(client.swarms.create).not.toHaveBeenCalled();
    expect(client.swarms.run).not.toHaveBeenCalled();
  });

  it('reports interactive plain-text commands on demand with /help when health is not wired', async () => {
    const client = {
      swarms: mockSwarms(),
    };
    const inputs = ['/help', 'exit'];
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
        createReadline: () => readline as any,
      }
    );

    expect(errorSpy).not.toHaveBeenCalled();
    expect(logSpy).toHaveBeenCalledWith(
      'Commands: /help show available commands · exit quit'
    );
    expect(client.swarms.create).not.toHaveBeenCalled();
    expect(client.swarms.run).not.toHaveBeenCalled();
  });

  it('reports unavailable /health checks in interactive plain-text launch when health is not wired', async () => {
    const client = {
      swarms: mockSwarms(),
    };
    const inputs = ['/health', 'exit'];
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
        createReadline: () => readline as any,
      }
    );

    expect(errorSpy).not.toHaveBeenCalled();
    expect(logSpy).toHaveBeenCalledWith('Type "exit" to quit.\n');
    expect(logSpy).toHaveBeenCalledWith(
      'Daemon health checks unavailable in this session.'
    );
    expect(client.swarms.create).not.toHaveBeenCalled();
    expect(client.swarms.run).not.toHaveBeenCalled();
  });

  it('forwards api-key overrides to daemon-backed plain-text launch', async () => {
    const client = {
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-override' }),
        run: vi.fn().mockResolvedValue({
          result: {
            status: 'success',
            data: { text: 'override accepted' },
            durationMs: 4,
          },
        }),
      }),
    };

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
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

    expect(client.swarms.create).toHaveBeenCalledWith(
      expect.objectContaining({
        manager: expect.objectContaining({
          settings: { apiKey: 'secret' },
        }),
        workers: expect.arrayContaining([
          expect.objectContaining({
            settings: { apiKey: 'secret' },
          }),
        ]),
      })
    );
    expect(client.swarms.run).toHaveBeenCalledWith('swarm-override', {
      text: 'Refuse override',
    });
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('reuses one daemon swarm across interactive TUI tasks', async () => {
    const harness = createDaemonTuiHarness();
    const client = {
      swarms: mockSwarms({
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
      }),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as unknown as {
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
    expect(readHistory(agencyDir)).toEqual([
      expect.objectContaining({
        task: 'First task',
        result: 'interactive daemon result',
        isError: false,
      }),
      expect.objectContaining({
        task: 'Second task',
        result: 'interactive daemon result',
        isError: false,
      }),
    ]);
    expect(harness.render).toHaveBeenCalledOnce();
  });

  it('preloads persisted history and resumes the last result in TUI app props', async () => {
    const harness = createDaemonTuiHarness();
    writeFileSync(
      join(agencyDir, LAUNCH_HISTORY_FILENAME),
      JSON.stringify(
        [
          {
            id: 'run-1',
            timestamp: 1,
            task: 'Earlier task',
            result: 'Earlier result',
            isError: false,
          },
        ],
        null,
        2
      ) + '\n'
    );

    const client = {
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        subscribe: vi.fn().mockImplementation(emptySubscription),
      }),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement();
    expect(element).toBeDefined();

    const initialResults = element?.props.initialResults;
    expect(Array.isArray(initialResults)).toBe(true);
    expect(initialResults).toEqual([
      expect.objectContaining({
        task: 'Earlier task',
        result: 'Earlier result',
      }),
    ]);
    expect(element?.props.resumeLastResult).toBe(true);
  });

  it('exposes clear-history wiring alongside resumed result props', async () => {
    const harness = createDaemonTuiHarness();
    writeFileSync(
      join(agencyDir, LAUNCH_HISTORY_FILENAME),
      JSON.stringify(
        [
          {
            id: 'run-1',
            timestamp: 1,
            task: 'Earlier task',
            result: 'Earlier result',
            isError: false,
          },
        ],
        null,
        2
      ) + '\n'
    );

    const client = {
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        subscribe: vi.fn().mockImplementation(emptySubscription),
      }),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as unknown as {
      props: {
        initialResults: Array<{ task: string; result: string }>;
        resumeLastResult: boolean;
        onClearHistory: () => void;
      };
    };

    expect(element.props.initialResults).toEqual([
      expect.objectContaining({
        task: 'Earlier task',
        result: 'Earlier result',
      }),
    ]);
    expect(element.props.resumeLastResult).toBe(true);

    element.props.onClearHistory();

    expect(readHistory(agencyDir)).toEqual([]);
  });

  it('persists saved-run labels through the TUI history update callback', async () => {
    const harness = createDaemonTuiHarness();
    writeFileSync(
      join(agencyDir, LAUNCH_HISTORY_FILENAME),
      JSON.stringify(
        [
          {
            id: 'run-1',
            timestamp: 1,
            task: 'Earlier task',
            result: 'Earlier result',
            isError: false,
          },
        ],
        null,
        2
      ) + '\n'
    );

    const client = {
      swarms: mockSwarms({
        create: vi.fn().mockResolvedValue({ id: 'swarm-1' }),
        subscribe: vi.fn().mockImplementation(emptySubscription),
      }),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as unknown as {
      props: {
        onHistoryUpdated: (
          entries: Array<{
            id: string;
            timestamp: number;
            task: string;
            result: string;
            isError: boolean;
            label?: string;
          }>
        ) => void;
      };
    };

    element.props.onHistoryUpdated([
      {
        id: 'run-1',
        timestamp: 1,
        task: 'Earlier task',
        result: 'Earlier result',
        isError: false,
        label: 'launch hotfix',
      },
    ]);

    expect(readHistory(agencyDir)).toEqual([
      expect.objectContaining({
        task: 'Earlier task',
        result: 'Earlier result',
        label: 'launch hotfix',
      }),
    ]);
  });

  it('clears persisted history through the TUI callback before recording new runs', async () => {
    const harness = createDaemonTuiHarness();
    writeFileSync(
      join(agencyDir, LAUNCH_HISTORY_FILENAME),
      JSON.stringify(
        [
          {
            id: 'run-1',
            timestamp: 1,
            task: 'Earlier task',
            result: 'Earlier result',
            isError: false,
          },
        ],
        null,
        2
      ) + '\n'
    );

    const client = {
      swarms: mockSwarms({
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
            data: { text: 'Fresh result' },
            durationMs: 5,
          },
        }),
        subscribe: vi.fn().mockImplementation(emptySubscription),
      }),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as unknown as {
      props: {
        onClearHistory: () => void;
        onTask: (input: string) => Promise<{ status: string }>;
      };
    };

    element.props.onClearHistory();

    expect(readHistory(agencyDir)).toEqual([]);

    await expect(element.props.onTask('Fresh task')).resolves.toEqual(
      expect.objectContaining({ status: 'success' })
    );

    expect(readHistory(agencyDir)).toEqual([
      expect.objectContaining({
        task: 'Fresh task',
        result: 'Fresh result',
        isError: false,
      }),
    ]);
  });

  it('clears preloaded history and then reuses one daemon swarm for fresh interactive runs', async () => {
    const harness = createDaemonTuiHarness();
    writeFileSync(
      join(agencyDir, LAUNCH_HISTORY_FILENAME),
      JSON.stringify(
        [
          {
            id: 'run-1',
            timestamp: 1,
            task: 'Earlier task',
            result: 'Earlier result',
            isError: false,
          },
        ],
        null,
        2
      ) + '\n'
    );

    const client = {
      swarms: mockSwarms({
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
      }),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as unknown as {
      props: {
        initialResults: Array<{ task: string; result: string }>;
        resumeLastResult: boolean;
        onClearHistory: () => void;
        onTask: (input: string) => Promise<{ status: string }>;
      };
    };

    expect(element.props.initialResults).toEqual([
      expect.objectContaining({
        task: 'Earlier task',
        result: 'Earlier result',
      }),
    ]);
    expect(element.props.resumeLastResult).toBe(true);

    element.props.onClearHistory();

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
    expect(readHistory(agencyDir)).toEqual([
      expect.objectContaining({
        task: 'First task',
        result: 'interactive daemon result',
        isError: false,
      }),
      expect.objectContaining({
        task: 'Second task',
        result: 'interactive daemon result',
        isError: false,
      }),
    ]);
  });

  it('keeps single-shot TUI launches mounted until the user exits', async () => {
    const harness = createDaemonTuiHarness();
    const client = {
      swarms: mockSwarms({
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
            data: { text: 'single-shot result' },
            durationMs: 8,
          },
        }),
        subscribe: vi.fn().mockImplementation(emptySubscription),
      }),
    };

    await executeLaunchCommand(
      'Ship the patch',
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    expect(client.swarms.create).toHaveBeenCalledOnce();
    expect(client.swarms.run).toHaveBeenCalledWith('swarm-1', {
      text: 'Ship the patch',
    });
    expect(harness.render).toHaveBeenCalledOnce();
    expect(harness.waitUntilExit).toHaveBeenCalledOnce();
    expect(harness.unmount).not.toHaveBeenCalled();
  });

  it('returns a task error instead of throwing when daemon TUI swarm creation fails', async () => {
    const harness = createDaemonTuiHarness();
    const client = {
      swarms: mockSwarms({
        create: vi.fn().mockRejectedValue(new Error('daemon unavailable')),
      }),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as unknown as {
      props: {
        onTask: (input: string) => Promise<{ status: string; error?: string }>;
      };
    };

    await expect(element.props.onTask('First task')).resolves.toEqual({
      status: 'error',
      error: 'daemon unavailable',
      durationMs: 0,
    });
    expect(harness.events).toEqual([
      {
        type: 'agent:spawned',
        data: { agentId: 'launch:manager', name: 'manager' },
        agentId: 'launch:manager',
      },
      {
        type: 'agent:spawned',
        data: { agentId: 'launch:worker-a', name: 'worker-a' },
        agentId: 'launch:worker-a',
      },
      {
        type: 'agent:message',
        data: {
          from: 'user',
          to: 'manager',
          message: { text: 'First task' },
        },
        agentId: 'launch:manager',
      },
      {
        type: 'task:failed',
        data: { agentId: 'launch:manager', error: 'daemon unavailable' },
        agentId: 'launch:manager',
      },
      {
        type: 'swarm:completed',
        data: {
          result: {
            status: 'error',
            error: 'daemon unavailable',
            durationMs: 0,
          },
        },
        agentId: 'launch:manager',
      },
    ]);
    expect(client.swarms.run).not.toHaveBeenCalled();
  });

  it('passes a persistent daemon preflight warning into the TUI when health fails', async () => {
    const harness = createDaemonTuiHarness();
    const client = {
      health: vi.fn().mockRejectedValue(new Error('daemon unavailable')),
      swarms: mockSwarms(),
    };

    await executeLaunchCommand(
      undefined,
      {
        dir: agencyDir,
        tui: true,
      },
      { client, ...harness.deps }
    );

    const element = harness.getElement() as unknown as {
      props: {
        preflightWarning?: string;
        pollDaemonWarning?: () => Promise<string | undefined>;
      };
    };

    expect(element.props.preflightWarning).toContain('daemon unavailable');
    expect(element.props.preflightWarning).toContain(
      'Launch tasks will fail until the daemon is reachable.'
    );
    expect(typeof element.props.pollDaemonWarning).toBe('function');
  });

  it('relays tool result text into the TUI event bus', async () => {
    const emit = vi.fn().mockResolvedValue(undefined);

    await relayLaunchSwarmEvent(
      {
        on() {
          return () => undefined;
        },
        emit,
        clear() {},
      },
      [
        {
          id: 'launch:manager',
          name: 'manager',
          role: 'orchestrator',
        },
      ],
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
      }
    );

    expect(emit).toHaveBeenCalledWith(
      'tool:after',
      expect.objectContaining({
        toolName: 'memory_search',
        result: 'Found prior note',
      }),
      'launch:manager'
    );
  });
});
