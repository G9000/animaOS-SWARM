import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../../lib/daemon-api';
import { memoryFixture } from '../../test/memory';

import { emptyLiveRun } from '../../lib/session-events';
import type { AgentDetail } from '../../lib/types';
import { automationFixture } from '../../test/automations';
import { runFixture } from '../../test/live';
import { sessionFixture } from '../../test/sessions';
import { totalsFixture } from '../../test/usage';
import { SessionView, type SessionViewProps } from './SessionView';

const agent: AgentDetail = {
  id: 'agent-main',
  name: 'Nova',
  provider: 'openai',
  model: 'gpt-5.4',
  toolNames: [],
  created_at_ms: 1,
  status: 'Idle',
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  messages: [],
};
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

function renderView(overrides: Partial<SessionViewProps> = {}) {
  const props: SessionViewProps = {
    agent,
    session: null,
    messages: [],
    hasOlder: false,
    loadingOlder: false,
    onLoadOlder: vi.fn(),
    missing: false,
    telegramAvailable: true,
    scrollerRef: createRef<HTMLDivElement>(),
    onSuggestion: vi.fn(),
    composer: {
      draft: '',
      setDraft: vi.fn(),
      sending: false,
      disabled: false,
      offline: false,
      onSend: vi.fn(),
      error: null,
      onDismissError: vi.fn(),
    },
    onNewChat: vi.fn(),
    onOpenWork: vi.fn(),
    onRename: vi.fn().mockResolvedValue(true),
    onToggleArchived: vi.fn(),
    onExport: vi.fn(),
    ...overrides,
  };
  const { rerender } = render(<SessionView {...props} />);
  return {
    ...props,
    rerender: (next: Partial<SessionViewProps>) =>
      rerender(<SessionView {...props} {...next} />),
  };
}

beforeEach(() => {
  // The header reads the session's usage; most tests have none.
  vi.spyOn(daemon, 'getSession').mockResolvedValue(sessionFixture('room-7'));
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SessionView', () => {
  it('saves a message to the viewed session’s agent as a memory', async () => {
    const user = userEvent.setup();
    const saveMemory = vi
      .spyOn(daemon, 'saveMemory')
      .mockResolvedValue(memoryFixture('mem-1'));
    renderView({
      agent: { ...agent, id: 'helper-7', name: 'Researcher' },
      session: sessionFixture('room-9', {
        agentId: 'helper-7',
        kind: 'helper',
      }),
      messages: [
        {
          id: 'm1',
          role: 'Assistant',
          content: { text: 'Found the answer' },
          created_at_ms: 1,
        },
      ],
    });

    await user.click(screen.getByRole('button', { name: 'Save to memory' }));
    expect(
      await screen.findByRole('button', { name: '✓ Saved to memory' }),
    ).toBeDisabled();
    expect(saveMemory).toHaveBeenCalledWith(
      expect.objectContaining({
        agentId: 'helper-7',
        agentName: 'Researcher',
        sessionId: 'room-9',
        content: 'Found the answer',
      }),
    );
  });

  it('saves a helper’s message with the session’s own agent when the agent shown is the companion', async () => {
    const user = userEvent.setup();
    const saveMemory = vi
      .spyOn(daemon, 'saveMemory')
      .mockResolvedValue(memoryFixture('mem-2'));
    // The helper is not in the agents list, so the page passes the companion.
    renderView({
      session: sessionFixture('room-10', {
        agentId: 'helper-9',
        kind: 'helper',
      }),
      messages: [
        {
          id: 'm1',
          role: 'Assistant',
          content: { text: 'Found it' },
          created_at_ms: 1,
        },
      ],
    });

    await user.click(screen.getByRole('button', { name: 'Save to memory' }));
    expect(
      await screen.findByRole('button', { name: '✓ Saved to memory' }),
    ).toBeDisabled();
    const saved = saveMemory.mock.calls[0][0];
    expect(saved.agentId).toBe('helper-9');
    expect(saved.agentName).not.toBe('Nova');
    expect(saved.agentName).toBeTruthy();
  });

  it('the header shows the session’s usage line and hides it when there are no calls', async () => {
    vi.mocked(daemon.getSession).mockResolvedValue(
      sessionFixture('room-7', {
        usage: totalsFixture({
          calls: 2,
          totalTokens: 12_300,
          costMicros: 40_000,
        }),
      }),
    );
    renderView({ session: sessionFixture('room-7') });
    expect(await screen.findByText('12.3k tokens · $0.04')).toBeVisible();
    expect(daemon.getSession).toHaveBeenCalledWith('agent-main', 'room-7');
  });

  it('shows no usage line for a session without calls', async () => {
    vi.mocked(daemon.getSession).mockResolvedValue(
      sessionFixture('room-7', { usage: totalsFixture() }),
    );
    renderView({ session: sessionFixture('room-7') });
    await waitFor(() => expect(daemon.getSession).toHaveBeenCalled());
    expect(screen.queryByText(/tokens ·/)).not.toBeInTheDocument();
  });

  it('the line refreshes when a run finishes', async () => {
    const working = runFixture('run_1', { status: 'working' });
    const done = runFixture('run_1', { status: 'completed' });
    vi.mocked(daemon.getSession).mockResolvedValue(
      sessionFixture('room-7', {
        usage: totalsFixture({
          calls: 1,
          totalTokens: 100,
          costMicros: 10_000,
        }),
      }),
    );
    const props = renderView({
      session: sessionFixture('room-7'),
      runs: [emptyLiveRun(working)],
    });
    expect(await screen.findByText('100 tokens · $0.01')).toBeVisible();
    expect(daemon.getSession).toHaveBeenCalledTimes(1);

    vi.mocked(daemon.getSession).mockResolvedValue(
      sessionFixture('room-7', {
        usage: totalsFixture({
          calls: 2,
          totalTokens: 250,
          costMicros: 20_000,
        }),
      }),
    );
    props.rerender({ runs: [emptyLiveRun(done)] });
    expect(await screen.findByText('250 tokens · $0.02')).toBeVisible();
    expect(daemon.getSession).toHaveBeenCalledTimes(2);
  });

  it('opens a new chat on the welcome screen with the companion composer', () => {
    renderView();
    expect(
      screen.getByRole('heading', { name: 'Say something to Nova' }),
    ).toBeVisible();
    expect(screen.getByPlaceholderText('Message Nova…')).toBeVisible();
  });

  it('shows a chat with its header, older history, rename, archive, and export', async () => {
    const user = userEvent.setup();
    const props = renderView({
      session: sessionFixture('chat:plans', { title: 'Plans' }),
      messages: [
        {
          id: 'm1',
          role: 'Assistant',
          content: { text: 'Here is the plan' },
          created_at_ms: 1,
        },
      ],
      hasOlder: true,
    });

    expect(screen.getByRole('heading', { name: 'Plans' })).toBeVisible();
    expect(
      screen.getByText('Chat', { selector: '.session-kind-badge' }),
    ).toBeVisible();
    await user.click(
      screen.getByRole('button', { name: 'Load older messages' }),
    );
    expect(props.onLoadOlder).toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    const title = screen.getByRole('textbox', { name: 'Session title' });
    await user.clear(title);
    await user.type(title, 'Offsite{Enter}');
    expect(props.onRename).toHaveBeenCalledWith('Offsite');
    await user.click(screen.getByRole('button', { name: 'Archive' }));
    expect(props.onToggleArchived).toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Export' }));
    expect(props.onExport).toHaveBeenCalled();
  });

  it('replies in a Telegram session through the Telegram composer', () => {
    renderView({
      session: sessionFixture('telegram:tg-1', {
        kind: 'telegram',
        title: 'Telegram · @nova_bot',
      }),
    });
    expect(screen.getByPlaceholderText('Reply on Telegram…')).toBeVisible();
  });

  it('replies to a check-in through its own composer', () => {
    renderView({
      session: sessionFixture('schedule:daily', {
        kind: 'checkin',
        origin: 'schedule',
        title: 'Check-in · goals',
      }),
    });
    expect(
      screen.getByPlaceholderText('Reply to this check-in…'),
    ).toBeVisible();
  });

  it('shows a message that is still on its way', () => {
    renderView({
      session: sessionFixture('chat:plans', { title: 'Plans' }),
      pending: [
        {
          key: 'k1',
          text: 'Book the train',
          createdAtMs: 1,
          status: 'retrying',
        },
      ],
    });
    expect(screen.getByText('Book the train')).toBeVisible();
    expect(screen.getByText('Not delivered yet · retrying…')).toBeVisible();
  });

  it('shows a read-only note instead of a composer for jobs', async () => {
    const props = renderView({
      session: sessionFixture('job:1', {
        kind: 'job',
        title: 'Job · Report',
        capabilities: readOnly,
      }),
    });

    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.getByRole('note')).toHaveTextContent(
      'Job sessions are read-only',
    );
    expect(screen.getByText('No messages yet.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Open Work' }));
    expect(props.onOpenWork).toHaveBeenCalled();
  });

  it('shows the reply in progress, outcomes, and where the companion’s view begins', async () => {
    const user = userEvent.setup();
    const onCompact = vi.fn();
    const onSendAgain = vi.fn();
    const failed = runFixture('run_f', {
      sessionId: 'chat:plans',
      status: 'failed',
      error: { code: 'model_error', message: 'provider unavailable' },
    });
    const working = runFixture('run_w', {
      sessionId: 'chat:plans',
      status: 'running',
      createdAtMs: 5,
      startedAtMs: Date.now(),
      input: { text: 'And Sunday?', attachmentIds: [], skill: null },
    });
    renderView({
      session: sessionFixture('chat:plans', {
        title: 'Plans',
        activeRuns: 1,
        contextTrimmed: { droppedThroughMessageId: 'm1', atMs: 1 },
      }),
      messages: [
        {
          id: 'm1',
          role: 'User',
          content: { text: 'Plan Saturday', metadata: { runId: 'run_f' } },
          created_at_ms: 1,
        },
      ],
      runs: [
        emptyLiveRun(failed),
        {
          ...emptyLiveRun(working),
          steps: [{ stepId: 'run_w:1', text: 'Sunday is free', textOffset: 0 }],
        },
      ],
      actions: { onCompact, onSendAgain },
    });

    expect(
      screen.getByText('Earlier messages are outside the companion’s view'),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Compact' }));
    expect(onCompact).toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onSendAgain).toHaveBeenCalledWith(failed);
    expect(screen.getByText('And Sunday?')).toBeVisible();
    expect(screen.getByText('Sunday is free')).toBeVisible();
    // The live run speaks for itself: no second "thinking" indicator.
    expect(screen.queryByText('Nova is thinking')).not.toBeInTheDocument();
  });

  it('announces a finished reply and says when earlier messages could not be summarized', () => {
    renderView({
      session: sessionFixture('chat:plans', {
        title: 'Plans',
        compactionError: { message: 'model unavailable', atMs: 2 },
      }),
      announcement: 'Nova replied.',
    });

    expect(screen.getByText('Nova replied.')).toHaveAttribute(
      'aria-live',
      'polite',
    );
    const note = screen.getByText(
      'Earlier messages could not be summarized: model unavailable',
    );
    expect(note).toBeVisible();
    // The note sits inside a status region that stays mounted, so its text
    // is announced when it arrives (S3b-E).
    expect(note.closest('[role="status"]')).not.toBeNull();
  });

  it('still says the companion is thinking about a run it has no view of', () => {
    renderView({
      session: sessionFixture('chat:plans', { title: 'Plans', activeRuns: 1 }),
      runs: [
        emptyLiveRun(
          runFixture('run_old', {
            sessionId: 'chat:plans',
            status: 'completed',
            finishedAtMs: 1,
          }),
        ),
      ],
    });

    expect(screen.getByText('Nova is thinking')).toBeVisible();
  });

  it('offers a new chat when the session was deleted elsewhere', async () => {
    const props = renderView({
      session: sessionFixture('chat:gone'),
      missing: true,
    });
    expect(screen.getByText('This session was deleted.')).toBeVisible();
    await userEvent.click(
      screen.getByRole('button', { name: 'Start a new chat' }),
    );
    expect(props.onNewChat).toHaveBeenCalled();
  });

  it('names a check-in’s schedule and opens its automation', async () => {
    const user = userEvent.setup();
    const automation = automationFixture('daily');
    const props = renderView({
      session: sessionFixture('schedule:daily', {
        kind: 'checkin',
        origin: 'schedule',
        title: 'Check-in · goals',
      }),
      automation,
      onEditAutomation: vi.fn(),
    });

    expect(screen.getByText('Check-in · every 30 min')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Edit automation' }));
    expect(props.onEditAutomation).toHaveBeenCalledWith(automation);
  });
});
