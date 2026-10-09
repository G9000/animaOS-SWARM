import { describe, expect, it } from 'vitest';

import { automationFixture } from '../test/automations';
import {
  AGENT_CREATED_NOTE,
  OUTCOME_LABELS,
  PHRASE_NOT_UNDERSTOOD,
  RUN_OUTCOME_LABELS,
  automationNoticeFor,
  checkinScheduleId,
  describeActiveHours,
  describeTrigger,
} from './automations';
import type { ToolStep } from './transcript';

function step(overrides: Partial<ToolStep> = {}): ToolStep {
  return {
    stepId: 'run_1:0',
    toolCallId: 'call-1',
    name: 'create_automation',
    argumentsPreview: '{}',
    status: 'success',
    durationMs: 5,
    result: 'Created',
    truncated: false,
    runId: 'run_1',
    helper: null,
    ...overrides,
  };
}

describe('automation labels', () => {
  it('describes triggers and active hours in words', () => {
    expect(describeTrigger({ type: 'interval', intervalMs: 1_800_000 })).toBe(
      'every 30 min',
    );
    expect(describeTrigger({ type: 'interval', intervalMs: 3_600_000 })).toBe(
      'every hour',
    );
    expect(describeTrigger({ type: 'interval', intervalMs: 7_200_000 })).toBe(
      'every 2 hours',
    );
    expect(describeTrigger({ type: 'interval', intervalMs: 86_400_000 })).toBe(
      'every day',
    );
    expect(
      describeTrigger({ type: 'daily', hour: 9, minute: 5, timeZone: 'UTC' }),
    ).toBe('daily at 09:05 (UTC)');
    expect(
      describeTrigger({
        type: 'cron',
        expression: '0 9 * * 1-5',
        timeZone: 'Europe/London',
      }),
    ).toBe('cron 0 9 * * 1-5 (Europe/London)');
    expect(
      describeTrigger({ type: 'once', atMs: 0 }).startsWith('once, '),
    ).toBe(true);
    const hours = { start: '08:00', end: '22:00', timeZone: 'UTC' };
    expect(describeActiveHours({ ...hours, days: [0, 1, 2, 3, 4, 5, 6] })).toBe(
      '08:00–22:00, every day (UTC)',
    );
    expect(describeActiveHours({ ...hours, days: [1, 2, 3, 4, 5] })).toBe(
      '08:00–22:00, weekdays (UTC)',
    );
    expect(describeActiveHours({ ...hours, days: [6, 0] })).toBe(
      '08:00–22:00, weekends (UTC)',
    );
    expect(describeActiveHours({ ...hours, days: [5, 1] })).toBe(
      '08:00–22:00, Mon, Fri (UTC)',
    );
    expect(OUTCOME_LABELS.error).toBe('Failed');
    expect(RUN_OUTCOME_LABELS.failed).toBe('Failed');
    expect(PHRASE_NOT_UNDERSTOOD).toContain('cron');
    expect(AGENT_CREATED_NOTE).toBe('Created by your companion');
  });

  it('finds the automation a create_automation call made', () => {
    const made = automationFixture('schedule-1', {
      createdBy: {
        kind: 'agent',
        agentId: 'agent-main',
        sessionId: 'chat:1',
        runId: 'run_1',
        toolCallId: 'call-1',
      },
    });
    const others = [automationFixture('schedule-0'), made];
    expect(automationNoticeFor(step(), others)).toBe(made);
    expect(automationNoticeFor(step({ runId: null }), others)).toBe(made);
    expect(automationNoticeFor(step({ runId: 'run_2' }), others)).toBeNull();
    expect(automationNoticeFor(step({ status: 'error' }), others)).toBeNull();
    expect(
      automationNoticeFor(step({ name: 'list_automations' }), others),
    ).toBeNull();
    expect(automationNoticeFor(step(), [])).toBeNull();
  });

  it('reads a check-in session id', () => {
    expect(checkinScheduleId('schedule:schedule-1')).toBe('schedule-1');
    expect(checkinScheduleId('schedule:')).toBeNull();
    expect(checkinScheduleId('chat:1')).toBeNull();
  });
});
