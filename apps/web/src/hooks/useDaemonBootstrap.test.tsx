import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
} from '../lib/daemon-api';
import { useDaemonBootstrap } from './useDaemonBootstrap';

vi.mock('../lib/daemon-api', async () => {
  const actual =
    await vi.importActual<typeof import('../lib/daemon-api')>(
      '../lib/daemon-api',
    );

  return {
    ...actual,
    daemon: {
      ...actual.daemon,
      health: vi.fn(),
      listAgents: vi.fn(),
      listProviders: vi.fn(),
    },
  };
});

const provider: DaemonProvider = {
  id: 'deterministic',
  label: 'Deterministic',
  requiresKey: false,
  configured: true,
  apiKeyEnvs: [],
};

function snapshot(id: string, createdAtMs: number, name = id): DaemonSnapshot {
  return {
    state: {
      id,
      name,
      status: 'idle',
      config: {
        name,
        model: 'deterministic',
        provider: 'deterministic',
        system: null,
        tools: [
          {
            name: 'read_file',
            description: 'Read a workspace file',
            parameters: {
              type: 'object',
              properties: { file_path: { type: 'string' } },
              required: ['file_path'],
            },
            examples: null,
          },
        ],
      },
      createdAtMs,
      tokenUsage: {
        promptTokens: 0,
        completionTokens: 0,
        totalTokens: 0,
      },
    },
    messageCount: 0,
    messages: [],
    eventCount: 0,
  };
}

const healthMock = vi.mocked(daemon.health);
const listAgentsMock = vi.mocked(daemon.listAgents);
const listProvidersMock = vi.mocked(daemon.listProviders);

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });

  return { promise, resolve, reject };
}

async function flushBootstrap() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function resolveBootstrap(agents: DaemonSnapshot[] = []) {
  healthMock.mockResolvedValue({ status: 'ok' });
  listAgentsMock.mockResolvedValue({ agents });
  listProvidersMock.mockResolvedValue({ providers: [provider] });
}

beforeEach(() => {
  vi.useRealTimers();
  healthMock.mockReset();
  listAgentsMock.mockReset();
  listProvidersMock.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useDaemonBootstrap', () => {
  it('starts synchronously in an unknown, unloaded state', () => {
    healthMock.mockReturnValue(
      new Promise<{ status: string }>(() => undefined),
    );
    listAgentsMock.mockReturnValue(
      new Promise<{ agents: DaemonSnapshot[] }>(() => undefined),
    );
    listProvidersMock.mockReturnValue(
      new Promise<{ providers: DaemonProvider[] }>(() => undefined),
    );

    const { result, unmount } = renderHook(() => useDaemonBootstrap());

    expect(result.current.connection).toBe('unknown');
    expect(result.current.loaded).toBe(false);
    expect(result.current.connection).not.toBe('online');
    expect(result.current.agents).toEqual([]);
    expect(result.current.providers).toBeNull();

    unmount();
  });

  it('returns the entire sorted snapshot collection and provider catalog', async () => {
    const later = snapshot('later', 20);
    const firstTie = snapshot('a-first', 10);
    const secondTie = snapshot('b-second', 10);
    resolveBootstrap([later, secondTie, firstTie]);

    const { result } = renderHook(() => useDaemonBootstrap());

    await waitFor(() => {
      expect(result.current.loaded).toBe(true);
      expect(result.current.connection).toBe('online');
    });

    expect(result.current.agents).toEqual([firstTie, secondTie, later]);
    expect(result.current.providers).toEqual([provider]);
    expect(result.current.providersError).toBeNull();
    expect(result.current).not.toHaveProperty('mainAgent');
  });

  it.each(['health', 'agents'] as const)(
    'reports a failed %s request as offline and refreshAgents can retry',
    async (failedRequest) => {
      const available = snapshot('available', 10);
      if (failedRequest === 'health') {
        healthMock.mockRejectedValueOnce(new Error('daemon unavailable'));
        listAgentsMock.mockResolvedValueOnce({ agents: [available] });
      } else {
        healthMock.mockResolvedValueOnce({ status: 'ok' });
        listAgentsMock.mockRejectedValueOnce(new Error('agents unavailable'));
      }
      listProvidersMock.mockResolvedValue({ providers: [provider] });

      const { result } = renderHook(() => useDaemonBootstrap());

      await waitFor(() => {
        expect(result.current.loaded).toBe(true);
        expect(result.current.connection).toBe('offline');
      });
      expect(result.current.agents).toEqual(
        failedRequest === 'health' ? [available] : [],
      );

      listAgentsMock.mockResolvedValueOnce({ agents: [available] });
      await act(async () => {
        await result.current.refreshAgents();
      });

      expect(result.current.connection).toBe('online');
      expect(result.current.agents).toEqual([available]);
    },
  );

  it('keeps last-known agents when a later poll fails', async () => {
    vi.useFakeTimers();
    const known = snapshot('known', 10);
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock
      .mockResolvedValueOnce({ agents: [known] })
      .mockRejectedValueOnce(new Error('poll failed'));
    listProvidersMock.mockResolvedValue({ providers: [provider] });

    const { result } = renderHook(() => useDaemonBootstrap());

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(result.current.connection).toBe('online');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });

    expect(result.current.connection).toBe('offline');
    expect(result.current.agents).toEqual([known]);
  });

  it('does not start another poll until the in-flight poll settles', async () => {
    vi.useFakeTimers();
    const initial = snapshot('initial', 10);
    const firstPoll = deferred<{ agents: DaemonSnapshot[] }>();
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock
      .mockResolvedValueOnce({ agents: [initial] })
      .mockReturnValueOnce(firstPoll.promise)
      .mockResolvedValue({ agents: [initial] });
    listProvidersMock.mockResolvedValue({ providers: [provider] });

    renderHook(() => useDaemonBootstrap());
    await flushBootstrap();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(listAgentsMock).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(20_000);
    });
    expect(listAgentsMock).toHaveBeenCalledTimes(2);

    await act(async () => {
      firstPoll.resolve({ agents: [initial] });
      await firstPoll.promise;
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(4_999);
    });
    expect(listAgentsMock).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(listAgentsMock).toHaveBeenCalledTimes(3);
  });

  it('ignores an older failed refresh after a newer refresh recovers', async () => {
    const initial = snapshot('initial', 10);
    const recovered = snapshot('recovered', 20);
    const staleRefresh = deferred<{ agents: DaemonSnapshot[] }>();
    const currentRefresh = deferred<{ agents: DaemonSnapshot[] }>();
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock
      .mockResolvedValueOnce({ agents: [initial] })
      .mockReturnValueOnce(staleRefresh.promise)
      .mockReturnValueOnce(currentRefresh.promise);
    listProvidersMock.mockResolvedValue({ providers: [provider] });

    const { result } = renderHook(() => useDaemonBootstrap());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    const staleRequest = result.current.refreshAgents();
    const currentRequest = result.current.refreshAgents();

    await act(async () => {
      currentRefresh.resolve({ agents: [recovered] });
      await currentRequest;
    });
    expect(result.current.connection).toBe('online');
    expect(result.current.agents).toEqual([recovered]);

    await act(async () => {
      staleRefresh.reject(new Error('stale failure'));
      await staleRequest;
    });

    expect(result.current.connection).toBe('online');
    expect(result.current.agents).toEqual([recovered]);
  });

  it('ignores an older successful refresh after a newer snapshot is published', async () => {
    const initial = snapshot('initial', 10);
    const stale = snapshot('stale', 20);
    const current = snapshot('current', 30);
    const staleRefresh = deferred<{ agents: DaemonSnapshot[] }>();
    const currentRefresh = deferred<{ agents: DaemonSnapshot[] }>();
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock
      .mockResolvedValueOnce({ agents: [initial] })
      .mockReturnValueOnce(staleRefresh.promise)
      .mockReturnValueOnce(currentRefresh.promise);
    listProvidersMock.mockResolvedValue({ providers: [provider] });

    const { result } = renderHook(() => useDaemonBootstrap());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    const staleRequest = result.current.refreshAgents();
    const currentRequest = result.current.refreshAgents();

    await act(async () => {
      currentRefresh.resolve({ agents: [current] });
      await currentRequest;
    });
    await act(async () => {
      staleRefresh.resolve({ agents: [stale] });
      await staleRequest;
    });

    expect(result.current.agents).toEqual([current]);
  });

  it.each(['accept', 'remove'] as const)(
    'does not let a poll started before %s overwrite the local collection mutation',
    async (mutation) => {
      vi.useFakeTimers();
      const keep = snapshot('keep', 20);
      const remove = snapshot('remove', 30);
      const created = snapshot('created', 10);
      const poll = deferred<{ agents: DaemonSnapshot[] }>();
      healthMock.mockResolvedValue({ status: 'ok' });
      listAgentsMock
        .mockResolvedValueOnce({ agents: [keep, remove] })
        .mockReturnValueOnce(poll.promise);
      listProvidersMock.mockResolvedValue({ providers: [provider] });

      const { result } = renderHook(() => useDaemonBootstrap());
      await flushBootstrap();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });

      act(() => {
        if (mutation === 'accept') {
          result.current.acceptAgentSnapshot(created);
        } else {
          result.current.removeAgentSnapshot(remove.state.id);
        }
      });

      await act(async () => {
        poll.resolve({ agents: [keep, remove] });
        await poll.promise;
        await Promise.resolve();
      });

      expect(result.current.agents).toEqual(
        mutation === 'accept' ? [created, keep, remove] : [keep],
      );
    },
  );

  it('keeps provider failure distinct and retries providers without resetting agents', async () => {
    const known = snapshot('known', 10);
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock.mockResolvedValue({ agents: [known] });
    listProvidersMock.mockRejectedValueOnce(
      new Error('provider catalog failed'),
    );

    const { result } = renderHook(() => useDaemonBootstrap());

    await waitFor(() => {
      expect(result.current.loaded).toBe(true);
      expect(result.current.providersError).toBe('provider catalog failed');
    });
    expect(result.current.connection).toBe('online');
    expect(result.current.providers).toBeNull();

    listProvidersMock.mockResolvedValueOnce({ providers: [provider] });
    await act(async () => {
      await result.current.retryProviders();
    });

    expect(result.current.providers).toEqual([provider]);
    expect(result.current.providersError).toBeNull();
    expect(result.current.agents).toEqual([known]);
  });

  it('publishes daemon availability without waiting for providers', async () => {
    const known = snapshot('known', 10);
    const providers = deferred<{ providers: DaemonProvider[] }>();
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock.mockResolvedValue({ agents: [known] });
    listProvidersMock.mockReturnValue(providers.promise);

    const { result } = renderHook(() => useDaemonBootstrap());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    expect(result.current.connection).toBe('online');
    expect(result.current.agents).toEqual([known]);
    expect(result.current.providers).toBeNull();
    expect(result.current.providersError).toBeNull();
  });

  it('ignores an older provider retry after a newer catalog is published', async () => {
    const oldProvider = { ...provider, id: 'old', label: 'Old' };
    const newProvider = { ...provider, id: 'new', label: 'New' };
    const staleRetry = deferred<{ providers: DaemonProvider[] }>();
    const currentRetry = deferred<{ providers: DaemonProvider[] }>();
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock.mockResolvedValue({ agents: [] });
    listProvidersMock
      .mockResolvedValueOnce({ providers: [provider] })
      .mockReturnValueOnce(staleRetry.promise)
      .mockReturnValueOnce(currentRetry.promise);

    const { result } = renderHook(() => useDaemonBootstrap());
    await waitFor(() => expect(result.current.providers).toEqual([provider]));

    const staleRequest = result.current.retryProviders();
    const currentRequest = result.current.retryProviders();

    await act(async () => {
      currentRetry.resolve({ providers: [newProvider] });
      await currentRequest;
    });
    await act(async () => {
      staleRetry.resolve({ providers: [oldProvider] });
      await staleRequest;
    });

    expect(result.current.providers).toEqual([newProvider]);
  });

  it('does not publish or reschedule in-flight poll and provider retry work after unmount', async () => {
    vi.useFakeTimers();
    const poll = deferred<{ agents: DaemonSnapshot[] }>();
    const retry = deferred<{ providers: DaemonProvider[] }>();
    const changedProvider = { ...provider, id: 'changed', label: 'Changed' };
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock
      .mockResolvedValueOnce({ agents: [] })
      .mockReturnValueOnce(poll.promise);
    listProvidersMock
      .mockResolvedValueOnce({ providers: [provider] })
      .mockReturnValueOnce(retry.promise);

    const { result, unmount } = renderHook(() => useDaemonBootstrap());
    await flushBootstrap();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    const retryRequest = result.current.retryProviders();
    const providersBeforeUnmount = result.current.providers;

    unmount();
    await act(async () => {
      poll.resolve({ agents: [snapshot('late', 10)] });
      retry.resolve({ providers: [changedProvider] });
      await retryRequest;
      await Promise.resolve();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });

    expect(listAgentsMock).toHaveBeenCalledTimes(2);
    expect(result.current.providers).toBe(providersBeforeUnmount);
  });

  it('does not publish or schedule polling from a superseded effect lifetime', async () => {
    vi.useFakeTimers();
    const firstHealth = deferred<{ status: string }>();
    const secondHealth = deferred<{ status: string }>();
    const firstAgents = deferred<{ agents: DaemonSnapshot[] }>();
    const secondAgents = deferred<{ agents: DaemonSnapshot[] }>();
    const firstProviders = deferred<{ providers: DaemonProvider[] }>();
    const secondProviders = deferred<{ providers: DaemonProvider[] }>();
    healthMock
      .mockReturnValueOnce(firstHealth.promise)
      .mockReturnValueOnce(secondHealth.promise);
    listAgentsMock
      .mockReturnValueOnce(firstAgents.promise)
      .mockReturnValueOnce(secondAgents.promise);
    listProvidersMock
      .mockReturnValueOnce(firstProviders.promise)
      .mockReturnValueOnce(secondProviders.promise);

    const { result, unmount } = renderHook(() => useDaemonBootstrap(), {
      reactStrictMode: true,
    });
    expect(listAgentsMock).toHaveBeenCalledTimes(2);

    await act(async () => {
      firstHealth.resolve({ status: 'ok' });
      firstAgents.resolve({ agents: [snapshot('superseded', 10)] });
      await firstHealth.promise;
      await firstAgents.promise;
      await Promise.resolve();
    });

    expect(result.current.loaded).toBe(false);
    expect(result.current.agents).toEqual([]);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(listAgentsMock).toHaveBeenCalledTimes(2);

    unmount();
  });

  it('adopts a created snapshot immediately and retains it after polling failure', async () => {
    vi.useFakeTimers();
    const existing = snapshot('existing', 20);
    const created = snapshot('created', 10);
    healthMock.mockResolvedValue({ status: 'ok' });
    listAgentsMock
      .mockResolvedValueOnce({ agents: [existing] })
      .mockRejectedValueOnce(new Error('poll failed'));
    listProvidersMock.mockResolvedValue({ providers: [provider] });

    const { result } = renderHook(() => useDaemonBootstrap());
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    act(() => {
      result.current.acceptAgentSnapshot(created);
    });
    expect(result.current.agents).toEqual([created, existing]);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });

    expect(result.current.connection).toBe('offline');
    expect(result.current.agents).toEqual([created, existing]);
  });

  it('replaces a matching snapshot by state id, retains others, and sorts a copy', async () => {
    const first = snapshot('first', 10);
    const oldMatching = snapshot('matching', 30, 'Old');
    const last = snapshot('last', 40);
    const updatedMatching = snapshot('matching', 20, 'Updated');
    resolveBootstrap([last, oldMatching, first]);

    const { result } = renderHook(() => useDaemonBootstrap());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    const before = result.current.agents;
    act(() => {
      result.current.acceptAgentSnapshot(updatedMatching);
    });

    expect(result.current.agents).toEqual([first, updatedMatching, last]);
    expect(result.current.agents).not.toBe(before);
    expect(
      result.current.agents.filter(({ state }) => state.id === 'matching'),
    ).toEqual([updatedMatching]);
  });

  it('removes only the snapshot with the matching state id', async () => {
    const first = snapshot('first', 10);
    const remove = snapshot('remove', 20);
    const last = snapshot('last', 30);
    resolveBootstrap([first, remove, last]);

    const { result } = renderHook(() => useDaemonBootstrap());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    act(() => {
      result.current.removeAgentSnapshot('remove');
    });

    expect(result.current.agents).toEqual([first, last]);
  });
});
