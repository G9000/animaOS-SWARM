import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionMessage } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import {
  messageCreatedEvent,
  runEvent,
  runFixture,
  scriptedAgentEvents,
  sessionEvent,
  snapshotEvent,
  snapshotRun,
} from '../test/live';
import {
  LIVE_REFRESH_DELAY_MS,
  useLiveSession,
  type LiveSessionOptions,
} from './useLiveSession';

const NO_MESSAGES: SessionMessage[] = [];

function setup(initial: Partial<LiveSessionOptions> = {}) {
  const events = scriptedAgentEvents();
  const refreshSessions = vi.fn();
  const refreshMessages = vi.fn();
  const view = renderHook(
    (props: Partial<LiveSessionOptions>) =>
      useLiveSession({
        agentId: 'agent-main',
        session: { agentId: 'agent-main', sessionId: 'room-7' },
        replyName: 'Nova',
        listedActiveRuns: null,
        messages: NO_MESSAGES,
        refreshSessions,
        refreshMessages,
        ...props,
      }),
    { initialProps: initial },
  );
  return { ...view, events, refreshSessions, refreshMessages };
}

/** Lets the scripted stream deliver what was pushed, then runs due timers. */
async function flush(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

function room7Run() {
  return runFixture('run_7', {
    sessionId: 'room-7',
    status: 'running',
    startedAtMs: 1,
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(daemon, 'sessionRuns').mockResolvedValue([]);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('useLiveSession', () => {
  it('refreshes what a burst of events changed once it settles', async () => {
    const { events, refreshSessions, refreshMessages } = setup();
    await flush();
    const stream = events.streams[0];
    stream.push(snapshotEvent([]));
    await flush(LIVE_REFRESH_DELAY_MS);
    refreshSessions.mockClear();
    refreshMessages.mockClear();
    const runs = vi.mocked(daemon.sessionRuns).mock.calls.length;

    const run = room7Run();
    stream.push(
      runEvent('run.started', run, 2),
      messageCreatedEvent(run, 'm1', 'user', 3),
      sessionEvent('session.updated', 'chat:other', 4),
    );
    await flush(LIVE_REFRESH_DELAY_MS - 1);
    expect(refreshSessions).not.toHaveBeenCalled();
    expect(refreshMessages).not.toHaveBeenCalled();
    await flush(1);
    expect(refreshSessions).toHaveBeenCalledTimes(1);
    expect(refreshMessages).toHaveBeenCalledTimes(1);
    expect(vi.mocked(daemon.sessionRuns).mock.calls.length).toBe(runs + 1);
  });

  it('ignores another session’s messages but still refreshes the sidebar', async () => {
    const { events, refreshSessions, refreshMessages } = setup();
    await flush();
    events.streams[0].push(snapshotEvent([]));
    await flush(LIVE_REFRESH_DELAY_MS);
    refreshSessions.mockClear();
    refreshMessages.mockClear();

    const other = runFixture('run_o', { sessionId: 'chat:other' });
    events.streams[0].push(
      runEvent('run.completed', { ...other, status: 'completed' }, 2),
      messageCreatedEvent(other, 'm1', 'assistant', 3),
    );
    await flush(LIVE_REFRESH_DELAY_MS);
    expect(refreshSessions).toHaveBeenCalledTimes(1);
    expect(refreshMessages).not.toHaveBeenCalled();
  });

  it('drops a pending refresh when the view unmounts', async () => {
    const { events, refreshSessions, refreshMessages, unmount } = setup();
    await flush();
    events.streams[0].push(snapshotEvent([]));
    await flush(LIVE_REFRESH_DELAY_MS);
    refreshSessions.mockClear();
    refreshMessages.mockClear();

    events.streams[0].push(
      runEvent('run.completed', { ...room7Run(), status: 'completed' }, 2),
    );
    await flush();
    unmount();
    await flush(LIVE_REFRESH_DELAY_MS * 2);
    expect(refreshSessions).not.toHaveBeenCalled();
    expect(refreshMessages).not.toHaveBeenCalled();
  });

  it('drops the refreshes of a session left behind but keeps the sidebar’s', async () => {
    const { events, refreshSessions, refreshMessages, rerender } = setup();
    await flush();
    events.streams[0].push(snapshotEvent([]));
    await flush(LIVE_REFRESH_DELAY_MS);
    refreshSessions.mockClear();
    refreshMessages.mockClear();
    const runs = vi.mocked(daemon.sessionRuns).mock.calls.length;

    events.streams[0].push(
      runEvent('run.completed', { ...room7Run(), status: 'completed' }, 2),
    );
    await flush();
    rerender({ session: { agentId: 'agent-main', sessionId: 'chat:b' } });
    await flush();
    // Opening chat:b reads its ledger once; room-7's refresh is gone.
    const opened = vi.mocked(daemon.sessionRuns).mock.calls.length;
    expect(opened).toBe(runs + 1);
    await flush(LIVE_REFRESH_DELAY_MS * 2);
    expect(refreshMessages).not.toHaveBeenCalled();
    expect(vi.mocked(daemon.sessionRuns).mock.calls.length).toBe(opened);
    expect(refreshSessions).toHaveBeenCalledTimes(1);
  });

  it('drops every pending refresh when the companion changes', async () => {
    const { events, refreshSessions, refreshMessages, rerender } = setup();
    await flush();
    events.streams[0].push(snapshotEvent([]));
    await flush(LIVE_REFRESH_DELAY_MS);
    refreshSessions.mockClear();
    refreshMessages.mockClear();

    events.streams[0].push(sessionEvent('session.created', 'chat:new', 2));
    await flush();
    rerender({ agentId: 'agent-next', session: null });
    await flush(LIVE_REFRESH_DELAY_MS * 2);
    expect(refreshSessions).not.toHaveBeenCalled();
  });

  it('announces a finished reply once per run, only for the open session', async () => {
    const { events, result, rerender } = setup();
    await flush();
    const stream = events.streams[0];
    const run = room7Run();
    stream.push(snapshotEvent([snapshotRun(run)]));
    await flush(20);
    expect(result.current.announcement).toBe('');

    const done = { ...run, status: 'completed' as const, finishedAtMs: 2 };
    stream.push(runEvent('run.completed', done, 2));
    await flush(20);
    expect(result.current.announcement).toBe('Nova replied.');
    stream.push(runEvent('run.completed', done, 3), snapshotEvent([], 4));
    await flush(20);
    expect(result.current.announcement).toBe('Nova replied.');

    rerender({ session: { agentId: 'agent-main', sessionId: 'chat:b' } });
    expect(result.current.announcement).toBe('');
    await flush();
  });

  it('re-reads the ledger once for a run its history shows but the view thinks is still running', async () => {
    vi.mocked(daemon.sessionRuns).mockResolvedValue([room7Run()]);
    const { rerender, result } = setup();
    await flush();
    expect(result.current.activeRun?.id).toBe('run_7');
    const reads = vi.mocked(daemon.sessionRuns).mock.calls.length;

    const reply: SessionMessage = {
      id: 'a1',
      role: 'assistant',
      text: 'Done',
      attachments: [],
      metadata: { runId: 'run_7' },
      createdAtMs: 3,
    };
    rerender({ messages: [reply] });
    await flush();
    expect(vi.mocked(daemon.sessionRuns).mock.calls.length).toBe(reads + 1);
    // Checked once: a later page of the same history reads nothing more.
    rerender({ messages: [{ ...reply }] });
    await flush();
    expect(vi.mocked(daemon.sessionRuns).mock.calls.length).toBe(reads + 1);
  });
});
