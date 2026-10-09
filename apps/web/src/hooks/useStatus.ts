import { useCallback, useEffect, useRef, useState } from 'react';
import type { DaemonStatus } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { STATUS_POLL_MS, statusErrorMessage } from '../lib/status';

export interface StatusOptions {
  /** False while the daemon is offline: nothing is read. */
  enabled: boolean;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
}

export interface StatusView {
  status: DaemonStatus | null;
  loaded: boolean;
  error: string | null;
  /** The HTTP status of the failed read, when the daemon refused it. */
  errorStatus: number | null;
  refresh: () => void;
}

/** The Health page's data (spec §15.4): the status aggregate, read on mount,
 *  on an epoch change, and every `STATUS_POLL_MS`. A failed read keeps the
 *  last status; an unchanged answer keeps its object. */
export function useStatus({ enabled, epoch }: StatusOptions): StatusView {
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [failure, setFailure] = useState<{
    message: string;
    status: number | null;
  } | null>(null);
  // The newest read wins; a late answer to an older one is dropped.
  const sequence = useRef(0);
  const unmounted = useRef(false);
  const stopped = useRef(false);

  const load = useCallback(async () => {
    const mine = ++sequence.current;
    try {
      const next = await daemon.status();
      if (unmounted.current || mine !== sequence.current) return;
      stopped.current = false;
      setStatus((previous) =>
        previous && JSON.stringify(previous) === JSON.stringify(next)
          ? previous
          : next,
      );
      setFailure(null);
    } catch (error) {
      if (unmounted.current || mine !== sequence.current) return;
      const failed = statusErrorMessage(error);
      // An older daemon will not grow the route while the page is open.
      stopped.current = failed.status === 404;
      setFailure(failed);
    }
    setLoaded(true);
  }, []);

  useEffect(() => {
    unmounted.current = false;
    return () => {
      unmounted.current = true;
    };
  }, []);

  useEffect(() => {
    if (!enabled) return;
    stopped.current = false;
    void load();
    const timer = setInterval(() => {
      if (!stopped.current) void load();
    }, STATUS_POLL_MS);
    return () => clearInterval(timer);
  }, [enabled, epoch, load]);

  const refresh = useCallback(() => {
    void load();
  }, [load]);

  return {
    status,
    loaded,
    error: failure?.message ?? null,
    errorStatus: failure?.status ?? null,
    refresh,
  };
}
