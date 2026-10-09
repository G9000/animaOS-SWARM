import {
  DaemonHttpError,
  type UsageGroup,
  type UsageTotals,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from './approvals';

/** The ranges the Usage page offers, in days (spec §15.4). */
export const USAGE_RANGES_DAYS = [7, 30, 90] as const;
export type UsageRangeDays = (typeof USAGE_RANGES_DAYS)[number];
export const DEFAULT_USAGE_RANGE_DAYS: UsageRangeDays = 30;

/** A cost the daemon could not price. */
export const UNPRICED = '—';

export const USAGE_EMPTY = 'No model calls in this range yet.';
export const USAGE_TOO_OLD = 'Update the daemon to see usage.';
export const USAGE_UNPRICED_NOTE =
  'Some calls have no known price, so their cost is not counted.';
export const USAGE_SUBSCRIPTION_NOTE = (count: number) =>
  `${count} call${count === 1 ? '' : 's'} on your ChatGPT subscription have no per-token cost.`;
export const USAGE_EXPORT_FAILED = 'Couldn’t export the usage. Try again.';
export const USAGE_TRUNCATED_NOTE =
  'This range is too large to total completely; choose a shorter one.';

const DAY_MS = 86_400_000;

export interface UsageRange {
  from: number;
  to: number;
  tzOffsetMinutes: number;
}

/** `days` whole local days ending with today: `to` is the start of the next
 *  local day, `from` is `days` local days earlier. Days are cut at the
 *  browser's current UTC offset (the plan's decision). */
export function usageRange(days: number, now: Date): UsageRange {
  const to = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  const from = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() + 1 - days,
  );
  return {
    from: from.getTime(),
    to: to.getTime(),
    tzOffsetMinutes: -now.getTimezoneOffset(),
  };
}

function dayNumber(ms: number, tzOffsetMinutes: number): number {
  return Math.floor((ms + tzOffsetMinutes * 60_000) / DAY_MS);
}

function dayKey(day: number): string {
  return new Date(day * DAY_MS).toISOString().slice(0, 10);
}

const ZERO_TOTALS: UsageTotals = {
  calls: 0,
  promptTokens: 0,
  completionTokens: 0,
  cachedPromptTokens: 0,
  reasoningTokens: 0,
  totalTokens: 0,
  costMicros: 0,
  unpricedCalls: 0,
  subscriptionCalls: 0,
};

/** Every day of the range, ascending, with zero totals for the days the
 *  daemon sent nothing for. */
export function fillDays(
  groups: readonly UsageGroup[],
  range: UsageRange,
): UsageGroup[] {
  const byKey = new Map(groups.map((group) => [group.key, group.totals]));
  const first = dayNumber(range.from, range.tzOffsetMinutes);
  const last = dayNumber(range.to - 1, range.tzOffsetMinutes);
  const days: UsageGroup[] = [];
  for (let day = first; day <= last; day += 1) {
    const key = dayKey(day);
    days.push({ key, totals: byKey.get(key) ?? ZERO_TOTALS });
  }
  return days;
}

const COUNT_UNITS = [
  { size: 1_000, suffix: 'k' },
  { size: 1_000_000, suffix: 'M' },
  { size: 1_000_000_000, suffix: 'B' },
] as const;

/** 999 → '999', 1234 → '1.2k', 3_400_000 → '3.4M'. */
export function formatTokens(count: number): string {
  if (count < 1_000) return String(count);
  let index = 0;
  while (
    index < COUNT_UNITS.length - 1 &&
    count >= COUNT_UNITS[index + 1].size
  ) {
    index += 1;
  }
  let scaled = Math.round((count / COUNT_UNITS[index].size) * 10) / 10;
  // 999_950 rounds up to the next unit instead of reading '1000k'.
  if (scaled >= 1_000 && index < COUNT_UNITS.length - 1) {
    index += 1;
    scaled = Math.round((count / COUNT_UNITS[index].size) * 10) / 10;
  }
  return `${scaled.toFixed(1).replace(/\.0$/, '')}${COUNT_UNITS[index].suffix}`;
}

/** Micro-USD as dollars: sub-cent amounts keep four decimals. */
export function formatCost(micros: number | null): string {
  if (micros === null) return UNPRICED;
  if (micros === 0) return '$0.00';
  const dollars = micros / 1_000_000;
  if (micros < 10_000) return `$${dollars.toFixed(4)}`;
  return `$${dollars.toLocaleString('en-US', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

export function costNote(totals: UsageTotals): string | null {
  return totals.unpricedCalls > 0 ? USAGE_UNPRICED_NOTE : null;
}

export function subscriptionNote(totals: UsageTotals): string | null {
  return totals.subscriptionCalls > 0
    ? USAGE_SUBSCRIPTION_NOTE(totals.subscriptionCalls)
    : null;
}

const SOURCE_LABELS: Record<string, string> = {
  chat: 'Chat',
  telegram: 'Telegram',
  automation: 'Automation',
  job: 'Job',
  helper: 'Helper',
  api: 'API',
  title: 'Titles',
  compaction: 'Compaction',
  profile: 'Profile',
  agency: 'Agency',
};

export function sourceLabel(source: string): string {
  return SOURCE_LABELS[source] ?? source;
}

/** A cost cell: the priced total, with a mark when some calls were not
 *  priced. */
export function costCell(totals: UsageTotals): string {
  const cost = formatCost(totals.costMicros);
  return totals.unpricedCalls > 0 ? `${cost} + unpriced` : cost;
}

/** The session header's line, e.g. '12.3k tokens · $0.04'; null when the
 *  session made no calls. */
export function sessionUsageLine(
  totals: UsageTotals | null | undefined,
): string | null {
  if (!totals || totals.calls === 0) return null;
  const nothingPriced = totals.costMicros === 0 && totals.unpricedCalls > 0;
  const cost = nothingPriced ? UNPRICED : formatCost(totals.costMicros);
  return `${formatTokens(totals.totalTokens)} tokens · ${cost}`;
}

export function usageErrorMessage(error: unknown): {
  message: string;
  status: number | null;
} {
  if (error instanceof DaemonHttpError) {
    return {
      message: error.status === 404 ? USAGE_TOO_OLD : error.message,
      status: error.status,
    };
  }
  return { message: COMPANION_UNREACHABLE, status: null };
}

function localDate(ms: number): string {
  const date = new Date(ms);
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${date.getFullYear()}-${month}-${day}`;
}

/** 'anima-usage-<first day>-to-<last day>.csv' in local dates. */
export function usageCsvFilename(range: { from: number; to: number }): string {
  return `anima-usage-${localDate(range.from)}-to-${localDate(range.to - 1)}.csv`;
}
