import type { Run } from '@animaOS-SWARM/sdk';

/** Why an interrupted run did not reply: a restart in the web's words,
 *  otherwise the daemon's own reason, such as a stop or a full queue that
 *  left a steer unread. */
function interruptedReason(run: Run): string {
  switch (run.error?.code) {
    case 'restart_during_run':
      return 'The daemon restarted while this reply was running.';
    case 'restart_before_start':
      return 'The daemon restarted before this message was sent.';
    default:
      return run.error?.message.trim() || 'This message wasn’t sent.';
  }
}

/** How a run ended when it did not simply reply (spec §15.2). */
export function RunOutcomeCard({
  run,
  onSendAgain,
  resent = false,
}: {
  run: Run;
  onSendAgain?: (run: Run) => void;
  /** Already sent again from this page: its button is used up. */
  resent?: boolean;
}) {
  if (run.status === 'cancelled')
    return (
      <p className="run-outcome" data-outcome="stopped">
        Stopped
      </p>
    );
  if (run.status === 'failed')
    return (
      <div className="run-outcome" data-outcome="failed">
        <p>
          {run.error
            ? `This reply failed: ${run.error.message}`
            : 'This reply failed.'}
        </p>
        {onSendAgain && (
          <button
            type="button"
            className="studio-tool-button"
            disabled={resent}
            onClick={() => onSendAgain(run)}
          >
            Retry
          </button>
        )}
      </div>
    );
  return (
    <div className="run-outcome" data-outcome="interrupted">
      <p>{interruptedReason(run)}</p>
      {run.toolsStarted.length > 0 && (
        <p className="run-outcome-warning">
          Tools had started ({run.toolsStarted.join(', ')}). Check their effects
          before sending again.
        </p>
      )}
      {onSendAgain && (
        <button
          type="button"
          className="studio-tool-button"
          disabled={resent}
          onClick={() => onSendAgain(run)}
        >
          Send again
        </button>
      )}
    </div>
  );
}
