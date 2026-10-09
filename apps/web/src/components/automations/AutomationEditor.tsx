import { useEffect, useMemo, useState } from 'react';
import {
  DaemonHttpError,
  MAX_AUTOMATION_NAME_CHARS,
  type ActiveHours,
  type Automation,
  type AutomationTarget,
  type AutomationTrigger,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE, formatWhen } from '../../lib/approvals';
import {
  DAY_NAMES,
  PHRASE_NOT_UNDERSTOOD,
  describeTrigger,
  localTimeZone,
} from '../../lib/automations';
import { daemon } from '../../lib/daemon-api';
import { PHRASE_EXAMPLES, parseSchedule } from '../../lib/schedule-parse';
import { invisibleNote, revealInvisible } from '../../lib/skills';

export interface AutomationDraft {
  name: string;
  prompt: string;
  /** Null while editing: the schedule stays as it is. */
  trigger: AutomationTrigger | null;
  activeHours: ActiveHours | null;
  /** Null when the owner did not pick one, or picked Telegram with no
   *  connector to point at: an edit then keeps what is stored, and a new
   *  automation runs in its own thread. */
  target: AutomationTarget | null;
}

export interface AutomationEditorProps {
  /** Null for a new automation. */
  automation: Automation | null;
  /** The companion's Telegram connector with an approved chat, if any. */
  telegramConnectorId: string | null;
  /** True when the daemon took it. */
  onSave: (draft: AutomationDraft) => Promise<boolean>;
  onCancel: () => void;
}

type Preview =
  | { key: string; runs: number[] }
  | { key: string; error: string }
  | null;

const EVERY_DAY = [0, 1, 2, 3, 4, 5, 6];

function zoneOf(automation: Automation | null): string {
  const trigger = automation?.trigger;
  if (trigger && (trigger.type === 'cron' || trigger.type === 'daily'))
    return trigger.timeZone;
  return automation?.activeHours?.timeZone ?? localTimeZone();
}

/** Create or edit an automation (spec §15.4): a plain-language schedule or
 *  a cron expression, the daemon's preview of the next runs, active hours,
 *  and where it runs. */
export function AutomationEditor({
  automation,
  telegramConnectorId,
  onSave,
  onCancel,
}: AutomationEditorProps) {
  const [name, setName] = useState(automation?.name ?? '');
  const [prompt, setPrompt] = useState(automation?.prompt ?? '');
  const [cronMode, setCronMode] = useState(automation?.trigger.type === 'cron');
  const [phrase, setPhrase] = useState('');
  const [expression, setExpression] = useState(
    automation?.trigger.type === 'cron' ? automation.trigger.expression : '',
  );
  const [timeZone, setTimeZone] = useState(() => zoneOf(automation));
  const [hoursOn, setHoursOn] = useState(automation?.activeHours != null);
  const [start, setStart] = useState(automation?.activeHours?.start ?? '08:00');
  const [end, setEnd] = useState(automation?.activeHours?.end ?? '22:00');
  const [days, setDays] = useState<number[]>(
    automation?.activeHours?.days ?? EVERY_DAY,
  );
  const initialTarget =
    automation?.target.type === 'connector' ? 'telegram' : 'workspace';
  const [target, setTarget] = useState<'workspace' | 'telegram'>(initialTarget);
  const [preview, setPreview] = useState<Preview>(null);
  const [saving, setSaving] = useState(false);

  const trigger = useMemo<AutomationTrigger | null>(() => {
    if (cronMode)
      return expression.trim()
        ? { type: 'cron', expression: expression.trim(), timeZone }
        : null;
    if (!phrase.trim()) return null;
    return parseSchedule(phrase, { nowMs: Date.now(), timeZone });
  }, [cronMode, expression, phrase, timeZone]);
  const phraseProblem = !cronMode && phrase.trim() !== '' && trigger === null;
  const activeHours: ActiveHours | null = hoursOn
    ? { start, end, days: [...days].sort((a, b) => a - b), timeZone }
    : null;
  const shown = trigger ?? (cronMode ? null : (automation?.trigger ?? null));
  const previewKey = JSON.stringify([
    phraseProblem ? null : shown,
    activeHours,
  ]);

  useEffect(() => {
    const [previewed, hours] = JSON.parse(previewKey) as [
      AutomationTrigger | null,
      ActiveHours | null,
    ];
    if (!previewed) {
      setPreview(null);
      return;
    }
    const controller = new AbortController();
    daemon
      .previewAutomation(
        { trigger: previewed, ...(hours ? { activeHours: hours } : {}) },
        { signal: controller.signal },
      )
      .then((runs) => {
        if (!controller.signal.aborted) setPreview({ key: previewKey, runs });
      })
      .catch((caught: unknown) => {
        if (controller.signal.aborted) return;
        setPreview({
          key: previewKey,
          error:
            caught instanceof DaemonHttpError
              ? caught.message
              : COMPANION_UNREACHABLE,
        });
      });
    return () => controller.abort();
  }, [previewKey]);

  // The daemon refuses the worst invisible characters; the owner is told
  // about the rest in what they are about to save.
  const hiddenNote = invisibleNote(
    revealInvisible(name).count + revealInvisible(prompt).count,
  );
  const keepsSchedule = automation !== null && !cronMode && trigger === null;
  // A new schedule is saved only once the daemon has shown its next runs.
  const previewed =
    preview !== null && preview.key === previewKey && 'runs' in preview;
  const canSave =
    prompt.trim() !== '' &&
    !phraseProblem &&
    (trigger !== null ? previewed : keepsSchedule) &&
    (!hoursOn || days.length > 0) &&
    !saving;

  const pickedTarget = (): AutomationTarget | null => {
    if (automation && target === initialTarget) return null;
    if (target === 'workspace') return { type: 'workspace' };
    return telegramConnectorId
      ? { type: 'connector', connectorId: telegramConnectorId }
      : null;
  };

  const submit = async () => {
    if (!canSave) return;
    setSaving(true);
    const saved = await onSave({
      name: name.trim(),
      prompt,
      trigger,
      activeHours,
      target: pickedTarget(),
    });
    if (!saved) setSaving(false);
  };

  const toggleDay = (day: number) =>
    setDays((current) =>
      current.includes(day)
        ? current.filter((item) => item !== day)
        : [...current, day],
    );

  return (
    <form
      className="automation-editor"
      aria-label={
        automation
          ? `Edit ${revealInvisible(automation.name).text}`
          : 'New automation'
      }
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <label className="automation-field">
        Name
        <input
          value={name}
          maxLength={MAX_AUTOMATION_NAME_CHARS}
          placeholder="From the prompt when empty"
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label className="automation-field">
        Prompt
        <textarea
          value={prompt}
          rows={3}
          onChange={(event) => setPrompt(event.target.value)}
        />
      </label>
      {hiddenNote && (
        <p className="automation-problem" role="status">
          {hiddenNote}
        </p>
      )}
      <fieldset className="automation-fieldset">
        <legend>Schedule</legend>
        {cronMode ? (
          <label className="automation-field">
            Cron expression
            <input
              value={expression}
              placeholder="0 9 * * 1-5"
              onChange={(event) => setExpression(event.target.value)}
            />
          </label>
        ) : (
          <label className="automation-field">
            When
            <input
              value={phrase}
              placeholder={
                automation
                  ? `Keep: ${describeTrigger(automation.trigger)}`
                  : PHRASE_EXAMPLES[1]
              }
              onChange={(event) => setPhrase(event.target.value)}
            />
          </label>
        )}
        <label className="automation-check">
          <input
            type="checkbox"
            checked={cronMode}
            onChange={(event) => setCronMode(event.target.checked)}
          />
          Use a cron expression
        </label>
        <label className="automation-field">
          Time zone
          <input
            value={timeZone}
            onChange={(event) => setTimeZone(event.target.value)}
          />
        </label>
        {phraseProblem && (
          <p className="automation-problem" role="status">
            {PHRASE_NOT_UNDERSTOOD}
          </p>
        )}
        {preview && 'runs' in preview && (
          <section className="automation-preview" aria-label="Next runs">
            <ul>
              {preview.runs.map((run) => (
                <li key={run}>{formatWhen(run)}</li>
              ))}
            </ul>
          </section>
        )}
        {preview && 'error' in preview && (
          <p className="automation-problem" role="status">
            {preview.error}
          </p>
        )}
      </fieldset>
      <fieldset className="automation-fieldset">
        <legend>Active hours</legend>
        <label className="automation-check">
          <input
            type="checkbox"
            checked={hoursOn}
            onChange={(event) => setHoursOn(event.target.checked)}
          />
          Only run between
        </label>
        {hoursOn && (
          <div className="automation-hours">
            <label className="automation-field">
              From
              <input
                type="time"
                value={start}
                onChange={(event) => setStart(event.target.value)}
              />
            </label>
            <label className="automation-field">
              To
              <input
                type="time"
                value={end}
                onChange={(event) => setEnd(event.target.value)}
              />
            </label>
            <div className="automation-days">
              {DAY_NAMES.map((day, index) => (
                <label key={day} className="automation-check">
                  <input
                    type="checkbox"
                    checked={days.includes(index)}
                    onChange={() => toggleDay(index)}
                  />
                  {day}
                </label>
              ))}
            </div>
          </div>
        )}
      </fieldset>
      <label className="automation-field">
        Runs in
        <select
          value={target}
          onChange={(event) =>
            setTarget(event.target.value as 'workspace' | 'telegram')
          }
        >
          <option value="workspace">Its own thread</option>
          <option value="telegram" disabled={!telegramConnectorId}>
            Telegram
          </option>
        </select>
      </label>
      <div className="automation-actions">
        <button
          type="submit"
          className="studio-tool-button"
          disabled={!canSave}
        >
          {automation ? 'Save changes' : 'Create automation'}
        </button>
        <button type="button" className="studio-tool-button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
