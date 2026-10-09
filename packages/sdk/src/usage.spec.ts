import { describe, expect, it } from 'vitest';

import {
  createDaemonClient,
  DaemonHttpError,
  MAX_PRICE_MICROS_PER_MTOK,
  MAX_PRICING_OVERRIDES,
  MAX_USAGE_RECORDS_LIMIT,
  type PricingOverride,
  type UsageSummary,
  type UsageTotals,
} from './index.js';

function transport(respond: (url: string, init?: RequestInit) => Response) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return respond(String(url), init);
    },
  });
  return { usage: client.usage, requests };
}

const totals: UsageTotals = {
  calls: 3,
  promptTokens: 100,
  completionTokens: 50,
  cachedPromptTokens: 10,
  reasoningTokens: 5,
  totalTokens: 150,
  costMicros: 1234,
  unpricedCalls: 1,
  subscriptionCalls: 0,
};

const override: PricingOverride = {
  provider: 'openai',
  model: 'gpt-5',
  inputMicrosPerMtok: 1_000_000,
  outputMicrosPerMtok: 4_000_000,
  cachedInputMicrosPerMtok: null,
};

describe('usage client', () => {
  it('summary sends only the options that are set', async () => {
    const { usage, requests } = transport(() => Response.json({}));

    await usage.summary();
    await usage.summary({
      from: 0,
      to: 10,
      agentId: 'agent/a',
      groupBy: 'day',
      tzOffsetMinutes: -60,
    });

    expect(requests.map(({ url }) => url)).toEqual([
      '/api/usage/summary',
      '/api/usage/summary?from=0&to=10&agentId=agent%2Fa&groupBy=day&tzOffsetMinutes=-60',
    ]);
  });

  it('summary parses totals and groups', async () => {
    const summary: UsageSummary = {
      from: 1,
      to: 2,
      groupBy: 'model',
      tzOffsetMinutes: 0,
      totals,
      groups: [{ key: 'openai/gpt-5', totals }],
      truncated: false,
    };
    const { usage } = transport(() => Response.json(summary));

    expect(await usage.summary({ groupBy: 'model' })).toEqual(summary);
  });

  it('records sends the cursor and limit', async () => {
    const page = { records: [], nextCursor: 'n2' };
    const { usage, requests } = transport(() => Response.json(page));

    expect(
      await usage.records({ from: 5, agentId: 'a', cursor: 'n1', limit: 20 }),
    ).toEqual(page);
    expect(requests[0].url).toBe(
      '/api/usage/records?from=5&agentId=a&cursor=n1&limit=20',
    );
  });

  it('exportCsv returns the text and sends the range', async () => {
    const csv = 'id,createdAt\nu1,2026-01-01\n';
    const { usage, requests } = transport(
      () => new Response(csv, { headers: { 'content-type': 'text/csv' } }),
    );

    expect(await usage.exportCsv({ from: 1, to: 9, agentId: 'a' })).toBe(csv);
    expect(requests[0].url).toBe('/api/usage/export.csv?from=1&to=9&agentId=a');
  });

  it('pricing reads the overrides and the table date', async () => {
    const pricing = { overrides: [override], tableDate: '2026-09-01' };
    const { usage, requests } = transport(() => Response.json(pricing));

    expect(await usage.pricing()).toEqual(pricing);
    expect(requests[0].url).toBe('/api/usage/pricing');
  });

  it('setPricing puts the overrides', async () => {
    const { usage, requests } = transport((_url, init) =>
      Response.json({
        overrides: JSON.parse(String(init?.body)).overrides,
        tableDate: '2026-09-01',
      }),
    );

    const result = await usage.setPricing([
      { ...override, extra: 'dropped' } as PricingOverride,
    ]);

    expect(requests[0].init?.method).toBe('PUT');
    expect(JSON.parse(String(requests[0].init?.body))).toEqual({
      overrides: [override],
    });
    expect(result.overrides).toEqual([override]);
  });

  it('a daemon refusal reaches the caller as DaemonHttpError', async () => {
    const { usage } = transport(() =>
      Response.json(
        { error: 'groupBy must be one of day, model' },
        { status: 400 },
      ),
    );

    const failure = await usage.summary().catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(DaemonHttpError);
    expect(failure).toMatchObject({
      status: 400,
      message: 'groupBy must be one of day, model',
    });
  });

  it('the limits match the daemon', () => {
    expect(MAX_PRICING_OVERRIDES).toBe(100);
    expect(MAX_PRICE_MICROS_PER_MTOK).toBe(1_000_000_000_000);
    expect(MAX_USAGE_RECORDS_LIMIT).toBe(200);
  });
});
