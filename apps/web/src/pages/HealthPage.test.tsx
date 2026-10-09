import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StatusTooOldError } from '@animaOS-SWARM/sdk';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';
import { HEALTH_TOO_OLD } from '../lib/status';
import { statusFixture } from '../test/system';
import { HealthPage } from './HealthPage';

beforeEach(() => {
  vi.spyOn(daemon, 'status').mockResolvedValue(statusFixture());
});

afterEach(() => {
  vi.restoreAllMocks();
});

const card = (name: string) => screen.findByRole('region', { name });

describe('HealthPage', () => {
  it('shows a card for each area', async () => {
    render(<HealthPage online epoch={1} />);
    expect(
      await screen.findByRole('heading', { name: 'How the daemon is doing' }),
    ).toBeVisible();
    for (const name of [
      'Readiness',
      'Storage',
      'Providers',
      'Connectors',
      'Automations',
      'Pending approvals',
      'Runs',
      'Daemon',
    ]) {
      expect(await card(name)).toBeVisible();
    }
    expect(
      within(await card('Pending approvals')).getByText(
        'Nothing is waiting for you',
      ),
    ).toBeVisible();
    expect(within(await card('Readiness')).getByText('OK')).toBeVisible();
    expect(
      within(await card('Providers')).getByText('1 of 2 configured'),
    ).toBeVisible();
  });

  it('a not-ready daemon lists its issues', async () => {
    vi.mocked(daemon.status).mockResolvedValue(
      statusFixture({
        readiness: { status: 'not_ready', issues: ['No provider is set up'] },
      }),
    );
    render(<HealthPage online epoch={1} />);
    const readiness = await card('Readiness');
    expect(within(readiness).getByText('Problem')).toBeVisible();
    expect(within(readiness).getByText('No provider is set up')).toBeVisible();
  });

  it('a failing history store shows its redacted error as text', async () => {
    const base = statusFixture();
    vi.mocked(daemon.status).mockResolvedValue(
      statusFixture({
        storage: {
          ...base.storage,
          history: {
            ...base.storage.history,
            healthy: false,
            lastError: 'write failed <b>[redacted]</b>​',
          },
        },
      }),
    );
    const { container } = render(<HealthPage online epoch={1} />);
    const storage = await card('Storage');
    expect(within(storage).getByText('Problem')).toBeVisible();
    expect(
      within(storage).getByText(
        'Last error: write failed <b>[redacted]</b>⟨U+200B⟩',
      ),
    ).toBeVisible();
    expect(container.querySelector('b')).toBeNull();
  });

  it('pending approvals link to the Approvals page', async () => {
    vi.mocked(daemon.status).mockResolvedValue(
      statusFixture({ approvals: { pending: 2 } }),
    );
    render(<HealthPage online epoch={1} />);
    const approvals = await card('Pending approvals');
    expect(
      within(approvals).getByText('2 requests are waiting for you'),
    ).toBeVisible();
    expect(
      within(approvals).getByRole('link', { name: 'Review approvals' }),
    ).toHaveAttribute('href', '#/approvals');
    expect(within(approvals).getByText('Needs attention')).toBeVisible();
  });

  it('failing automations link to Automations', async () => {
    vi.mocked(daemon.status).mockResolvedValue(
      statusFixture({
        automations: { total: 4, enabled: 3, failing: 1, failuresTotal: 2 },
      }),
    );
    render(<HealthPage online epoch={1} />);
    const automations = await card('Automations');
    expect(within(automations).getByText('3 enabled, 1 failing')).toBeVisible();
    expect(
      within(automations).getByRole('link', { name: 'Review automations' }),
    ).toHaveAttribute('href', '#/automations');
  });

  it('shows version, revision, and uptime', async () => {
    render(<HealthPage online epoch={1} />);
    const daemonCard = await card('Daemon');
    expect(within(daemonCard).getByText('Version 0.9.1')).toBeVisible();
    expect(within(daemonCard).getByText('Build abc1234')).toBeVisible();
    expect(within(daemonCard).getByText('Up 3h 05m')).toBeVisible();
    expect(within(daemonCard).getByText('2 event subscribers')).toBeVisible();
  });

  it('refresh reads again', async () => {
    const user = userEvent.setup();
    render(<HealthPage online epoch={1} />);
    await card('Daemon');
    expect(daemon.status).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(daemon.status).toHaveBeenCalledTimes(2);
    await card('Daemon');
  });

  it('shows the loading text before the first answer', async () => {
    vi.mocked(daemon.status).mockReturnValue(new Promise(() => undefined));
    render(<HealthPage online epoch={1} />);
    expect(screen.getByText('Loading health…')).toBeVisible();
  });

  it('offline and too-old states', async () => {
    const { unmount } = render(<HealthPage online={false} epoch={1} />);
    expect(screen.getByText(COMPANION_UNREACHABLE)).toBeVisible();
    expect(daemon.status).not.toHaveBeenCalled();
    unmount();

    vi.mocked(daemon.status).mockRejectedValue(new StatusTooOldError());
    render(<HealthPage online epoch={1} />);
    expect(await screen.findByText(HEALTH_TOO_OLD)).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
  });

  it('keeps the cards and says so when a later read fails', async () => {
    const user = userEvent.setup();
    render(<HealthPage online epoch={1} />);
    await card('Daemon');
    vi.mocked(daemon.status).mockRejectedValue(new TypeError('offline'));
    await user.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      COMPANION_UNREACHABLE,
    );
    expect(await card('Daemon')).toBeVisible();
  });
});
