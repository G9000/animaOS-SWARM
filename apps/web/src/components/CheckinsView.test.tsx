import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../lib/types';
import { CheckinsView } from './CheckinsView';

const agent: AgentDetail = {
  id: 'a1',
  name: 'Nova',
  provider: 'openai',
  model: 'gpt',
  toolNames: [],
  created_at_ms: 1,
  status: 'Idle',
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  messages: [],
};

describe('CheckinsView daemon schedules', () => {
  it('explains daemon execution and disables Telegram until a chat is approved', async () => {
    const user = userEvent.setup();
    const setTarget = vi.fn();
    render(
      <CheckinsView
        agent={agent}
        checkins={[]}
        prompt="Check goals"
        setPrompt={vi.fn()}
        intervalMin={30}
        setIntervalMin={vi.fn()}
        addCheckin={vi.fn()}
        removeCheckin={vi.fn()}
        error={null}
        target="workspace"
        setTarget={setTarget}
        telegramAvailable={false}
        busy={false}
      />,
    );
    expect(screen.getByText(/runs while anima-daemon is active/)).toBeVisible();
    expect(screen.getByRole('option', { name: 'Telegram' })).toBeDisabled();
    await user.selectOptions(screen.getByRole('combobox'), 'workspace');
    expect(screen.getByRole('button', { name: 'Add prompt' })).toBeEnabled();
  });
});
