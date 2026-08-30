import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { WorkspaceStep } from './WorkspaceStep';

function setup(overrides: Partial<Parameters<typeof WorkspaceStep>[0]> = {}) {
  const props = {
    companyName: '',
    mission: '',
    rootPath: 'C:\\anima',
    values: [] as string[],
    verifying: false,
    verifyStatus: null,
    onCompanyNameChange: vi.fn(),
    onMissionChange: vi.fn(),
    onRootPathChange: vi.fn(),
    onValuesChange: vi.fn(),
    onVerify: vi.fn(),
    companyInputRef: { current: null },
    ...overrides,
  };
  render(<WorkspaceStep {...props} />);
  return props;
}

describe('WorkspaceStep', () => {
  it('renders company, mission, folder, and values fields', () => {
    setup();
    expect(screen.getByLabelText(/company name/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/mission/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/office location/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/values/i)).toBeInTheDocument();
  });

  it('emits edits', async () => {
    const props = setup();
    await userEvent.type(screen.getByLabelText(/company name/i), 'N');
    expect(props.onCompanyNameChange).toHaveBeenLastCalledWith('N');
  });

  it('shows a verifying state and verify result', () => {
    setup({ verifying: true });
    expect(screen.getByRole('button', { name: /verifying/i })).toBeDisabled();
  });

  it('disables Verify until a folder is entered', () => {
    setup({ rootPath: 'C:\\anima' });
    expect(screen.getByRole('button', { name: 'Verify' })).toBeEnabled();

    setup({ rootPath: '   ' });
    expect(screen.getAllByRole('button', { name: 'Verify' })[1]).toBeDisabled();
  });

  it('shows create-vs-existing result copy', () => {
    setup({ verifyStatus: { ok: true, willCreate: true } });
    expect(screen.getByText(/will be created/i)).toBeInTheDocument();
  });

  it('lets commas through so multiple values can be typed', async () => {
    const props = setup();
    const field = screen.getByLabelText(/values/i);
    await userEvent.type(field, 'a, b, ,c');
    expect(props.onValuesChange).toHaveBeenLastCalledWith(['a', 'b', 'c']);
    expect(field).toHaveValue('a, b, ,c');
  });

  it('caps values at 5', async () => {
    const props = setup();
    await userEvent.type(screen.getByLabelText(/values/i), 'a,b,c,d,e,f,g');
    expect(props.onValuesChange).toHaveBeenLastCalledWith([
      'a',
      'b',
      'c',
      'd',
      'e',
    ]);
  });

  it('shows a verify error alert', () => {
    setup({ verifyStatus: { ok: false, message: 'Nope' } });
    expect(screen.getByRole('alert')).toHaveTextContent('Nope');
  });
});
