import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import {
  deltaEvent,
  resyncEvent,
  runEvent,
  runFixture,
  scriptedAgentEvents,
  snapshotEvent,
  snapshotRun,
} from '../test/live';
import {
  RESYNC_BACKOFF_AFTER,
  STREAM_RETRY_MAX_MS,
  STREAM_RETRY_MIN_MS,
  retryDelay,
  useAgentEvents,
} from './useAgentEvents';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('retryDelay', () => {
  it('backs off from 1 to 30 seconds with jitter', () => {
    expect(retryDelay(0, () => 0)).toBe(STREAM_RETRY_MIN_MS);
    expect(retryDelay(0, () => 1)).toBe(STREAM_RETRY_MIN_MS);
    expect(retryDelay(1, () => 0)).toBe(1_000);
    expect(retryDelay(1, () => 1)).toBe(2_000);
    expect(retryDelay(3, () => 0.5)).toBe(6_000);
    expect(retryDelay(10, () => 0)).toBe(15_000);
    expect(retryDelay(10, () => 1)).toBe(STREAM_RETRY_MAX_MS);
  });
});

describe('useAgentEvents', () => {
  it('shares one stream per companion and closes it with the last view', async () => {
    const { streams } = scriptedAgentEvents();
    const first = renderHook(() => useAgentEvents('agent-main'));
    const second = renderHook(() => useAgentEvents('agent-main'));

    await waitFor(() => expect(streams).toHaveLength(1));
    first.unmount();
    expect(streams[0].signal?.aborted).toBe(false);
    second.unmount();
    expect(streams[0].signal?.aborted).toBe(true);
  });

  it('applies events and reports the stream open after its snapshot', async () => {
    const { latest } = scriptedAgentEvents();
    const run = runFixture('run_1', { status: 'running' });
    const { result } = renderHook(() => useAgentEvents('agent-main'));
    expect(result.current.status).toBe('connecting');

    act(() =>
      latest().push(
        snapshotEvent([snapshotRun(run)]),
        deltaEvent(run, 'run_1:1', 0, 'Hi', 2),
      ),
    );

    await waitFor(() => expect(result.current.status).toBe('open'));
    await waitFor(() =>
      expect(result.current.state.runs.run_1?.steps[0]?.text).toBe('Hi'),
    );
  });

  it('hands every event to its listener as it arrives', async () => {
    const { latest } = scriptedAgentEvents();
    const onEvent = vi.fn();
    renderHook(() => useAgentEvents('agent-main', onEvent));

    act(() =>
      latest().push(
        snapshotEvent([]),
        runEvent('run.queued', runFixture('run_1'), 2),
      ),
    );

    await waitFor(() => expect(onEvent).toHaveBeenCalledTimes(2));
    expect(onEvent.mock.calls.map(([event]) => event.type)).toEqual([
      'stream.snapshot',
      'run.queued',
    ]);
  });

  it('reconnects with back-off after the stream drops', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const { streams } = scriptedAgentEvents();
    const { result } = renderHook(() => useAgentEvents('agent-main'));

    await act(async () => streams[0].push(snapshotEvent([])));
    await act(async () => streams[0].end());
    await waitFor(() => expect(result.current.status).toBe('reconnecting'));
    expect(streams).toHaveLength(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(STREAM_RETRY_MIN_MS);
    });
    expect(streams).toHaveLength(2);
  });

  it('logs and backs off (never retries at once) on a non-404 failure such as 403 or 429', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { streams } = scriptedAgentEvents();
    const { result } = renderHook(() => useAgentEvents('agent-main'));

    const rateLimited = new DaemonHttpError(429, { error: 'slow down' });
    await act(async () => streams[0].fail(rateLimited));

    await waitFor(() => expect(result.current.status).toBe('reconnecting'));
    expect(warn).toHaveBeenCalled();
    // Not reconnected immediately: still just the one stream.
    expect(streams).toHaveLength(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(STREAM_RETRY_MIN_MS);
    });
    expect(streams).toHaveLength(2);

    warn.mockClear();
    const forbidden = new DaemonHttpError(403, { error: 'no local owner' });
    await act(async () => streams[1].fail(forbidden));
    await waitFor(() => expect(result.current.status).toBe('reconnecting'));
    expect(warn).toHaveBeenCalled();
    expect(streams).toHaveLength(2);
  });

  it('stops for good on a 404 when the agent still exists (the daemon predates the stream)', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(daemon, 'getAgent').mockResolvedValue({
      agent: { state: { id: 'agent-main' } },
    } as never);
    const { streams } = scriptedAgentEvents();
    const { result } = renderHook(() => useAgentEvents('agent-main'));

    await act(async () =>
      streams[0].fail(new DaemonHttpError(404, { error: 'not found' })),
    );
    await waitFor(() => expect(result.current.status).toBe('unsupported'));
    expect(daemon.getAgent).toHaveBeenCalledWith('agent-main');
    await act(async () => {
      await vi.advanceTimersByTimeAsync(STREAM_RETRY_MAX_MS);
    });
    expect(streams).toHaveLength(1);
  });

  it('keeps reconnecting (never "unsupported") on a 404 for an unknown or deleted agent', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(Math, 'random').mockReturnValue(0);
    vi.spyOn(daemon, 'getAgent').mockRejectedValue(
      new DaemonHttpError(404, { error: 'agent not found' }),
    );
    const { streams } = scriptedAgentEvents();
    const { result } = renderHook(() => useAgentEvents('agent-ghost'));

    await act(async () =>
      streams[0].fail(new DaemonHttpError(404, { error: 'not found' })),
    );
    await waitFor(() => expect(result.current.status).toBe('reconnecting'));
    expect(daemon.getAgent).toHaveBeenCalledWith('agent-ghost');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(STREAM_RETRY_MIN_MS);
    });
    expect(streams).toHaveLength(2);
    expect(result.current.status).not.toBe('unsupported');
  });

  it('reconnects at once after a resync so a fresh snapshot replaces what it missed', async () => {
    const { streams } = scriptedAgentEvents();
    const run = runFixture('run_1', { status: 'running' });
    const { result } = renderHook(() => useAgentEvents('agent-main'));

    act(() =>
      streams[0].push(snapshotEvent([snapshotRun(run)]), resyncEvent(40, 2)),
    );
    await waitFor(() => expect(streams).toHaveLength(2));
    expect(streams[0].signal?.aborted).toBe(true);

    act(() => streams[1].push(snapshotEvent([])));
    await waitFor(() =>
      expect(result.current.state.runs.run_1).toBeUndefined(),
    );
  });

  it('backs off once several resyncs land in a row without a normal event between them', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const { streams } = scriptedAgentEvents();
    renderHook(() => useAgentEvents('agent-main'));

    // The first RESYNC_BACKOFF_AFTER resync-only connections reconnect at
    // once, same as a single isolated resync.
    for (let attempt = 0; attempt < RESYNC_BACKOFF_AFTER; attempt += 1) {
      const stream = streams[streams.length - 1];
      const expectedLength = streams.length + 1;
      await act(async () => stream.push(snapshotEvent([]), resyncEvent(1, 2)));
      await waitFor(() => expect(streams).toHaveLength(expectedLength));
    }

    // The next connection also only ever sees a resync: the streak is now
    // RESYNC_BACKOFF_AFTER long, so this one waits for the normal back-off
    // instead of reconnecting immediately.
    const flappingStream = streams[streams.length - 1];
    const beforeBackoff = streams.length;
    await act(async () =>
      flappingStream.push(snapshotEvent([]), resyncEvent(1, 2)),
    );
    expect(streams).toHaveLength(beforeBackoff);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(STREAM_RETRY_MIN_MS);
    });
    expect(streams).toHaveLength(beforeBackoff + 1);
  });

  it('resets the resync streak once a normal event proves the stream is healthy', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const { streams } = scriptedAgentEvents();
    renderHook(() => useAgentEvents('agent-main'));

    for (let attempt = 0; attempt < RESYNC_BACKOFF_AFTER; attempt += 1) {
      const stream = streams[streams.length - 1];
      const expectedLength = streams.length + 1;
      await act(async () => stream.push(snapshotEvent([]), resyncEvent(1, 2)));
      await waitFor(() => expect(streams).toHaveLength(expectedLength));
    }

    // This time the connection also delivers a real event before its
    // resync, which proves the stream is otherwise healthy and resets the
    // streak: the next resync-only connection still reconnects at once.
    const healthyStream = streams[streams.length - 1];
    const run = runFixture('run_1');
    const beforeHealthy = streams.length;
    await act(async () =>
      healthyStream.push(
        snapshotEvent([]),
        runEvent('run.queued', run, 2),
        resyncEvent(1, 3),
      ),
    );
    await waitFor(() => expect(streams).toHaveLength(beforeHealthy + 1));

    const nextStream = streams[streams.length - 1];
    const beforeNext = streams.length;
    await act(async () =>
      nextStream.push(snapshotEvent([]), resyncEvent(1, 2)),
    );
    await waitFor(() => expect(streams).toHaveLength(beforeNext + 1));
  });

  it('stops for good when the daemon has no event stream', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(daemon, 'getAgent').mockResolvedValue({
      agent: { state: { id: 'agent-main' } },
    } as never);
    const { streams } = scriptedAgentEvents();
    const { result } = renderHook(() => useAgentEvents('agent-main'));

    await act(async () =>
      streams[0].fail(new DaemonHttpError(404, { error: 'not found' })),
    );
    await waitFor(() => expect(result.current.status).toBe('unsupported'));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(STREAM_RETRY_MAX_MS);
    });
    expect(streams).toHaveLength(1);
  });
});
