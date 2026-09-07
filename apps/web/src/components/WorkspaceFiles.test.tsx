import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { daemon } from '../lib/daemon-api';
import { WorkspaceFiles } from './WorkspaceFiles';

afterEach(() => vi.restoreAllMocks());
const files = [{ path: 'notes/brief.md', name: 'brief.md', sizeBytes: 120, modifiedAtMs: null }, { path: 'plan.txt', name: 'plan.txt', sizeBytes: 2000, modifiedAtMs: 1000 }];

it('searches real paths and renders escaped, read-only truncated previews', async () => {
  vi.spyOn(daemon, 'listWorkspaceFiles').mockResolvedValue({ files, truncated: true });
  const read = vi.spyOn(daemon, 'readWorkspaceFile').mockResolvedValue({ path: 'notes/brief.md', content: '<script>danger</script>', truncated: true });
  render(<WorkspaceFiles online />);
  expect(await screen.findByRole('button', { name: /notes\/brief.md/ })).toBeVisible();
  fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'notes/' } });
  expect(screen.queryByRole('button', { name: /plan.txt/ })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: /notes\/brief.md/ }));
  expect(await screen.findByText('<script>danger</script>')).toBeVisible();
  expect(read).toHaveBeenCalledWith('notes/brief.md');
  expect(screen.getByText(/preview is truncated/i)).toBeVisible();
  expect(screen.getByText(/file list is truncated/i)).toBeVisible();
});

it('ignores a late preview after selecting another file', async () => {
  vi.spyOn(daemon, 'listWorkspaceFiles').mockResolvedValue({ files, truncated: false });
  let finish!: (value: { path: string; content: string; truncated: boolean }) => void;
  vi.spyOn(daemon, 'readWorkspaceFile').mockImplementation((path) => path === 'plan.txt' ? Promise.resolve({ path, content: 'New preview', truncated: false }) : new Promise((resolve) => { finish = resolve; }));
  render(<WorkspaceFiles online />);
  fireEvent.click(await screen.findByRole('button', { name: /notes\/brief.md/ }));
  fireEvent.click(screen.getByRole('button', { name: /plan.txt/ }));
  expect(await screen.findByText('New preview')).toBeVisible();
  await act(async () => finish({ path: 'notes/brief.md', content: 'Old preview', truncated: false }));
  expect(screen.queryByText('Old preview')).not.toBeInTheDocument();
});

it('avoids requests offline and recovers from listing failure', async () => {
  const list = vi.spyOn(daemon, 'listWorkspaceFiles').mockRejectedValueOnce(new Error('Unavailable')).mockResolvedValue({ files: [], truncated: false });
  const { rerender } = render(<WorkspaceFiles online={false} />);
  expect(screen.getByText(/connect to the daemon/i)).toBeVisible();
  expect(list).not.toHaveBeenCalled();
  rerender(<WorkspaceFiles online />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Unavailable');
  fireEvent.click(screen.getByRole('button', { name: 'Refresh files' }));
  expect(await screen.findByText('No workspace files yet.')).toBeVisible();
});
