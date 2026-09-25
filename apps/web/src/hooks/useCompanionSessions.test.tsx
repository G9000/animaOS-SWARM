import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  DaemonTooOldError,
  type Session,
  type SessionListOptions,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { sessionFixture } from '../test/sessions';
import {
  SESSION_LIST_MAX_PAGES,
  SESSION_LIST_POLL_MS,
  useCompanionSessions,
} from './useCompanionSessions';

const nativeSetTimeout = window.setTimeout.bind(window);

function ids(sessions: readonly Session[]): string[] {
  return sessions.map((session) => session.id);
}

/** Catches the 10 s poll's callback instead of arming a real timer. */
function capturePoll() {
  const poll: { run?: () => void; armed: number } = { armed: 0 };
  vi.spyOn(window, 'setTimeout').mockImplementation(((
    handler: TimerHandler,
    timeout?: number,
  ) => {
    if (typeof handler === 'function' && timeout === SESSION_LIST_POLL_MS) {
      poll.armed += 1;
      poll.run = handler as () => void;
      return 1;
    }
    return nativeSetTimeout(handler, timeout);
  }) as typeof window.setTimeout);
  return poll;
}

/**
 * A daemon that lists `order` (or what `order` gives for the request's
 * filters) one session per page, with a `page-N` cursor for page N. While
 * held, every request waits for `releaseAll`, which answers them (oldest or
 * newest first) from the listing as it is then.
 */
function pagedDaemon(
  order: Session[] | ((options: SessionListOptions) => Session[]),
) {
  let listing = order;
  let holding = false;
  const held: Array<() => void> = [];
  const answer = (options: SessionListOptions) => {
    const sessions = typeof listing === 'function' ? listing(options) : listing;
    const page = options.cursor
      ? Number(options.cursor.slice('page-'.length))
      : 1;
    return {
      sessions: sessions.slice(page - 1, page),
      nextCursor: page < sessions.length ? `page-${page + 1}` : null,
    };
  };
  const list = vi
    .spyOn(daemon, 'listSessions')
    .mockImplementation((_agentId: string, options: SessionListOptions = {}) =>
      holding
        ? new Promise((resolve) => held.push(() => resolve(answer(options))))
        : Promise.resolve(answer(options)),
    );
  return {
    list,
    setOrder(next: Session[]) {
      listing = next;
    },
    hold() {
      holding = true;
    },
    async releaseAll(newestFirst = false) {
      holding = false;
      while (held.length > 0) {
        const release = newestFirst ? held.pop() : held.shift();
        await act(async () => {
          release?.();
        });
      }
    },
  };
}

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

    await waitFor(() =>
      expect(result.current.error).toBe('daemon unavailable'),
    );
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

  it('exposes hasMore from the cursor and adds the next page via loadMore', async () => {
    const server = pagedDaemon([
      sessionFixture('chat:1'),
      sessionFixture('chat:2'),
    ]);
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );

    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    expect(result.current.hasMore).toBe(true);

    await act(async () => {
      await result.current.loadMore();
    });

    expect(ids(result.current.sessions)).toEqual(['chat:1', 'chat:2']);
    expect(result.current.hasMore).toBe(false);
    expect(result.current.loadingMore).toBe(false);
    expect(server.list).toHaveBeenLastCalledWith('agent-main', {
      includeHelpers: true,
      archived: false,
      limit: 200,
      cursor: 'page-2',
    });
  });

  it('removes a session regardless of which page it was loaded from', async () => {
    pagedDaemon([sessionFixture('chat:1'), sessionFixture('chat:2')]);
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await act(async () => {
      await result.current.loadMore();
    });
    expect(ids(result.current.sessions)).toEqual(['chat:1', 'chat:2']);

    act(() => result.current.remove(sessionFixture('chat:2')));
    expect(ids(result.current.sessions)).toEqual(['chat:1']);
  });

  it('keeps a session that moves from page 2 to page 1 between polls exactly once', async () => {
    const poll = capturePoll();
    const a = sessionFixture('chat:a');
    const b = sessionFixture('chat:b');
    const server = pagedDaemon([a, b]);
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await act(async () => {
      await result.current.loadMore();
    });
    expect(ids(result.current.sessions)).toEqual(['chat:a', 'chat:b']);

    // chat:b gets new activity: it is page 1 now, and chat:a slides to page 2.
    server.setOrder([b, a]);
    await waitFor(() => expect(poll.run).toBeDefined());
    await act(async () => {
      poll.run?.();
    });

    await waitFor(() =>
      expect(ids(result.current.sessions)).toEqual(['chat:b', 'chat:a']),
    );
    expect(result.current.hasMore).toBe(false);
  });

  it('lists a session that two pages both hold once, as the first page has it', async () => {
    const a = sessionFixture('chat:a', { title: 'page 1 copy' });
    const b = sessionFixture('chat:b');
    vi.spyOn(daemon, 'listSessions').mockImplementation(
      async (_agentId: string, options: SessionListOptions = {}) =>
        options.cursor
          ? {
              sessions: [sessionFixture('chat:a', { title: 'page 2 copy' }), b],
              nextCursor: null,
            }
          : { sessions: [a], nextCursor: 'page-2' },
    );
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.hasMore).toBe(true));

    await act(async () => {
      await result.current.loadMore();
    });

    expect(result.current.sessions.map((session) => session.title)).toEqual([
      'page 1 copy',
      'New chat',
    ]);
    expect(ids(result.current.sessions)).toEqual(['chat:a', 'chat:b']);
  });

  it('shows a rename of a session on page 2 after the refresh', async () => {
    const a = sessionFixture('chat:a', { title: 'A' });
    const server = pagedDaemon([a, sessionFixture('chat:b', { title: 'B' })]);
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await act(async () => {
      await result.current.loadMore();
    });

    server.setOrder([a, sessionFixture('chat:b', { title: 'B renamed' })]);
    await act(async () => {
      await result.current.refresh();
    });

    expect(result.current.sessions.map((session) => session.title)).toEqual([
      'A',
      'B renamed',
    ]);
  });

  it.each([
    ['in order', false],
    ['newest first', true],
  ])(
    'never leaves loadingMore stuck when a poll overlaps loadMore, and ends with every page (replies %s)',
    async (_order, newestFirst) => {
      const poll = capturePoll();
      const server = pagedDaemon([
        sessionFixture('chat:a'),
        sessionFixture('chat:b'),
      ]);
      const { result } = renderHook(() =>
        useCompanionSessions('agent-main', { archived: false, query: '' }),
      );
      await waitFor(() => expect(result.current.sessions).toHaveLength(1));
      await waitFor(() => expect(poll.run).toBeDefined());

      server.hold();
      act(() => {
        void result.current.loadMore();
      });
      expect(result.current.loadingMore).toBe(true);
      // The poll lands while loadMore's request is still in flight.
      act(() => {
        poll.run?.();
      });
      await server.releaseAll(newestFirst);

      await waitFor(() => expect(result.current.loadingMore).toBe(false));
      expect(ids(result.current.sessions)).toEqual(['chat:a', 'chat:b']);
      expect(result.current.hasMore).toBe(false);
    },
  );

  it('keeps the previous list, loading, until the first page of a new filter lands', async () => {
    const server = pagedDaemon((options) =>
      options.archived
        ? [sessionFixture('chat:z')]
        : [sessionFixture('chat:a')],
    );
    const lengths: number[] = [];
    const { result, rerender } = renderHook(
      ({ archived }) => {
        const listed = useCompanionSessions('agent-main', {
          archived,
          query: '',
        });
        lengths.push(listed.sessions.length);
        return listed;
      },
      { initialProps: { archived: false } },
    );
    await waitFor(() =>
      expect(ids(result.current.sessions)).toEqual(['chat:a']),
    );
    const loaded = lengths.length;

    server.hold();
    rerender({ archived: true });
    await waitFor(() => expect(result.current.loading).toBe(true));
    expect(ids(result.current.sessions)).toEqual(['chat:a']);

    await server.releaseAll();
    await waitFor(() =>
      expect(ids(result.current.sessions)).toEqual(['chat:z']),
    );
    expect(lengths.slice(loaded)).not.toContain(0);
    expect(result.current.loading).toBe(false);
  });

  it('keeps hasMore false across polls once every page is loaded', async () => {
    const poll = capturePoll();
    pagedDaemon([sessionFixture('chat:a'), sessionFixture('chat:b')]);
    const hasMore: boolean[] = [];
    const { result } = renderHook(() => {
      const listed = useCompanionSessions('agent-main', {
        archived: false,
        query: '',
      });
      hasMore.push(listed.hasMore);
      return listed;
    });
    await waitFor(() => expect(result.current.hasMore).toBe(true));
    await act(async () => {
      await result.current.loadMore();
    });
    expect(result.current.hasMore).toBe(false);
    const loaded = hasMore.length;

    for (let round = 0; round < 2; round += 1) {
      await waitFor(() => expect(poll.run).toBeDefined());
      const run = poll.run;
      poll.run = undefined;
      await act(async () => {
        run?.();
      });
      await waitFor(() => expect(poll.run).toBeDefined());
    }

    expect(ids(result.current.sessions)).toEqual(['chat:a', 'chat:b']);
    expect(hasMore.slice(loaded)).not.toContain(true);
  });

  it('stops scheduling further polls once the daemon is flagged too old, and a successful refresh re-arms them', async () => {
    const poll = capturePoll();
    const list = vi
      .spyOn(daemon, 'listSessions')
      .mockResolvedValueOnce({
        sessions: [sessionFixture('chat:1')],
        nextCursor: null,
      })
      .mockRejectedValueOnce(new DaemonTooOldError('agent-main'));
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );

    // The first refresh succeeds and arms the routine 10 s poll.
    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    await waitFor(() => expect(poll.armed).toBe(1));

    // That poll flags the daemon as too old.
    await act(async () => {
      poll.run?.();
    });
    await waitFor(() => expect(result.current.daemonTooOld).toBe(true));
    expect(list).toHaveBeenCalledTimes(2);
    // No further timer is armed once the daemon is flagged too old.
    expect(poll.armed).toBe(1);

    // A manual refresh still asks the daemon, clears the flag, and re-arms
    // the 10 s poll (R3).
    list.mockResolvedValue({ sessions: [], nextCursor: null });
    await act(async () => {
      await result.current.refresh();
    });
    expect(list).toHaveBeenCalledTimes(3);
    expect(result.current.daemonTooOld).toBe(false);
    await waitFor(() => expect(poll.armed).toBe(2));
    await act(async () => {
      poll.run?.();
    });
    expect(list).toHaveBeenCalledTimes(4);
  });

  it('stops adding pages at the page cap', async () => {
    pagedDaemon(
      Array.from({ length: SESSION_LIST_MAX_PAGES + 2 }, (_, index) =>
        sessionFixture(`chat:${index}`),
      ),
    );
    const { result } = renderHook(() =>
      useCompanionSessions('agent-main', { archived: false, query: '' }),
    );
    await waitFor(() => expect(result.current.hasMore).toBe(true));

    for (let page = 1; page < SESSION_LIST_MAX_PAGES + 2; page += 1) {
      await act(async () => {
        await result.current.loadMore();
      });
    }

    expect(result.current.sessions).toHaveLength(SESSION_LIST_MAX_PAGES);
    expect(result.current.hasMore).toBe(false);
    expect(result.current.loadingMore).toBe(false);
  });
});
