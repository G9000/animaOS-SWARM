import type {
  ActiveHours,
  Automation,
  AutomationOutcome,
  AutomationRunOutcome,
  AutomationTrigger,
} from '@animaOS-SWARM/sdk';

import { formatWhen } from './approvals';
import type { ToolStep } from './transcript';

export const DAY_NAMES = [
  'Sun',
  'Mon',
  'Tue',
  'Wed',
  'Thu',
  'Fri',
  'Sat',
] as const;

export const PHRASE_NOT_UNDERSTOOD =
  'Try “every 2 hours”, “weekdays at 9am”, “every monday at 8:30”, “tomorrow at 15:00”, or “in 20 minutes”, or enter a cron expression.';
export const AGENT_CREATED_NOTE = 'Created by your companion';

/** The browser's IANA time zone, for wall-clock phrases and presets. */
export function localTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

function pad(value: number): string {
  return String(value).padStart(2, '0');
}

/** A trigger in words (spec §15.2 "Check-in · every 30 min"). */
export function describeTrigger(trigger: AutomationTrigger): string {
  switch (trigger.type) {
    case 'interval': {
      const ms = trigger.intervalMs;
      if (ms % 86_400_000 === 0)
        return ms === 86_400_000
          ? 'every day'
          : `every ${ms / 86_400_000} days`;
      if (ms % 3_600_000 === 0)
        return ms === 3_600_000
          ? 'every hour'
          : `every ${ms / 3_600_000} hours`;
      if (ms % 60_000 === 0) return `every ${ms / 60_000} min`;
      return `every ${Math.round(ms / 1000)} sec`;
    }
    case 'daily':
      return `daily at ${pad(trigger.hour)}:${pad(trigger.minute)} (${trigger.timeZone})`;
    case 'cron':
      return `cron ${trigger.expression} (${trigger.timeZone})`;
    case 'once':
      return `once, ${formatWhen(trigger.atMs)}`;
  }
}

export function describeActiveHours(hours: ActiveHours): string {
  const days = [...hours.days].sort((left, right) => left - right);
  const key = days.join(',');
  const label =
    days.length === 7
      ? 'every day'
      : key === '1,2,3,4,5'
        ? 'weekdays'
        : key === '0,6'
          ? 'weekends'
          : days.map((day) => DAY_NAMES[day]).join(', ');
  return `${hours.start}–${hours.end}, ${label} (${hours.timeZone})`;
}

export const OUTCOME_LABELS: Record<AutomationOutcome['status'], string> = {
  silent: 'Nothing to report',
  spoke: 'Replied',
  error: 'Failed',
  stopped: 'Stopped',
};

export const RUN_OUTCOME_LABELS: Record<AutomationRunOutcome, string> = {
  silent: 'Nothing to report',
  spoke: 'Replied',
  failed: 'Failed',
  stopped: 'Stopped',
};

/** The automation a successful `create_automation` call made, by its tool
 *  call (and run, when the step knows it). */
export function automationNoticeFor(
  step: ToolStep,
  automations: readonly Automation[],
): Automation | null {
  if (step.name !== 'create_automation' || step.status !== 'success')
    return null;
  return (
    automations.find(
      (automation) =>
        automation.createdBy.kind === 'agent' &&
        automation.createdBy.toolCallId === step.toolCallId &&
        (step.runId === null || automation.createdBy.runId === step.runId),
    ) ?? null
  );
}

/** A check-in session's automation id (`schedule:<id>`). */
export function checkinScheduleId(sessionId: string): string | null {
  const id = sessionId.startsWith('schedule:')
    ? sessionId.slice('schedule:'.length)
    : '';
  return id || null;
}
