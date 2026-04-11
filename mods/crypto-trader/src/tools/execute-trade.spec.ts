import { afterEach, describe, expect, it } from 'vitest';
import { executeTrade } from './execute-trade.js';
import { getTrades, resetTrades } from './store.js';

afterEach(() => {
  resetTrades();
});

const validArgs = {
  action: 'BUY',
  token: 'bitcoin',
  amount: 1,
  price: 95000,
  reason: 'Trend is bullish',
};

describe('execute_trade tool', () => {
  it('records a trade and returns a tx hash', async () => {
    const result = await executeTrade.execute(validArgs) as {
      success: boolean;
      tx_hash: string;
      trade: { action: string; token: string };
    };

    expect(result.success).toBe(true);
    expect(result.tx_hash).toMatch(/^0x[0-9a-f]+/);
    expect(result.trade.action).toBe('BUY');
    expect(result.trade.token).toBe('bitcoin');
    expect(getTrades()).toHaveLength(1);
  });

  it('throws on invalid action', async () => {
    await expect(executeTrade.execute({ ...validArgs, action: 'YOLO' })).rejects.toThrow('Invalid action');
  });

  it('throws when token is missing', async () => {
    await expect(executeTrade.execute({ ...validArgs, token: '' })).rejects.toThrow('token is required');
  });

  it('throws when amount is zero or negative', async () => {
    await expect(executeTrade.execute({ ...validArgs, amount: 0 })).rejects.toThrow('amount must be greater than 0');
  });

  it('throws when price is zero or negative', async () => {
    await expect(executeTrade.execute({ ...validArgs, price: -1 })).rejects.toThrow('price must be greater than 0');
  });
});
