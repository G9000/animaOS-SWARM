import type { Session, SessionKind } from '@animaOS-SWARM/sdk';

export const SESSION_KIND_ORDER: readonly SessionKind[] = [
  'chat',
  'telegram',
  'checkin',
  'job',
  'helper',
];

export const SESSION_KIND_LABELS: Record<SessionKind, string> = {
  chat: 'Chat',
  telegram: 'Telegram',
  checkin: 'Check-in',
  job: 'Job',
  helper: 'Helper',
};

export const SESSION_KIND_FILTER_LABELS: Record<SessionKind, string> = {
  chat: 'Chats',
  telegram: 'Telegram',
  checkin: 'Check-ins',
  job: 'Jobs',
  helper: 'Helpers',
};

export type SessionGroupLabel = 'Today' | 'Yesterday' | 'Previous 7 days' | 'Older';

export interface SessionNode {
  session: Session;
  helpers: Session[];
}

export interface SessionGroup {
  label: SessionGroupLabel;
  nodes: SessionNode[];
}

const DAY_MS = 24 * 60 * 60 * 1000;
const GROUP_ORDER: readonly SessionGroupLabel[] = [
  'Today',
  'Yesterday',
  'Previous 7 days',
  'Older',
];

/** Sessions are keyed by agent and id; helper sessions belong to other agents. */
export function sessionKey(session: Pick<Session, 'agentId' | 'id'>): string {
  return `${session.agentId}\u0000${session.id}`;
}

/** The kinds present, in sidebar order (spec §15.1: only kinds that exist). */
export function presentKinds(sessions: readonly Session[]): SessionKind[] {
  const kinds = new Set(sessions.map((session) => session.kind));
  return SESSION_KIND_ORDER.filter((kind) => kinds.has(kind));
}

function groupLabel(activityMs: number, now: Date): SessionGroupLabel {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (activityMs >= today) return 'Today';
  if (activityMs >= today - DAY_MS) return 'Yesterday';
  if (activityMs >= today - 7 * DAY_MS) return 'Previous 7 days';
  return 'Older';
}

/**
 * Top-level sessions by last activity (local days). With `nestHelpers`, a
 * helper session sits under the listed session it came from; one whose parent
 * is not listed stays at the top level.
 */
export function groupSessions(
  sessions: readonly Session[],
  now: Date = new Date(),
  nestHelpers = true,
): SessionGroup[] {
  const listed = new Map(sessions.map((session) => [sessionKey(session), session]));
  const children = new Map<string, Session[]>();
  const roots: Session[] = [];
  for (const session of sessions) {
    const parentKey =
      nestHelpers &&
      session.kind === 'helper' &&
      session.parentAgentId &&
      session.parentSessionId
        ? sessionKey({ agentId: session.parentAgentId, id: session.parentSessionId })
        : null;
    const parent = parentKey ? listed.get(parentKey) : undefined;
    if (parentKey && parent && parent.kind !== 'helper') {
      children.set(parentKey, [...(children.get(parentKey) ?? []), session]);
    } else {
      roots.push(session);
    }
  }
  const groups = new Map<SessionGroupLabel, SessionNode[]>();
  for (const session of roots) {
    const label = groupLabel(session.lastActivityAtMs, now);
    groups.set(label, [
      ...(groups.get(label) ?? []),
      { session, helpers: children.get(sessionKey(session)) ?? [] },
    ]);
  }
  return GROUP_ORDER.filter((label) => groups.has(label)).map((label) => ({
    label,
    nodes: groups.get(label) ?? [],
  }));
}

/** A download name from a title, matching the daemon's export file name. */
export function exportFileName(title: string): string {
  const stem = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+/, '')
    .slice(0, 60)
    .replace(/-+$/, '');
  return `${stem || 'session'}.md`;
}
