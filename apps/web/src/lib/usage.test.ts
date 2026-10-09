import { DaemonHttpError } from '@animaOS-SWARM/sdk';
import { describe, expect, it } from 'vitest';

import { COMPANION_UNREACHABLE } from './approvals';
import {
  DEFAULT_USAGE_RANGE_DAYS,
  UNPRICED,
  USAGE_EMPTY,
  USAGE_EXPORT_FAILED,
  USAGE_RANGES_DAYS,
  USAGE_SUBSCRIPTION_NOTE,
  USAGE_TOO_OLD,
  USAGE_TRUNCATED_NOTE,
  USAGE_UNPRICED_NOTE,
  costCell,
  costNote,
  fillDays,
  formatCost,
  formatTokens,
  sessionUsageLine,
  sourceLabel,
  subscriptionNote,
  usageCsvFilename,
  usageErrorMessage,
  usageRange,
} from './usage';
import { groupFixture, totalsFixture } from '../test/usage';

const DAY = 86_400_000;

describe('usageRange', () => {
  it('builds a range of whole local days ending tomorrow', () => {
    const now = new Date(2026, 8, 23, 15, 30);
    const range = usageRange(7, now);
    expect(range.to).toBe(new Date(2026, 8, 24).getTime());
    expect(range.from).toBe(new Date(2026, 8, 17).getTime());
    expect(range.tzOffsetMinutes).toBe(-now.getTimezoneOffset() || 0);
  });
});

describe('fillDays', () => {
  it('fills missing days with zero totals', () => {
    const start = Date.UTC(2026, 8, 21);
    const range = { from: start, to: start + 3 * DAY, tzOffsetMinutes: 0 };
    const filled = fillDays(
      [groupFixture('2026-09-22', { calls: 2, totalTokens: 50 })],
      range,
    );
    expect(filled.map((day) => day.key)).toEqual([
      '2026-09-21',
      '2026-09-22',
      '2026-09-23',
    ]);
    expect(filled.map((day) => day.totals.totalTokens)).toEqual([0, 50, 0]);
    expect(filled[0].totals.calls).toBe(0);
  });

  it('cuts days at the range offset', () => {
    // 02:00 UTC is still the previous evening at -300 minutes.
    const from = Date.UTC(2026, 8, 21, 5);
    const range = { from, to: from + 2 * DAY, tzOffsetMinutes: -300 };
    expect(fillDays([], range).map((day) => day.key)).toEqual([
      '2026-09-21',
      '2026-09-22',
    ]);
  });
});

describe('formatting', () => {
  it('formats tokens at the boundaries 999, 1000, 1234, 3.4M', () => {
    expect(formatTokens(0)).toBe('0');
    expect(formatTokens(999)).toBe('999');
    expect(formatTokens(1000)).toBe('1k');
    expect(formatTokens(1234)).toBe('1.2k');
    expect(formatTokens(999_950)).toBe('1M');
    expect(formatTokens(3_400_000)).toBe('3.4M');
  });

  it('formats cost: null as a dash, zero, sub-cent, cents, thousands', () => {
    expect(formatCost(null)).toBe(UNPRICED);
    expect(formatCost(0)).toBe('$0.00');
    expect(formatCost(4_200)).toBe('$0.0042');
    expect(formatCost(10_000)).toBe('$0.01');
    expect(formatCost(1_230_000)).toBe('$1.23');
    expect(formatCost(1_204_500_000)).toBe('$1,204.50');
  });

  it('notes unpriced and subscription calls', () => {
    expect(costNote(totalsFixture())).toBeNull();
    expect(costNote(totalsFixture({ unpricedCalls: 1 }))).toBe(
      USAGE_UNPRICED_NOTE,
    );
    expect(subscriptionNote(totalsFixture())).toBeNull();
    expect(subscriptionNote(totalsFixture({ subscriptionCalls: 2 }))).toBe(
      USAGE_SUBSCRIPTION_NOTE(2),
    );
    expect(costCell(totalsFixture({ costMicros: 1_230_000 }))).toBe('$1.23');
    expect(
      costCell(totalsFixture({ costMicros: 1_230_000, unpricedCalls: 1 })),
    ).toBe('$1.23 + unpriced');
  });

  it('labels sources', () => {
    expect(sourceLabel('chat')).toBe('Chat');
    expect(sourceLabel('api')).toBe('API');
    expect(sourceLabel('mystery')).toBe('mystery');
  });

  it('builds the session usage line', () => {
    expect(sessionUsageLine(null)).toBeNull();
    expect(sessionUsageLine(undefined)).toBeNull();
    expect(sessionUsageLine(totalsFixture())).toBeNull();
    expect(
      sessionUsageLine(
        totalsFixture({ calls: 3, totalTokens: 12_300, costMicros: 40_000 }),
      ),
    ).toBe('12.3k tokens · $0.04');
    expect(
      sessionUsageLine(
        totalsFixture({ calls: 3, totalTokens: 12_300, unpricedCalls: 3 }),
      ),
    ).toBe('12.3k tokens · —');
  });
});

describe('usageErrorMessage', () => {
  it('maps a daemon refusal and a network failure', () => {
    expect(
      usageErrorMessage(new DaemonHttpError(500, { error: 'Broken' })),
    ).toEqual({ message: 'Broken', status: 500 });
    expect(
      usageErrorMessage(new DaemonHttpError(404, { error: 'Not found' })),
    ).toEqual({ message: USAGE_TOO_OLD, status: 404 });
    expect(usageErrorMessage(new TypeError('Failed to fetch'))).toEqual({
      message: COMPANION_UNREACHABLE,
      status: null,
    });
  });
});

describe('usageCsvFilename', () => {
  it('names the csv file by local dates', () => {
    const range = usageRange(7, new Date(2026, 8, 23, 12));
    expect(usageCsvFilename(range)).toBe(
      'anima-usage-2026-09-17-to-2026-09-23.csv',
    );
  });
});

describe('owner-facing strings', () => {
  it('keeps the exact wording', () => {
    expect(USAGE_RANGES_DAYS).toEqual([7, 30, 90]);
    expect(DEFAULT_USAGE_RANGE_DAYS).toBe(30);
    expect(UNPRICED).toBe('—');
    expect(USAGE_EMPTY).toBe('No model calls in this range yet.');
    expect(USAGE_TOO_OLD).toBe('Update the daemon to see usage.');
    expect(USAGE_UNPRICED_NOTE).toBe(
      'Some calls have no known price, so their cost is not counted.',
    );
    expect(USAGE_SUBSCRIPTION_NOTE(1)).toBe(
      '1 call on your ChatGPT subscription have no per-token cost.',
    );
    expect(USAGE_SUBSCRIPTION_NOTE(3)).toBe(
      '3 calls on your ChatGPT subscription have no per-token cost.',
    );
    expect(USAGE_EXPORT_FAILED).toBe('Couldn’t export the usage. Try again.');
    expect(USAGE_TRUNCATED_NOTE).toBe(
      'This range is too large to total completely; choose a shorter one.',
    );
  });
});
