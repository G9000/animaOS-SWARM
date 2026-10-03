import type { AutomationTrigger } from '@animaOS-SWARM/sdk';

/** Spec §15.4: phrases the Automations page understands. The daemon
 *  previews and stores what they mean; the browser never computes fire
 *  times. */
export const PHRASE_EXAMPLES = [
  'every 2 hours',
  'weekdays at 9am',
  'every monday at 8:30',
  'tomorrow at 15:00',
  'in 20 minutes',
] as const;

export interface ScheduleParseOptions {
  /** Now, in epoch milliseconds: "in 20 minutes", "today", "tomorrow". */
  nowMs: number;
  /** The owner's IANA time zone; wall-clock phrases are read in it. */
  timeZone: string;
}

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

const UNITS: Record<string, number> = {
  minute: MINUTE_MS,
  minutes: MINUTE_MS,
  min: MINUTE_MS,
  mins: MINUTE_MS,
  hour: HOUR_MS,
  hours: HOUR_MS,
  hr: HOUR_MS,
  hrs: HOUR_MS,
  day: DAY_MS,
  days: DAY_MS,
};

const WEEKDAYS: Record<string, number> = {
  sunday: 0,
  sun: 0,
  monday: 1,
  mon: 1,
  tuesday: 2,
  tue: 2,
  tues: 2,
  wednesday: 3,
  wed: 3,
  thursday: 4,
  thu: 4,
  thur: 4,
  thurs: 4,
  friday: 5,
  fri: 5,
  saturday: 6,
  sat: 6,
};

/** `9`, `9am`, `9:30 pm`, `15:00`, `noon`, or `midnight`. */
export function parseClock(
  text: string,
): { hour: number; minute: number } | null {
  const clock = text.trim().toLowerCase();
  if (clock === 'noon') return { hour: 12, minute: 0 };
  if (clock === 'midnight') return { hour: 0, minute: 0 };
  const match = /^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$/.exec(clock);
  if (!match) return null;
  let hour = Number(match[1]);
  const minute = match[2] === undefined ? 0 : Number(match[2]);
  if (minute > 59) return null;
  if (match[3]) {
    if (hour < 1 || hour > 12) return null;
    hour = (hour % 12) + (match[3] === 'pm' ? 12 : 0);
  } else if (hour > 23) {
    return null;
  }
  return { hour, minute };
}

interface WallClock {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
}

/** The wall-clock reading of `ms` in `timeZone`. */
function wallClock(ms: number, timeZone: string): WallClock {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone,
    hourCycle: 'h23',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).formatToParts(new Date(ms));
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    Number(parts.find((item) => item.type === type)?.value ?? 0);
  return {
    year: part('year'),
    month: part('month'),
    day: part('day'),
    hour: part('hour') % 24,
    minute: part('minute'),
    second: part('second'),
  };
}

/** The instant of a wall-clock time in `timeZone` (month 1–12). */
export function zonedTimeToUtc(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute: number,
  timeZone: string,
): number {
  const wall = Date.UTC(year, month - 1, day, hour, minute);
  const offset = (ms: number) => {
    const clock = wallClock(ms, timeZone);
    return (
      Date.UTC(
        clock.year,
        clock.month - 1,
        clock.day,
        clock.hour,
        clock.minute,
        clock.second,
      ) - ms
    );
  };
  const first = wall - offset(wall);
  return wall - offset(first);
}

/** The instant of `clock` on the day `days` after today in `timeZone`. */
function onDay(
  options: ScheduleParseOptions,
  days: number,
  clock: { hour: number; minute: number },
): number {
  const today = wallClock(options.nowMs, options.timeZone);
  const date = new Date(
    Date.UTC(today.year, today.month - 1, today.day + days),
  );
  return zonedTimeToUtc(
    date.getUTCFullYear(),
    date.getUTCMonth() + 1,
    date.getUTCDate(),
    clock.hour,
    clock.minute,
    options.timeZone,
  );
}

/** Day names ("friday", "tue, fri"). Without `every`, each must be plural
 *  ("fridays"), since a bare day name could mean just the next one. */
function weekdays(text: string, pluralOnly: boolean): number[] | null {
  const raw = text
    .split(/\s*(?:,|\band\b)\s*/)
    .map((name) => name.trim())
    .filter(Boolean);
  if (raw.length === 0) return null;
  if (pluralOnly && raw.some((name) => !name.endsWith('s'))) return null;
  const days = raw.map((name) => WEEKDAYS[name.replace(/s$/, '')]);
  if (days.some((day) => day === undefined)) return null;
  return [...new Set(days as number[])].sort((left, right) => left - right);
}

function cron(
  clock: { hour: number; minute: number },
  days: string,
  timeZone: string,
): AutomationTrigger {
  return {
    type: 'cron',
    expression: `${clock.minute} ${clock.hour} * * ${days}`,
    timeZone,
  };
}

/** True when `timeZone` is a zone the runtime knows. An empty or half-typed
 *  name ("Europe/Lon") is not, and `Intl.DateTimeFormat` throws for it. */
function knownTimeZone(timeZone: string): boolean {
  try {
    new Intl.DateTimeFormat('en-US', { timeZone });
    return true;
  } catch {
    return false;
  }
}

/** What a plain-language phrase means, or null (spec §15.4). */
export function parseSchedule(
  text: string,
  options: ScheduleParseOptions,
): AutomationTrigger | null {
  const phrase = text.trim().toLowerCase().replace(/\s+/g, ' ');
  if (!phrase) return null;
  const { timeZone } = options;
  if (!knownTimeZone(timeZone)) return null;

  let match = /^(?:every day|daily|each day) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    return clock && { type: 'daily', ...clock, timeZone };
  }
  match = /^(?:every weekday|weekdays) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    return clock && cron(clock, '1-5', timeZone);
  }
  match = /^(?:every weekend|weekends) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    return clock && cron(clock, '0,6', timeZone);
  }
  match = /^(today|tomorrow) at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[2]);
    if (!clock) return null;
    const atMs = onDay(options, match[1] === 'tomorrow' ? 1 : 0, clock);
    return atMs > options.nowMs ? { type: 'once', atMs } : null;
  }
  match = /^at (.+)$/.exec(phrase);
  if (match) {
    const clock = parseClock(match[1]);
    if (!clock) return null;
    const today = onDay(options, 0, clock);
    return {
      type: 'once',
      atMs: today > options.nowMs ? today : onDay(options, 1, clock),
    };
  }
  match = /^in (\d+) ([a-z]+)$/.exec(phrase);
  if (match) {
    const count = Number(match[1]);
    const unit = UNITS[match[2]];
    return count >= 1 && unit
      ? { type: 'once', atMs: options.nowMs + count * unit }
      : null;
  }
  match = /^every (?:(\d+) )?([a-z]+)$/.exec(phrase);
  if (match) {
    const count = match[1] === undefined ? 1 : Number(match[1]);
    const unit = UNITS[match[2]];
    return count >= 1 && unit
      ? { type: 'interval', intervalMs: count * unit }
      : null;
  }
  // `every friday at 9`, or a plural day name (`fridays at 9`). A bare
  // `friday at 9` or `on friday at 9` could mean one day, so it is not read.
  match = /^(?:(every )|on )?([a-z, ]+?) at (.+)$/.exec(phrase);
  if (match) {
    const days = weekdays(match[2], match[1] === undefined);
    const clock = parseClock(match[3]);
    return days && clock ? cron(clock, days.join(','), timeZone) : null;
  }
  return null;
}
