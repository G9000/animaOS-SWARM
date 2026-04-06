import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { ResultEntry } from '@animaOS-SWARM/tui';

export const LAUNCH_HISTORY_FILENAME = 'anima-history.json';
const MAX_HISTORY_ENTRIES = 100;

function historyPath(dir: string): string {
  return join(dir, LAUNCH_HISTORY_FILENAME);
}

function isResultEntry(value: unknown): value is ResultEntry {
  if (!value || typeof value !== 'object') {
    return false;
  }

  const candidate = value as Partial<ResultEntry>;
  return (
    typeof candidate.id === 'string' &&
    typeof candidate.timestamp === 'number' &&
    typeof candidate.task === 'string' &&
    typeof candidate.result === 'string' &&
    typeof candidate.isError === 'boolean' &&
    (typeof candidate.label === 'undefined' ||
      typeof candidate.label === 'string')
  );
}

export function loadLaunchHistory(dir: string): ResultEntry[] {
  try {
    const parsed = JSON.parse(
      readFileSync(historyPath(dir), 'utf-8')
    ) as unknown;
    if (!Array.isArray(parsed)) {
      return [];
    }

    return parsed.filter(isResultEntry).slice(-MAX_HISTORY_ENTRIES);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      return [];
    }

    return [];
  }
}

export function saveLaunchHistory(dir: string, entries: ResultEntry[]): void {
  writeFileSync(
    historyPath(dir),
    JSON.stringify(entries.slice(-MAX_HISTORY_ENTRIES), null, 2) + '\n'
  );
}

export function appendLaunchHistory(
  dir: string,
  entry: ResultEntry
): ResultEntry[] {
  const nextEntries = [...loadLaunchHistory(dir), entry].slice(
    -MAX_HISTORY_ENTRIES
  );
  saveLaunchHistory(dir, nextEntries);
  return nextEntries;
}

export function clearLaunchHistory(dir: string): void {
  saveLaunchHistory(dir, []);
}
