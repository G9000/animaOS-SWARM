import { describe, expect, it } from 'vitest';

import {
  PHRASE_EXAMPLES,
  parseClock,
  parseSchedule,
  zonedTimeToUtc,
} from './schedule-parse';

/** 2026-01-05 08:00 UTC, a Monday. */
const NOW = Date.UTC(2026, 0, 5, 8, 0);
const utc = { nowMs: NOW, timeZone: 'UTC' };
const kualaLumpur = { nowMs: NOW, timeZone: 'Asia/Kuala_Lumpur' };

describe('parseClock', () => {
  it('reads 12- and 24-hour times, noon, and midnight', () => {
    expect(parseClock('9')).toEqual({ hour: 9, minute: 0 });
    expect(parseClock('9am')).toEqual({ hour: 9, minute: 0 });
    expect(parseClock('9:30 pm')).toEqual({ hour: 21, minute: 30 });
    expect(parseClock('12am')).toEqual({ hour: 0, minute: 0 });
    expect(parseClock('12pm')).toEqual({ hour: 12, minute: 0 });
    expect(parseClock('15:00')).toEqual({ hour: 15, minute: 0 });
    expect(parseClock('noon')).toEqual({ hour: 12, minute: 0 });
    expect(parseClock('midnight')).toEqual({ hour: 0, minute: 0 });
    for (const bad of ['25:00', '9:60', '13pm', '0am', 'soon', '']) {
      expect(parseClock(bad), bad).toBeNull();
    }
  });
});

describe('parseSchedule', () => {
  it('reads the spec examples', () => {
    expect(PHRASE_EXAMPLES).toEqual([
      'every 2 hours',
      'weekdays at 9am',
      'every monday at 8:30',
      'tomorrow at 15:00',
      'in 20 minutes',
    ]);
    expect(parseSchedule('every 2 hours', utc)).toEqual({
      type: 'interval',
      intervalMs: 7_200_000,
    });
    expect(parseSchedule('Weekdays at 9am', utc)).toEqual({
      type: 'cron',
      expression: '0 9 * * 1-5',
      timeZone: 'UTC',
    });
    expect(parseSchedule('every monday at 8:30', utc)).toEqual({
      type: 'cron',
      expression: '30 8 * * 1',
      timeZone: 'UTC',
    });
    // Kuala Lumpur is UTC+8: 15:00 there tomorrow is 07:00 UTC on the 6th.
    expect(parseSchedule('tomorrow at 15:00', kualaLumpur)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 6, 7, 0),
    });
    expect(parseSchedule('in 20 minutes', utc)).toEqual({
      type: 'once',
      atMs: NOW + 1_200_000,
    });
  });

  it('reads intervals, days, weekends, and lists of weekdays', () => {
    expect(parseSchedule('every hour', utc)).toEqual({
      type: 'interval',
      intervalMs: 3_600_000,
    });
    expect(parseSchedule('every 30 mins', utc)).toEqual({
      type: 'interval',
      intervalMs: 1_800_000,
    });
    expect(parseSchedule('every day at 9:30', utc)).toEqual({
      type: 'daily',
      hour: 9,
      minute: 30,
      timeZone: 'UTC',
    });
    expect(parseSchedule('daily at noon', kualaLumpur)).toEqual({
      type: 'daily',
      hour: 12,
      minute: 0,
      timeZone: 'Asia/Kuala_Lumpur',
    });
    expect(parseSchedule('weekends at 10am', utc)).toEqual({
      type: 'cron',
      expression: '0 10 * * 0,6',
      timeZone: 'UTC',
    });
    expect(parseSchedule('mondays and thursdays at 7pm', utc)).toEqual({
      type: 'cron',
      expression: '0 19 * * 1,4',
      timeZone: 'UTC',
    });
    expect(parseSchedule('every tue, fri at 6:15', utc)).toEqual({
      type: 'cron',
      expression: '15 6 * * 2,5',
      timeZone: 'UTC',
    });
  });

  it('reads one-time phrases against now, in the time zone', () => {
    expect(parseSchedule('today at 17:00', kualaLumpur)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 5, 9, 0),
    });
    expect(parseSchedule('today at 15:00', kualaLumpur)).toBeNull();
    expect(parseSchedule('at 9am', utc)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 5, 9, 0),
    });
    expect(parseSchedule('at 7am', utc)).toEqual({
      type: 'once',
      atMs: Date.UTC(2026, 0, 6, 7, 0),
    });
    expect(parseSchedule('in 2 days', utc)).toEqual({
      type: 'once',
      atMs: NOW + 2 * 86_400_000,
    });
  });

  it('crosses a daylight-saving change on the wall clock', () => {
    // New York springs forward on 2026-03-08: 09:00 EDT is 13:00 UTC.
    expect(
      parseSchedule('tomorrow at 9:00', {
        nowMs: Date.UTC(2026, 2, 7, 12, 0),
        timeZone: 'America/New_York',
      }),
    ).toEqual({ type: 'once', atMs: Date.UTC(2026, 2, 8, 13, 0) });
    expect(zonedTimeToUtc(2026, 3, 7, 9, 0, 'America/New_York')).toBe(
      Date.UTC(2026, 2, 7, 14, 0),
    );
  });

  it('returns null for what it does not understand', () => {
    for (const phrase of [
      '',
      'whenever',
      'every 0 minutes',
      'every 2 weeks',
      'tomorrow at 25:00',
      'every someday at 9',
      '0 9 * * *',
      'friday at 9',
      'on friday at 9',
      'monday at 8:30',
      'on monday and thursday at 7pm',
      'fridays and tuesday at 9',
    ]) {
      expect(parseSchedule(phrase, utc), phrase).toBeNull();
    }
  });

  it('reads a day name only after every or in the plural', () => {
    for (const phrase of [
      'every friday at 9',
      'fridays at 9',
      'on fridays at 9',
    ]) {
      expect(parseSchedule(phrase, utc), phrase).toEqual({
        type: 'cron',
        expression: '0 9 * * 5',
        timeZone: 'UTC',
      });
    }
  });

  it('returns null, and does not throw, for an empty or half-typed time zone', () => {
    for (const timeZone of ['', 'Europe/Lon', 'Not/AZone']) {
      for (const phrase of [
        'tomorrow at 9',
        'every monday at 8',
        'every hour',
      ]) {
        expect(
          parseSchedule(phrase, { nowMs: NOW, timeZone }),
          `${phrase} in "${timeZone}"`,
        ).toBeNull();
      }
    }
  });
});
