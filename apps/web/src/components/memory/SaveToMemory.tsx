import { useEffect, useRef, useState } from 'react';

import {
  SAVE_SHORTENED_NOTE,
  SAVE_TO_MEMORY_LABEL,
  SAVED_TO_MEMORY_LABEL,
  SAVING_LABEL,
  type SaveOutcome,
  type SavedState,
} from '../../hooks/useSaveToMemory';
import type { ChatMessage } from '../../lib/types';

type State =
  | { kind: 'idle' }
  | { kind: 'saving' }
  | { kind: 'saved'; shortened: boolean }
  | { kind: 'failed'; message: string };

/** A message's Save to memory button, next to Copy (spec §15.2). A message
 *  already saved shows it as saved after a remount, through `saved`. */
export function SaveToMemory({
  message,
  save,
  saved,
}: {
  message: ChatMessage;
  save: (message: ChatMessage) => Promise<SaveOutcome>;
  saved: SavedState | null;
}) {
  const [state, setState] = useState<State>({ kind: 'idle' });
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const shown: State =
    state.kind === 'saved' || state.kind === 'saving'
      ? state
      : saved
        ? { kind: 'saved', shortened: saved.shortened }
        : state;
  const busy = shown.kind === 'saving' || shown.kind === 'saved';
  return (
    <span className="studio-copy-message studio-save-memory">
      <button
        type="button"
        disabled={busy}
        onClick={() => {
          setState({ kind: 'saving' });
          void save(message).then((outcome) => {
            if (!mounted.current) return;
            setState(
              outcome.kind === 'saved'
                ? { kind: 'saved', shortened: outcome.shortened }
                : { kind: 'failed', message: outcome.message },
            );
          });
        }}
      >
        {shown.kind === 'saving'
          ? SAVING_LABEL
          : shown.kind === 'saved'
            ? SAVED_TO_MEMORY_LABEL
            : SAVE_TO_MEMORY_LABEL}
      </button>
      <span role="status" data-failed={shown.kind === 'failed' || undefined}>
        {shown.kind === 'failed'
          ? shown.message
          : shown.kind === 'saved' && shown.shortened
            ? SAVE_SHORTENED_NOTE
            : null}
      </span>
    </span>
  );
}
