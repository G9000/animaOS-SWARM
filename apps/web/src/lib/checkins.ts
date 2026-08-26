// Check-ins: proactive recurring prompts for the single agent.
// The daemon has no scheduler, so check-ins are stored locally (per agent)
// and fired from this tab through POST /api/agents/:id/run. The user message
// is tagged via metadata { kind: 'checkin' } so chat can hide the mechanics;
// a reply of exactly CHECKIN_OK marks a silent tick.

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
