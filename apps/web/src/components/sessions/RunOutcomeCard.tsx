import type { Run } from '@animaOS-SWARM/sdk';

/** How a run ended when it did not simply reply (spec §15.2). */
export function RunOutcomeCard({
  run,
  onSendAgain,
}: {
  run: Run;
  onSendAgain?: (run: Run) => void;
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
            onClick={() => onSendAgain(run)}
          >
            Retry
          </button>
        )}
      </div>
    );
  const duringRun = run.error?.code === 'restart_during_run';
  return (
    <div className="run-outcome" data-outcome="interrupted">
      <p>
        {duringRun
          ? 'The daemon restarted while this reply was running.'
          : 'The daemon restarted before this message was sent.'}
      </p>
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
          onClick={() => onSendAgain(run)}
        >
          Send again
        </button>
      )}
    </div>
  );
}
