import { describe, expect, it } from 'vitest';

import { statusFixture } from '../test/system';
import {
  HEALTH_TOO_OLD,
  STATUS_POLL_MS,
  approvalsSummary,
  formatUptime,
  healthCards,
} from './status';

const byId = (status = statusFixture()) =>
  Object.fromEntries(healthCards(status).map((card) => [card.id, card]));

describe('healthCards', () => {
  it('builds the cards in order', () => {
    expect(healthCards(statusFixture()).map((card) => card.id)).toEqual([
      'readiness',
      'storage',
      'providers',
      'connectors',
      'automations',
      'approvals',
      'runs',
      'daemon',
    ]);
    const cards = byId();
    expect(cards.readiness.state).toBe('ok');
    expect(cards.storage.summary).toBe('History store: sqlite');
    expect(cards.storage.details).toContain('3 writes waiting to be saved');
    expect(cards.providers.summary).toBe('1 of 2 configured');
    expect(cards.providers.details).toEqual(['OpenAI']);
    expect(cards.runs.summary).toBe('1 running, 2 queued');
    expect(cards.runs.details).toEqual(['completed: 5', 'running: 1']);
    expect(cards.daemon.summary).toBe('Version 0.9.1');
    expect(cards.daemon.details).toEqual([
      'Up 3h 05m',
      'Build abc1234',
      '2 event subscribers',
    ]);
  });

  it('readiness is bad with issues listed', () => {
    const cards = byId(
      statusFixture({
        readiness: { status: 'not_ready', issues: ['No provider', 'No vault'] },
      }),
    );
    expect(cards.readiness.state).toBe('bad');
    expect(cards.readiness.details).toEqual(['No provider', 'No vault']);
  });

  it('a failing history store is bad and shows the redacted error', () => {
    const base = statusFixture();
    const cards = byId(
      statusFixture({
        storage: {
          ...base.storage,
          history: {
            ...base.storage.history,
            healthy: false,
            lastError: 'write failed: [redacted]',
          },
        },
      }),
    );
    expect(cards.storage.state).toBe('bad');
    expect(cards.storage.details).toContain(
      'Last error: write failed: [redacted]',
    );
  });

  it('connectors warn when one is not running', () => {
    const base = statusFixture().connectors[0];
    const cards = byId(
      statusFixture({
        connectors: [base, { ...base, id: 'telegram-2', status: 'degraded' }],
      }),
    );
    expect(cards.connectors.state).toBe('warn');
    expect(cards.connectors.summary).toBe('1 of 2 connectors not ready');
    expect(cards.connectors.details).toEqual([
      'telegram · ready',
      'telegram · degraded',
    ]);
    expect(byId().connectors.state).toBe('ok');
  });

  it('automations warn on failing ones and link to Automations', () => {
    const cards = byId(
      statusFixture({
        automations: { total: 4, enabled: 3, failing: 2, failuresTotal: 9 },
      }),
    );
    expect(cards.automations.state).toBe('warn');
    expect(cards.automations.summary).toBe('3 enabled, 2 failing');
    expect(cards.automations.link).toEqual({
      label: 'Review automations',
      hash: '#/automations',
    });
    expect(byId().automations.link).toBeUndefined();
  });

  it('pending approvals warn and link to Approvals', () => {
    const cards = byId(statusFixture({ approvals: { pending: 2 } }));
    expect(cards.approvals.state).toBe('warn');
    expect(cards.approvals.summary).toBe('2 requests are waiting for you');
    expect(cards.approvals.link).toEqual({
      label: 'Review approvals',
      hash: '#/approvals',
    });
    expect(byId().approvals.state).toBe('ok');
  });
});

describe('formatting', () => {
  it('formats uptime', () => {
    expect(formatUptime(45)).toBe('45s');
    expect(formatUptime(720)).toBe('12m');
    expect(formatUptime(11_100)).toBe('3h 05m');
    expect(formatUptime(2 * 86_400 + 4 * 3_600 + 60)).toBe('2d 4h');
    expect(formatUptime(-3)).toBe('0s');
  });

  it('approvals summary wording', () => {
    expect(approvalsSummary(0)).toBe('Nothing is waiting for you');
    expect(approvalsSummary(1)).toBe('1 request is waiting for you');
    expect(approvalsSummary(5)).toBe('5 requests are waiting for you');
  });

  it('owner-facing strings', () => {
    expect(HEALTH_TOO_OLD).toBe('Update the daemon to see its health.');
    expect(STATUS_POLL_MS).toBe(15_000);
  });
});
