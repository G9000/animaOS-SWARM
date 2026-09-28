import { useEffect, useMemo, useRef, useState } from 'react';
import type { Run, Session } from '@animaOS-SWARM/sdk';

import type { LiveState } from '../lib/session-events';
import type {
  HelperTarget,
  ToolStep,
  TranscriptActions,
} from '../lib/transcript';

/** The session a helper card opens: its live child run's, else the listed
 *  helper session the call's run started. */
export function helperSessionTarget(
  step: ToolStep,
  liveRuns: LiveState['runs'],
  sessions: readonly Session[],
): HelperTarget | null {
  const helperAgentId = step.helper?.agentId;
  if (!helperAgentId || !step.runId) return null;
  const child = Object.values(liveRuns).find(
    (item) =>
      item.run.parentRunId === step.runId && item.run.agentId === helperAgentId,
  );
  if (child)
    return { agentId: child.run.agentId, sessionId: child.run.sessionId };
  const session = sessions.find(
    (item) => item.agentId === helperAgentId && item.parentRunId === step.runId,
  );
  return session ? { agentId: session.agentId, sessionId: session.id } : null;
}

export interface TranscriptActionOptions {
  /** The open session, once its record is known. */
  session: Session | null;
  /** Its messages can be sent again (a Telegram session needs its connector). */
  resendable: boolean;
  /** The companion's live runs, where a helper card finds its child run. */
  liveRuns: LiveState['runs'];
  /** The listed sessions, where it finds a finished helper's session. */
  sessions: readonly Session[];
  stopRun: (run: Run) => void;
  /** Takes back a send the daemon has not accepted (S3b-I). */
  cancelPending: (key: string) => void;
  /** Sends a run's message again; false when it could not go. */
  sendAgain: (run: Run) => boolean;
  compact: (session: Session) => void;
  /** A manual Compact is in flight (S3b-C). */
  compacting: boolean;
  openSession: (target: HelperTarget) => void;
}

/**
 * What the owner can do from the open session's transcript (spec §15.2):
 * cancel a queued message, send a failed or interrupted one again, compact
 * the session, and open a helper's session, each as the session's
 * capabilities allow. The actions keep one identity per capability and
 * sessions-list change, so neither a keystroke in the composer nor a
 * streamed delta renders every message again. A helper card reads the live
 * runs as it renders: a live run's cards render with each of its events,
 * and a history card again once the sessions list, read after every run's
 * lifecycle event, changes.
 */
export function useTranscriptActions({
  session,
  resendable,
  liveRuns,
  sessions,
  stopRun,
  cancelPending,
  sendAgain,
  compact,
  compacting,
  openSession,
}: TranscriptActionOptions): TranscriptActions {
  const latest = { stopRun, cancelPending, sendAgain, compact, openSession };
  const latestRef = useRef(latest);
  useEffect(() => {
    latestRef.current = latest;
  });
  // Read during render, so the cards rendered with it see this render's runs.
  const liveRunsRef = useRef(liveRuns);
  liveRunsRef.current = liveRuns;
  // A run sent again from this page is not sent a second time: its button
  // is used up for as long as the page is open.
  const [resent, setResent] = useState<ReadonlySet<string>>(() => new Set());
  const resentRef = useRef(resent);
  const cancellable = session?.capabilities.stop === true;
  const canResend = resendable && session?.capabilities.send === true;
  const compactable = session?.capabilities.compact ? session : null;
  return useMemo<TranscriptActions>(
    () => ({
      ...(cancellable
        ? { onCancelQueued: (run: Run) => latestRef.current.stopRun(run) }
        : {}),
      onCancelPending: (key) => latestRef.current.cancelPending(key),
      ...(canResend
        ? {
            onSendAgain: (run: Run) => {
              if (resentRef.current.has(run.id)) return;
              if (!latestRef.current.sendAgain(run)) return;
              resentRef.current = new Set(resentRef.current).add(run.id);
              setResent(resentRef.current);
            },
          }
        : {}),
      resentRunIds: resent,
      ...(compactable
        ? {
            onCompact: () => latestRef.current.compact(compactable),
            compacting,
          }
        : {}),
      helperSession: (step) =>
        helperSessionTarget(step, liveRunsRef.current, sessions),
      onOpenSession: (target) => latestRef.current.openSession(target),
    }),
    [cancellable, canResend, compactable, sessions, resent, compacting],
  );
}
