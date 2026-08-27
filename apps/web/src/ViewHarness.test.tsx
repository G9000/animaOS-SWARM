import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { toolNamesForProfile } from './lib/agent-access';
import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
} from './lib/daemon-api';
import { ViewHarness } from './ViewHarness';

const providers: DaemonProvider[] = [
  {
    id: 'openai',
    label: 'OpenAI',
    requiresKey: true,
    configured: true,
    apiKeyEnvs: ['OPENAI_API_KEY'],
  },
  {
    id: 'anthropic',
    label: 'Anthropic',
    requiresKey: true,
    configured: true,
    apiKeyEnvs: ['ANTHROPIC_API_KEY'],
  },
];

const nativeSetTimeout = window.setTimeout.bind(window);

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function snapshot(
  id: string,
  name: string,
  createdAtMs: number,
  tools = toolNamesForProfile('collaborate'),
): DaemonSnapshot {
  return {
    state: {
      id,
      name,
      status: 'idle',
      config: {
        name,
        provider: 'openai',
        model: 'gpt-4.1',
        system: 'Be precise',
        tools: tools.map((tool) => ({
          name: tool,
          description: tool,
          parameters: {},
        })),
      },
      createdAtMs,
      tokenUsage: {
        promptTokens: 0,
        completionTokens: 0,
        totalTokens: 0,
      },
    },
    messageCount: 0,
    messages: [],
    eventCount: 0,
  };
}

function mockProviders() {
  vi.spyOn(daemon, 'listProviders').mockResolvedValue({ providers });
}

afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
  vi.restoreAllMocks();
});

describe('ViewHarness workspace controller', () => {
  it('renders neutral connecting copy for unknown connection state and never claims connected', () => {
    vi.spyOn(daemon, 'health').mockReturnValue(new Promise(() => undefined));
    vi.spyOn(daemon, 'listAgents').mockReturnValue(
      new Promise(() => undefined),
    );
    vi.spyOn(daemon, 'listProviders').mockReturnValue(
      new Promise(() => undefined),
    );

    render(<ViewHarness />);

    expect(screen.getByText('Connecting to anima-daemon…')).toBeVisible();
    expect(screen.getByText('Checking daemon availability')).toBeVisible();
    expect(screen.queryByText(/connected/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
  });

  it('renders a focused offline retry state with the rust host command and no onboarding or navigation', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'health').mockRejectedValue(new Error('offline'));
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockRejectedValue(new Error('offline'));
    mockProviders();

    render(<ViewHarness />);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Offline');
    expect(alert).toHaveTextContent('bun dev --host rust');
    const retry = screen.getByRole('button', { name: 'Retry connection' });
    expect(retry).toHaveFocus();
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(
      screen.queryByRole('heading', { name: 'Create your agent' }),
    ).not.toBeInTheDocument();

    await user.click(retry);
    expect(listAgents).toHaveBeenCalledTimes(2);
  });

  it('renders only onboarding when the online daemon has zero agents', async () => {
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [] });
    mockProviders();

    render(<ViewHarness />);

    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(screen.queryByText('Daemon Online')).not.toBeInTheDocument();
  });

  it('selects the oldest agent by creation time then id for chat, settings, and Main', async () => {
    const user = userEvent.setup();
    const alpha = snapshot('agent-a', 'Alpha', 10);
    const beta = snapshot('agent-b', 'Beta', 10);
    const later = snapshot('agent-later', 'Later', 20);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({
      agents: [later, beta, alpha],
    });
    mockProviders();
    const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
      agent: alpha,
      result: { status: 'success', durationMs: 1, data: { text: 'done' } },
    });

    render(<ViewHarness />);

    expect(
      await screen.findByRole('heading', { name: 'Say something to Alpha' }),
    ).toBeVisible();
    await user.type(screen.getByPlaceholderText('Message Alpha…'), 'Hello');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(runAgent).toHaveBeenCalledWith('agent-a', 'Hello');

    await user.click(screen.getByRole('button', { name: 'Agents' }));
    expect(
      screen.getByRole('article', { name: 'Alpha agent' }),
    ).toHaveTextContent('Main');
    expect(
      screen.getByRole('article', { name: 'Beta agent' }),
    ).toHaveTextContent('Read only');

    await user.click(screen.getByRole('button', { name: 'Settings' }));
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();
    expect(screen.getByDisplayValue('Alpha')).toBeVisible();
  });

  it('promotes the next-oldest agent after deleting Main and reloads its workspace', async () => {
    const user = userEvent.setup();
    const first = snapshot('agent-first', 'First', 1);
    const next = snapshot('agent-next', 'Next', 2);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [next, first] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to First' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Say something to Next' }),
    ).toBeVisible();
    expect(daemon.deleteAgent).toHaveBeenCalledWith('agent-first');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('patches main identity, provider, model, and system while preserving its messages', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    nova.messages = [
      {
        id: 'message-1',
        agentId: 'agent-main',
        roomId: 'room-1',
        role: 'assistant',
        content: { text: 'Existing conversation' },
        createdAtMs: 2,
      },
    ];
    nova.messageCount = 1;
    const updated = structuredClone(nova);
    updated.state.name = 'Nova Prime';
    updated.state.config.name = 'Nova Prime';
    updated.state.config.provider = 'anthropic';
    updated.state.config.model = 'claude-sonnet-4-6';
    updated.state.config.system = 'Be concise';
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockResolvedValue({ agent: updated });

    render(<ViewHarness />);

    await screen.findByText('Existing conversation');
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Nova Prime');
    const [provider, model] = screen.getAllByRole('combobox');
    await user.selectOptions(provider, 'anthropic');
    await user.selectOptions(model, 'claude-sonnet-4-6');
    const system = screen.getByPlaceholderText(
      'Leave empty for the daemon default.',
    );
    await user.clear(system);
    await user.type(system, 'Be concise');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(updateAgent).toHaveBeenCalledWith('agent-main', {
      name: 'Nova Prime',
      provider: 'anthropic',
      model: 'claude-sonnet-4-6',
      system: 'Be concise',
    });
    expect(await screen.findByDisplayValue('Nova Prime')).toBeVisible();
    expect(screen.getByText('Existing conversation')).toBeVisible();
  });

  it('returns to onboarding after deleting the final agent', async () => {
    const user = userEvent.setup();
    const only = snapshot('agent-only', 'Only', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [only] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to Only' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
  });

  it('keeps reset authoritative when an older poll resolves after deletion', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    const stalePoll = deferred<{ agents: DaemonSnapshot[] }>();
    const deletion = deferred<{ deleted: boolean }>();
    const listAgents = vi
      .spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [nova] })
      .mockReturnValueOnce(stalePoll.promise);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockReturnValue(deletion.promise);
    let runPoll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 5_000) {
        runPoll = handler;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to Nova' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    act(() => runPoll?.());
    expect(listAgents).toHaveBeenCalledTimes(2);

    await act(async () => {
      deletion.resolve({ deleted: true });
      await deletion.promise;
    });
    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();

    await act(async () => {
      stalePoll.resolve({ agents: [nova] });
      await stalePoll.promise;
    });
    expect(
      screen.getByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    expect(
      screen.queryByRole('heading', { name: 'Say something to Nova' }),
    ).not.toBeInTheDocument();
  });

  it('keeps the last-known shell and explicitly labels Offline after a late poll failure', async () => {
    vi.useFakeTimers();
    const nova = snapshot('agent-main', 'Nova', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [nova] })
      .mockRejectedValueOnce(new Error('poll failed'));
    mockProviders();

    render(<ViewHarness />);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(screen.getByText('Daemon Online')).toBeVisible();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });

    expect(screen.getByText('Daemon Offline')).toBeVisible();
    expect(screen.getByLabelText('Daemon offline')).toBeVisible();
    expect(
      screen.getByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    expect(screen.getByRole('navigation')).toBeVisible();
    expect(daemon.listAgents).toHaveBeenCalledTimes(2);
  });
});
