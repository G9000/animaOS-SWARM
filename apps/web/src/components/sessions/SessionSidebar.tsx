import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { Session, SessionKind } from '@animaOS-SWARM/sdk';

import {
  SESSION_KIND_FILTER_LABELS,
  groupSessions,
  presentKinds,
  sessionKey,
} from '../../lib/session-groups';

/** Typing settles this long before the list is searched. */
export const SESSION_SEARCH_DEBOUNCE_MS = 250;

export interface SessionSidebarProps {
  sessions: readonly Session[];
  /** `sessionKey` of the open session. */
  activeKey: string | null;
  query: string;
  onQueryChange: (query: string) => void;
  showArchived: boolean;
  onShowArchivedChange: (show: boolean) => void;
  error?: string | null;
  now?: Date;
  onOpen: (session: Session) => void;
  onRename: (session: Session, title: string) => Promise<boolean>;
  onArchive: (session: Session, archived: boolean) => Promise<void>;
  onExport: (session: Session) => Promise<void>;
  onDelete: (session: Session) => Promise<void>;
}

type RowActions = Pick<
  SessionSidebarProps,
  'onOpen' | 'onRename' | 'onArchive' | 'onExport' | 'onDelete'
>;

function rowLabel(session: Session): string {
  return [
    session.title,
    session.activeRuns > 0 ? 'working' : null,
    session.unread ? 'unread' : null,
  ]
    .filter(Boolean)
    .join(', ');
}

function SessionRow({
  session,
  active,
  actions,
}: {
  session: Session;
  active: boolean;
  actions: RowActions;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState(session.title);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const closeMenu = () => {
    setMenuOpen(false);
    setConfirmDelete(false);
  };
  // Controller ruling 1 (M2 pre-flight audit): Escape closes the row menu
  // and returns focus to the trigger that opened it.
  const closeMenuToTrigger = () => {
    closeMenu();
    menuTriggerRef.current?.focus();
  };
  const cancelRename = () => {
    setRenaming(false);
    setTitle(session.title);
  };

  return (
    <div
      className="session-row"
      onKeyDown={(event) => {
        if (event.key === 'Escape' && menuOpen) {
          event.stopPropagation();
          closeMenuToTrigger();
        }
      }}
    >
      {renaming ? (
        <form
          className="flex min-w-0 flex-1 items-center gap-1"
          onSubmit={(event) => {
            event.preventDefault();
            void actions.onRename(session, title).then((saved) => {
              if (saved) setRenaming(false);
            });
          }}
        >
          <input
            className="session-rename"
            aria-label={`Rename ${session.title}`}
            value={title}
            maxLength={120}
            autoFocus
            onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                // Handled here: an enclosing drawer stays open.
                event.preventDefault();
                cancelRename();
              }
            }}
          />
          <button type="submit" className="studio-tool-button">
            Save
          </button>
        </form>
      ) : (
        <button
          type="button"
          data-session-row
          className="session-row-button"
          aria-label={rowLabel(session)}
          aria-current={active ? 'page' : undefined}
          title={session.preview ?? session.title}
          onClick={() => actions.onOpen(session)}
        >
          {session.activeRuns > 0 && <span className="session-pulse" aria-hidden />}
          <span className="session-title">{session.title}</span>
          {session.unread && <span className="session-unread-dot" aria-hidden />}
        </button>
      )}
      <button
        type="button"
        ref={menuTriggerRef}
        className="session-row-actions"
        aria-label={`Actions for ${session.title}`}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={() => {
          setConfirmDelete(false);
          setMenuOpen((open) => !open);
        }}
      >
        ⋯
      </button>
      {menuOpen && (
        <div className="session-menu" role="menu" aria-label={`${session.title} actions`}>
          {confirmDelete ? (
            <>
              <p className="px-2 py-1 text-xs text-ink-2">
                Delete “{session.title}”? Its messages are removed; memories are
                kept.
              </p>
              <button
                type="button"
                role="menuitem"
                className="is-danger"
                onClick={() => {
                  closeMenu();
                  void actions.onDelete(session);
                }}
              >
                Delete session
              </button>
              <button type="button" role="menuitem" onClick={closeMenu}>
                Cancel
              </button>
            </>
          ) : (
            <>
              {session.capabilities.rename && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    closeMenu();
                    setTitle(session.title);
                    setRenaming(true);
                  }}
                >
                  Rename
                </button>
              )}
              {session.capabilities.archive && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    closeMenu();
                    void actions.onArchive(session, !session.archived);
                  }}
                >
                  {session.archived ? 'Unarchive' : 'Archive'}
                </button>
              )}
              {session.capabilities.export && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    closeMenu();
                    void actions.onExport(session);
                  }}
                >
                  Export Markdown
                </button>
              )}
              {session.capabilities.delete && (
                <button
                  type="button"
                  role="menuitem"
                  className="is-danger"
                  onClick={() => setConfirmDelete(true)}
                >
                  Delete
                </button>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

function moveBetweenRows(event: KeyboardEvent<HTMLDivElement>) {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
  const rows = Array.from(
    event.currentTarget.querySelectorAll<HTMLButtonElement>('[data-session-row]'),
  );
  const index = rows.findIndex((row) => row === document.activeElement);
  if (index === -1) return;
  event.preventDefault();
  const next = index + (event.key === 'ArrowDown' ? 1 : -1);
  rows[Math.min(rows.length - 1, Math.max(0, next))]?.focus();
}

/** The sessions list of the sidebar and the mobile drawer (spec §15.1). */
export function SessionSidebar({
  sessions,
  activeKey,
  query,
  onQueryChange,
  showArchived,
  onShowArchivedChange,
  error = null,
  now,
  ...actions
}: SessionSidebarProps) {
  const [text, setText] = useState(query);
  const [kind, setKind] = useState<SessionKind | null>(null);
  useEffect(() => {
    setText(query);
  }, [query]);
  useEffect(() => {
    if (text === query) return;
    const timer = window.setTimeout(
      () => onQueryChange(text),
      SESSION_SEARCH_DEBOUNCE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [text, query, onQueryChange]);

  const kinds = presentKinds(sessions);
  const activeKind = kind && kinds.includes(kind) ? kind : null;
  const visible = activeKind
    ? sessions.filter((session) => session.kind === activeKind)
    : sessions;
  const groups = groupSessions(visible, now ?? new Date(), activeKind === null);

  return (
    <nav className="session-sidebar" aria-label="Sessions">
      <input
        type="search"
        className="session-search"
        aria-label="Search sessions"
        placeholder="Search chats…"
        value={text}
        onChange={(event) => setText(event.target.value)}
      />
      {kinds.length > 1 && (
        <div className="session-chips" role="group" aria-label="Session kinds">
          <button
            type="button"
            className="session-chip"
            aria-pressed={activeKind === null}
            onClick={() => setKind(null)}
          >
            All
          </button>
          {kinds.map((item) => (
            <button
              key={item}
              type="button"
              className="session-chip"
              aria-pressed={activeKind === item}
              onClick={() => setKind(item)}
            >
              {SESSION_KIND_FILTER_LABELS[item]}
            </button>
          ))}
        </div>
      )}
      <div className="session-list" onKeyDown={moveBetweenRows}>
        {groups.length === 0 ? (
          <p className="px-2 py-3 text-xs text-ink-3">
            {query.trim()
              ? 'No sessions match.'
              : showArchived
                ? 'No archived sessions.'
                : 'No chats yet.'}
          </p>
        ) : (
          groups.map((group) => (
            <div key={group.label} role="group" aria-label={group.label}>
              <p className="session-group-label" aria-hidden>
                {group.label}
              </p>
              <ul>
                {group.nodes.map(({ session, helpers }) => (
                  <li key={sessionKey(session)}>
                    <SessionRow
                      session={session}
                      active={activeKey === sessionKey(session)}
                      actions={actions}
                    />
                    {helpers.length > 0 && (
                      <ul
                        className="session-children"
                        aria-label={`Helpers of ${session.title}`}
                      >
                        {helpers.map((helper) => (
                          <li key={sessionKey(helper)}>
                            <SessionRow
                              session={helper}
                              active={activeKey === sessionKey(helper)}
                              actions={actions}
                            />
                          </li>
                        ))}
                      </ul>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          ))
        )}
      </div>
      <button
        type="button"
        className="session-archived-toggle"
        aria-pressed={showArchived}
        onClick={() => onShowArchivedChange(!showArchived)}
      >
        {showArchived ? 'Hide archived' : 'Show archived'}
      </button>
      {error && (
        <p role="alert" className="px-2 text-xs text-danger">
          {error}
        </p>
      )}
    </nav>
  );
}
