import { describe, expect, it } from 'vitest';

import { connectorStatusLabel, safeIntegrationError } from './telegram';

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
});
