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
});
