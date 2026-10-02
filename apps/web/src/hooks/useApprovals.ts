import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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

type ErrorSource = 'pending' | 'decided' | 'settings' | 'action';

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
  // Each source keeps its own error, so one read failing neither hides
  // the others' results nor is cleared by an unrelated success.
  const [errors, setErrors] = useState<Record<ErrorSource, string | null>>({
    pending: null,
    decided: null,
    settings: null,
    action: null,
  });
  const setError = useCallback(
    (source: ErrorSource, value: string | null) =>
      setErrors((current) =>
        current[source] === value ? current : { ...current, [source]: value },
      ),
    [],
  );
  const [reads, setReads] = useState(0);
  const refresh = useCallback(() => setReads((value) => value + 1), []);
  // A settled approval leaves the stream's pending set: the decided list
  // has a new entry.
  const pendingKey = Object.keys(streamApprovals).sort().join('\u0000');
  const policyRef = useRef<ApprovalPolicy | null>(null);
  const savesInFlight = useRef(0);
  const loadMore = useRef<AbortController | null>(null);

  // No load-more outlives the list it would extend.
  useEffect(() => () => loadMore.current?.abort(), []);

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    loadMore.current?.abort();
    void daemon.listApprovals({ status: 'decided', agentId, signal }).then(
      (page) => {
        if (signal.aborted) return;
        setDecided(page.approvals);
        setCursor(page.nextCursor);
        setError('decided', null);
      },
      (caught: unknown) => {
        if (!signal.aborted) setError('decided', message(caught));
      },
    );
    return () => controller.abort();
  }, [agentId, reads, pendingKey, epoch, setError]);

  useEffect(() => {
    if (streamOpen) return;
    const controller = new AbortController();
    const { signal } = controller;
    void daemon.listApprovals({ status: 'pending', agentId, signal }).then(
      (page) => {
        if (signal.aborted) return;
        setReadPending(page.approvals);
        setError('pending', null);
      },
      (caught: unknown) => {
        if (!signal.aborted) setError('pending', message(caught));
      },
    );
    return () => controller.abort();
  }, [agentId, streamOpen, reads, epoch, setError]);

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    void Promise.all([
      daemon.approvalPolicy(agentId, { signal }),
      daemon.approvalRules(agentId, { signal }),
    ]).then(
      ([nextPolicy, nextRules]) => {
        if (signal.aborted) return;
        policyRef.current = nextPolicy;
        setPolicy(nextPolicy);
        setRules(nextRules.rules);
        setTools(nextRules.tools);
        setError('settings', null);
      },
      (caught: unknown) => {
        if (!signal.aborted) setError('settings', message(caught));
      },
    );
    return () => controller.abort();
  }, [agentId, reads, setError]);

  const pending = useMemo(
    () => (streamOpen ? pendingApprovals(streamApprovals) : readPending),
    [streamOpen, streamApprovals, readPending],
  );

  const loadMoreDecided = useCallback(() => {
    if (!cursor) return;
    loadMore.current?.abort();
    const controller = new AbortController();
    loadMore.current = controller;
    const { signal } = controller;
    void daemon
      .listApprovals({ status: 'decided', agentId, cursor, signal })
      .then(
        (page) => {
          if (signal.aborted) return;
          setDecided((current) => [
            ...current,
            ...page.approvals.filter(
              (approval) => !current.some((known) => known.id === approval.id),
            ),
          ]);
          setCursor(page.nextCursor);
        },
        (caught: unknown) => {
          if (!signal.aborted) setError('action', message(caught));
        },
      );
  }, [agentId, cursor, setError]);

  // Built from the latest policy, not the rendered one, so two quick
  // changes both stand. The choice shows at once; the daemon's answer
  // replaces it when no newer change is still being saved.
  const setPolicyAction = useCallback(
    async (klass: PolicyClass, action: ApprovalPolicyAction) => {
      const before = policyRef.current ?? DEFAULT_APPROVAL_POLICY;
      const next = { ...before, [klass]: action };
      policyRef.current = next;
      setPolicy(next);
      savesInFlight.current += 1;
      try {
        const saved = await daemon.setApprovalPolicy(agentId, next);
        savesInFlight.current -= 1;
        if (savesInFlight.current === 0) {
          policyRef.current = saved;
          setPolicy(saved);
        }
        setError('action', null);
      } catch (caught) {
        savesInFlight.current -= 1;
        if (policyRef.current?.[klass] === action) {
          const reverted = { ...policyRef.current, [klass]: before[klass] };
          policyRef.current = reverted;
          setPolicy(reverted);
        }
        setError('action', message(caught));
      }
    },
    [agentId, setError],
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
        setError('action', null);
        return true;
      } catch (caught) {
        setError('action', message(caught));
        return false;
      }
    },
    [agentId, setError],
  );

  const removeRule = useCallback(
    async (rule: ApprovalRule) => {
      try {
        await daemon.removeApprovalRule(agentId, rule.id);
        setRules((current) => current.filter((known) => known.id !== rule.id));
        setError('action', null);
      } catch (caught) {
        setError('action', message(caught));
      }
    },
    [agentId, setError],
  );

  return {
    pending,
    decided,
    hasMoreDecided: cursor !== null,
    loadMoreDecided,
    policy,
    rules,
    tools,
    error: Object.values(errors).find((value) => value !== null) ?? null,
    setPolicyAction,
    addRule,
    removeRule,
    refresh,
  };
}
