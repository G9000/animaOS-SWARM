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
import {
  DaemonTooOldError,
  type Session,
  type SessionMessage,
} from '@animaOS-SWARM/sdk';

import { toolNamesForProfile } from './lib/agent-access';
import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
} from './lib/daemon-api';
import { SESSION_MESSAGES_POLL_MS } from './hooks/useSessionMessages';
import { sessionFixture } from './test/sessions';
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

async function openChat() {
  fireEvent.click(await screen.findByRole('button', { name: 'New chat' }));
}

const readOnly = {
  send: false,
  steer: false,
  stop: true,
  rename: false,
  archive: true,
  delete: false,
  compact: false,
  export: true,
};

interface SessionRoutes {
  sessions: Session[];
}

let routes: SessionRoutes;

/** In-memory session routes: created chats are listed as `chat:new-<n>`. */
function mockSessionRoutes(): SessionRoutes {
  const state: SessionRoutes = { sessions: [] };
  let created = 0;
  vi.spyOn(daemon, 'listSessions').mockImplementation(async () => ({
    sessions: [...state.sessions],
    nextCursor: null,
  }));
  vi.spyOn(daemon, 'getSession').mockImplementation(
    async (_agentId, sessionId) => {
      const found = state.sessions.find((item) => item.id === sessionId);
      if (!found) throw Object.assign(new Error('not found'), { status: 404 });
      return found;
    },
  );
  vi.spyOn(daemon, 'createSession').mockImplementation(async (agentId) => {
    created += 1;
    const session = sessionFixture(`chat:new-${created}`, { agentId });
    state.sessions.unshift(session);
    return session;
  });
  vi.spyOn(daemon, 'updateSession').mockImplementation(
    async (_agentId, sessionId, patch) => {
      const index = state.sessions.findIndex((item) => item.id === sessionId);
      const updated: Session = {
        ...state.sessions[index],
        ...patch,
        unread:
          patch.lastReadAtMs !== undefined
            ? false
            : state.sessions[index].unread,
      };
      state.sessions[index] = updated;
      return updated;
    },
  );
  vi.spyOn(daemon, 'deleteSession').mockImplementation(
    async (_agentId, sessionId) => {
      state.sessions = state.sessions.filter((item) => item.id !== sessionId);
    },
  );
  vi.spyOn(daemon, 'sessionMessages').mockResolvedValue({
    messages: [],
    nextBefore: null,
  });
  return state;
}

/** Changes a mocked session's derived fields, such as its active runs. */
function setSessionFields(sessionId: string, patch: Partial<Session>) {
  routes.sessions = routes.sessions.map((item) =>
    item.id === sessionId ? { ...item, ...patch } : item,
  );
}

function sessionReads(sessionId: string): number {
  return vi
    .mocked(daemon.getSession)
    .mock.calls.filter(([, id]) => id === sessionId).length;
}

/** Session messages read from an agent snapshot's room, like the daemon route. */
function messagesFromSnapshot(snapshotOf: () => DaemonSnapshot) {
  vi.spyOn(daemon, 'sessionMessages').mockImplementation(
    async (_agentId, sessionId) => ({
      messages: snapshotOf()
        .messages.filter((message) => message.roomId === sessionId)
        .map(
          (message): SessionMessage => ({
            id: message.id,
            role: message.role as SessionMessage['role'],
            text: message.content.text,
            attachments: [],
            metadata: message.content.metadata ?? {},
            createdAtMs: message.createdAtMs,
          }),
        ),
      nextBefore: null,
    }),
  );
}

function mockProviders() {
  vi.spyOn(daemon, 'listProviders').mockResolvedValue({ providers });
}

/** Fake timers that keep pace with real time, so user-event and findBy work
 *  as usual, while `elapse` runs the polls that are due at once. */
function fakeClock() {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  return userEvent.setup({
    advanceTimers: (ms) => vi.advanceTimersByTime(ms),
  });
}

async function elapse(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

it('opens the main companion without automatically executing a prepared assignment', async () => {
  const alpha = snapshot('alpha', 'Alpha', 1);
  alpha.state.config.system =
    'Prepared first assignment (do not start until the owner asks):\\nDraft a weekly plan.';
  const beta = snapshot('beta', 'Beta', 2);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [alpha, beta] });
  mockProviders();
  const send = vi.spyOn(daemon, 'runAgent');
  render(<ViewHarness />);
  expect(await screen.findByPlaceholderText('Message Alpha…')).toBeVisible();
  expect(send).not.toHaveBeenCalled();
  expect(
    screen.queryByRole('button', { name: 'Team' }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Message Beta' }),
  ).not.toBeInTheDocument();
});

it('keeps the companion draft and failed send while opening Work', async () => {
  const user = userEvent.setup();
  const alpha = snapshot('alpha', 'Alpha', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [alpha] });
  mockProviders();
  const run = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
  vi.spyOn(daemon, 'runAgent').mockReturnValue(run.promise);
  render(<ViewHarness />);
  await user.type(
    await screen.findByPlaceholderText('Message Alpha…'),
    'Alpha request',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  await act(async () => {
    run.reject(new Error('Alpha disconnected'));
  });
  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  await user.click(
    await screen.findByRole('button', { name: 'Restore message' }),
  );
  expect(screen.getByPlaceholderText('Message Alpha…')).toHaveValue(
    'Alpha request',
  );
});

function withMessage(
  source: DaemonSnapshot,
  text: string,
  roomId = `room-${source.state.id}`,
): DaemonSnapshot {
  const updated = structuredClone(source);
  updated.messages = [
    {
      id: `message-${source.state.id}`,
      agentId: source.state.id,
      roomId,
      role: 'assistant',
      content: { text },
      createdAtMs: source.state.createdAtMs + 1,
    },
  ];
  updated.messageCount = 1;
  return updated;
}

it('retains a completed reply after opening Work and keeps settings on the companion', async () => {
  const user = userEvent.setup();
  const alpha = snapshot('alpha', 'Alpha', 1);
  const beta = snapshot('beta', 'Beta', 2);
  let current = alpha;
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [alpha, beta] });
  mockProviders();
  messagesFromSnapshot(() => current);
  const run = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
  vi.spyOn(daemon, 'runAgent').mockReturnValue(run.promise);
  render(<ViewHarness />);
  await user.type(
    await screen.findByPlaceholderText('Message Alpha…'),
    'Alpha request',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  current = withMessage(alpha, 'Alpha finished', 'chat:new-1');
  await act(async () =>
    run.resolve({
      agent: current,
      result: {
        status: 'success',
        durationMs: 1,
        data: { text: 'Alpha finished' },
      },
    }),
  );
  expect(await screen.findByText('Alpha finished')).not.toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  expect(
    within(screen.getByRole('dialog')).getByDisplayValue('Alpha'),
  ).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Close settings' }));
  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  expect(screen.getByText('Alpha finished')).toBeVisible();
});

it('lists peer requests as read-only helper sessions apart from the owner chat', async () => {
  const user = userEvent.setup();
  const alpha = withMessage(
    snapshot('alpha', 'Alpha', 1),
    'Owner reply',
    'chat:owner',
  );
  alpha.messages.push({
    id: 'peer-message',
    agentId: 'alpha',
    roomId: 'peer:beta:alpha',
    role: 'user',
    content: {
      text: 'Private teammate request',
      metadata: {
        communication: {
          kind: 'peer',
          fromAgentId: 'beta',
          toAgentId: 'alpha',
        },
      },
    },
    createdAtMs: 3,
  });
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [alpha, snapshot('beta', 'Beta', 2)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:owner', {
      agentId: 'alpha',
      title: 'Owner chat',
      lastActivityAtMs: Date.now(),
    }),
    sessionFixture('peer:beta:alpha', {
      agentId: 'alpha',
      kind: 'helper',
      origin: 'peer',
      title: 'Messages from Beta',
      parentAgentId: 'beta',
      capabilities: readOnly,
      lastActivityAtMs: Date.now() - 1,
    }),
  );
  messagesFromSnapshot(() => alpha);
  window.history.replaceState(null, '', '/#/s/chat%3Aowner');
  render(<ViewHarness />);
  await screen.findByText('Owner reply');
  expect(
    within(screen.getByLabelText('Conversation with Alpha')).queryByText(
      'Private teammate request',
    ),
  ).not.toBeInTheDocument();
  await user.click(
    await screen.findByRole('button', { name: 'Messages from Beta' }),
  );
  expect(await screen.findByText('Private teammate request')).toBeVisible();
  expect(screen.getByRole('note')).toHaveTextContent(
    'Helper sessions are read-only.',
  );
  expect(
    screen.queryByPlaceholderText('Message Alpha…'),
  ).not.toBeInTheDocument();
});

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
  vi.spyOn(daemon, 'agentTasks').mockResolvedValue({
    tasks: [],
    revision: '1',
  });
  vi.spyOn(daemon, 'listConnectors').mockResolvedValue({ connectors: [] });
  vi.spyOn(daemon, 'listSchedules').mockResolvedValue({ schedules: [] });
  vi.spyOn(daemon, 'importLegacySchedules').mockResolvedValue({
    schedules: [],
  });
  routes = mockSessionRoutes();
});

afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
  sessionStorage.clear();
  window.history.replaceState(null, '', '/');
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
    await openChat();
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
      await screen.findByRole('heading', { name: 'Set up your companion' }),
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
    await openChat();

    expect(
      await screen.findByRole('heading', { name: 'Say something to Alpha' }),
    ).toBeVisible();
    await user.type(screen.getByPlaceholderText('Message Alpha…'), 'Hello');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() =>
      expect(runAgent).toHaveBeenCalledWith(
        'agent-a',
        'Hello',
        expect.objectContaining({ clientRequestId: expect.any(String) }),
        'chat:new-1',
      ),
    );
    expect(daemon.createSession).toHaveBeenCalledWith('agent-a');

    expect(screen.getByText('Companion')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Message Beta' }),
    ).not.toBeInTheDocument();

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
    await openChat();

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
    let current = next;
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [next, first] });
    mockProviders();
    vi.spyOn(daemon, 'deleteAgent').mockResolvedValue({ deleted: true });
    messagesFromSnapshot(() => current);
    const runAgent = vi
      .spyOn(daemon, 'runAgent')
      .mockImplementation(async (_id, _text, _metadata, roomId) => {
        current = withMessage(next, 'Next is responsive', roomId);
        return {
          agent: current,
          result: {
            status: 'success',
            durationMs: 1,
            data: { text: 'Next is responsive' },
          },
        };
      });
    const removeItem = vi
      .spyOn(Storage.prototype, 'removeItem')
      .mockImplementation(() => {
        throw new DOMException('Storage access denied', 'SecurityError');
      });

    render(<ViewHarness />);
    await openChat();

    await screen.findByRole('heading', { name: 'Say something to First' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Say something to Next' }),
    ).toBeVisible();
    expect(removeItem).toHaveBeenCalledWith('animaos.checkins.agent-first');
    await user.type(screen.getByPlaceholderText('Message Next…'), 'Continue');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() =>
      expect(runAgent).toHaveBeenCalledWith(
        'agent-next',
        'Continue',
        expect.objectContaining({ clientRequestId: expect.any(String) }),
        'chat:new-1',
      ),
    );
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
    routes.sessions.push(
      sessionFixture('room-1', { title: 'Earlier chat', origin: 'api' }),
    );
    messagesFromSnapshot(() => nova);
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockResolvedValue({ agent: updated });
    window.history.replaceState(null, '', '/#/s/room-1');

    render(<ViewHarness />);

    await screen.findByText('Existing conversation');
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Nova Prime');
    const provider = screen.getByRole('combobox', { name: 'Provider' });
    const model = screen.getByRole('combobox', { name: 'Model' });
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
    expect(
      screen.getByRole('heading', {
        name: 'Nova Prime',
        exact: true,
        hidden: true,
      }),
    ).toBeVisible();
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
    await openChat();

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
    await openChat();

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
      await screen.findByRole('heading', { name: 'Set up your companion' }),
    ).toBeVisible();
  });

  it('recovers a failed message without overwriting a newer draft or sending automatically', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    const run = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    const send = vi.spyOn(daemon, 'runAgent').mockReturnValue(run.promise);
    render(<ViewHarness />);
    await openChat();
    const input = await screen.findByPlaceholderText('Message Nova…');
    await user.type(input, 'Original request');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await user.type(input, 'New thought');
    await act(async () => {
      run.reject(new Error('Connection lost'));
    });
    await user.click(
      await screen.findByRole('button', { name: 'Restore message' }),
    );
    expect(input).toHaveValue('New thought\n\nOriginal request');
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('keeps every failed message until explicitly restored or dismissed', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    vi.spyOn(daemon, 'runAgent').mockRejectedValue(new Error('Network failed'));
    render(<ViewHarness />);
    await openChat();
    const input = await screen.findByPlaceholderText('Message Nova…');
    await user.type(input, 'First');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await screen.findByRole('button', { name: 'Restore message' });
    await user.type(input, 'Second');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await user.click(
      await screen.findByRole('button', { name: 'Restore message' }),
    );
    expect(input).toHaveValue('First');
    await user.click(screen.getByRole('button', { name: 'Restore message' }));
    expect(input).toHaveValue('First\n\nSecond');
  });

  it('recovers a send failure even when settings were saved during the request', async () => {
    const user = userEvent.setup();
    const nova = snapshot('agent-main', 'Nova', 1);
    const updated = snapshot('agent-main', 'Nova Prime', 1);
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [nova] });
    mockProviders();
    vi.spyOn(daemon, 'updateAgent').mockResolvedValue({ agent: updated });
    const run = deferred<Awaited<ReturnType<typeof daemon.runAgent>>>();
    vi.spyOn(daemon, 'runAgent').mockReturnValue(run.promise);
    render(<ViewHarness />);
    await openChat();
    await user.type(
      await screen.findByPlaceholderText('Message Nova…'),
      'Keep this request',
    );
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const nameField = screen.getByDisplayValue('Nova');
    await user.clear(nameField);
    await user.type(nameField, 'Nova Prime');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() =>
      expect(
        screen.getByRole('heading', { name: 'Nova Prime', hidden: true }),
      ).toBeInTheDocument(),
    );
    await user.click(screen.getByRole('button', { name: 'Close settings' }));
    await act(async () => {
      run.reject(new Error('Connection lost'));
    });
    await user.click(
      await screen.findByRole('button', { name: 'Restore message' }),
    );
    expect(screen.getByPlaceholderText('Message Nova Prime…')).toHaveValue(
      'Keep this request',
    );
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
    await openChat();

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
    await openChat();

    await screen.findByRole('heading', { name: 'Say something to Nova' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Unsaved Nova');
    const provider = screen.getByRole('combobox', { name: 'Provider' });
    const model = screen.getByRole('combobox', { name: 'Model' });
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
    expect(screen.getByRole('combobox', { name: 'Provider' })).toHaveValue(
      'anthropic',
    );
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
    routes.sessions.push(
      sessionFixture('room-1', { title: 'Earlier chat', origin: 'api' }),
    );
    messagesFromSnapshot(() => nova);
    const updateAgent = vi
      .spyOn(daemon, 'updateAgent')
      .mockReturnValue(update.promise);
    window.history.replaceState(null, '', '/#/s/room-1');

    render(<ViewHarness />);

    await screen.findByText('Existing conversation');
    expect(screen.getByText('Welcome back')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Unsaved Nova');
    const provider = screen.getByRole('combobox', { name: 'Provider' });
    const model = screen.getByRole('combobox', { name: 'Model' });
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
    expect(screen.getByRole('combobox', { name: 'Provider' })).toHaveValue(
      'anthropic',
    );
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
    await openChat();

    await screen.findByRole('heading', { name: 'Say something to Only' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Set up your companion' }),
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
    await openChat();

    await screen.findByRole('heading', { name: 'Say something to Only' });
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));

    expect(
      await screen.findByRole('heading', { name: 'Set up your companion' }),
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
    await openChat();

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
      await screen.findByRole('heading', { name: 'Set up your companion' }),
    ).toBeVisible();

    await act(async () => {
      stalePoll.resolve({ agents: [nova] });
      await stalePoll.promise;
    });
    expect(
      screen.getByRole('heading', { name: 'Set up your companion' }),
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
    fireEvent.click(screen.getByRole('button', { name: 'New chat' }));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });

    expect(
      screen.getByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    expect(
      screen.getByRole('navigation', { name: 'Workspace navigation' }),
    ).toBeVisible();
    expect(daemon.listAgents).toHaveBeenCalledTimes(2);
    const runAgent = vi.spyOn(daemon, 'runAgent');
    const input = screen.getByPlaceholderText('Message Nova…');
    fireEvent.change(input, { target: { value: 'Keep drafting offline' } });
    expect(input).toHaveValue('Keep drafting offline');
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(runAgent).not.toHaveBeenCalled();
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
    await openChat();
    await screen.findByRole('heading', { name: 'Say something to Alpha' });
    await user.type(
      screen.getByPlaceholderText('Message Alpha…'),
      'Alpha work',
    );
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() =>
      expect(daemon.runAgent).toHaveBeenCalledWith(
        'agent-a',
        'Alpha work',
        expect.objectContaining({ clientRequestId: expect.any(String) }),
        'chat:new-1',
      ),
    );

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
    expect(screen.getByPlaceholderText('Message Beta…')).toBeVisible();
    expect(
      screen.queryByPlaceholderText('Message Alpha…'),
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
    await openChat();
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
    expect(
      screen.queryByPlaceholderText('Message Alpha draft…'),
    ).not.toBeInTheDocument();
  });
});

it('reconciles a timed-out send with its saved request ID without offering a duplicate retry', async () => {
  const user = userEvent.setup();
  let current = snapshot('agent-main', 'Nova', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  messagesFromSnapshot(() => current);
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (id, text, metadata, roomId) => {
      current = withMessage(
        snapshot(id, 'Nova', 1),
        'Completed despite timeout',
        roomId,
      );
      current.state.status = 'completed';
      current.messages.unshift({
        id: 'request',
        agentId: id,
        roomId: roomId ?? '',
        role: 'user',
        content: { text, metadata },
        createdAtMs: 2,
      });
      throw Object.assign(new Error('daemon request failed (408)'), {
        status: 408,
      });
    });
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Commit',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await screen.findByText('Completed despite timeout');
  await waitFor(() =>
    expect(
      screen.queryByRole('button', { name: 'Restore message' }),
    ).not.toBeInTheDocument(),
  );
  expect(screen.queryByText(/response timed out/)).not.toBeInTheDocument();
  expect(run).toHaveBeenCalledTimes(1);
});

it('does not mistake an older identical message for the timed-out request', async () => {
  const user = userEvent.setup();
  const current = snapshot('agent-main', 'Nova', 1);
  current.messages.push({
    id: 'old',
    agentId: current.state.id,
    role: 'user',
    content: { text: 'Commit', metadata: { clientRequestId: 'older-request' } },
    createdAtMs: 1,
  });
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [current] });
  mockProviders();
  vi.spyOn(daemon, 'runAgent').mockRejectedValue(
    Object.assign(new Error('timeout'), { status: 408 }),
  );
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Commit',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  expect(
    await screen.findByRole('button', { name: 'Restore message' }),
  ).toBeVisible();
});

it('keeps a timed-out running request locked until the daemon confirms its completion', async () => {
  const user = fakeClock();
  let current = snapshot('agent-main', 'Nova', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  messagesFromSnapshot(() => current);
  let requestMetadata: Record<string, unknown> | undefined;
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (_id, _text, metadata, roomId) => {
      requestMetadata = metadata;
      current = { ...current, state: { ...current.state, status: 'running' } };
      // The run keeps going in its session after the request times out.
      setSessionFields(roomId ?? '', { activeRuns: 1 });
      throw Object.assign(new Error('timeout'), { status: 408 });
    });
  render(<ViewHarness />);
  await openChat();
  const input = await screen.findByPlaceholderText('Message Nova…');
  await user.type(input, 'Long work');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await screen.findByText(/Checking the daemon for completion/);
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
  current = withMessage(
    snapshot('agent-main', 'Nova', 1),
    'Long work completed',
    'chat:new-1',
  );
  current.state.status = 'completed';
  current.messages.unshift({
    id: 'long-request',
    agentId: current.state.id,
    roomId: 'chat:new-1',
    role: 'user',
    content: { text: 'Long work', metadata: requestMetadata },
    createdAtMs: 2,
  });
  setSessionFields('chat:new-1', { activeRuns: 0 });
  await elapse(SESSION_MESSAGES_POLL_MS);
  await screen.findByText('Long work completed');
  await waitFor(() =>
    expect(
      screen.queryByText(/Checking the daemon for completion/),
    ).not.toBeInTheDocument(),
  );
  expect(run).toHaveBeenCalledTimes(1);
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
});

it('does not keep a timed-out send locked while a check-in runs in another session', async () => {
  const user = userEvent.setup();
  // The check-in's run makes the agent as a whole report running.
  const busy = snapshot('agent-main', 'Nova', 1);
  busy.state.status = 'running';
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [busy] });
  mockProviders();
  routes.sessions.push(
    sessionFixture('schedule:daily', {
      kind: 'checkin',
      origin: 'schedule',
      title: 'Daily check-in',
      activeRuns: 1,
      lastActivityAtMs: Date.now(),
    }),
  );
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockRejectedValue(Object.assign(new Error('timeout'), { status: 408 }));
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Plan my week',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(
    await screen.findByRole('button', { name: 'Restore message' }),
  ).toBeVisible();
  expect(screen.getByText(/has not confirmed this message/)).toBeVisible();
  expect(
    screen.queryByText(/Checking the daemon for completion/),
  ).not.toBeInTheDocument();
  expect(run).toHaveBeenCalledTimes(1);
});

it('does not declare a send unconfirmed while it waits behind another run in its room', async () => {
  const user = fakeClock();
  // The agent reads idle between runs; only the session knows its room is busy.
  let current = snapshot('agent-main', 'Nova', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-7', {
      title: 'Weekend plans',
      origin: 'api',
      lastActivityAtMs: Date.now(),
    }),
  );
  messagesFromSnapshot(() => current);
  let requestMetadata: Record<string, unknown> | undefined;
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (_id, _text, metadata) => {
      requestMetadata = metadata;
      // Another run in this room went first; this request is queued behind it.
      setSessionFields('room-7', { activeRuns: 1 });
      throw Object.assign(new Error('timeout'), { status: 408 });
    });
  window.history.replaceState(null, '', '/#/s/room-7');
  render(<ViewHarness />);
  const input = await screen.findByPlaceholderText('Message Nova…');
  await waitFor(() => expect(input).toBeEnabled());
  await user.type(input, 'Queued thought');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  await screen.findByText(/Checking the daemon for completion/);
  // The session is read again because its room is still busy.
  const reads = sessionReads('room-7');
  await elapse(SESSION_MESSAGES_POLL_MS);
  expect(sessionReads('room-7')).toBeGreaterThan(reads);
  expect(screen.queryByText(/has not confirmed/)).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();

  current = structuredClone(current);
  current.messages.push(
    {
      id: 'queued-request',
      agentId: 'agent-main',
      roomId: 'room-7',
      role: 'user',
      content: { text: 'Queued thought', metadata: requestMetadata },
      createdAtMs: 5,
    },
    {
      id: 'queued-reply',
      agentId: 'agent-main',
      roomId: 'room-7',
      role: 'assistant',
      content: { text: 'Answered after the queue' },
      createdAtMs: 6,
    },
  );
  setSessionFields('room-7', { activeRuns: 0 });
  await elapse(SESSION_MESSAGES_POLL_MS);
  expect(await screen.findByText('Answered after the queue')).toBeVisible();
  await waitFor(() =>
    expect(
      screen.queryByText(/Checking the daemon for completion/),
    ).not.toBeInTheDocument(),
  );
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
  // The record read when the send settled replaces the listed one at once.
  expect(input).toBeEnabled();
  expect(screen.queryByText('Nova is thinking')).not.toBeInTheDocument();
  expect(run).toHaveBeenCalledTimes(1);
});

it('does not clear a newer recovery entry when an older identical send is confirmed', async () => {
  const user = userEvent.setup();
  let current = snapshot('agent-main', 'Nova', 1);
  let firstMetadata: Record<string, unknown> | undefined;
  let calls = 0;
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  vi.spyOn(daemon, 'runAgent').mockImplementation(
    async (id, text, metadata) => {
      if (++calls === 1) {
        firstMetadata = metadata;
        throw new Error('Network lost');
      }
      current = snapshot(id, 'Nova', 1);
      current.messages.push({
        id: 'older',
        agentId: id,
        role: 'user',
        content: { text, metadata: firstMetadata },
        createdAtMs: 2,
      });
      throw Object.assign(new Error('timeout'), { status: 408 });
    },
  );
  render(<ViewHarness />);
  await openChat();
  const input = await screen.findByPlaceholderText('Message Nova…');
  await user.type(input, 'Commit');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(
    await screen.findByRole('button', { name: 'Dismiss recoverable message' }),
  );
  await user.type(input, 'Commit');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  expect(
    await screen.findByRole('button', { name: 'Restore message' }),
  ).toBeVisible();
  expect(screen.getByText(/1 recoverable message/)).toBeVisible();
});

it('opens an existing session from the sidebar, marks it read, and sends in its room', async () => {
  const user = userEvent.setup();
  let current = withMessage(
    snapshot('agent-main', 'Nova', 1),
    'Earlier answer',
    'room-7',
  );
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-7', {
      title: 'Weekend plans',
      origin: 'api',
      unread: true,
      lastActivityAtMs: Date.now(),
    }),
  );
  messagesFromSnapshot(() => current);
  const run = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (id, text, metadata, roomId) => {
      current = structuredClone(current);
      current.messages.push(
        {
          id: 'user-2',
          agentId: id,
          roomId: roomId ?? '',
          role: 'user',
          content: { text, metadata },
          createdAtMs: 3,
        },
        {
          id: 'reply-2',
          agentId: id,
          roomId: roomId ?? '',
          role: 'assistant',
          content: { text: 'Saturday works' },
          createdAtMs: 4,
        },
      );
      return {
        agent: current,
        result: {
          status: 'success',
          durationMs: 1,
          data: { text: 'Saturday works' },
        },
      };
    });
  render(<ViewHarness />);

  await user.click(
    await screen.findByRole('button', { name: 'Weekend plans, unread' }),
  );
  expect(await screen.findByText('Earlier answer')).toBeVisible();
  await waitFor(() =>
    expect(daemon.updateSession).toHaveBeenCalledWith('agent-main', 'room-7', {
      lastReadAtMs: 2,
    }),
  );
  await user.type(
    screen.getByPlaceholderText('Message Nova…'),
    'Does Saturday work?',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(run).toHaveBeenCalledWith(
    'agent-main',
    'Does Saturday work?',
    expect.objectContaining({ clientRequestId: expect.any(String) }),
    'room-7',
  );
  expect(daemon.createSession).not.toHaveBeenCalled();
  expect(await screen.findByText('Saturday works')).toBeVisible();
});

it('keeps the composer disabled until the open session record loads', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  // An archived legacy session is not listed, so its record is read on its own.
  const record = deferred<Session>();
  vi.mocked(daemon.getSession).mockReturnValue(record.promise);
  const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
    agent: snapshot('agent-main', 'Nova', 1),
    result: { status: 'success', durationMs: 1, data: { text: 'ok' } },
  });
  window.history.replaceState(null, '', '/#/s/legacy-room%3Aabc');
  render(<ViewHarness />);

  const input = await screen.findByPlaceholderText('Message Nova…');
  expect(input).toBeDisabled();
  await act(async () => {
    record.resolve(
      sessionFixture('legacy-room:abc', {
        roomId: 'direct:agent-main',
        title: 'Earlier chat',
        archived: true,
      }),
    );
  });
  await waitFor(() => expect(input).toBeEnabled());
  await user.type(input, 'Hello again');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(runAgent).toHaveBeenCalledWith(
    'agent-main',
    'Hello again',
    expect.objectContaining({ clientRequestId: expect.any(String) }),
    'direct:agent-main',
  );
});

it('retries an unlisted session record that fails to load and says why', async () => {
  fakeClock();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  // An archived legacy session is not listed, so its record is read on its own.
  vi.mocked(daemon.getSession)
    .mockRejectedValueOnce(
      Object.assign(new Error('daemon unavailable'), { status: 503 }),
    )
    .mockResolvedValue(
      sessionFixture('legacy-room:abc', {
        roomId: 'direct:agent-main',
        title: 'Earlier chat',
        archived: true,
      }),
    );
  window.history.replaceState(null, '', '/#/s/legacy-room%3Aabc');
  render(<ViewHarness />);

  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Session details could not be loaded: daemon unavailable',
  );
  const input = screen.getByPlaceholderText('Message Nova…');
  expect(input).toBeDisabled();

  await elapse(SESSION_MESSAGES_POLL_MS);
  await waitFor(() => expect(input).toBeEnabled());
  expect(
    screen.queryByText(/Session details could not be loaded/),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Earlier chat' })).toBeVisible();
});

it('shows why older messages could not be loaded', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:long', {
      title: 'Long history',
      lastActivityAtMs: Date.now(),
    }),
  );
  vi.mocked(daemon.sessionMessages).mockImplementation(
    async (_agentId, _sessionId, options = {}) => {
      if (options.before)
        throw Object.assign(new Error('history store is unavailable'), {
          status: 503,
        });
      return {
        messages: [
          {
            id: 'm2',
            role: 'assistant',
            text: 'Latest answer',
            attachments: [],
            metadata: {},
            createdAtMs: 2,
          },
        ],
        nextBefore: 'm2',
      };
    },
  );
  window.history.replaceState(null, '', '/#/s/chat%3Along');
  render(<ViewHarness />);

  await screen.findByText('Latest answer');
  await user.click(screen.getByRole('button', { name: 'Load older messages' }));

  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Messages could not be loaded: history store is unavailable',
  );
});

it('keeps each conversation draft across a reload', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-7', {
      title: 'Weekend plans',
      origin: 'api',
      lastActivityAtMs: Date.now(),
    }),
  );
  window.history.replaceState(null, '', '/#/s/room-7');
  const first = render(<ViewHarness />);
  const input = await screen.findByPlaceholderText('Message Nova…');
  await waitFor(() => expect(input).toBeEnabled());
  await user.type(input, 'Half-written thought');
  first.unmount();

  render(<ViewHarness />);
  expect(await screen.findByPlaceholderText('Message Nova…')).toHaveValue(
    'Half-written thought',
  );
  await user.click(screen.getByRole('button', { name: 'New chat' }));
  expect(screen.getByPlaceholderText('Message Nova…')).toHaveValue('');
});

it('keeps drafts in memory when session storage refuses them', async () => {
  const user = userEvent.setup();
  const setItem = Storage.prototype.setItem;
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
    this: Storage,
    key: string,
    value: string,
  ) {
    if (this === window.sessionStorage)
      throw new DOMException(
        'The quota has been exceeded.',
        'QuotaExceededError',
      );
    setItem.call(this, key, value);
  });
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-7', {
      title: 'Weekend plans',
      origin: 'api',
      lastActivityAtMs: Date.now(),
    }),
  );
  render(<ViewHarness />);

  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Keep this thought',
  );
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  expect(screen.getByPlaceholderText('Message Nova…')).toHaveValue(
    'Keep this thought',
  );
  await user.click(
    await screen.findByRole('button', { name: 'Weekend plans' }),
  );
  await waitFor(() =>
    expect(screen.getByPlaceholderText('Message Nova…')).toHaveValue(''),
  );
  await user.click(screen.getByRole('button', { name: 'New chat' }));
  expect(screen.getByPlaceholderText('Message Nova…')).toHaveValue(
    'Keep this thought',
  );
});

it('creates one session for the first send and moves text typed meanwhile into it', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  const creating = deferred<void>();
  const createSession = vi.mocked(daemon.createSession);
  const createListed = createSession.getMockImplementation()!;
  createSession.mockImplementationOnce(async (agentId) => {
    await creating.promise;
    return createListed(agentId);
  });
  const runAgent = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (id, text, _metadata, roomId) => {
      // The daemon titles a new chat from its first message.
      setSessionFields(roomId ?? '', { title: text });
      return {
        agent: snapshot(id, 'Nova', 1),
        result: { status: 'success', durationMs: 1, data: { text: 'ok' } },
      };
    });
  render(<ViewHarness />);
  await openChat();
  const input = await screen.findByPlaceholderText('Message Nova…');
  await user.type(input, 'First question');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.type(input, 'A follow-up{Enter}');

  await act(async () => creating.resolve());
  await waitFor(() => expect(window.location.hash).toBe('#/s/chat%3Anew-1'));
  expect(screen.getByPlaceholderText('Message Nova…')).toHaveValue(
    'A follow-up',
  );
  expect(createSession).toHaveBeenCalledTimes(1);
  expect(runAgent).toHaveBeenCalledTimes(1);
  expect(runAgent).toHaveBeenCalledWith(
    'agent-main',
    'First question',
    expect.objectContaining({ clientRequestId: expect.any(String) }),
    'chat:new-1',
  );
  await screen.findByRole('button', { name: 'First question' });
  await user.click(screen.getByRole('button', { name: 'New chat' }));
  expect(screen.getByPlaceholderText('Message Nova…')).toHaveValue('');
});

it('stays in a session opened while the first send was creating its chat', async () => {
  const user = userEvent.setup();
  const current = withMessage(
    snapshot('agent-main', 'Nova', 1),
    'Earlier answer',
    'room-7',
  );
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [current] });
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-7', {
      title: 'Weekend plans',
      origin: 'api',
      lastActivityAtMs: Date.now(),
    }),
  );
  messagesFromSnapshot(() => current);
  const creating = deferred<void>();
  const createSession = vi.mocked(daemon.createSession);
  const createListed = createSession.getMockImplementation()!;
  createSession.mockImplementationOnce(async (agentId) => {
    await creating.promise;
    return createListed(agentId);
  });
  const runAgent = vi.spyOn(daemon, 'runAgent').mockResolvedValue({
    agent: current,
    result: { status: 'success', durationMs: 1, data: { text: 'ok' } },
  });
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Plan the launch',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(
    await screen.findByRole('button', { name: 'Weekend plans' }),
  );

  await act(async () => creating.resolve());
  await waitFor(() =>
    expect(runAgent).toHaveBeenCalledWith(
      'agent-main',
      'Plan the launch',
      expect.objectContaining({ clientRequestId: expect.any(String) }),
      'chat:new-1',
    ),
  );
  expect(window.location.hash).toBe('#/s/room-7');
  expect(screen.getByText('Earlier answer')).toBeVisible();
});

it('opens the new session on a page that still shows the conversation', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  vi.spyOn(daemon, 'runAgent').mockResolvedValue({
    agent: snapshot('agent-main', 'Nova', 1),
    result: { status: 'success', durationMs: 1, data: { text: 'ok' } },
  });
  // Approvals arrives in a later release; until then it shows the chat.
  window.history.replaceState(null, '', '/#/approvals');
  render(<ViewHarness />);

  await user.type(await screen.findByPlaceholderText('Message Nova…'), 'Hello');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  // A reload of the page must find the session, not a new chat.
  await waitFor(() => expect(window.location.hash).toBe('#/s/chat%3Anew-1'));
});

it('keeps a page open when the first send creates its session, then returns to that session', async () => {
  const user = userEvent.setup();
  let current = snapshot('agent-main', 'Nova', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockImplementation(async () => ({
    agents: [current],
  }));
  mockProviders();
  messagesFromSnapshot(() => current);
  const creating = deferred<void>();
  const createSession = vi.mocked(daemon.createSession);
  const createListed = createSession.getMockImplementation()!;
  createSession.mockImplementationOnce(async (agentId) => {
    await creating.promise;
    return createListed(agentId);
  });
  const runAgent = vi
    .spyOn(daemon, 'runAgent')
    .mockImplementation(async (id, _text, _metadata, roomId) => {
      current = withMessage(
        snapshot(id, 'Nova', 1),
        'Launch plan ready',
        roomId,
      );
      return {
        agent: current,
        result: { status: 'success', durationMs: 1, data: { text: 'ok' } },
      };
    });
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Plan the launch',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));

  await act(async () => creating.resolve());
  await waitFor(() =>
    expect(runAgent).toHaveBeenCalledWith(
      'agent-main',
      'Plan the launch',
      expect.objectContaining({ clientRequestId: expect.any(String) }),
      'chat:new-1',
    ),
  );
  expect(window.location.hash).toBe('#/work');
  expect(
    screen.getByRole('button', { name: 'Work', exact: true }),
  ).toHaveAttribute('aria-current', 'page');

  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  expect(window.location.hash).toBe('#/s/chat%3Anew-1');
  expect(await screen.findByText('Launch plan ready')).toBeVisible();
});

it('asks for a daemon update instead of failing sends when sessions are missing', async () => {
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  vi.mocked(daemon.listSessions).mockRejectedValue(
    new DaemonTooOldError('agent-main'),
  );
  const runAgent = vi.spyOn(daemon, 'runAgent');
  render(<ViewHarness />);

  expect(await screen.findByText('Update the daemon')).toBeVisible();
  const input = screen.getByPlaceholderText('Message Nova…');
  expect(input).toBeDisabled();
  fireEvent.change(input, { target: { value: 'Hello' } });
  fireEvent.keyDown(input, { key: 'Enter' });
  expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  expect(daemon.createSession).not.toHaveBeenCalled();
  expect(runAgent).not.toHaveBeenCalled();
});

it('keeps an opened page when the open session finishes deleting behind it', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:old', {
      title: 'Old plan',
      lastActivityAtMs: Date.now(),
    }),
  );
  const deletion = deferred<void>();
  const deleteSession = vi.mocked(daemon.deleteSession);
  const deleteListed = deleteSession.getMockImplementation()!;
  deleteSession.mockImplementationOnce(async (agentId, sessionId) => {
    await deletion.promise;
    return deleteListed(agentId, sessionId);
  });
  window.history.replaceState(null, '', '/#/s/chat%3Aold');
  render(<ViewHarness />);

  await user.click(
    await screen.findByRole('button', { name: 'Actions for Old plan' }),
  );
  await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
  await user.click(screen.getByRole('menuitem', { name: 'Delete session' }));
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  await act(async () => deletion.resolve());

  expect(window.location.hash).toBe('#/work');
  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  expect(window.location.hash).toBe('#/');
  expect(
    await screen.findByRole('heading', { name: 'Say something to Nova' }),
  ).toBeVisible();
});

it('keeps the open session while a sidebar search filters it out', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('schedule:daily', {
      kind: 'checkin',
      origin: 'schedule',
      title: 'Daily check-in',
      capabilities: readOnly,
      lastActivityAtMs: Date.now(),
    }),
  );
  // A search lists only matches, and the record cannot be read on its own now.
  vi.mocked(daemon.listSessions).mockImplementation(
    async (_agentId, options = {}) => ({
      sessions: options.q ? [] : [...routes.sessions],
      nextCursor: null,
    }),
  );
  vi.mocked(daemon.getSession).mockRejectedValue(
    Object.assign(new Error('history store is unavailable'), { status: 503 }),
  );
  window.history.replaceState(null, '', '/#/s/schedule%3Adaily');
  render(<ViewHarness />);

  expect(
    await screen.findByRole('heading', { name: 'Daily check-in' }),
  ).toBeVisible();
  await user.type(
    screen.getByRole('searchbox', { name: 'Search sessions' }),
    'budget',
  );
  await screen.findByText('No sessions match.');

  expect(screen.getByRole('heading', { name: 'Daily check-in' })).toBeVisible();
  expect(screen.getByRole('note')).toHaveTextContent(
    'Replying to a check-in is not available yet.',
  );
  expect(
    screen.queryByPlaceholderText('Message Nova…'),
  ).not.toBeInTheDocument();
});

it('shows a failed header action in the session view', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:old', {
      title: 'Old plan',
      lastActivityAtMs: Date.now(),
    }),
  );
  vi.mocked(daemon.updateSession).mockRejectedValue(
    new Error('archive refused'),
  );
  window.history.replaceState(null, '', '/#/s/chat%3Aold');
  render(<ViewHarness />);

  // On mobile the sidebar sits in a closed drawer, so the view says it too.
  const view = await screen.findByRole('region', { name: 'Old plan' });
  await user.click(within(view).getByRole('button', { name: 'Archive' }));
  expect(await within(view).findByRole('alert')).toHaveTextContent(
    'archive refused',
  );
});

it('shows a failed session action in the sidebar instead of rejecting', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:old', {
      title: 'Old plan',
      lastActivityAtMs: Date.now(),
    }),
  );
  vi.mocked(daemon.updateSession).mockRejectedValue(
    new Error('archive refused'),
  );
  render(<ViewHarness />);

  await user.click(
    await screen.findByRole('button', { name: 'Actions for Old plan' }),
  );
  await user.click(screen.getByRole('menuitem', { name: 'Archive' }));

  expect(
    await within(
      screen.getByRole('navigation', { name: 'Sessions' }),
    ).findByRole('alert'),
  ).toHaveTextContent('archive refused');
});

it('marks a session read only once it is back on screen', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-7', {
      title: 'Weekend plans',
      origin: 'api',
      unread: true,
      lastActivityAtMs: Date.now(),
    }),
  );
  const page = deferred<Awaited<ReturnType<typeof daemon.sessionMessages>>>();
  vi.mocked(daemon.sessionMessages).mockReturnValueOnce(page.promise);
  window.history.replaceState(null, '', '/#/s/room-7');
  render(<ViewHarness />);

  await screen.findByRole('button', { name: 'Weekend plans, unread' });
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  await act(async () =>
    page.resolve({
      messages: [
        {
          id: 'm1',
          role: 'assistant',
          text: 'Saturday works',
          attachments: [],
          metadata: {},
          createdAtMs: 2,
        },
      ],
      nextBefore: null,
    }),
  );
  expect(await screen.findByText('Saturday works')).not.toBeVisible();
  expect(daemon.updateSession).not.toHaveBeenCalled();

  await user.click(screen.getByRole('button', { name: 'Open companion chat' }));
  await waitFor(() =>
    expect(daemon.updateSession).toHaveBeenCalledWith('agent-main', 'room-7', {
      lastReadAtMs: 2,
    }),
  );
});

it.each([
  {
    action: 'rename',
    fail: () =>
      vi
        .mocked(daemon.updateSession)
        .mockRejectedValue(new Error('rename refused')),
    run: async (user: ReturnType<typeof userEvent.setup>) => {
      await user.click(screen.getByRole('menuitem', { name: 'Rename' }));
      const field = screen.getByRole('textbox', { name: 'Rename Old plan' });
      await user.clear(field);
      await user.type(field, 'New plan');
      await user.click(screen.getByRole('button', { name: 'Save' }));
    },
    message: 'rename refused',
    // The rename stays open with the typed title, ready to retry.
    kept: () =>
      expect(
        screen.getByRole('textbox', { name: 'Rename Old plan' }),
      ).toHaveValue('New plan'),
  },
  {
    action: 'export',
    fail: () =>
      vi
        .spyOn(daemon, 'exportSession')
        .mockRejectedValue(new Error('export refused')),
    run: async (user: ReturnType<typeof userEvent.setup>) => {
      await user.click(
        screen.getByRole('menuitem', { name: 'Export Markdown' }),
      );
    },
    message: 'export refused',
    kept: () =>
      expect(
        screen.getByRole('button', { name: 'Old plan' }),
      ).toBeInTheDocument(),
  },
  {
    action: 'delete',
    fail: () =>
      vi
        .mocked(daemon.deleteSession)
        .mockRejectedValue(new Error('delete refused')),
    run: async (user: ReturnType<typeof userEvent.setup>) => {
      await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
      await user.click(
        screen.getByRole('menuitem', { name: 'Delete session' }),
      );
    },
    message: 'delete refused',
    kept: () =>
      expect(
        screen.getByRole('button', { name: 'Old plan' }),
      ).toBeInTheDocument(),
  },
])(
  'shows a failed sidebar $action instead of rejecting',
  async ({ fail, run, message, kept }) => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents').mockResolvedValue({
      agents: [snapshot('agent-main', 'Nova', 1)],
    });
    mockProviders();
    routes.sessions.push(
      sessionFixture('chat:old', {
        title: 'Old plan',
        lastActivityAtMs: Date.now(),
      }),
    );
    fail();
    render(<ViewHarness />);

    await user.click(
      await screen.findByRole('button', { name: 'Actions for Old plan' }),
    );
    await run(user);

    const sidebar = screen.getByRole('navigation', { name: 'Sessions' });
    expect(await within(sidebar).findByRole('alert')).toHaveTextContent(
      message,
    );
    kept();
  },
);

it('keeps each session’s own messages and draft when switching between them', async () => {
  const user = userEvent.setup();
  const current = snapshot('agent-main', 'Nova', 1);
  current.messages = [
    {
      id: 'a1',
      agentId: 'agent-main',
      roomId: 'chat:a',
      role: 'assistant',
      content: { text: 'Answer in A' },
      createdAtMs: 2,
    },
    {
      id: 'b1',
      agentId: 'agent-main',
      roomId: 'chat:b',
      role: 'assistant',
      content: { text: 'Answer in B' },
      createdAtMs: 3,
    },
  ];
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [current] });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:a', { title: 'Plan A', lastActivityAtMs: Date.now() }),
    sessionFixture('chat:b', {
      title: 'Plan B',
      lastActivityAtMs: Date.now() - 1,
    }),
  );
  messagesFromSnapshot(() => current);
  render(<ViewHarness />);
  const composer = () => screen.getByPlaceholderText('Message Nova…');

  await user.click(await screen.findByRole('button', { name: 'Plan A' }));
  expect(await screen.findByText('Answer in A')).toBeVisible();
  await waitFor(() => expect(composer()).toBeEnabled());
  await user.type(composer(), 'Draft for A');

  await user.click(screen.getByRole('button', { name: 'Plan B' }));
  expect(await screen.findByText('Answer in B')).toBeVisible();
  expect(screen.queryByText('Answer in A')).not.toBeInTheDocument();
  expect(composer()).toHaveValue('');
  await user.type(composer(), 'Draft for B');

  await user.click(screen.getByRole('button', { name: 'Plan A' }));
  expect(await screen.findByText('Answer in A')).toBeVisible();
  expect(screen.queryByText('Answer in B')).not.toBeInTheDocument();
  expect(composer()).toHaveValue('Draft for A');
  await user.click(screen.getByRole('button', { name: 'Plan B' }));
  expect(await screen.findByText('Answer in B')).toBeVisible();
  expect(composer()).toHaveValue('Draft for B');
});

it('replies to a Telegram session through its connector', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  vi.spyOn(daemon, 'listConnectors').mockResolvedValue({
    connectors: [
      {
        id: 'tg-1',
        agentId: 'agent-main',
        roomId: 'telegram:tg-1',
        type: 'telegram',
        bot: { id: '1', username: 'nova_bot', displayName: 'Nova' },
        approvedChat: null,
        pendingPairing: null,
        status: 'ready',
        enabled: true,
        createdAtMs: 1,
        updatedAtMs: 1,
      },
    ],
  });
  routes.sessions.push(
    sessionFixture('telegram:tg-1', {
      kind: 'telegram',
      origin: 'telegram',
      title: 'Telegram · @nova_bot',
      lastActivityAtMs: Date.now(),
    }),
  );
  const runAgent = vi.spyOn(daemon, 'runAgent');
  const reply = vi.spyOn(daemon, 'sendConnectorMessage').mockResolvedValue({
    messages: [],
    result: { status: 'success', durationMs: 1 },
    deliveryQueued: true,
  });
  window.history.replaceState(null, '', '/#/s/telegram%3Atg-1');
  render(<ViewHarness />);

  await user.type(
    await screen.findByPlaceholderText('Reply on Telegram…'),
    'On my way',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(reply).toHaveBeenCalledWith(
    'agent-main',
    'tg-1',
    'On my way',
    expect.stringMatching(/^telegram-/),
  );
  expect(runAgent).not.toHaveBeenCalled();
});

/** A Telegram session with its ready connector, opened in the harness. */
async function openTelegramSession() {
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  vi.spyOn(daemon, 'listConnectors').mockResolvedValue({
    connectors: [
      {
        id: 'tg-1',
        agentId: 'agent-main',
        roomId: 'telegram:tg-1',
        type: 'telegram',
        bot: { id: '1', username: 'nova_bot', displayName: 'Nova' },
        approvedChat: {
          id: '42',
          kind: 'private',
          title: null,
          username: 'owner',
        },
        pendingPairing: null,
        status: 'ready',
        enabled: true,
        createdAtMs: 1,
        updatedAtMs: 1,
      },
    ],
  });
  routes.sessions.push(
    sessionFixture('telegram:tg-1', {
      kind: 'telegram',
      origin: 'telegram',
      title: 'Telegram · @nova_bot',
      lastActivityAtMs: Date.now(),
    }),
  );
  window.history.replaceState(null, '', '/#/s/telegram%3Atg-1');
  render(<ViewHarness />);
  const input = await screen.findByPlaceholderText('Reply on Telegram…');
  await waitFor(() => expect(input).toBeEnabled());
  return input;
}

it('reports a Telegram reply that is queued for delivery', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'sendConnectorMessage').mockResolvedValue({
    messages: [],
    result: { status: 'success', durationMs: 1 },
    deliveryQueued: true,
  });
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(await screen.findByText('Queued for Telegram delivery')).toBeVisible();
});

it('resends a restored Telegram reply with its key and gives a new reply a new key', async () => {
  const user = userEvent.setup();
  const reply = vi
    .spyOn(daemon, 'sendConnectorMessage')
    .mockRejectedValueOnce(
      Object.assign(new Error('daemon request failed (408)'), { status: 408 }),
    )
    .mockResolvedValue({
      messages: [],
      result: { status: 'success', durationMs: 1 },
      deliveryQueued: false,
    });
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(
    await screen.findByRole('button', { name: 'Restore message' }),
  );
  expect(input).toHaveValue('On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  // The daemon joins a retry that reuses the key instead of sending twice.
  await waitFor(() => expect(reply).toHaveBeenCalledTimes(2));
  const [, , , firstKey] = reply.mock.calls[0];
  expect(reply.mock.calls[1]).toEqual([
    'agent-main',
    'tg-1',
    'On my way',
    firstKey,
  ]);

  await user.type(input, 'Running late');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(reply).toHaveBeenCalledTimes(3));
  const [, , text, newKey] = reply.mock.calls[2];
  expect(text).toBe('Running late');
  expect(newKey).toMatch(/^telegram-/);
  expect(newKey).not.toBe(firstKey);
});

it('returns to a new chat when the open session is deleted from the sidebar', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('chat:old', {
      title: 'Old plan',
      lastActivityAtMs: Date.now(),
    }),
  );
  window.history.replaceState(null, '', '/#/s/chat%3Aold');
  render(<ViewHarness />);

  await user.click(
    await screen.findByRole('button', { name: 'Actions for Old plan' }),
  );
  await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
  await user.click(screen.getByRole('menuitem', { name: 'Delete session' }));

  await waitFor(() =>
    expect(daemon.deleteSession).toHaveBeenCalledWith('agent-main', 'chat:old'),
  );
  expect(
    await screen.findByRole('heading', { name: 'Say something to Nova' }),
  ).toBeVisible();
  expect(window.location.hash).toBe('#/');
});
