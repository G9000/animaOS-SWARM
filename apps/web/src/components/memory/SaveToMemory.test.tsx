import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import {
  SAVE_SHORTENED_NOTE,
  type SaveOutcome,
} from '../../hooks/useSaveToMemory';
import type { ChatMessage } from '../../lib/types';
import { SaveToMemory } from './SaveToMemory';

const message: ChatMessage = {
  id: 'm1',
  role: 'Assistant',
  content: { text: 'Remember this' },
  created_at_ms: 1,
};

function deferred() {
  let resolve!: (outcome: SaveOutcome) => void;
  const promise = new Promise<SaveOutcome>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe('SaveToMemory', () => {
  it('shows Save to memory, then Saving…, then ✓ Saved to memory', async () => {
    const user = userEvent.setup();
    const pending = deferred();
    const save = vi.fn().mockReturnValue(pending.promise);
    render(<SaveToMemory message={message} save={save} saved={null} />);

    await user.click(screen.getByRole('button', { name: 'Save to memory' }));
    expect(save).toHaveBeenCalledWith(message);
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();

    pending.resolve({ kind: 'saved', shortened: false });
    const done = await screen.findByRole('button', {
      name: '✓ Saved to memory',
    });
    expect(done).toBeDisabled();
    expect(screen.queryByText(SAVE_SHORTENED_NOTE)).toBeNull();
  });

  it('shows the saved state of a message saved before this one mounted', () => {
    render(
      <SaveToMemory
        message={message}
        save={vi.fn()}
        saved={{ shortened: true }}
      />,
    );
    expect(
      screen.getByRole('button', { name: '✓ Saved to memory' }),
    ).toBeDisabled();
    expect(screen.getByText(SAVE_SHORTENED_NOTE)).toBeVisible();
  });

  it('shows the shortened note', async () => {
    const user = userEvent.setup();
    const save = vi.fn().mockResolvedValue({ kind: 'saved', shortened: true });
    render(<SaveToMemory message={message} save={save} saved={null} />);

    await user.click(screen.getByRole('button', { name: 'Save to memory' }));
    expect(await screen.findByText(SAVE_SHORTENED_NOTE)).toBeVisible();
  });

  it('shows the failure and lets the owner try again', async () => {
    const user = userEvent.setup();
    const save = vi
      .fn()
      .mockResolvedValueOnce({ kind: 'failed', message: 'No hidden text' })
      .mockResolvedValueOnce({ kind: 'saved', shortened: false });
    render(<SaveToMemory message={message} save={save} saved={null} />);

    await user.click(screen.getByRole('button', { name: 'Save to memory' }));
    expect(await screen.findByText('No hidden text')).toBeVisible();
    const retry = screen.getByRole('button', { name: 'Save to memory' });
    expect(retry).toBeEnabled();

    await user.click(retry);
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(
      await screen.findByRole('button', { name: '✓ Saved to memory' }),
    ).toBeDisabled();
    expect(screen.queryByText('No hidden text')).toBeNull();
  });

  it('announces the result politely', async () => {
    const user = userEvent.setup();
    const save = vi
      .fn()
      .mockResolvedValue({ kind: 'failed', message: 'Couldn’t save that.' });
    render(<SaveToMemory message={message} save={save} saved={null} />);

    // Mounted empty, so the text arriving is announced.
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
    await user.click(screen.getByRole('button', { name: 'Save to memory' }));
    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent(
        'Couldn’t save that.',
      ),
    );
  });
});
