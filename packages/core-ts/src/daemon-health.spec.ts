import { describe, expect, it } from 'vitest';
import {
  DAEMON_CONNECTION_LOST_MESSAGE,
  DAEMON_HEALTHY_MESSAGE,
  DAEMON_RECOVERED_MESSAGE,
  describeDaemonWarningTransition,
  formatDaemonUnreachableWarning,
} from './daemon-health.js';

describe('daemon health helpers', () => {
  it('formats unreachable daemon warnings consistently', () => {
    expect(formatDaemonUnreachableWarning('daemon unavailable')).toBe(
      'daemon unavailable. Launch tasks will fail until the daemon is reachable.'
    );
  });

  it('reports manual health checks as healthy when no warning exists', () => {
    expect(describeDaemonWarningTransition(null, null, 'manual')).toEqual({
      message: DAEMON_HEALTHY_MESSAGE,
      recovered: false,
    });
  });

  it('reports manual health checks as recovered when a warning clears', () => {
    expect(
      describeDaemonWarningTransition('daemon unavailable', null, 'manual')
    ).toEqual({
      message: DAEMON_RECOVERED_MESSAGE,
      recovered: true,
    });
  });

  it('reports lost daemon connectivity during polling', () => {
    expect(
      describeDaemonWarningTransition(null, 'daemon unavailable', 'poll')
    ).toEqual({
      message: DAEMON_CONNECTION_LOST_MESSAGE,
      recovered: false,
    });
  });

  it('reports recovered daemon connectivity during polling', () => {
    expect(
      describeDaemonWarningTransition('daemon unavailable', null, 'poll')
    ).toEqual({
      message: DAEMON_RECOVERED_MESSAGE,
      recovered: true,
    });
  });
});
