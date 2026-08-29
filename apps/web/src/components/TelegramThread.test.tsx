import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { TelegramThread } from './TelegramThread';

describe('TelegramThread', () => {
  it('renders dedicated messages, pages older history, and announces queued delivery', async () => {
    const user = userEvent.setup();
    const loadOlder = vi.fn(async () => true);
    const send = vi.fn(async () => true);
    render(
      <TelegramThread
        agentName="Nova"
        messages={[
          {
            id: 'm1',
            agentId: 'a1',
            roomId: 'telegram:c1',
            role: 'assistant',
            content: { text: 'Telegram only' },
            createdAtMs: 1,
          },
        ]}
        hasOlder
        busy={null}
        error={null}
        deliveryQueued
        loadOlder={loadOlder}
        send={send}
      />,
    );
    expect(screen.getByText('Telegram only')).toBeVisible();
    expect(screen.getByRole('status')).toHaveTextContent(
      'Queued for Telegram delivery',
    );
    await user.click(
      screen.getByRole('button', { name: 'Load older messages' }),
    );
    await user.type(
      screen.getByPlaceholderText('Message Nova on Telegram…'),
      'hello',
    );
    await user.click(screen.getByRole('button', { name: 'Send to Telegram' }));
    expect(loadOlder).toHaveBeenCalledOnce();
    expect(send).toHaveBeenCalledWith('hello');
  });
});
