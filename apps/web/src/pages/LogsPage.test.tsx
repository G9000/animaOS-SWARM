import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useLogs, type LogsView } from '../hooks/useLogs';
import { COMPANION_UNREACHABLE } from '../lib/approvals';
import {
  LOGS_CONNECTING,
  LOGS_COPIED,
  LOGS_COPY_FAILED,
  LOGS_EMPTY,
  LOGS_PAUSED_NOTE,
  LOGS_RECONNECTING,
  LOGS_TOO_OLD,
  formatLogTime,
} from '../lib/logs';
import { logLineFixture } from '../test/system';
import { LogsPage } from './LogsPage';

vi.mock('../hooks/useLogs', () => ({ useLogs: vi.fn() }));

function view(overrides: Partial<LogsView> = {}): LogsView {
  return {
    lines: [],
    held: 0,
    connected: true,
    retrying: false,
    loaded: true,
    error: null,
    errorStatus: null,
    refresh: vi.fn(),
    ...overrides,
  };
}

function lastOptions() {
  return vi.mocked(useLogs).mock.calls.at(-1)?.[0];
}

beforeEach(() => {
  vi.mocked(useLogs).mockReturnValue(view());
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(useLogs).mockReset();
});

describe('LogsPage', () => {
  it('lists lines with time, level, target, and message', () => {
    const line = logLineFixture(1, {
      level: 'warn',
      target: 'anima_daemon::store',
      message: 'Disk is slow',
    });
    vi.mocked(useLogs).mockReturnValue(view({ lines: [line] }));
    render(<LogsPage online />);

    expect(
      screen.getByRole('heading', { name: 'What the daemon is saying' }),
    ).toBeVisible();
    const region = screen.getByRole('log', { name: 'Daemon logs' });
    expect(region).toHaveAttribute('aria-live', 'off');
    expect(within(region).getByText(formatLogTime(line.at))).toBeVisible();
    expect(within(region).getByText('Warning')).toBeVisible();
    expect(within(region).getByText('anima_daemon::store')).toBeVisible();
    expect(within(region).getByText('Disk is slow')).toBeVisible();
  });

  it('filters by level and submitted search (the hook is called with them)', async () => {
    const user = userEvent.setup();
    render(<LogsPage online />);
    expect(lastOptions()).toMatchObject({
      enabled: true,
      level: null,
      query: '',
      paused: false,
    });

    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Show at least' }),
      'warn',
    );
    expect(lastOptions()?.level).toBe('warn');

    await user.type(
      screen.getByRole('searchbox', { name: 'Search logs' }),
      'disk',
    );
    // Typing alone does not search.
    expect(lastOptions()?.query).toBe('');
    await user.keyboard('{Enter}');
    expect(lastOptions()?.query).toBe('disk');

    await user.clear(screen.getByRole('searchbox', { name: 'Search logs' }));
    await user.type(
      screen.getByRole('searchbox', { name: 'Search logs' }),
      '  net ',
    );
    await user.click(screen.getByRole('button', { name: 'Search' }));
    expect(lastOptions()?.query).toBe('net');
  });

  it('pause and resume with the held count', async () => {
    const user = userEvent.setup();
    vi.mocked(useLogs).mockReturnValue(view({ held: 12 }));
    render(<LogsPage online />);
    const toggle = screen.getByRole('button', { name: 'Pause' });
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    expect(screen.queryByText('12 new lines held')).not.toBeInTheDocument();

    await user.click(toggle);
    expect(lastOptions()?.paused).toBe(true);
    expect(screen.getByRole('button', { name: 'Resume' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(screen.getByText('12 new lines held')).toBeVisible();
    expect(screen.getByText(LOGS_PAUSED_NOTE)).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Resume' }));
    expect(lastOptions()?.paused).toBe(false);
    expect(screen.queryByText(LOGS_PAUSED_NOTE)).not.toBeInTheDocument();
  });

  it('copy reports success and failure', async () => {
    const user = userEvent.setup();
    const writeText = vi
      .spyOn(navigator.clipboard, 'writeText')
      .mockResolvedValue(undefined);
    const line = logLineFixture(1, {
      level: 'error',
      target: 't',
      message: 'boom',
    });
    vi.mocked(useLogs).mockReturnValue(view({ lines: [line] }));
    render(<LogsPage online />);

    await user.click(screen.getByRole('button', { name: 'Copy' }));
    expect(writeText).toHaveBeenCalledWith(
      `${formatLogTime(line.at)} ERROR t boom`,
    );
    expect(await screen.findByText(LOGS_COPIED)).toBeVisible();

    writeText.mockRejectedValue(new Error('denied'));
    await user.click(screen.getByRole('button', { name: 'Copy' }));
    expect(await screen.findByText(LOGS_COPY_FAILED)).toBeVisible();
    expect(screen.queryByText(LOGS_COPIED)).not.toBeInTheDocument();

    // The message stays until the filter changes.
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Show at least' }),
      'error',
    );
    expect(screen.queryByText(LOGS_COPY_FAILED)).not.toBeInTheDocument();
  });

  it('shows hidden characters as markers', () => {
    vi.mocked(useLogs).mockReturnValue(
      view({
        lines: [
          logLineFixture(1, {
            target: 'tool​name',
            message: 'pay​ment',
          }),
        ],
      }),
    );
    render(<LogsPage online />);
    const region = screen.getByRole('log');
    expect(within(region).getByText('tool⟨U+200B⟩name')).toBeVisible();
    expect(within(region).getByText('pay⟨U+200B⟩ment')).toBeVisible();
    expect(
      within(region).getAllByText('This text contains 1 invisible character'),
    ).toHaveLength(2);
  });

  it('renders log text as text, never markup', () => {
    const markup = '<img src=x onerror=alert(1)> <b>bold</b>';
    vi.mocked(useLogs).mockReturnValue(
      view({ lines: [logLineFixture(1, { message: markup })] }),
    );
    const { container } = render(<LogsPage online />);
    expect(screen.getByText(markup)).toBeVisible();
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('b')).toBeNull();
  });

  it('shows connecting and reconnecting notes', () => {
    vi.mocked(useLogs).mockReturnValue(
      view({ connected: false, loaded: false }),
    );
    const { rerender } = render(<LogsPage online />);
    expect(screen.getByText(LOGS_CONNECTING)).toBeVisible();

    vi.mocked(useLogs).mockReturnValue(
      view({ connected: false, loaded: true }),
    );
    rerender(<LogsPage online />);
    // A quiet stream that has not dropped shows no note.
    expect(screen.queryByText(LOGS_RECONNECTING)).not.toBeInTheDocument();
    expect(screen.queryByText(LOGS_CONNECTING)).not.toBeInTheDocument();

    vi.mocked(useLogs).mockReturnValue(
      view({ connected: false, loaded: true, retrying: true }),
    );
    rerender(<LogsPage online />);
    expect(screen.getByText(LOGS_RECONNECTING)).toBeVisible();
    expect(screen.queryByText(LOGS_CONNECTING)).not.toBeInTheDocument();

    vi.mocked(useLogs).mockReturnValue(view({ connected: true }));
    rerender(<LogsPage online />);
    expect(screen.queryByText(LOGS_RECONNECTING)).not.toBeInTheDocument();
  });

  it('shows the daemon message when it refuses a stream', () => {
    vi.mocked(useLogs).mockReturnValue(
      view({
        connected: false,
        error: 'Too many log streams are open',
        errorStatus: 429,
      }),
    );
    render(<LogsPage online />);
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Too many log streams are open',
    );
  });

  it('offline shows the unreachable text and no stream', () => {
    render(<LogsPage online={false} />);
    expect(screen.getByText(COMPANION_UNREACHABLE)).toBeVisible();
    expect(lastOptions()?.enabled).toBe(false);
    expect(screen.queryByRole('log')).not.toBeInTheDocument();
  });

  it('a 404 says to update the daemon', () => {
    vi.mocked(useLogs).mockReturnValue(
      view({ error: LOGS_TOO_OLD, errorStatus: 404, connected: false }),
    );
    render(<LogsPage online />);
    expect(screen.getByText(LOGS_TOO_OLD)).toBeVisible();
    expect(screen.queryByRole('log')).not.toBeInTheDocument();
  });

  it('shows the empty state', () => {
    render(<LogsPage online />);
    expect(screen.getByText(LOGS_EMPTY)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copy' })).toBeDisabled();
  });
});
