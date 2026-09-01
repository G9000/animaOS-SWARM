import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

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
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
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

beforeEach(() => {
  vi.spyOn(daemon, 'listConnectors').mockResolvedValue({ connectors: [] });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  vi.spyOn(daemon, 'importLegacySchedules').mockResolvedValue({
    schedules: [],
  });
});

afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
  vi.restoreAllMocks();
});

describe('ViewHarness workspace controller', () => {
  it('uploads a workspace avatar and refreshes daemon-owned workspace state', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    const getWorkspace = vi
      .spyOn(daemon, 'getWorkspace')
      .mockResolvedValueOnce({
        configured: true,
        workspace: {
          rootPath: '/workspaces/northwind',
          companyName: 'Northwind Research',
          mission: 'Map supply chains',
          values: ['rigor'],
          hasAvatar: false,
        },
        defaultRoot: '/workspaces',
      })
      .mockResolvedValue({
        configured: true,
        workspace: {
          rootPath: '/workspaces/northwind',
          companyName: 'Northwind Research',
          mission: 'Map supply chains',
          values: ['rigor'],
          hasAvatar: true,
        },
        defaultRoot: '/workspaces',
      });
    const uploadWorkspaceAvatar = vi
      .spyOn(daemon, 'uploadWorkspaceAvatar')
      .mockResolvedValue(undefined);
    Object.defineProperties(URL, {
      createObjectURL: {
        configurable: true,
        value: vi.fn(() => 'blob:workspace-avatar-preview'),
      },
      revokeObjectURL: {
        configurable: true,
        value: vi.fn(),
      },
    });

    render(<ViewHarness />);

    const input = await screen.findByLabelText('Workspace avatar image file');
    const file = new File(['avatar'], 'avatar.png', { type: 'image/png' });
    await user.upload(input, file);

    await waitFor(() =>
      expect(uploadWorkspaceAvatar).toHaveBeenCalledWith(file),
    );
    await waitFor(() => expect(getWorkspace).toHaveBeenCalledTimes(2));
  });

  it('imports legacy prompts into the daemon without starting a browser execution timer', async () => {
    const nova = snapshot('agent-main', 'Nova', 1);
    localStorage.setItem(
      'animaos.checkins.agent-main',
      JSON.stringify([
        {
          id: 'legacy',
          prompt: 'Check goals',
          intervalSecs: 60,
          createdAtMs: 1,
        },
      ]),
    );
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    const interval = vi.spyOn(window, 'setInterval');
    const runAgent = vi.spyOn(daemon, 'runAgent');

    render(<ViewHarness />);
    await screen.findByRole('heading', { name: 'Say something to Nova' });
    await waitFor(() =>
      expect(daemon.importLegacySchedules).toHaveBeenCalled(),
    );
    expect(interval.mock.calls.some(([, delay]) => delay === 10_000)).toBe(
      false,
    );
    expect(runAgent).not.toHaveBeenCalled();
  });
  it('makes the workspace inert while settings are open and restores trigger focus on close', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();

    render(<ViewHarness />);

    const trigger = await screen.findByRole('button', { name: 'Settings' });
    await user.click(trigger);
    expect(screen.getByTestId('workspace-background')).toHaveAttribute(
      'aria-hidden',
      'true',
    );
    expect(screen.getByTestId('workspace-background')).toHaveAttribute('inert');
    expect(
      screen.getByRole('dialog', { name: 'Agent settings' }),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.getByTestId('workspace-background')).not.toHaveAttribute(
      'aria-hidden',
    );
    expect(screen.getByTestId('workspace-background')).not.toHaveAttribute(
      'inert',
    );
  });

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
      await screen.findByRole('heading', { name: 'Set up your workspace' }),
    ).toBeVisible();
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(screen.queryByText('Welcome back')).not.toBeInTheDocument();
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
    expect(screen.getByText('Nova Prime')).toBeVisible();
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();
  });

  it('locks the settings transaction and ignores Reset until a deferred PATCH is adopted', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    const updated = structuredClone(nova);
    updated.state.name = 'Nova Prime';
    updated.state.config.name = 'Nova Prime';
    updated.state.config.tools = toolNamesForProfile('operate').map((tool) => ({
      name: tool,
      description: tool,
      parameters: {},
    }));
    const update = deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockReturnValue(update.promise);
    const deleteAgent = vi
      .spyOn(daemon, 'deleteAgent')
      .mockResolvedValue({ deleted: true });

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to Nova' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Nova Prime');
    await user.click(screen.getByRole('radio', { name: /^Operate/ }));
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(updateAgent).toHaveBeenCalledWith('agent-main', {
      name: 'Nova Prime',
      tools: toolNamesForProfile('operate'),
    });
    const panel = screen.getByRole('dialog', { name: 'Agent settings' });
    expect(within(panel).getByDisplayValue('Nova Prime')).toBeDisabled();
    expect(
      within(panel).getByRole('radio', { name: /^Operate/ }),
    ).toBeDisabled();
    const reset = within(panel).getByRole('button', { name: 'Reset' });
    expect(reset).toBeDisabled();
    reset.removeAttribute('disabled');
    fireEvent.click(reset);
    expect(deleteAgent).not.toHaveBeenCalled();

    await act(async () => {
      update.resolve({ agent: updated });
      await update.promise;
    });

    expect(await screen.findByDisplayValue('Nova Prime')).toBeEnabled();
    expect(screen.getByRole('radio', { name: /^Operate/ })).toBeChecked();
    expect(screen.getByRole('radio', { name: /^Operate/ })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Reset' })).toBeEnabled();
  });

  it('locks settings during reset and rejects a forced save until DELETE settles', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    const deletion = deferred<Awaited<ReturnType<typeof daemon.deleteAgent>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockReturnValue(deletion.promise);
    const updateAgent = vi.spyOn(daemon, 'updateAgent');

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to Nova' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Unsaved Nova');
    const save = screen.getByRole('button', { name: 'Save changes' });
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(screen.getByRole('button', { name: 'Resetting…' })).toBeDisabled();
    expect(screen.getByDisplayValue('Unsaved Nova')).toBeDisabled();
    expect(save).toBeDisabled();
    save.removeAttribute('disabled');
    fireEvent.click(save);
    expect(updateAgent).not.toHaveBeenCalled();

    await act(async () => {
      deletion.resolve({ deleted: true });
      await deletion.promise;
    });
    expect(
      await screen.findByRole('heading', { name: 'Set up your workspace' }),
    ).toBeVisible();
  });

  it('does not surface a pre-existing workspace error as a settings failure', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    vi.spyOn(daemon, 'runAgent').mockRejectedValue(
      new Error('workspace connection failed'),
    );

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to Nova' });
    await user.type(
      screen.getByPlaceholderText('Message Nova…'),
      'Trigger failure',
    );
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(
      await screen.findByText('workspace connection failed'),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Settings' }));

    const panel = screen.getByRole('dialog', { name: 'Agent settings' });
    expect(
      within(panel).queryByText('workspace connection failed'),
    ).not.toBeInTheDocument();
    expect(within(panel).queryByRole('alert')).not.toBeInTheDocument();
    expect(
      within(panel).getByRole('button', { name: 'No changes' }),
    ).not.toHaveAttribute('aria-describedby');
  });

  it('keeps the full draft mounted through a deferred reset failure, then allows close', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    const deletion = deferred<Awaited<ReturnType<typeof daemon.deleteAgent>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockReturnValue(deletion.promise);

    render(<ViewHarness />);

    await screen.findByRole('heading', { name: 'Say something to Nova' });
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
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    const close = screen.getByRole('button', { name: 'Close settings' });
    expect(close).toBeDisabled();
    expect(close).toHaveAccessibleDescription(/resetting/i);
    await user.click(close);
    fireEvent.click(screen.getByTestId('settings-backdrop'));
    await user.keyboard('{Escape}');
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();

    await act(async () => {
      deletion.reject(new Error('DELETE denied'));
      await deletion.promise.catch(() => undefined);
    });

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('DELETE denied');
    expect(alert).toHaveAttribute('id', 'settings-reset-error');
    expect(alert).toHaveAttribute('aria-live', 'assertive');
    expect(alert).toHaveFocus();
    expect(screen.getByRole('button', { name: 'Reset' })).toHaveAttribute(
      'aria-describedby',
      'settings-reset-error',
    );
    expect(
      screen.getByRole('button', { name: 'Save changes' }),
    ).not.toHaveAttribute('aria-describedby');
    expect(screen.getByDisplayValue('Unsaved Nova')).toBeVisible();
    expect(screen.getAllByRole('combobox')[0]).toHaveValue('anthropic');
    expect(screen.getByDisplayValue('anthropic/unsaved-model')).toBeVisible();
    expect(screen.getByDisplayValue('Unsaved system')).toBeVisible();
    expect(screen.getByRole('radio', { name: /^Operate/ })).toBeChecked();
    expect(close).toBeEnabled();
    await user.click(close);
    expect(
      screen.queryByRole('heading', { name: 'Agent settings' }),
    ).not.toBeInTheDocument();
  });

  it('keeps the full draft mounted through a deferred save failure, then allows close', async () => {
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
    const update = deferred<Awaited<ReturnType<typeof daemon.updateAgent>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockReturnValue(update.promise);

    render(<ViewHarness />);

    await screen.findByText('Existing conversation');
    expect(screen.getByText('Welcome back')).toBeVisible();
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
    const close = screen.getByRole('button', { name: 'Close settings' });
    expect(close).toBeDisabled();
    expect(close).toHaveAccessibleDescription(/saving/i);
    await user.click(close);
    fireEvent.click(screen.getByTestId('settings-backdrop'));
    await user.keyboard('{Escape}');
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();

    await act(async () => {
      update.reject(new Error('PATCH denied'));
      await update.promise.catch(() => undefined);
    });

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('PATCH denied');
    expect(alert).toHaveAttribute('aria-live', 'assertive');
    expect(alert).toHaveFocus();
    expect(screen.getByDisplayValue('Unsaved Nova')).toBeVisible();
    expect(screen.getAllByRole('combobox')[0]).toHaveValue('anthropic');
    expect(screen.getByDisplayValue('anthropic/unsaved-model')).toBeVisible();
    expect(screen.getByDisplayValue('Unsaved system')).toBeVisible();
    expect(screen.getByRole('radio', { name: /^Operate/ })).toBeChecked();
    expect(screen.getByText('Welcome back')).toBeVisible();
    expect(screen.getByText('Existing conversation')).toBeVisible();
    expect(
      screen.getByRole('heading', { name: 'Agent settings' }),
    ).toBeVisible();
    expect(close).toBeEnabled();
    await user.click(close);
    expect(
      screen.queryByRole('heading', { name: 'Agent settings' }),
    ).not.toBeInTheDocument();
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
      await screen.findByRole('heading', { name: 'Set up your workspace' }),
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
      await screen.findByRole('heading', { name: 'Set up your workspace' }),
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
      await screen.findByRole('heading', { name: 'Set up your workspace' }),
    ).toBeVisible();

    await act(async () => {
      stalePoll.resolve({ agents: [nova] });
      await stalePoll.promise;
    });
    expect(
      screen.getByRole('heading', { name: 'Set up your workspace' }),
    ).toBeVisible();
    expect(
      screen.queryByRole('heading', { name: 'Say something to Nova' }),
    ).not.toBeInTheDocument();
  });

  it('keeps the last-known shell after a late poll failure', async () => {
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
    expect(screen.getByText('Welcome back')).toBeVisible();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });

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
});
