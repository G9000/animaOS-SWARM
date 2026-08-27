import { act, render, screen, waitFor } from '@testing-library/react';
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

function withMessage(source: DaemonSnapshot, text: string): DaemonSnapshot {
  const updated = structuredClone(source);
  updated.messages = [
    {
      id: `message-${source.state.id}`,
      agentId: source.state.id,
      roomId: `room-${source.state.id}`,
      role: 'assistant',
      content: { text },
      createdAtMs: source.state.createdAtMs + 1,
    },
  ];
  updated.messageCount = 1;
  return updated;
}

function capturePollTimer() {
  let poll: (() => void) | undefined;
  vi.spyOn(window, 'setTimeout').mockImplementation(((
    handler: TimerHandler,
    timeout?: number,
  ) => {
    if (typeof handler === 'function' && timeout === 5_000) {
      poll = handler;
      return 1;
    }
    return nativeSetTimeout(handler, timeout);
  }) as typeof window.setTimeout);
  return () => {
    if (!poll) throw new Error('poll timer was not scheduled');
    poll();
  };
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

  it('promotes the next agent and keeps its controller usable when local cleanup fails after DELETE', async () => {
    const user = userEvent.setup();
    const first = snapshot('agent-first', 'First', 1);
    const next = snapshot('agent-next', 'Next', 2);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [next, first] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
    const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
      agent: withMessage(next, 'Next is responsive'),
      result: {
        status: 'success',
        durationMs: 1,
        data: { text: 'Next is responsive' },
      },
    });
    const removeItem = vi
      .spyOn(Storage.prototype, 'removeItem')
      .mockImplementation(() => {
        throw new DOMException('Storage access denied', 'SecurityError');
      });

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to First' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Say something to Next' }),
    ).toBeVisible();
    expect(removeItem).toHaveBeenCalledWith('animaos.checkins.agent-first');
    await user.type(screen.getByPlaceholderText('Message Next…'), 'Continue');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(runAgent).toHaveBeenCalledWith('agent-next', 'Continue');
    expect(await screen.findByText('Next is responsive')).toBeVisible();
  });

  it('patches main identity, provider, model, system, and deliberate access while preserving its messages', async () => {
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
    updated.state.config.tools = toolNamesForProfile('operate').map((tool) => ({
      name: tool,
      description: tool,
      parameters: {},
    }));
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
    await user.click(screen.getByRole('radio', { name: /^Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(updateAgent).toHaveBeenCalledWith('agent-main', {
      name: 'Nova Prime',
      provider: 'anthropic',
      model: 'claude-sonnet-4-6',
      system: 'Be concise',
      tools: toolNamesForProfile('operate'),
    });
    expect(await screen.findByDisplayValue('Nova Prime')).toBeVisible();
    expect(screen.getByText('Existing conversation')).toBeVisible();
    expect(screen.getByText('Access Operate')).toBeVisible();
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();
  });

  it('keeps failed access and identity edits in the open panel without changing the current agent', async () => {
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
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockRejectedValue(new Error('PATCH denied'));

    render(<ViewHarness />);

    await screen.findByText('Existing conversation');
    expect(screen.getByText('Access Collaborate')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Unsaved Nova');
    const [provider, model] = screen.getAllByRole('combobox');
    await user.selectOptions(provider, 'anthropic');
    await user.selectOptions(model, '__custom__');
    await user.type(
      screen.getByPlaceholderText('model id, e.g. llama3.1'),
      'anthropic/unsaved-model',
    );
    const system = screen.getByPlaceholderText(
      'Leave empty for the daemon default.',
    );
    await user.clear(system);
    await user.type(system, 'Unsaved system');
    await user.click(screen.getByRole('radio', { name: /^Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(updateAgent).toHaveBeenCalledWith('agent-main', {
      name: 'Unsaved Nova',
      provider: 'anthropic',
      model: 'anthropic/unsaved-model',
      system: 'Unsaved system',
      tools: toolNamesForProfile('operate'),
    });
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('PATCH denied');
    expect(alert).toHaveFocus();
    expect(screen.getByDisplayValue('Unsaved Nova')).toBeVisible();
    expect(screen.getByDisplayValue('anthropic/unsaved-model')).toBeVisible();
    expect(screen.getByDisplayValue('Unsaved system')).toBeVisible();
    expect(screen.getByRole('radio', { name: /^Operate/ })).toBeChecked();
    expect(screen.getByText('Access Collaborate')).toBeVisible();
    expect(screen.getByText('Existing conversation')).toBeVisible();
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();
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

  it('returns to onboarding when local cleanup fails after the final DELETE', async () => {
    const user = userEvent.setup();
    const only = snapshot('agent-only', 'Only', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [only] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
    const removeItem = vi
      .spyOn(Storage.prototype, 'removeItem')
      .mockImplementation(() => {
        throw new DOMException('Storage access denied', 'SecurityError');
      });

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to Only' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Create your main agent' }),
    ).toBeVisible();
    expect(removeItem).toHaveBeenCalledWith('animaos.checkins.agent-only');
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

  it('does not re-add the previous main when its pending run resolves after poll replacement', async () => {
    const user = userEvent.setup();
    const first = snapshot('agent-a', 'Alpha', 1);
    const next = snapshot('agent-b', 'Beta', 2);
    const replacement = deferred<{ agents: DaemonSnapshot[] }>();
    const run = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [first] })
      .mockReturnValueOnce(replacement.promise);
    mockProviders();
    vi.spyOn(daemon, 'runAgent').mockReturnValue(run.promise);
    const poll = capturePollTimer();

    render(<ViewHarness />);
    await screen.findByRole('heading', { name: 'Say something to Alpha' });
    await user.type(
      screen.getByPlaceholderText('Message Alpha…'),
      'Alpha work',
    );
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(daemon.runAgent).toHaveBeenCalledWith('agent-a', 'Alpha work');

    act(() => poll());
    await act(async () => {
      replacement.resolve({ agents: [next] });
      await replacement.promise;
    });
    await screen.findByRole('heading', { name: 'Say something to Beta' });

    await act(async () => {
      run.resolve({
        agent: withMessage(first, 'Stale Alpha reply'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Stale Alpha reply' },
        },
      });
      await run.promise;
    });

    expect(
      screen.getByRole('heading', { name: 'Say something to Beta' }),
    ).toBeVisible();
    expect(screen.queryByText('Stale Alpha reply')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Agents' }));
    expect(screen.getByRole('article', { name: 'Beta agent' })).toBeVisible();
    expect(
      screen.queryByRole('article', { name: 'Alpha agent' }),
    ).not.toBeInTheDocument();
  });

  it('does not re-add the previous main when its pending PATCH resolves after poll replacement', async () => {
    const user = userEvent.setup();
    const first = snapshot('agent-a', 'Alpha', 1);
    const next = snapshot('agent-b', 'Beta', 2);
    const replacement = deferred<{ agents: DaemonSnapshot[] }>();
    const update = deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [first] })
      .mockReturnValueOnce(replacement.promise);
    mockProviders();
    vi.spyOn(daemon, 'updateAgent').mockReturnValue(update.promise);
    const poll = capturePollTimer();

    render(<ViewHarness />);
    await screen.findByRole('heading', { name: 'Say something to Alpha' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Alpha');
    await user.clear(name);
    await user.type(name, 'Alpha draft');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    act(() => poll());
    await act(async () => {
      replacement.resolve({ agents: [next] });
      await replacement.promise;
    });
    await screen.findByRole('heading', { name: 'Say something to Beta' });

    const staleUpdate = structuredClone(first);
    staleUpdate.state.name = 'Alpha draft';
    staleUpdate.state.config.name = 'Alpha draft';
    await act(async () => {
      update.resolve({ agent: staleUpdate });
      await update.promise;
    });

    expect(
      screen.getByRole('heading', { name: 'Say something to Beta' }),
    ).toBeVisible();
    expect(
      screen.queryByRole('heading', { name: 'Agent settings' }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Agents' }));
    expect(
      screen.queryByRole('article', { name: 'Alpha draft agent' }),
    ).not.toBeInTheDocument();
  });

  it('does not stamp Beta check-ins or re-add Alpha when an Alpha check-in resolves after poll replacement', async () => {
    const user = userEvent.setup();
    const now = 100_000;
    vi.spyOn(Date, 'now').mockReturnValue(now);
    const first = snapshot('agent-a', 'Alpha', 1);
    const next = snapshot('agent-b', 'Beta', 2);
    localStorage.setItem(
      'animaos.checkins.agent-a',
      JSON.stringify([
        {
          id: 'alpha-checkin',
          prompt: 'Alpha private goals',
          intervalSecs: 1,
          createdAtMs: 0,
        },
      ]),
    );
    localStorage.setItem(
      'animaos.checkins.agent-b',
      JSON.stringify([
        {
          id: 'beta-checkin',
          prompt: 'Beta saved goals',
          intervalSecs: 60,
          createdAtMs: now,
        },
      ]),
    );
    const replacement = deferred<{ agents: DaemonSnapshot[] }>();
    const checkin = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [first] })
      .mockReturnValueOnce(replacement.promise);
    mockProviders();
    vi.spyOn(daemon, 'runAgent').mockReturnValue(checkin.promise);
    const poll = capturePollTimer();
    let runCheckins: (() => unknown) | undefined;
    vi.spyOn(window, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === 10_000) {
        runCheckins = handler;
      }
      return 1;
    }) as typeof window.setInterval);

    render(<ViewHarness />);
    await screen.findByRole('heading', { name: 'Say something to Alpha' });
    await user.click(screen.getByRole('button', { name: 'Activity' }));
    await screen.findByText('Alpha private goals');
    act(() => void runCheckins?.());
    await waitFor(() => expect(daemon.runAgent).toHaveBeenCalledTimes(1));

    act(() => poll());
    await act(async () => {
      replacement.resolve({ agents: [next] });
      await replacement.promise;
    });
    await screen.findByRole('heading', { name: 'Say something to Beta' });

    await act(async () => {
      checkin.resolve({
        agent: withMessage(first, 'Stale Alpha check-in reply'),
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Stale Alpha check-in reply' },
        },
      });
      await checkin.promise;
    });

    await user.click(screen.getByRole('button', { name: 'Activity' }));
    expect(await screen.findByText('Beta saved goals')).toBeVisible();
    expect(screen.getByText('has not run yet')).toBeVisible();
    expect(screen.queryByText('Alpha private goals')).not.toBeInTheDocument();
    expect(localStorage.getItem('animaos.checkins.agent-b')).not.toContain(
      'lastOutcome',
    );
    await user.click(screen.getByRole('button', { name: 'Agents' }));
    expect(
      screen.queryByRole('article', { name: 'Alpha agent' }),
    ).not.toBeInTheDocument();
  });

  it('clears Alpha-scoped draft, prompt, error, settings, and view when polling selects Beta', async () => {
    const user = userEvent.setup();
    const first = snapshot('agent-a', 'Alpha', 1);
    const next = snapshot('agent-b', 'Beta', 2);
    localStorage.setItem(
      'animaos.checkins.agent-b',
      JSON.stringify([
        {
          id: 'beta-saved',
          prompt: 'Beta stored check-in',
          intervalSecs: 60,
          createdAtMs: Date.now(),
        },
      ]),
    );
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [first] })
      .mockResolvedValueOnce({ agents: [next] });
    mockProviders();
    vi.spyOn(daemon, 'runAgent').mockRejectedValue(new Error('Alpha failed'));
    const poll = capturePollTimer();

    render(<ViewHarness />);
    await screen.findByRole('heading', { name: 'Say something to Alpha' });
    await user.type(screen.getByPlaceholderText('Message Alpha…'), 'fail');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(await screen.findByText('Alpha failed')).toBeVisible();
    await user.type(
      screen.getByPlaceholderText('Message Alpha…'),
      'Alpha private draft',
    );
    await user.click(screen.getByRole('button', { name: 'Activity' }));
    await user.type(
      screen.getByPlaceholderText(/Check my goals/),
      'Alpha private prompt',
    );
    await user.click(screen.getByRole('button', { name: 'Settings' }));

    act(() => poll());
    await screen.findByRole('heading', { name: 'Say something to Beta' });

    expect(
      screen.queryByRole('heading', { name: 'Agent settings' }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText('Alpha failed')).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText('Message Beta…')).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Activity' }));
    expect(screen.getByPlaceholderText(/Check my goals/)).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Add prompt' })).toBeDisabled();
    expect(await screen.findByText('Beta stored check-in')).toBeVisible();
  });

  it('clears First-scoped draft and prompt while preserving Next check-ins on reset promotion', async () => {
    const user = userEvent.setup();
    const first = snapshot('agent-first', 'First', 1);
    const next = snapshot('agent-next', 'Next', 2);
    localStorage.setItem(
      'animaos.checkins.agent-next',
      JSON.stringify([
        {
          id: 'next-saved',
          prompt: 'Next stored check-in',
          intervalSecs: 60,
          createdAtMs: Date.now(),
        },
      ]),
    );
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [next, first] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });

    render(<ViewHarness />);
    await screen.findByRole('heading', { name: 'Say something to First' });
    await user.type(
      screen.getByPlaceholderText('Message First…'),
      'First private draft',
    );
    await user.click(screen.getByRole('button', { name: 'Activity' }));
    await user.type(
      screen.getByPlaceholderText(/Check my goals/),
      'First private prompt',
    );
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    await screen.findByRole('heading', { name: 'Say something to Next' });
    expect(screen.getByPlaceholderText('Message Next…')).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(
      screen.queryByRole('heading', { name: 'Agent settings' }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Activity' }));
    expect(screen.getByPlaceholderText(/Check my goals/)).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Add prompt' })).toBeDisabled();
    expect(await screen.findByText('Next stored check-in')).toBeVisible();
  });
});
