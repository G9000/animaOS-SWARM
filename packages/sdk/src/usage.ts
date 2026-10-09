import type { DaemonClient } from './client.js';

export type UsageSource =
  | 'chat'
  | 'telegram'
  | 'automation'
  | 'job'
  | 'helper'
  | 'api'
  | 'title'
  | 'compaction'
  | 'profile'
  | 'agency';
export type PricingSource =
  | 'table'
  | 'override'
  | 'free'
  | 'subscription'
  | 'unknown';

/** One model call (spec §11). Money is micro-USD; times are epoch ms. */
export interface UsageRecord {
  id: string;
  agentId: string;
  sessionId: string | null;
  runId: string | null;
  source: UsageSource;
  provider: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  cachedPromptTokens: number;
  reasoningTokens: number;
  totalTokens: number;
  /** Null when the call could not be priced. */
  costMicros: number | null;
  pricingSource: PricingSource;
  durationMs: number;
  createdAtMs: number;
}

export interface UsageTotals {
  calls: number;
  promptTokens: number;
  completionTokens: number;
  cachedPromptTokens: number;
  reasoningTokens: number;
  totalTokens: number;
  /** The sum of the priced calls. */
  costMicros: number;
  /** Calls with no cost that are not subscription calls. */
  unpricedCalls: number;
  subscriptionCalls: number;
}

export type UsageGroupBy = 'day' | 'model' | 'source' | 'session';

export interface UsageQuery {
  /** Epoch ms, inclusive; the daemon default is 30 days before `to`. */
  from?: number;
  /** Epoch ms, exclusive; the daemon default is now. */
  to?: number;
  agentId?: string;
  sessionId?: string;
  groupBy?: UsageGroupBy;
  /** -840 to 840; days are cut at this offset. */
  tzOffsetMinutes?: number;
}

export interface UsageGroup {
  key: string;
  totals: UsageTotals;
}

export interface UsageSummary {
  from: number;
  to: number;
  groupBy: UsageGroupBy | null;
  tzOffsetMinutes: number;
  totals: UsageTotals;
  groups: UsageGroup[];
  /** The range held more rows than the daemon sums. */
  truncated: boolean;
}

export interface UsageRecordsQuery {
  from?: number;
  to?: number;
  agentId?: string;
  cursor?: string;
  /** 1 to `MAX_USAGE_RECORDS_LIMIT`; the daemon default is 50. */
  limit?: number;
}

export interface UsageRecordsPage {
  records: UsageRecord[];
  nextCursor: string | null;
}

export interface UsageExportQuery {
  from?: number;
  to?: number;
  agentId?: string;
}

export interface PricingOverride {
  provider: string;
  /** A lowercase prefix; the longest match wins and beats the table. */
  model: string;
  inputMicrosPerMtok: number;
  outputMicrosPerMtok: number;
  cachedInputMicrosPerMtok: number | null;
}

export interface Pricing {
  overrides: PricingOverride[];
  tableDate: string;
}

/** The daemon's limits (spec §16); the SDK validates nothing. */
export const MAX_PRICING_OVERRIDES = 100;
export const MAX_PRICE_MICROS_PER_MTOK = 1_000_000_000_000;
export const MAX_USAGE_RECORDS_LIMIT = 200;

function queryString(
  entries: [string, string | number | undefined | null][],
): string {
  const search = new URLSearchParams();
  for (const [key, value] of entries) {
    if (value !== undefined && value !== null && value !== '') {
      search.set(key, String(value));
    }
  }
  const text = search.toString();
  return text ? `?${text}` : '';
}

// The daemon refuses unknown keys, so bodies carry only the known fields.
function overrideBody(override: PricingOverride): PricingOverride {
  return {
    provider: override.provider,
    model: override.model,
    inputMicrosPerMtok: override.inputMicrosPerMtok,
    outputMicrosPerMtok: override.outputMicrosPerMtok,
    cachedInputMicrosPerMtok: override.cachedInputMicrosPerMtok ?? null,
  };
}

export class UsageClient {
  constructor(private readonly client: DaemonClient) {}

  async summary(query: UsageQuery = {}): Promise<UsageSummary> {
    return this.client.requestJson<UsageSummary>(
      `/api/usage/summary${queryString([
        ['from', query.from],
        ['to', query.to],
        ['agentId', query.agentId],
        ['sessionId', query.sessionId],
        ['groupBy', query.groupBy],
        ['tzOffsetMinutes', query.tzOffsetMinutes],
      ])}`,
    );
  }

  /** Newest first. */
  async records(query: UsageRecordsQuery = {}): Promise<UsageRecordsPage> {
    return this.client.requestJson<UsageRecordsPage>(
      `/api/usage/records${queryString([
        ['from', query.from],
        ['to', query.to],
        ['agentId', query.agentId],
        ['cursor', query.cursor],
        ['limit', query.limit],
      ])}`,
    );
  }

  /** The CSV text; the caller decides how to save it. */
  async exportCsv(query: UsageExportQuery = {}): Promise<string> {
    return this.client.requestText(
      `/api/usage/export.csv${queryString([
        ['from', query.from],
        ['to', query.to],
        ['agentId', query.agentId],
      ])}`,
    );
  }

  async pricing(): Promise<Pricing> {
    return this.client.requestJson<Pricing>('/api/usage/pricing');
  }

  /** Replaces every override. */
  async setPricing(overrides: PricingOverride[]): Promise<Pricing> {
    return this.client.requestJson<Pricing>('/api/usage/pricing', {
      method: 'PUT',
      body: { overrides: overrides.map(overrideBody) },
    });
  }
}
