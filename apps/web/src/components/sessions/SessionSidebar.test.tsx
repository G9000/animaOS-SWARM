import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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
    onArchive: vi.fn().mockResolvedValue(undefined),
    onExport: vi.fn().mockResolvedValue(undefined),
    onDelete: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
  render(<SessionSidebar {...props} />);
  return props;
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
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search sessions' }), {
      target: { value: 'budget' },
    });
    expect(props.onQueryChange).not.toHaveBeenCalled();
    await waitFor(() => expect(props.onQueryChange).toHaveBeenCalledWith('budget'));
  });

  it('renames, archives, exports, and deletes after confirming that memories are kept', async () => {
    const user = userEvent.setup();
    const plans = sessionFixture('chat:plans', {
      title: 'Plans',
      lastActivityAtMs: NOW.getTime(),
    });
    const props = renderSidebar({ sessions: [plans] });
    const menu = () => screen.getByRole('button', { name: 'Actions for Plans' });

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
    fireEvent.keyDown(screen.getByRole('button', { name: 'A' }), { key: 'ArrowDown' });
    expect(screen.getByRole('button', { name: 'B' })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole('button', { name: 'B' }), { key: 'ArrowUp' });
    expect(screen.getByRole('button', { name: 'A' })).toHaveFocus();

    await userEvent.click(screen.getByRole('button', { name: 'Show archived' }));
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
    const rowTrigger = screen.getByRole('button', { name: 'Actions for Plans' });

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
    fireEvent.keyDown(screen.getByRole('menu', { name: 'Draft a plan actions' }), {
      key: 'Escape',
    });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(helperTrigger).toHaveFocus();
  });
});
