import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { AgentStep } from './AgentStep';

function setup(overrides: Partial<Parameters<typeof AgentStep>[0]> = {}) {
  const props = {
    name: 'Anima',
    presetId: 'chief-of-staff' as const,
    intent: '',
    bio: '',
    adjectives: [] as string[],
    style: '',
    system: '',
    generating: false,
    generateAvailable: true,
    generateError: null,
    onNameChange: vi.fn(),
    onPresetChange: vi.fn(),
    onIntentChange: vi.fn(),
    onBioChange: vi.fn(),
    onStyleChange: vi.fn(),
    onSystemChange: vi.fn(),
    onGenerate: vi.fn(),
    nameInputRef: { current: null },
    ...overrides,
  };
  render(<AgentStep {...props} />);
  return props;
}

describe('AgentStep', () => {
  it('renders the four preset cards', () => {
    setup();
    expect(screen.getByText('Chief of Staff')).toBeInTheDocument();
    expect(screen.getByText('Calm Assistant')).toBeInTheDocument();
    expect(screen.getByText('Senior Engineer')).toBeInTheDocument();
    expect(screen.getByText('Creative Partner')).toBeInTheDocument();
  });

  it('marks the selected preset', () => {
    setup({ presetId: 'senior-engineer' });
    expect(
      screen.getByRole('radio', { name: /senior engineer/i }),
    ).toBeChecked();
  });

  it('selecting a preset emits onPresetChange', async () => {
    const props = setup();
    await userEvent.click(
      screen.getByRole('radio', { name: /creative partner/i }),
    );
    expect(props.onPresetChange).toHaveBeenCalledWith('creative-partner');
  });

  it('generate is disabled without intent', () => {
    setup({ intent: '' });
    expect(
      screen.getByRole('button', { name: /generate profile/i }),
    ).toBeDisabled();
  });

  it('generate click emits onGenerate when intent exists', async () => {
    const props = setup({ intent: 'watch my portfolio' });
    await userEvent.click(
      screen.getByRole('button', { name: /generate profile/i }),
    );
    expect(props.onGenerate).toHaveBeenCalled();
  });

  it('shows fallback notice when generation is unavailable', () => {
    setup({ generateAvailable: false });
    expect(screen.getByText(/template/i)).toBeInTheDocument();
  });

  it('profile fields stay editable after generation', async () => {
    const props = setup({ bio: 'A bio', system: 'A system' });
    await userEvent.type(screen.getByLabelText(/^bio/i), '!');
    expect(props.onBioChange).toHaveBeenCalled();
    await userEvent.type(screen.getByLabelText(/instructions/i), '!');
    expect(props.onSystemChange).toHaveBeenCalled();
  });

  it('switches to the regenerate label once a system prompt exists', () => {
    setup({ system: 'A system', intent: 'x' });
    expect(
      screen.getByRole('button', { name: /regenerate profile/i }),
    ).toBeEnabled();
  });

  it('shows generateError in an alert', () => {
    setup({ generateError: 'boom' });
    expect(screen.getByRole('alert')).toHaveTextContent('boom');
  });

  it('disables generate while generating', () => {
    setup({ intent: 'x', generating: true });
    expect(
      screen.getByRole('button', { name: /generating/i }),
    ).toBeDisabled();
  });

  it('disables generate when generation is unavailable', () => {
    setup({ intent: 'x', generateAvailable: false });
    expect(
      screen.getByRole('button', { name: /generate profile/i }),
    ).toBeDisabled();
  });
});
