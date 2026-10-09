import type { UsageGroup, UsageSummary, UsageTotals } from '@animaOS-SWARM/sdk';

export function totalsFixture(
  overrides: Partial<UsageTotals> = {},
): UsageTotals {
  return {
    calls: 0,
    promptTokens: 0,
    completionTokens: 0,
    cachedPromptTokens: 0,
    reasoningTokens: 0,
    totalTokens: 0,
    costMicros: 0,
    unpricedCalls: 0,
    subscriptionCalls: 0,
    ...overrides,
  };
}

export function groupFixture(
  key: string,
  overrides: Partial<UsageTotals> = {},
): UsageGroup {
  return { key, totals: totalsFixture(overrides) };
}

export function summaryFixture(
  overrides: Partial<UsageSummary> = {},
): UsageSummary {
  return {
    from: 0,
    to: 0,
    groupBy: null,
    tzOffsetMinutes: 0,
    totals: totalsFixture(),
    groups: [],
    truncated: false,
    ...overrides,
  };
}
