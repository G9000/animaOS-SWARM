import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError, DEFAULT_APPROVAL_POLICY } from '@animaOS-SWARM/sdk';

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

  it('keeps two quick policy changes', async () => {
    const save = vi
      .spyOn(daemon, 'setApprovalPolicy')
      .mockImplementation(async (_agentId, policy) => policy);
    const { result } = renderHook(() =>
      useApprovals({
        agentId: 'agent-main',
        streamApprovals: {},
        streamOpen: true,
        epoch: 0,
      }),
    );
    await waitFor(() => expect(result.current.policy).not.toBeNull());

    await act(async () => {
      await Promise.all([
        result.current.setPolicyAction('exec', 'deny'),
        result.current.setPolicyAction('network', 'allow'),
      ]);
    });

    expect(save).toHaveBeenLastCalledWith('agent-main', {
      ...DEFAULT_APPROVAL_POLICY,
      exec: 'deny',
      network: 'allow',
    });
    expect(result.current.policy).toEqual({
      ...DEFAULT_APPROVAL_POLICY,
      exec: 'deny',
      network: 'allow',
    });
  });

  it('still lists pending approvals when the decided list cannot be read', async () => {
    const approval = approvalFixture('apr_1');
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) => {
      if (options.status === 'decided')
        throw new DaemonHttpError(503, { error: 'history store unavailable' });
      return { approvals: [approval], nextCursor: null };
    });
    const { result } = renderHook(() =>
      useApprovals({
        agentId: 'agent-main',
        streamApprovals: {},
        streamOpen: false,
        epoch: 0,
      }),
    );

    await waitFor(() => expect(result.current.pending).toEqual([approval]));
    await waitFor(() =>
      expect(result.current.error).toBe('history store unavailable'),
    );
    // The policy and rules reads settled fine and do not clear it.
    await waitFor(() => expect(result.current.policy).not.toBeNull());
    expect(result.current.error).toBe('history store unavailable');
  });

  it('abandons a load-more still in flight when the list is read again', async () => {
    const first = approvalFixture('apr_1');
    const older = approvalFixture('apr_0');
    let olderSignal: AbortSignal | undefined;
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) => {
      if (options.cursor) {
        olderSignal = options.signal;
        return { approvals: [older], nextCursor: null };
      }
      return { approvals: [first], nextCursor: 'c1' };
    });
    const { result } = renderHook(() =>
      useApprovals({
        agentId: 'agent-main',
        streamApprovals: {},
        streamOpen: true,
        epoch: 0,
      }),
    );
    await waitFor(() => expect(result.current.hasMoreDecided).toBe(true));

    act(() => result.current.loadMoreDecided());
    await waitFor(() => expect(olderSignal).toBeDefined());
    act(() => result.current.refresh());

    expect(olderSignal?.aborted).toBe(true);
    await waitFor(() => expect(result.current.policy).not.toBeNull());
  });
});
