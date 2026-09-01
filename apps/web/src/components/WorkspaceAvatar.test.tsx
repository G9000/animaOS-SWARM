import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { WorkspaceAvatar } from './WorkspaceAvatar';

const createObjectURL = vi.fn(() => 'blob:workspace-avatar');
const revokeObjectURL = vi.fn();

beforeEach(() => {
  createObjectURL.mockClear();
  revokeObjectURL.mockClear();
  Object.defineProperties(URL, {
    createObjectURL: { configurable: true, value: createObjectURL },
    revokeObjectURL: { configurable: true, value: revokeObjectURL },
  });
});

describe('WorkspaceAvatar', () => {
  it('renders the current orb as an accessible sidebar change control', () => {
    const { container } = render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar={false}
        uploadAvatar={vi.fn()}
      />,
    );

    expect(
      screen.getByRole('button', { name: 'Change workspace avatar' }),
    ).toHaveClass('h-11', 'w-11');
    expect(container.querySelector('.agent-orb-core')).not.toBeNull();
    expect(container.querySelector('img')).toBeNull();
  });

  it('renders the stored avatar as a decorative cover image', () => {
    const { container } = render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar
        uploadAvatar={vi.fn()}
      />,
    );

    const image = container.querySelector('img');
    expect(image).toHaveAttribute('alt', '');
    expect(image).toHaveAttribute('src', '/api/workspace/avatar?v=0');
    expect(image).toHaveClass('object-cover');
  });

  it('opens the picker with pointer, Enter, and Space activation', async () => {
    const user = userEvent.setup();
    render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar={false}
        uploadAvatar={vi.fn()}
      />,
    );
    const button = screen.getByRole('button', {
      name: 'Change workspace avatar',
    });
    const input = screen.getByLabelText('Workspace avatar image file');
    const pickerClick = vi.spyOn(input as HTMLInputElement, 'click');

    await user.click(button);
    button.focus();
    await user.keyboard('{Enter}');
    await user.keyboard(' ');

    expect(pickerClick).toHaveBeenCalledTimes(3);
  });

  it('rejects unsupported and oversized files without uploading', async () => {
    const user = userEvent.setup({ applyAccept: false });
    const uploadAvatar = vi.fn();
    render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar={false}
        uploadAvatar={uploadAvatar}
      />,
    );
    const input = screen.getByLabelText('Workspace avatar image file');

    await user.upload(
      input,
      new File(['gif'], 'avatar.gif', { type: 'image/gif' }),
    );
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Choose a PNG, JPEG, or WebP image.',
    );

    await user.upload(
      input,
      new File([new Uint8Array(5 * 1024 * 1024 + 1)], 'avatar.png', {
        type: 'image/png',
      }),
    );
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Choose an image no larger than 5 MiB.',
    );
    expect(screen.getByRole('alert')).toHaveAttribute('aria-live', 'polite');
    expect(uploadAvatar).not.toHaveBeenCalled();
    expect(createObjectURL).not.toHaveBeenCalled();
  });

  it('previews immediately, announces busy state, and supports repeat replacement', async () => {
    const user = userEvent.setup();
    let finishUpload: (() => void) | undefined;
    const uploadAvatar = vi
      .fn<(file: File) => Promise<void>>()
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            finishUpload = resolve;
          }),
      )
      .mockResolvedValueOnce();
    const { container } = render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar={false}
        uploadAvatar={uploadAvatar}
      />,
    );
    const input = screen.getByLabelText(
      'Workspace avatar image file',
    ) as HTMLInputElement;
    const file = new File(['png'], 'avatar.png', { type: 'image/png' });

    await user.upload(input, file);
    expect(container.querySelector('img')).toHaveAttribute(
      'src',
      'blob:workspace-avatar',
    );
    expect(
      screen.getByRole('button', { name: 'Change workspace avatar' }),
    ).toHaveAttribute('aria-busy', 'true');

    finishUpload?.();
    await waitFor(() =>
      expect(container.querySelector('img')).toHaveAttribute(
        'src',
        '/api/workspace/avatar?v=1',
      ),
    );
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:workspace-avatar');
    expect(input.value).toBe('');

    await user.upload(input, file);
    await waitFor(() => expect(uploadAvatar).toHaveBeenCalledTimes(2));
  });

  it('restores the confirmed image and reports an upload failure', async () => {
    const user = userEvent.setup();
    const uploadAvatar = vi.fn().mockRejectedValue(new Error('disk unavailable'));
    const { container } = render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar
        uploadAvatar={uploadAvatar}
      />,
    );

    await user.upload(
      screen.getByLabelText('Workspace avatar image file'),
      new File(['png'], 'avatar.png', { type: 'image/png' }),
    );

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'disk unavailable',
    );
    expect(container.querySelector('img')).toHaveAttribute(
      'src',
      '/api/workspace/avatar?v=0',
    );
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:workspace-avatar');
  });

  it('falls back after an image error and recovers on the next valid upload', async () => {
    const user = userEvent.setup();
    const uploadAvatar = vi.fn().mockResolvedValue(undefined);
    const { container } = render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar
        uploadAvatar={uploadAvatar}
      />,
    );

    fireEvent.error(container.querySelector('img') as HTMLImageElement);
    expect(container.querySelector('.agent-orb-core')).not.toBeNull();

    await user.upload(
      screen.getByLabelText('Workspace avatar image file'),
      new File(['png'], 'avatar.png', { type: 'image/png' }),
    );
    await waitFor(() =>
      expect(container.querySelector('img')).toHaveAttribute(
        'src',
        '/api/workspace/avatar?v=1',
      ),
    );
  });

  it('returns to the orb when a later daemon refresh reports no avatar', async () => {
    const user = userEvent.setup();
    const uploadAvatar = vi.fn().mockResolvedValue(undefined);
    const { container, rerender } = render(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar={false}
        uploadAvatar={uploadAvatar}
      />,
    );

    await user.upload(
      screen.getByLabelText('Workspace avatar image file'),
      new File(['png'], 'avatar.png', { type: 'image/png' }),
    );
    await waitFor(() => expect(container.querySelector('img')).not.toBeNull());

    rerender(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar
        uploadAvatar={uploadAvatar}
      />,
    );
    rerender(
      <WorkspaceAvatar
        placement="sidebar"
        hasAvatar={false}
        uploadAvatar={uploadAvatar}
      />,
    );

    await waitFor(() => expect(container.querySelector('img')).toBeNull());
    expect(container.querySelector('.agent-orb-core')).not.toBeNull();
  });

});
