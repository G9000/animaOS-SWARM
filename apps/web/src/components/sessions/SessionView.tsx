import {
  useEffect,
  useMemo,
  useState,
  type ReactNode,
  type RefObject,
} from 'react';
import type { Session } from '@animaOS-SWARM/sdk';

import { SESSION_KIND_LABELS } from '../../lib/session-groups';
import type { AgentDetail, ChatMessage } from '../../lib/types';
import { Composer, MessageList } from '../ChatScreen';
import { ghostBtnCls } from '../ui-bits';

export interface SessionComposerState {
  draft: string;
  setDraft: (value: string) => void;
  sending: boolean;
  disabled: boolean;
  offline: boolean;
  onSend: () => void;
  error: string | null;
  onDismissError: () => void;
  recovery?: {
    count: number;
    text: string;
    restore: () => void;
    dismiss: () => void;
  };
}

export interface SessionViewProps {
  agent: AgentDetail;
  /** null for a new chat that has no session yet. */
  session: Session | null;
  messages: ChatMessage[];
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
  notice?: ReactNode;
}

export type SessionFooter =
  | { kind: 'composer'; label?: string }
  | { kind: 'note'; text: string; action?: 'new-chat' | 'work' };

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
      return {
        kind: 'note',
        text: 'Replying to a check-in is not available yet. Start a new chat to follow up.',
        action: 'new-chat',
      };
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
}: {
  session: Session;
  onRename: (title: string) => Promise<boolean>;
  onToggleArchived: () => void;
  onExport: () => void;
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
      </span>
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
    </header>
  );
}

/** One session (or a new chat) on today's blocking run route (spec §15.2). */
export function SessionView({
  agent,
  session,
  messages,
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
  notice = null,
}: SessionViewProps) {
  const conversation = useMemo(
    () => ({ ...agent, messages }),
    [agent, messages],
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
        />
      ) : null}
      {notice}
      <MessageList
        agent={conversation}
        sending={composer.sending || (session?.activeRuns ?? 0) > 0}
        scrollerRef={scrollerRef}
        onSuggestion={onSuggestion}
        hasOlder={hasOlder}
        loadingOlder={loadingOlder}
        onLoadOlder={onLoadOlder}
        emptyState={
          session && session.kind !== 'chat' ? (
            <p className="session-footer-note">No messages yet.</p>
          ) : undefined
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
        />
      ) : (
        <div className="session-footer-note" role="note">
          <p>{footer.text}</p>
          {footer.action === 'new-chat' && (
            <button type="button" className={ghostBtnCls} onClick={onNewChat}>
              Start a new chat
            </button>
          )}
          {footer.action === 'work' && (
            <button type="button" className={ghostBtnCls} onClick={onOpenWork}>
              Open Work
            </button>
          )}
        </div>
      )}
    </section>
  );
}
