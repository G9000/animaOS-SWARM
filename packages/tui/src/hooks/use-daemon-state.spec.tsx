import React, { useEffect } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Text } from 'ink';
import {
  DAEMON_CONNECTION_LOST_MESSAGE,
  DAEMON_HEALTH_UNAVAILABLE_MESSAGE,
  DAEMON_RECOVERED_MESSAGE,
} from '@animaOS-SWARM/core';
import { cleanupInk, flushInk, renderInk } from '../test-harness.js';
import { useDaemonState } from './use-daemon-state.js';

afterEach(() => {
  cleanupInk();
  vi.clearAllMocks();
  vi.useRealTimers();
});

interface DaemonStateProbeProps {
  interactive?: boolean;
  phase?: 'waiting' | 'running';
  preflightWarning?: string;
  pollDaemonWarning?: () => Promise<string | undefined>;
  showMessage: (message: string) => void;
  manualCheckToken?: number;
  pollCheckToken?: number;
}

function DaemonStateProbe({
  interactive = true,
  phase = 'waiting',
  preflightWarning,
  pollDaemonWarning,
  showMessage,
  manualCheckToken = 0,
  pollCheckToken = 0,
}: DaemonStateProbeProps): React.ReactElement {
  const state = useDaemonState({
    interactive,
    phase,
    preflightWarning,
    pollDaemonWarning,
    showMessage,
  });

  useEffect(() => {
    if (manualCheckToken > 0) {
      void state.syncDaemonWarning('manual');
    }
  }, [manualCheckToken, state.syncDaemonWarning]);

  useEffect(() => {
    if (pollCheckToken > 0) {
      void state.syncDaemonWarning('poll');
    }
  }, [pollCheckToken, state.syncDaemonWarning]);

  return (
    <Text>
      {[
        `status=${state.daemonStatus ?? 'none'}`,
        `blocked=${String(state.taskEntryBlockedByDaemon)}`,
        `warning=${state.daemonWarning ?? 'none'}`,
        `recovery=${state.daemonRecoveryNotice ?? 'none'}`,
        `checked=${state.lastDaemonCheckAt ?? 'none'}`,
      ].join('|')}
    </Text>
  );
}

async function settleDaemonState() {
  await flushInk();
  await flushInk();
}

describe('useDaemonState', () => {
  it('reports unavailable manual health checks when no poller is wired', async () => {
    const showMessage = vi.fn();
    const rendered = renderInk(<DaemonStateProbe showMessage={showMessage} />);

    expect(rendered.lastFrame()).toContain('status=none');
    expect(rendered.lastFrame()).toContain('blocked=false');
    expect(rendered.lastFrame()).toContain('checked=none');

    rendered.rerender(
      <DaemonStateProbe showMessage={showMessage} manualCheckToken={1} />
    );
    await settleDaemonState();

    expect(showMessage).toHaveBeenCalledWith(DAEMON_HEALTH_UNAVAILABLE_MESSAGE);
    expect(rendered.lastFrame()).toContain('status=none');
    expect(rendered.lastFrame()).toContain('warning=none');
    expect(rendered.lastFrame()).toContain('blocked=false');
  });

  it('clears a preflight warning and restores task entry after a successful manual recheck', async () => {
    const showMessage = vi.fn();
    const pollDaemonWarning = vi.fn().mockResolvedValue(undefined);
    const rendered = renderInk(
      <DaemonStateProbe
        showMessage={showMessage}
        preflightWarning="daemon unavailable"
        pollDaemonWarning={pollDaemonWarning}
      />
    );

    expect(rendered.lastFrame()).toContain('status=down');
    expect(rendered.lastFrame()).toContain('warning=daemon unavailable');
    expect(rendered.lastFrame()).toContain('blocked=true');

    rendered.rerender(
      <DaemonStateProbe
        showMessage={showMessage}
        preflightWarning="daemon unavailable"
        pollDaemonWarning={pollDaemonWarning}
        manualCheckToken={1}
      />
    );
    await settleDaemonState();

    expect(showMessage).toHaveBeenCalledWith(DAEMON_RECOVERED_MESSAGE);
    expect(rendered.lastFrame()).toContain('status=up');
    expect(rendered.lastFrame()).toContain('warning=none');
    expect(rendered.lastFrame()).toContain('blocked=false');
    expect(rendered.lastFrame()).toContain(
      'recovery=Task entry restored. Freeform tasks are available'
    );
    expect(rendered.lastFrame()).toContain('again.');
  });

  it('applies polling transition semantics when a warning appears and clears', async () => {
    const showMessage = vi.fn();
    const pollDaemonWarning = vi
      .fn<() => Promise<string | undefined>>()
      .mockResolvedValueOnce('daemon unavailable')
      .mockResolvedValueOnce(undefined);
    const rendered = renderInk(
      <DaemonStateProbe
        showMessage={showMessage}
        pollDaemonWarning={pollDaemonWarning}
      />
    );

    expect(rendered.lastFrame()).toContain('status=up');
    expect(rendered.lastFrame()).toContain('blocked=false');

    rendered.rerender(
      <DaemonStateProbe
        showMessage={showMessage}
        pollDaemonWarning={pollDaemonWarning}
        pollCheckToken={1}
      />
    );
    await settleDaemonState();

    expect(showMessage).toHaveBeenCalledWith(DAEMON_CONNECTION_LOST_MESSAGE);
    expect(rendered.lastFrame()).toContain('status=down');
    expect(rendered.lastFrame()).toContain('warning=daemon unavailable');
    expect(rendered.lastFrame()).toContain('blocked=true');

    rendered.rerender(
      <DaemonStateProbe
        showMessage={showMessage}
        pollDaemonWarning={pollDaemonWarning}
        pollCheckToken={2}
      />
    );
    await settleDaemonState();

    expect(showMessage).toHaveBeenCalledWith(DAEMON_RECOVERED_MESSAGE);
    expect(rendered.lastFrame()).toContain('status=up');
    expect(rendered.lastFrame()).toContain('warning=none');
    expect(rendered.lastFrame()).toContain('blocked=false');
  });
});
