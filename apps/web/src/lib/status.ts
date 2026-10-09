import {
  DaemonHttpError,
  StatusTooOldError,
  type DaemonStatus,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from './approvals';

export const STATUS_POLL_MS = 15_000;
export const HEALTH_TOO_OLD = 'Update the daemon to see its health.';
export const HEALTH_LOADING = 'Loading health…';

export type CardState = 'ok' | 'warn' | 'bad';

export interface HealthCard {
  id:
    | 'readiness'
    | 'storage'
    | 'providers'
    | 'connectors'
    | 'automations'
    | 'approvals'
    | 'runs'
    | 'daemon';
  title: string;
  state: CardState;
  summary: string;
  details: string[];
  link?: { label: string; hash: string };
}

/** What a failed status read says, and the HTTP status behind it. */
export function statusErrorMessage(error: unknown): {
  message: string;
  status: number | null;
} {
  if (error instanceof StatusTooOldError)
    return { message: HEALTH_TOO_OLD, status: 404 };
  if (error instanceof DaemonHttpError)
    return {
      message: error.status === 404 ? HEALTH_TOO_OLD : error.message,
      status: error.status,
    };
  return { message: COMPANION_UNREACHABLE, status: null };
}

export function approvalsSummary(pending: number): string {
  if (pending <= 0) return 'Nothing is waiting for you';
  return pending === 1
    ? '1 request is waiting for you'
    : `${pending} requests are waiting for you`;
}

/** `45s`, `12m`, `3h 05m`, `2d 4h`. */
export function formatUptime(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

const plural = (count: number, one: string, many = `${one}s`) =>
  `${count} ${count === 1 ? one : many}`;

/** The Health page's cards, in a fixed order (spec §15.4). Every string may
 *  hold daemon text, so the page shows them through `RevealedText`. */
export function healthCards(status: DaemonStatus): HealthCard[] {
  const { readiness, storage, automations, runs } = status;
  const history = storage.history;
  const configured = status.providers.filter((provider) => provider.configured);
  // The daemon calls a connector that is up `ready`.
  const notReady = status.connectors.filter(
    (connector) => connector.enabled && connector.status !== 'ready',
  );

  const readinessCard: HealthCard =
    readiness.status === 'ready'
      ? {
          id: 'readiness',
          title: 'Readiness',
          state: 'ok',
          summary: 'The daemon is ready',
          details: [],
        }
      : {
          id: 'readiness',
          title: 'Readiness',
          state: 'bad',
          summary: 'The daemon is not ready',
          details: [...readiness.issues],
        };

  const storageDetails = [
    `Control plane: ${storage.controlPlane}`,
    `${plural(history.pendingFlush, 'write')} waiting to be saved`,
  ];
  if (history.ephemeral) storageDetails.push('History is kept in memory only');
  if (!history.healthy && history.lastError)
    storageDetails.push(`Last error: ${history.lastError}`);
  const storageCard: HealthCard = {
    id: 'storage',
    title: 'Storage',
    state: history.healthy ? 'ok' : 'bad',
    summary: history.healthy
      ? `History store: ${history.store}`
      : `History store ${history.store} is failing to save`,
    details: storageDetails,
  };

  const providersCard: HealthCard = {
    id: 'providers',
    title: 'Providers',
    state: configured.length > 0 ? 'ok' : 'warn',
    summary: `${configured.length} of ${status.providers.length} configured`,
    details: configured.map((provider) => provider.label),
  };

  const connectorsCard: HealthCard = {
    id: 'connectors',
    title: 'Connectors',
    state: notReady.length > 0 ? 'warn' : 'ok',
    summary:
      status.connectors.length === 0
        ? 'No connectors'
        : notReady.length > 0
          ? `${notReady.length} of ${plural(status.connectors.length, 'connector')} not ready`
          : plural(status.connectors.length, 'connector'),
    details: status.connectors.map(
      (connector) =>
        `${connector.type} · ${connector.status}${connector.enabled ? '' : ' (off)'}`,
    ),
  };

  const automationsCard: HealthCard = {
    id: 'automations',
    title: 'Automations',
    state: automations.failing > 0 ? 'warn' : 'ok',
    summary:
      automations.failing > 0
        ? `${automations.enabled} enabled, ${automations.failing} failing`
        : `${automations.enabled} enabled`,
    details: [`${automations.total} in all`],
    ...(automations.failing > 0
      ? { link: { label: 'Review automations', hash: '#/automations' } }
      : {}),
  };

  const approvalsCard: HealthCard = {
    id: 'approvals',
    title: 'Pending approvals',
    state: status.approvals.pending > 0 ? 'warn' : 'ok',
    summary: approvalsSummary(status.approvals.pending),
    details: [],
    ...(status.approvals.pending > 0
      ? { link: { label: 'Review approvals', hash: '#/approvals' } }
      : {}),
  };

  const runsCard: HealthCard = {
    id: 'runs',
    title: 'Runs',
    state: 'ok',
    summary: `${runs.running} running, ${runs.queued} queued`,
    details: Object.entries(runs.byStatus)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, count]) => `${name}: ${count}`),
  };

  const daemonDetails = [`Up ${formatUptime(status.uptimeSeconds)}`];
  if (status.buildRevision) daemonDetails.push(`Build ${status.buildRevision}`);
  daemonDetails.push(plural(status.events.subscribers, 'event subscriber'));
  const daemonCard: HealthCard = {
    id: 'daemon',
    title: 'Daemon',
    state: 'ok',
    summary: `Version ${status.version}`,
    details: daemonDetails,
  };

  return [
    readinessCard,
    storageCard,
    providersCard,
    connectorsCard,
    automationsCard,
    approvalsCard,
    runsCard,
    daemonCard,
  ];
}
