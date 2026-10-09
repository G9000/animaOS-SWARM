import {
  useEffect,
  useMemo,
  useState,
  type ReactNode,
  type RefObject,
} from 'react';
import {
  isTerminalRunStatus,
  type Automation,
  type Session,
} from '@animaOS-SWARM/sdk';

import { useSaveToMemory } from '../../hooks/useSaveToMemory';
import { useSessionUsage } from '../../hooks/useSessionUsage';
import { describeTrigger } from '../../lib/automations';
import type { LiveRun } from '../../lib/session-events';
import { SESSION_KIND_LABELS } from '../../lib/session-groups';
import { revealInvisible } from '../../lib/skills';
import type { SlashCommand } from '../../lib/slash-commands';
import { sessionUsageLine } from '../../lib/usage';
import type { AgentDetail, ChatMessage } from '../../lib/types';
import { Composer, MessageList } from '../ChatScreen';
import {
  buildHistory,
  placeRuns,
  type PendingBubble,
  type TranscriptActions,
} from '../../lib/transcript';
import { ghostBtnCls } from '../ui-bits';

export interface SessionComposerState {
  draft: string;
  setDraft: (value: string) => void;
  sending: boolean;
  disabled: boolean;
  offline: boolean;
  onSend: (text?: string) => void;
  error: string | null;
  onDismissError: () => void;
  recovery?: {
    count: number;
    text: string;
    restore: () => void;
    dismiss: () => void;
  };
  commands?: readonly SlashCommand[];
  /** This session's reply is in progress (spec §15.3). */
  runActive?: boolean;
  onStop?: () => void;
  onSteer?: () => void;
}

export interface SessionViewProps {
  agent: AgentDetail;
  /** null for a new chat that has no session yet. */
  session: Session | null;
  messages: ChatMessage[];
  /** Messages on their way to the daemon (spec §15.5). */
  pending?: readonly PendingBubble[];
  /** The session's runs from its stream and ledger (spec §15.2). */
  runs?: readonly LiveRun[];
  actions?: TranscriptActions;
  /** Set for helper sessions: who wrote their user turns. */
  delegatedBy?: string | null;
  /** Read politely to screen readers when a reply finishes (spec §15.5). */
  announcement?: string;
  hasOlder: boolean;
  loadingOlder: boolean;
  onLoadOlder: () => void;
  /** The session was deleted elsewhere. */
  missing: boolean;
  telegramAvailable: boolean;
  scrollerRef: RefObject<HTMLDivElement | null>;
  onSuggestion: (text: string) => void;
  composer: SessionComposerState;
  onNewChat: () => void;
  onOpenWork: () => void;
  onRename: (title: string) => Promise<boolean>;
  onToggleArchived: () => void;
  onExport: () => void;
  /** A check-in's automation, for its header (spec §15.2). */
  automation?: Automation | null;
  /** Opens the automation's editor on the Automations page. */
  onEditAutomation?: (automation: Automation) => void;
  notice?: ReactNode;
}

export type SessionFooter =
  | { kind: 'composer'; label?: string }
  | { kind: 'note'; text: string; action?: 'work' };

/** What replaces the composer for each kind (spec §3.2, §15.2). */
export function sessionFooter(
  session: Session | null,
  telegramAvailable: boolean,
): SessionFooter {
  if (!session) return { kind: 'composer' };
  switch (session.kind) {
    case 'chat':
      return { kind: 'composer' };
    case 'telegram':
      return telegramAvailable
        ? { kind: 'composer', label: 'Reply on Telegram' }
        : {
            kind: 'note',
            text: 'This Telegram connection is not available. Reconnect it in Connectors to reply.',
          };
    case 'checkin':
      return { kind: 'composer', label: 'Reply to this check-in' };
    case 'job':
      return {
        kind: 'note',
        text: 'Job sessions are read-only. Follow the job in Work.',
        action: 'work',
      };
    case 'helper':
      return { kind: 'note', text: 'Helper sessions are read-only.' };
  }
}

function SessionHeader({
  session,
  onRename,
  onToggleArchived,
  onExport,
  automation,
  onEditAutomation,
  usageLine,
}: {
  session: Session;
  onRename: (title: string) => Promise<boolean>;
  onToggleArchived: () => void;
  onExport: () => void;
  automation: Automation | null;
  onEditAutomation?: (automation: Automation) => void;
  /** Tokens and cost so far, e.g. '12.3k tokens · $0.04'. */
  usageLine: string | null;
}) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(session.title);
  useEffect(() => {
    if (!editing) setTitle(session.title);
  }, [editing, session.title]);
  const cancel = () => {
    setEditing(false);
    setTitle(session.title);
  };

  return (
    <header className="session-view-header">
      {editing ? (
        <form
          className="flex min-w-0 flex-1 items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void onRename(title).then((saved) => {
              if (saved) setEditing(false);
            });
          }}
        >
          <input
            className="session-rename"
            aria-label="Session title"
            value={title}
            maxLength={120}
            autoFocus
            onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') cancel();
            }}
          />
          <button type="submit" className={ghostBtnCls}>
            Save
          </button>
          <button type="button" className={ghostBtnCls} onClick={cancel}>
            Cancel
          </button>
        </form>
      ) : (
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold text-ink">
          {session.title}
        </h2>
      )}
      <span className="session-kind-badge">
        {SESSION_KIND_LABELS[session.kind]}
        {automation
          ? ` · ${revealInvisible(describeTrigger(automation.trigger)).text}`
          : ''}
      </span>
      {usageLine && <span className="session-usage-line">{usageLine}</span>}
      {!editing && session.capabilities.rename && (
        <button
          type="button"
          className={ghostBtnCls}
          onClick={() => setEditing(true)}
        >
          Rename
        </button>
      )}
      {session.capabilities.archive && (
        <button
          type="button"
          className={ghostBtnCls}
          onClick={onToggleArchived}
        >
          {session.archived ? 'Unarchive' : 'Archive'}
        </button>
      )}
      {session.capabilities.export && (
        <button type="button" className={ghostBtnCls} onClick={onExport}>
          Export
        </button>
      )}
      {automation && onEditAutomation && (
        <button
          type="button"
          className={ghostBtnCls}
          onClick={() => onEditAutomation(automation)}
        >
          Edit automation
        </button>
      )}
    </header>
  );
}

const EMPTY_PENDING: readonly PendingBubble[] = [];
const EMPTY_RUNS: readonly LiveRun[] = [];
// One element, so a render of the view keeps the message list's props.
const NO_MESSAGES = <p className="session-footer-note">No messages yet.</p>;

/** The name saved with a memory when the agent's own name is not at hand. */
const GENERIC_SAVE_AGENT_NAME = 'Helper agent';

/** One session (or a new chat): its transcript and composer (spec §15.2). */
export function SessionView({
  agent,
  session,
  messages,
  pending = EMPTY_PENDING,
  runs = EMPTY_RUNS,
  actions,
  delegatedBy = null,
  announcement = '',
  hasOlder,
  loadingOlder,
  onLoadOlder,
  missing,
  telegramAvailable,
  scrollerRef,
  onSuggestion,
  composer,
  onNewChat,
  onOpenWork,
  onRename,
  onToggleArchived,
  onExport,
  automation = null,
  onEditAutomation,
  notice = null,
}: SessionViewProps) {
  // The session record names its agent, so a helper's session saves to that
  // helper even when the agents list does not hold it (then `agent` is the
  // companion); the name is the agent's only when the ids match.
  const saveAgentId = session?.agentId ?? null;
  const saveAgentName =
    saveAgentId === agent.id ? agent.name : GENERIC_SAVE_AGENT_NAME;
  const sessionId = session?.id ?? null;
  const saveTarget = useMemo(
    () =>
      saveAgentId && sessionId
        ? { agentId: saveAgentId, agentName: saveAgentName, sessionId }
        : null,
    [saveAgentId, saveAgentName, sessionId],
  );
  const { save, savedState } = useSaveToMemory(saveTarget);
  // The usage line reads again when a run finishes.
  const finishedRuns = runs.filter((item) =>
    isTerminalRunStatus(item.run.status),
  ).length;
  const usage = useSessionUsage(saveTarget, finishedRuns);
  const transcriptActions = useMemo<TranscriptActions>(
    () => ({ ...actions, onSaveToMemory: save, savedToMemory: savedState }),
    [actions, save, savedState],
  );
  const conversation = useMemo(
    () => ({ ...agent, messages }),
    [agent, messages],
  );
  const trimmedThrough =
    session?.contextTrimmed?.droppedThroughMessageId ?? null;
  // History is built apart from the runs and sends, so a streamed delta
  // rebuilds only what it changes and history items keep their identity.
  const history = useMemo(
    () => buildHistory({ messages, trimmedThrough, delegatedBy }),
    [messages, trimmedThrough, delegatedBy],
  );
  const items = useMemo(
    () => placeRuns(history, { runs, pending, olderHistory: hasOlder }),
    [history, runs, pending, hasOlder],
  );
  if (missing) {
    return (
      <section
        className="flex h-full min-h-0 flex-col items-center justify-center gap-3 p-6 text-center"
        aria-label="Session"
      >
        <p className="text-sm text-ink-2">This session was deleted.</p>
        <button type="button" className={ghostBtnCls} onClick={onNewChat}>
          Start a new chat
        </button>
      </section>
    );
  }
  // A run the transcript shows speaks for itself; the thinking indicator
  // covers active runs it does not know about (no stream).
  const thinking =
    composer.sending ||
    ((session?.activeRuns ?? 0) > 0 &&
      !runs.some((item) => !isTerminalRunStatus(item.run.status)));
  const footer = sessionFooter(session, telegramAvailable);
  return (
    <section
      className="flex h-full min-h-0 flex-col"
      aria-label={session?.title ?? 'New chat'}
    >
      {session ? (
        <SessionHeader
          session={session}
          onRename={onRename}
          onToggleArchived={onToggleArchived}
          onExport={onExport}
          automation={automation}
          onEditAutomation={onEditAutomation}
          usageLine={sessionUsageLine(usage)}
        />
      ) : null}
      {notice}
      {/* Mounted empty and filled when an error arrives, so it is
       *  announced (S3b-E). */}
      <div role="status">
        {session?.compactionError ? (
          <p className="px-4 pt-3 text-xs text-ink-3">
            Earlier messages could not be summarized:{' '}
            {session.compactionError.message}
          </p>
        ) : null}
      </div>
      <MessageList
        agent={conversation}
        items={items}
        actions={transcriptActions}
        sending={thinking}
        scrollerRef={scrollerRef}
        onSuggestion={onSuggestion}
        hasOlder={hasOlder}
        loadingOlder={loadingOlder}
        onLoadOlder={onLoadOlder}
        emptyState={
          session && session.kind !== 'chat' ? NO_MESSAGES : undefined
        }
      />
      {footer.kind === 'composer' ? (
        <Composer
          agentName={agent.name}
          label={footer.label}
          draft={composer.draft}
          setDraft={composer.setDraft}
          sending={composer.sending}
          disabled={composer.disabled}
          offline={composer.offline}
          onSend={composer.onSend}
          error={composer.error}
          onDismissError={composer.onDismissError}
          recovery={composer.recovery}
          commands={composer.commands}
          runActive={composer.runActive}
          onStop={composer.onStop}
          onSteer={composer.onSteer}
        />
      ) : (
        <div className="session-footer-note" role="note">
          <p>{footer.text}</p>
          {footer.action === 'work' && (
            <button type="button" className={ghostBtnCls} onClick={onOpenWork}>
              Open Work
            </button>
          )}
        </div>
      )}
      <p className="sr-only" aria-live="polite" aria-atomic="true">
        {announcement}
      </p>
    </section>
  );
}
