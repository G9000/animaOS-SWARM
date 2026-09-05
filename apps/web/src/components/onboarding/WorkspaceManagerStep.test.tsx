import { createRef } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { WorkspaceManagerStep } from './WorkspaceManagerStep';

function setup(
  overrides: Partial<Parameters<typeof WorkspaceManagerStep>[0]> = {},
) {
  const props = {
    name: 'Anima',
    initiative: 'balanced' as const,
    communication: 'concise' as const,
    priorities: '',
    instructions:
      'Keep workspace context organized. Coordinate specialist work.',
    onNameChange: vi.fn(),
    onInitiativeChange: vi.fn(),
    onCommunicationChange: vi.fn(),
    onPrioritiesChange: vi.fn(),
    nameInputRef: createRef<HTMLInputElement>(),
    ...overrides,
  };
  render(<WorkspaceManagerStep {...props} />);
  return props;
}

describe('WorkspaceManagerStep', () => {
  it('presents the predefined manager without generic profile creation', () => {
    setup();
    expect(
      screen.getByRole('heading', { name: 'Workspace Manager' }),
    ).toBeInTheDocument();
    expect(screen.getByText('Calm')).toBeInTheDocument();
    expect(screen.getByText('Organized')).toBeInTheDocument();
    expect(screen.getByText('Transparent')).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /generate profile/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('group', { name: /personality preset/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/^bio$|^style$|^instructions$/i),
    ).not.toBeInTheDocument();
  });

  it('reflects controlled selections and emits initiative and communication choices', async () => {
    const props = setup();
    expect(screen.getByRole('radio', { name: 'Balanced' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'Concise' })).toBeChecked();
    await userEvent.click(screen.getByRole('radio', { name: 'Guided' }));
    expect(props.onInitiativeChange).toHaveBeenCalledWith('guided');
    await userEvent.click(screen.getByRole('radio', { name: 'Proactive' }));
    expect(props.onInitiativeChange).toHaveBeenCalledWith('proactive');
    await userEvent.click(screen.getByRole('radio', { name: 'Detailed' }));
    expect(props.onCommunicationChange).toHaveBeenCalledWith('detailed');
  });

  it('emits manager name and optional workspace preferences changes', () => {
    const props = setup();
    fireEvent.change(screen.getByRole('textbox', { name: 'Manager name' }), {
      target: { value: 'Atlas' },
    });
    expect(props.onNameChange).toHaveBeenCalledWith('Atlas');
    fireEvent.change(
      screen.getByRole('textbox', { name: 'Workspace preferences' }),
      { target: { value: 'Focus on launch readiness.' } },
    );
    expect(props.onPrioritiesChange).toHaveBeenCalledWith(
      'Focus on launch readiness.',
    );
  });

  it('connects name validation and the focus ref to the input', () => {
    const props = setup({ validationErrorId: 'manager-error' });
    const input = screen.getByRole('textbox', { name: 'Manager name' });
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAttribute('aria-describedby', 'manager-error');
    expect(props.nameInputRef.current).toBe(input);
  });

  it('keeps the composed instructions in a read-only expandable preview', async () => {
    const props = setup();
    const summary = screen.getByText('View manager instructions');
    const details = summary.closest('details');
    expect(details).not.toHaveAttribute('open');
    await userEvent.click(summary);
    expect(details).toHaveAttribute('open');
    expect(screen.getByText(props.instructions)).toBeVisible();
    expect(screen.getAllByRole('textbox')).toHaveLength(2);
  });
});
