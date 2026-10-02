import { useState, type FormEvent } from 'react';
import {
  DEFAULT_APPROVAL_POLICY,
  POLICY_CLASSES,
  type Approval,
  type ApprovalMatcherKind,
  type ApprovalPolicyAction,
  type PolicyClass,
  type Session,
} from '@animaOS-SWARM/sdk';

import { ApprovalCard } from '../components/sessions/ApprovalCard';
import { useApprovals, type ApprovalsView } from '../hooks/useApprovals';
import {
  CLASS_LABELS,
  MATCHER_KIND_LABELS,
  MAX_MATCHER_VALUE_CHARS,
  approvalOutcome,
  decideApproval,
  describeMatcher,
  formatWhen,
  isBroadExecMatcher,
  resolvedAt,
  type ApprovalDecide,
} from '../lib/approvals';
import type { LiveState } from '../lib/session-events';
import { sessionKey } from '../lib/session-groups';

const ACTIONS: readonly ApprovalPolicyAction[] = ['allow', 'ask', 'deny'];
const ACTION_LABELS: Record<ApprovalPolicyAction, string> = {
  allow: 'Allow',
  ask: 'Ask me first',
  deny: 'Deny',
};

function ClassRules({
  klass,
  view,
}: {
  klass: PolicyClass;
  view: ApprovalsView;
}) {
  const label = CLASS_LABELS[klass];
  const tools = view.tools.filter((tool) => tool.class === klass);
  const rules = view.rules.filter((rule) =>
    tools.some((tool) => tool.name === rule.tool),
  );
  const [toolName, setToolName] = useState('');
  const [kind, setKind] = useState<ApprovalMatcherKind | null>(null);
  const [value, setValue] = useState('');
  const tool = tools.find((item) => item.name === toolName) ?? tools[0];
  const matcherKind =
    tool && kind && tool.matcherKinds.includes(kind)
      ? kind
      : tool?.matcherKinds[0];

  const broad =
    !!tool &&
    !!matcherKind &&
    isBroadExecMatcher(tool.class, {
      kind: matcherKind,
      value: matcherKind === 'any' ? '' : value.trim(),
    });

  const add = async (event: FormEvent) => {
    event.preventDefault();
    if (!tool || !matcherKind) return;
    const kept = await view.addRule(tool.name, {
      kind: matcherKind,
      value: matcherKind === 'any' ? '' : value.trim(),
    });
    if (kept) setValue('');
  };

  return (
    <section className="approvals-class" aria-label={label}>
      <h3>{label}</h3>
      <label className="approvals-policy">
        <span>When one of these tools runs</span>
        <select
          aria-label={`Policy for ${label}`}
          value={view.policy?.[klass] ?? DEFAULT_APPROVAL_POLICY[klass]}
          disabled={view.policy === null}
          onChange={(event) =>
            void view.setPolicyAction(
              klass,
              event.target.value as ApprovalPolicyAction,
            )
          }
        >
          {ACTIONS.map((action) => (
            <option key={action} value={action}>
              {ACTION_LABELS[action]}
            </option>
          ))}
        </select>
      </label>
      {rules.length === 0 ? (
        <p className="approvals-empty">No rules yet.</p>
      ) : (
        <ul className="approvals-rules">
          {rules.map((rule) => {
            const covers = describeMatcher(rule.tool, rule.matcher);
            return (
              <li key={rule.id}>
                <span>Always allow {covers}</span>
                <button
                  type="button"
                  className="studio-tool-button"
                  aria-label={`Remove rule: ${covers}`}
                  onClick={() => void view.removeRule(rule)}
                >
                  Remove
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {tool && matcherKind && (
        <form
          className="approvals-add-rule"
          aria-label={`Add a rule for ${label}`}
          onSubmit={(event) => void add(event)}
        >
          <select
            aria-label="Tool"
            value={tool.name}
            onChange={(event) => setToolName(event.target.value)}
          >
            {tools.map((item) => (
              <option key={item.name} value={item.name}>
                {item.name}
              </option>
            ))}
          </select>
          <select
            aria-label="Match by"
            value={matcherKind}
            onChange={(event) =>
              setKind(event.target.value as ApprovalMatcherKind)
            }
          >
            {tool.matcherKinds.map((item) => (
              <option key={item} value={item}>
                {MATCHER_KIND_LABELS[item]}
              </option>
            ))}
          </select>
          {matcherKind !== 'any' && (
            <input
              aria-label="Match value"
              value={value}
              maxLength={MAX_MATCHER_VALUE_CHARS}
              onChange={(event) => setValue(event.target.value)}
            />
          )}
          <button
            type="submit"
            className="studio-tool-button"
            disabled={matcherKind !== 'any' && !value.trim()}
          >
            Add rule
          </button>
          {broad && (
            <p className="approval-card-warning">
              A rule this broad lets your companion run almost anything,
              including changing its own approval settings.
            </p>
          )}
        </form>
      )}
    </section>
  );
}

export interface ApprovalsPageProps {
  agentId: string;
  /** The companion's live stream state. */
  live: LiveState;
  streamOpen: boolean;
  /** The sessions list, for each card's "Open" button. */
  sessions: readonly Session[];
  onOpenSession: (approval: Approval) => void;
}

/** Spec §15.4: pending cards, rules and policy per class, and the 30-day
 *  decided history. */
export function ApprovalsPage({
  agentId,
  live,
  streamOpen,
  sessions,
  onOpenSession,
}: ApprovalsPageProps) {
  const view = useApprovals({
    agentId,
    streamApprovals: live.approvals,
    streamOpen,
    epoch: live.epoch,
  });
  const titles = new Map(
    sessions.map((session) => [sessionKey(session), session.title]),
  );
  const decide: ApprovalDecide = async (approval, input) => {
    const failure = await decideApproval(approval, input);
    // Without the stream nothing else tells this page it went through.
    if (!failure && !streamOpen) view.refresh();
    return failure;
  };

  return (
    <div className="approvals-page">
      {view.error && (
        <p className="approvals-error" role="alert">
          {view.error}
        </p>
      )}
      <section
        className="approvals-section"
        aria-labelledby="approvals-waiting"
      >
        <h2 id="approvals-waiting">Waiting for you</h2>
        {view.pending.length === 0 ? (
          <p className="approvals-empty">
            Nothing is waiting for your approval.
          </p>
        ) : (
          <div className="approvals-cards">
            {view.pending.map((approval) => (
              <ApprovalCard
                key={approval.id}
                approval={approval}
                onDecide={decide}
                context={
                  <button
                    type="button"
                    className="studio-tool-button"
                    onClick={() => onOpenSession(approval)}
                  >
                    Open{' '}
                    {titles.get(
                      sessionKey({
                        agentId: approval.agentId,
                        id: approval.sessionId,
                      }),
                    ) ?? 'chat'}
                  </button>
                }
              />
            ))}
          </div>
        )}
      </section>
      <section className="approvals-section" aria-labelledby="approvals-rules">
        <h2 id="approvals-rules">Rules</h2>
        <p className="approvals-help">
          Tools that only read never ask. For the others, choose what happens,
          and which calls are always allowed.
        </p>
        {POLICY_CLASSES.map((klass) => (
          <ClassRules key={klass} klass={klass} view={view} />
        ))}
      </section>
      <section
        className="approvals-section"
        aria-labelledby="approvals-decided"
      >
        <h2 id="approvals-decided">Decided in the last 30 days</h2>
        {view.decided.length === 0 ? (
          <p className="approvals-empty">No decisions in the last 30 days.</p>
        ) : (
          <ul className="approvals-decided">
            {view.decided.map((approval) => (
              <li key={approval.id}>
                <strong>{approval.tool}</strong>
                <span>{approvalOutcome(approval)}</span>
                {approval.resolution?.note && (
                  <span className="approvals-note">
                    “{approval.resolution.note}”
                  </span>
                )}
                <time dateTime={new Date(resolvedAt(approval)).toISOString()}>
                  {formatWhen(resolvedAt(approval))}
                </time>
              </li>
            ))}
          </ul>
        )}
        {view.hasMoreDecided && (
          <button
            type="button"
            className="studio-tool-button"
            onClick={view.loadMoreDecided}
          >
            Show older
          </button>
        )}
      </section>
    </div>
  );
}
