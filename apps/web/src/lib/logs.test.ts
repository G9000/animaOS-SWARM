import { describe, expect, it } from 'vitest';

import { logLineFixture } from '../test/system';
import {
  LOGS_CONNECTING,
  LOGS_COPIED,
  LOGS_COPY_FAILED,
  LOGS_EMPTY,
  LOGS_PAUSED_NOTE,
  LOGS_RECONNECTING,
  LOGS_TOO_OLD,
  MAX_LOGS_SHOWN,
  formatLogTime,
  logsAsText,
  mergeLines,
  nextReconnectDelay,
} from './logs';

describe('mergeLines', () => {
  it('merges ascending, drops duplicate seqs, and keeps the newest cap', () => {
    const existing = [logLineFixture(1), logLineFixture(3)];
    const merged = mergeLines(existing, [
      logLineFixture(4),
      logLineFixture(2),
      logLineFixture(3),
    ]);
    expect(merged.map((line) => line.seq)).toEqual([1, 2, 3, 4]);

    const capped = mergeLines(
      merged,
      [logLineFixture(5), logLineFixture(6)],
      3,
    );
    expect(capped.map((line) => line.seq)).toEqual([4, 5, 6]);
    expect(MAX_LOGS_SHOWN).toBe(2_000);
  });

  it('returns the same array when nothing changed', () => {
    const existing = [logLineFixture(1), logLineFixture(2)];
    expect(mergeLines(existing, [])).toBe(existing);
    expect(mergeLines(existing, [logLineFixture(2)])).toBe(existing);
  });
});

describe('log text', () => {
  it('formats the time', () => {
    const at = new Date(2026, 8, 23, 4, 5, 6, 7).getTime();
    expect(formatLogTime(at)).toBe('04:05:06.007');
  });

  it('copies lines as text', () => {
    const at = new Date(2026, 8, 23, 4, 5, 6, 7).getTime();
    expect(
      logsAsText([
        logLineFixture(1, { at, level: 'warn', target: 'a::b', message: 'x' }),
        logLineFixture(2, { at, level: 'error', target: 'c', message: 'y z' }),
      ]),
    ).toBe('04:05:06.007 WARN a::b x\n04:05:06.007 ERROR c y z');
    expect(logsAsText([])).toBe('');
  });
});

describe('nextReconnectDelay', () => {
  it('backs off from one second to thirty with jitter', () => {
    expect(nextReconnectDelay(0, 0)).toBe(1_000);
    expect(nextReconnectDelay(1, 0)).toBe(2_000);
    expect(nextReconnectDelay(4, 0)).toBe(16_000);
    expect(nextReconnectDelay(5, 0)).toBe(30_000);
    expect(nextReconnectDelay(12, 0)).toBe(30_000);
    expect(nextReconnectDelay(0, 0.5)).toBe(1_100);
    expect(nextReconnectDelay(12, 0.999)).toBeLessThanOrEqual(36_000);
  });
});

describe('strings', () => {
  it('owner-facing strings', () => {
    expect(LOGS_EMPTY).toBe('No log lines match.');
    expect(LOGS_CONNECTING).toBe('Connecting…');
    expect(LOGS_RECONNECTING).toBe('Reconnecting…');
    expect(LOGS_PAUSED_NOTE).toBe(
      'Paused. New lines are held and will appear when you resume.',
    );
    expect(LOGS_COPIED).toBe('Copied');
    expect(LOGS_COPY_FAILED).toBe(
      'Couldn’t copy. Select the lines and copy them instead.',
    );
    expect(LOGS_TOO_OLD).toBe('Update the daemon to see its logs.');
  });
});
