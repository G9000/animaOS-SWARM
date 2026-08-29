import type { TelegramConnectorStatus } from './daemon-api';

const STATUS_LABELS: Record<TelegramConnectorStatus, string> = {
  ready: 'Connected',
  pairing: 'Waiting for a message',
  credentialRequired: 'Token required',
  error: 'Error',
  degraded: 'Degraded',
  reconciling: 'Reconciling',
};

export function connectorStatusLabel(status: string): string {
  return STATUS_LABELS[status as TelegramConnectorStatus] ?? 'Unavailable';
}

export function createTelegramIdempotencyKey(): string {
  const random =
    globalThis.crypto?.randomUUID?.() ??
    `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `telegram-${random}`.slice(0, 128);
}

export function safeIntegrationError(error: unknown): string {
  const message = error instanceof Error ? error.message : '';
  if (/\d{5,}:[A-Za-z0-9_-]{4,}/.test(message)) {
    return 'Telegram request failed. Check the connector and try again.';
  }
  return (
    message || 'Telegram request failed. Check the connector and try again.'
  );
}
