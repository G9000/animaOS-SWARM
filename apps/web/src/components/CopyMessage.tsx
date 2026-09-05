import { useEffect, useRef, useState } from 'react';

export function CopyMessage({ text }: { text: string }) {
  const [state, setState] = useState<'idle' | 'copied' | 'error'>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <span className="studio-copy-message">
      <button
        type="button"
        aria-label={state === 'copied' ? 'Copied' : 'Copy message'}
        onClick={async () => {
          try {
            await navigator.clipboard.writeText(text);
            setState('copied');
          } catch {
            setState('error');
          }
          clearTimeout(timer.current);
          timer.current = setTimeout(() => setState('idle'), 2500);
        }}
      >
        {state === 'copied' ? '✓ Copied' : 'Copy'}
      </button>
      {state === 'error' && (
        <span role="status">
          Couldn’t copy. Select the text to copy it manually.
        </span>
      )}
    </span>
  );
}
