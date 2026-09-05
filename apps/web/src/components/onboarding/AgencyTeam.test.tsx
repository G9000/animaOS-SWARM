import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type { AgencyMember } from '../../lib/agency-templates';
import { AgencyTeam } from './AgencyTeam';

const members: AgencyMember[] = ['Strategist', 'Writer', 'Analyst'].map(
  (name) => ({
    name,
    bio: `${name} role`,
    system: `${name} instructions`,
    presetId: 'creative-partner',
  }),
);

function Team({ initial = members }: { initial?: AgencyMember[] }) {
  const [workers, setWorkers] = useState(initial);
  return (
    <AgencyTeam
      workers={workers}
      onChange={(index, field, value) =>
        setWorkers((current) =>
          current.map((worker, i) =>
            i === index ? { ...worker, [field]: value } : worker,
          ),
        )
      }
      onRemove={(index) =>
        setWorkers((current) => current.filter((_, i) => i !== index))
      }
    />
  );
}

describe('AgencyTeam', () => {
  it('starts with a compact roster and exposes one editor through keyboard controls', async () => {
    const user = userEvent.setup();
    render(<Team />);
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.getByText('Strategist role')).toBeVisible();
    const edit = screen.getByRole('button', { name: 'Edit Strategist' });
    expect(edit).toHaveAttribute('aria-expanded', 'false');
    edit.focus();
    await user.keyboard('{Enter}');
    expect(edit).toHaveAttribute('aria-expanded', 'true');
    expect(
      document.getElementById(edit.getAttribute('aria-controls')!),
    ).toBeVisible();
    expect(screen.getByLabelText('Specialist 1 instructions')).toHaveValue(
      'Strategist instructions',
    );
    await user.click(screen.getByRole('button', { name: 'Edit Writer' }));
    expect(
      screen.queryByLabelText('Specialist 1 instructions'),
    ).not.toBeInTheDocument();
    expect(screen.getByLabelText('Specialist 2 instructions')).toBeVisible();
  });

  it('retains edits through renaming, collapsing, and reopening', async () => {
    render(<Team />);
    await userEvent.click(screen.getByRole('button', { name: 'Edit Writer' }));
    const name = screen.getByLabelText('Specialist 2 name');
    fireEvent.change(name, { target: { value: 'Editor' } });
    expect(screen.getByLabelText('Specialist 2 name')).toBe(name);
    fireEvent.change(screen.getByLabelText('Specialist 2 role'), {
      target: { value: 'Review drafts' },
    });
    fireEvent.change(screen.getByLabelText('Specialist 2 instructions'), {
      target: { value: 'Check every claim' },
    });
    await userEvent.click(screen.getByRole('button', { name: 'Edit Editor' }));
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.getByText('Review drafts')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Edit Editor' }));
    expect(screen.getByLabelText('Specialist 2 instructions')).toHaveValue(
      'Check every claim',
    );
  });

  it('keeps the same member open when an earlier member is removed and closes a removed editor', async () => {
    render(<Team />);
    await userEvent.click(screen.getByRole('button', { name: 'Edit Writer' }));
    await userEvent.click(
      screen.getByRole('button', { name: 'Remove Strategist' }),
    );
    expect(screen.getByLabelText('Specialist 1 name')).toHaveValue('Writer');
    await userEvent.click(
      screen.getByRole('button', { name: 'Remove Analyst' }),
    );
    expect(screen.getByLabelText('Specialist 1 name')).toHaveValue('Writer');
    await userEvent.click(
      screen.getByRole('button', { name: 'Remove Writer' }),
    );
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(
      screen.getByText('Your workspace manager will be created on its own.'),
    ).toBeVisible();
  });

  it('does not open the next member after removing the edited member', async () => {
    render(<Team />);
    await userEvent.click(screen.getByRole('button', { name: 'Edit Writer' }));
    await userEvent.click(
      screen.getByRole('button', { name: 'Remove Writer' }),
    );
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Edit Analyst' }),
    ).toHaveAttribute('aria-expanded', 'false');
  });
});
