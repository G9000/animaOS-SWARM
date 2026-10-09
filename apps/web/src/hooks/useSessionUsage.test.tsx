import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../lib/daemon-api';
import { sessionFixture } from '../test/sessions';
import { totalsFixture } from '../test/usage';
import { useSessionUsage } from './useSessionUsage';

const target = { agentId: 'agent-main', sessionId: 'chat:1' };

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSessionUsage', () => {
  it('reads the session’s totals', async () => {
    vi.spyOn(daemon, 'getSession').mockResolvedValue(
      sessionFixture('chat:1', { usage: totalsFixture({ calls: 2 }) }),
    );
    const { result } = renderHook(() => useSessionUsage(target, 0));
    await waitFor(() => expect(result.current?.calls).toBe(2));
    expect(daemon.getSession).toHaveBeenCalledWith('agent-main', 'chat:1');
  });

  it('reads again when the key changes and keeps the last value meanwhile', async () => {
    let release: (value: ReturnType<typeof sessionFixture>) => void = () =>
      undefined;
    vi.spyOn(daemon, 'getSession')
      .mockResolvedValueOnce(
        sessionFixture('chat:1', { usage: totalsFixture({ calls: 2 }) }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            release = resolve;
          }),
      );
    const { result, rerender } = renderHook(
      ({ key }) => useSessionUsage(target, key),
      { initialProps: { key: 0 } },
    );
    await waitFor(() => expect(result.current?.calls).toBe(2));

    rerender({ key: 1 });
    expect(result.current?.calls).toBe(2);
    await act(async () => {
      release(sessionFixture('chat:1', { usage: totalsFixture({ calls: 3 }) }));
    });
    expect(result.current?.calls).toBe(3);
  });

  it('nothing is read without a target', async () => {
    vi.spyOn(daemon, 'getSession').mockResolvedValue(sessionFixture('chat:1'));
    const { result } = renderHook(() => useSessionUsage(null, 0));
    await act(async () => undefined);
    expect(daemon.getSession).not.toHaveBeenCalled();
    expect(result.current).toBeNull();
  });

  it('a failed read keeps the last value', async () => {
    vi.spyOn(daemon, 'getSession')
      .mockResolvedValueOnce(
        sessionFixture('chat:1', { usage: totalsFixture({ calls: 2 }) }),
      )
      .mockRejectedValueOnce(new TypeError('offline'));
    const { result, rerender } = renderHook(
      ({ key }) => useSessionUsage(target, key),
      { initialProps: { key: 0 } },
    );
    await waitFor(() => expect(result.current?.calls).toBe(2));
    rerender({ key: 1 });
    await waitFor(() =>
      expect(vi.mocked(daemon.getSession)).toHaveBeenCalledTimes(2),
    );
    await act(async () => undefined);
    expect(result.current?.calls).toBe(2);
  });
});
