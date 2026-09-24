import type { SessionMessage } from '@animaOS-SWARM/sdk';
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../lib/daemon-api';
import {
  SESSION_MESSAGES_POLL_MS,
  mergeNewest,
  useSessionMessages,
} from './useSessionMessages';

const nativeSetTimeout = window.setTimeout.bind(window);

function message(id: string, createdAtMs: number): SessionMessage {
  return {
    id,
    role: 'assistant',
    text: id,
    attachments: [],
    metadata: {},
    createdAtMs,
  };
}

function capturePolls() {
  const polls: (() => void)[] = [];
  vi.spyOn(window, 'setTimeout').mockImplementation(((
    handler: TimerHandler,
    timeout?: number,
  ) => {
    if (typeof handler === 'function' && timeout === SESSION_MESSAGES_POLL_MS) {
      polls.push(handler as () => void);
      return polls.length;
    }
    return nativeSetTimeout(handler, timeout);
  }) as typeof window.setTimeout);
  return polls;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSessionMessages', () => {
  it('loads the newest page, then older pages without duplicates', async () => {
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockImplementation(async (_agentId, _sessionId, options = {}) =>
        options.before
          ? { messages: [message('m1', 1), message('m2', 2)], nextBefore: null }
          : { messages: [message('m3', 3), message('m4', 4)], nextBefore: 'm3' },
      );
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:1'),
    );

    await waitFor(() =>
      expect(result.current.messages.map((item) => item.id)).toEqual(['m3', 'm4']),
    );
    expect(result.current.hasOlder).toBe(true);
    expect(pages).toHaveBeenCalledWith('agent-main', 'chat:1', { limit: 50 });

    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.messages.map((item) => item.id)).toEqual([
      'm1',
      'm2',
      'm3',
      'm4',
    ]);
    expect(result.current.hasOlder).toBe(false);
    expect(pages).toHaveBeenLastCalledWith('agent-main', 'chat:1', {
      before: 'm3',
      limit: 50,
    });
  });

  it('merges the polled newest page and reloads for a new refresh key', async () => {
    const polls = capturePolls();
    let newest = [message('m1', 1)];
    vi.spyOn(daemon, 'sessionMessages').mockImplementation(async () => ({
      messages: newest,
      nextBefore: null,
    }));
    const { result, rerender } = renderHook(
      ({ refreshKey }) => useSessionMessages('agent-main', 'chat:1', refreshKey),
      { initialProps: { refreshKey: 0 } },
    );
    const ids = () => result.current.messages.map((item) => item.id);

    await waitFor(() => expect(ids()).toEqual(['m1']));
    newest = [message('m1', 1), message('m2', 2)];
    await waitFor(() => expect(polls.length).toBeGreaterThan(0));
    await act(async () => {
      polls[polls.length - 1]();
    });
    await waitFor(() => expect(ids()).toEqual(['m1', 'm2']));

    newest = [message('m1', 1), message('m2', 2), message('m3', 3)];
    rerender({ refreshKey: 1 });
    await waitFor(() => expect(ids()).toEqual(['m1', 'm2', 'm3']));
  });

  it('reopens hasOlder when a poll gap would otherwise strand history', async () => {
    // Controller ruling 2 (M2 pre-flight audit): once older history has been
    // loaded, a normal poll must not move `nextBefore` (it would discard the
    // client's paging progress). But if the hot tail later rolls so far that
    // the newest page no longer connects to what is loaded (a gap — messages
    // arrived faster than the poll), freezing `nextBefore` would leave that
    // gap permanently unreachable. Such a poll must reopen `hasOlder` instead.
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockImplementation(async (_agentId, _sessionId, options = {}) =>
        options.before === 'm5'
          ? {
              messages: [
                message('m1', 1),
                message('m2', 2),
                message('m3', 3),
                message('m4', 4),
              ],
              nextBefore: null,
            }
          : { messages: [message('m5', 5), message('m6', 6)], nextBefore: 'm5' },
      );
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:1'),
    );

    await waitFor(() =>
      expect(result.current.messages.map((item) => item.id)).toEqual(['m5', 'm6']),
    );
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.messages.map((item) => item.id)).toEqual([
      'm1',
      'm2',
      'm3',
      'm4',
      'm5',
      'm6',
    ]);
    expect(result.current.hasOlder).toBe(false);

    // The hot tail rolled past what this client has (m7..m19 arrived unseen
    // between polls): the next newest page no longer connects to `messages`.
    pages.mockResolvedValue({
      messages: [message('m20', 20), message('m21', 21)],
      nextBefore: 'm20',
    });
    await act(async () => {
      await result.current.refresh();
    });

    expect(result.current.messages.map((item) => item.id)).toEqual([
      'm1',
      'm2',
      'm3',
      'm4',
      'm5',
      'm6',
      'm20',
      'm21',
    ]);
    expect(result.current.hasOlder).toBe(true);
    expect(pages).toHaveBeenLastCalledWith('agent-main', 'chat:1', { limit: 50 });
  });

  it('reports a deleted session and stops polling it', async () => {
    const polls = capturePolls();
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockRejectedValue(Object.assign(new Error('not found'), { status: 404 }));
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:gone'),
    );

    await waitFor(() => expect(result.current.missing).toBe(true));
    expect(polls).toHaveLength(0);
    expect(pages).toHaveBeenCalledTimes(1);
  });

  it('keeps loaded older messages when the newest page moves on', () => {
    expect(
      mergeNewest(
        [message('m1', 1), message('m2', 2), message('m3', 3)],
        [message('m2', 2), message('m3', 3), message('m4', 4)],
      ).map((item) => item.id),
    ).toEqual(['m1', 'm2', 'm3', 'm4']);
    expect(mergeNewest([message('m1', 1)], [])).toEqual([]);
  });

  it('merges a page that does not connect, keeping everything older (a gap)', () => {
    expect(
      mergeNewest(
        [message('m1', 1), message('m2', 2)],
        [message('m20', 20), message('m21', 21)],
      ).map((item) => item.id),
    ).toEqual(['m1', 'm2', 'm20', 'm21']);
  });
});
