import { afterEach, describe, expect, it } from 'vitest';
import { addTrade, getTrades, resetTrades } from './store.js';
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

describe('trade store', () => {
  it('starts empty', () => {
    expect(getTrades()).toEqual([]);
  });

  it('addTrade prepends newest trade first', () => {
    addTrade(makeTrade({ action: 'BUY' }));
    addTrade(makeTrade({ action: 'SELL' }));
    const trades = getTrades();
    expect(trades[0].action).toBe('SELL');
    expect(trades[1].action).toBe('BUY');
  });

  it('caps at 50 entries', () => {
    for (let i = 0; i < 55; i++) {
      addTrade(makeTrade({ price: i }));
    }
    expect(getTrades().length).toBe(50);
  });

  it('getTrades returns a copy (not the internal array)', () => {
    addTrade(makeTrade());
    const a = getTrades();
    const b = getTrades();
    expect(a).not.toBe(b);
  });
});
