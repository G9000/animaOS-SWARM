import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError, type UsageQuery } from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';
import * as download from '../lib/download';
import {
  USAGE_EMPTY,
  USAGE_SUBSCRIPTION_NOTE,
  USAGE_TOO_OLD,
  USAGE_TRUNCATED_NOTE,
  USAGE_UNPRICED_NOTE,
} from '../lib/usage';
import { sessionFixture } from '../test/sessions';
import { groupFixture, summaryFixture, totalsFixture } from '../test/usage';
import { UsagePage } from './UsagePage';

const TOTALS = totalsFixture({
  calls: 4,
  promptTokens: 3_000,
  completionTokens: 1_200,
  totalTokens: 4_200,
  costMicros: 1_230_000,
});

interface Options {
  totals?: ReturnType<typeof totalsFixture>;
  truncated?: boolean;
  models?: ReturnType<typeof groupFixture>[];
  sessions?: ReturnType<typeof groupFixture>[];
}

function mockSummaries(options: Options = {}) {
  const totals = options.totals ?? TOTALS;
  vi.spyOn(daemon, 'usageSummary').mockImplementation(
    async (query: UsageQuery) => {
      const base = { groupBy: query.groupBy ?? null, totals };
      switch (query.groupBy) {
        case 'day':
          return summaryFixture({
            ...base,
            truncated: options.truncated ?? false,
            groups:
              totals.calls === 0
                ? []
                : [
                    groupFixture(todayKey(), {
                      calls: 1,
                      totalTokens: 900,
                      costMicros: 250_000,
                    }),
                  ],
          });
        case 'model':
          return summaryFixture({
            ...base,
            groups: options.models ?? [
              groupFixture('openai/gpt-5.4', {
                calls: 4,
                totalTokens: 4_200,
                costMicros: 1_230_000,
              }),
            ],
          });
        case 'source':
          return summaryFixture({
            ...base,
            groups: [groupFixture('chat', { calls: 4, totalTokens: 4_200 })],
          });
        default:
          return summaryFixture({
            ...base,
            groups: options.sessions ?? [
              groupFixture('chat:1', { calls: 4, totalTokens: 4_200 }),
              groupFixture('', { calls: 1, totalTokens: 10 }),
            ],
          });
      }
    },
  );
}

function todayKey(): string {
  const now = new Date();
  return new Date(Date.UTC(now.getFullYear(), now.getMonth(), now.getDate()))
    .toISOString()
    .slice(0, 10);
}

function renderPage(
  props: Partial<{
    online: boolean;
    sessionId: string | null;
  }> = {},
) {
  return render(
    <UsagePage
      agentId="agent-main"
      online={props.online ?? true}
      epoch={1}
      sessionId={props.sessionId ?? null}
    />,
  );
}

beforeEach(() => {
  window.sessionStorage.clear();
  mockSummaries();
  vi.spyOn(daemon, 'getSession').mockResolvedValue(
    sessionFixture('chat:1', {
      usage: totalsFixture({
        calls: 2,
        totalTokens: 1_500,
        costMicros: 40_000,
      }),
    }),
  );
  vi.spyOn(daemon, 'exportUsageCsv').mockResolvedValue('id\n');
  vi.spyOn(download, 'downloadText').mockImplementation(() => undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
  window.sessionStorage.clear();
});

function card(name: string) {
  return screen.getByRole('group', { name });
}

describe('UsagePage', () => {
  it('shows the empty state', async () => {
    mockSummaries({ totals: totalsFixture() });
    renderPage();
    expect(await screen.findByText(USAGE_EMPTY)).toBeVisible();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('shows totals, cost, calls, today, and this chat', async () => {
    renderPage({ sessionId: 'chat:1' });
    expect(
      await screen.findByRole('heading', { name: 'What your companion costs' }),
    ).toBeVisible();

    const total = await screen.findByRole('group', { name: 'Total tokens' });
    expect(within(total).getByText('4.2k')).toBeVisible();
    expect(
      within(total).getByText('3k prompt · 1.2k completion'),
    ).toBeVisible();
    expect(within(card('Cost')).getByText('$1.23')).toBeVisible();
    expect(within(card('Calls')).getByText('4')).toBeVisible();
    const today = card('Today');
    expect(within(today).getByText('900')).toBeVisible();
    expect(within(today).getByText('$0.25')).toBeVisible();
    const chat = await screen.findByRole('group', { name: 'This chat' });
    expect(within(chat).getByText('1.5k')).toBeVisible();
    expect(within(chat).getByText('$0.04')).toBeVisible();
    expect(
      screen.getByRole('img', { name: /^Tokens per day, 30 days/ }),
    ).toBeVisible();
  });

  it('hides This chat when no session was viewed', async () => {
    renderPage();
    await screen.findByRole('group', { name: 'Total tokens' });
    expect(
      screen.queryByRole('group', { name: 'This chat' }),
    ).not.toBeInTheDocument();
    expect(daemon.getSession).not.toHaveBeenCalled();
  });

  it('notes unpriced and subscription calls', async () => {
    mockSummaries({
      totals: { ...TOTALS, unpricedCalls: 1, subscriptionCalls: 2 },
    });
    renderPage();
    expect(await screen.findByText(USAGE_UNPRICED_NOTE)).toBeVisible();
    expect(screen.getByText(USAGE_SUBSCRIPTION_NOTE(2))).toBeVisible();
  });

  it('switching the range reads again and remembers it', async () => {
    const user = userEvent.setup();
    const view = renderPage();
    await screen.findByRole('group', { name: 'Total tokens' });
    expect(screen.getByRole('button', { name: '30 days' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    const reads = vi.mocked(daemon.usageSummary).mock.calls.length;

    await user.click(screen.getByRole('button', { name: '7 days' }));

    await waitFor(() =>
      expect(vi.mocked(daemon.usageSummary).mock.calls.length).toBe(reads + 4),
    );
    expect(screen.getByRole('button', { name: '7 days' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(window.sessionStorage.getItem('anima.usage.range')).toBe('7');

    view.unmount();
    renderPage();
    expect(screen.getByRole('button', { name: '7 days' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await screen.findByRole('group', { name: 'Total tokens' });
  });

  it('still renders when storage throws', async () => {
    const user = userEvent.setup();
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    renderPage();
    await screen.findByRole('group', { name: 'Total tokens' });
    await user.click(screen.getByRole('button', { name: '90 days' }));
    expect(screen.getByRole('button', { name: '90 days' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  });

  it('renders the model, source, and session tables as text', async () => {
    mockSummaries({
      models: [
        groupFixture('<img src=x onerror=alert(1)>/<b>gpt</b>', {
          calls: 2,
          totalTokens: 2_000,
          costMicros: 500_000,
          unpricedCalls: 1,
        }),
      ],
    });
    const { container } = renderPage();
    expect(
      await screen.findByText('<img src=x onerror=alert(1)>/<b>gpt</b>'),
    ).toBeVisible();
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('b')).toBeNull();

    const models = screen.getByRole('region', { name: 'By model' });
    expect(within(models).getByText('$0.50 + unpriced')).toBeVisible();
    const sources = screen.getByRole('region', { name: 'By source' });
    expect(within(sources).getByText('Chat')).toBeVisible();
    const sessions = screen.getByRole('region', { name: 'Top chats' });
    expect(within(sessions).getByText('No chat')).toBeVisible();
  });

  it('session rows link to the session', async () => {
    renderPage();
    const link = await screen.findByRole('link', { name: 'chat:1' });
    expect(link).toHaveAttribute('href', '#/s/chat%3A1');
  });

  it('exports the csv', async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole('group', { name: 'Total tokens' });
    await user.click(screen.getByRole('button', { name: 'Export CSV' }));
    await waitFor(() => expect(download.downloadText).toHaveBeenCalled());
    expect(daemon.exportUsageCsv).toHaveBeenCalledWith(
      expect.objectContaining({ agentId: 'agent-main' }),
    );
    expect(vi.mocked(download.downloadText).mock.calls[0][0]).toMatch(
      /^anima-usage-\d{4}-\d{2}-\d{2}-to-\d{4}-\d{2}-\d{2}\.csv$/,
    );
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Export CSV' })).toBeEnabled(),
    );
  });

  it('offline shows the unreachable text and no lists', () => {
    renderPage({ online: false });
    expect(screen.getByText(COMPANION_UNREACHABLE)).toBeVisible();
    expect(daemon.usageSummary).not.toHaveBeenCalled();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('a 404 says to update the daemon', async () => {
    vi.mocked(daemon.usageSummary).mockRejectedValue(
      new DaemonHttpError(404, { error: 'Not found' }),
    );
    renderPage();
    expect(await screen.findByRole('alert')).toHaveTextContent(USAGE_TOO_OLD);
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Export CSV' }),
    ).not.toBeInTheDocument();
  });

  it('the truncated note shows', async () => {
    mockSummaries({ truncated: true });
    renderPage();
    expect(await screen.findByText(USAGE_TRUNCATED_NOTE)).toBeVisible();
  });
});
