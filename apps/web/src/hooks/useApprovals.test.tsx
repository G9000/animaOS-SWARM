import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_APPROVAL_POLICY } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { approvalFixture } from '../test/live';
import { useApprovals } from './useApprovals';

beforeEach(() => {
  vi.spyOn(daemon, 'listApprovals').mockResolvedValue({
    approvals: [],
    nextCursor: null,
  });
  vi.spyOn(daemon, 'approvalPolicy').mockResolvedValue(DEFAULT_APPROVAL_POLICY);
  vi.spyOn(daemon, 'approvalRules').mockResolvedValue({ rules: [], tools: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useApprovals', () => {
  it('reads the decided list again once a pending approval resolves', async () => {
    const approval = approvalFixture('apr_1');
    const { result, rerender } = renderHook(
      (props: { approvals: Record<string, typeof approval> }) =>
        useApprovals({
          agentId: 'agent-main',
          streamApprovals: props.approvals,
          streamOpen: true,
          epoch: 1,
        }),
      { initialProps: { approvals: { apr_1: approval } } },
    );
    await waitFor(() => expect(result.current.policy).not.toBeNull());
    expect(result.current.pending).toEqual([approval]);
    const decidedReads = () =>
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.filter(([options]) => options.status === 'decided').length;
    expect(decidedReads()).toBe(1);
    expect(
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.some(([options]) => options.status === 'pending'),
    ).toBe(false);

    rerender({ approvals: {} });

    await waitFor(() => expect(decidedReads()).toBe(2));
    expect(result.current.pending).toEqual([]);
  });

  it('reads pending approvals itself while the stream is closed, and aborts on unmount', async () => {
    const approval = approvalFixture('apr_1');
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) => ({
      approvals: options.status === 'pending' ? [approval] : [],
      nextCursor: null,
    }));
    const { result, unmount } = renderHook(() =>
      useApprovals({
        agentId: 'agent-main',
        streamApprovals: {},
        streamOpen: false,
        epoch: 0,
      }),
    );

    await waitFor(() => expect(result.current.pending).toEqual([approval]));
    const signals = vi
      .mocked(daemon.listApprovals)
      .mock.calls.map(([options]) => options.signal);
    unmount();
    expect(signals.every((signal) => signal?.aborted)).toBe(true);
  });
});
