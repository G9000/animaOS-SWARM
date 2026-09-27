import { useId, useState } from 'react';

import { formatElapsed, type ToolStep } from '../../lib/transcript';

const STATUS_LABELS: Record<ToolStep['status'], string> = {
  running: 'running',
  success: 'done',
  error: 'failed',
};

/** One tool call (spec §15.2): name, short arguments, a spinner then ✓ or ✗,
 *  its duration, and an expandable result. */
export function ToolStepCard({ step }: { step: ToolStep }) {
  const [open, setOpen] = useState(false);
  const resultId = useId();
  return (
    <div className="tool-step" data-status={step.status}>
      <button
        type="button"
        className="tool-step-toggle"
        aria-expanded={open}
        aria-controls={resultId}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="tool-step-icon" aria-hidden>
          {step.status === 'running' ? (
            <span className="tool-step-spinner" />
          ) : step.status === 'success' ? (
            '✓'
          ) : (
            '✗'
          )}
        </span>
        <span className="tool-step-name">{step.name}</span>
        {step.argumentsPreview && (
          <span className="tool-step-args">{step.argumentsPreview}</span>
        )}
        <span className="sr-only">, {STATUS_LABELS[step.status]}</span>
        {step.durationMs !== null && (
          <span className="tool-step-duration">
            {formatElapsed(step.durationMs)}
          </span>
        )}
      </button>
      {open && (
        <div id={resultId} className="tool-step-result">
          {step.result === null ? (
            <p>
              {step.status === 'running'
                ? 'Still running…'
                : 'No result was recorded.'}
            </p>
          ) : (
            <pre>{step.result}</pre>
          )}
          {step.truncated && (
            <p className="tool-step-note">Result shortened to 2 KiB.</p>
          )}
        </div>
      )}
    </div>
  );
}
