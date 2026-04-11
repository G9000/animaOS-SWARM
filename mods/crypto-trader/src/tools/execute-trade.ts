import { defineModTool } from '@animaOS-SWARM/mod-sdk';
import { addTrade } from './store.js';

export const executeTrade = defineModTool({
  name: 'execute_trade',
  description:
    'Record a trading decision. This is a mocked tool — no real transaction is executed. ' +
    'Records the decision to trade history for future reference.',
  parameters: {
    type: 'object',
    properties: {
      action: {
        type: 'string',
        enum: ['BUY', 'SELL', 'HOLD'],
        description: 'The trading action to take',
      },
      token: {
        type: 'string',
        description: 'CoinGecko token ID (e.g. "bitcoin", "ethereum")',
      },
      amount: {
        type: 'number',
        description: 'Amount of token to trade. Use 0 for HOLD decisions.',
      },
      price: {
        type: 'number',
        description: 'Current price at time of decision',
      },
      reason: {
        type: 'string',
        description: 'Agent reasoning for this trade decision',
      },
    },
    required: ['action', 'token', 'amount', 'price', 'reason'],
  },
  execute: async (args) => {
    const action = String(args['action'] ?? '') as 'BUY' | 'SELL' | 'HOLD';
    const token = String(args['token'] ?? '');
    const amount = Number(args['amount'] ?? 0);
    const price = Number(args['price'] ?? 0);
    const reason = String(args['reason'] ?? '');

    if (!['BUY', 'SELL', 'HOLD'].includes(action)) {
      throw new Error(`Invalid action "${action}". Must be BUY, SELL, or HOLD`);
    }
    if (!token) throw new Error('token is required');
    if (!reason) throw new Error('reason is required');
    if (action !== 'HOLD' && amount <= 0) throw new Error('amount must be greater than 0');
    if (price <= 0) throw new Error('price must be greater than 0');

    const trade = {
      timestamp: Date.now(),
      action,
      token,
      amount,
      price,
      reason,
    };

    addTrade(trade);

    // Mocked tx hash
    const txHash = `0x${Math.random().toString(16).slice(2).padEnd(64, '0')}`;

    return {
      success: true,
      tx_hash: txHash,
      trade,
    };
  },
});
