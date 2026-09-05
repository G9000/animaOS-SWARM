import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentMemory } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
import { AgentMemoryView } from './AgentProfileDetails';

afterEach(() => vi.restoreAllMocks());

it('does not show a previous agent memory response after switching profiles', async () => {
  let resolveFirst!: (value: { memories: AgentMemory[] }) => void;
  const memory = (content: string): AgentMemory => ({
    id: content,
    agentId: content,
    agentName: content,
    type: 'fact',
    content,
    importance: 1,
    createdAt: 1,
    scope: 'private',
  });
  vi.spyOn(daemon, 'recentAgentMemories').mockImplementation((id) =>
    id === 'first'
      ? new Promise((resolve) => {
          resolveFirst = resolve;
        })
      : Promise.resolve({ memories: [memory('Second agent memory')] }),
  );
  const view = render(<AgentMemoryView agentId="first" />);
  view.rerender(<AgentMemoryView agentId="second" />);
  expect(await screen.findByText('Second agent memory')).toBeVisible();
  await act(async () =>
    resolveFirst({ memories: [memory('First agent memory')] }),
  );
  expect(screen.queryByText('First agent memory')).toBeNull();
  fireEvent.change(screen.getByLabelText('Search agent memory'), {
    target: { value: 'absent' },
  });
  expect(screen.getByText('No matching memories.')).toBeVisible();
});

it('shows a memory failure and retries without claiming the agent has no memories', async () => {
  const fetch = vi
    .spyOn(daemon, 'recentAgentMemories')
    .mockRejectedValueOnce(new Error('Memory unavailable'))
    .mockResolvedValue({ memories: [] });
  render(<AgentMemoryView agentId="first" />);
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Memory unavailable',
  );
  expect(screen.queryByText(/No saved memories yet/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Refresh memory' }));
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  expect(await screen.findByText(/No saved memories yet/)).toBeVisible();
});
