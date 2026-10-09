import type { DaemonClient } from './client.js';

export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';
export const LOG_LEVELS: readonly LogLevel[] = [
  'error',
  'warn',
  'info',
  'debug',
  'trace',
];

/** Untrusted text: the daemon redacts secrets, but a line can still hold
 *  text a model or provider wrote. Show it as text, never as markup. */
export interface LogLine {
  seq: number;
  /** Epoch ms. */
  at: number;
  level: LogLevel;
  target: string;
  message: string;
}

export interface LogsQuery {
  /** The lowest level to show. */
  level?: LogLevel;
  /** Case-insensitive text in the message or target. */
  q?: string;
  /** Only lines with a higher seq. */
  after?: number;
  limit?: number;
}

export interface LogsPage {
  /** Oldest first. */
  lines: LogLine[];
  newestSeq: number;
}

export type LogEvent =
  | { kind: 'line'; line: LogLine }
  | { kind: 'resync'; newestSeq: number };

export interface LogStreamOptions {
  level?: LogLevel;
  q?: string;
  after?: number;
  signal?: AbortSignal;
}

/** The daemon's limit (spec §16). */
export const MAX_LOGS_LIMIT = 1_000;

function queryString(entries: [string, string | number | undefined][]): string {
  const search = new URLSearchParams();
  for (const [key, value] of entries) {
    if (value !== undefined && value !== '') search.set(key, String(value));
  }
  const text = search.toString();
  return text ? `?${text}` : '';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object';
}

export class LogsClient {
  constructor(private readonly client: DaemonClient) {}

  async list(query: LogsQuery = {}): Promise<LogsPage> {
    return this.client.requestJson<LogsPage>(
      `/api/logs${queryString([
        ['level', query.level],
        ['q', query.q],
        ['after', query.after],
        ['limit', query.limit],
      ])}`,
    );
  }

  /** Ends when the connection closes; reconnecting is the caller's choice.
   *  `resync` means refetch the list after your newest seq. */
  async *stream(options: LogStreamOptions = {}): AsyncGenerator<LogEvent> {
    for await (const event of this.client.subscribe<unknown>(
      `/api/logs/stream${queryString([
        ['level', options.level],
        ['q', options.q],
        ['after', options.after],
      ])}`,
      { signal: options.signal },
    )) {
      const data = event.data;
      if (event.event === 'log' && isRecord(data) && 'seq' in data) {
        yield { kind: 'line', line: data as unknown as LogLine };
        continue;
      }
      if (
        event.event === 'resync' &&
        isRecord(data) &&
        typeof data.newestSeq === 'number'
      ) {
        yield { kind: 'resync', newestSeq: data.newestSeq };
        continue;
      }
      // Skipped, but said, so a daemon and console that disagree about the
      // wire show it.
      console.warn('Skipped a malformed log event', {
        event: event.event,
        id: event.id,
        data: typeof data === 'string' ? data.slice(0, 200) : data,
      });
    }
  }
}
