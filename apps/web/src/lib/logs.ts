import {
  DaemonHttpError,
  type LogLevel,
  type LogLine,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from './approvals';

/** Lines the page holds at once; older ones drop (spec §16). */
export const MAX_LOGS_SHOWN = 2_000;
/** Lines read from the tail when the page opens or the filter changes. */
export const LOGS_FETCH_LIMIT = 500;

export const LOG_LEVEL_LABELS: Record<LogLevel, string> = {
  error: 'Error',
  warn: 'Warning',
  info: 'Info',
  debug: 'Debug',
  trace: 'Trace',
};

export const LOGS_EMPTY = 'No log lines match.';
export const LOGS_CONNECTING = 'Connecting…';
export const LOGS_RECONNECTING = 'Reconnecting…';
export const LOGS_PAUSED_NOTE =
  'Paused. New lines are held and will appear when you resume.';
export const LOGS_COPIED = 'Copied';
export const LOGS_COPY_FAILED =
  'Couldn’t copy. Select the lines and copy them instead.';
export const LOGS_TOO_OLD = 'Update the daemon to see its logs.';
export const LOGS_LOADING = 'Loading logs…';

export const LOG_RECONNECT_BASE_MS = 1_000;
export const LOG_RECONNECT_MAX_MS = 30_000;
const LOG_RECONNECT_JITTER = 0.2;

/** Ascending by seq, a repeated seq dropped, the newest `cap` kept. Returns
 *  `existing` itself when nothing changed, so a quiet stream does not render
 *  again. */
export function mergeLines(
  existing: readonly LogLine[],
  incoming: readonly LogLine[],
  cap: number = MAX_LOGS_SHOWN,
): LogLine[] {
  if (incoming.length === 0) return existing as LogLine[];
  const known = new Set(existing.map((line) => line.seq));
  const fresh: LogLine[] = [];
  for (const line of incoming) {
    if (known.has(line.seq)) continue;
    known.add(line.seq);
    fresh.push(line);
  }
  if (fresh.length === 0) return existing as LogLine[];
  const merged = [...existing, ...fresh].sort((a, b) => a.seq - b.seq);
  return merged.length > cap ? merged.slice(merged.length - cap) : merged;
}

const pad = (value: number, width: number) =>
  String(value).padStart(width, '0');

/** `HH:MM:SS.mmm` in the browser's local time. */
export function formatLogTime(ms: number): string {
  const date = new Date(ms);
  return `${pad(date.getHours(), 2)}:${pad(date.getMinutes(), 2)}:${pad(date.getSeconds(), 2)}.${pad(date.getMilliseconds(), 3)}`;
}

/** One line per log line: time, level, target, message. */
export function logsAsText(lines: readonly LogLine[]): string {
  return lines
    .map(
      (line) =>
        `${formatLogTime(line.at)} ${line.level.toUpperCase()} ${line.target} ${line.message}`,
    )
    .join('\n');
}

/** One second doubling to thirty, plus up to 20% jitter from `random` in
 *  [0, 1). `attempt` counts from 0. */
export function nextReconnectDelay(attempt: number, random: number): number {
  const base = Math.min(
    LOG_RECONNECT_BASE_MS * 2 ** Math.max(0, attempt),
    LOG_RECONNECT_MAX_MS,
  );
  return Math.round(base * (1 + LOG_RECONNECT_JITTER * random));
}

/** The newest seq in an ascending list, or 0. */
export function newestSeq(lines: readonly LogLine[]): number {
  return lines.length > 0 ? lines[lines.length - 1].seq : 0;
}

/** What a failed logs read or stream says, and the HTTP status behind it. The
 *  daemon's own message (the 429 for too many streams included) is shown. */
export function logsErrorMessage(error: unknown): {
  message: string;
  status: number | null;
} {
  if (error instanceof DaemonHttpError)
    return {
      message: error.status === 404 ? LOGS_TOO_OLD : error.message,
      status: error.status,
    };
  return { message: COMPANION_UNREACHABLE, status: null };
}
