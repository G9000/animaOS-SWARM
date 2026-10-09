import { act, renderHook } from '@testing-library/react';
import { StatusTooOldError } from '@animaOS-SWARM/sdk';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';
import { HEALTH_TOO_OLD, STATUS_POLL_MS } from '../lib/status';
import { statusFixture } from '../test/system';
import { useStatus, type StatusOptions } from './useStatus';

const online: StatusOptions = { enabled: true, epoch: 1 };

async function settle() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(daemon, 'status').mockResolvedValue(statusFixture());
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('useStatus', () => {
  it('reads on mount and on epoch change', async () => {
    const { result, rerender } = renderHook(
      (props: StatusOptions) => useStatus(props),
      { initialProps: online },
    );
    await settle();
    expect(daemon.status).toHaveBeenCalledTimes(1);
    expect(result.current.status?.version).toBe('0.9.1');
    expect(result.current.loaded).toBe(true);

    rerender({ ...online, epoch: 2 });
    await settle();
    expect(daemon.status).toHaveBeenCalledTimes(2);
  });

  it('polls every 15 seconds and stops on unmount', async () => {
    const { unmount } = renderHook(() => useStatus(online));
    await settle();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(STATUS_POLL_MS);
    });
    expect(daemon.status).toHaveBeenCalledTimes(2);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(STATUS_POLL_MS);
    });
    expect(daemon.status).toHaveBeenCalledTimes(3);

    unmount();
    await vi.advanceTimersByTimeAsync(STATUS_POLL_MS * 3);
    expect(daemon.status).toHaveBeenCalledTimes(3);
  });

  it('keeps the same status object when nothing changed', async () => {
    const { result } = renderHook(() => useStatus(online));
    await settle();
    const first = result.current.status;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(STATUS_POLL_MS);
    });
    expect(result.current.status).toBe(first);
  });

  it('a stale answer is ignored', async () => {
    let releaseFirst: (value: ReturnType<typeof statusFixture>) => void = () =>
      undefined;
    vi.mocked(daemon.status).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          releaseFirst = resolve;
        }),
    );
    const { result } = renderHook(() => useStatus(online));
    await settle();
    act(() => result.current.refresh());
    await settle();
    expect(result.current.status?.version).toBe('0.9.1');

    await act(async () => releaseFirst(statusFixture({ version: 'old' })));
    expect(result.current.status?.version).toBe('0.9.1');
  });

  it('a failed read keeps the last status and sets the error', async () => {
    const { result } = renderHook(() => useStatus(online));
    await settle();
    vi.mocked(daemon.status).mockRejectedValue(new TypeError('offline'));
    act(() => result.current.refresh());
    await settle();
    expect(result.current.error).toBe(COMPANION_UNREACHABLE);
    expect(result.current.status?.version).toBe('0.9.1');

    vi.mocked(daemon.status).mockResolvedValue(statusFixture());
    act(() => result.current.refresh());
    await settle();
    expect(result.current.error).toBeNull();
  });

  it('a 404 sets the update text and stops polling', async () => {
    vi.mocked(daemon.status).mockRejectedValue(new StatusTooOldError());
    const { result } = renderHook(() => useStatus(online));
    await settle();
    expect(result.current.error).toBe(HEALTH_TOO_OLD);
    expect(result.current.errorStatus).toBe(404);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(STATUS_POLL_MS * 3);
    });
    expect(daemon.status).toHaveBeenCalledTimes(1);
  });

  it('reads nothing while offline', async () => {
    renderHook(() => useStatus({ enabled: false, epoch: 1 }));
    await settle();
    expect(daemon.status).not.toHaveBeenCalled();
  });
});
