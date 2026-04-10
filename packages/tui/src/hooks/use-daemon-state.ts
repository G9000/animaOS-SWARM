import { useState, useEffect, useRef, useCallback } from 'react';
import {
  DAEMON_HEALTH_UNAVAILABLE_MESSAGE,
  describeDaemonWarningTransition,
} from '@animaOS-SWARM/core';

const DAEMON_WARNING_POLL_MS = 5000;
const DAEMON_RECOVERY_NOTICE_MS = 4000;
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

      const nextWarning = (await pollDaemonWarning()) ?? null;
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
  }, [pollDaemonWarning, syncDaemonWarning]);

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
