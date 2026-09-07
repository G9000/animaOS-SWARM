import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { DaemonCapabilities } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
import { WorkspaceCapabilities } from './WorkspaceCapabilities';

vi.mock('../lib/daemon-api', () => ({ daemon: { capabilities: vi.fn() } }));
afterEach(() => vi.resetAllMocks());

const inventory = {
  schemaVersion: 1 as const,
  tools: [
    {
      name: 'read_file',
      description: 'Read workspace files',
      category: 'workspace' as const,
      requirements: ['Workspace folder'],
    },
  ],
  persistence: {
    controlPlane: 'durable',
    memory: 'durable',
    executionJournal: false,
  },
  extensions: [
    {
      id: 'camera',
      label: 'Camera',
      status: 'planned' as const,
      description: 'Future camera integration.',
    },
  ],
  limitations: ['Interrupted runs require review.'],
};

it('distinguishes loading, registered tools, authority and unavailable modules', async () => {
  let finish!: (value: typeof inventory) => void;
  vi.mocked(daemon.capabilities).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  render(<WorkspaceCapabilities online />);
  expect(screen.getByRole('status')).toHaveTextContent('Loading capabilities');
  await act(async () => finish(inventory));
  expect(screen.getByText('read_file')).toBeVisible();
  expect(screen.getByText(/registration does not grant/i)).toBeVisible();
  expect(screen.getByText('Workspace folder')).toBeVisible();
  expect(screen.getByText('Planned · unavailable')).toBeVisible();
  expect(
    screen.getByText(/automatic run continuation is not available/i),
  ).toBeVisible();
  expect(
    screen.queryByRole('button', { name: /enable camera/i }),
  ).not.toBeInTheDocument();
  fireEvent.change(
    screen.getByRole('searchbox', { name: 'Search capabilities' }),
    { target: { value: 'missing tool' } },
  );
  expect(screen.getByText('No tools match your search.')).toBeVisible();
  expect(screen.queryByText('read_file')).not.toBeInTheDocument();
  expect(screen.getByText('Camera')).toBeVisible();
});

it('does not request inventory offline and allows retry after failure', async () => {
  vi.mocked(daemon.capabilities)
    .mockRejectedValueOnce(new Error('Daemon unavailable'))
    .mockResolvedValue(inventory);
  const { rerender } = render(<WorkspaceCapabilities online={false} />);
  expect(screen.getByText(/connect to the daemon/i)).toBeVisible();
  expect(daemon.capabilities).not.toHaveBeenCalled();
  rerender(<WorkspaceCapabilities online />);
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Daemon unavailable',
  );
  fireEvent.click(screen.getByRole('button', { name: 'Refresh capabilities' }));
  expect(await screen.findByText('read_file')).toBeVisible();
});

it('discards an in-flight response when the daemon disconnects', async () => {
  let finish!: (value: typeof inventory) => void;
  vi.mocked(daemon.capabilities).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { rerender } = render(<WorkspaceCapabilities online />);
  rerender(<WorkspaceCapabilities online={false} />);
  await act(async () => finish(inventory));
  expect(screen.queryByText('read_file')).not.toBeInTheDocument();
  expect(screen.getByText(/connect to the daemon/i)).toBeVisible();
});

it('keeps tools from newer daemon categories visible and searchable', async () => {
  vi.mocked(daemon.capabilities).mockResolvedValue({
    ...inventory,
    tools: [
      ...inventory.tools,
      {
        name: 'future_tool',
        description: 'A newer tool',
        category: 'new-category',
        requirements: ['Additional access'],
      },
    ],
  } as unknown as DaemonCapabilities);
  render(<WorkspaceCapabilities online />);
  expect(await screen.findByText('future_tool')).toBeVisible();
  expect(screen.getByRole('region', { name: 'Other tools' })).toHaveTextContent(
    'Additional access',
  );
  fireEvent.change(screen.getByRole('searchbox'), {
    target: { value: 'new-category' },
  });
  expect(screen.getByText('future_tool')).toBeVisible();
  expect(screen.queryByText('read_file')).not.toBeInTheDocument();
});
