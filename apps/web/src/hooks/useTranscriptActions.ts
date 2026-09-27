import { useEffect, useMemo, useRef } from 'react';
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
  sendAgain: (run: Run) => void;
  compact: (session: Session) => void;
  openSession: (target: HelperTarget) => void;
}

/**
 * What the owner can do from the open session's transcript (spec §15.2):
 * cancel a queued message, send a failed or interrupted one again, compact
 * the session, and open a helper's session, each as the session's
 * capabilities allow. The actions keep one identity per capability, list,
 * and stream change, so a keystroke in the composer does not re-render
 * every message.
 */
export function useTranscriptActions({
  session,
  resendable,
  liveRuns,
  sessions,
  stopRun,
  sendAgain,
  compact,
  openSession,
}: TranscriptActionOptions): TranscriptActions {
  const latestRef = useRef({ stopRun, sendAgain, compact, openSession });
  useEffect(() => {
    latestRef.current = { stopRun, sendAgain, compact, openSession };
  });
  const cancellable = session?.capabilities.stop === true;
  const canResend = resendable && session?.capabilities.send === true;
  const compactable = session?.capabilities.compact ? session : null;
  return useMemo<TranscriptActions>(
    () => ({
      ...(cancellable
        ? { onCancelQueued: (run: Run) => latestRef.current.stopRun(run) }
        : {}),
      ...(canResend
        ? { onSendAgain: (run: Run) => latestRef.current.sendAgain(run) }
        : {}),
      ...(compactable
        ? { onCompact: () => latestRef.current.compact(compactable) }
        : {}),
      helperSession: (step) => helperSessionTarget(step, liveRuns, sessions),
      onOpenSession: (target) => latestRef.current.openSession(target),
    }),
    [cancellable, canResend, compactable, liveRuns, sessions],
  );
}
