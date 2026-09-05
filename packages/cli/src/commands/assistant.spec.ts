import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mockDaemonClient = {
  agents: {
    list: vi.fn(),
    run: vi.fn(),
  },
  requestJson: vi.fn(),
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
} as any;

vi.mock('../client.js', () => ({
  createCliDaemonClient: vi.fn(() => mockDaemonClient),
}));

const ASSISTANT_SNAPSHOT = {
  state: {
    id: 'agent-assistant-1',
    name: 'assistant',
    status: 'idle',
    config: {
      name: 'assistant',
      model: 'gpt-4o-mini',
    },
    tokenUsage: {
      promptTokens: 0,
      completionTokens: 0,
      totalTokens: 0,
    },
    createdAtMs: Date.now(),
  },
  messageCount: 0,
  eventCount: 1,
  lastTask: null,
};

describe('assistant command', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockDaemonClient.agents.list.mockReset();
    mockDaemonClient.agents.run.mockReset();
    mockDaemonClient.requestJson.mockReset();
    process.exitCode = undefined;
    delete process.env.ANIMAOS_USER_ID;
    delete process.env.ANIMAOS_USER_NAME;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    process.exitCode = undefined;
    delete process.env.ANIMAOS_USER_ID;
    delete process.env.ANIMAOS_USER_NAME;
  });

  it('errors when the assistant agent is not found', async () => {
    mockDaemonClient.agents.list.mockResolvedValue([
      {
        ...ASSISTANT_SNAPSHOT,
        state: {
          ...ASSISTANT_SNAPSHOT.state,
          id: 'agent-other',
          name: 'other',
        },
      },
    ]);

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeAssistantChatCommand } = await import('./assistant.js');

    await executeAssistantChatCommand(
      { name: 'assistant' },
      { client: mockDaemonClient }
    );

    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining('ANIMAOS_RS_ASSISTANT_ENABLED=1')
    );
    expect(process.exitCode).toBe(1);
    expect(mockDaemonClient.agents.run).not.toHaveBeenCalled();
  });

  it('sends user metadata with each run and prints replies', async () => {
    mockDaemonClient.agents.list.mockResolvedValue([ASSISTANT_SNAPSHOT]);
    mockDaemonClient.agents.run.mockResolvedValue({
      agent: ASSISTANT_SNAPSHOT,
      result: {
        status: 'success',
        data: { text: 'assistant reply' },
        durationMs: 8,
      },
    });

    const readline = {
      question: vi.fn(),
      close: vi.fn(),
    };
    const inputs = ['hello assistant', 'exit'];
    readline.question.mockImplementation(
      (_prompt: string, callback: (input: string) => void) => {
        callback(inputs.shift() ?? 'exit');
      }
    );

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeAssistantChatCommand } = await import('./assistant.js');

    await executeAssistantChatCommand(
      { name: 'assistant', userId: 'user-42', userName: 'Leo' },
      {
        client: mockDaemonClient,
        createReadline: () => readline,
      }
    );

    expect(mockDaemonClient.agents.run).toHaveBeenCalledWith(
      'agent-assistant-1',
      {
        text: 'hello assistant',
        metadata: { userId: 'user-42', userName: 'Leo' },
      }
    );
    expect(logSpy).toHaveBeenCalledWith('\nagent > assistant reply\n');
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('falls back to environment and OS username for metadata', async () => {
    process.env.ANIMAOS_USER_ID = 'env-user-id';
    mockDaemonClient.agents.list.mockResolvedValue([ASSISTANT_SNAPSHOT]);
    mockDaemonClient.agents.run.mockResolvedValue({
      agent: ASSISTANT_SNAPSHOT,
      result: {
        status: 'success',
        data: { text: 'ok' },
        durationMs: 1,
      },
    });

    const readline = {
      question: vi.fn(
        (
          _prompt: string,
          optionsOrCallback: object | ((input: string) => void),
          callback?: (input: string) => void
        ) => {
          const answer =
            typeof optionsOrCallback === 'function'
              ? optionsOrCallback
              : callback;
          answer?.('exit');
        }
      ),
      close: vi.fn(),
    };

    const { executeAssistantChatCommand } = await import('./assistant.js');

    await executeAssistantChatCommand(
      { name: 'assistant' },
      {
        client: mockDaemonClient,
        createReadline: () => readline,
      }
    );

    expect(mockDaemonClient.agents.run).not.toHaveBeenCalled();

    const inputs = ['hi', 'exit'];
    readline.question.mockImplementation(
      (
        _prompt: string,
        optionsOrCallback: object | ((input: string) => void),
        callback?: (input: string) => void
      ) => {
        const answer =
          typeof optionsOrCallback === 'function'
            ? optionsOrCallback
            : callback;
        answer?.(inputs.shift() ?? 'exit');
      }
    );

    await executeAssistantChatCommand(
      { name: 'assistant' },
      {
        client: mockDaemonClient,
        createReadline: () => readline,
      }
    );

    const expectedUserName =
      process.env.ANIMAOS_USER_NAME ||
      process.env.USERNAME ||
      process.env.USER ||
      'unknown';
    expect(mockDaemonClient.agents.run).toHaveBeenCalledWith(
      'agent-assistant-1',
      {
        text: 'hi',
        metadata: { userId: 'env-user-id', userName: expectedUserName },
      }
    );
  });

  it('exits the chat REPL cleanly on "exit"', async () => {
    mockDaemonClient.agents.list.mockResolvedValue([ASSISTANT_SNAPSHOT]);

    const readline = {
      question: vi.fn(
        (
          _prompt: string,
          optionsOrCallback: object | ((input: string) => void),
          callback?: (input: string) => void
        ) => {
          const answer =
            typeof optionsOrCallback === 'function'
              ? optionsOrCallback
              : callback;
          answer?.('exit');
        }
      ),
      close: vi.fn(),
    };

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeAssistantChatCommand } = await import('./assistant.js');

    await expect(
      executeAssistantChatCommand(
        { name: 'assistant' },
        {
          client: mockDaemonClient,
          createReadline: () => readline,
        }
      )
    ).resolves.toBeUndefined();

    expect(readline.close).toHaveBeenCalledOnce();
    expect(logSpy).toHaveBeenCalledWith('Bye.');
    expect(mockDaemonClient.agents.run).not.toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('inbox prints outbox messages with ISO time and job', async () => {
    mockDaemonClient.requestJson.mockResolvedValue({
      messages: [
        {
          id: 'msg-1',
          job: 'morning-briefing',
          text: 'Good morning!',
          createdAtMs: 1756100000000,
        },
      ],
    });

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeAssistantInboxCommand } = await import('./assistant.js');

    await executeAssistantInboxCommand(
      { since: '1756000000000', limit: '10' },
      { client: mockDaemonClient }
    );

    expect(mockDaemonClient.requestJson).toHaveBeenCalledWith(
      '/api/assistant/outbox?since=1756000000000&limit=10'
    );
    expect(logSpy).toHaveBeenCalledWith(
      `[${new Date(1756100000000).toISOString()}] (morning-briefing) Good morning!`
    );
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('inbox prints "No new messages." when the outbox is empty', async () => {
    mockDaemonClient.requestJson.mockResolvedValue({ messages: [] });

    const logSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
    const { executeAssistantInboxCommand } = await import('./assistant.js');

    await executeAssistantInboxCommand({}, { client: mockDaemonClient });

    expect(mockDaemonClient.requestJson).toHaveBeenCalledWith(
      '/api/assistant/outbox'
    );
    expect(logSpy).toHaveBeenCalledWith('No new messages.');
  });

  it('inbox reports daemon failures without throwing', async () => {
    mockDaemonClient.requestJson.mockRejectedValue(
      new Error('daemon unavailable')
    );

    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { executeAssistantInboxCommand } = await import('./assistant.js');

    await expect(
      executeAssistantInboxCommand({}, { client: mockDaemonClient })
    ).resolves.toBeUndefined();

    expect(errorSpy).toHaveBeenCalledWith('Error:', 'daemon unavailable');
    expect(process.exitCode).toBe(1);
  });
});
