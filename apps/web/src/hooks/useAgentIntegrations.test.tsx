import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  daemon,
  type DaemonSchedule,
  type TelegramConnector,
} from '../lib/daemon-api';
import { useAgentIntegrations } from './useAgentIntegrations';

vi.mock('../lib/daemon-api', async () => {
  const actual =
    await vi.importActual<typeof import('../lib/daemon-api')>(
      '../lib/daemon-api',
    );
  return {
    ...actual,
    daemon: {
      ...actual.daemon,
      listConnectors: vi.fn(),
      listSchedules: vi.fn(),
      createTelegramConnector: vi.fn(),
    },
  };
});

const connector: TelegramConnector = {
  id: 'connector-1',
  agentId: 'agent-a',
  roomId: 'telegram:connector-1',
  type: 'telegram',
  bot: { id: '1', username: 'anima_bot', displayName: 'Anima' },
  approvedChat: null,
  pendingPairing: null,
  status: 'pairing',
  enabled: true,
  createdAtMs: 1,
  updatedAtMs: 1,
};
const schedule: DaemonSchedule = {
  id: 'schedule-1',
  importIdempotencyKey: null,
  agentId: 'agent-a',
  prompt: 'Check',
  trigger: { type: 'interval', intervalMs: 60_000 },
  enabled: true,
  target: { type: 'workspace' },
  nextDueAtMs: 60_001,
  lastFiredAtMs: null,
  lastOutcome: null,
  createdAtMs: 1,
  updatedAtMs: 1,
};

beforeEach(() => {
  vi.mocked(daemon.listConnectors).mockReset();
  vi.mocked(daemon.listSchedules).mockReset();
  vi.mocked(daemon.createTelegramConnector).mockReset();
});

describe('useAgentIntegrations', () => {
  it('loads connectors and schedules independently', async () => {
    vi.mocked(daemon.listConnectors).mockRejectedValue(
      new Error('connector failed'),
    );
    vi.mocked(daemon.listSchedules).mockResolvedValue({
      schedules: [schedule],
    });
    const { result } = renderHook(() => useAgentIntegrations('agent-a'));
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.connectorError).toBe('connector failed');
    expect(result.current.scheduleError).toBeNull();
    expect(result.current.schedules).toEqual([schedule]);
  });

  it('clears old agent data and fences stale mutation completion', async () => {
    vi.mocked(daemon.listConnectors)
      .mockResolvedValueOnce({ connectors: [connector] })
      .mockResolvedValueOnce({ connectors: [] });
    vi.mocked(daemon.listSchedules)
      .mockResolvedValueOnce({ schedules: [schedule] })
      .mockResolvedValueOnce({ schedules: [] });
    let resolveCreate!: (value: { connector: TelegramConnector }) => void;
    vi.mocked(daemon.createTelegramConnector).mockReturnValue(
      new Promise((resolve) => {
        resolveCreate = resolve;
      }),
    );
    const { result, rerender } = renderHook(
      ({ id }) => useAgentIntegrations(id),
      { initialProps: { id: 'agent-a' as string | null } },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => {
      void result.current.connectTelegram('secret');
    });
    rerender({ id: 'agent-b' });
    expect(result.current.connectors).toEqual([]);
    await act(async () => {
      resolveCreate({ connector });
      await Promise.resolve();
    });
    expect(result.current.connectors).toEqual([]);
  });
});
