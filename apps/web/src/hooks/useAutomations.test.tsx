import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { automationFixture } from '../test/automations';
import { useAutomations } from './useAutomations';

beforeEach(() => {
  vi.spyOn(daemon, 'listAutomations').mockResolvedValue([
    automationFixture('schedule-1'),
  ]);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useAutomations', () => {
  it('reads the list, and again when an automation event arrives', async () => {
    const { result, rerender } = renderHook(
      (props: { version: number }) =>
        useAutomations({
          agentId: 'agent-main',
          version: props.version,
          epoch: 0,
          enabled: true,
        }),
      { initialProps: { version: 0 } },
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.automations.map((item) => item.id)).toEqual([
      'schedule-1',
    ]);
    expect(daemon.listAutomations).toHaveBeenCalledWith('agent-main', {
      signal: expect.any(AbortSignal),
    });

    rerender({ version: 1 });

    await waitFor(() =>
      expect(daemon.listAutomations).toHaveBeenCalledTimes(2),
    );
  });

  it('reads nothing while disabled or without a companion', async () => {
    const { rerender } = renderHook(
      (props: { enabled: boolean; agentId: string | null }) =>
        useAutomations({
          agentId: props.agentId,
          version: 0,
          epoch: 0,
          enabled: props.enabled,
        }),
      {
        initialProps: {
          enabled: false,
          agentId: 'agent-main' as string | null,
        },
      },
    );
    rerender({ enabled: true, agentId: null });
    expect(daemon.listAutomations).not.toHaveBeenCalled();
  });

  it('acts through the daemon, reads again, and reports refusals', async () => {
    const run = vi
      .spyOn(daemon, 'runAutomationNow')
      .mockResolvedValue(automationFixture('schedule-1', { running: true }));
    vi.spyOn(daemon, 'deleteAutomation').mockRejectedValue(
      new DaemonHttpError(409, { error: 'This automation is already running' }),
    );
    const update = vi
      .spyOn(daemon, 'updateAutomation')
      .mockResolvedValue(automationFixture('schedule-1', { enabled: false }));
    const { result } = renderHook(() =>
      useAutomations({
        agentId: 'agent-main',
        version: 0,
        epoch: 0,
        enabled: true,
      }),
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    const first = result.current.runNow;

    let done = false;
    await act(async () => {
      done = await result.current.runNow(result.current.automations[0]);
    });
    expect(done).toBe(true);
    expect(run).toHaveBeenCalledWith('agent-main', 'schedule-1');
    expect(result.current.runNow).toBe(first);

    await act(async () => {
      done = await result.current.setEnabled(
        result.current.automations[0],
        false,
      );
    });
    expect(update).toHaveBeenCalledWith('agent-main', 'schedule-1', {
      enabled: false,
    });

    await act(async () => {
      done = await result.current.remove(result.current.automations[0]);
    });
    expect(done).toBe(false);
    expect(result.current.error).toBe('This automation is already running');
    expect(result.current.errorStatus).toBe(409);
    // Each action, and the 409, read the list again.
    expect(daemon.listAutomations).toHaveBeenCalledTimes(4);
  });
});
