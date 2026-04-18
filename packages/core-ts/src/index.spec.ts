import { describe, expect, it } from 'vitest';
import {
  DAEMON_RECOVERED_MESSAGE,
  EventBus,
  agent,
  describeDaemonWarningTransition,
} from './index.js';

describe('core package root exports', () => {
  it('supports a minimal consumer flow through the package entrypoint', async () => {
    const eventBus = new EventBus();
    const events: Array<{ agentId?: string; name: string }> = [];
    const manager = agent({ name: 'manager', model: 'gpt-5.4' });

    eventBus.on<{ agentId: string; name: string }>('agent:spawned', (event) => {
      events.push({ agentId: event.agentId, name: event.data.name });
    });

    await eventBus.emit(
      'agent:spawned',
      { agentId: 'launch:manager', name: manager.name },
      'launch:manager'
    );

    expect(events).toEqual([{ agentId: 'launch:manager', name: 'manager' }]);
    expect(
      describeDaemonWarningTransition('daemon unavailable', null, 'manual')
    ).toEqual({
      message: DAEMON_RECOVERED_MESSAGE,
      recovered: true,
    });
  });
});
