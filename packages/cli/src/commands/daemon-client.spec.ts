import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Command } from 'commander';

const mockDaemonClient = {
  agents: {
    create: vi.fn(),
    run: vi.fn(),
    list: vi.fn(),
    get: vi.fn(),
    recentMemories: vi.fn(),
  },
  swarms: {
    create: vi.fn(),
    run: vi.fn(),
    get: vi.fn(),
    subscribe: vi.fn(),
  },
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
} as any;

vi.mock('../client.js', () => ({
  DaemonHttpError: class DaemonHttpError extends Error {
    constructor(public readonly status: number, public readonly body: unknown) {
      super(
        typeof body === 'object' &&
          body !== null &&
          'error' in body &&
          typeof body.error === 'string'
          ? body.error
          : `Daemon request failed with status ${status}`
      );
    }
  },
  createCliDaemonClient: vi.fn(() => mockDaemonClient),
}));

vi.mock('./create.js', () => ({
  createCommand: new Command('create'),
}));

vi.mock('./launch.js', () => ({
  launchCommand: new Command('launch'),
}));

describe('CLI daemon-backed command cutover', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockDaemonClient.agents.create.mockReset();
    mockDaemonClient.agents.run.mockReset();
    mockDaemonClient.agents.list.mockReset();
    mockDaemonClient.swarms.create.mockReset();
    mockDaemonClient.swarms.run.mockReset();
    process.exitCode = undefined;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    process.exitCode = undefined;
  });

  it('run command delegates task execution to the daemon client', async () => {
    mockDaemonClient.agents.create.mockResolvedValue({
      state: {
        id: 'agent-1',
        name: 'task-agent',
        status: 'idle',
        config: {
          name: 'task-agent',
          model: 'gpt-4o-mini',
        },
        tokenUsage: {
          promptTokens: 0,
          completionTokens: 0,
          totalTokens: 0,
        },
        createdAt: Date.now(),
      },
      messageCount: 0,
      eventCount: 1,
      lastTask: null,
    });
    mockDaemonClient.agents.run.mockResolvedValue({
      agent: {
        state: {
          id: 'agent-1',
          name: 'task-agent',
          status: 'completed',
          config: {
            name: 'task-agent',
            model: 'gpt-4o-mini',
          },
          tokenUsage: {
            promptTokens: 20,
            completionTokens: 22,
            totalTokens: 42,
          },
          createdAt: Date.now(),
        },
        messageCount: 2,
        eventCount: 8,
        lastTask: null,
      },
      result: {
        status: 'success',
        data: { text: 'daemon handled task' },
        durationMs: 12,
      },
    });

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeRunCommand } = await import('./run.js');

    await executeRunCommand('Ship the daemon cutover', {
      model: 'gpt-4o-mini',
      provider: 'openai',
      name: 'task-agent',
      tui: false,
    });

    expect(mockDaemonClient.agents.create).toHaveBeenCalledWith({
      model: 'gpt-4o-mini',
      name: 'task-agent',
      provider: 'openai',
      system:
        'You are a helpful task agent. Use tools when needed. Be concise.',
    });
    expect(mockDaemonClient.agents.run).toHaveBeenCalledWith('agent-1', {
      text: 'Ship the daemon cutover',
    });
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('parses the real run CLI path without passing the commander instance as the daemon client', async () => {
    mockDaemonClient.agents.create.mockResolvedValue({
      state: {
        id: 'agent-parse-1',
        name: 'task-agent',
        status: 'idle',
        config: {
          name: 'task-agent',
          model: 'gpt-4o-mini',
        },
        tokenUsage: {
          promptTokens: 0,
          completionTokens: 0,
          totalTokens: 0,
        },
        createdAt: Date.now(),
      },
      messageCount: 0,
      eventCount: 1,
      lastTask: null,
    });
    mockDaemonClient.agents.run.mockResolvedValue({
      agent: {
        state: {
          id: 'agent-parse-1',
          name: 'task-agent',
          status: 'completed',
          config: {
            name: 'task-agent',
            model: 'gpt-4o-mini',
          },
          tokenUsage: {
            promptTokens: 1,
            completionTokens: 1,
            totalTokens: 2,
          },
          createdAt: Date.now(),
        },
        messageCount: 2,
        eventCount: 8,
        lastTask: null,
      },
      result: {
        status: 'success',
        data: { text: 'parsed daemon path' },
        durationMs: 5,
      },
    });

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { buildProgram } = await import('../index.js');

    await buildProgram().parseAsync(['node', 'animaos', 'run', 'Parsed task'], {
      from: 'node',
    });

    expect(mockDaemonClient.agents.create).toHaveBeenCalledOnce();
    expect(mockDaemonClient.agents.run).toHaveBeenCalledWith('agent-parse-1', {
      text: 'Parsed task',
    });
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('parses swarm-mode run requests through the daemon swarm client', async () => {
    mockDaemonClient.swarms.create.mockResolvedValue({
      id: 'swarm-1',
      strategy: 'round-robin',
      agents: [],
      tokenUsage: {
        promptTokens: 0,
        completionTokens: 0,
        totalTokens: 0,
      },
      createdAt: Date.now(),
    });
    mockDaemonClient.swarms.run.mockResolvedValue({
      swarm: {
        id: 'swarm-1',
        strategy: 'round-robin',
        agents: [],
        tokenUsage: {
          promptTokens: 3,
          completionTokens: 4,
          totalTokens: 7,
        },
        createdAt: Date.now(),
      },
      result: {
        status: 'success',
        data: { text: 'swarm daemon path' },
        durationMs: 9,
      },
    });

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { buildProgram } = await import('../index.js');

    await buildProgram().parseAsync(
      [
        'node',
        'animaos',
        'run',
        'Coordinate this task',
        '--strategy',
        'round-robin',
      ],
      {
        from: 'node',
      }
    );

    expect(mockDaemonClient.swarms.create).toHaveBeenCalledWith({
      strategy: 'round-robin',
      manager: {
        name: 'manager',
        model: 'gpt-4o-mini',
        provider: 'openai',
        system:
          'You are a task manager. Break complex tasks into subtasks and delegate to workers. Synthesize results into a final answer.',
      },
      workers: [
        {
          name: 'worker',
          model: 'gpt-4o-mini',
          provider: 'openai',
          system:
            'You are a helpful worker agent. Complete the assigned task concisely and accurately.',
        },
      ],
    });
    expect(mockDaemonClient.swarms.run).toHaveBeenCalledWith('swarm-1', {
      text: 'Coordinate this task',
    });
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('reports daemon failures without throwing from the run command', async () => {
    mockDaemonClient.agents.create.mockResolvedValue({
      state: {
        id: 'agent-err-1',
        name: 'task-agent',
        status: 'idle',
        config: {
          name: 'task-agent',
          model: 'gpt-4o-mini',
        },
        tokenUsage: {
          promptTokens: 0,
          completionTokens: 0,
          totalTokens: 0,
        },
        createdAt: Date.now(),
      },
      messageCount: 0,
      eventCount: 1,
      lastTask: null,
    });
    mockDaemonClient.agents.run.mockRejectedValue(
      new Error('daemon unavailable')
    );

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeRunCommand } = await import('./run.js');

    await expect(
      executeRunCommand('Fail the daemon path', {
        model: 'gpt-4o-mini',
        provider: 'openai',
        name: 'task-agent',
        tui: false,
      })
    ).resolves.toBeUndefined();

    expect(errorSpy).toHaveBeenCalledWith('Error:', 'daemon unavailable');
    expect(process.exitCode).toBe(1);
  });

  it('forwards api-key overrides to daemon-backed run', async () => {
    mockDaemonClient.agents.create.mockResolvedValue({
      state: {
        id: 'agent-run-override',
        name: 'task-agent',
        status: 'idle',
        tokenUsage: {
          totalTokens: 7,
        },
      },
    });
    mockDaemonClient.agents.run.mockResolvedValue({
      agent: {
        state: {
          tokenUsage: {
            totalTokens: 7,
          },
        },
      },
      result: {
        status: 'success',
        data: { text: 'daemon handled override' },
        durationMs: 5,
      },
    });

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeRunCommand } = await import('./run.js');

    await expect(
      executeRunCommand('Reject ignored api key', {
        model: 'gpt-4o-mini',
        provider: 'openai',
        name: 'task-agent',
        apiKey: 'secret',
        tui: false,
      })
    ).resolves.toBeUndefined();

    expect(mockDaemonClient.agents.create).toHaveBeenCalledWith({
      model: 'gpt-4o-mini',
      name: 'task-agent',
      provider: 'openai',
      settings: { apiKey: 'secret' },
      system:
        'You are a helpful task agent. Use tools when needed. Be concise.',
    });
    expect(mockDaemonClient.agents.run).toHaveBeenCalledWith(
      'agent-run-override',
      { text: 'Reject ignored api key' }
    );
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('chat uses a single daemon-backed agent session without forcing a provider', async () => {
    mockDaemonClient.agents.create.mockResolvedValue({
      state: {
        id: 'agent-chat-1',
        name: 'task-agent',
        status: 'idle',
      },
    });
    mockDaemonClient.agents.run.mockResolvedValue({
      result: {
        status: 'success',
        data: { text: 'daemon reply' },
        durationMs: 8,
      },
    });

    const readline = {
      question: vi.fn(),
      close: vi.fn(),
    };
    const inputs = ['hello daemon', 'exit'];
    readline.question.mockImplementation(
      (_prompt: string, callback: (input: string) => void) => {
        callback(inputs.shift() ?? 'exit');
      }
    );

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeChatCommand } = await import('./chat.js');

    await executeChatCommand(
      {
        model: 'gpt-4o-mini',
        name: 'task-agent',
      },
      {
        client: mockDaemonClient,
        createReadline: () => readline,
      }
    );

    expect(mockDaemonClient.agents.create).toHaveBeenCalledWith({
      model: 'gpt-4o-mini',
      name: 'task-agent',
      provider: 'openai',
      settings: undefined,
      system:
        'You are a helpful task agent. Use tools when needed. Be concise.',
    });
    expect(mockDaemonClient.agents.run).toHaveBeenCalledWith('agent-chat-1', {
      text: 'hello daemon',
    });
    expect(logSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('forwards api-key overrides to daemon-backed chat', async () => {
    mockDaemonClient.agents.create.mockResolvedValue({
      state: {
        id: 'agent-chat-override',
        name: 'task-agent',
        status: 'idle',
      },
    });

    const readline = {
      question: vi.fn((_: string, callback: (input: string) => void) => {
        callback('exit');
      }),
      close: vi.fn(),
    };

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeChatCommand } = await import('./chat.js');

    await expect(
      executeChatCommand(
        {
          model: 'gpt-4o-mini',
          name: 'task-agent',
          apiKey: 'secret',
        },
        {
          client: mockDaemonClient,
          createReadline: () => readline as any,
        }
      )
    ).resolves.toBeUndefined();

    expect(mockDaemonClient.agents.create).toHaveBeenCalledWith({
      model: 'gpt-4o-mini',
      name: 'task-agent',
      provider: 'openai',
      settings: { apiKey: 'secret' },
      system:
        'You are a helpful task agent. Use tools when needed. Be concise.',
    });
    expect(readline.close).toHaveBeenCalledOnce();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('reports daemon setup failures without throwing from chat', async () => {
    mockDaemonClient.agents.create.mockRejectedValue(
      new Error('daemon unavailable')
    );

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeChatCommand } = await import('./chat.js');

    await expect(
      executeChatCommand(
        {
          model: 'gpt-4o-mini',
          name: 'task-agent',
        },
        {
          client: mockDaemonClient,
        }
      )
    ).resolves.toBeUndefined();

    expect(errorSpy).toHaveBeenCalledWith('Error:', 'daemon unavailable');
    expect(process.exitCode).toBe(1);
  });

  it('agents list reads daemon-backed agent snapshots', async () => {
    mockDaemonClient.agents.list.mockResolvedValue([
      {
        state: {
          id: 'agent-1',
          name: 'planner',
          status: 'idle',
          config: {
            name: 'planner',
            model: 'gpt-4o-mini',
          },
          tokenUsage: {
            promptTokens: 0,
            completionTokens: 0,
            totalTokens: 0,
          },
          createdAt: Date.now(),
        },
        messageCount: 0,
        eventCount: 1,
        lastTask: null,
      },
    ]);

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const { executeAgentsListCommand } = await import('./agents.js');

    await executeAgentsListCommand();

    expect(mockDaemonClient.agents.list).toHaveBeenCalled();
    expect(logSpy).toHaveBeenCalled();
  });

  it('reports daemon failures without throwing from the agents list command', async () => {
    mockDaemonClient.agents.list.mockRejectedValue(
      new Error('daemon unavailable')
    );

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { buildProgram } = await import('../index.js');

    await expect(
      buildProgram().parseAsync(['node', 'animaos', 'agents', 'list'], {
        from: 'node',
      })
    ).resolves.toBeInstanceOf(Command);

    expect(errorSpy).toHaveBeenCalledWith('Error:', 'daemon unavailable');
    expect(process.exitCode).toBe(1);
  });
});
