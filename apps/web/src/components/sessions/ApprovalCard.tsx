import { useId, useState, type ReactNode } from 'react';
import {
  MAX_APPROVAL_NOTE_CHARS,
  type Approval,
  type ApprovalDecision,
  type ApprovalMatcher,
  type ApprovalMatcherKind,
} from '@animaOS-SWARM/sdk';

import {
  CLASS_LABELS,
  DECISION_LABELS,
  MATCHER_KIND_LABELS,
  MAX_MATCHER_VALUE_CHARS,
  describeMatcher,
  isBroadExecMatcher,
  revealHiddenCharacters,
  type ApprovalDecide,
} from '../../lib/approvals';
import { formatTime } from '../ui-bits';

const DECISIONS: readonly ApprovalDecision[] = [
  'allow_once',
  'allow_session',
  'allow_always',
  'deny',
];

const SENT: Record<ApprovalDecision, string> = {
  allow_once: 'Allowed once. Continuing…',
  allow_session: 'Allowed for this session. Continuing…',
  allow_always: 'Always allowed. Continuing…',
  deny: 'Denied. The companion carries on without it.',
};

function isScoped(decision: ApprovalDecision): boolean {
  return decision === 'allow_session' || decision === 'allow_always';
}

function MatcherEditor({
  kinds,
  matcher,
  onChange,
  onDone,
}: {
  kinds: readonly ApprovalMatcherKind[];
  matcher: ApprovalMatcher;
  onChange: (matcher: ApprovalMatcher) => void;
  onDone: () => void;
}) {
  return (
    <fieldset className="approval-card-matcher">
      <legend className="approval-card-label">
        What “for this session” and “always” cover
      </legend>
      <select
        aria-label="Match by"
        className="approval-card-input"
        value={matcher.kind}
        onChange={(event) => {
          const kind = event.target.value as ApprovalMatcherKind;
          onChange({ kind, value: kind === 'any' ? '' : matcher.value });
        }}
      >
        {kinds.map((kind) => (
          <option key={kind} value={kind}>
            {MATCHER_KIND_LABELS[kind]}
          </option>
        ))}
      </select>
      {matcher.kind !== 'any' && (
        <input
          aria-label="Match value"
          className="approval-card-input"
          maxLength={MAX_MATCHER_VALUE_CHARS}
          value={matcher.value}
          onChange={(event) =>
            onChange({ ...matcher, value: event.target.value })
          }
        />
      )}
      <button type="button" className="studio-tool-button" onClick={onDone}>
        Done
      </button>
    </fieldset>
  );
}

/**
 * A call waiting for the owner (spec §7.3, §15.2): what it wants to do,
 * shown as plain text, and the four decisions with an optional note and an
 * editable scope for "for this session" and "always".
 */
export function ApprovalCard({
  approval,
  onDecide,
  context,
  canPersist = false,
}: {
  approval: Approval;
  onDecide?: ApprovalDecide;
  /** Shown in the header, such as the session the call came from. */
  context?: ReactNode;
  /** Offer "Always allow": only for the companion's calls; another
   *  agent's rules are not managed in this console yet. */
  canPersist?: boolean;
}) {
  const [note, setNote] = useState('');
  const [matcher, setMatcher] = useState<ApprovalMatcher>(
    approval.suggestedMatcher,
  );
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState<ApprovalDecision | null>(null);
  const [sent, setSent] = useState<ApprovalDecision | null>(null);
  const [error, setError] = useState<string | null>(null);
  const noteId = useId();
  const hintId = useId();

  // The owner cannot scope a rule on arguments they cannot fully see, and
  // the daemon rejects a scope with no value.
  const scopeEmpty = matcher.kind !== 'any' && matcher.value.trim() === '';
  const scopeBlocked = approval.argumentsTruncated || scopeEmpty;
  const broad = isBroadExecMatcher(approval.class, matcher);

  const decide = async (decision: ApprovalDecision) => {
    if (!onDecide || busy) return;
    setBusy(decision);
    setError(null);
    const trimmed = note.trim();
    const failure = await onDecide(approval, {
      decision,
      ...(trimmed ? { note: trimmed } : {}),
      ...(isScoped(decision) ? { matcher } : {}),
    });
    setBusy(null);
    if (failure) setError(failure);
    else setSent(decision);
  };

  const decisions = DECISIONS.filter(
    (decision) => canPersist || decision !== 'allow_always',
  );

  return (
    <section
      className="approval-card"
      aria-label={`Approval needed: ${approval.tool}`}
    >
      <header className="approval-card-header">
        <strong className="approval-card-tool">
          {revealHiddenCharacters(approval.tool)}
        </strong>
        <span className="approval-card-class">
          {CLASS_LABELS[approval.class]}
        </span>
        {context}
      </header>
      <p className="approval-card-lead">This call waits for your approval.</p>
      {/* The model wrote these: a text node, never markup (spec §14). */}
      <pre className="approval-card-arguments" aria-label="Arguments">
        {revealHiddenCharacters(approval.arguments)}
      </pre>
      {approval.argumentsTruncated && (
        <p className="approval-card-note">Arguments shortened to 16 KiB.</p>
      )}
      {sent ? (
        <p className="approval-card-sent" role="status">
          {SENT[sent]}
        </p>
      ) : (
        <>
          <label className="approval-card-label" htmlFor={noteId}>
            Note (optional)
          </label>
          <textarea
            id={noteId}
            className="approval-card-input"
            rows={2}
            maxLength={MAX_APPROVAL_NOTE_CHARS}
            value={note}
            onChange={(event) => setNote(event.target.value)}
          />
          {editing ? (
            <MatcherEditor
              kinds={approval.matcherKinds}
              matcher={matcher}
              onChange={setMatcher}
              onDone={() => setEditing(false)}
            />
          ) : (
            <p className="approval-card-scope">
              For this session or always:{' '}
              {scopeEmpty
                ? 'choose what it covers'
                : describeMatcher(approval.tool, matcher)}{' '}
              <button
                type="button"
                className="studio-tool-button"
                onClick={() => setEditing(true)}
              >
                Edit
              </button>
            </p>
          )}
          {approval.argumentsTruncated && (
            <p id={hintId} className="approval-card-note">
              Arguments were cut; only Allow once or Deny.
            </p>
          )}
          {broad && (
            <p className="approval-card-warning">
              A rule this broad lets your companion run almost anything,
              including changing its own approval settings.
            </p>
          )}
          {!canPersist && (
            <p className="approval-card-note">
              Rules for other agents are not managed here yet.
            </p>
          )}
          <div className="approval-card-actions">
            {decisions.map((decision) => (
              <button
                key={decision}
                type="button"
                className={
                  decision === 'deny'
                    ? 'studio-tool-button approval-card-deny'
                    : 'studio-tool-button'
                }
                disabled={
                  !onDecide ||
                  busy !== null ||
                  (isScoped(decision) && scopeBlocked)
                }
                aria-describedby={
                  isScoped(decision) && approval.argumentsTruncated
                    ? hintId
                    : undefined
                }
                onClick={() => void decide(decision)}
              >
                {DECISION_LABELS[decision]}
              </button>
            ))}
          </div>
          {error && (
            <p className="approval-card-error" role="alert">
              {error}
            </p>
          )}
        </>
      )}
      <p className="approval-card-expiry">
        Waits until {formatTime(approval.expiresAtMs)}
      </p>
    </section>
  );
}
