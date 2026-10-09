import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DaemonHttpError,
  type LogEvent,
  type LogLine,
  type LogStreamOptions,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { LOGS_TOO_OLD } from '../lib/logs';
import { logLineFixture } from '../test/system';
import { useLogs, type LogsOptions } from './useLogs';

const options: LogsOptions = {
  enabled: true,
  level: null,
  query: '',
  paused: false,
  reconnectDelay: () => 0,
};

type Item = LogEvent | { fail: unknown } | { end: true };

/** A stream the test drives by hand. It ends when its signal aborts. */
class Channel {
  readonly queue: Item[] = [];
  wake: (() => void) | null = null;

  constructor(readonly options: LogStreamOptions) {}

  send(item: Item) {
    this.queue.push(item);
    this.wake?.();
    this.wake = null;
  }
  line(seq: number) {
    this.send({ kind: 'line', line: logLineFixture(seq) });
  }

  async *events(): AsyncGenerator<LogEvent> {
    const signal = this.options.signal;
    while (!signal?.aborted) {
      const item = this.queue.shift();
      if (!item) {
        await new Promise<void>((resolve) => {
          this.wake = resolve;
          signal?.addEventListener('abort', () => resolve(), { once: true });
        });
        continue;
      }
      if ('fail' in item) throw item.fail;
      if ('end' in item) return;
      yield item;
    }
  }
}

let streams: Channel[];

function page(seqs: number[], newestSeq = seqs.at(-1) ?? 0) {
  return { lines: seqs.map((seq) => logLineFixture(seq)), newestSeq };
}

const seqs = (lines: LogLine[]) => lines.map((line) => line.seq);

beforeEach(() => {
  streams = [];
  vi.spyOn(daemon, 'logs').mockResolvedValue(page([1, 2]));
  vi.spyOn(daemon, 'logStream').mockImplementation((streamOptions) => {
    const channel = new Channel(streamOptions);
    streams.push(channel);
    return channel.events();
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useLogs', () => {
  it('loads the recent lines and then streams new ones after the newest seq', async () => {
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));

    expect(daemon.logs).toHaveBeenCalledWith({
      level: undefined,
      q: undefined,
      limit: 500,
    });
    expect(streams[0].options.after).toBe(2);
    expect(seqs(result.current.lines)).toEqual([1, 2]);
    expect(result.current.loaded).toBe(true);

    await act(async () => streams[0].line(3));
    expect(result.current.connected).toBe(true);
    expect(seqs(result.current.lines)).toEqual([1, 2, 3]);
  });

  it('a duplicate line from the stream is dropped', async () => {
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));
    const before = result.current.lines;

    await act(async () => streams[0].line(2));
    await act(async () => streams[0].line(3));
    expect(seqs(result.current.lines)).toEqual([1, 2, 3]);
    expect(result.current.lines).not.toBe(before);
    const mid = result.current.lines;
    await act(async () => streams[0].line(3));
    expect(result.current.lines).toBe(mid);
  });

  it('a resync refetches after the newest seq', async () => {
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));

    vi.mocked(daemon.logs).mockResolvedValueOnce(page([3, 4]));
    await act(async () => streams[0].send({ kind: 'resync', newestSeq: 4 }));
    await waitFor(() =>
      expect(seqs(result.current.lines)).toEqual([1, 2, 3, 4]),
    );
    expect(vi.mocked(daemon.logs).mock.calls[1][0]).toMatchObject({
      after: 2,
      limit: 500,
    });
  });

  it('a resync with a full page catches up from that page, not past it', async () => {
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));
    await act(async () => streams[0].line(3));

    const full = Array.from({ length: 500 }, (_, index) => 4 + index);
    // The daemon's newest seq is far ahead of the page it returned.
    vi.mocked(daemon.logs)
      .mockResolvedValueOnce(page(full, 900))
      .mockResolvedValueOnce(page([504, 505], 900));
    await act(async () => streams[0].send({ kind: 'resync', newestSeq: 900 }));
    await waitFor(() => expect(result.current.lines).toHaveLength(505));

    const calls = vi.mocked(daemon.logs).mock.calls;
    expect(calls[1][0]).toMatchObject({ after: 3 });
    expect(calls[2][0]).toMatchObject({ after: 503 });
    expect(seqs(result.current.lines).slice(-3)).toEqual([503, 504, 505]);
  });

  it('is not connected, and keeps a failure, until the stream delivers', async () => {
    vi.mocked(daemon.logStream).mockImplementationOnce(() => {
      // eslint-disable-next-line require-yield
      return (async function* (): AsyncGenerator<LogEvent> {
        throw new DaemonHttpError(429, {
          error: 'Too many log streams are open',
        });
      })();
    });
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));
    // The stream is open but has said nothing.
    expect(result.current.connected).toBe(false);
    expect(result.current.error).toBe('Too many log streams are open');

    await act(async () => streams[0].line(3));
    expect(result.current.connected).toBe(true);
    expect(result.current.error).toBeNull();
  });

  it('a closed stream reconnects after the delay and resumes after the newest seq', async () => {
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));
    await act(async () => streams[0].line(3));

    await act(async () => streams[0].send({ end: true }));
    await waitFor(() => expect(streams).toHaveLength(2));
    expect(streams[1].options.after).toBe(3);

    await act(async () => streams[1].send({ fail: new TypeError('offline') }));
    await waitFor(() => expect(streams).toHaveLength(3));
    expect(streams[2].options.after).toBe(3);
    expect(daemon.logs).toHaveBeenCalledTimes(1);

    await act(async () => streams[2].line(4));
    expect(seqs(result.current.lines)).toEqual([1, 2, 3, 4]);
    expect(result.current.error).toBeNull();
  });

  it('shows the daemon message when it refuses the stream', async () => {
    vi.mocked(daemon.logStream).mockImplementationOnce(() => {
      // eslint-disable-next-line require-yield
      return (async function* (): AsyncGenerator<LogEvent> {
        throw new DaemonHttpError(429, {
          error: 'Too many log streams are open',
        });
      })();
    });
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() =>
      expect(result.current.error).toBe('Too many log streams are open'),
    );
    expect(result.current.errorStatus).toBe(429);
    // It keeps trying, and a stream that opens and delivers clears it.
    await waitFor(() => expect(streams).toHaveLength(1));
    await act(async () => streams[0].line(3));
    expect(result.current.error).toBeNull();
  });

  it('pausing holds new lines and resuming merges them', async () => {
    const { result, rerender } = renderHook(
      (props: LogsOptions) => useLogs(props),
      { initialProps: options },
    );
    await waitFor(() => expect(streams).toHaveLength(1));

    rerender({ ...options, paused: true });
    await act(async () => streams[0].line(3));
    await act(async () => streams[0].line(4));
    expect(result.current.held).toBe(2);
    expect(seqs(result.current.lines)).toEqual([1, 2]);

    rerender({ ...options, paused: false });
    expect(result.current.held).toBe(0);
    expect(seqs(result.current.lines)).toEqual([1, 2, 3, 4]);
  });

  it('changing the filter aborts the old stream and ignores its late lines', async () => {
    const { result, rerender } = renderHook(
      (props: LogsOptions) => useLogs(props),
      { initialProps: options },
    );
    await waitFor(() => expect(streams).toHaveLength(1));

    vi.mocked(daemon.logs).mockResolvedValue(page([10]));
    rerender({ ...options, level: 'warn', query: 'disk' });
    await waitFor(() => expect(streams).toHaveLength(2));

    expect(streams[0].options.signal?.aborted).toBe(true);
    expect(daemon.logs).toHaveBeenLastCalledWith({
      level: 'warn',
      q: 'disk',
      limit: 500,
    });
    expect(streams[1].options).toMatchObject({
      level: 'warn',
      q: 'disk',
      after: 10,
    });
    await act(async () => streams[0].line(99));
    expect(seqs(result.current.lines)).toEqual([10]);
  });

  it('unmounting aborts the stream', async () => {
    const { unmount } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));
    unmount();
    expect(streams[0].options.signal?.aborted).toBe(true);
  });

  it('a 404 sets the update text', async () => {
    vi.mocked(daemon.logs).mockRejectedValue(
      new DaemonHttpError(404, { error: 'Not found' }),
    );
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(result.current.error).toBe(LOGS_TOO_OLD));
    expect(result.current.errorStatus).toBe(404);
    expect(daemon.logStream).not.toHaveBeenCalled();
  });

  it('opens nothing while disabled', async () => {
    renderHook(() => useLogs({ ...options, enabled: false }));
    await act(async () => undefined);
    expect(daemon.logs).not.toHaveBeenCalled();
    expect(daemon.logStream).not.toHaveBeenCalled();
  });

  it('an empty reload keeps the empty array', async () => {
    vi.mocked(daemon.logs).mockResolvedValue(page([], 0));
    const { result } = renderHook(() => useLogs(options));
    await waitFor(() => expect(streams).toHaveLength(1));
    const before = result.current.lines;

    act(() => result.current.refresh());
    await waitFor(() => expect(streams).toHaveLength(2));
    expect(result.current.lines).toBe(before);
    expect(result.current.lines).toHaveLength(0);
  });
});
