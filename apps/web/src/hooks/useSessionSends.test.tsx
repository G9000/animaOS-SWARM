import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonConnectionError, DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { runFixture } from '../test/live';
import {
  SEND_RETRY_DELAYS_MS,
  isRetryableSendError,
  useSessionSends,
  type NewSessionSend,
} from './useSessionSends';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function message(key: string, sessionId = 'chat:1'): NewSessionSend {
  return {
    key,
    agentId: 'agent-main',
    sessionId,
    conversation: `agent-main\u0000session:${sessionId}`,
    text: `text ${key}`,
    mode: 'queue',
    telegram: false,
  };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

describe('isRetryableSendError', () => {
  it('retries only failures a retry can fix', () => {
    expect(
      isRetryableSendError(new DaemonConnectionError('', new Error('down'))),
    ).toBe(true);
    for (const status of [408, 502, 504])
      expect(isRetryableSendError(new DaemonHttpError(status, null))).toBe(
        true,
      );
    for (const status of [400, 409, 429, 503])
      expect(isRetryableSendError(new DaemonHttpError(status, null))).toBe(
        false,
      );
    expect(isRetryableSendError(new Error('boom'))).toBe(false);
  });
});

describe('useSessionSends', () => {
  it('sends a skill message with its skill', async () => {
    const startRun = vi.spyOn(daemon, 'startRun').mockResolvedValue({
      run: runFixture('run_1'),
    });
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn() }),
    );

    act(() => result.current.send({ ...message('k1'), skill: 'notes' }));

    await waitFor(() =>
      expect(startRun).toHaveBeenCalledWith(
        'agent-main',
        'chat:1',
        { text: 'text k1', mode: 'queue', skill: 'notes' },
        'k1',
      ),
    );
  });

  it('sends one message at a time per session, in the order written', async () => {
    const first = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
    const startRun = vi
      .spyOn(daemon, 'startRun')
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue({ run: runFixture('run_2') });
    const onAccepted = vi.fn();
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted, onFailed: vi.fn() }),
    );

    act(() => {
      result.current.send(message('a'));
      result.current.send(message('b'));
      result.current.send(message('c', 'chat:2'));
    });

    expect(
      startRun.mock.calls.map(([, sessionId, , key]) => [sessionId, key]),
    ).toEqual([
      ['chat:1', 'a'],
      ['chat:2', 'c'],
    ]);
    await waitFor(() =>
      expect(result.current.sends.map((send) => send.key)).toEqual(['a', 'b']),
    );
    await act(async () => first.resolve({ run: runFixture('run_1') }));
    await waitFor(() => expect(startRun).toHaveBeenCalledTimes(3));
    expect(startRun.mock.calls[2][3]).toBe('b');
    await waitFor(() => expect(result.current.sends).toEqual([]));
    expect(onAccepted.mock.calls.map(([send]) => send.key)).toEqual([
      'c',
      'a',
      'b',
    ]);
  });

  it('retries a send that may not have arrived with its key, then gives up', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const failure = new DaemonConnectionError('', new Error('down'));
    const startRun = vi.spyOn(daemon, 'startRun').mockRejectedValue(failure);
    const onFailed = vi.fn();
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed }),
    );

    act(() => result.current.send(message('k')));
    await waitFor(() => expect(result.current.sends[0]?.failures).toBe(1));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(
        SEND_RETRY_DELAYS_MS.reduce((sum, delay) => sum + delay, 0),
      );
    });

    await waitFor(() => expect(onFailed).toHaveBeenCalled());
    expect(startRun).toHaveBeenCalledTimes(SEND_RETRY_DELAYS_MS.length + 1);
    expect(new Set(startRun.mock.calls.map(([, , , key]) => key))).toEqual(
      new Set(['k']),
    );
    expect(onFailed).toHaveBeenCalledWith(
      expect.objectContaining({ key: 'k', failures: 3 }),
      failure,
    );
    expect(result.current.sends).toEqual([]);
  });

  it('waits 1, 2, then 4 seconds before each retry', async () => {
    vi.useFakeTimers();
    const startRun = vi
      .spyOn(daemon, 'startRun')
      .mockRejectedValue(new DaemonHttpError(502, null));
    const onFailed = vi.fn();
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed }),
    );
    const elapse = (ms: number) =>
      act(async () => {
        await vi.advanceTimersByTimeAsync(ms);
      });

    act(() => result.current.send(message('k')));
    await elapse(0);
    for (const [index, delay] of SEND_RETRY_DELAYS_MS.entries()) {
      await elapse(delay - 1);
      expect(startRun).toHaveBeenCalledTimes(index + 1);
      await elapse(1);
      expect(startRun).toHaveBeenCalledTimes(index + 2);
    }
    expect(onFailed).toHaveBeenCalledTimes(1);
  });

  it('gives an answer from the daemon back at once', async () => {
    const refused = new DaemonHttpError(429, {
      error:
        'This companion already has 8 queued messages; wait for one to start',
    });
    const startRun = vi.spyOn(daemon, 'startRun').mockRejectedValue(refused);
    const onFailed = vi.fn();
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed }),
    );

    act(() => result.current.send(message('k')));

    await waitFor(() =>
      expect(onFailed).toHaveBeenCalledWith(
        expect.objectContaining({ key: 'k' }),
        refused,
      ),
    );
    expect(startRun).toHaveBeenCalledTimes(1);
  });

  it('keeps a steer until the run it joined applies it', async () => {
    vi.spyOn(daemon, 'startRun').mockResolvedValue({
      run: runFixture('run_1', { status: 'running' }),
      steer: { status: 'pending' },
    });
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn() }),
    );

    act(() => result.current.send({ ...message('s'), mode: 'steer' }));
    await waitFor(() =>
      expect(result.current.sends[0]?.steeringRunId).toBe('run_1'),
    );
    act(() => result.current.settle('s'));
    expect(result.current.sends).toEqual([]);
  });

  it('keeps a retried steer that the daemon answers as a steer', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    // The first attempt joined the run, but its answer was lost.
    vi.spyOn(daemon, 'startRun')
      .mockRejectedValueOnce(new DaemonConnectionError('', new Error('down')))
      .mockResolvedValue({
        run: runFixture('run_1', { status: 'running' }),
        steer: { status: 'pending' },
      });
    const onAccepted = vi.fn();
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted, onFailed: vi.fn() }),
    );

    act(() => result.current.send({ ...message('s'), mode: 'steer' }));
    await waitFor(() => expect(result.current.sends[0]?.failures).toBe(1));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(SEND_RETRY_DELAYS_MS[0]);
    });

    await waitFor(() =>
      expect(result.current.sends[0]?.steeringRunId).toBe('run_1'),
    );
    expect(result.current.sends.map((send) => send.key)).toEqual(['s']);
    expect(onAccepted).toHaveBeenCalledTimes(1);
  });

  it('forgets a deleted companion’s sends without reporting them', async () => {
    const first = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
    const startRun = vi
      .spyOn(daemon, 'startRun')
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue({ run: runFixture('run_2') });
    const onAccepted = vi.fn();
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted, onFailed: vi.fn() }),
    );

    act(() => {
      result.current.send(message('a'));
      result.current.send(message('b'));
    });
    act(() => result.current.forgetAgent('agent-main'));
    expect(result.current.sends).toEqual([]);
    await act(async () => first.resolve({ run: runFixture('run_1') }));

    expect(onAccepted).not.toHaveBeenCalled();
    expect(startRun).toHaveBeenCalledTimes(1);
    // The session is free again for the next message.
    await act(async () => result.current.send(message('c')));
    expect(startRun).toHaveBeenCalledTimes(2);
    expect(onAccepted.mock.calls.map(([send]) => send.key)).toEqual(['c']);
  });

  it('persists each unaccepted send, and drops it once the daemon accepts it (S3b-A)', async () => {
    const first = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
    vi.spyOn(daemon, 'startRun').mockReturnValue(first.promise);
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn() }),
    );

    act(() => result.current.send(message('a')));
    expect(
      JSON.parse(sessionStorage.getItem('animaos.pendingSends') ?? 'null'),
    ).toEqual([
      {
        key: 'a',
        text: 'text a',
        conversation: 'agent-main\u0000session:chat:1',
        createdAtMs: expect.any(Number),
      },
    ]);

    await act(async () => first.resolve({ run: runFixture('run_1') }));
    await waitFor(() => expect(result.current.sends).toEqual([]));
    expect(sessionStorage.getItem('animaos.pendingSends')).toBeNull();
  });

  it('does not save a steer the daemon already accepted, so a reload never offers it again', async () => {
    vi.spyOn(daemon, 'startRun').mockResolvedValue({
      run: runFixture('run_1', { status: 'running' }),
      steer: { status: 'pending' },
    });
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn() }),
    );

    act(() => result.current.send({ ...message('s'), mode: 'steer' }));
    await waitFor(() =>
      expect(result.current.sends[0]?.steeringRunId).toBe('run_1'),
    );
    // The bubble stays on the page until the run applies it, but the daemon
    // holds the steer now, so nothing is saved for a reload to restore.
    expect(sessionStorage.getItem('animaos.pendingSends')).toBeNull();
  });

  it('restores a send left over from a previous load to the recovery panel, its key reused (S3b-A)', () => {
    sessionStorage.setItem(
      'animaos.pendingSends',
      JSON.stringify([
        {
          key: 'r',
          text: 'still retrying',
          conversation: 'agent-main\u0000session:chat:1',
          createdAtMs: 1000,
        },
      ]),
    );
    const onRestore = vi.fn();
    renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn(), onRestore }),
    );

    expect(onRestore).toHaveBeenCalledTimes(1);
    expect(onRestore).toHaveBeenCalledWith({
      key: 'r',
      text: 'still retrying',
      conversation: 'agent-main\u0000session:chat:1',
      createdAtMs: 1000,
    });
    // Handed off once: a second mount finds nothing left to restore.
    expect(sessionStorage.getItem('animaos.pendingSends')).toBeNull();
  });

  it('never throws when session storage is blocked (S3b-A)', async () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('full', 'QuotaExceededError');
    });
    vi.spyOn(daemon, 'startRun').mockResolvedValue({ run: runFixture('r') });
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn() }),
    );

    await act(async () => {
      expect(() => result.current.send(message('a'))).not.toThrow();
    });
  });

  it('cancels a retrying send: its retry never fires and the next one goes (S3b-I)', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const startRun = vi
      .spyOn(daemon, 'startRun')
      .mockRejectedValueOnce(new DaemonConnectionError('', new Error('down')))
      .mockResolvedValue({ run: runFixture('run_2') });
    const onAccepted = vi.fn();
    const onFailed = vi.fn();
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted, onFailed }),
    );

    act(() => {
      result.current.send(message('a'));
      result.current.send(message('b'));
    });
    await waitFor(() => expect(result.current.sends[0]?.failures).toBe(1));
    let cancelled: ReturnType<typeof result.current.cancel> = null;
    act(() => {
      cancelled = result.current.cancel('a');
    });
    expect(cancelled).toMatchObject({ key: 'a', text: 'text a' });
    await waitFor(() =>
      expect(onAccepted.mock.calls.map(([send]) => send.key)).toEqual(['b']),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(startRun.mock.calls.map(([, , , key]) => key)).toEqual(['a', 'b']);
    expect(onFailed).not.toHaveBeenCalled();
    expect(result.current.sends).toEqual([]);
    expect(sessionStorage.getItem('animaos.pendingSends')).toBeNull();
  });

  it('cancels a send waiting its turn without sending it, but not a steer (S3b-I)', async () => {
    const first = deferred<Awaited<ReturnType<typeof daemon.startRun>>>();
    const startRun = vi
      .spyOn(daemon, 'startRun')
      .mockReturnValueOnce(first.promise);
    const { result } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn() }),
    );

    act(() => {
      result.current.send(message('a'));
      result.current.send(message('b'));
    });
    act(() => {
      expect(result.current.cancel('b')).toMatchObject({ key: 'b' });
    });
    expect(result.current.sends.map((item) => item.key)).toEqual(['a']);
    await act(async () =>
      first.resolve({
        run: runFixture('run_1'),
        steer: { status: 'pending' },
      }),
    );
    expect(startRun).toHaveBeenCalledTimes(1);
    // A steer the daemon took is no longer the page's to cancel.
    act(() => {
      expect(result.current.cancel('a')).toBeNull();
    });
    expect(result.current.sends.map((item) => item.key)).toEqual(['a']);
    expect(result.current.cancel('missing')).toBeNull();
  });

  it('stops retrying once the page closes', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const startRun = vi
      .spyOn(daemon, 'startRun')
      .mockRejectedValue(new DaemonHttpError(504, null));
    const { result, unmount } = renderHook(() =>
      useSessionSends({ onAccepted: vi.fn(), onFailed: vi.fn() }),
    );

    act(() => result.current.send(message('k')));
    await waitFor(() => expect(startRun).toHaveBeenCalledTimes(1));
    unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(startRun).toHaveBeenCalledTimes(1);
  });
});
