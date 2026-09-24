import { describe, expect, it } from 'vitest';

import { sessionFixture } from '../test/sessions';
import { exportFileName, groupSessions, presentKinds } from './session-groups';

const NOW = new Date(2026, 8, 24, 12, 0, 0);
const HOUR = 60 * 60 * 1000;

describe('session groups', () => {
  it('groups by local day and nests helpers under their listed parent', () => {
    const today = sessionFixture('chat:today', {
      title: 'Today',
      lastActivityAtMs: NOW.getTime() - HOUR,
    });
    const helper = sessionFixture('room-9', {
      agentId: 'helper-1',
      kind: 'helper',
      parentAgentId: 'agent-main',
      parentSessionId: 'chat:today',
      lastActivityAtMs: NOW.getTime() - 2 * HOUR,
    });
    const orphan = sessionFixture('peer:beta:agent-main', {
      kind: 'helper',
      parentAgentId: 'beta',
      lastActivityAtMs: NOW.getTime() - 3 * HOUR,
    });
    const yesterday = sessionFixture('chat:yesterday', {
      lastActivityAtMs: NOW.getTime() - 24 * HOUR,
    });
    const week = sessionFixture('chat:week', {
      lastActivityAtMs: NOW.getTime() - 4 * 24 * HOUR,
    });
    const older = sessionFixture('chat:older', {
      lastActivityAtMs: NOW.getTime() - 30 * 24 * HOUR,
    });

    const groups = groupSessions([today, helper, orphan, yesterday, week, older], NOW);

    expect(
      groups.map((group) => [
        group.label,
        group.nodes.map((node) => [
          node.session.id,
          node.helpers.map((item) => item.id),
        ]),
      ]),
    ).toEqual([
      [
        'Today',
        [
          ['chat:today', ['room-9']],
          ['peer:beta:agent-main', []],
        ],
      ],
      ['Yesterday', [['chat:yesterday', []]]],
      ['Previous 7 days', [['chat:week', []]]],
      ['Older', [['chat:older', []]]],
    ]);
    expect(
      groupSessions([today, helper], NOW, false)[0].nodes.map((node) => node.session.id),
    ).toEqual(['chat:today', 'room-9']);
  });

  it('lists only the kinds present, in sidebar order', () => {
    expect(
      presentKinds([
        sessionFixture('job:1', { kind: 'job' }),
        sessionFixture('chat:1'),
        sessionFixture('telegram:t', { kind: 'telegram' }),
      ]),
    ).toEqual(['chat', 'telegram', 'job']);
  });

  it('names export files like the daemon', () => {
    expect(exportFileName('Check-in · Check status')).toBe('check-in-check-status.md');
    expect(exportFileName('···')).toBe('session.md');
  });
});
