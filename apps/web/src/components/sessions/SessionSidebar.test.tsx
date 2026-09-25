import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { sessionFixture } from '../../test/sessions';
import { SessionSidebar, type SessionSidebarProps } from './SessionSidebar';

const NOW = new Date(2026, 8, 24, 12, 0, 0);
const HOUR = 60 * 60 * 1000;
const readOnly = {
  send: false,
  steer: false,
  stop: true,
  rename: false,
  archive: true,
  delete: false,
  compact: false,
  export: true,
};

function renderSidebar(overrides: Partial<SessionSidebarProps> = {}) {
  const props: SessionSidebarProps = {
    sessions: [],
    activeKey: null,
    query: '',
    onQueryChange: vi.fn(),
    showArchived: false,
    onShowArchivedChange: vi.fn(),
    now: NOW,
    onOpen: vi.fn(),
    onRename: vi.fn().mockResolvedValue(true),
    onArchive: vi.fn().mockResolvedValue(true),
    onExport: vi.fn().mockResolvedValue(undefined),
    onDelete: vi.fn().mockResolvedValue(true),
    ...overrides,
  };
  const view = render(<SessionSidebar {...props} />);
  const rerenderWith = (nextOverrides: Partial<SessionSidebarProps>) => {
    Object.assign(props, nextOverrides);
    view.rerender(<SessionSidebar {...props} />);
  };
  return { ...props, rerenderWith };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SessionSidebar', () => {
  it('groups by day, nests helpers, and marks unread, working, and current rows', async () => {
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      unread: true,
      lastActivityAtMs: NOW.getTime() - HOUR,
    });
    const helper = sessionFixture('room-9', {
      agentId: 'helper-1',
      kind: 'helper',
      title: 'Draft a plan',
      parentAgentId: 'agent-main',
      parentSessionId: 'chat:plans',
      activeRuns: 1,
      capabilities: readOnly,
      lastActivityAtMs: NOW.getTime() - 2 * HOUR,
    });
    const old = sessionFixture('chat:old', {
      title: 'Old notes',
      lastActivityAtMs: NOW.getTime() - 40 * 24 * HOUR,
    });
    const props = renderSidebar({
      sessions: [plans, helper, old],
      activeKey: 'agent-main\u0000chat:plans',
    });

    const today = screen.getByRole('group', { name: 'Today' });
    expect(
      within(today).getByRole('button', { name: 'Plans, unread' }),
    ).toHaveAttribute('aria-current', 'page');
    expect(
      within(
        within(today).getByRole('list', { name: 'Helpers of Plans' }),
      ).getByRole('button', { name: 'Draft a plan, working' }),
    ).toBeVisible();
    expect(
      within(screen.getByRole('group', { name: 'Older' })).getByRole('button', {
        name: 'Old notes',
      }),
    ).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Old notes' }));
    expect(props.onOpen).toHaveBeenCalledWith(old);
  });

  it('filters by the kinds present and follows each kind’s capabilities', async () => {
    const user = userEvent.setup();
    renderSidebar({
      sessions: [
        sessionFixture('chat:1', {
          title: 'Chat one',
          lastActivityAtMs: NOW.getTime(),
        }),
        sessionFixture('job:1', {
          kind: 'job',
          title: 'Job · Report',
          capabilities: readOnly,
          lastActivityAtMs: NOW.getTime(),
        }),
      ],
    });

    const chips = screen.getByRole('group', { name: 'Session kinds' });
    expect(
      within(chips)
        .getAllByRole('button')
        .map((chip) => chip.textContent),
    ).toEqual(['All', 'Chats', 'Jobs']);
    await user.click(within(chips).getByRole('button', { name: 'Jobs' }));
    expect(
      screen.queryByRole('button', { name: 'Chat one' }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole('button', { name: 'Actions for Job · Report' }),
    );
    expect(
      screen.queryByRole('menuitem', { name: 'Rename' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('menuitem', { name: 'Delete' }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole('menuitem', { name: 'Export Markdown' }),
    ).toBeVisible();
  });

  it('waits for typing to settle before searching', async () => {
    const props = renderSidebar();
    fireEvent.change(
      screen.getByRole('searchbox', { name: 'Search sessions' }),
      {
        target: { value: 'budget' },
      },
    );
    expect(props.onQueryChange).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(props.onQueryChange).toHaveBeenCalledWith('budget'),
    );
  });

  it('renames, archives, exports, and deletes after confirming that memories are kept', async () => {
    const user = userEvent.setup();
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      lastActivityAtMs: NOW.getTime(),
    });
    const props = renderSidebar({ sessions: [plans] });
    const menu = () =>
      screen.getByRole('button', { name: 'Actions for Plans' });

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Rename' }));
    const input = screen.getByRole('textbox', { name: 'Rename Plans' });
    await user.clear(input);
    await user.type(input, 'Offsite{Enter}');
    expect(props.onRename).toHaveBeenCalledWith(plans, 'Offsite');

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Archive' }));
    expect(props.onArchive).toHaveBeenCalledWith(plans, true);

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Export Markdown' }));
    expect(props.onExport).toHaveBeenCalledWith(plans);

    await user.click(menu());
    await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
    expect(props.onDelete).not.toHaveBeenCalled();
    expect(screen.getByText(/memories are kept/i)).toBeVisible();
    await user.click(screen.getByRole('menuitem', { name: 'Delete session' }));
    expect(props.onDelete).toHaveBeenCalledWith(plans);
  });

  it('moves between rows with the arrow keys and asks for archived sessions', async () => {
    const props = renderSidebar({
      sessions: ['A', 'B', 'C'].map((title, index) =>
        sessionFixture(`chat:${title}`, {
          title,
          lastActivityAtMs: NOW.getTime() - index * 1_000,
        }),
      ),
    });

    screen.getByRole('button', { name: 'A' }).focus();
    fireEvent.keyDown(screen.getByRole('button', { name: 'A' }), {
      key: 'ArrowDown',
    });
    expect(screen.getByRole('button', { name: 'B' })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole('button', { name: 'B' }), {
      key: 'ArrowUp',
    });
    expect(screen.getByRole('button', { name: 'A' })).toHaveFocus();

    await userEvent.click(
      screen.getByRole('button', { name: 'Show archived' }),
    );
    expect(props.onShowArchivedChange).toHaveBeenCalledWith(true);
  });

  // Controller ruling 1 (M2 pre-flight audit): row menus — including nested
  // helper rows, which head their own "Helpers of <title>" list — close on
  // Escape and return focus to the trigger that opened them.
  it('closes the row menu on Escape and returns focus to its trigger', async () => {
    const user = userEvent.setup();
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      lastActivityAtMs: NOW.getTime(),
    });
    const helper = sessionFixture('room-9', {
      agentId: 'helper-1',
      kind: 'helper',
      title: 'Draft a plan',
      parentAgentId: 'agent-main',
      parentSessionId: 'chat:plans',
      capabilities: readOnly,
      lastActivityAtMs: NOW.getTime(),
    });
    renderSidebar({ sessions: [plans, helper] });
    const rowTrigger = screen.getByRole('button', {
      name: 'Actions for Plans',
    });

    await user.click(rowTrigger);
    expect(screen.getByRole('menu', { name: 'Plans actions' })).toBeVisible();
    fireEvent.keyDown(screen.getByRole('menu', { name: 'Plans actions' }), {
      key: 'Escape',
    });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(rowTrigger).toHaveFocus();

    // Escape also closes the row menu's delete-confirmation sub-view.
    await user.click(rowTrigger);
    await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
    expect(screen.getByText(/memories are kept/i)).toBeVisible();
    fireEvent.keyDown(screen.getByRole('menu', { name: 'Plans actions' }), {
      key: 'Escape',
    });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(rowTrigger).toHaveFocus();

    // A nested helper row heads its own list but shares the same row menu.
    const helperTrigger = screen.getByRole('button', {
      name: 'Actions for Draft a plan',
    });
    await user.click(helperTrigger);
    expect(
      screen.getByRole('menu', { name: 'Draft a plan actions' }),
    ).toBeVisible();
    fireEvent.keyDown(
      screen.getByRole('menu', { name: 'Draft a plan actions' }),
      {
        key: 'Escape',
      },
    );
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(helperTrigger).toHaveFocus();
  });

  // C: sidebar paging.
  it('shows a keyboard-reachable Load more sessions button while more sessions remain', async () => {
    const onLoadMore = vi.fn();
    const props = renderSidebar({
      sessions: [
        sessionFixture('chat:1', {
          title: 'One',
          lastActivityAtMs: NOW.getTime(),
        }),
      ],
      hasMore: true,
      loadingMore: false,
      onLoadMore,
    });

    const button = screen.getByRole('button', { name: 'Load more sessions' });
    expect(button).toBeEnabled();
    button.focus();
    expect(button).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(onLoadMore).toHaveBeenCalledOnce();

    props.rerenderWith({ loadingMore: true });
    expect(
      screen.getByRole('button', { name: 'Load more sessions' }),
    ).toBeDisabled();
  });

  it('hides the Load more sessions button once hasMore is false', () => {
    renderSidebar({
      sessions: [sessionFixture('chat:1', { title: 'One' })],
      hasMore: false,
    });
    expect(
      screen.queryByRole('button', { name: 'Load more sessions' }),
    ).not.toBeInTheDocument();
  });

  // D5: focus after a row menu action (Controller ruling 1's parity for the
  // other row actions, plus Delete's own destination).
  it('returns focus to the row trigger after Rename, Archive, and Export', async () => {
    const user = userEvent.setup();
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      lastActivityAtMs: NOW.getTime(),
    });
    renderSidebar({ sessions: [plans] });
    const trigger = () =>
      screen.getByRole('button', { name: 'Actions for Plans' });

    await user.click(trigger());
    await user.click(screen.getByRole('menuitem', { name: 'Rename' }));
    await user.type(
      screen.getByRole('textbox', { name: 'Rename Plans' }),
      '{Enter}',
    );
    await waitFor(() => expect(trigger()).toHaveFocus());

    await user.click(trigger());
    await user.click(screen.getByRole('menuitem', { name: 'Archive' }));
    expect(trigger()).toHaveFocus();

    await user.click(trigger());
    await user.click(screen.getByRole('menuitem', { name: 'Export Markdown' }));
    expect(trigger()).toHaveFocus();
  });

  it('moves focus to the next row, then the previous row, then the search box after Delete', async () => {
    const user = userEvent.setup();
    const make = (id: string, title: string) =>
      sessionFixture(`chat:${id}`, { title, lastActivityAtMs: NOW.getTime() });
    let sessions = [
      make('a', 'Alpha'),
      make('b', 'Bravo'),
      make('c', 'Charlie'),
    ];
    const props = renderSidebar({ sessions });
    const deleteRow = async (title: string) => {
      await user.click(
        screen.getByRole('button', { name: `Actions for ${title}` }),
      );
      await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
      await user.click(
        screen.getByRole('menuitem', { name: 'Delete session' }),
      );
    };

    // Deleting the middle row moves focus to the next row (Charlie).
    await deleteRow('Bravo');
    sessions = sessions.filter((item) => item.title !== 'Bravo');
    props.rerenderWith({ sessions });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Charlie' })).toHaveFocus(),
    );

    // Deleting the last row moves focus to the previous row (Alpha).
    await deleteRow('Charlie');
    sessions = sessions.filter((item) => item.title !== 'Charlie');
    props.rerenderWith({ sessions });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Alpha' })).toHaveFocus(),
    );

    // Deleting the only remaining row falls back to the search box.
    await deleteRow('Alpha');
    sessions = [];
    props.rerenderWith({ sessions });
    await waitFor(() =>
      expect(
        screen.getByRole('searchbox', { name: 'Search sessions' }),
      ).toHaveFocus(),
    );
  });
  // R4 (residual round): focus after a row leaves the list.
  it('moves focus to the next row once an archived or unarchived row leaves the list', async () => {
    const user = userEvent.setup();
    const make = (id: string, title: string, archived = false) =>
      sessionFixture(`chat:${id}`, {
        title,
        archived,
        lastActivityAtMs: NOW.getTime(),
      });
    let sessions = [
      make('a', 'Alpha'),
      make('b', 'Bravo'),
      make('c', 'Charlie'),
    ];
    const props = renderSidebar({ sessions });

    await user.click(screen.getByRole('button', { name: 'Actions for Bravo' }));
    await user.click(screen.getByRole('menuitem', { name: 'Archive' }));
    expect(props.onArchive).toHaveBeenCalledWith(sessions[1], true);
    // The refresh that follows lists Bravo no more.
    sessions = sessions.filter((item) => item.title !== 'Bravo');
    props.rerenderWith({ sessions });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Charlie' })).toHaveFocus(),
    );

    // Unarchive in the archived view does the same (the previous row here).
    sessions = [make('d', 'Delta', true), make('e', 'Echo', true)];
    props.rerenderWith({ sessions, showArchived: true });
    await user.click(screen.getByRole('button', { name: 'Actions for Echo' }));
    await user.click(screen.getByRole('menuitem', { name: 'Unarchive' }));
    expect(props.onArchive).toHaveBeenLastCalledWith(sessions[1], false);
    sessions = sessions.filter((item) => item.title !== 'Echo');
    props.rerenderWith({ sessions });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Delta' })).toHaveFocus(),
    );
  });

  it('forgets the focus target of a Delete or Archive that failed', async () => {
    const user = userEvent.setup();
    const make = (id: string, title: string) =>
      sessionFixture(`chat:${id}`, { title, lastActivityAtMs: NOW.getTime() });
    let sessions = [
      make('a', 'Alpha'),
      make('b', 'Bravo'),
      make('c', 'Charlie'),
    ];
    const props = renderSidebar({
      sessions,
      onDelete: vi.fn().mockResolvedValue(false),
      onArchive: vi.fn().mockResolvedValue(false),
    });

    await user.click(screen.getByRole('button', { name: 'Actions for Bravo' }));
    await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
    await user.click(screen.getByRole('menuitem', { name: 'Delete session' }));
    await waitFor(() => expect(props.onDelete).toHaveBeenCalledOnce());
    await act(async () => {});
    // Bravo leaves later for another reason (another tab deleted it): focus,
    // wherever it is, is not pulled to the row after it.
    sessions = sessions.filter((item) => item.title !== 'Bravo');
    props.rerenderWith({ sessions });
    await act(async () => {});
    expect(screen.getByRole('button', { name: 'Charlie' })).not.toHaveFocus();

    await user.click(screen.getByRole('button', { name: 'Actions for Alpha' }));
    await user.click(screen.getByRole('menuitem', { name: 'Archive' }));
    await waitFor(() => expect(props.onArchive).toHaveBeenCalledOnce());
    await act(async () => {});
    (document.activeElement as HTMLElement | null)?.blur();
    sessions = sessions.filter((item) => item.title !== 'Alpha');
    props.rerenderWith({ sessions });
    await act(async () => {});
    expect(screen.getByRole('button', { name: 'Charlie' })).not.toHaveFocus();
  });

  it('skips a deleted chat’s own helpers, which move to the top level, when choosing the next row', async () => {
    const user = userEvent.setup();
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      lastActivityAtMs: NOW.getTime() - HOUR,
    });
    const helper = sessionFixture('room-9', {
      agentId: 'helper-1',
      kind: 'helper',
      title: 'Draft a plan',
      parentAgentId: 'agent-main',
      parentSessionId: 'chat:plans',
      capabilities: readOnly,
      lastActivityAtMs: NOW.getTime() - 2 * HOUR,
    });
    const old = sessionFixture('chat:old', {
      title: 'Old notes',
      lastActivityAtMs: NOW.getTime() - 40 * 24 * HOUR,
    });
    const props = renderSidebar({ sessions: [plans, helper, old] });

    await user.click(screen.getByRole('button', { name: 'Actions for Plans' }));
    await user.click(screen.getByRole('menuitem', { name: 'Delete' }));
    await user.click(screen.getByRole('menuitem', { name: 'Delete session' }));
    props.rerenderWith({ sessions: [helper, old] });

    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Old notes' })).toHaveFocus(),
    );
  });
});
