import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { runFixture } from '../test/live';
import { SESSION_RUNS_LIMIT, useSessionRuns } from './useSessionRuns';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSessionRuns', () => {
  it('reads the session’s runs and reads them again on a refresh', async () => {
    const list = vi
      .spyOn(daemon, 'sessionRuns')
      .mockResolvedValueOnce([runFixture('run_1')])
      .mockResolvedValueOnce([runFixture('run_2'), runFixture('run_1')]);
    const { result, rerender } = renderHook(
      ({ refresh }) => useSessionRuns('agent-main', 'chat:1', refresh),
      { initialProps: { refresh: 0 } },
    );

    await waitFor(() =>
      expect(result.current.map((run) => run.id)).toEqual(['run_1']),
    );
    expect(list).toHaveBeenCalledWith('agent-main', 'chat:1', {
      limit: SESSION_RUNS_LIMIT,
    });
    rerender({ refresh: 1 });
    await waitFor(() =>
      expect(result.current.map((run) => run.id)).toEqual(['run_2', 'run_1']),
    );
  });

  it('shows no runs of another session, nor any when the route is missing', async () => {
    vi.spyOn(daemon, 'sessionRuns')
      .mockResolvedValueOnce([runFixture('run_1')])
      .mockRejectedValueOnce(new DaemonHttpError(404, { error: 'not found' }));
    const { result, rerender } = renderHook(
      ({ sessionId }) => useSessionRuns('agent-main', sessionId),
      { initialProps: { sessionId: 'chat:1' } },
    );

    await waitFor(() => expect(result.current).toHaveLength(1));
    rerender({ sessionId: 'chat:2' });
    expect(result.current).toEqual([]);
    await waitFor(() => expect(daemon.sessionRuns).toHaveBeenCalledTimes(2));
    expect(result.current).toEqual([]);
  });
});
