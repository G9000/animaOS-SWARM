import { afterEach, describe, expect, it, vi } from 'vitest';
import { getPrice } from './get-price.js';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('get_price tool', () => {
  it('returns price and 24h change on success', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        bitcoin: { usd: 95000, usd_24h_change: 2.5 },
      }),
    }));

    const result = await getPrice.execute({ token: 'bitcoin', currency: 'usd' });
    expect(result).toEqual({ token: 'bitcoin', currency: 'usd', price: 95000, change_24h: 2.5 });
  });

  it('throws when token arg is missing', async () => {
    await expect(getPrice.execute({ currency: 'usd' })).rejects.toThrow('token is required');
  });

  it('throws on non-200 API response', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: false,
      status: 429,
      statusText: 'Too Many Requests',
    }));

    await expect(getPrice.execute({ token: 'bitcoin', currency: 'usd' })).rejects.toThrow('CoinGecko API error: 429');
  });

  it('throws when token not found in response', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({}),
    }));

    await expect(getPrice.execute({ token: 'unknowncoin', currency: 'usd' })).rejects.toThrow('Token "unknowncoin" not found');
  });
});
