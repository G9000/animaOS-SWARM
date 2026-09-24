import type { SessionMessage, SessionMessagePage } from '@animaOS-SWARM/sdk';
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

  it('replaces the list and reopens hasOlder when a poll gap disconnects from it', async () => {
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

    // Controller ruling on ruling 2: a gap replaces the list outright rather
    // than splicing older messages back in (which risked misordering once
    // `loadOlder` later re-fetched the skipped range — see the next test).
    expect(result.current.messages.map((item) => item.id)).toEqual(['m20', 'm21']);
    expect(result.current.hasOlder).toBe(true);
    expect(pages).toHaveBeenLastCalledWith('agent-main', 'chat:1', { limit: 50 });
  });

  it('keeps strict chronological order with no duplicates or drops after loadOlder fills a gap', async () => {
    let newest = [message('m5', 5), message('m6', 6)];
    let newestCursor: string | null = 'm5';
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockImplementation(async (_agentId, _sessionId, options = {}) => {
        if (options.before === 'm5')
          return {
            messages: [
              message('m1', 1),
              message('m2', 2),
              message('m3', 3),
              message('m4', 4),
            ],
            nextBefore: null,
          };
        if (options.before === 'm20')
          return {
            messages: [message('m17', 17), message('m18', 18), message('m19', 19)],
            nextBefore: null,
          };
        return { messages: newest, nextBefore: newestCursor };
      });
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

    // A gap: the hot tail rolled past m7..m19 between polls.
    newest = [message('m20', 20), message('m21', 21)];
    newestCursor = 'm20';
    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.messages.map((item) => item.id)).toEqual(['m20', 'm21']);
    expect(result.current.hasOlder).toBe(true);
    expect(pages).toHaveBeenLastCalledWith('agent-main', 'chat:1', { limit: 50 });

    // Scrolling back from the new frontier re-fetches the skipped range and
    // must merge it in, in order, with no duplicate or dropped ids.
    await act(async () => {
      await result.current.loadOlder();
    });

    const ids = result.current.messages.map((item) => item.id);
    expect(ids).toEqual(['m17', 'm18', 'm19', 'm20', 'm21']);
    expect(new Set(ids).size).toBe(ids.length);
    expect(result.current.messages.map((item) => item.createdAtMs)).toEqual([
      17, 18, 19, 20, 21,
    ]);
  });

  it('keeps the newer refresh in place when an older one resolves last', async () => {
    const polls = capturePolls();
    let resolveFirst: ((page: SessionMessagePage) => void) | undefined;
    let resolveSecond: ((page: SessionMessagePage) => void) | undefined;
    let calls = 0;
    vi.spyOn(daemon, 'sessionMessages').mockImplementation(
      () =>
        new Promise<SessionMessagePage>((resolve) => {
          calls += 1;
          if (calls === 1) resolveFirst = resolve;
          else resolveSecond = resolve;
        }),
    );
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:1'),
    );
    await waitFor(() => expect(resolveFirst).toBeDefined());

    // A second, overlapping refresh (e.g. a `refreshKey` reload) starts
    // before the first (e.g. a routine poll) has resolved.
    act(() => {
      void result.current.refresh();
    });
    await waitFor(() => expect(resolveSecond).toBeDefined());

    // The newer call resolves first…
    await act(async () => {
      resolveSecond?.({
        messages: [message('m1', 1), message('m2', 2)],
        nextBefore: null,
      });
    });
    await waitFor(() =>
      expect(result.current.messages.map((item) => item.id)).toEqual(['m1', 'm2']),
    );

    // …then the older call resolves last, with a smaller/stale page — it
    // must be discarded outright, not roll the newer result back.
    await act(async () => {
      resolveFirst?.({ messages: [message('m1', 1)], nextBefore: null });
    });
    await waitFor(() => expect(polls.length).toBeGreaterThan(0));
    expect(result.current.messages.map((item) => item.id)).toEqual(['m1', 'm2']);
  });

  it('drops an older page that no longer attaches to the list after a gap replaces it', async () => {
    let resolveOlder: ((page: SessionMessagePage) => void) | undefined;
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockImplementation(async (_agentId, _sessionId, options = {}) => {
        if (options.before === 'm20') {
          return new Promise<SessionMessagePage>((resolve) => {
            resolveOlder = resolve;
          });
        }
        return {
          messages: [message('m20', 20), message('m21', 21)],
          nextBefore: 'm20',
        };
      });
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:1'),
    );
    await waitFor(() =>
      expect(result.current.messages.map((item) => item.id)).toEqual(['m20', 'm21']),
    );

    // loadOlder fires "before m20" and is left in flight.
    let older: Promise<void> | undefined;
    act(() => {
      older = result.current.loadOlder();
    });
    await waitFor(() => expect(resolveOlder).toBeDefined());
    expect(result.current.loadingOlder).toBe(true);

    // While that fetch is outstanding, a further gap replaces the list —
    // "before m20" no longer attaches to anything in `messages`.
    pages.mockResolvedValue({
      messages: [message('m22', 22), message('m23', 23)],
      nextBefore: 'm22',
    });
    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.messages.map((item) => item.id)).toEqual(['m22', 'm23']);

    // The stale "before m20" page now resolves; it must be dropped outright
    // rather than merged onto a list it no longer attaches to (Important,
    // M2 pre-flight audit fix round 2), and `loadingOlder` must still clear.
    await act(async () => {
      resolveOlder?.({
        messages: [message('m17', 17), message('m18', 18), message('m19', 19)],
        nextBefore: 'm5',
      });
      await older;
    });

    expect(result.current.messages.map((item) => item.id)).toEqual(['m22', 'm23']);
    expect(result.current.loadingOlder).toBe(false);
    expect(result.current.hasOlder).toBe(true);

    // nextBefore must still be the replacement page's cursor ('m22'), not
    // the stale page's ('m5') — proven by what the next loadOlder requests.
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(pages).toHaveBeenLastCalledWith('agent-main', 'chat:1', {
      before: 'm22',
      limit: 50,
    });
  });

  it('still prepends the older page when a non-gap poll runs while it is in flight', async () => {
    let resolveOlder: ((page: SessionMessagePage) => void) | undefined;
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockImplementation(async (_agentId, _sessionId, options = {}) => {
        if (options.before === 'm5') {
          return new Promise<SessionMessagePage>((resolve) => {
            resolveOlder = resolve;
          });
        }
        return { messages: [message('m5', 5), message('m6', 6)], nextBefore: 'm5' };
      });
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:1'),
    );
    await waitFor(() =>
      expect(result.current.messages.map((item) => item.id)).toEqual(['m5', 'm6']),
    );

    let older: Promise<void> | undefined;
    act(() => {
      older = result.current.loadOlder();
    });
    await waitFor(() => expect(resolveOlder).toBeDefined());

    // A normal (non-gap) poll runs while the older fetch is outstanding: it
    // extends the tail without moving the head, so the anchor still matches.
    pages.mockResolvedValue({
      messages: [message('m5', 5), message('m6', 6), message('m7', 7)],
      nextBefore: 'm5',
    });
    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.messages.map((item) => item.id)).toEqual([
      'm5',
      'm6',
      'm7',
    ]);

    await act(async () => {
      resolveOlder?.({
        messages: [
          message('m1', 1),
          message('m2', 2),
          message('m3', 3),
          message('m4', 4),
        ],
        nextBefore: null,
      });
      await older;
    });

    expect(result.current.messages.map((item) => item.id)).toEqual([
      'm1',
      'm2',
      'm3',
      'm4',
      'm5',
      'm6',
      'm7',
    ]);
    expect(result.current.hasOlder).toBe(false);
    expect(result.current.loadingOlder).toBe(false);
  });

  it('keeps an older-page error through polls until older messages load or the session changes', async () => {
    const polls = capturePolls();
    let olderFails = true;
    vi.spyOn(daemon, 'sessionMessages').mockImplementation(
      async (_agentId, _sessionId, options = {}) => {
        if (!options.before)
          return { messages: [message('m3', 3)], nextBefore: 'm3' };
        if (olderFails)
          throw Object.assign(new Error('history store is unavailable'), {
            status: 503,
          });
        return { messages: [message('m2', 2)], nextBefore: 'm2' };
      },
    );
    const { result, rerender } = renderHook(
      ({ sessionId }) => useSessionMessages('agent-main', sessionId),
      { initialProps: { sessionId: 'chat:1' } },
    );
    await waitFor(() => expect(result.current.hasOlder).toBe(true));

    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.error).toBe('history store is unavailable');
    // A successful newest-page poll does not clear it.
    await waitFor(() => expect(polls.length).toBeGreaterThan(0));
    await act(async () => {
      polls[polls.length - 1]();
    });
    expect(result.current.error).toBe('history store is unavailable');

    olderFails = false;
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.error).toBeNull();
    expect(result.current.messages.map((item) => item.id)).toEqual(['m2', 'm3']);

    olderFails = true;
    await act(async () => {
      await result.current.loadOlder();
    });
    expect(result.current.error).toBe('history store is unavailable');
    rerender({ sessionId: 'chat:2' });
    expect(result.current.error).toBeNull();
  });

  it('clears a newest-page error once a later poll succeeds', async () => {
    const polls = capturePolls();
    const pages = vi
      .spyOn(daemon, 'sessionMessages')
      .mockRejectedValueOnce(new Error('daemon unavailable'))
      .mockResolvedValue({ messages: [message('m1', 1)], nextBefore: null });
    const { result } = renderHook(() =>
      useSessionMessages('agent-main', 'chat:1'),
    );

    await waitFor(() => expect(result.current.error).toBe('daemon unavailable'));
    await waitFor(() => expect(polls.length).toBeGreaterThan(0));
    await act(async () => {
      polls[polls.length - 1]();
    });
    await waitFor(() => expect(result.current.error).toBeNull());
    expect(pages).toHaveBeenCalledTimes(2);
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

  it('replaces the list outright when a page does not connect (a gap)', () => {
    expect(
      mergeNewest(
        [message('m1', 1), message('m2', 2)],
        [message('m20', 20), message('m21', 21)],
      ).map((item) => item.id),
    ).toEqual(['m20', 'm21']);
  });

  it('does not drop a same-millisecond message (ties resolve by id, not time)', () => {
    const a = message('m1', 100);
    const b = message('m2', 100);
    const c = message('m3', 100);
    expect(mergeNewest([a, b], [b, c]).map((item) => item.id)).toEqual([
      'm1',
      'm2',
      'm3',
    ]);
  });
});
