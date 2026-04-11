export interface Trade {
  timestamp: number;
  action: 'BUY' | 'SELL' | 'HOLD';
  token: string;
  amount: number;
  price: number;
  reason: string;
}

const MAX_TRADES = 50;
const trades: Trade[] = [];

export function addTrade(trade: Trade): void {
  trades.unshift(trade);
  if (trades.length > MAX_TRADES) {
    trades.splice(MAX_TRADES);
  }
}

export function getTrades(): Trade[] {
  return [...trades];
}

/** For testing only */
export function resetTrades(): void {
  trades.splice(0);
}
