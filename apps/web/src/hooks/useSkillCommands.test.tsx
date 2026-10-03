import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { daemon } from '../lib/daemon-api';
import { SLASH_COMMANDS } from '../lib/slash-commands';
import { skillFixture } from '../test/skills';
import { useSkillCommands } from './useSkillCommands';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSkillCommands', () => {
  it('adds a command per usable skill and reads again on a skill event', async () => {
    vi.spyOn(daemon, 'listSkills').mockResolvedValue([skillFixture('notes')]);
    const { result, rerender } = renderHook(
      (props: { version: number }) =>
        useSkillCommands({ version: props.version, epoch: 0, enabled: true }),
      { initialProps: { version: 0 } },
    );
    expect(result.current).toEqual(SLASH_COMMANDS);
    await waitFor(() =>
      expect(result.current.map((command) => command.name)).toContain('notes'),
    );

    rerender({ version: 1 });

    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
  });

  it('keeps the built-ins when skills cannot be read or the daemon is offline', async () => {
    const list = vi
      .spyOn(daemon, 'listSkills')
      .mockRejectedValue(new Error('offline'));
    const { result, rerender } = renderHook(
      (props: { enabled: boolean }) =>
        useSkillCommands({ version: 0, epoch: 0, enabled: props.enabled }),
      { initialProps: { enabled: false } },
    );
    expect(list).not.toHaveBeenCalled();
    rerender({ enabled: true });
    await waitFor(() => expect(list).toHaveBeenCalled());
    expect(result.current).toEqual(SLASH_COMMANDS);
  });

  it('an_empty_reload_does_not_re_render', async () => {
    vi.spyOn(daemon, 'listSkills').mockResolvedValue([]);
    let renders = 0;
    renderHook(() => {
      renders += 1;
      return useSkillCommands({ version: 0, epoch: 0, enabled: true });
    });

    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalled());
    await act(async () => {});

    expect(renders).toBe(1);
  });
});
