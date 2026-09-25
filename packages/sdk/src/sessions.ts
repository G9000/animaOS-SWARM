import { DaemonHttpError } from './client.js';
import type { DaemonClient } from './client.js';

export type SessionKind = 'chat' | 'telegram' | 'checkin' | 'job' | 'helper';
export type SessionOrigin =
  | 'web'
  | 'api'
  | 'telegram'
  | 'schedule'
  | 'job'
  | 'delegation'
  | 'peer';
export type SessionTitleSource =
  | 'first_message'
  | 'generated'
  | 'owner'
  | 'system';

export interface SessionCapabilities {
  send: boolean;
  steer: boolean;
  stop: boolean;
  rename: boolean;
  archive: boolean;
  delete: boolean;
  compact: boolean;
  export: boolean;
}

export interface SessionSummary {
  text: string;
  throughMessageId: string;
  createdAtMs: number;
  sourceMessageCount: number;
}

export interface SessionContextTrimmed {
  droppedThroughMessageId: string;
  atMs: number;
}

/** Why a search result matched; `messageId` is null for a title match. */
export interface SessionMatch {
  messageId: string | null;
  snippet: string;
}

/** A session with the daemon's derived fields (spec §3.2). */
export interface Session {
  id: string;
  agentId: string;
  /** The transcript room; equals `id` except for mapped legacy rooms. */
  roomId: string;
  kind: SessionKind;
  origin: SessionOrigin;
  title: string;
  titleSource: SessionTitleSource;
  createdAtMs: number;
  lastActivityAtMs: number;
  lastReadAtMs: number | null;
  archived: boolean;
  parentSessionId: string | null;
  parentRunId: string | null;
  parentAgentId: string | null;
  summary: SessionSummary | null;
  contextTrimmed: SessionContextTrimmed | null;
  messageCount: number;
  preview: string | null;
  activeRuns: number;
  pendingApprovals: number;
  unread: boolean;
  capabilities: SessionCapabilities;
  match?: SessionMatch;
}

export interface SessionPage {
  sessions: Session[];
  nextCursor: string | null;
}

export interface SessionListOptions {
  kind?: SessionKind;
  /** true lists only archived sessions; the daemon default is false. */
  archived?: boolean;
  q?: string;
  cursor?: string;
  /** 1–200; the daemon default is 50. */
  limit?: number;
  /** The daemon default is true. */
  includeHelpers?: boolean;
  signal?: AbortSignal;
}

export interface SessionMessageAttachment {
  type: 'file' | 'image' | 'url';
  name: string;
}

export interface SessionMessage {
  id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  text: string;
  attachments: SessionMessageAttachment[];
  metadata: Record<string, unknown>;
  createdAtMs: number;
  /** Present on silent check-in messages when `includeHidden` is set. */
  hidden?: boolean;
}

export interface SessionMessagePage {
  /** Oldest to newest within the page. */
  messages: SessionMessage[];
  /**
   * The `before` cursor for the next (older) page, or null once history is
   * exhausted. A page degraded by an unreadable history store returns a
   * cursor here instead of null even when the hot tail itself is exhausted;
   * requesting that cursor then fails with a 503 rather than the page
   * quietly claiming there is nothing more.
   */
  nextBefore: string | null;
}

export interface SessionMessageOptions {
  before?: string;
  limit?: number;
  includeHidden?: boolean;
  signal?: AbortSignal;
}

export interface SessionUpdateInput {
  title?: string;
  archived?: boolean;
  lastReadAtMs?: number;
}

/**
 * Thrown only by `SessionsClient.list`. The daemon answers an unknown route
 * with the same JSON 404 as a real not-found, so a 404 from `GET
 * /api/agents/{id}/sessions` alone cannot tell "the daemon predates the
 * sessions routes" apart from "the agent does not exist" (Controller ruling
 * 2, M2 pre-flight audit; spec §13.4). `list` resolves the ambiguity by
 * probing `GET /api/agents/{id}`: if the agent exists, the sessions route is
 * the thing missing, so it raises this instead of the plain `DaemonHttpError`.
 * `code` is a stable string a caller can check without importing this class.
 */
export class DaemonTooOldError extends Error {
  readonly code = 'daemon_too_old' as const;

  constructor(readonly agentId: string) {
    super(
      `The daemon does not support sessions for agent "${agentId}" yet; update the daemon.`,
    );
    this.name = 'DaemonTooOldError';
  }
}

export class SessionsClient {
  constructor(private readonly client: DaemonClient) {}

  async list(
    agentId: string,
    options: SessionListOptions = {},
  ): Promise<SessionPage> {
    const search = new URLSearchParams();
    if (options.kind) search.set('kind', options.kind);
    if (options.archived !== undefined)
      search.set('archived', String(options.archived));
    if (options.q) search.set('q', options.q);
    if (options.cursor) search.set('cursor', options.cursor);
    if (options.limit !== undefined) search.set('limit', String(options.limit));
    if (options.includeHelpers !== undefined)
      search.set('includeHelpers', String(options.includeHelpers));
    try {
      return await this.client.requestJson<SessionPage>(
        withQuery(sessionsPath(agentId), search),
        { signal: options.signal },
      );
    } catch (error) {
      if (error instanceof DaemonHttpError && error.status === 404) {
        if (await this.agentExists(agentId)) {
          throw new DaemonTooOldError(agentId);
        }
      }
      throw error;
    }
  }

  /** Whether `GET /api/agents/{agentId}` answers 200 (Ruling 2, M2 pre-flight audit). */
  private async agentExists(agentId: string): Promise<boolean> {
    try {
      await this.client.agents.get(agentId);
      return true;
    } catch {
      return false;
    }
  }

  async get(
    agentId: string,
    sessionId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<Session> {
    const response = await this.client.requestJson<{ session: Session }>(
      sessionPath(agentId, sessionId),
      { signal: options.signal },
    );
    return response.session;
  }

  async messages(
    agentId: string,
    sessionId: string,
    options: SessionMessageOptions = {},
  ): Promise<SessionMessagePage> {
    const search = new URLSearchParams();
    if (options.before) search.set('before', options.before);
    if (options.limit !== undefined) search.set('limit', String(options.limit));
    if (options.includeHidden !== undefined)
      search.set('includeHidden', String(options.includeHidden));
    return this.client.requestJson<SessionMessagePage>(
      withQuery(`${sessionPath(agentId, sessionId)}/messages`, search),
      { signal: options.signal },
    );
  }

  async create(
    agentId: string,
    input: { title?: string } = {},
  ): Promise<Session> {
    const response = await this.client.requestJson<{ session: Session }>(
      sessionsPath(agentId),
      { method: 'POST', body: input },
    );
    return response.session;
  }

  async update(
    agentId: string,
    sessionId: string,
    patch: SessionUpdateInput,
  ): Promise<Session> {
    const response = await this.client.requestJson<{ session: Session }>(
      sessionPath(agentId, sessionId),
      { method: 'PATCH', body: patch },
    );
    return response.session;
  }

  async remove(agentId: string, sessionId: string): Promise<void> {
    await this.client.requestJson<{ deleted: boolean }>(
      sessionPath(agentId, sessionId),
      { method: 'DELETE' },
    );
  }

  /** The full transcript as Markdown, including archived history and
   *  messages kept only in the history store, with silent check-in turns
   *  marked rather than hidden. */
  async exportMarkdown(
    agentId: string,
    sessionId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<string> {
    return this.client.requestText(
      `${sessionPath(agentId, sessionId)}/export`,
      {
        signal: options.signal,
      },
    );
  }
}

function sessionsPath(agentId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}/sessions`;
}

function sessionPath(agentId: string, sessionId: string): string {
  return `${sessionsPath(agentId)}/${encodeURIComponent(sessionId)}`;
}

function withQuery(path: string, search: URLSearchParams): string {
  const query = search.toString();
  return query ? `${path}?${query}` : path;
}
