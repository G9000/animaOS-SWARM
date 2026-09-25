import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonTooOldError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { sessionFixture } from '../test/sessions';
import {
  SESSION_LIST_POLL_MS,
  useCompanionSessions,
} from './useCompanionSessions';

const nativeSetTimeout = window.setTimeout.bind(window);

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useCompanionSessions', () => {
  it('loads helper sessions too and reloads for a new search', async () => {
    const list = vi.spyOn(daemon, 'listSessions').mockResolvedValue({
      sessions: [sessionFixture('chat:1')],
      nextCursor: null,
    });
    const { result, rerender } = renderHook(
      ({ query }) =>
        useCompanionSessions('agent-main', { archived: false, query }),
      { initialProps: { query: '' } },
    );

    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    expect(list).toHaveBeenLastCalledWith('agent-main', {
      includeHelpers: true,
      archived: false,
      limit: 200,
    });
    rerender({ query: ' budget ' });
    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith('agent-main', {
        includeHelpers: true,
        archived: false,
        limit: 200,
        q: 'budget',
      }),
    );
  });

  it('polls on its own interval and keeps the list when a poll fails', async () => {
    let poll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === SESSION_LIST_POLL_MS) {
        poll = handler as () => void;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);
    const list = vi
      .spyOn(daemon, 'listSessions')
      .mockResolvedValueOnce({
        sessions: [sessionFixture('chat:1')],
        nextCursor: null,
      })
      .mockRejectedValueOnce(new Error('daemon unavailable'));
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );

    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await waitFor(() => expect(poll).toBeDefined());
    await act(async () => {
      poll?.();
    });

    await waitFor(() => expect(result.current.error).toBe('daemon unavailable'));
    expect(result.current.sessions).toHaveLength(1);
    expect(list).toHaveBeenCalledTimes(2);
  });

  it('shows a created session at once and forgets a deleted one', async () => {
    vi.spyOn(daemon, 'listSessions').mockResolvedValue({
      sessions: [sessionFixture('chat:1')],
      nextCursor: null,
    });
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));

    act(() => result.current.upsert(sessionFixture('chat:2')));
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:2',
      'chat:1',
    ]);
    act(() => result.current.remove(sessionFixture('chat:1')));
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:2',
    ]);
  });

  it('flags a daemon that predates the sessions routes', async () => {
    vi.spyOn(daemon, 'listSessions').mockRejectedValue(
      new DaemonTooOldError('agent-main'),
    );
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );

    await waitFor(() => expect(result.current.daemonTooOld).toBe(true));
  });

  it('exposes hasMore from the cursor and appends the next page via loadMore', async () => {
    const list = vi
      .spyOn(daemon, 'listSessions')
      .mockResolvedValueOnce({
        sessions: [sessionFixture('chat:1')],
        nextCursor: 'cursor-1',
      })
      .mockResolvedValueOnce({
        sessions: [sessionFixture('chat:2')],
        nextCursor: null,
      });
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );

    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    expect(result.current.hasMore).toBe(true);

    await act(async () => {
      await result.current.loadMore();
    });

    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:1',
      'chat:2',
    ]);
    expect(result.current.hasMore).toBe(false);
    expect(list).toHaveBeenLastCalledWith('agent-main', {
      includeHelpers: true,
      archived: false,
      limit: 200,
      cursor: 'cursor-1',
    });
  });

  it('merges a poll into the first page only, keeping already-loaded older pages', async () => {
    let poll: (() => void) | undefined;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === SESSION_LIST_POLL_MS) {
        poll = handler as () => void;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);
    const list = vi.spyOn(daemon, 'listSessions').mockResolvedValueOnce({
      sessions: [sessionFixture('chat:1', { title: 'One' })],
      nextCursor: 'cursor-1',
    });
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));

    list.mockResolvedValueOnce({
      sessions: [sessionFixture('chat:2', { title: 'Two' })],
      nextCursor: 'cursor-2',
    });
    await act(async () => {
      await result.current.loadMore();
    });
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:1',
      'chat:2',
    ]);

    // The 10 s poll refreshes only the first page; "chat:2" (loaded via
    // loadMore) must survive, and "chat:1" must be updated in place.
    list.mockResolvedValueOnce({
      sessions: [sessionFixture('chat:1', { title: 'One (renamed)' })],
      nextCursor: 'cursor-1',
    });
    await waitFor(() => expect(poll).toBeDefined());
    await act(async () => {
      poll?.();
    });

    await waitFor(() =>
      expect(result.current.sessions.map((session) => session.title)).toEqual([
        'One (renamed)',
        'Two',
      ]),
    );
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:1',
      'chat:2',
    ]);
    expect(list).toHaveBeenNthCalledWith(3, 'agent-main', {
      includeHelpers: true,
      archived: false,
      limit: 200,
    });
  });

  it('removes a session regardless of which page it was loaded from', async () => {
    vi.spyOn(daemon, 'listSessions')
      .mockResolvedValueOnce({
        sessions: [sessionFixture('chat:1')],
        nextCursor: 'cursor-1',
      })
      .mockResolvedValueOnce({
        sessions: [sessionFixture('chat:2')],
        nextCursor: null,
      });
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await act(async () => {
      await result.current.loadMore();
    });
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:1',
      'chat:2',
    ]);

    act(() => result.current.remove(sessionFixture('chat:2')));
    expect(result.current.sessions.map((session) => session.id)).toEqual([
      'chat:1',
    ]);
  });

  it('stops scheduling further polls once the daemon is flagged too old, but a manual refresh still works', async () => {
    let poll: (() => void) | undefined;
    let scheduledCount = 0;
    vi.spyOn(window, 'setTimeout').mockImplementation(((
      handler: TimerHandler,
      timeout?: number,
    ) => {
      if (typeof handler === 'function' && timeout === SESSION_LIST_POLL_MS) {
        scheduledCount += 1;
        poll = handler as () => void;
        return 1;
      }
      return nativeSetTimeout(handler, timeout);
    }) as typeof window.setTimeout);
    const list = vi
      .spyOn(daemon, 'listSessions')
      .mockResolvedValueOnce({ sessions: [sessionFixture('chat:1')], nextCursor: null })
      .mockRejectedValueOnce(new DaemonTooOldError('agent-main'));
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );

    // The first refresh succeeds and arms the routine 10 s poll.
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await waitFor(() => expect(scheduledCount).toBe(1));

    // That poll flags the daemon as too old.
    await act(async () => {
      poll?.();
    });
    await waitFor(() => expect(result.current.daemonTooOld).toBe(true));
    expect(list).toHaveBeenCalledTimes(2);
    // No further timer is armed once the daemon is flagged too old.
    expect(scheduledCount).toBe(1);

    // A manual refresh still asks the daemon, and can clear the flag again.
    list.mockResolvedValueOnce({ sessions: [], nextCursor: null });
    await act(async () => {
      await result.current.refresh();
    });
    expect(list).toHaveBeenCalledTimes(3);
    expect(result.current.daemonTooOld).toBe(false);
  });
});
