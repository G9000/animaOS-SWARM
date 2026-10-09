import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../../lib/approvals';
import { automationFixture } from '../../test/automations';
import { AutomationNoticeCard } from './AutomationNoticeCard';

const made = automationFixture('schedule-1', { name: 'Stretch' });

describe('AutomationNoticeCard', () => {
  it("shows the daemon's message when Undo is refused, and allows another try", async () => {
    const user = userEvent.setup();
    const onUndo = vi
      .fn()
      .mockRejectedValueOnce(
        new DaemonHttpError(409, {
          error: 'This automation is already running',
        }),
      )
      .mockResolvedValue(true);
    render(<AutomationNoticeCard automation={made} onUndo={onUndo} />);
    const card = screen.getByRole('note', { name: 'Automation Stretch' });

    await user.click(within(card).getByRole('button', { name: 'Undo' }));
    expect(
      await within(card).findByText('This automation is already running'),
    ).toBeInTheDocument();
    expect(within(card).getByRole('button', { name: 'Undo' })).toBeEnabled();

    await user.click(within(card).getByRole('button', { name: 'Undo' }));
    expect(onUndo).toHaveBeenCalledTimes(2);
    expect(
      within(card).queryByText('This automation is already running'),
    ).toBeNull();
  });

  it('says the companion is unreachable when Undo fails any other way', async () => {
    const user = userEvent.setup();
    const onUndo = vi.fn().mockRejectedValue(new TypeError('fetch failed'));
    render(<AutomationNoticeCard automation={made} onUndo={onUndo} />);

    await user.click(screen.getByRole('button', { name: 'Undo' }));
    expect(await screen.findByText(COMPANION_UNREACHABLE)).toBeInTheDocument();
  });
});
