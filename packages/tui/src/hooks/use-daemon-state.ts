import { useState, useEffect, useRef, useCallback } from 'react';
import {
  DAEMON_HEALTH_UNAVAILABLE_MESSAGE,
  describeDaemonWarningTransition,
} from '@animaOS-SWARM/core';

const DAEMON_WARNING_POLL_MS = 5000;
const DAEMON_RECOVERY_NOTICE_MS = 4000;
/** Hard cap on how long a single `pollDaemonWarning` call may hang before we
 * abandon it and re-arm the next poll. Without this, a hung HTTP call would
 * permanently stall polling because the in-flight guard never clears. */
const DAEMON_POLL_TIMEOUT_MS = 8000;
const TASK_ENTRY_RESTORED_MESSAGE =
  'Task entry restored. Freeform tasks are available again.';

type DaemonSyncSource = 'poll' | 'manual';
type AppPhase = 'waiting' | 'running';

export interface UseDaemonStateOptions {
  interactive: boolean;
  phase: AppPhase;
  preflightWarning?: string;
  pollDaemonWarning?: () => Promise<string | undefined>;
  showMessage: (message: string) => void;
  /** Forwarded from `AppProps.onWarning`. Logged when a poll exceeds
   * `DAEMON_POLL_TIMEOUT_MS` or when the injected callback rejects. */
  onWarning?: (where: string, detail: unknown) => void;
}

export interface UseDaemonStateResult {
  daemonWarning: string | null;
  daemonRecoveryNotice: string | null;
  daemonStatus: 'up' | 'down' | undefined;
  lastDaemonCheckAt: number | null;
  taskEntryBlockedByDaemon: boolean;
  syncDaemonWarning: (source: DaemonSyncSource) => Promise<void>;
}

export function useDaemonState({
  interactive,
  phase,
  preflightWarning,
  pollDaemonWarning,
  showMessage,
  onWarning,
}: UseDaemonStateOptions): UseDaemonStateResult {
  const [daemonWarning, setDaemonWarning] = useState<string | null>(
    preflightWarning ?? null
  );
  const [daemonRecoveryNotice, setDaemonRecoveryNotice] = useState<
    string | null
  >(null);
  const [lastDaemonCheckAt, setLastDaemonCheckAt] = useState<number | null>(
    pollDaemonWarning || typeof preflightWarning !== 'undefined'
      ? Date.now()
      : null
  );
  const daemonWarningRef = useRef<string | null>(preflightWarning ?? null);
  const daemonRecoveryNoticeTimeoutRef = useRef<ReturnType<
    typeof setTimeout
  > | null>(null);

  const clearDaemonRecoveryNotice = useCallback(() => {
    if (daemonRecoveryNoticeTimeoutRef.current) {
      clearTimeout(daemonRecoveryNoticeTimeoutRef.current);
      daemonRecoveryNoticeTimeoutRef.current = null;
    }
    setDaemonRecoveryNotice(null);
  }, []);

  const showDaemonRecoveryNotice = useCallback(() => {
    if (!interactive || phase !== 'waiting') {
      return;
    }

    if (daemonRecoveryNoticeTimeoutRef.current) {
      clearTimeout(daemonRecoveryNoticeTimeoutRef.current);
    }

    setDaemonRecoveryNotice(TASK_ENTRY_RESTORED_MESSAGE);
    daemonRecoveryNoticeTimeoutRef.current = setTimeout(() => {
      setDaemonRecoveryNotice(null);
      daemonRecoveryNoticeTimeoutRef.current = null;
    }, DAEMON_RECOVERY_NOTICE_MS);
  }, [interactive, phase]);

  const syncDaemonWarning = useCallback(
    async (source: DaemonSyncSource) => {
      if (!pollDaemonWarning) {
        if (source === 'manual') {
          showMessage(DAEMON_HEALTH_UNAVAILABLE_MESSAGE);
        }
        return;
      }

      // Race the user-supplied poll against a hard timeout. AbortController
      // would be cleaner but the callback signature is opaque (no signal
      // parameter), so we rely on Promise.race + a cleanup flag instead.
      let timedOut = false;
      let timeoutHandle: ReturnType<typeof setTimeout> | null = null;
      const timeoutPromise = new Promise<string | undefined>((resolve) => {
        timeoutHandle = setTimeout(() => {
          timedOut = true;
          resolve(undefined);
        }, DAEMON_POLL_TIMEOUT_MS);
      });
      let pollResult: string | undefined;
      try {
        pollResult = await Promise.race([pollDaemonWarning(), timeoutPromise]);
      } catch (error) {
        onWarning?.('useDaemonState.pollDaemonWarning.rejected', error);
        pollResult = undefined;
      } finally {
        if (timeoutHandle !== null) {
          clearTimeout(timeoutHandle);
        }
      }
      if (timedOut) {
        onWarning?.('useDaemonState.pollDaemonWarning.timeout', {
          source,
          timeoutMs: DAEMON_POLL_TIMEOUT_MS,
        });
      }
      const nextWarning = pollResult ?? null;
      const previousWarning = daemonWarningRef.current;
      daemonWarningRef.current = nextWarning;
      setDaemonWarning(nextWarning);
      if (nextWarning) {
        clearDaemonRecoveryNotice();
      }
      setLastDaemonCheckAt(Date.now());

      const transition = describeDaemonWarningTransition(
        previousWarning,
        nextWarning,
        source
      );
      if (transition.message) {
        showMessage(transition.message);
      }
      if (transition.recovered) {
        showDaemonRecoveryNotice();
      }
    },
    [
      clearDaemonRecoveryNotice,
      onWarning,
      pollDaemonWarning,
      showDaemonRecoveryNotice,
      showMessage,
    ]
  );

  useEffect(() => {
    const nextWarning = preflightWarning ?? null;
    daemonWarningRef.current = nextWarning;
    setDaemonWarning(nextWarning);
    if (nextWarning) {
      clearDaemonRecoveryNotice();
    }
    if (pollDaemonWarning || typeof preflightWarning !== 'undefined') {
      setLastDaemonCheckAt(Date.now());
      return;
    }
    setLastDaemonCheckAt(null);
  }, [clearDaemonRecoveryNotice, pollDaemonWarning, preflightWarning]);

  useEffect(() => clearDaemonRecoveryNotice, [clearDaemonRecoveryNotice]);

  useEffect(() => {
    if (!pollDaemonWarning) {
      return;
    }
    // Skip background polling while a task is running. The in-flight task
    // surfaces daemon errors through its own response, and a parallel poll
    // would only race the task's own state transitions.
    if (phase === 'running') {
      return;
    }

    let disposed = false;
    let inFlight = false;

    const pollDaemonWarningNow = async () => {
      if (disposed || inFlight) {
        return;
      }

      inFlight = true;
      try {
        await syncDaemonWarning('poll');
      } finally {
        inFlight = false;
      }
    };

    const interval = setInterval(() => {
      void pollDaemonWarningNow();
    }, DAEMON_WARNING_POLL_MS);

    return () => {
      disposed = true;
      clearInterval(interval);
    };
  }, [phase, pollDaemonWarning, syncDaemonWarning]);

  return {
    daemonWarning,
    daemonRecoveryNotice,
    daemonStatus: daemonWarning
      ? 'down'
      : pollDaemonWarning || typeof preflightWarning !== 'undefined'
      ? 'up'
      : undefined,
    lastDaemonCheckAt,
    taskEntryBlockedByDaemon:
      interactive && phase === 'waiting' && Boolean(daemonWarning),
    syncDaemonWarning,
  };
}
