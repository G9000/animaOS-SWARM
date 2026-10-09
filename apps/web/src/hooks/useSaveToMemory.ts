import { useCallback, useRef } from 'react';
import { DaemonHttpError, MAX_MEMORY_EDIT_CHARS } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import type { ChatMessage } from '../lib/types';

export const SAVE_TO_MEMORY_LABEL = 'Save to memory';
export const SAVED_TO_MEMORY_LABEL = '✓ Saved to memory';
export const SAVING_LABEL = 'Saving…';
export const SAVE_FAILED = 'Couldn’t save that. Try again.';
export const SAVE_SHORTENED_NOTE = 'Saved the first 8,000 characters';
export const SAVED_FROM_CHAT_TAG = 'saved-from-chat';
/** Spec §10: a message the owner chose to keep matters more than most. */
export const SAVED_IMPORTANCE = 0.75;

export type SaveOutcome =
  | { kind: 'saved'; shortened: boolean }
  | { kind: 'failed'; message: string };

export interface SaveToMemoryTarget {
  agentId: string;
  agentName: string;
  sessionId: string;
}

export interface SavedState {
  shortened: boolean;
}

const FAILED: SaveOutcome = { kind: 'failed', message: SAVE_FAILED };

/** The message text as the memory's content: trimmed, and cut to the length
 *  the memory can still be edited at (counted in characters, as the daemon
 *  counts them, so a pair of surrogates is never split). */
function contentOf(message: ChatMessage): {
  content: string;
  shortened: boolean;
} {
  const text = message.content.text.trim();
  if (text.length <= MAX_MEMORY_EDIT_CHARS) {
    return { content: text, shortened: false };
  }
  const characters = Array.from(text);
  if (characters.length <= MAX_MEMORY_EDIT_CHARS) {
    return { content: text, shortened: false };
  }
  return {
    content: characters.slice(0, MAX_MEMORY_EDIT_CHARS).join('').trimEnd(),
    shortened: true,
  };
}

/**
 * Saves a chat message as a Fact (spec §10). One save per message: a saved
 * message answers 'saved' again without calling the daemon, and a save in
 * flight is shared. Both functions keep their identity while the target
 * does, so the transcript's bubbles do not render again when one is saved.
 */
export function useSaveToMemory(target: SaveToMemoryTarget | null): {
  save: (message: ChatMessage) => Promise<SaveOutcome>;
  savedState: (messageId: string) => SavedState | null;
} {
  const saved = useRef(new Map<string, SavedState>());
  const inFlight = useRef(new Map<string, Promise<SaveOutcome>>());
  const agentId = target?.agentId;
  const agentName = target?.agentName;
  const sessionId = target?.sessionId;

  const save = useCallback(
    (message: ChatMessage): Promise<SaveOutcome> => {
      const done = saved.current.get(message.id);
      if (done) {
        return Promise.resolve({ kind: 'saved', shortened: done.shortened });
      }
      const pending = inFlight.current.get(message.id);
      if (pending) return pending;
      if (!agentId || !agentName || !sessionId) return Promise.resolve(FAILED);
      const { content, shortened } = contentOf(message);
      if (!content) return Promise.resolve(FAILED);
      const request = daemon
        .saveMemory({
          agentId,
          agentName,
          type: 'fact',
          content,
          importance: SAVED_IMPORTANCE,
          tags: [SAVED_FROM_CHAT_TAG],
          sessionId,
        })
        .then(
          (): SaveOutcome => {
            saved.current.set(message.id, { shortened });
            return { kind: 'saved', shortened };
          },
          (error: unknown): SaveOutcome =>
            error instanceof DaemonHttpError
              ? { kind: 'failed', message: error.message }
              : FAILED,
        )
        .finally(() => {
          inFlight.current.delete(message.id);
        });
      inFlight.current.set(message.id, request);
      return request;
    },
    [agentId, agentName, sessionId],
  );
  const savedState = useCallback(
    (messageId: string) => saved.current.get(messageId) ?? null,
    [],
  );
  return { save, savedState };
}
