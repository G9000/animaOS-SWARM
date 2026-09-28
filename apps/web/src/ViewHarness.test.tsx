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
  DaemonConnectionError,
  DaemonHttpError,
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
import {
  SESSION_LIST_LIVE_POLL_MS,
  SESSION_LIST_POLL_MS,
} from './hooks/useCompanionSessions';
import { STREAM_RETRY_MIN_MS } from './hooks/useAgentEvents';
import { LIVE_REFRESH_DELAY_MS } from './hooks/useLiveSession';
import {
  SESSION_MESSAGES_LIVE_POLL_MS,
  SESSION_MESSAGES_POLL_MS,
} from './hooks/useSessionMessages';
import { SEND_RETRY_DELAYS_MS } from './hooks/useSessionSends';
import {
  BOOTSTRAP_POLL_MS,
  BOOTSTRAP_SUMMARY_POLL_MS,
} from './hooks/useDaemonBootstrap';
import { sessionFixture } from './test/sessions';
import {
  deltaEvent,
  idleAgentEvents,
  messageCreatedEvent,
  resyncEvent,
  runEvent,
  runFixture,
  scriptedAgentEvents,
  sessionEvent,
  snapshotEvent,
  snapshotRun,
  steeredEvent,
  toolFinishedEvent,
  toolStartedEvent,
} from './test/live';
import { ViewHarness } from './ViewHarness';

/** Every Markdown render, by its text: the real component still renders. */
const markdownRenders = vi.hoisted(() => vi.fn<(text: string) => void>());
vi.mock('./components/MarkdownMessage', async (importOriginal) => {
  const actual =
    await importOriginal<typeof import('./components/MarkdownMessage')>();
  return {
    ...actual,
    MarkdownMessage: (props: { children: string }) => {
      markdownRenders(props.children);
      return <actual.MarkdownMessage {...props} />;
    },
  };
});

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

/** `daemon.startRun` accepting every message as a queued run (spec §4.2). */
function mockRuns() {
  let accepted = 0;
  vi.spyOn(daemon, 'startRun').mockImplementation(
    async (agentId, sessionId, input) => {
      accepted += 1;
      return {
        run: runFixture(`run_${accepted}`, {
          agentId,
          sessionId,
          input: { text: input.text, attachmentIds: [], skill: null },
        }),
      };
    },
  );
}

/** What the runs route answers for a message whose run already finished. */
function acceptedRun(agentId: string, sessionId: string, text: string) {
  return {
    run: runFixture('run_done', {
      agentId,
      sessionId,
      status: 'completed' as const,
      input: { text, attachmentIds: [], skill: null },
    }),
  };
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
  const send = vi.mocked(daemon.startRun);
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
  const run = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
  vi.mocked(daemon.startRun).mockReturnValue(run.promise);
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
  const run = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
  vi.mocked(daemon.startRun).mockReturnValue(run.promise);
  render(<ViewHarness />);
  await user.type(
    await screen.findByPlaceholderText('Message Alpha…'),
    'Alpha request',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(screen.getByRole('button', { name: 'Work', exact: true }));
  current = withMessage(alpha, 'Alpha finished', 'chat:new-1');
  await act(async () =>
    run.resolve(acceptedRun('alpha', 'chat:new-1', 'Alpha request')),
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
  // The peer's request is credited to the peer that sent it.
  expect(screen.getByText('From Beta')).toBeVisible();
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
  mockRuns();
  // No stream events unless a test scripts them; the harness polls.
  idleAgentEvents();
  vi.spyOn(daemon, 'sessionRuns').mockResolvedValue([]);
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
    const startRun = vi.mocked(daemon.startRun);

    render(<ViewHarness />);
    await openChat();
    await screen.findByRole('heading', { name: 'Say something to Nova' });
    await waitFor(() =>
      expect(daemon.importLegacySchedules).toHaveBeenCalled(),
    );
    expect(interval.mock.calls.some(([, delay]) => delay === 10_000)).toBe(
      false,
    );
    expect(startRun).not.toHaveBeenCalled();
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
    const startRun = vi.mocked(daemon.startRun);

    render(<ViewHarness />);
    await openChat();

    expect(
      await screen.findByRole('heading', { name: 'Say something to Alpha' }),
    ).toBeVisible();
    await user.type(screen.getByPlaceholderText('Message Alpha…'), 'Hello');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() =>
      expect(startRun).toHaveBeenCalledWith(
        'agent-a',
        'chat:new-1',
        { text: 'Hello', mode: 'queue' },
        expect.any(String),
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
    const startRun = vi
      .mocked(daemon.startRun)
      .mockImplementation(async (id, sessionId, input) => {
        current = withMessage(next, 'Next is responsive', sessionId);
        return acceptedRun(id, sessionId, input.text);
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
      expect(startRun).toHaveBeenCalledWith(
        'agent-next',
        'chat:new-1',
        { text: 'Continue', mode: 'queue' },
        expect.any(String),
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
    const run = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
    const send = vi.mocked(daemon.startRun).mockReturnValue(run.promise);
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
    vi.mocked(daemon.startRun).mockRejectedValue(new Error('Network failed'));
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
    const run = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
    vi.mocked(daemon.startRun).mockReturnValue(run.promise);
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
    vi.mocked(daemon.startRun).mockRejectedValue(
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
    const startRun = vi.mocked(daemon.startRun);
    const input = screen.getByPlaceholderText('Message Nova…');
    fireEvent.change(input, { target: { value: 'Keep drafting offline' } });
    expect(input).toHaveValue('Keep drafting offline');
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(startRun).not.toHaveBeenCalled();
  });

  it('ignores a message accepted after the companion changed', async () => {
    const user = userEvent.setup();
    const first = snapshot('agent-a', 'Alpha', 1);
    const next = snapshot('agent-b', 'Beta', 2);
    const replacement = deferred<{ agents: DaemonSnapshot[] }>();
    const run = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
    vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
    vi.spyOn(daemon, 'listAgents')
      .mockResolvedValueOnce({ agents: [first] })
      .mockReturnValueOnce(replacement.promise);
    mockProviders();
    vi.mocked(daemon.startRun).mockReturnValue(run.promise);
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
      expect(daemon.startRun).toHaveBeenCalledWith(
        'agent-a',
        'chat:new-1',
        { text: 'Alpha work', mode: 'queue' },
        expect.any(String),
      ),
    );

    act(() => poll());
    await act(async () => {
      replacement.resolve({ agents: [next] });
      await replacement.promise;
    });
    await screen.findByRole('heading', { name: 'Say something to Beta' });

    await act(async () => {
      run.resolve(acceptedRun('agent-a', 'chat:new-1', 'Alpha work'));
      await run.promise;
    });

    expect(
      screen.getByRole('heading', { name: 'Say something to Beta' }),
    ).toBeVisible();
    expect(screen.queryByText('Alpha work')).not.toBeInTheDocument();
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

it('retries a message that did not reach the daemon with the same key', async () => {
  const user = fakeClock();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  const startRun = vi.mocked(daemon.startRun);
  startRun.mockRejectedValueOnce(
    new DaemonConnectionError('', new TypeError('Failed to fetch')),
  );
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'Plan the week',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(
    await screen.findByText('Not delivered yet · retrying…'),
  ).toBeVisible();
  expect(screen.getByText('Plan the week')).toBeVisible();
  await elapse(SEND_RETRY_DELAYS_MS[0]);
  await waitFor(() => expect(startRun).toHaveBeenCalledTimes(2));
  expect(startRun.mock.calls[1][3]).toBe(startRun.mock.calls[0][3]);
  await waitFor(() =>
    expect(
      screen.queryByText('Not delivered yet · retrying…'),
    ).not.toBeInTheDocument(),
  );
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
});

it('returns a message the daemon refused to the recovery panel without retrying', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1)],
  });
  mockProviders();
  const startRun = vi.mocked(daemon.startRun).mockRejectedValue(
    new DaemonHttpError(429, {
      error:
        'This companion already has 8 queued messages; wait for one to start',
    }),
  );
  render(<ViewHarness />);
  await openChat();
  await user.type(
    await screen.findByPlaceholderText('Message Nova…'),
    'One more thing',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(
    await screen.findByText(
      'This companion already has 8 queued messages; wait for one to start',
    ),
  ).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Restore message' }));
  expect(screen.getByPlaceholderText('Message Nova…')).toHaveValue(
    'One more thing',
  );
  expect(startRun).toHaveBeenCalledTimes(1);
});

it('keeps the composer usable while the session has a reply in progress', async () => {
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
      activeRuns: 1,
      lastActivityAtMs: Date.now(),
    }),
  );
  window.history.replaceState(null, '', '/#/s/room-7');
  render(<ViewHarness />);

  const input = await screen.findByPlaceholderText('Message Nova…');
  await waitFor(() => expect(input).toBeEnabled());
  await user.type(input, 'And one more{Enter}');

  expect(daemon.startRun).toHaveBeenCalledWith(
    'agent-main',
    'room-7',
    { text: 'And one more', mode: 'queue' },
    expect.any(String),
  );
});

it('sends a session’s messages one at a time, in the order written', async () => {
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
  const first = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
  const startRun = vi.mocked(daemon.startRun);
  startRun.mockReturnValueOnce(first.promise);
  window.history.replaceState(null, '', '/#/s/room-7');
  render(<ViewHarness />);

  const input = await screen.findByPlaceholderText('Message Nova…');
  await waitFor(() => expect(input).toBeEnabled());
  await user.type(input, 'First{Enter}');
  await user.type(input, 'Second{Enter}');

  expect(screen.getByText('Second')).toBeVisible();
  expect(startRun).toHaveBeenCalledTimes(1);
  await act(async () =>
    first.resolve(acceptedRun('agent-main', 'room-7', 'First')),
  );
  await waitFor(() => expect(startRun).toHaveBeenCalledTimes(2));
  expect(startRun.mock.calls.map(([, , body]) => body.text)).toEqual([
    'First',
    'Second',
  ]);
});

it('gives a restored message that was edited a new key', async () => {
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
  const startRun = vi.mocked(daemon.startRun);
  startRun.mockRejectedValueOnce(
    new DaemonHttpError(503, { error: 'The control plane could not be saved' }),
  );
  window.history.replaceState(null, '', '/#/s/room-7');
  render(<ViewHarness />);

  const input = await screen.findByPlaceholderText('Message Nova…');
  await waitFor(() => expect(input).toBeEnabled());
  await user.type(input, 'Book the train{Enter}');
  await user.click(
    await screen.findByRole('button', { name: 'Restore message' }),
  );
  await user.type(input, ' tonight{Enter}');

  await waitFor(() => expect(startRun).toHaveBeenCalledTimes(2));
  const [, , , firstKey] = startRun.mock.calls[0];
  const [, , body, editedKey] = startRun.mock.calls[1];
  expect(body.text).toBe('Book the train tonight');
  expect(editedKey).not.toBe(firstKey);
});

it('replies to a check-in through the runs route', async () => {
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
      lastActivityAtMs: Date.now(),
    }),
  );
  window.history.replaceState(null, '', '/#/s/schedule%3Adaily');
  render(<ViewHarness />);

  const input = await screen.findByPlaceholderText('Reply to this check-in…');
  await waitFor(() => expect(input).toBeEnabled());
  await user.type(input, 'Goals are on track{Enter}');

  expect(daemon.startRun).toHaveBeenCalledWith(
    'agent-main',
    'schedule:daily',
    { text: 'Goals are on track', mode: 'queue' },
    expect.any(String),
  );
});

it('opens an existing session from the sidebar, marks it read, and sends to it', async () => {
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
    .mocked(daemon.startRun)
    .mockImplementation(async (id, sessionId, input) => {
      current = structuredClone(current);
      current.messages.push(
        {
          id: 'user-2',
          agentId: id,
          roomId: sessionId,
          role: 'user',
          content: { text: input.text },
          createdAtMs: 3,
        },
        {
          id: 'reply-2',
          agentId: id,
          roomId: sessionId,
          role: 'assistant',
          content: { text: 'Saturday works' },
          createdAtMs: 4,
        },
      );
      return acceptedRun(id, sessionId, input.text);
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
    'room-7',
    { text: 'Does Saturday work?', mode: 'queue' },
    expect.any(String),
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
  const startRun = vi.mocked(daemon.startRun);
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

  expect(startRun).toHaveBeenCalledWith(
    'agent-main',
    'legacy-room:abc',
    { text: 'Hello again', mode: 'queue' },
    expect.any(String),
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

it('offers a message still retrying when the page reloads before it is answered (S3b-A)', async () => {
  const user = userEvent.setup();
  const alpha = snapshot('alpha', 'Alpha', 1);
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({ agents: [alpha] });
  mockProviders();
  vi.mocked(daemon.startRun).mockReturnValue(new Promise(() => {}));
  const first = render(<ViewHarness />);
  await user.type(
    await screen.findByPlaceholderText('Message Alpha…'),
    'Still on its way',
  );
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(1));
  first.unmount();

  render(<ViewHarness />);
  await user.click(
    await screen.findByRole('button', { name: 'Restore message' }),
  );
  expect(screen.getByPlaceholderText('Message Alpha…')).toHaveValue(
    'Still on its way',
  );
  // The restored key is reused: sending it again joins, not doubles.
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(2));
  const keys = vi.mocked(daemon.startRun).mock.calls.map(([, , , key]) => key);
  expect(keys[1]).toBe(keys[0]);
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
  const startRun = vi
    .mocked(daemon.startRun)
    .mockImplementation(async (id, sessionId, input) => {
      // The daemon titles a new chat from its first message.
      setSessionFields(sessionId, { title: input.text });
      return acceptedRun(id, sessionId, input.text);
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
  expect(startRun).toHaveBeenCalledTimes(1);
  expect(startRun).toHaveBeenCalledWith(
    'agent-main',
    'chat:new-1',
    { text: 'First question', mode: 'queue' },
    expect.any(String),
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
  const startRun = vi.mocked(daemon.startRun);
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
    expect(startRun).toHaveBeenCalledWith(
      'agent-main',
      'chat:new-1',
      { text: 'Plan the launch', mode: 'queue' },
      expect.any(String),
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
  const startRun = vi
    .mocked(daemon.startRun)
    .mockImplementation(async (id, sessionId, input) => {
      current = withMessage(
        snapshot(id, 'Nova', 1),
        'Launch plan ready',
        sessionId,
      );
      return acceptedRun(id, sessionId, input.text);
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
    expect(startRun).toHaveBeenCalledWith(
      'agent-main',
      'chat:new-1',
      { text: 'Plan the launch', mode: 'queue' },
      expect.any(String),
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
  const startRun = vi.mocked(daemon.startRun);
  render(<ViewHarness />);

  expect(await screen.findByText('Update the daemon')).toBeVisible();
  const input = screen.getByPlaceholderText('Message Nova…');
  expect(input).toBeDisabled();
  fireEvent.change(input, { target: { value: 'Hello' } });
  fireEvent.keyDown(input, { key: 'Enter' });
  expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  expect(daemon.createSession).not.toHaveBeenCalled();
  expect(startRun).not.toHaveBeenCalled();
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
  // Check-ins take replies from M3 on (spec §15.3).
  expect(screen.getByPlaceholderText('Reply to this check-in…')).toBeVisible();
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

it('replies to a Telegram session through the runs route', async () => {
  const user = userEvent.setup();
  const reply = vi.spyOn(daemon, 'sendConnectorMessage');
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(daemon.startRun).toHaveBeenCalledWith(
    'agent-main',
    'telegram:tg-1',
    { text: 'On my way', mode: 'queue' },
    expect.any(String),
  );
  expect(reply).not.toHaveBeenCalled();
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

/** The first Telegram reply's run (`run_1`) ends as `status`: its messages
 *  are committed when it ends, and the ledger then holds how it ended. */
function endTelegramRun(status: 'completed' | 'failed') {
  const startRun = vi.mocked(daemon.startRun);
  vi.mocked(daemon.sessionMessages).mockImplementation(async () => ({
    messages: startRun.mock.calls.length
      ? [
          {
            id: 'reply-1',
            role: 'assistant',
            text: 'Be right there',
            attachments: [],
            metadata: { runId: 'run_1' },
            createdAtMs: 2,
          },
        ]
      : [],
    nextBefore: null,
  }));
  vi.spyOn(daemon, 'sessionRuns').mockResolvedValue([
    runFixture('run_1', {
      sessionId: 'telegram:tg-1',
      source: 'telegram',
      status,
    }),
  ]);
}

it('reports a Telegram reply that is queued for delivery', async () => {
  const user = userEvent.setup();
  endTelegramRun('completed');
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(await screen.findByText('Queued for Telegram delivery')).toBeVisible();
});

it('does not report delivery for a Telegram reply whose run failed', async () => {
  const user = userEvent.setup();
  endTelegramRun('failed');
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(await screen.findByText('Be right there')).toBeVisible();
  await waitFor(() =>
    expect(daemon.sessionRuns).toHaveBeenCalledWith(
      'agent-main',
      'telegram:tg-1',
      { limit: 20 },
    ),
  );
  await act(async () => {
    await Promise.all(
      vi.mocked(daemon.sessionRuns).mock.results.map((item) => item.value),
    );
  });
  expect(
    screen.queryByText('Queued for Telegram delivery'),
  ).not.toBeInTheDocument();
});

it('resends a restored message with its key and gives a new message a new key', async () => {
  const user = userEvent.setup();
  const startRun = vi.mocked(daemon.startRun);
  startRun.mockRejectedValueOnce(new Error('Telegram is reconnecting'));
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await user.click(
    await screen.findByRole('button', { name: 'Restore message' }),
  );
  expect(input).toHaveValue('On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  // The daemon joins a resend that reuses the key instead of sending twice.
  await waitFor(() => expect(startRun).toHaveBeenCalledTimes(2));
  const [, , , firstKey] = startRun.mock.calls[0];
  expect(startRun.mock.calls[1]).toEqual([
    'agent-main',
    'telegram:tg-1',
    { text: 'On my way', mode: 'queue' },
    firstKey,
  ]);

  await user.type(input, 'Running late');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(startRun).toHaveBeenCalledTimes(3));
  const [, , body, newKey] = startRun.mock.calls[2];
  expect(body.text).toBe('Running late');
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

/** Past the harness's settle for live refreshes (150 ms). */
const LIVE_SETTLE_MS = 200;

/** Nova with its chat `room-7` open and a scripted event stream. */
async function openLiveSession(overrides: Partial<Session> = {}) {
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
      ...overrides,
    }),
  );
  window.history.replaceState(null, '', '/#/s/room-7');
  const events = scriptedAgentEvents();
  render(<ViewHarness />);
  const input = await screen.findByPlaceholderText('Message Nova…');
  await waitFor(() => expect(input).toBeEnabled());
  await waitFor(() => expect(events.streams).toHaveLength(1));
  return { input, stream: events.streams[0], events };
}

function runningRun() {
  return runFixture('run_7', {
    sessionId: 'room-7',
    status: 'running',
    createdAtMs: Date.now(),
    startedAtMs: Date.now(),
    input: { text: 'Plan Saturday', attachmentIds: [], skill: null },
  });
}

it('streams a reply into the open session with its tool steps, then shows the committed reply', async () => {
  const { stream } = await openLiveSession();
  const run = runningRun();
  act(() =>
    stream.push(
      snapshotEvent([]),
      runEvent('run.started', run, 2),
      toolStartedEvent(
        run,
        'call_1',
        'web_search',
        3,
        '{"query":"weather saturday"}',
      ),
      deltaEvent(run, 'run_7:2', 0, 'Saturday looks sunny', 4),
    ),
  );

  expect(await screen.findByText('Saturday looks sunny')).toBeVisible();
  expect(screen.getByText('Plan Saturday')).toBeVisible();
  expect(screen.getByRole('button', { name: /web_search/ })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Stop' })).toBeVisible();

  vi.mocked(daemon.sessionMessages).mockResolvedValue({
    messages: [
      {
        id: 'u1',
        role: 'user',
        text: 'Plan Saturday',
        attachments: [],
        metadata: { runId: 'run_7' },
        createdAtMs: 2,
      },
      {
        id: 'a1',
        role: 'assistant',
        text: 'Saturday looks sunny, go hiking.',
        attachments: [],
        metadata: { runId: 'run_7', stepId: 'run_7:2' },
        createdAtMs: 3,
      },
    ],
    nextBefore: null,
  });
  act(() =>
    stream.push(
      messageCreatedEvent(run, 'u1', 'user', 5),
      messageCreatedEvent(run, 'a1', 'assistant', 6),
      runEvent(
        'run.completed',
        {
          ...run,
          status: 'completed',
          finishedAtMs: Date.now(),
          replyMessageId: 'a1',
        },
        7,
      ),
    ),
  );

  expect(
    await screen.findByText('Saturday looks sunny, go hiking.'),
  ).toBeVisible();
  await waitFor(() =>
    expect(screen.queryByText('Saturday looks sunny')).not.toBeInTheDocument(),
  );
  expect(screen.getByText('Nova replied.')).toHaveAttribute(
    'aria-live',
    'polite',
  );
  // The stream's state reaches the view once per frame; the history read
  // that opening the stream started can land first.
  await waitFor(() =>
    expect(
      screen.queryByRole('button', { name: 'Stop' }),
    ).not.toBeInTheDocument(),
  );
});

it('streams a reply without rendering unchanged history again', async () => {
  const earlier = (
    id: string,
    role: SessionMessage['role'],
    text: string,
  ): SessionMessage => ({
    id,
    role,
    text,
    attachments: [],
    metadata: {},
    createdAtMs: 1,
  });
  vi.mocked(daemon.sessionMessages).mockResolvedValue({
    messages: [
      earlier('u0', 'user', 'Earlier question'),
      earlier('a0', 'assistant', 'Earlier reply'),
    ],
    // Older history, so the view offers to load it.
    nextBefore: 'cursor-1',
  });
  const { stream } = await openLiveSession();
  await screen.findByText('Earlier reply');
  const run = runningRun();
  act(() => stream.push(snapshotEvent([]), runEvent('run.started', run, 2)));
  await screen.findByText('Plan Saturday');
  // The reads the stream's snapshot asks for land first: a read replaces
  // the messages it returns.
  await act(async () => {
    await new Promise((resolve) =>
      window.setTimeout(resolve, LIVE_REFRESH_DELAY_MS * 2),
    );
  });
  const reads = vi.mocked(daemon.sessionMessages).mock.calls.length;
  const rendersOf = (text: string) =>
    markdownRenders.mock.calls.filter(([rendered]) => rendered === text).length;
  const before = [rendersOf('Earlier question'), rendersOf('Earlier reply')];

  let text = '';
  for (let index = 0; index < 20; index += 1) {
    const piece = `word${index} `;
    act(() =>
      stream.push(deltaEvent(run, 'run_7:1', text.length, piece, 3 + index)),
    );
    text += piece;
    // Each delta reaches the view in a frame of its own.
    expect(await screen.findByText(text.trim())).toBeVisible();
  }

  expect(rendersOf(text)).toBeGreaterThanOrEqual(1);
  // Nothing but the deltas changed while they streamed.
  expect(vi.mocked(daemon.sessionMessages).mock.calls.length).toBe(reads);
  expect([rendersOf('Earlier question'), rendersOf('Earlier reply')]).toEqual(
    before,
  );
});

it('stops the reply in progress from the composer', async () => {
  const user = userEvent.setup();
  const stopRun = vi
    .spyOn(daemon, 'stopRun')
    .mockImplementation(async (_agentId, runId) =>
      runFixture(runId, { sessionId: 'room-7', status: 'cancelled' }),
    );
  const { stream } = await openLiveSession();
  act(() =>
    stream.push(
      snapshotEvent([
        snapshotRun(runningRun(), {
          stepId: 'run_7:1',
          text: 'Thinking it over',
        }),
      ]),
    ),
  );

  await user.click(await screen.findByRole('button', { name: 'Stop' }));
  expect(stopRun).toHaveBeenCalledWith('agent-main', 'run_7');
});

it('steers a message into the running reply with Ctrl+Enter', async () => {
  const user = userEvent.setup();
  const { input, stream } = await openLiveSession();
  const run = runningRun();
  act(() => stream.push(snapshotEvent([snapshotRun(run)])));
  await screen.findByRole('button', { name: 'Stop' });
  vi.mocked(daemon.startRun).mockResolvedValueOnce({
    run,
    steer: { status: 'pending' },
  });

  await user.type(input, 'also check trains');
  await user.keyboard('{Control>}{Enter}{/Control}');

  expect(daemon.startRun).toHaveBeenCalledWith(
    'agent-main',
    'room-7',
    { text: 'also check trains', mode: 'steer' },
    expect.any(String),
  );
  expect(
    await screen.findByText('Joining the reply in progress…'),
  ).toBeVisible();
  act(() => stream.push(steeredEvent(run, 'm-steer', 'also check trains', 2)));
  await waitFor(() =>
    expect(
      screen.queryByText('Joining the reply in progress…'),
    ).not.toBeInTheDocument(),
  );
  expect(screen.getByText('also check trains')).toBeVisible();
});

it('cancels a queued message', async () => {
  const user = userEvent.setup();
  vi.spyOn(daemon, 'stopRun').mockImplementation(async (_agentId, runId) =>
    runFixture(runId, { sessionId: 'room-7', status: 'cancelled' }),
  );
  const { stream } = await openLiveSession();
  act(() =>
    stream.push(
      snapshotEvent([
        snapshotRun(
          runFixture('run_q', {
            sessionId: 'room-7',
            createdAtMs: Date.now(),
            input: { text: 'Later please', attachmentIds: [], skill: null },
          }),
        ),
      ]),
    ),
  );

  expect(await screen.findByText('Later please')).toBeVisible();
  expect(screen.getByText('Queued')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(daemon.stopRun).toHaveBeenCalledWith('agent-main', 'run_q');
});

it('offers to send an interrupted message again, warning when tools had started', async () => {
  const user = userEvent.setup();
  vi.mocked(daemon.sessionRuns).mockResolvedValue([
    runFixture('run_i', {
      sessionId: 'room-7',
      status: 'interrupted',
      createdAtMs: Date.now(),
      toolsStarted: ['bash'],
      error: { code: 'restart_during_run', message: 'The daemon restarted' },
      input: { text: 'Clean the logs', attachmentIds: [], skill: null },
    }),
  ]);
  await openLiveSession();

  expect(
    await screen.findByText(
      'The daemon restarted while this reply was running.',
    ),
  ).toBeVisible();
  expect(
    screen.getByText(
      'Tools had started (bash). Check their effects before sending again.',
    ),
  ).toBeVisible();
  expect(screen.getByText('Clean the logs')).toBeVisible();
  const sendAgain = screen.getByRole('button', { name: 'Send again' });
  await user.click(sendAgain);
  expect(daemon.startRun).toHaveBeenCalledWith(
    'agent-main',
    'room-7',
    { text: 'Clean the logs', mode: 'queue' },
    expect.any(String),
  );
  // Used once, it cannot send the message a second time.
  await waitFor(() => expect(sendAgain).toBeDisabled());
  await user.click(sendAgain);
  expect(daemon.startRun).toHaveBeenCalledTimes(1);
});

it('runs slash commands instead of sending them', async () => {
  const user = userEvent.setup();
  const { input } = await openLiveSession();
  const compact = vi
    .spyOn(daemon, 'compactSession')
    .mockImplementation(
      async (_agentId, sessionId) =>
        routes.sessions.find((item) => item.id === sessionId)!,
    );

  await user.type(input, '/compact{Enter}');
  await waitFor(() =>
    expect(compact).toHaveBeenCalledWith('agent-main', 'room-7'),
  );
  expect(input).toHaveValue('');

  await user.type(input, '/rename{Enter}');
  expect(input).toHaveValue('/rename ');
  await user.type(input, 'Offsite{Enter}');
  await waitFor(() =>
    expect(daemon.updateSession).toHaveBeenCalledWith('agent-main', 'room-7', {
      title: 'Offsite',
    }),
  );

  await user.type(input, '/stop{Enter}');
  expect(await screen.findByText('/stop is not available here.')).toBeVisible();
  expect(input).toHaveValue('/stop');

  await user.clear(input);
  await user.type(input, '/new{Enter}');
  await waitFor(() => expect(window.location.hash).toBe('#/'));
  expect(daemon.startRun).not.toHaveBeenCalled();
});

it('switches the bootstrap to agent summaries once the event stream opens', async () => {
  fakeClock();
  const summaries = vi.spyOn(daemon, 'listAgentSummaries').mockResolvedValue([
    {
      state: snapshot('agent-main', 'Nova', 1).state,
      messageCount: 0,
      eventCount: 0,
      lastTask: null,
    },
  ]);
  const { stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await elapse(100);
  const fullReads = vi.mocked(daemon.listAgents).mock.calls.length;

  await elapse(BOOTSTRAP_POLL_MS * 2);
  expect(vi.mocked(daemon.listAgents).mock.calls.length).toBe(fullReads);
  await elapse(BOOTSTRAP_SUMMARY_POLL_MS);
  expect(summaries).toHaveBeenCalled();
});

it('opens a helper session by its agent and credits its task to the companion', async () => {
  const helper = snapshot('helper-7', 'Researcher', 2);
  helper.state.config.settings = { additional: { workspaceRole: 'helper' } };
  vi.spyOn(daemon, 'health').mockResolvedValue({ status: 'ok' });
  vi.spyOn(daemon, 'listAgents').mockResolvedValue({
    agents: [snapshot('agent-main', 'Nova', 1), helper],
  });
  mockProviders();
  routes.sessions.push(
    sessionFixture('room-9', {
      agentId: 'helper-7',
      kind: 'helper',
      origin: 'delegation',
      title: 'Compare vendors',
      parentAgentId: 'agent-main',
      parentRunId: 'run_1',
      capabilities: readOnly,
      lastActivityAtMs: Date.now(),
    }),
  );
  vi.mocked(daemon.sessionMessages).mockImplementation(
    async (agentId, sessionId) => ({
      messages:
        agentId === 'helper-7' && sessionId === 'room-9'
          ? [
              {
                id: 't1',
                role: 'user',
                text: 'Task delegated by workspace manager Nova (agent-main). Return the result and any blockers. Do not delegate further.\n\nCompare vendors',
                attachments: [],
                metadata: {},
                createdAtMs: 2,
              },
              {
                id: 'r1',
                role: 'assistant',
                text: 'Vendor B is cheaper',
                attachments: [],
                metadata: {},
                createdAtMs: 3,
              },
            ]
          : [],
      nextBefore: null,
    }),
  );
  window.history.replaceState(null, '', '/#/s/helper-7/room-9');
  render(<ViewHarness />);

  const conversation = await screen.findByLabelText(
    'Conversation with Researcher',
  );
  expect(
    await within(conversation).findByText('Vendor B is cheaper'),
  ).toBeVisible();
  expect(within(conversation).getByText('From Nova')).toBeVisible();
  expect(within(conversation).getByText('Compare vendors')).toBeVisible();
  expect(
    screen.queryByText(/Task delegated by workspace manager/),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('note')).toHaveTextContent(
    'Helper sessions are read-only.',
  );
});

it('refreshes the sidebar when the stream reports a session change', async () => {
  const { stream } = await openLiveSession();
  routes.sessions.push(
    sessionFixture('chat:elsewhere', {
      title: 'Made on Telegram',
      lastActivityAtMs: Date.now(),
    }),
  );
  act(() => stream.push(sessionEvent('session.created', 'chat:elsewhere', 1)));

  expect(
    await screen.findByRole('button', { name: 'Made on Telegram' }),
  ).toBeVisible();
});

it('coalesces the reads a burst of accepted sends asks for', async () => {
  const user = userEvent.setup();
  const { input, stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  const settle = () =>
    act(async () => {
      await new Promise((resolve) =>
        window.setTimeout(resolve, LIVE_REFRESH_DELAY_MS * 2),
      );
    });
  await settle();
  const first = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
  const startRun = vi.mocked(daemon.startRun);
  startRun.mockReturnValueOnce(first.promise);
  for (let index = 1; index <= 8; index += 1)
    await user.type(input, `Message ${index}{Enter}`);
  const reads = () => [
    vi.mocked(daemon.listSessions).mock.calls.length,
    vi.mocked(daemon.sessionMessages).mock.calls.length,
    vi.mocked(daemon.sessionRuns).mock.calls.length,
  ];
  const before = reads();

  await act(async () =>
    first.resolve(acceptedRun('agent-main', 'room-7', 'Message 1')),
  );
  await waitFor(() => expect(startRun).toHaveBeenCalledTimes(8));
  await settle();
  // One read of each kind once the burst settles: 3, not 24.
  expect(reads().map((count, index) => count - before[index])).toEqual([
    1, 1, 1,
  ]);
});

it('says it is reconnecting when the stream drops', async () => {
  const { stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  act(() => stream.end());

  expect(await screen.findByText('Reconnecting…')).toBeVisible();
});

/** A steer the harness accepted into `run_7`, the reply in progress. */
async function steerIntoRunningReply(user = userEvent.setup()) {
  const live = await openLiveSession();
  const run = runningRun();
  act(() => live.stream.push(snapshotEvent([snapshotRun(run)])));
  await screen.findByRole('button', { name: 'Stop' });
  vi.mocked(daemon.startRun).mockResolvedValueOnce({
    run,
    steer: { status: 'pending' },
  });
  await user.type(live.input, 'also check trains');
  await user.keyboard('{Control>}{Enter}{/Control}');
  expect(
    await screen.findByText('Joining the reply in progress…'),
  ).toBeVisible();
  return {
    ...live,
    user,
    run,
    key: vi.mocked(daemon.startRun).mock.calls[0][3],
  };
}

function userMessage(
  id: string,
  text: string,
  metadata: Record<string, unknown>,
  createdAtMs = 2,
): SessionMessage {
  return { id, role: 'user', text, attachments: [], metadata, createdAtMs };
}

it('keeps a steer through a reconnect until its message is in history', async () => {
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const { stream, events, run, key } = await steerIntoRunningReply();

  // A dropped stream and its new snapshot say nothing about the steer.
  act(() => stream.end());
  expect(await screen.findByText('Reconnecting…')).toBeVisible();
  await waitFor(() => expect(events.streams).toHaveLength(2), {
    timeout: STREAM_RETRY_MIN_MS * 2,
  });
  act(() => events.streams[1].push(snapshotEvent([snapshotRun(run)])));
  await waitFor(() =>
    expect(screen.queryByText('Reconnecting…')).not.toBeInTheDocument(),
  );
  expect(screen.getByText('Joining the reply in progress…')).toBeVisible();

  // The reply took it: its message, keyed by the send, is in history.
  vi.mocked(daemon.sessionMessages).mockResolvedValue({
    messages: [
      userMessage('u1', 'Plan Saturday', { runId: 'run_7' }),
      userMessage(
        'm-steer',
        'also check trains',
        { runId: 'run_7', steer: true, clientRequestId: key },
        3,
      ),
    ],
    nextBefore: null,
  });
  act(() =>
    events.streams[1].push(
      messageCreatedEvent(run, 'm-steer', 'user', 2),
      runEvent(
        'run.completed',
        { ...run, status: 'completed', finishedAtMs: Date.now() },
        3,
      ),
    ),
  );
  await waitFor(() =>
    expect(
      screen.queryByText('Joining the reply in progress…'),
    ).not.toBeInTheDocument(),
  );
  expect(screen.getByText('also check trains')).toBeVisible();
});

it('shows a steer the reply did not take as a queued message of its own', async () => {
  const { stream, run, key } = await steerIntoRunningReply();

  // The reply ended before its next model call: the daemon queued the
  // steer, under the steer's own key.
  act(() =>
    stream.push(
      runEvent(
        'run.queued',
        runFixture('run_8', {
          sessionId: 'room-7',
          idempotencyKey: key,
          createdAtMs: Date.now(),
          input: { text: 'also check trains', attachmentIds: [], skill: null },
        }),
        2,
      ),
      runEvent(
        'run.completed',
        { ...run, status: 'completed', finishedAtMs: Date.now() },
        3,
      ),
    ),
  );

  await waitFor(() =>
    expect(
      screen.queryByText('Joining the reply in progress…'),
    ).not.toBeInTheDocument(),
  );
  expect(screen.getByText('also check trains')).toBeVisible();
  expect(screen.getByText('Queued')).toBeVisible();
});

it('moves a steer to the recovery panel when its reply fails without it', async () => {
  const { stream, run, user, input, key } = await steerIntoRunningReply();

  act(() =>
    stream.push(
      runEvent(
        'run.failed',
        {
          ...run,
          status: 'failed',
          finishedAtMs: Date.now(),
          error: { code: 'model_error', message: 'provider unavailable' },
        },
        2,
      ),
    ),
  );

  await user.click(
    await screen.findByRole('button', { name: 'Restore message' }),
  );
  expect(
    screen.queryByText('Joining the reply in progress…'),
  ).not.toBeInTheDocument();
  expect(input).toHaveValue('also check trains');
  // Sent again it is a new message: the steer's key still names the old run.
  await user.keyboard('{Enter}');
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(2));
  const [, , body, resendKey] = vi.mocked(daemon.startRun).mock.calls[1];
  expect(body).toEqual({ text: 'also check trains', mode: 'queue' });
  expect(resendKey).not.toBe(key);
});

it('announces each finished reply in the open session once', async () => {
  const { stream } = await openLiveSession();
  const run = runningRun();
  const done = {
    ...run,
    status: 'completed' as const,
    finishedAtMs: Date.now(),
    replyMessageId: 'a1',
  };
  act(() =>
    stream.push(
      snapshotEvent([snapshotRun(run)]),
      runEvent('run.completed', done, 2),
    ),
  );
  const region = await screen.findByText('Nova replied.');
  expect(region.textContent).toBe('Nova replied.');

  // The same finish again and a new snapshot announce nothing new.
  await act(async () =>
    stream.push(runEvent('run.completed', done, 3), snapshotEvent([], 4)),
  );
  await act(async () => {
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  });
  expect(region.textContent).toBe('Nova replied.');

  // Another session's reply is not announced here; the next one here is,
  // with text that differs so screen readers read it again.
  await act(async () =>
    stream.push(
      runEvent(
        'run.completed',
        runFixture('run_x', {
          sessionId: 'chat:other',
          status: 'completed',
          finishedAtMs: Date.now(),
        }),
        5,
      ),
    ),
  );
  expect(region.textContent).toBe('Nova replied.');
  await act(async () =>
    stream.push(
      runEvent(
        'run.completed',
        runFixture('run_9', {
          sessionId: 'room-7',
          status: 'completed',
          finishedAtMs: Date.now(),
        }),
        6,
      ),
    ),
  );
  await waitFor(() => expect(region.textContent).toBe('Nova replied. '));
});

it('keeps polling as before when the daemon has no event stream', async () => {
  fakeClock();
  vi.spyOn(daemon, 'getAgent').mockResolvedValue({
    agent: snapshot('agent-main', 'Nova', 1),
  });
  // A daemon without the event stream (M2) has no runs route either.
  vi.mocked(daemon.startRun).mockRejectedValue(
    new DaemonHttpError(404, { error: 'Not found' }),
  );
  const summaries = vi.spyOn(daemon, 'listAgentSummaries');
  const { input, stream } = await openLiveSession();
  await act(async () => {
    stream.fail(new DaemonHttpError(404, { error: 'Not found' }));
    await vi.advanceTimersByTimeAsync(50);
  });
  expect(daemon.getAgent).toHaveBeenCalledWith('agent-main');
  const calls = () => ({
    agents: vi.mocked(daemon.listAgents).mock.calls.length,
    sessions: vi.mocked(daemon.listSessions).mock.calls.length,
    messages: vi.mocked(daemon.sessionMessages).mock.calls.length,
  });
  const before = calls();

  await elapse(BOOTSTRAP_POLL_MS);
  expect(calls().agents).toBeGreaterThan(before.agents);
  expect(calls().messages).toBeGreaterThan(before.messages);
  await elapse(SESSION_LIST_POLL_MS);
  expect(calls().sessions).toBeGreaterThan(before.sessions);
  expect(summaries).not.toHaveBeenCalled();
  expect(daemon.agentEvents).toHaveBeenCalledTimes(1);
  expect(screen.queryByText('Reconnecting…')).not.toBeInTheDocument();

  // It cannot take a message: the page says to update it instead.
  expect(screen.getByText('Update the daemon')).toBeVisible();
  expect(
    screen.getByText(/sends messages as live runs, which this anima-daemon/),
  ).toBeVisible();
  expect(input).toBeDisabled();
  fireEvent.change(input, { target: { value: 'Still here' } });
  fireEvent.keyDown(input, { key: 'Enter' });
  expect(daemon.startRun).not.toHaveBeenCalled();
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
});

it('asks for a daemon update when a session that exists has no runs route', async () => {
  const user = userEvent.setup();
  vi.mocked(daemon.startRun).mockRejectedValue(
    new DaemonHttpError(404, { error: 'Not found' }),
  );
  const { input } = await openLiveSession();
  const reads = vi.mocked(daemon.getSession).mock.calls.length;

  await user.type(input, 'Still here{Enter}');

  expect(await screen.findByText('Update the daemon')).toBeVisible();
  // The session was read to tell a missing route from a missing session.
  expect(vi.mocked(daemon.getSession).mock.calls.slice(reads)).toEqual([
    ['agent-main', 'room-7'],
  ]);
  await waitFor(() => expect(input).toBeDisabled());
  // Nothing goes to recovery: the message waits in the composer.
  expect(input).toHaveValue('Still here');
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
  expect(daemon.startRun).toHaveBeenCalledTimes(1);
});

it('recovers a message sent to a session that is gone, without asking for an update', async () => {
  const user = userEvent.setup();
  vi.mocked(daemon.startRun).mockRejectedValue(
    new DaemonHttpError(404, { error: 'Session not found' }),
  );
  const { input } = await openLiveSession();
  vi.mocked(daemon.getSession).mockRejectedValue(
    Object.assign(new Error('not found'), { status: 404 }),
  );

  await user.type(input, 'Still here{Enter}');

  expect(
    await screen.findByRole('button', { name: 'Restore message' }),
  ).toBeVisible();
  expect(screen.queryByText('Update the daemon')).not.toBeInTheDocument();
  expect(input).toBeEnabled();
});

it('shows an accepted message as its queued run until the reply begins', async () => {
  const user = userEvent.setup();
  const { input, stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await user.type(input, 'Book the train{Enter}');
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(1));
  const { run } = await vi.mocked(daemon.startRun).mock.results[0].value;

  act(() => stream.push(runEvent('run.queued', run, 2)));
  expect(await screen.findByText('Queued')).toBeVisible();
  expect(screen.getByText('Book the train')).toBeVisible();
  act(() =>
    stream.push(
      runEvent(
        'run.started',
        { ...run, status: 'running', startedAtMs: Date.now() },
        3,
      ),
    ),
  );
  await waitFor(() =>
    expect(screen.queryByText('Queued')).not.toBeInTheDocument(),
  );
  expect(screen.getByText('Book the train')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Stop' })).toBeVisible();
});

it('shows a message once when its run arrives before the daemon’s answer', async () => {
  const user = userEvent.setup();
  const answer = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
  vi.mocked(daemon.startRun).mockReturnValue(answer.promise);
  const { input, stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await user.type(input, 'Book the train{Enter}');
  expect(await screen.findByText('Sending…')).toBeVisible();
  const [[, , , key]] = vi.mocked(daemon.startRun).mock.calls;
  const run = runFixture('run_1', {
    sessionId: 'room-7',
    idempotencyKey: key,
    createdAtMs: Date.now(),
    input: { text: 'Book the train', attachmentIds: [], skill: null },
  });

  act(() => stream.push(runEvent('run.queued', run, 2)));
  expect(await screen.findByText('Queued')).toBeVisible();
  expect(screen.getAllByText('Book the train')).toHaveLength(1);
  expect(screen.queryByText('Sending…')).not.toBeInTheDocument();

  await act(async () => answer.resolve({ run }));
  expect(screen.getAllByText('Book the train')).toHaveLength(1);
});

it('shows an accepted message from the ledger without the event stream', async () => {
  const user = userEvent.setup();
  vi.mocked(daemon.sessionRuns).mockImplementation(async () =>
    vi.mocked(daemon.startRun).mock.calls.length > 0
      ? [
          runFixture('run_1', {
            sessionId: 'room-7',
            createdAtMs: Date.now(),
            input: { text: 'Book the train', attachmentIds: [], skill: null },
          }),
        ]
      : [],
  );
  const { input } = await openLiveSession();
  await user.type(input, 'Book the train{Enter}');

  expect(await screen.findByText('Queued')).toBeVisible();
  expect(screen.getByText('Book the train')).toBeVisible();
});

it('says why an accepted message failed and sends it again as a new message', async () => {
  const user = userEvent.setup();
  const { input, stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await user.type(input, 'Book the train{Enter}');
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(1));
  const { run } = await vi.mocked(daemon.startRun).mock.results[0].value;

  act(() =>
    stream.push(
      runEvent(
        'run.failed',
        {
          ...run,
          status: 'failed',
          startedAtMs: Date.now(),
          finishedAtMs: Date.now(),
          error: { code: 'model_error', message: 'provider unavailable' },
        },
        2,
      ),
    ),
  );
  expect(
    await screen.findByText('This reply failed: provider unavailable'),
  ).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(2));
  const startRun = vi.mocked(daemon.startRun);
  expect(startRun.mock.calls[1][2]).toEqual({
    text: 'Book the train',
    mode: 'queue',
  });
  expect(startRun.mock.calls[1][3]).not.toBe(startRun.mock.calls[0][3]);
});

it('drops the thinking indicator and marks the reply read once its run ends', async () => {
  const { stream } = await openLiveSession({ activeRuns: 1 });
  expect(await screen.findByText('Nova is thinking')).toBeVisible();
  const run = runningRun();
  act(() => stream.push(snapshotEvent([snapshotRun(run)])));
  await waitFor(() =>
    expect(screen.queryByText('Nova is thinking')).not.toBeInTheDocument(),
  );
  await waitFor(() => expect(daemon.sessionRuns).toHaveBeenCalledTimes(2));

  // The run ends; the sidebar's next read is slow to answer.
  const listing = deferred<Awaited<ReturnType<typeof daemon.listSessions>>>();
  vi.mocked(daemon.listSessions).mockReturnValueOnce(listing.promise);
  setSessionFields('room-7', { activeRuns: 0, unread: true });
  vi.mocked(daemon.sessionMessages).mockResolvedValue({
    messages: [
      {
        id: 'a1',
        role: 'assistant',
        text: 'Saturday works',
        attachments: [],
        metadata: { runId: 'run_7' },
        createdAtMs: 5,
      },
    ],
    nextBefore: null,
  });
  act(() =>
    stream.push(
      messageCreatedEvent(run, 'a1', 'assistant', 2),
      runEvent(
        'run.completed',
        { ...run, status: 'completed', finishedAtMs: Date.now() },
        3,
      ),
    ),
  );
  expect(await screen.findByText('Saturday works')).toBeVisible();
  // The stream knows the run ended: the stale listing is not believed.
  expect(screen.queryByText('Nova is thinking')).not.toBeInTheDocument();

  await act(async () =>
    listing.resolve({ sessions: [...routes.sessions], nextCursor: null }),
  );
  await waitFor(() =>
    expect(daemon.updateSession).toHaveBeenCalledWith('agent-main', 'room-7', {
      lastReadAtMs: 5,
    }),
  );
});

it('reads a new message into the open session when the stream reports it', async () => {
  const { stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await waitFor(() => expect(daemon.sessionRuns).toHaveBeenCalledTimes(2));
  vi.mocked(daemon.sessionMessages).mockResolvedValue({
    messages: [userMessage('in-1', 'Hello from the API', {}, 4)],
    nextBefore: null,
  });

  act(() =>
    stream.push({
      type: 'message.created',
      agentId: 'agent-main',
      sessionId: 'room-7',
      seq: 2,
      at: 1,
      messageId: 'in-1',
      role: 'user',
      stepId: null,
    }),
  );
  expect(await screen.findByText('Hello from the API')).toBeVisible();
});

it('reads the sidebar, the messages, and the ledger again after a resync', async () => {
  const { stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await waitFor(() => expect(daemon.sessionRuns).toHaveBeenCalledTimes(2));
  const counts = {
    sessions: vi.mocked(daemon.listSessions).mock.calls.length,
    messages: vi.mocked(daemon.sessionMessages).mock.calls.length,
    runs: vi.mocked(daemon.sessionRuns).mock.calls.length,
  };

  act(() => stream.push(resyncEvent(12, 2)));
  await waitFor(() => {
    expect(vi.mocked(daemon.listSessions).mock.calls.length).toBeGreaterThan(
      counts.sessions,
    );
    expect(vi.mocked(daemon.sessionMessages).mock.calls.length).toBeGreaterThan(
      counts.messages,
    );
    expect(vi.mocked(daemon.sessionRuns).mock.calls.length).toBeGreaterThan(
      counts.runs,
    );
  });
});

it('reads a Telegram reply’s run once, even when the ledger does not have it', async () => {
  const user = fakeClock();
  endTelegramRun('completed');
  vi.mocked(daemon.sessionRuns).mockResolvedValue([]);
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  expect(await screen.findByText('Be right there')).toBeVisible();
  await elapse(100);
  const reads = vi.mocked(daemon.sessionRuns).mock.calls.length;
  const polls = vi.mocked(daemon.sessionMessages).mock.calls.length;

  await elapse(SESSION_MESSAGES_POLL_MS * 3);
  expect(vi.mocked(daemon.sessionMessages).mock.calls.length).toBeGreaterThan(
    polls,
  );
  expect(vi.mocked(daemon.sessionRuns).mock.calls.length).toBe(reads);
  expect(
    screen.queryByText('Queued for Telegram delivery'),
  ).not.toBeInTheDocument();
});

it('reports Telegram delivery when the stream says the reply’s run completed', async () => {
  const user = userEvent.setup();
  const events = scriptedAgentEvents();
  const input = await openTelegramSession();
  await waitFor(() => expect(events.streams).toHaveLength(1));
  act(() => events.streams[0].push(snapshotEvent([])));
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(1));
  const { run } = await vi.mocked(daemon.startRun).mock.results[0].value;

  act(() =>
    events.streams[0].push(
      runEvent(
        'run.completed',
        { ...run, status: 'completed', finishedAtMs: Date.now() },
        2,
      ),
    ),
  );
  expect(await screen.findByText('Queued for Telegram delivery')).toBeVisible();
});

it('says it is reconnecting once while its retries keep failing', async () => {
  fakeClock();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  const { stream, events } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  act(() => stream.end());
  const notice = await screen.findByText('Reconnecting…');

  await elapse(STREAM_RETRY_MIN_MS);
  await waitFor(() => expect(events.streams).toHaveLength(2));
  await act(async () =>
    events.streams[1].fail(
      new DaemonConnectionError('', new TypeError('Failed to fetch')),
    ),
  );
  await elapse(0);
  // The same notice stays: nothing new for a screen reader to read.
  expect(screen.getByText('Reconnecting…')).toBe(notice);

  await elapse(STREAM_RETRY_MIN_MS * 2);
  await waitFor(() => expect(events.streams).toHaveLength(3));
  act(() => events.streams[2].push(snapshotEvent([])));
  await waitFor(() =>
    expect(screen.queryByText('Reconnecting…')).not.toBeInTheDocument(),
  );
});

it('opens a helper’s session from its card', async () => {
  const user = userEvent.setup();
  routes.sessions.push(
    sessionFixture('room-9', {
      agentId: 'helper-7',
      kind: 'helper',
      origin: 'delegation',
      title: 'Compare vendors',
      parentAgentId: 'agent-main',
      parentRunId: 'run_7',
      capabilities: readOnly,
      lastActivityAtMs: Date.now() - 1,
    }),
  );
  const { stream } = await openLiveSession();
  const run = runningRun();
  act(() =>
    stream.push(
      snapshotEvent([snapshotRun(run)]),
      toolStartedEvent(
        run,
        'call_h',
        'spawn_helper',
        2,
        '{"name":"Researcher","task":"Compare vendors"}',
      ),
      toolFinishedEvent(run, 'call_h', 'spawn_helper', 3, {
        resultPreview: '{"agentId":"helper-7"}',
      }),
    ),
  );

  await user.click(await screen.findByRole('button', { name: 'Open session' }));
  await waitFor(() => expect(window.location.hash).toBe('#/s/helper-7/room-9'));
  expect(
    await screen.findByRole('heading', { name: 'Compare vendors' }),
  ).toBeVisible();
  // The helper's session is read through the companion's one stream.
  expect(daemon.agentEvents).toHaveBeenCalledTimes(1);
});

it('reads the ledger again when the sidebar shows the open session’s runs changed', async () => {
  fakeClock();
  vi.mocked(daemon.sessionRuns).mockResolvedValue([runningRun()]);
  // The stream never opens: the harness polls.
  await openLiveSession({ activeRuns: 1 });
  expect(await screen.findByRole('button', { name: 'Stop' })).toBeVisible();

  vi.mocked(daemon.sessionRuns).mockResolvedValue([
    {
      ...runningRun(),
      status: 'failed',
      finishedAtMs: Date.now(),
      error: { code: 'model_error', message: 'provider unavailable' },
    },
  ]);
  setSessionFields('room-7', { activeRuns: 0 });
  await elapse(SESSION_LIST_POLL_MS);

  expect(
    await screen.findByText('This reply failed: provider unavailable'),
  ).toBeVisible();
  expect(
    screen.queryByRole('button', { name: 'Stop' }),
  ).not.toBeInTheDocument();
});

it('says the open session was deleted as soon as the stream reports it', async () => {
  const { stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await waitFor(() => expect(daemon.sessionRuns).toHaveBeenCalledTimes(2));
  vi.mocked(daemon.sessionMessages).mockRejectedValue(
    Object.assign(new Error('not found'), { status: 404 }),
  );

  act(() => stream.push(sessionEvent('session.deleted', 'room-7', 2)));
  expect(await screen.findByText('This session was deleted.')).toBeVisible();
});

it('reads how a Telegram reply’s run ended once its messages arrive', async () => {
  const user = userEvent.setup();
  endTelegramRun('completed');
  const ended = vi.mocked(daemon.sessionRuns).getMockImplementation()!;
  const listMessages = vi
    .mocked(daemon.sessionMessages)
    .getMockImplementation()!;
  let replied = false;
  vi.mocked(daemon.sessionMessages).mockImplementation(async (...args) => {
    const page = await listMessages(...args);
    replied ||= page.messages.length > 0;
    return page;
  });
  // The ledger knows how the run ended only once its messages are committed.
  vi.mocked(daemon.sessionRuns).mockImplementation(async (...args) =>
    replied ? ended(...args) : [],
  );
  const input = await openTelegramSession();
  await user.type(input, 'On my way');
  await user.click(screen.getByRole('button', { name: 'Send' }));

  expect(await screen.findByText('Queued for Telegram delivery')).toBeVisible();
});

it('polls slowly while the stream is open and reads full records for Settings', async () => {
  const user = fakeClock();
  vi.spyOn(daemon, 'listAgentSummaries').mockResolvedValue([
    {
      state: snapshot('agent-main', 'Nova', 1).state,
      messageCount: 0,
      eventCount: 0,
      lastTask: null,
    },
  ]);
  const { stream } = await openLiveSession();
  act(() => stream.push(snapshotEvent([])));
  await elapse(LIVE_SETTLE_MS);
  const calls = () => ({
    sessions: vi.mocked(daemon.listSessions).mock.calls.length,
    messages: vi.mocked(daemon.sessionMessages).mock.calls.length,
  });
  const opened = calls();

  await elapse(SESSION_LIST_POLL_MS);
  expect(calls()).toEqual(opened);
  await elapse(SESSION_MESSAGES_LIVE_POLL_MS - SESSION_LIST_POLL_MS);
  expect(calls().messages).toBeGreaterThan(opened.messages);
  expect(calls().sessions).toBe(opened.sessions);
  await elapse(SESSION_LIST_LIVE_POLL_MS - SESSION_MESSAGES_LIVE_POLL_MS);
  expect(calls().sessions).toBeGreaterThan(opened.sessions);

  const fullReads = vi.mocked(daemon.listAgents).mock.calls.length;
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await waitFor(() =>
    expect(vi.mocked(daemon.listAgents).mock.calls.length).toBe(fullReads + 1),
  );
});

it('does not offer a failed reply’s steer twice when the ledger shows it as its own run', async () => {
  const { stream, run, key } = await steerIntoRunningReply();
  // The commit failed: the joined run fails first, and the steer's own
  // `failed_before_start` run is announced only after a second save, so
  // the ledger read after the failure is the first to show it.
  const ledger = deferred<Awaited<ReturnType<typeof daemon.sessionRuns>>>();
  vi.mocked(daemon.sessionRuns).mockReturnValue(ledger.promise);
  const failed = {
    ...run,
    status: 'failed' as const,
    finishedAtMs: Date.now(),
    error: { code: 'commit_failed', message: 'The reply could not be saved' },
  };
  const readMessages = vi.mocked(daemon.sessionMessages);
  const before = readMessages.mock.calls.length;
  act(() => stream.push(runEvent('run.failed', failed, 2)));
  // History, read again at once, lands first, without the steer.
  await waitFor(() =>
    expect(readMessages.mock.calls.length).toBeGreaterThan(before),
  );
  await act(async () => {
    await readMessages.mock.results[before].value;
  });
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
  expect(screen.getByText('Joining the reply in progress…')).toBeVisible();

  await act(async () =>
    ledger.resolve([
      runFixture('run_8', {
        sessionId: 'room-7',
        idempotencyKey: key,
        status: 'interrupted',
        createdAtMs: Date.now(),
        error: {
          code: 'failed_before_start',
          message:
            'The run this message joined failed before reading it; send it again.',
        },
        input: { text: 'also check trains', attachmentIds: [], skill: null },
      }),
      failed,
    ]),
  );
  expect(
    await screen.findByText(
      'The run this message joined failed before reading it; send it again.',
    ),
  ).toBeVisible();
  expect(screen.getByText('also check trains')).toBeVisible();
  expect(
    screen.queryByText('Joining the reply in progress…'),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
});

it('does not announce a session’s last reply again when it is reopened', async () => {
  const user = userEvent.setup();
  routes.sessions.push(
    sessionFixture('chat:other', {
      title: 'Other plans',
      lastActivityAtMs: Date.now() - 1,
    }),
  );
  const { stream } = await openLiveSession();
  const run = runningRun();
  act(() =>
    stream.push(
      snapshotEvent([snapshotRun(run)]),
      runEvent(
        'run.completed',
        { ...run, status: 'completed', finishedAtMs: Date.now() },
        2,
      ),
    ),
  );
  expect(await screen.findByText('Nova replied.')).toBeInTheDocument();
  const region = () => document.querySelector('p.sr-only[aria-live="polite"]');

  await user.click(screen.getByRole('button', { name: 'Other plans' }));
  await screen.findByRole('heading', { name: 'Other plans' });
  expect(region()?.textContent).toBe('');
  await user.click(screen.getByRole('button', { name: 'Weekend plans' }));
  await screen.findByRole('heading', { name: 'Weekend plans' });
  expect(region()?.textContent).toBe('');

  // A new reply there is announced.
  act(() =>
    stream.push(
      runEvent(
        'run.completed',
        runFixture('run_9', {
          sessionId: 'room-7',
          status: 'completed',
          finishedAtMs: Date.now(),
        }),
        3,
      ),
    ),
  );
  await waitFor(() => expect(region()?.textContent).toBe('Nova replied.'));
});

it('shows an accepted message as its queued run right after the daemon accepts it', async () => {
  const user = userEvent.setup();
  const { input } = await openLiveSession();
  // Without the stream, the ledger's next read is slow to answer.
  const ledger = deferred<Awaited<ReturnType<typeof daemon.sessionRuns>>>();
  vi.mocked(daemon.sessionRuns).mockReturnValue(ledger.promise);

  await user.type(input, 'Book the train{Enter}');
  await waitFor(() => expect(daemon.startRun).toHaveBeenCalledTimes(1));
  expect(await screen.findByText('Queued')).toBeVisible();
  expect(screen.getByText('Book the train')).toBeVisible();
  expect(screen.queryByText('Sending…')).not.toBeInTheDocument();
});

it('keeps a failed reply’s steer recovery to the poll’s pace while history cannot be read', async () => {
  const user = fakeClock();
  vi.spyOn(daemon, 'listAgentSummaries').mockResolvedValue([
    {
      state: snapshot('agent-main', 'Nova', 1).state,
      messageCount: 0,
      eventCount: 0,
      lastTask: null,
    },
  ]);
  const { stream, run } = await steerIntoRunningReply(user);
  await elapse(LIVE_SETTLE_MS);
  // The daemon stops answering history right after the reply fails.
  vi.mocked(daemon.sessionMessages).mockRejectedValue(
    new DaemonHttpError(503, { error: 'History is unavailable' }),
  );
  const reads = () => ({
    history: vi.mocked(daemon.sessionMessages).mock.calls.length,
    ledger: vi.mocked(daemon.sessionRuns).mock.calls.length,
  });
  const before = reads();
  act(() =>
    stream.push(
      runEvent(
        'run.failed',
        {
          ...run,
          status: 'failed',
          finishedAtMs: Date.now(),
          error: { code: 'model_error', message: 'provider unavailable' },
        },
        2,
      ),
    ),
  );

  const polls = 10;
  for (let poll = 0; poll < polls; poll += 1)
    await elapse(SESSION_MESSAGES_LIVE_POLL_MS);
  const during = reads();
  expect(during.history - before.history).toBeLessThanOrEqual(2 * polls);
  expect(during.ledger - before.ledger).toBeLessThanOrEqual(2 * polls);
  expect(
    screen.queryByRole('button', { name: 'Restore message' }),
  ).not.toBeInTheDocument();
  expect(screen.getByText('Joining the reply in progress…')).toBeVisible();

  // History answers again: the next poll's read, with the ledger's, moves
  // the steer to the recovery panel.
  vi.mocked(daemon.sessionMessages).mockResolvedValue({
    messages: [],
    nextBefore: null,
  });
  await elapse(SESSION_MESSAGES_LIVE_POLL_MS);
  expect(
    await screen.findByRole('button', { name: 'Restore message' }),
  ).toBeVisible();
  expect(
    screen.queryByText('Joining the reply in progress…'),
  ).not.toBeInTheDocument();
});
