import type { HelperTarget, ToolStep } from '../../lib/transcript';

const HELPER_STATUS: Record<ToolStep['status'], string> = {
  running: 'Working…',
  success: 'Finished',
  error: 'Failed',
};

/** A helper or delegated run started by a tool call (spec §15.2): its live
 *  status and a way into its session. */
export function HelperCard({
  step,
  target,
  onOpen,
}: {
  step: ToolStep;
  target: HelperTarget | null;
  onOpen?: (target: HelperTarget) => void;
}) {
  return (
    <div className="helper-card" data-status={step.status}>
      <div className="helper-card-body">
        <p className="helper-card-title">
          Helper · {step.helper?.label ?? step.name}
        </p>
        <p className="helper-card-status">{HELPER_STATUS[step.status]}</p>
      </div>
      {target && onOpen && (
        <button
          type="button"
          className="studio-tool-button"
          onClick={() => onOpen(target)}
        >
          Open session
        </button>
      )}
    </div>
  );
}
