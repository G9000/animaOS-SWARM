import { defineModTool } from '@animaOS-SWARM/mod-sdk';
import { getTrades } from './store.js';

export const getTradeHistory = defineModTool({
  name: 'get_trade_history',
  description:
    'Return recent trade decisions made by the agent swarm. ' +
    'Results are ordered newest-first. Optionally limit the number of results.',
  parameters: {
    type: 'object',
    properties: {
      limit: {
        type: 'number',
        description: 'Maximum number of trades to return (default: 10, max: 50)',
      },
    },
    required: [],
  },
  execute: async (args) => {
    const limit = Math.min(Math.max(1, Number(args['limit'] ?? 10)), 50);
    const trades = getTrades().slice(0, limit);
    return { trades, total: trades.length };
  },
});
