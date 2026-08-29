import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { TelegramConnector } from '../lib/daemon-api';
import { TelegramSettings } from './TelegramSettings';

const connector: TelegramConnector = {
  id: 'c1',
  agentId: 'a1',
  roomId: 'telegram:c1',
  type: 'telegram',
  bot: { id: '1', username: 'nova_bot', displayName: 'Nova Bot' },
  approvedChat: null,
  pendingPairing: {
    chat: { id: '42', kind: 'private', title: 'Leo', username: 'leo' },
    requestedAtMs: 1,
  },
  status: 'pairing',
  enabled: true,
  createdAtMs: 1,
  updatedAtMs: 1,
};

describe('TelegramSettings', () => {
  it('keeps the token password local, clears it after failure, and shows a safe error', async () => {
    const user = userEvent.setup();
    const connect = vi.fn(async () => false);
    render(
      <TelegramSettings
        connector={null}
        busy={null}
        error="Telegram unavailable"
        connect={connect}
        replace={vi.fn()}
        approve={vi.fn()}
        restart={vi.fn()}
        disconnect={vi.fn()}
      />,
    );
    const token = screen.getByLabelText('Bot token');
    expect(token).toHaveAttribute('type', 'password');
    await user.type(token, '123456:secret');
    await user.click(screen.getByRole('button', { name: 'Connect Telegram' }));
    expect(connect).toHaveBeenCalledWith('123456:secret');
    expect(token).toHaveValue('');
    expect(screen.getByRole('alert')).toHaveTextContent('Telegram unavailable');
  });

  it('approves the pending chat and exposes only safe bot identity', async () => {
    const user = userEvent.setup();
    const approve = vi.fn(async () => true);
    render(
      <TelegramSettings
        connector={connector}
        busy={null}
        error={null}
        connect={vi.fn()}
        replace={vi.fn()}
        approve={approve}
        restart={vi.fn()}
        disconnect={vi.fn()}
      />,
    );
    expect(screen.getByText('@nova_bot')).toBeVisible();
    expect(screen.getByText(/Leo/)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Approve chat' }));
    expect(approve).toHaveBeenCalledWith('c1', '42');
  });
});
