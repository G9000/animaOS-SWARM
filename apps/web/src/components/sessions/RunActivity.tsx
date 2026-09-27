import { useEffect, useState, type ReactNode } from 'react';
import { isTerminalRunStatus, type Run } from '@animaOS-SWARM/sdk';

import { isActiveRun, type LiveRun } from '../../lib/session-events';
import {
  formatElapsed,
  liveToolSteps,
  type PendingBubble,
  type ToolStep,
  type TranscriptActions,
} from '../../lib/transcript';
import type { ChatMessage } from '../../lib/types';
import { HelperCard } from './HelperCard';
import { ToolStepCard } from './ToolStepCard';

type RenderMessage = (message: ChatMessage) => ReactNode;

/** Sources whose input is a message someone wrote into the session. */
const WRITTEN_INPUT: ReadonlySet<Run['source']> = new Set([
  'web',
  'api',
  'telegram',
]);

function useElapsed(
  startedAtMs: number | null,
  finishedAtMs: number | null,
): number {
  const [now, setNow] = useState(() => Date.now());
  const running = startedAtMs !== null && finishedAtMs === null;
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [running]);
  if (startedAtMs === null) return 0;
  return Math.max(0, (finishedAtMs ?? now) - startedAtMs);
}

/** Tool steps as cards (spec §15.2): open while the run works, then
 *  collapsed to "Used N tools · Ns". */
export function ToolBlock({
  steps,
  active,
  elapsedMs,
  actions,
}: {
  steps: readonly ToolStep[];
  active: boolean;
  elapsedMs?: number;
  actions?: TranscriptActions;
}) {
  const [open, setOpen] = useState(false);
  const count = steps.length;
  const total =
    elapsedMs ?? steps.reduce((sum, step) => sum + (step.durationMs ?? 0), 0);
  const label = active
    ? count === 0
      ? `Working · ${formatElapsed(total)}`
      : `Working · ${count} ${count === 1 ? 'step' : 'steps'} · ${formatElapsed(total)}`
    : `Used ${count} ${count === 1 ? 'tool' : 'tools'} · ${formatElapsed(total)}`;
  const expanded = active || open;
  return (
    <div className="tool-block" data-active={active || undefined}>
      {active ? (
        <p className="tool-block-label">{label}</p>
      ) : (
        <button
          type="button"
          className="tool-block-toggle"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          {label}
        </button>
      )}
      {expanded && count > 0 && (
        <ul className="tool-block-steps">
          {steps.map((step, index) => (
            // `toolCallId` is not always unique: some providers (e.g. the
            // Google adapter's `call_{name}` fallback) reuse the same id
            // when a run calls the same tool twice, so the index breaks
            // the tie.
            <li key={`${index}:${step.toolCallId}`}>
              {step.helper ? (
                <HelperCard
                  step={step}
                  target={actions?.helperSession?.(step) ?? null}
                  onOpen={actions?.onOpenSession}
                />
              ) : (
                <ToolStepCard step={step} />
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** A run not yet in the session's history (spec §15.2): a queued message,
 *  the reply in progress, or a finished one waiting for its messages. */
export function RunActivity({
  live,
  agentName,
  actions,
  renderMessage,
}: {
  live: LiveRun;
  agentName: string;
  actions?: TranscriptActions;
  renderMessage: RenderMessage;
}) {
  const { run } = live;
  const active = isActiveRun(run);
  const elapsed = useElapsed(
    run.startedAtMs,
    isTerminalRunStatus(run.status)
      ? (run.finishedAtMs ?? run.startedAtMs)
      : null,
  );
  const input = WRITTEN_INPUT.has(run.source)
    ? renderMessage({
        id: `${run.id}:input`,
        role: 'User',
        content: { text: run.input.text },
        created_at_ms: run.createdAtMs,
      })
    : null;

  if (run.status === 'queued') {
    return (
      <div className="run-queued">
        {input}
        <div className="run-queued-meta">
          <span>Queued</span>
          {actions?.onCancelQueued && (
            <button
              type="button"
              className="studio-tool-button"
              onClick={() => actions.onCancelQueued?.(run)}
            >
              Cancel
            </button>
          )}
        </div>
      </div>
    );
  }

  const steps = liveToolSteps(live);
  const at = run.startedAtMs ?? run.createdAtMs;
  return (
    <section
      className="run-activity"
      aria-label={active ? `${agentName} is replying` : `${agentName}’s reply`}
    >
      {input}
      {live.steers.map((steer) => (
        <div key={steer.messageId}>
          {renderMessage({
            id: steer.messageId,
            role: 'User',
            content: { text: steer.text, metadata: { steer: true } },
            created_at_ms: at,
          })}
        </div>
      ))}
      {(active || steps.length > 0) && (
        <ToolBlock
          steps={steps}
          active={active}
          elapsedMs={active ? elapsed : undefined}
          actions={actions}
        />
      )}
      {live.phase === 'compacting' && (
        <p className="run-phase" role="status">
          Compacting earlier messages…
        </p>
      )}
      {live.steps
        .filter((step) => step.text)
        .map((step) => (
          <div key={step.stepId}>
            {renderMessage({
              id: step.stepId,
              role: 'Assistant',
              content: {
                text: step.textOffset > 0 ? `…${step.text}` : step.text,
              },
              created_at_ms: at,
            })}
          </div>
        ))}
    </section>
  );
}

const PENDING_LABELS: Record<PendingBubble['status'], string> = {
  sending: 'Sending…',
  retrying: 'Not delivered yet · retrying…',
  steering: 'Joining the reply in progress…',
};

/** A message on its way to the daemon (spec §15.5). */
export function PendingMessage({
  pending,
  renderMessage,
}: {
  pending: PendingBubble;
  renderMessage: RenderMessage;
}) {
  return (
    <div className="pending-message" data-status={pending.status}>
      {renderMessage({
        id: `pending:${pending.key}`,
        role: 'User',
        content: { text: pending.text },
        created_at_ms: pending.createdAtMs,
      })}
      <p className="pending-message-label">{PENDING_LABELS[pending.status]}</p>
    </div>
  );
}
