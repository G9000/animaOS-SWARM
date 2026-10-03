import { useEffect, useState } from 'react';
import type { Automation } from '@animaOS-SWARM/sdk';

import {
  AutomationEditor,
  type AutomationDraft,
} from '../components/automations/AutomationEditor';
import { AutomationHistory } from '../components/automations/AutomationHistory';
import type { AutomationsView } from '../hooks/useAutomations';
import { formatWhen } from '../lib/approvals';
import {
  AGENT_CREATED_NOTE,
  OUTCOME_LABELS,
  describeActiveHours,
  describeTrigger,
  localTimeZone,
} from '../lib/automations';
import { revealInvisible } from '../lib/skills';

export interface AutomationsPageProps {
  view: AutomationsView;
  agentId: string;
  /** `LiveState.automationsVersion`, so an open history reads again. */
  version: number;
  online: boolean;
  /** The companion's Telegram connector with an approved chat, if any. */
  telegramConnectorId: string | null;
  /** An automation to open in the editor ("Edit automation" in a check-in). */
  focusId?: string | null;
  onFocusHandled?: () => void;
  onOpenSession: (sessionId: string) => void;
}

/** Spec §15.4: automations with their next run, last outcome, and
 *  failures; create and edit; Run now, pause and resume, delete; history. */
export function AutomationsPage({
  view,
  agentId,
  version,
  online,
  telegramConnectorId,
  focusId = null,
  onFocusHandled,
  onOpenSession,
}: AutomationsPageProps) {
  const [editing, setEditing] = useState<{
    automation: Automation | null;
  } | null>(null);
  const [historyId, setHistoryId] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  useEffect(() => {
    if (!focusId) return;
    const focused = view.automations.find((item) => item.id === focusId);
    if (!focused) return;
    setEditing({ automation: focused });
    onFocusHandled?.();
  }, [focusId, view.automations, onFocusHandled]);

  const hasHeartbeat = view.automations.some(
    (item) => item.preset === 'heartbeat',
  );
  const history = historyId
    ? (view.automations.find((item) => item.id === historyId) ?? null)
    : null;

  const save = async (draft: AutomationDraft) => {
    const current = editing?.automation ?? null;
    let saved = false;
    if (current) {
      saved = await view.update(current, {
        // The daemon refuses an empty name; the target is sent only when
        // the owner changed it.
        ...(draft.name ? { name: draft.name } : {}),
        prompt: draft.prompt,
        ...(draft.target ? { target: draft.target } : {}),
        activeHours: draft.activeHours,
        ...(draft.trigger ? { trigger: draft.trigger } : {}),
      });
    } else if (draft.trigger) {
      saved = await view.create({
        prompt: draft.prompt,
        trigger: draft.trigger,
        target: draft.target ?? { type: 'workspace' },
        ...(draft.name ? { name: draft.name } : {}),
        ...(draft.activeHours ? { activeHours: draft.activeHours } : {}),
      });
    }
    if (saved) setEditing(null);
    return saved;
  };

  return (
    <div className="automations-page">
      {view.error && (
        <p className="automations-error" role="alert">
          {view.error}
        </p>
      )}
      <section
        className="automations-section"
        aria-labelledby="automations-heading"
      >
        <div className="automations-header">
          <h2 id="automations-heading">Automations</h2>
          <button
            type="button"
            className="studio-tool-button"
            disabled={!online}
            onClick={() => setEditing({ automation: null })}
          >
            New automation
          </button>
          <button
            type="button"
            className="studio-tool-button"
            disabled={!online || hasHeartbeat}
            onClick={() => void view.createHeartbeat(localTimeZone())}
          >
            Add heartbeat
          </button>
        </div>
        {editing && (
          <AutomationEditor
            key={editing.automation?.id ?? 'new'}
            automation={editing.automation}
            telegramConnectorId={telegramConnectorId}
            onSave={save}
            onCancel={() => setEditing(null)}
          />
        )}
        {view.loaded && view.automations.length === 0 ? (
          <p className="automations-empty">
            No automations yet. Create one, add the heartbeat, or ask your
            companion to schedule something.
          </p>
        ) : (
          <ul className="automations-list">
            {view.automations.map((automation) => {
              // Names and prompts can be the companion's: invisible
              // characters are shown as markers, never rendered.
              const name = revealInvisible(automation.name).text;
              const prompt = revealInvisible(automation.prompt).text;
              return (
                <li key={automation.id}>
                  <article
                    className="automation-row"
                    aria-label={name}
                    data-enabled={automation.enabled || undefined}
                  >
                    <div className="automation-row-header">
                      <h3 className="automation-name">{name}</h3>
                      {automation.preset === 'heartbeat' && (
                        <span className="automation-badge">Heartbeat</span>
                      )}
                      {automation.running && (
                        <span className="automation-badge" data-running>
                          Running
                        </span>
                      )}
                      {!automation.enabled && (
                        <span className="automation-badge">Paused</span>
                      )}
                    </div>
                    <p className="automation-schedule">
                      {describeTrigger(automation.trigger)}
                      {automation.activeHours &&
                        ` · ${describeActiveHours(automation.activeHours)}`}
                    </p>
                    <p className="automation-meta">
                      {automation.enabled
                        ? `Next run ${formatWhen(automation.nextDueAtMs)}`
                        : 'Paused'}
                      {automation.lastOutcome &&
                        ` · Last: ${OUTCOME_LABELS[automation.lastOutcome.status]}`}
                      {automation.counters.consecutiveFailures > 0 &&
                        ` · ${automation.counters.consecutiveFailures} failed in a row`}
                      {automation.target.type === 'connector' && ' · Telegram'}
                    </p>
                    {automation.createdBy.kind === 'agent' && (
                      <p className="automation-note">{AGENT_CREATED_NOTE}</p>
                    )}
                    <pre className="automation-prompt">{prompt}</pre>
                    <div className="automation-actions">
                      <button
                        type="button"
                        className="studio-tool-button"
                        disabled={!online || automation.running}
                        onClick={() => void view.runNow(automation)}
                      >
                        Run now
                      </button>
                      <button
                        type="button"
                        className="studio-tool-button"
                        disabled={!online}
                        onClick={() =>
                          void view.setEnabled(automation, !automation.enabled)
                        }
                      >
                        {automation.enabled ? 'Pause' : 'Resume'}
                      </button>
                      <button
                        type="button"
                        className="studio-tool-button"
                        disabled={!online}
                        onClick={() => setEditing({ automation })}
                      >
                        Edit
                      </button>
                      <button
                        type="button"
                        className="studio-tool-button"
                        onClick={() => setHistoryId(automation.id)}
                      >
                        History
                      </button>
                      {automation.lastFiredAtMs !== null &&
                        automation.target.type === 'workspace' && (
                          <button
                            type="button"
                            className="studio-tool-button"
                            onClick={() =>
                              onOpenSession(`schedule:${automation.id}`)
                            }
                          >
                            Open thread
                          </button>
                        )}
                      {confirming === automation.id ? (
                        <>
                          <span className="automations-empty">
                            Delete it? Its check-in thread stays until you
                            delete it.
                          </span>
                          <button
                            type="button"
                            className="studio-tool-button"
                            onClick={() => {
                              setConfirming(null);
                              void view.remove(automation);
                            }}
                          >
                            Confirm delete
                          </button>
                          <button
                            type="button"
                            className="studio-tool-button"
                            onClick={() => setConfirming(null)}
                          >
                            Keep
                          </button>
                        </>
                      ) : (
                        <button
                          type="button"
                          className="studio-tool-button"
                          disabled={!online}
                          onClick={() => setConfirming(automation.id)}
                        >
                          Delete
                        </button>
                      )}
                    </div>
                  </article>
                </li>
              );
            })}
          </ul>
        )}
      </section>
      {history && (
        <AutomationHistory
          agentId={agentId}
          automation={history}
          version={version}
          onClose={() => setHistoryId(null)}
          onOpenSession={onOpenSession}
        />
      )}
    </div>
  );
}
