import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import type { WorkspaceInspectFound } from '../../lib/daemon-api';

import { ResumeCard } from './ResumeCard';

const PREVIEW: WorkspaceInspectFound = {
  found: true,
  companyName: 'Northwind Research',
  mission: 'Continuous equity research',
  values: ['cite sources'],
  orchestrator: {
    name: 'Anima',
    bio: 'Keeps the team moving.',
    provider: 'moonshot',
    model: 'kimi-k2',
  },
  workers: [
    {
      name: 'Scout',
      bio: 'Finds things.',
      provider: 'moonshot',
      model: 'kimi-k2',
    },
    {
      name: 'Scribe',
      bio: 'Writes things.',
      provider: 'moonshot',
      model: 'kimi-k2',
    },
  ],
  providerAvailable: true,
};

function setup(overrides: Partial<Parameters<typeof ResumeCard>[0]> = {}) {
  const props = {
    preview: PREVIEW,
    rootPath: 'C:\\anima\\northwind',
    resuming: false,
    resumeError: null,
    onResume: vi.fn(),
    onSetupFresh: vi.fn(),
    ...overrides,
  };
  render(<ResumeCard {...props} />);
  return props;
}

describe('ResumeCard', () => {
  it('renders company, mission, folder, and the agent roster', () => {
    setup();
    expect(screen.getByText('Northwind Research')).toBeInTheDocument();
    expect(screen.getByText('Continuous equity research')).toBeInTheDocument();
    expect(
      screen.getByTitle('C:\\anima\\northwind'),
    ).toBeInTheDocument();
    expect(screen.getByText('Anima')).toBeInTheDocument();
    expect(screen.getByText('Scout')).toBeInTheDocument();
    expect(screen.getByText('Scribe')).toBeInTheDocument();
  });

  it('warns when the provider is unavailable', () => {
    setup({ preview: { ...PREVIEW, providerAvailable: false } });
    expect(screen.getByText(/offline|not configured/i)).toBeInTheDocument();
  });

  it('omits the warning when the provider is available', () => {
    setup();
    expect(screen.queryByText(/offline|not configured/i)).toBeNull();
  });

  it('emits onResume and onSetupFresh', async () => {
    const props = setup();
    await userEvent.click(
      screen.getByRole('button', { name: /resume workspace/i }),
    );
    expect(props.onResume).toHaveBeenCalledTimes(1);
    expect(props.onSetupFresh).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole('button', { name: /set up fresh/i }),
    );
    expect(props.onSetupFresh).toHaveBeenCalledTimes(1);
    expect(props.onResume).toHaveBeenCalledTimes(1);
  });

  it('disables resume while resuming and shows errors', () => {
    setup({ resuming: true, resumeError: 'boom' });
    expect(
      screen.getByRole('button', { name: /resuming/i }),
    ).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent('boom');
  });
});
