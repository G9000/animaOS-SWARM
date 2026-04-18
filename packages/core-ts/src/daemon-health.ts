export type DaemonWarningSource = 'manual' | 'poll';

export interface DaemonWarningTransition {
  message?: string;
  recovered: boolean;
}

export const DAEMON_HEALTH_UNAVAILABLE_MESSAGE =
  'Daemon health checks unavailable in this session.';

export const DAEMON_HEALTHY_MESSAGE = 'Daemon reachable. Launch tasks can run.';

export const DAEMON_RECOVERED_MESSAGE =
  'Daemon reachable again. Launch tasks can run.';

export const DAEMON_CONNECTION_LOST_MESSAGE =
  'Daemon connection lost. Launch tasks will fail until the daemon is reachable.';

export function formatDaemonUnreachableWarning(errorMessage: string): string {
  return `${errorMessage}. Launch tasks will fail until the daemon is reachable.`;
}

export function describeDaemonWarningTransition(
  previousWarning: string | null | undefined,
  nextWarning: string | null | undefined,
  source: DaemonWarningSource
): DaemonWarningTransition {
  const previous = previousWarning ?? null;
  const next = nextWarning ?? null;

  if (source === 'manual') {
    if (next) {
      return { message: next, recovered: false };
    }

    if (previous) {
      return { message: DAEMON_RECOVERED_MESSAGE, recovered: true };
    }

    return { message: DAEMON_HEALTHY_MESSAGE, recovered: false };
  }

  if (previous && !next) {
    return { message: DAEMON_RECOVERED_MESSAGE, recovered: true };
  }

  if (!previous && next) {
    return { message: DAEMON_CONNECTION_LOST_MESSAGE, recovered: false };
  }

  return { recovered: false };
}
