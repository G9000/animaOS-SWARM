import { afterEach, describe, expect, it } from 'vitest';
import { getTradeHistory } from './get-trade-history.js';
import { addTrade, resetTrades } from './store.js';
import type { Trade } from './store.js';

const makeTrade = (overrides: Partial<Trade> = {}): Trade => ({
  timestamp: Date.now(),
  action: 'BUY',
  token: 'bitcoin',
  amount: 1,
  price: 50000,
  reason: 'test',
  ...overrides,
});

afterEach(() => {
  resetTrades();
});

describe('get_trade_history tool', () => {
  it('returns empty array when no trades', async () => {
    const result = await getTradeHistory.execute({}) as { trades: Trade[]; total: number };
    expect(result.trades).toEqual([]);
    expect(result.total).toBe(0);
  });

  it('returns all trades up to default limit of 10', async () => {
    for (let i = 0; i < 15; i++) addTrade(makeTrade({ price: i }));
    const result = await getTradeHistory.execute({}) as { trades: Trade[]; total: number };
    expect(result.trades).toHaveLength(10);
    expect(result.total).toBe(10);
  });

  it('respects custom limit', async () => {
    for (let i = 0; i < 20; i++) addTrade(makeTrade({ price: i }));
    const result = await getTradeHistory.execute({ limit: 5 }) as { trades: Trade[]; total: number };
    expect(result.trades).toHaveLength(5);
  });

  it('caps limit at 50', async () => {
    for (let i = 0; i < 50; i++) addTrade(makeTrade({ price: i }));
    const result = await getTradeHistory.execute({ limit: 999 }) as { trades: Trade[]; total: number };
    expect(result.trades).toHaveLength(50);
  });
});
