import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  DaemonHttpError,
  DEFAULT_APPROVAL_POLICY,
  type Approval,
  type ApprovalMatcher,
  type ApprovalPolicy,
  type ApprovalPolicyAction,
  type ApprovalRule,
  type ApprovalTool,
  type PolicyClass,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';
import { pendingApprovals, type LiveState } from '../lib/session-events';

export interface ApprovalsOptions {
  agentId: string;
  /** The companion stream's pending approvals. */
  streamApprovals: LiveState['approvals'];
  /** While open, the stream's pending approvals are the ones shown. */
  streamOpen: boolean;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
}

export interface ApprovalsView {
  /** Waiting for the owner, oldest first. */
  pending: Approval[];
  /** Decided in the last 30 days, newest first. */
  decided: Approval[];
  hasMoreDecided: boolean;
  loadMoreDecided: () => void;
  /** Null until read. */
  policy: ApprovalPolicy | null;
  rules: ApprovalRule[];
  /** The tools a rule can cover. */
  tools: ApprovalTool[];
  error: string | null;
  setPolicyAction: (
    klass: PolicyClass,
    action: ApprovalPolicyAction,
  ) => Promise<void>;
  /** True when the daemon kept the rule. */
  addRule: (tool: string, matcher: ApprovalMatcher) => Promise<boolean>;
  removeRule: (rule: ApprovalRule) => Promise<void>;
  /** Reads the lists again, as a decision made without the stream needs. */
  refresh: () => void;
}

function message(error: unknown): string {
  return error instanceof DaemonHttpError
    ? error.message
    : COMPANION_UNREACHABLE;
}

/** The Approvals page's data (spec §15.4, §15.5 `useApprovals`). */
export function useApprovals({
  agentId,
  streamApprovals,
  streamOpen,
  epoch,
}: ApprovalsOptions): ApprovalsView {
  const [readPending, setReadPending] = useState<Approval[]>([]);
  const [decided, setDecided] = useState<Approval[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [policy, setPolicy] = useState<ApprovalPolicy | null>(null);
  const [rules, setRules] = useState<ApprovalRule[]>([]);
  const [tools, setTools] = useState<ApprovalTool[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [reads, setReads] = useState(0);
  const refresh = useCallback(() => setReads((value) => value + 1), []);
  // A settled approval leaves the stream's pending set: the decided list
  // has a new entry.
  const pendingKey = Object.keys(streamApprovals).sort().join('\u0000');

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    void Promise.all([
      daemon.listApprovals({ status: 'decided', agentId, signal }),
      streamOpen
        ? null
        : daemon.listApprovals({ status: 'pending', agentId, signal }),
    ]).then(
      ([decidedPage, pendingPage]) => {
        if (signal.aborted) return;
        setDecided(decidedPage.approvals);
        setCursor(decidedPage.nextCursor);
        if (pendingPage) setReadPending(pendingPage.approvals);
        setError(null);
      },
      (caught: unknown) => {
        if (!signal.aborted) setError(message(caught));
      },
    );
    return () => controller.abort();
  }, [agentId, streamOpen, reads, pendingKey, epoch]);

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    void Promise.all([
      daemon.approvalPolicy(agentId, { signal }),
      daemon.approvalRules(agentId, { signal }),
    ]).then(
      ([nextPolicy, nextRules]) => {
        if (signal.aborted) return;
        setPolicy(nextPolicy);
        setRules(nextRules.rules);
        setTools(nextRules.tools);
      },
      (caught: unknown) => {
        if (!signal.aborted) setError(message(caught));
      },
    );
    return () => controller.abort();
  }, [agentId, reads]);

  const pending = useMemo(
    () => (streamOpen ? pendingApprovals(streamApprovals) : readPending),
    [streamOpen, streamApprovals, readPending],
  );

  const loadMoreDecided = useCallback(() => {
    if (!cursor) return;
    void daemon.listApprovals({ status: 'decided', agentId, cursor }).then(
      (page) => {
        setDecided((current) => [
          ...current,
          ...page.approvals.filter(
            (approval) => !current.some((known) => known.id === approval.id),
          ),
        ]);
        setCursor(page.nextCursor);
      },
      (caught: unknown) => setError(message(caught)),
    );
  }, [agentId, cursor]);

  const setPolicyAction = useCallback(
    async (klass: PolicyClass, action: ApprovalPolicyAction) => {
      const next = { ...(policy ?? DEFAULT_APPROVAL_POLICY), [klass]: action };
      try {
        setPolicy(await daemon.setApprovalPolicy(agentId, next));
        setError(null);
      } catch (caught) {
        setError(message(caught));
      }
    },
    [agentId, policy],
  );

  const addRule = useCallback(
    async (tool: string, matcher: ApprovalMatcher) => {
      try {
        const rule = await daemon.addApprovalRule(agentId, { tool, matcher });
        setRules((current) =>
          current.some((known) => known.id === rule.id)
            ? current
            : [...current, rule],
        );
        setError(null);
        return true;
      } catch (caught) {
        setError(message(caught));
        return false;
      }
    },
    [agentId],
  );

  const removeRule = useCallback(
    async (rule: ApprovalRule) => {
      try {
        await daemon.removeApprovalRule(agentId, rule.id);
        setRules((current) => current.filter((known) => known.id !== rule.id));
        setError(null);
      } catch (caught) {
        setError(message(caught));
      }
    },
    [agentId],
  );

  return {
    pending,
    decided,
    hasMoreDecided: cursor !== null,
    loadMoreDecided,
    policy,
    rules,
    tools,
    error,
    setPolicyAction,
    addRule,
    removeRule,
    refresh,
  };
}
