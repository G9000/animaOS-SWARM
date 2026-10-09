import { DaemonHttpError, type DaemonClient } from './client.js';

export interface StatusReadiness {
  status: 'ready' | 'not_ready';
  issues: string[];
}

export interface StatusHistory {
  /** `memory`, `sqlite`, or `postgres`. */
  store: string;
  ephemeral: boolean;
  /** False while the store's writes are failing. */
  healthy: boolean;
  pendingFlush: number;
  usageQueued: number;
  /** Redacted, at most 500 characters. */
  lastError: string | null;
  failingSinceMs: number | null;
  flushErrors: number;
}

export interface StatusStorage {
  persistenceMode: string;
  /** `json`, `postgres`, or `memory`. */
  controlPlane: string;
  controlPlaneDurability: string;
  history: StatusHistory;
}

export interface StatusProvider {
  id: string;
  label: string;
  configured: boolean;
}

export interface StatusConnector {
  id: string;
  agentId: string;
  type: string;
  status: string;
  enabled: boolean;
}

export interface StatusAutomations {
  total: number;
  enabled: number;
  /** Automations whose latest runs failed in a row. */
  failing: number;
  failuresTotal: number;
}

export interface StatusRuns {
  running: number;
  queued: number;
  /** Runs the daemon still holds, which prunes old finished runs. */
  byStatus: Record<string, number>;
}

export interface StatusLimits {
  maxRequestBytes: number;
  maxConcurrentRuns: number;
  maxRunsPerAgent: number;
  queuedRunsPerAgent: number;
  maxBackgroundProcesses: number;
  logBufferLines: number;
  eventBuffer: number;
}

/** The daemon's health aggregate (spec §11.3). */
export interface DaemonStatus {
  version: string;
  buildRevision: string | null;
  startedAtMs: number;
  nowMs: number;
  uptimeSeconds: number;
  readiness: StatusReadiness;
  storage: StatusStorage;
  providers: StatusProvider[];
  /** At most 50; never a credential. */
  connectors: StatusConnector[];
  automations: StatusAutomations;
  approvals: { pending: number };
  runs: StatusRuns;
  events: { subscribers: number; laggedEvents: number };
  logs: { buffered: number; newestSeq: number };
  limits: StatusLimits;
}

export const STATUS_TOO_OLD = 'Update the daemon to see its health.';

/** The daemon has no `/api/status` yet (spec §13.4). `code` is a stable
 *  string a caller can check without importing this class. */
export class StatusTooOldError extends Error {
  readonly code = 'daemon_too_old' as const;

  constructor() {
    super(STATUS_TOO_OLD);
    this.name = 'StatusTooOldError';
  }
}

export class StatusClient {
  constructor(private readonly client: DaemonClient) {}

  async get(): Promise<DaemonStatus> {
    try {
      return await this.client.requestJson<DaemonStatus>('/api/status');
    } catch (error) {
      if (error instanceof DaemonHttpError && error.status === 404) {
        throw new StatusTooOldError();
      }
      throw error;
    }
  }
}
