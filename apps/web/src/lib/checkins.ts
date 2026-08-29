import { daemon, type LegacyScheduleInput } from './daemon-api';

// Browser records are legacy input only. New proactive prompts are persisted
// and executed by anima-daemon, including while this tab is closed.

export const CHECKIN_SENTINEL = 'CHECKIN_OK';

export interface Checkin {
  id: string;
  prompt: string;
  intervalSecs: number;
  createdAtMs: number;
  lastRunAtMs?: number;
  lastOutcome?: 'silent' | 'spoke' | 'error';
  lastReply?: string;
}

const storageKey = (agentId: string) => `animaos.checkins.${agentId}`;

function isFiniteNonNegative(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

function parseLegacy(value: unknown): LegacyScheduleInput | null {
  if (!value || typeof value !== 'object') return null;
  const item = value as Record<string, unknown>;
  if (
    typeof item.id !== 'string' ||
    !item.id ||
    item.id.length > 256 ||
    typeof item.prompt !== 'string' ||
    !item.prompt.trim() ||
    typeof item.intervalSecs !== 'number' ||
    !Number.isFinite(item.intervalSecs) ||
    item.intervalSecs <= 0 ||
    !isFiniteNonNegative(item.createdAtMs) ||
    (item.lastRunAtMs !== undefined && !isFiniteNonNegative(item.lastRunAtMs))
  )
    return null;
  return {
    id: item.id,
    prompt: item.prompt,
    intervalSecs: item.intervalSecs,
    createdAtMs: item.createdAtMs,
    ...(item.lastRunAtMs === undefined
      ? {}
      : { lastRunAtMs: item.lastRunAtMs as number }),
  };
}

export function readLegacyCheckins(agentId: string): {
  valid: LegacyScheduleInput[];
  malformedCount: number;
  exists: boolean;
} {
  const raw = localStorage.getItem(storageKey(agentId));
  if (raw === null) return { valid: [], malformedCount: 0, exists: false };
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed))
      return { valid: [], malformedCount: 1, exists: true };
    const projected = parsed.map(parseLegacy);
    return {
      valid: projected.filter(
        (item): item is LegacyScheduleInput => item !== null,
      ),
      malformedCount: projected.filter((item) => item === null).length,
      exists: true,
    };
  } catch {
    return { valid: [], malformedCount: 1, exists: true };
  }
}

export function legacyImportKey(agentId: string, recordId: string): string {
  return `legacy:${agentId}:${recordId}`;
}

export async function importLegacyCheckins(agentId: string): Promise<{
  imported: number;
  malformed: number;
  complete: boolean;
}> {
  const records = readLegacyCheckins(agentId);
  if (!records.exists) return { imported: 0, malformed: 0, complete: true };
  if (records.valid.length > 0) {
    await daemon.importLegacySchedules(agentId, { schedules: records.valid });
  }
  if (records.malformedCount > 0) {
    return {
      imported: records.valid.length,
      malformed: records.malformedCount,
      complete: false,
    };
  }
  localStorage.removeItem(storageKey(agentId));
  return { imported: records.valid.length, malformed: 0, complete: true };
}

export function loadCheckins(agentId: string): Checkin[] {
  try {
    const raw = localStorage.getItem(storageKey(agentId));
    if (!raw) return [];
    const parsed = JSON.parse(raw) as Checkin[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

export function saveCheckins(agentId: string, checkins: Checkin[]): void {
  localStorage.setItem(storageKey(agentId), JSON.stringify(checkins));
}

export function clearCheckins(agentId: string): void {
  localStorage.removeItem(storageKey(agentId));
}

export function newCheckin(prompt: string, intervalSecs: number): Checkin {
  return {
    id: `ci-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    prompt,
    intervalSecs,
    createdAtMs: Date.now(),
  };
}

export function isDue(c: Checkin, nowMs: number): boolean {
  const base = c.lastRunAtMs ?? c.createdAtMs;
  return nowMs - base >= c.intervalSecs * 1000;
}

/** Prompt actually sent to the agent: original intent + silence convention. */
export function wrapPrompt(c: Checkin): string {
  return `${c.prompt}\n\n(This is a scheduled check-in. If you have nothing worth saying right now, reply with exactly ${CHECKIN_SENTINEL} and nothing else.)`;
}

export function formatInterval(intervalSecs: number): string {
  if (intervalSecs < 60) return `${Math.round(intervalSecs)}s`;
  const min = Math.round(intervalSecs / 60);
  if (min < 60) return `${min}m`;
  const h = Math.round(min / 60);
  if (h < 24) return `${h}h`;
  return `${Math.round(h / 24)}d`;
}

export function formatRelative(ms: number): string {
  const diff = Math.max(0, Date.now() - ms);
  const min = Math.floor(diff / 60000);
  if (min < 1) return 'just now';
  if (min < 60) return `${min}m ago`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}
