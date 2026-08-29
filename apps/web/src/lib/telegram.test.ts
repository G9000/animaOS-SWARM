import { describe, expect, it, vi } from 'vitest';

import {
  connectorStatusLabel,
  createTelegramIdempotencyKey,
  safeIntegrationError,
} from './telegram';

describe('telegram helpers', () => {
  it('maps daemon statuses to safe user labels', () => {
    expect(connectorStatusLabel('credentialRequired')).toBe('Token required');
    expect(connectorStatusLabel('reconciling')).toBe('Reconciling');
    expect(connectorStatusLabel('unexpected')).toBe('Unavailable');
  });

  it('does not surface token-shaped strings from errors', () => {
    expect(
      safeIntegrationError(new Error('Telegram rejected 123456:supersecret')),
    ).toBe('Telegram request failed. Check the connector and try again.');
  });

  it('creates bounded visible idempotency keys', () => {
    vi.stubGlobal('crypto', { randomUUID: () => 'fixed-uuid' });
    const key = createTelegramIdempotencyKey();
    expect(key).toBe('telegram-fixed-uuid');
    expect(key.length).toBeLessThanOrEqual(128);
  });
});
