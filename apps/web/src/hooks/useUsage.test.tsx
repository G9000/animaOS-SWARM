import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError, type UsageQuery } from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';
import * as download from '../lib/download';
import { USAGE_EXPORT_FAILED, USAGE_TOO_OLD } from '../lib/usage';
import { groupFixture, summaryFixture, totalsFixture } from '../test/usage';
import { useUsage, type UsageOptions } from './useUsage';

const NOW = new Date(2026, 8, 23, 12);
const online: UsageOptions = {
  enabled: true,
  days: 7,
  agentId: 'agent-main',
  epoch: 1,
  now: () => NOW,
};

function dayKey(offset: number): string {
  const date = new Date(
    NOW.getFullYear(),
    NOW.getMonth(),
    NOW.getDate() - offset,
  );
  const utc = new Date(
    Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()),
  );
  return utc.toISOString().slice(0, 10);
}

function answer(query: UsageQuery) {
  switch (query.groupBy) {
    case 'day':
      return summaryFixture({
        groupBy: 'day',
        totals: totalsFixture({ calls: 3, totalTokens: 300 }),
        groups: [
          groupFixture(dayKey(0), { calls: 2, totalTokens: 200 }),
          groupFixture(dayKey(3), { calls: 1, totalTokens: 100 }),
        ],
      });
    case 'model':
      return summaryFixture({
        groupBy: 'model',
        groups: [groupFixture('openai/gpt-5.4', { calls: 3 })],
      });
    case 'source':
      return summaryFixture({
        groupBy: 'source',
        groups: [groupFixture('chat', { calls: 3 })],
      });
    default:
      return summaryFixture({
        groupBy: 'session',
        groups: [groupFixture('chat:1', { calls: 3 })],
      });
  }
}

beforeEach(() => {
  vi.spyOn(daemon, 'usageSummary').mockImplementation(async (query) =>
    answer(query),
  );
  vi.spyOn(daemon, 'exportUsageCsv').mockResolvedValue('id\n');
  vi.spyOn(download, 'downloadText').mockImplementation(() => undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useUsage', () => {
  it('reads the four summaries for the range', async () => {
    const { result } = renderHook(() => useUsage(online));
    await waitFor(() => expect(result.current.loaded).toBe(true));

    const calls = vi.mocked(daemon.usageSummary).mock.calls.map(([q]) => q);
    expect(calls.map((query) => query.groupBy)).toEqual([
      'day',
      'model',
      'source',
      'session',
    ]);
    const { from, to, tzOffsetMinutes } = result.current.range;
    for (const query of calls) {
      expect(query).toMatchObject({
        from,
        to,
        tzOffsetMinutes,
        agentId: 'agent-main',
      });
    }
    expect(to).toBe(new Date(2026, 8, 24).getTime());
    expect(result.current.totals?.totalTokens).toBe(300);
    expect(result.current.days).toHaveLength(7);
    expect(result.current.models.map((group) => group.key)).toEqual([
      'openai/gpt-5.4',
    ]);
    expect(result.current.sources.map((group) => group.key)).toEqual(['chat']);
    expect(result.current.sessions.map((group) => group.key)).toEqual([
      'chat:1',
    ]);
    expect(result.current.error).toBeNull();
  });

  it('today is the last day', async () => {
    const { result } = renderHook(() => useUsage(online));
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.days.at(-1)?.key).toBe(dayKey(0));
    expect(result.current.today?.totalTokens).toBe(200);
  });

  it('a range change reads again', async () => {
    const { result, rerender } = renderHook(
      (props: UsageOptions) => useUsage(props),
      { initialProps: online },
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    rerender({ ...online, days: 30 });
    await waitFor(() => expect(result.current.days).toHaveLength(30));
    expect(daemon.usageSummary).toHaveBeenCalledTimes(8);
  });

  it('a stale read is ignored', async () => {
    let releaseFirst: (summary: ReturnType<typeof answer>) => void = () =>
      undefined;
    vi.mocked(daemon.usageSummary).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          releaseFirst = resolve;
        }),
    );
    const { result } = renderHook(() => useUsage(online));
    await waitFor(() =>
      expect(vi.mocked(daemon.usageSummary).mock.calls.length).toBe(4),
    );
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    await act(async () => {
      releaseFirst(
        summaryFixture({
          groupBy: 'day',
          totals: totalsFixture({ totalTokens: 1 }),
        }),
      );
    });
    expect(result.current.totals?.totalTokens).toBe(300);
  });

  it('a failed list keeps the others and sets the error', async () => {
    const { result } = renderHook(() => useUsage(online));
    await waitFor(() => expect(result.current.loaded).toBe(true));
    const keptModels = result.current.models;
    vi.mocked(daemon.usageSummary).mockImplementation(async (query) => {
      if (query.groupBy === 'source') throw new TypeError('offline');
      return answer({ ...query, groupBy: query.groupBy });
    });
    act(() => result.current.refresh());
    await waitFor(() =>
      expect(result.current.error).toBe(COMPANION_UNREACHABLE),
    );
    expect(result.current.sources.map((group) => group.key)).toEqual(['chat']);
    expect(result.current.models).toBe(keptModels);
    expect(result.current.totals?.totalTokens).toBe(300);
  });

  it('an empty reload keeps the empty arrays', async () => {
    vi.mocked(daemon.usageSummary).mockImplementation(async (query) =>
      summaryFixture({ groupBy: query.groupBy ?? null }),
    );
    const { result } = renderHook(() => useUsage(online));
    await waitFor(() => expect(result.current.loaded).toBe(true));
    const before = result.current;
    act(() => result.current.refresh());
    await waitFor(() =>
      expect(vi.mocked(daemon.usageSummary).mock.calls.length).toBe(8),
    );
    await act(async () => undefined);
    expect(result.current.models).toBe(before.models);
    expect(result.current.sources).toBe(before.sources);
    expect(result.current.sessions).toBe(before.sessions);
    expect(result.current.days).toBe(before.days);
    expect(result.current.models).toHaveLength(0);
  });

  it('a 404 sets the update-the-daemon text and the status', async () => {
    vi.mocked(daemon.usageSummary).mockRejectedValue(
      new DaemonHttpError(404, { error: 'Not found' }),
    );
    const { result } = renderHook(() => useUsage(online));
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.error).toBe(USAGE_TOO_OLD);
    expect(result.current.errorStatus).toBe(404);
  });

  it('nothing is read while offline or without an agent', async () => {
    renderHook(() => useUsage({ ...online, enabled: false }));
    renderHook(() => useUsage({ ...online, agentId: null }));
    await act(async () => undefined);
    expect(daemon.usageSummary).not.toHaveBeenCalled();
  });

  it('exportCsv downloads and reports a failure', async () => {
    const { result } = renderHook(() => useUsage(online));
    await waitFor(() => expect(result.current.loaded).toBe(true));

    let saved = false;
    await act(async () => {
      saved = await result.current.exportCsv();
    });
    expect(saved).toBe(true);
    const { from, to } = result.current.range;
    expect(daemon.exportUsageCsv).toHaveBeenCalledWith({
      from,
      to,
      agentId: 'agent-main',
    });
    expect(download.downloadText).toHaveBeenCalledWith(
      'anima-usage-2026-09-17-to-2026-09-23.csv',
      'id\n',
      'text/csv',
    );

    vi.mocked(daemon.exportUsageCsv).mockRejectedValue(new Error('no'));
    await act(async () => {
      saved = await result.current.exportCsv();
    });
    expect(saved).toBe(false);
    expect(result.current.error).toBe(USAGE_EXPORT_FAILED);
  });
});
