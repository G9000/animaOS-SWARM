import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError, MAX_MEMORY_EDIT_CHARS } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import type { ChatMessage } from '../lib/types';
import { memoryFixture } from '../test/memory';
import {
  SAVE_FAILED,
  useSaveToMemory,
  type SaveOutcome,
  type SaveToMemoryTarget,
} from './useSaveToMemory';

const target: SaveToMemoryTarget = {
  agentId: 'agent-main',
  agentName: 'Nova',
  sessionId: 'room-1',
};

function message(text: string, id = 'm1'): ChatMessage {
  return { id, role: 'Assistant', content: { text }, created_at_ms: 1 };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSaveToMemory', () => {
  it('sends the message as a Fact with importance 0.75, the saved-from-chat tag, and the session id', async () => {
    const saveMemory = vi
      .spyOn(daemon, 'saveMemory')
      .mockResolvedValue(memoryFixture('mem-1'));
    const { result } = renderHook(() => useSaveToMemory(target));

    let outcome: SaveOutcome | undefined;
    await act(async () => {
      outcome = await result.current.save(message('  Likes tea \n'));
    });

    expect(outcome).toEqual({ kind: 'saved', shortened: false });
    expect(saveMemory).toHaveBeenCalledWith({
      agentId: 'agent-main',
      agentName: 'Nova',
      type: 'fact',
      content: 'Likes tea',
      importance: 0.75,
      tags: ['saved-from-chat'],
      sessionId: 'room-1',
    });
  });

  it('a saved message answers saved again without calling the daemon', async () => {
    const saveMemory = vi
      .spyOn(daemon, 'saveMemory')
      .mockResolvedValue(memoryFixture('mem-1'));
    const { result } = renderHook(() => useSaveToMemory(target));

    await act(async () => {
      await result.current.save(message('Likes tea'));
    });
    let again: SaveOutcome | undefined;
    await act(async () => {
      again = await result.current.save(message('Likes tea'));
    });

    expect(again).toEqual({ kind: 'saved', shortened: false });
    expect(saveMemory).toHaveBeenCalledTimes(1);
  });

  it('two clicks in flight call the daemon once', async () => {
    let finish!: (value: ReturnType<typeof memoryFixture>) => void;
    const saveMemory = vi.spyOn(daemon, 'saveMemory').mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const { result } = renderHook(() => useSaveToMemory(target));

    let first!: Promise<SaveOutcome>;
    let second!: Promise<SaveOutcome>;
    act(() => {
      first = result.current.save(message('Likes tea'));
      second = result.current.save(message('Likes tea'));
    });
    await act(async () => {
      finish(memoryFixture('mem-1'));
      await Promise.all([first, second]);
    });

    expect(saveMemory).toHaveBeenCalledTimes(1);
    expect(await first).toEqual(await second);
  });

  it('a long message is cut to 8,000 characters and says so', async () => {
    const saveMemory = vi
      .spyOn(daemon, 'saveMemory')
      .mockResolvedValue(memoryFixture('mem-1'));
    const { result } = renderHook(() => useSaveToMemory(target));
    // Two UTF-16 units each: the cut counts characters, not units.
    const emoji = String.fromCodePoint(0x1f600);
    const long = emoji.repeat(MAX_MEMORY_EDIT_CHARS + 5);

    let outcome: SaveOutcome | undefined;
    await act(async () => {
      outcome = await result.current.save(message(long));
    });

    expect(outcome).toEqual({ kind: 'saved', shortened: true });
    const sent = saveMemory.mock.calls[0][0].content;
    expect(Array.from(sent)).toHaveLength(MAX_MEMORY_EDIT_CHARS);
    expect(sent).toBe(emoji.repeat(MAX_MEMORY_EDIT_CHARS));
    expect(result.current.savedState('m1')).toEqual({ shortened: true });
  });

  it('a daemon refusal answers its message and can be tried again', async () => {
    const saveMemory = vi
      .spyOn(daemon, 'saveMemory')
      .mockRejectedValueOnce(
        new DaemonHttpError(400, {
          error: 'Memory text must not contain invisible tag characters',
          code: 'MEMORY_TEXT_HIDDEN',
        }),
      )
      .mockResolvedValueOnce(memoryFixture('mem-1'));
    const { result } = renderHook(() => useSaveToMemory(target));

    let first: SaveOutcome | undefined;
    await act(async () => {
      first = await result.current.save(message('Likes tea'));
    });
    expect(first).toEqual({
      kind: 'failed',
      message: 'Memory text must not contain invisible tag characters',
    });
    expect(result.current.savedState('m1')).toBeNull();

    let second: SaveOutcome | undefined;
    await act(async () => {
      second = await result.current.save(message('Likes tea'));
    });
    expect(second).toEqual({ kind: 'saved', shortened: false });
    expect(saveMemory).toHaveBeenCalledTimes(2);
  });

  it('an unreachable daemon answers the generic failure', async () => {
    vi.spyOn(daemon, 'saveMemory').mockRejectedValue(new TypeError('offline'));
    const { result } = renderHook(() => useSaveToMemory(target));

    let outcome: SaveOutcome | undefined;
    await act(async () => {
      outcome = await result.current.save(message('Likes tea'));
    });

    expect(outcome).toEqual({ kind: 'failed', message: SAVE_FAILED });
  });

  it('saved state survives a new render of the hook', async () => {
    vi.spyOn(daemon, 'saveMemory').mockResolvedValue(memoryFixture('mem-1'));
    const { result, rerender } = renderHook(() => useSaveToMemory(target));
    expect(result.current.savedState('m1')).toBeNull();

    await act(async () => {
      await result.current.save(message('Likes tea'));
    });
    const { save, savedState } = result.current;
    rerender();

    expect(result.current.savedState('m1')).toEqual({ shortened: false });
    expect(result.current.savedState('other')).toBeNull();
    // Stable while the target is, so bubbles do not render again.
    expect(result.current.save).toBe(save);
    expect(result.current.savedState).toBe(savedState);
  });

  it('nothing is sent without a target', async () => {
    const saveMemory = vi
      .spyOn(daemon, 'saveMemory')
      .mockResolvedValue(memoryFixture('mem-1'));
    const { result } = renderHook(() => useSaveToMemory(null));

    let outcome: SaveOutcome | undefined;
    await act(async () => {
      outcome = await result.current.save(message('Likes tea'));
    });

    expect(outcome).toEqual({ kind: 'failed', message: SAVE_FAILED });
    expect(saveMemory).not.toHaveBeenCalled();
  });
});
