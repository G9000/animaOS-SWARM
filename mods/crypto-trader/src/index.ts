import type { ModPlugin } from '@animaOS-SWARM/mod-sdk';
import { getPrice } from './tools/get-price.js';
import { executeTrade } from './tools/execute-trade.js';
import { getTradeHistory } from './tools/get-trade-history.js';

const plugin: ModPlugin = {
  name: 'crypto-trader',
  description: 'AI-powered crypto market intelligence — get prices, analyze trends, record decisions',
  tools: [getPrice, executeTrade, getTradeHistory],
};

export default plugin;
