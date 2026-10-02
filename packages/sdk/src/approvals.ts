import type { DaemonClient } from './client.js';

/** How much a tool can change (spec §7.1). Read-class tools never ask. */
export type RiskClass = 'read' | 'write' | 'exec' | 'network' | 'delegate';
/** The classes a policy sets. */
export type PolicyClass = Exclude<RiskClass, 'read'>;
export const POLICY_CLASSES: readonly PolicyClass[] = [
  'write',
  'exec',
  'network',
  'delegate',
];

export type ApprovalPolicyAction = 'allow' | 'ask' | 'deny';
export type ApprovalPolicy = Record<PolicyClass, ApprovalPolicyAction>;
/** The daemon's default (spec §7.2): ask only before `exec`. */
export const DEFAULT_APPROVAL_POLICY: ApprovalPolicy = {
  write: 'allow',
  exec: 'ask',
  network: 'allow',
  delegate: 'allow',
};

export type ApprovalMatcherKind =
  | 'command_prefix'
  | 'path_glob'
  | 'domain'
  | 'any';
export interface ApprovalMatcher {
  kind: ApprovalMatcherKind;
  /** Empty for `any`. */
  value: string;
}

export type ApprovalStatus =
  | 'pending'
  | 'allowed'
  | 'denied'
  | 'stopped'
  | 'expired';
export type ApprovalDecision =
  | 'allow_once'
  | 'allow_session'
  | 'allow_always'
  | 'deny';
export type ApprovalResolvedBy = 'owner' | 'timeout' | 'stop' | 'restart';

export interface ApprovalResolution {
  /** `null` for a stopped or expired request. */
  decision: ApprovalDecision | null;
  note: string | null;
  /** The allowance's or rule's matcher. */
  matcher: ApprovalMatcher | null;
  ruleId: string | null;
  resolvedBy: ApprovalResolvedBy;
  resolvedAtMs: number;
}

/** A tool call waiting for, or decided by, the owner (spec §7.3). */
export interface Approval {
  id: string;
  agentId: string;
  sessionId: string;
  runId: string;
  toolCallId: string;
  tool: string;
  class: RiskClass;
  /** The call's arguments as JSON text, at most 16 KiB. The model wrote
   *  them: show them as text, never as markup. */
  arguments: string;
  argumentsTruncated: boolean;
  suggestedMatcher: ApprovalMatcher;
  /** The matcher kinds an allowance or rule for this tool may use. */
  matcherKinds: ApprovalMatcherKind[];
  createdAtMs: number;
  expiresAtMs: number;
  status: ApprovalStatus;
  /** Send it back with a decision. */
  revision: number;
  resolution: ApprovalResolution | null;
}

export interface ApprovalRule {
  id: string;
  agentId: string;
  tool: string;
  matcher: ApprovalMatcher;
  createdAtMs: number;
  /** The approval whose "Always allow" created it. */
  fromApprovalId: string | null;
}

/** A tool a rule can cover. */
export interface ApprovalTool {
  name: string;
  class: PolicyClass;
  matcherKinds: ApprovalMatcherKind[];
}

export interface ApprovalRules {
  rules: ApprovalRule[];
  tools: ApprovalTool[];
}

export interface ApprovalPage {
  approvals: Approval[];
  /** `decided` only: pass it as `cursor` for the next, older page. */
  nextCursor: string | null;
}

export interface ApprovalListOptions {
  /** `pending`: oldest first. `decided`: the last 30 days, newest first. */
  status: 'pending' | 'decided';
  agentId?: string;
  cursor?: string;
  /** `decided` only: 1–100; the daemon default is 50. */
  limit?: number;
  signal?: AbortSignal;
}

export interface ApprovalDecisionInput {
  decision: ApprovalDecision;
  /** At most `MAX_APPROVAL_NOTE_CHARS`; a denial's reaches the model. */
  note?: string;
  /** For `allow_session` and `allow_always`; the suggestion when absent. */
  matcher?: ApprovalMatcher;
  revision: number;
}

export interface ApprovalRuleInput {
  tool: string;
  matcher: ApprovalMatcher;
}

/** The longest note the daemon keeps (spec §16). */
export const MAX_APPROVAL_NOTE_CHARS = 1_000;

// The daemon refuses unknown keys, so bodies carry only the known fields.
function matcherBody({ kind, value }: ApprovalMatcher): ApprovalMatcher {
  return { kind, value };
}

function agentPath(agentId: string): string {
  return `/api/agents/${encodeURIComponent(agentId)}`;
}

export class ApprovalsClient {
  constructor(private readonly client: DaemonClient) {}

  async list(options: ApprovalListOptions): Promise<ApprovalPage> {
    const search = new URLSearchParams({ status: options.status });
    if (options.agentId) search.set('agentId', options.agentId);
    if (options.cursor) search.set('cursor', options.cursor);
    if (options.limit !== undefined) search.set('limit', String(options.limit));
    return this.client.requestJson<ApprovalPage>(
      `/api/approvals?${search.toString()}`,
      { signal: options.signal },
    );
  }

  /** The owner's decision (spec §7.3); the same decision again returns the
   *  approval unchanged. */
  async decide(
    approvalId: string,
    input: ApprovalDecisionInput,
  ): Promise<Approval> {
    const response = await this.client.requestJson<{ approval: Approval }>(
      `/api/approvals/${encodeURIComponent(approvalId)}/decision`,
      {
        method: 'POST',
        body: {
          decision: input.decision,
          note: input.note,
          matcher: input.matcher && matcherBody(input.matcher),
          revision: input.revision,
        },
      },
    );
    return response.approval;
  }

  async policy(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<ApprovalPolicy> {
    const response = await this.client.requestJson<{
      policy: ApprovalPolicy;
    }>(`${agentPath(agentId)}/approval-policy`, { signal: options.signal });
    return response.policy;
  }

  async setPolicy(
    agentId: string,
    policy: ApprovalPolicy,
  ): Promise<ApprovalPolicy> {
    const response = await this.client.requestJson<{
      policy: ApprovalPolicy;
    }>(`${agentPath(agentId)}/approval-policy`, {
      method: 'PUT',
      body: {
        write: policy.write,
        exec: policy.exec,
        network: policy.network,
        delegate: policy.delegate,
      },
    });
    return response.policy;
  }

  async rules(
    agentId: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<ApprovalRules> {
    return this.client.requestJson<ApprovalRules>(
      `${agentPath(agentId)}/approval-rules`,
      { signal: options.signal },
    );
  }

  async addRule(
    agentId: string,
    input: ApprovalRuleInput,
  ): Promise<ApprovalRule> {
    const response = await this.client.requestJson<{ rule: ApprovalRule }>(
      `${agentPath(agentId)}/approval-rules`,
      {
        method: 'POST',
        body: { tool: input.tool, matcher: matcherBody(input.matcher) },
      },
    );
    return response.rule;
  }

  async removeRule(agentId: string, ruleId: string): Promise<void> {
    await this.client.requestJson<{ deleted: boolean }>(
      `${agentPath(agentId)}/approval-rules/${encodeURIComponent(ruleId)}`,
      { method: 'DELETE' },
    );
  }
}
