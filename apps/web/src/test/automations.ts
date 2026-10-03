import type { Automation, AutomationRun } from '@animaOS-SWARM/sdk';

export function automationFixture(
  id: string,
  overrides: Partial<Automation> = {},
): Automation {
  return {
    id,
    agentId: 'agent-main',
    name: `Automation ${id}`,
    prompt: 'Check in',
    trigger: { type: 'interval', intervalMs: 1_800_000 },
    activeHours: null,
    enabled: true,
    target: { type: 'workspace' },
    nextDueAtMs: 10,
    lastFiredAtMs: null,
    lastOutcome: null,
    running: false,
    createdBy: { kind: 'owner' },
    preset: null,
    counters: { runs: 0, failures: 0, consecutiveFailures: 0 },
    importIdempotencyKey: null,
    createdAtMs: 1,
    updatedAtMs: 1,
    ...overrides,
  };
}

export function automationRunFixture(
  id: string,
  overrides: Partial<AutomationRun> = {},
): AutomationRun {
  return {
    id,
    scheduleId: 'schedule-1',
    agentId: 'agent-main',
    firedAtMs: 5,
    finishedAtMs: 9,
    outcome: 'spoke',
    runId: 'run_1',
    sessionId: 'schedule:schedule-1',
    errorCode: null,
    manual: false,
    ...overrides,
  };
}
