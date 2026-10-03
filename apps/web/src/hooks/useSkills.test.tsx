import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { skillDraftFixture, skillFixture } from '../test/skills';
import { useSkills } from './useSkills';

beforeEach(() => {
  vi.spyOn(daemon, 'listSkills').mockResolvedValue([skillFixture('notes')]);
  vi.spyOn(daemon, 'listSkillDrafts').mockImplementation(async ({ status }) =>
    status === 'pending'
      ? [skillDraftFixture('skd_1')]
      : [skillDraftFixture('skd_0', { status: 'rejected', decidedAtMs: 5 })],
  );
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSkills', () => {
  it('reads skills and drafts, and again when a skill event arrives', async () => {
    const { result, rerender } = renderHook(
      (props: { version: number }) =>
        useSkills({ version: props.version, epoch: 1, enabled: true }),
      { initialProps: { version: 0 } },
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.skills.map((skill) => skill.slug)).toEqual(['notes']);
    expect(result.current.pending.map((draft) => draft.id)).toEqual(['skd_1']);
    expect(result.current.decided.map((draft) => draft.id)).toEqual(['skd_0']);
    expect(daemon.listSkills).toHaveBeenCalledTimes(1);

    rerender({ version: 1 });

    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
  });

  it('reads nothing while disabled and says when skills need a workspace', async () => {
    const { result, rerender } = renderHook(
      (props: { enabled: boolean }) =>
        useSkills({ version: 0, epoch: 0, enabled: props.enabled }),
      { initialProps: { enabled: false } },
    );
    expect(daemon.listSkills).not.toHaveBeenCalled();
    vi.mocked(daemon.listSkills).mockRejectedValue(
      new DaemonHttpError(409, { error: 'Skills need a configured workspace' }),
    );

    rerender({ enabled: true });

    await waitFor(() =>
      expect(result.current.unavailable).toBe(
        'Skills need a configured workspace',
      ),
    );
    expect(result.current.error).toBeNull();
  });

  it('acts through the daemon, refreshes, and reports failures', async () => {
    const approve = vi.spyOn(daemon, 'approveSkillDraft').mockResolvedValue({
      skill: skillFixture('notes'),
      draft: skillDraftFixture('skd_1', { status: 'approved' }),
    });
    vi.spyOn(daemon, 'rejectSkillDraft').mockRejectedValue(
      new DaemonHttpError(409, { error: 'This draft was already decided' }),
    );
    const { result } = renderHook(() =>
      useSkills({ version: 0, epoch: 0, enabled: true }),
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));

    let kept = false;
    await act(async () => {
      kept = await result.current.approveDraft(result.current.pending[0], {
        body: 'Edited',
      });
    });
    expect(kept).toBe(true);
    expect(approve).toHaveBeenCalledWith('skd_1', { body: 'Edited' });
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));

    await act(async () => {
      kept = await result.current.rejectDraft(result.current.pending[0]);
    });
    expect(kept).toBe(false);
    expect(result.current.error).toBe('This draft was already decided');
  });

  it('an_action_answers_after_the_list_was_read_again', async () => {
    vi.spyOn(daemon, 'approveSkillDraft').mockResolvedValue({
      skill: skillFixture('notes'),
      draft: skillDraftFixture('skd_1', { status: 'approved' }),
    });
    const { result } = renderHook(() =>
      useSkills({ version: 0, epoch: 0, enabled: true }),
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));

    let kept = false;
    await act(async () => {
      kept = await result.current.approveDraft(result.current.pending[0]);
    });

    expect(kept).toBe(true);
    expect(daemon.listSkills).toHaveBeenCalledTimes(2);
  });

  it('answers true even when the reload after an action fails', async () => {
    vi.spyOn(daemon, 'setSkillEnabled').mockResolvedValue(
      skillFixture('notes', { enabled: false }),
    );
    const { result } = renderHook(() =>
      useSkills({ version: 0, epoch: 0, enabled: true }),
    );
    await waitFor(() => expect(result.current.loaded).toBe(true));
    vi.mocked(daemon.listSkills).mockRejectedValue(new TypeError('offline'));

    let kept = false;
    await act(async () => {
      kept = await result.current.setEnabled(result.current.skills[0], false);
    });

    expect(kept).toBe(true);
    expect(result.current.error).not.toBeNull();
  });
});
