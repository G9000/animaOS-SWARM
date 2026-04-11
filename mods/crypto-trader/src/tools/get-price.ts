import { defineModTool } from '@animaOS-SWARM/mod-sdk';

export const getPrice = defineModTool({
  name: 'get_price',
  description:
    'Fetch the current market price of a cryptocurrency. ' +
    'Returns the price and 24h price change percentage.',
  parameters: {
    type: 'object',
    properties: {
      token: {
        type: 'string',
        description: 'CoinGecko token ID (e.g. "bitcoin", "ethereum", "solana")',
      },
      currency: {
        type: 'string',
        description: 'Target currency code (e.g. "usd", "eur")',
      },
    },
    required: ['token', 'currency'],
  },
  execute: async (args) => {
    const token = String(args['token'] ?? '');
    const currency = String(args['currency'] ?? 'usd');

    if (!token) {
      throw new Error('token is required');
    }

    const url = `https://api.coingecko.com/api/v3/simple/price?ids=${encodeURIComponent(token)}&vs_currencies=${encodeURIComponent(currency)}&include_24hr_change=true`;
    const response = await fetch(url);

    if (!response.ok) {
      throw new Error(`CoinGecko API error: ${response.status} ${response.statusText}`);
    }

    const data = (await response.json()) as Record<string, Record<string, number>>;
    const tokenData = data[token];

    if (!tokenData) {
      throw new Error(`Token "${token}" not found on CoinGecko`);
    }

    return {
      token,
      currency,
      price: tokenData[currency],
      change_24h: tokenData[`${currency}_24h_change`] ?? null,
    };
  },
});
