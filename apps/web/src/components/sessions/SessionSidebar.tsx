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
  /** Whether an older page (`useCompanionSessions`' `loadMore`) remains. */
  hasMore?: boolean;
  loadingMore?: boolean;
  onLoadMore?: () => void;
  onOpen: (session: Session) => void;
  onRename: (session: Session, title: string) => Promise<boolean>;
  /** Resolves true once the change is saved, false when it failed. */
  onArchive: (session: Session, archived: boolean) => Promise<boolean>;
  onExport: (session: Session) => Promise<void>;
  /** Resolves true once the session is deleted, false when it failed. */
  onDelete: (session: Session) => Promise<boolean>;
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
              if (saved) {
                setRenaming(false);
                // D5: parity with Escape — focus returns to this row's trigger.
                menuTriggerRef.current?.focus();
              }
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
          data-session-key={sessionKey(session)}
          className="session-row-button"
          aria-label={rowLabel(session)}
          aria-current={active ? 'page' : undefined}
          title={session.preview ?? session.title}
          onClick={() => actions.onOpen(session)}
        >
          {session.activeRuns > 0 && (
            <span className="session-pulse" aria-hidden />
          )}
          <span className="session-title">{session.title}</span>
          {session.unread && (
            <span className="session-unread-dot" aria-hidden />
          )}
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
        <div
          className="session-menu"
          role="menu"
          aria-label={`${session.title} actions`}
        >
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
                    // D5: parity with Escape — focus returns to the trigger.
                    // Once the refresh drops the row from this list, the
                    // sidebar moves focus on to a neighbouring row (R4).
                    closeMenuToTrigger();
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
                    // D5: parity with Escape — focus returns to the trigger.
                    closeMenuToTrigger();
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
    event.currentTarget.querySelectorAll<HTMLButtonElement>(
      '[data-session-row]',
    ),
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
  hasMore = false,
  loadingMore = false,
  onLoadMore,
  ...actions
}: SessionSidebarProps) {
  const [text, setText] = useState(query);
  const [kind, setKind] = useState<SessionKind | null>(null);
  const navRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  // D5 (Delete), R4 (Archive/Unarchive): the row and its position are gone
  // once the list drops it, so the target to focus afterward — a sibling
  // row, found before the action — is captured up front and resolved once
  // `sessions` actually drops the key.
  const pendingRemovalRef = useRef<{
    key: string;
    target: HTMLElement | null;
  } | null>(null);
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
  useEffect(() => {
    const pending = pendingRemovalRef.current;
    if (!pending) return;
    if (sessions.some((session) => sessionKey(session) === pending.key)) return;
    pendingRemovalRef.current = null;
    // Only focus the removal dropped is moved: it sits on the body, or on a
    // container around the list that caught it (the mobile drawer's panel).
    // Focus the owner has taken elsewhere meanwhile stays where it is.
    const active = document.activeElement;
    const nav = navRef.current;
    if (active && nav && !active.contains(nav)) return;
    const target = pending.target;
    if (target && target.isConnected) target.focus();
    else searchRef.current?.focus();
  }, [sessions]);

  /** The row to focus once `key`'s row leaves the list: the next row outside
   *  its own list item (a chat's helpers remount at the top level once it
   *  goes, R4), or else the previous row. */
  const rowAfterRemoval = (key: string): HTMLElement | null => {
    const rows = listRef.current
      ? Array.from(
          listRef.current.querySelectorAll<HTMLElement>('[data-session-row]'),
        )
      : [];
    const index = rows.findIndex((row) => row.dataset.sessionKey === key);
    if (index === -1) return null;
    const item = rows[index].closest('li');
    const next = rows
      .slice(index + 1)
      .find((row) => !(item && item.contains(row)));
    return next ?? rows[index - 1] ?? null;
  };
  /** Runs an action that takes the row out of the current list (Delete, or
   *  Archive/Unarchive once the refresh lands) and remembers where focus goes
   *  then; a failed action leaves the row, so nothing stays armed (R4). */
  const removing = (
    session: Session,
    action: () => Promise<boolean>,
  ): Promise<boolean> => {
    const key = sessionKey(session);
    const pending = { key, target: rowAfterRemoval(key) };
    pendingRemovalRef.current = pending;
    const settle = (done: boolean) => {
      if (!done && pendingRemovalRef.current === pending)
        pendingRemovalRef.current = null;
      return done;
    };
    return action().then(settle, (caught: unknown) => {
      settle(false);
      throw caught;
    });
  };
  const wrappedActions: RowActions = {
    ...actions,
    onArchive: (session, archived) =>
      removing(session, () => actions.onArchive(session, archived)),
    onDelete: (session) => removing(session, () => actions.onDelete(session)),
  };

  const kinds = presentKinds(sessions);
  const activeKind = kind && kinds.includes(kind) ? kind : null;
  const visible = activeKind
    ? sessions.filter((session) => session.kind === activeKind)
    : sessions;
  const groups = groupSessions(visible, now ?? new Date(), activeKind === null);

  return (
    <nav className="session-sidebar" aria-label="Sessions" ref={navRef}>
      <input
        type="search"
        ref={searchRef}
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
      <div className="session-list" ref={listRef} onKeyDown={moveBetweenRows}>
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
                      actions={wrappedActions}
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
                              actions={wrappedActions}
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
        {hasMore && (
          <button
            type="button"
            className="session-load-more"
            disabled={loadingMore}
            aria-busy={loadingMore || undefined}
            onClick={() => onLoadMore?.()}
          >
            Load more sessions
          </button>
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
