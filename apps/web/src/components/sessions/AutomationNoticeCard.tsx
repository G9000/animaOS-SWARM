import { useState } from 'react';
import { DaemonHttpError, type Automation } from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE, formatWhen } from '../../lib/approvals';
import { describeActiveHours, describeTrigger } from '../../lib/automations';
import { revealInvisible } from '../../lib/skills';

/** An automation the companion created (spec §9.3, §15.2), with Undo. Its
 *  name and schedule are the companion's text: shown as text, with any
 *  invisible characters made visible. */
export function AutomationNoticeCard({
  automation,
  onUndo,
}: {
  automation: Automation;
  onUndo?: (automation: Automation) => Promise<boolean>;
}) {
  const [undoing, setUndoing] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const undo = async () => {
    if (!onUndo) return;
    setUndoing(true);
    setProblem(null);
    try {
      // On success the automation leaves the list, and this card with it.
      if (await onUndo(automation)) return;
    } catch (caught) {
      setProblem(
        caught instanceof DaemonHttpError
          ? caught.message
          : COMPANION_UNREACHABLE,
      );
    }
    setUndoing(false);
  };
  const name = revealInvisible(automation.name).text;
  const detail = revealInvisible(
    describeTrigger(automation.trigger) +
      (automation.activeHours
        ? ` · ${describeActiveHours(automation.activeHours)}`
        : '') +
      (automation.enabled
        ? ` · next ${formatWhen(automation.nextDueAtMs)}`
        : ' · paused'),
  ).text;
  return (
    <div
      className="automation-notice"
      role="note"
      aria-label={`Automation ${name}`}
    >
      <p className="automation-notice-title">
        <span className="automation-badge">New automation</span> {name}
      </p>
      <p className="automation-notice-detail">{detail}</p>
      {onUndo && (
        <button
          type="button"
          className="studio-tool-button"
          disabled={undoing}
          onClick={() => void undo()}
        >
          {undoing ? 'Undoing…' : 'Undo'}
        </button>
      )}
      {problem && (
        <p className="automation-problem" role="status">
          {problem}
        </p>
      )}
    </div>
  );
}
