import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatGptConnection } from './ChatGptConnection';

const api = vi.hoisted(() => ({
  status: vi.fn(),
  login: vi.fn(),
  cancel: vi.fn(),
  disconnect: vi.fn(),
}));
vi.mock('@animaOS-SWARM/sdk', () => ({
  createDaemonClient: () => ({ chatgpt: api }),
}));
const disconnected = {
  connected: false,
  accountId: null,
  planType: null,
  login: null,
  error: null,
};
const pending = () => ({
  ...disconnected,
  login: {
    userCode: 'ABCD-1234',
    verificationUrl: 'https://auth.openai.com/codex/device',
    expiresAtMs: Date.now() + 15000,
  },
});
const connected = {
  ...disconnected,
  connected: true,
  planType: 'Plus',
  accountId: 'account-1',
};
async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe('ChatGPT connection lifecycle', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetAllMocks();
    api.status.mockResolvedValue(disconnected);
  });
  afterEach(() => vi.useRealTimers());

  it('recovers pending login and stops polling when authorization completes', async () => {
    api.status.mockResolvedValueOnce(pending()).mockResolvedValue(connected);
    const changed = vi.fn();
    render(<ChatGptConnection onConnectionChange={changed} />);
    await settle();
    expect(screen.getByText('ABCD-1234')).toBeTruthy();
    expect(
      screen
        .getByRole('link', { name: 'Continue on OpenAI' })
        .getAttribute('href'),
    ).toBe('https://auth.openai.com/codex/device');
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(screen.getByText('Connected · Plus')).toBeTruthy();
    expect(changed).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(12000);
    });
    expect(api.status).toHaveBeenCalledTimes(2);
  });

  it('starts and cancels sign-in and refuses unexpected verification links', async () => {
    api.login.mockResolvedValue({
      ...pending(),
      login: {
        ...pending().login,
        verificationUrl: 'https://evil.example/codex/device',
      },
    });
    api.cancel.mockResolvedValue(disconnected);
    render(<ChatGptConnection />);
    await settle();
    fireEvent.click(screen.getByRole('button', { name: 'Connect ChatGPT' }));
    await settle();
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.getByRole('alert').textContent).toContain(
      'unexpected verification address',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Cancel sign-in' }));
    await settle();
    expect(api.cancel).toHaveBeenCalledOnce();
    expect(screen.queryByText('ABCD-1234')).toBeNull();
  });

  it('shows disconnect errors and permits retry without claiming disconnection', async () => {
    api.status.mockResolvedValue(connected);
    api.disconnect
      .mockRejectedValueOnce(new Error('vault unavailable'))
      .mockResolvedValue(disconnected);
    render(<ChatGptConnection />);
    await settle();
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect ChatGPT' }));
    await settle();
    expect(screen.getByRole('alert').textContent).toContain(
      'vault unavailable',
    );
    expect(screen.getByText('Connected · Plus')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect ChatGPT' }));
    await settle();
    expect(screen.getByText('ChatGPT is not connected.')).toBeTruthy();
  });

  it('can clear a partially deleted saved connection after reopening settings', async () => {
    api.status.mockRejectedValue(new Error('ChatGPT credential vault is unavailable'));
    api.disconnect.mockResolvedValue(disconnected);
    render(<ChatGptConnection />);
    await settle();
    fireEvent.click(screen.getByRole('button', { name: 'Clear saved ChatGPT connection' }));
    await settle();
    expect(api.disconnect).toHaveBeenCalledOnce();
    expect(screen.getByText('ChatGPT is not connected.')).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('expires a pending code and stops polling after expiry', async () => {
    const login = pending();
    login.login.expiresAtMs = Date.now() + 1000;
    api.status.mockResolvedValue(login);
    render(<ChatGptConnection />);
    await settle();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(
      screen.getByText('Sign-in expired. Start again for a new code.'),
    ).toBeTruthy();
    expect(screen.queryByRole('link')).toBeNull();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    expect(api.status).toHaveBeenCalledTimes(2);
  });

  it('keeps a failed cancellation recoverable and clears polling on unmount', async () => {
    api.status.mockResolvedValue(pending());
    api.cancel.mockRejectedValue(new Error('cancel failed'));
    const view = render(<ChatGptConnection />);
    await settle();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel sign-in' }));
    await settle();
    expect(screen.getByRole('alert').textContent).toContain('cancel failed');
    expect(screen.getByText('ABCD-1234')).toBeTruthy();
    view.unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(15000);
    });
    expect(api.status).toHaveBeenCalledTimes(1);
  });

  it('does not poll disconnected accounts', async () => {
    render(<ChatGptConnection />);
    await settle();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(15000);
    });
    expect(api.status).toHaveBeenCalledTimes(1);
  });
});
